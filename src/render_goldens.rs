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

#[test]
fn routes_around_a_box_in_the_way() {
    let mut app = board(&[(2, 5, 8, 3, "A"), (22, 5, 10, 3, "mid"), (44, 5, 8, 3, "B")], &[(0, 2)]);
    assert_render(&mut app, r#"
  ┌──────┐            ┌────────┐            ┌──────┐
  │A     │─┐          │mid     │          ┌>│B     │
  └──────┘ │          └────────┘          │ └──────┘
           │                              │
           └──────────────────────────────┘
"#);
}

#[test]
fn fan_out_keeps_the_straight_edge_in_the_middle_and_detours_the_rest() {
    let mut app = board(
        &[(2, 8, 10, 3, "hub"), (30, 1, 10, 3, "a"), (30, 8, 10, 3, "b"), (30, 15, 10, 3, "c")],
        &[(0, 1), (0, 2), (0, 3)],
    );
    assert_render(&mut app, r#"
                              ┌────────┐
                              │a       │
                              └────────┘
                                   ^
             ┌─────────────────────┘
             │
             │
  ┌────────┐─┘                ┌────────┐
  │hub     │─────────────────>│b       │
  └────────┘─┐                └────────┘
             │
             │
             └─────────────────────┐
                                   v
                              ┌────────┐
                              │c       │
                              └────────┘
"#);
}

#[test]
fn fan_in_spreads_across_the_target_and_avoids_siblings() {
    let mut app = board(
        &[(2, 1, 8, 3, "s1"), (20, 1, 8, 3, "s2"), (38, 1, 8, 3, "s3"), (14, 12, 24, 3, "target")],
        &[(0, 3), (1, 3), (2, 3)],
    );
    assert_render(&mut app, r#"
  ┌──────┐          ┌──────┐          ┌──────┐
  │s1    │─┐        │s2    │   ┌──────│s3    │
  └──────┘ │        └──────┘   │      └──────┘
           │            │      │
           │            │      │
           │            │      │
           │            │      │
           │            │      │
           │            │      │
           └────────┐   │      │
                    v   v      v
              ┌──────────────────────┐
              │target                │
              └──────────────────────┘
"#);
}

#[test]
fn forced_left_to_right_sides_wrap_around_the_outside() {
    let mut app = board(&[(20, 2, 8, 3, "A"), (40, 2, 8, 3, "B")], &[(0, 1)]);
    let edge_id = app.canvas.edges[0].id.clone();
    app.dispatch(Request::SetEdgeSides { id: edge_id, from_side: Some("left".into()), to_side: Some("right".into()) }).unwrap();
    assert_render(&mut app, r#"
  ┌──────┐            ┌──────┐
┌─│A     │            │B     │<┐
│ └──────┘            └──────┘ │
│                              │
└──────────────────────────────┘
"#);
}

#[test]
fn stacked_fan_out_uses_separate_corridors_and_marks_crossings() {
    let mut app = board(&[(2, 2, 8, 3, "hub"), (26, 1, 8, 3, "t1"), (26, 7, 8, 3, "t2"), (26, 13, 8, 3, "t3")], &[(0, 1), (0, 2), (0, 3), (3, 1)]);
    assert_render(&mut app, r#"
                          ┌──────┐
  ┌──────┐─┐              │t1    │
  │hub   │─┼─────────────>└──────┘
  └──────┘─┤                 ^
           ├─────────────────┴┬────┐
           │                  v    │
           │              ┌──────┐ │
           │              │t2    │ │
           │              └──────┘ │
           │                       │
           └─────────────────┬┬────┘
                             │v
                          ┌──────┐
                          │t3    │
                          └──────┘
"#);
}
