//! Auto-layout: layered, left to right, the way dependency graphs are
//! usually drawn. The point is the `--api` story — an agent can pour
//! nodes and edges in without computing a single coordinate, then ask
//! for `layout` and get a readable diagram — but the same request sits
//! on a key in the TUI for untangling a board drawn by hand.

use std::collections::HashMap;

use crate::model::{Canvas, NodeKind, ShapeId};

const GAP_X: i32 = 6;
const GAP_Y: i32 = 2;
const COMPONENT_GAP: i32 = 4;

/// New top-left positions for every node the layout touches. Group
/// fences are left alone — they're regions, not graph nodes, and
/// moving one would drag its members through the group-carry logic in
/// ways a layout pass shouldn't.
pub fn layered(canvas: &Canvas) -> Vec<(ShapeId, i32, i32)> {
    let nodes: Vec<&crate::model::Node> =
        canvas.nodes.iter().filter(|n| !matches!(n.kind, NodeKind::Group { .. })).collect();
    if nodes.is_empty() {
        return Vec::new();
    }
    let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for e in &canvas.edges {
        if let (Some(&a), Some(&b)) = (index.get(e.from.as_str()), index.get(e.to.as_str()))
            && a != b
        {
            succs[a].push(b);
            preds[b].push(a);
        }
    }

    // Longest-path ranks by relaxation, capped at V rounds so a cycle
    // settles instead of spinning forever.
    let mut rank = vec![0i32; nodes.len()];
    for _ in 0..nodes.len() {
        let mut changed = false;
        for a in 0..nodes.len() {
            for &b in &succs[a] {
                if rank[b] < rank[a] + 1 {
                    rank[b] = rank[a] + 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Connected components, so unrelated graphs stack instead of
    // interleaving.
    let mut comp = vec![usize::MAX; nodes.len()];
    let mut n_comp = 0;
    for start in 0..nodes.len() {
        if comp[start] != usize::MAX {
            continue;
        }
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            if comp[i] != usize::MAX {
                continue;
            }
            comp[i] = n_comp;
            stack.extend(succs[i].iter().chain(preds[i].iter()));
        }
        n_comp += 1;
    }

    // The whole arrangement starts where the content already is, so
    // layout tidies in place rather than teleporting the board.
    let origin_x = nodes.iter().map(|n| n.rect.x).min().unwrap_or(0);
    let origin_y = nodes.iter().map(|n| n.rect.y).min().unwrap_or(0);

    let mut out = Vec::new();
    let mut comp_y = origin_y;
    for c in 0..n_comp {
        let members: Vec<usize> = (0..nodes.len()).filter(|&i| comp[i] == c).collect();
        // Within each layer, order by the average position of
        // predecessors (one barycenter sweep) so edges mostly run to a
        // neighbour instead of across the whole column.
        let max_rank = members.iter().map(|&i| rank[i]).max().unwrap_or(0);
        let mut layers: Vec<Vec<usize>> = vec![Vec::new(); (max_rank + 1) as usize];
        for &i in &members {
            layers[rank[i] as usize].push(i);
        }
        let mut order: HashMap<usize, usize> = HashMap::new();
        for layer in &layers {
            for (k, &i) in layer.iter().enumerate() {
                order.insert(i, k);
            }
        }
        for layer in &mut layers {
            layer.sort_by_key(|&i| {
                let ps = &preds[i];
                if ps.is_empty() {
                    order.get(&i).copied().unwrap_or(0) as i64 * 100
                } else {
                    (ps.iter().filter_map(|p| order.get(p)).sum::<usize>() as i64 * 100) / ps.len() as i64
                }
            });
            for (k, &i) in layer.iter().enumerate() {
                order.insert(i, k);
            }
        }

        let mut x = origin_x;
        let mut comp_bottom = comp_y;
        for layer in &layers {
            if layer.is_empty() {
                continue;
            }
            let mut y = comp_y;
            let mut col_w = 0i32;
            for &i in layer {
                out.push((nodes[i].id.clone(), x, y));
                y += nodes[i].rect.height as i32 + GAP_Y;
                col_w = col_w.max(nodes[i].rect.width as i32);
            }
            comp_bottom = comp_bottom.max(y - GAP_Y);
            x += col_w + GAP_X;
        }
        comp_y = comp_bottom + COMPONENT_GAP;
    }
    out
}
