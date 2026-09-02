//! Boxes and connectors as a CRDT (yrs, the Rust port of Yjs), synced
//! through a sidecar file next to the board's own `.canvas`. A human
//! dragging a box in the TUI and an agent placing one through `--api`
//! both mutate their own local `Doc` and persist its full state; either
//! side picking up the other's file merges rather than overwrites, so
//! two writers working on different boxes (or even different fields of
//! the same box) never have to choose whose change wins the way a
//! whole-file save would.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use yrs::types::ToJson;
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, In, Map, MapPrelim, MapRef, ReadTxn, StateVector, Transact, Update};

/// yrs docs must never share a client id with another active peer, or
/// merges silently corrupt (see `ClientID`'s own doc comment). `Doc::new`
/// picks one via `fastrand`, which seeds itself from little more than
/// the clock and reliably collides between our own short-lived `--api`
/// processes when several launch within the same instant — so this
/// pulls straight from the OS's own CSPRNG instead.
fn random_client_id() -> u64 {
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut buf = [0u8; 8];
        if f.read_exact(&mut buf).is_ok() {
            // Client ids are 53-bit; yrs debug-asserts the high bits are clear.
            return u64::from_le_bytes(buf) & ((1u64 << 53) - 1);
        }
    }
    std::process::id() as u64
}

#[derive(Debug, Clone)]
pub struct NodeFields {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// The node's own content — display text for a text box, a path for
    /// a file box, a URL for a link, a label for a group. Which one
    /// `kind` says.
    pub text: String,
    /// A file node's anchor within its file — `None` for every other
    /// kind, which have no such thing.
    pub subpath: Option<String>,
    pub color: Option<String>,
    pub shape: String,
    /// "text" | "file" | "link" | "group".
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct EdgeFields {
    pub from: String,
    pub to: String,
    pub from_side: Option<String>,
    pub to_side: Option<String>,
    pub from_end: String,
    pub to_end: String,
    pub from_row: Option<i64>,
    pub from_col: Option<i64>,
    pub to_row: Option<i64>,
    pub to_col: Option<i64>,
    pub color: Option<String>,
    pub label: Option<String>,
}

pub struct Collab {
    doc: Doc,
    nodes: MapRef,
    edges: MapRef,
    path: PathBuf,
    mtime: Option<SystemTime>,
}

fn crdt_path(canvas_path: &Path) -> PathBuf {
    let mut s = canvas_path.as_os_str().to_owned();
    s.push(".crdt");
    PathBuf::from(s)
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

impl Collab {
    /// Opens (or creates) the sidecar next to `canvas_path` and merges
    /// in whatever is already there.
    pub fn open(canvas_path: &Path) -> Self {
        let doc = Doc::with_client_id(random_client_id());
        let nodes = doc.get_or_insert_map("nodes");
        let edges = doc.get_or_insert_map("edges");
        let mut collab = Collab { doc, nodes, edges, path: crdt_path(canvas_path), mtime: None };
        collab.pull();
        collab
    }

    fn persist(&mut self) {
        let bytes = self.doc.transact().encode_state_as_update_v1(&StateVector::default());
        if std::fs::write(&self.path, bytes).is_ok() {
            self.mtime = mtime_of(&self.path);
        }
    }

    /// Merges in whatever's on disk now, if it's moved since we last
    /// looked. Safe to call unconditionally every frame — applying a
    /// state we've already fully seen is a no-op, and CRDT merges never
    /// lose a local change that hasn't made it to disk yet.
    pub fn pull(&mut self) -> bool {
        let Some(mtime) = mtime_of(&self.path) else { return false };
        if Some(mtime) == self.mtime {
            return false;
        }
        self.mtime = Some(mtime);
        let Ok(bytes) = std::fs::read(&self.path) else { return false };
        let Ok(update) = Update::decode_v1(&bytes) else { return false };
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update).is_ok()
    }

    /// Upserts a box's full field set as one map, then persists.
    pub fn set_node(&mut self, id: &str, f: &NodeFields) {
        let color: Any = match &f.color {
            Some(c) => Any::String(Arc::from(c.as_str())),
            None => Any::Null,
        };
        let subpath: Any = match &f.subpath {
            Some(sp) => Any::String(Arc::from(sp.as_str())),
            None => Any::Null,
        };
        let entry: [(Arc<str>, In); 9] = [
            (Arc::from("x"), In::Any(Any::BigInt(f.x))),
            (Arc::from("y"), In::Any(Any::BigInt(f.y))),
            (Arc::from("w"), In::Any(Any::BigInt(f.w))),
            (Arc::from("h"), In::Any(Any::BigInt(f.h))),
            (Arc::from("text"), In::Any(Any::String(Arc::from(f.text.as_str())))),
            (Arc::from("subpath"), In::Any(subpath)),
            (Arc::from("color"), In::Any(color)),
            (Arc::from("shape"), In::Any(Any::String(Arc::from(f.shape.as_str())))),
            (Arc::from("kind"), In::Any(Any::String(Arc::from(f.kind.as_str())))),
        ];
        {
            let mut txn = self.doc.transact_mut();
            self.nodes.insert(&mut txn, id, MapPrelim::from(entry));
        }
        self.persist();
    }

    pub fn remove_node(&mut self, id: &str) {
        {
            let mut txn = self.doc.transact_mut();
            self.nodes.remove(&mut txn, id);
        }
        self.persist();
    }

    /// The merged state of every box right now.
    pub fn snapshot(&self) -> Vec<(String, NodeFields)> {
        let txn = self.doc.transact();
        let mut out = Vec::new();
        for (key, value) in self.nodes.iter(&txn) {
            let Any::Map(fields) = value.to_json(&txn) else { continue };
            let get_i64 = |k: &str| fields.get(k).and_then(|v| match v {
                Any::BigInt(n) => Some(*n),
                Any::Number(n) => Some(*n as i64),
                _ => None,
            }).unwrap_or(0);
            let get_str = |k: &str| match fields.get(k) {
                Some(Any::String(s)) => s.to_string(),
                _ => String::new(),
            };
            let color = match fields.get("color") {
                Some(Any::String(s)) => Some(s.to_string()),
                _ => None,
            };
            out.push((
                key.to_string(),
                NodeFields {
                    x: get_i64("x"),
                    y: get_i64("y"),
                    w: get_i64("w"),
                    h: get_i64("h"),
                    text: get_str("text"),
                    subpath: match fields.get("subpath") {
                        Some(Any::String(s)) => Some(s.to_string()),
                        _ => None,
                    },
                    color,
                    shape: get_str("shape"),
                    kind: get_str("kind"),
                },
            ));
        }
        out
    }

    /// Upserts a connector's full field set as one map, then persists.
    pub fn set_edge(&mut self, id: &str, f: &EdgeFields) {
        let opt_string = |v: &Option<String>| match v {
            Some(s) => Any::String(Arc::from(s.as_str())),
            None => Any::Null,
        };
        let opt_int = |v: Option<i64>| match v {
            Some(n) => Any::BigInt(n),
            None => Any::Null,
        };
        let entry: [(Arc<str>, In); 12] = [
            (Arc::from("from"), In::Any(Any::String(Arc::from(f.from.as_str())))),
            (Arc::from("to"), In::Any(Any::String(Arc::from(f.to.as_str())))),
            (Arc::from("from_side"), In::Any(opt_string(&f.from_side))),
            (Arc::from("to_side"), In::Any(opt_string(&f.to_side))),
            (Arc::from("from_end"), In::Any(Any::String(Arc::from(f.from_end.as_str())))),
            (Arc::from("to_end"), In::Any(Any::String(Arc::from(f.to_end.as_str())))),
            (Arc::from("from_row"), In::Any(opt_int(f.from_row))),
            (Arc::from("from_col"), In::Any(opt_int(f.from_col))),
            (Arc::from("to_row"), In::Any(opt_int(f.to_row))),
            (Arc::from("to_col"), In::Any(opt_int(f.to_col))),
            (Arc::from("color"), In::Any(opt_string(&f.color))),
            (Arc::from("label"), In::Any(opt_string(&f.label))),
        ];
        {
            let mut txn = self.doc.transact_mut();
            self.edges.insert(&mut txn, id, MapPrelim::from(entry));
        }
        self.persist();
    }

    pub fn remove_edge(&mut self, id: &str) {
        {
            let mut txn = self.doc.transact_mut();
            self.edges.remove(&mut txn, id);
        }
        self.persist();
    }

    /// The merged state of every connector right now.
    pub fn snapshot_edges(&self) -> Vec<(String, EdgeFields)> {
        let txn = self.doc.transact();
        let mut out = Vec::new();
        for (key, value) in self.edges.iter(&txn) {
            let Any::Map(fields) = value.to_json(&txn) else { continue };
            let get_str = |k: &str| match fields.get(k) {
                Some(Any::String(s)) => s.to_string(),
                _ => String::new(),
            };
            let get_opt_str = |k: &str| match fields.get(k) {
                Some(Any::String(s)) => Some(s.to_string()),
                _ => None,
            };
            let get_opt_int = |k: &str| match fields.get(k) {
                Some(Any::BigInt(n)) => Some(*n),
                Some(Any::Number(n)) => Some(*n as i64),
                _ => None,
            };
            out.push((
                key.to_string(),
                EdgeFields {
                    from: get_str("from"),
                    to: get_str("to"),
                    from_side: get_opt_str("from_side"),
                    to_side: get_opt_str("to_side"),
                    from_end: get_str("from_end"),
                    to_end: get_str("to_end"),
                    from_row: get_opt_int("from_row"),
                    from_col: get_opt_int("from_col"),
                    to_row: get_opt_int("to_row"),
                    to_col: get_opt_int("to_col"),
                    color: get_opt_str("color"),
                    label: get_opt_str("label"),
                },
            ));
        }
        out
    }
}
