//! Boxes as a CRDT (yrs, the Rust port of Yjs), synced through a sidecar
//! file next to the board's own `.canvas`. A human dragging a box in the
//! TUI and an agent placing one through `--api` both mutate their own
//! local `Doc` and persist its full state; either side picking up the
//! other's file merges rather than overwrites, so two writers working
//! on different boxes (or even different fields of the same box) never
//! have to choose whose change wins the way a whole-file save would.
//!
//! Edges aren't part of this yet — they still go through the plain
//! JSON-Canvas reload path, which refuses to clobber unsaved local
//! work rather than merging it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use yrs::types::ToJson;
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, In, Map, MapPrelim, MapRef, ReadTxn, StateVector, Transact, Update};

#[derive(Debug, Clone)]
pub struct NodeFields {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub text: String,
    pub color: Option<String>,
    pub shape: String,
}

pub struct Collab {
    doc: Doc,
    nodes: MapRef,
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
        let doc = Doc::new();
        let nodes = doc.get_or_insert_map("nodes");
        let mut collab = Collab { doc, nodes, path: crdt_path(canvas_path), mtime: None };
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
        let entry: [(Arc<str>, In); 7] = [
            (Arc::from("x"), In::Any(Any::BigInt(f.x))),
            (Arc::from("y"), In::Any(Any::BigInt(f.y))),
            (Arc::from("w"), In::Any(Any::BigInt(f.w))),
            (Arc::from("h"), In::Any(Any::BigInt(f.h))),
            (Arc::from("text"), In::Any(Any::String(Arc::from(f.text.as_str())))),
            (Arc::from("color"), In::Any(color)),
            (Arc::from("shape"), In::Any(Any::String(Arc::from(f.shape.as_str())))),
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
                    color,
                    shape: get_str("shape"),
                },
            ));
        }
        out
    }
}
