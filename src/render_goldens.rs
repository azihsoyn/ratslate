//! Golden renders for connector routing. Each case builds a small board
//! through the same `Request`s `--api` takes and compares `to_ascii`
//! against the expected drawing, so a routing change that alters a
//! shape we already got right fails here with the two pictures side by
//! side. Trailing whitespace on a line is ignored.

use crate::app::{App, Request, Response};

fn board(nodes: &[(i32, i32, u16, u16, &str)], edges: &[(usize, usize)]) -> App {
    let mut app = App::new(None);
    let mut ids = Vec::new();
    for &(x, y, w, h, text) in nodes {
        let id = match app.dispatch(Request::Place { x, y, w: Some(w), h: Some(h) }).unwrap() {
            Response::Placed { id } => id,
            _ => unreachable!(),
        };
        app.dispatch(Request::SetText { id: id.clone(), text: text.to_string() }).unwrap();
        ids.push(id);
    }
    for &(a, b) in edges {
        app.dispatch(Request::Connect { from: ids[a].clone(), to: ids[b].clone() }).unwrap();
    }
    app
}

fn assert_render(app: &mut App, expected: &str) {
    let got = crate::render::to_ascii(app);
    let norm = |s: &str| s.trim_start_matches('\n').lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n").trim_end().to_string();
    let (g, e) = (norm(&got), norm(expected));
    assert!(g == e, "render differs.\n--- expected ---\n{e}\n--- got ---\n{g}\n");
}

#[test]
fn straight_right() {
    let mut app = board(&[(2, 2, 8, 3, "A"), (30, 2, 8, 3, "B")], &[(0, 1)]);
    assert_render(&mut app, r#"
  ┌──────┐                    ┌──────┐
  │A     │───────────────────>│B     │
  └──────┘                    └──────┘
"#);
}

#[test]
fn bend_down_right() {
    let mut app = board(&[(2, 2, 8, 3, "A"), (30, 12, 8, 3, "B")], &[(0, 1)]);
    assert_render(&mut app, r#"
  ┌──────┐
  │A     │────────────────────────┐
  └──────┘                        │
                                  │
                                  │
                                  │
                                  │
                                  │
                                  │
                                  v
                              ┌──────┐
                              │B     │
                              └──────┘
"#);
}

#[test]
fn straight_down() {
    let mut app = board(&[(8, 2, 8, 3, "A"), (2, 12, 8, 3, "B")], &[(0, 1)]);
    assert_render(&mut app, r#"
        ┌──────┐
        │A     │
        └──────┘
         │
         │
         │
         │
         │
         │
         v
  ┌──────┐
  │B     │
  └──────┘
"#);
}

#[test]
fn bidirectional_pair_gets_two_rows() {
    let mut app = board(&[(2, 2, 8, 3, "A"), (30, 2, 8, 3, "B")], &[(0, 1), (1, 0)]);
    assert_render(&mut app, r#"
  ┌──────┐                    ┌──────┐
  │A     │───────────────────>│B     │
  └──────┘<───────────────────└──────┘
"#);
}

#[test]
fn bidirectional_pair_offset_boxes_still_two_rows() {
    let mut app = board(&[(2, 2, 8, 5, "A"), (30, 3, 8, 5, "B")], &[(0, 1), (1, 0)]);
    assert_render(&mut app, r#"
  ┌──────┐
  │A     │                    ┌──────┐
  │      │───────────────────>│B     │
  │      │<───────────────────│      │
  └──────┘                    │      │
                              └──────┘
"#);
}
