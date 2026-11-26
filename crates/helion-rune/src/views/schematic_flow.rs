//! Signal-flow placement and channel routing for one schematic sheet.
//!
//! Nodes go in columns by logic level: a node sits one column right of its
//! latest driver. Feedback edges (found by depth-first search) are left out
//! of the levelling, so a loop through a register still flows left to right
//! and only its closing edge runs backwards. Inside a column nodes are
//! ordered by the average row of their neighbours, which removes most wire
//! crossings. Every column shares one row grid.
//!
//! Wires run only in the channels between columns and in the gaps between
//! rows. Each net gets a track of its own in every channel and gap it
//! uses, and channels and gaps grow to hold their tracks. So no wire is
//! hidden under a node, and two nets never share a segment.
//!
//! The module is pure geometry over indices: the schematic view maps its
//! cells, blocks and ports onto [`End`]s and reads rects and paths back.

use std::collections::{HashMap, HashSet};

/// One end of a connection: an input terminal on the sheet's left edge, a
/// node, or an output terminal on the right edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum End {
    Left(usize),
    Node(usize),
    Right(usize),
}

/// One driver-to-sink connection of net `net`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub net: usize,
    pub from: End,
    pub to: End,
}

/// A laid-out sheet: a rect `[x, y, w, h]` per node and terminal, and one
/// orthogonal path per input edge (same order as the input).
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub nodes: Vec<[f32; 4]>,
    pub left: Vec<[f32; 4]>,
    pub right: Vec<[f32; 4]>,
    pub paths: Vec<Vec<[f32; 2]>>,
    /// Extent from the origin to the right and bottom edges.
    pub size: [f32; 2],
    /// Column of each node (0 is leftmost).
    pub level: Vec<usize>,
}

/// From a channel's edge to its first and last track.
const CH_PAD: f32 = 8.0;
/// Between vertical tracks in a channel.
const TRACK: f32 = 5.0;
/// Smallest gap above a row.
const ROW_GAP: f32 = 8.0;
/// Between horizontal lanes in a row gap.
const LANE: f32 = 4.0;
/// Clear space kept between two nets' segments on one track.
const CLEAR: f32 = 3.0;
/// Clear height kept between two nets' pins that face each other.
const PIN_CLEAR: f32 = 2.5;

/// Strongly connected component per node (Tarjan, without recursion): two
/// nodes share a component exactly when each reaches the other.
fn components(outs: &[Vec<usize>]) -> Vec<usize> {
    const NEW: usize = usize::MAX;
    let n = outs.len();
    let (mut index, mut low, mut comp) = (vec![NEW; n], vec![0usize; n], vec![NEW; n]);
    let (mut next, mut count) = (0usize, 0usize);
    let mut open: Vec<usize> = Vec::new();
    let mut on = vec![false; n];
    let mut call: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if index[start] != NEW {
            continue;
        }
        let mut enter = |u: usize, call: &mut Vec<(usize, usize)>, index: &mut [usize], low: &mut [usize], open: &mut Vec<usize>, on: &mut [bool]| {
            index[u] = next;
            low[u] = next;
            next += 1;
            open.push(u);
            on[u] = true;
            call.push((u, 0));
        };
        enter(start, &mut call, &mut index, &mut low, &mut open, &mut on);
        while let Some(&(u, i)) = call.last() {
            if let Some(&v) = outs[u].get(i) {
                call.last_mut().expect("just read").1 += 1;
                if index[v] == NEW {
                    enter(v, &mut call, &mut index, &mut low, &mut open, &mut on);
                } else if on[v] {
                    low[u] = low[u].min(index[v]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[u]);
                }
                if low[u] == index[u] {
                    while let Some(w) = open.pop() {
                        on[w] = false;
                        comp[w] = count;
                        if w == u {
                            break;
                        }
                    }
                    count += 1;
                }
            }
        }
    }
    comp
}

/// Column per node, and the column count. A loop is cut at a register's
/// output (`seq` marks registers and latches), so its logic flows into the
/// register and only the feedback runs backwards; a loop with no register
/// in it is cut where a depth-first search closes it. A node with no driver
/// at all (a constant) moves next to its first load.
fn levels(n: usize, seq: &[bool], edges: &[Edge]) -> (Vec<usize>, usize) {
    let mut outs: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut has_in = vec![false; n];
    let mut fed_left = vec![false; n];
    for e in edges {
        match (e.from, e.to) {
            (End::Node(a), End::Node(b)) if a != b => {
                outs[a].push(b);
                has_in[b] = true;
            }
            (End::Left(_), End::Node(b)) => fed_left[b] = true,
            _ => {}
        }
    }
    for o in &mut outs {
        o.sort_unstable();
        o.dedup();
    }
    let comp = components(&outs);
    let mut back: HashSet<(usize, usize)> = HashSet::new();
    for u in (0..n).filter(|&u| seq.get(u).copied().unwrap_or(false)) {
        back.extend(outs[u].iter().filter(|&&v| comp[v] == comp[u]).map(|&v| (u, v)));
    }
    // Depth-first search over what is left: an edge into a node still on
    // the stack closes a loop that has no register.
    let mut roots: Vec<usize> = (0..n).collect();
    roots.sort_by_key(|&i| (has_in[i], !fed_left[i], i));
    let mut color = vec![0u8; n];
    let mut post: Vec<usize> = Vec::with_capacity(n);
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for root in roots {
        if color[root] != 0 {
            continue;
        }
        color[root] = 1;
        stack.push((root, 0));
        while let Some(&(u, i)) = stack.last() {
            if let Some(&v) = outs[u].get(i) {
                stack.last_mut().expect("just read").1 += 1;
                if back.contains(&(u, v)) {
                    continue;
                }
                match color[v] {
                    0 => {
                        color[v] = 1;
                        stack.push((v, 0));
                    }
                    1 => {
                        back.insert((u, v));
                    }
                    _ => {}
                }
            } else {
                color[u] = 2;
                post.push(u);
                stack.pop();
            }
        }
    }
    // Reverse postorder is a topological order once the loop edges are out.
    let mut level = vec![0usize; n];
    for &u in post.iter().rev() {
        for &v in &outs[u] {
            if !back.contains(&(u, v)) {
                level[v] = level[v].max(level[u] + 1);
            }
        }
    }
    for u in 0..n {
        if has_in[u] || fed_left[u] {
            continue;
        }
        if let Some(first) = outs[u].iter().filter(|&&v| !back.contains(&(u, v))).map(|&v| level[v]).min() {
            level[u] = first.saturating_sub(1);
        }
    }
    // Close up columns the move above emptied.
    let mut used: Vec<usize> = level.clone();
    used.sort_unstable();
    used.dedup();
    for l in &mut level {
        *l = used.binary_search(l).expect("own level");
    }
    (level, used.len())
}

/// Assign each `(key, lo, hi)` span the lowest track where it keeps `clear`
/// from the spans already there. Returns the track per key and the count.
fn tracks<K: Copy + Eq + std::hash::Hash + Ord>(mut spans: Vec<(K, f32, f32)>, clear: f32) -> (HashMap<K, usize>, usize) {
    spans.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)).then(a.0.cmp(&b.0)));
    let mut ends: Vec<f32> = Vec::new();
    let mut of = HashMap::new();
    for (k, lo, hi) in spans {
        let t = ends.iter().position(|&e| e + clear < lo).unwrap_or(ends.len());
        if t == ends.len() {
            ends.push(hi);
        } else {
            ends[t] = hi;
        }
        of.insert(k, t);
    }
    (of, ends.len())
}

/// Grow the span stored for `k` to cover `lo..=hi`.
fn widen<K: Eq + std::hash::Hash>(m: &mut HashMap<K, (f32, f32)>, k: K, lo: f32, hi: f32) {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    m.entry(k).and_modify(|s| *s = (s.0.min(lo), s.1.max(hi))).or_insert((lo, hi));
}

/// Lay out `sizes.len()` nodes (`[w, h]` each; `seq` marks the registers), `n_left` input terminals and
/// `n_right` output terminals of size `term`, connected by `edges`, with the
/// sheet's top-left corner at `origin`.
pub fn layout(sizes: &[[f32; 2]], seq: &[bool], n_left: usize, n_right: usize, term: [f32; 2], edges: &[Edge], origin: [f32; 2]) -> Sheet {
    let n = sizes.len();
    let (level, ncols) = levels(n, seq, edges);
    let col = |e: End| -> usize {
        // Channel-space column: terminals sit one column outside the nodes.
        match e {
            End::Left(_) => 0,
            End::Node(u) => level[u] + 1,
            End::Right(_) => ncols + 1,
        }
    };

    // ---- order inside columns ----
    let mut ins: Vec<Vec<End>> = vec![Vec::new(); n];
    let mut outs: Vec<Vec<End>> = vec![Vec::new(); n];
    let mut left_loads: Vec<Vec<usize>> = vec![Vec::new(); n_left];
    let mut right_drivers: Vec<Vec<End>> = vec![Vec::new(); n_right];
    for e in edges {
        if let End::Node(b) = e.to {
            ins[b].push(e.from);
        }
        if let End::Node(a) = e.from {
            outs[a].push(e.to);
        }
        if let (End::Left(i), End::Node(b)) = (e.from, e.to) {
            left_loads[i].push(b);
        }
        if let End::Right(j) = e.to {
            right_drivers[j].push(e.from);
        }
    }
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); ncols];
    for u in 0..n {
        cols[level[u]].push(u);
    }
    let mut pos = vec![0f32; n];
    for c in &cols {
        for (r, &u) in c.iter().enumerate() {
            pos[u] = r as f32;
        }
    }
    let mut left_pos: Vec<f32> = (0..n_left).map(|i| i as f32).collect();
    let mut right_pos: Vec<f32> = (0..n_right).map(|j| j as f32).collect();
    let mean = |v: &[f32]| (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32);
    let resort = |items: &mut Vec<usize>, key: &dyn Fn(usize) -> Option<f32>, pos: &mut [f32]| {
        let keyed: HashMap<usize, f32> = items.iter().map(|&u| (u, key(u).unwrap_or(pos[u]))).collect();
        items.sort_by(|a, b| keyed[a].total_cmp(&keyed[b]).then(a.cmp(b)));
        for (r, &u) in items.iter().enumerate() {
            pos[u] = r as f32;
        }
    };
    for sweep in 0..3 {
        let forward = sweep != 1;
        let order: Vec<usize> = if forward { (0..ncols).collect() } else { (0..ncols).rev().collect() };
        for c in order {
            let snapshot = pos.clone();
            let key = |u: usize| {
                let near: Vec<f32> = (if forward { &ins[u] } else { &outs[u] })
                    .iter()
                    .filter_map(|&e| match e {
                        End::Left(i) => forward.then(|| left_pos[i]),
                        End::Right(j) => (!forward).then(|| right_pos[j]),
                        End::Node(v) => ((forward && level[v] < c) || (!forward && level[v] > c)).then(|| snapshot[v]),
                    })
                    .collect();
                mean(&near)
            };
            let mut items = std::mem::take(&mut cols[c]);
            resort(&mut items, &key, &mut pos);
            cols[c] = items;
        }
        if !forward {
            // Inputs follow the loads they feed.
            let mut order: Vec<usize> = (0..n_left).collect();
            let key = |i: usize| mean(&left_loads[i].iter().map(|&u| pos[u]).collect::<Vec<_>>());
            resort(&mut order, &key, &mut left_pos);
        }
    }
    {
        let mut order: Vec<usize> = (0..n_right).collect();
        let key = |j: usize| {
            mean(
                &right_drivers[j]
                    .iter()
                    .map(|&e| match e {
                        End::Left(i) => left_pos[i],
                        End::Node(u) => pos[u],
                        End::Right(k) => k as f32,
                    })
                    .collect::<Vec<_>>(),
            )
        };
        resort(&mut order, &key, &mut right_pos);
    }
    let row = |e: End| -> usize {
        match e {
            End::Left(i) => left_pos[i] as usize,
            End::Node(u) => pos[u] as usize,
            End::Right(j) => right_pos[j] as usize,
        }
    };
    let rows = cols.iter().map(Vec::len).max().unwrap_or(0).max(n_left).max(n_right);

    // ---- connections: one per distinct (net, from, to) ----
    let mut conns: Vec<Edge> = edges.to_vec();
    conns.sort_by_key(|e| (e.net, e.from, e.to));
    conns.dedup();
    // A sink one column right of its driver is reached inside one channel;
    // any other goes over the gap above the sink's row.
    let direct = |e: &Edge| col(e.to) == col(e.from) + 1;

    // ---- lanes in the row gaps, then the row grid ----
    let mut gap_spans: HashMap<(usize, usize), (f32, f32)> = HashMap::new();
    for e in conns.iter().filter(|e| !direct(e)) {
        widen(&mut gap_spans, (row(e.to), e.net), col(e.from) as f32, col(e.to) as f32 - 1.0);
    }
    let mut lane: HashMap<(usize, usize), usize> = HashMap::new();
    let mut lanes = vec![0usize; rows];
    let mut by_row: Vec<Vec<(usize, f32, f32)>> = vec![Vec::new(); rows];
    for (&(r, net), &(lo, hi)) in &gap_spans {
        by_row[r].push((net, lo, hi));
    }
    for (r, spans) in by_row.into_iter().enumerate() {
        let (of, count) = tracks(spans, 0.0);
        lanes[r] = count;
        lane.extend(of.into_iter().map(|(net, t)| ((r, net), t)));
    }
    let mut row_h = vec![0f32; rows];
    for u in 0..n {
        let r = pos[u] as usize;
        row_h[r] = row_h[r].max(sizes[u][1]);
    }
    for r in 0..rows.min(n_left.max(n_right)) {
        row_h[r] = row_h[r].max(term[1]);
    }
    let mut row_top = vec![0f32; rows];
    let mut y = origin[1];
    for r in 0..rows {
        y += ROW_GAP.max(6.0 + LANE * lanes[r] as f32);
        row_top[r] = y;
        y += row_h[r];
    }
    let bottom = y;
    let lane_y = |r: usize, net: usize| row_top[r] - 4.0 - LANE * lane.get(&(r, net)).copied().unwrap_or(0) as f32;
    // Top and height of an end (x comes later).
    let span_y = |e: End| -> (f32, f32) {
        let r = row(e);
        match e {
            End::Node(u) => (row_top[r], sizes[u][1]),
            _ => (row_top[r] + (row_h[r] - term[1]) / 2.0, term[1]),
        }
    };

    // ---- pins: one per net on each side of an end ----
    let mut in_nets: HashMap<End, Vec<(bool, f32, usize)>> = HashMap::new();
    let mut out_nets: HashMap<End, Vec<usize>> = HashMap::new();
    for e in &conns {
        // Wires from the gap above take the top pins, then drivers top to bottom.
        let (top, h) = span_y(e.from);
        let key = if direct(e) { (true, top + h / 2.0) } else { (false, lane.get(&(row(e.to), e.net)).copied().unwrap_or(0) as f32) };
        in_nets.entry(e.to).or_default().push((key.0, key.1, e.net));
        out_nets.entry(e.from).or_default().push(e.net);
    }
    let mut pin_in: HashMap<(End, usize), f32> = HashMap::new();
    for (&end, nets) in &mut in_nets {
        nets.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut seen: Vec<usize> = Vec::new();
        for &(_, _, net) in nets.iter() {
            if !seen.contains(&net) {
                seen.push(net);
            }
        }
        let (top, h) = span_y(end);
        for (i, net) in seen.iter().enumerate() {
            pin_in.insert((end, *net), top + h * (i + 1) as f32 / (seen.len() + 1) as f32);
        }
    }
    // An output pin faces the input pins of the end in the same row one
    // column right. It lines up with its own net's pin there (a straight
    // wire) and keeps clear of the other nets' pins, so a driver's stub and
    // a neighbour's input stub never lie on one line.
    let mut facing: HashMap<(usize, usize), Vec<(usize, f32)>> = HashMap::new();
    for (&(end, net), &y) in &pin_in {
        facing.entry((col(end), row(end))).or_default().push((net, y));
    }
    let mut pin_out: HashMap<(End, usize), f32> = HashMap::new();
    for (&end, nets) in &mut out_nets {
        nets.sort_unstable();
        nets.dedup();
        let (top, h) = span_y(end);
        let across: &[(usize, f32)] = facing.get(&(col(end) + 1, row(end))).map_or(&[], Vec::as_slice);
        let mut taken: Vec<f32> = Vec::new();
        for (j, &net) in nets.iter().enumerate() {
            let inside = |y: f32| y >= top + 2.0 && y <= top + h - 2.0;
            let ideal = across
                .iter()
                .find(|&&(other, y)| other == net && inside(y))
                .map_or(top + h * (j + 1) as f32 / (nets.len() + 1) as f32, |&(_, y)| y);
            let free = |y: f32| inside(y) && across.iter().all(|&(other, at)| other == net || (at - y).abs() >= PIN_CLEAR) && taken.iter().all(|&at| (at - y).abs() >= PIN_CLEAR);
            let y = (0..(2.0 * h) as usize)
                .flat_map(|step| {
                    let d = step as f32 * 0.5;
                    [ideal + d, ideal - d]
                })
                .find(|&y| free(y))
                .unwrap_or(ideal);
            taken.push(y);
            pin_out.insert((end, net), y);
        }
    }

    // ---- tracks in the channels, then the column grid ----
    // Channel k is left of node column k; channel `ncols` is before the outputs.
    let mut ch_spans: HashMap<(usize, usize), (f32, f32)> = HashMap::new();
    for e in &conns {
        let (y0, y1) = (pin_out[&(e.from, e.net)], pin_in[&(e.to, e.net)]);
        if direct(e) {
            widen(&mut ch_spans, (col(e.from), e.net), y0, y1);
        } else {
            let gy = lane_y(row(e.to), e.net);
            widen(&mut ch_spans, (col(e.from), e.net), y0, gy);
            widen(&mut ch_spans, (col(e.to) - 1, e.net), gy, y1);
        }
    }
    let mut track: HashMap<(usize, usize), usize> = HashMap::new();
    let mut ch_w = vec![2.0 * CH_PAD; ncols + 1];
    let mut by_channel: Vec<Vec<(usize, f32, f32)>> = vec![Vec::new(); ncols + 1];
    for (&(k, net), &(lo, hi)) in &ch_spans {
        by_channel[k].push((net, lo, hi));
    }
    for ((k, w), spans) in ch_w.iter_mut().enumerate().zip(by_channel) {
        let (of, count) = tracks(spans, CLEAR);
        *w += TRACK * count.saturating_sub(1) as f32;
        track.extend(of.into_iter().map(|(net, t)| ((k, net), t)));
    }
    let mut col_w = vec![0f32; ncols];
    for u in 0..n {
        col_w[level[u]] = col_w[level[u]].max(sizes[u][0]);
    }
    let mut x = origin[0];
    let left_x = x;
    if n_left > 0 {
        x += term[0];
    }
    let mut ch_x = vec![0f32; ncols + 1];
    let mut col_x = vec![0f32; ncols];
    for k in 0..ncols {
        ch_x[k] = x;
        x += ch_w[k];
        col_x[k] = x;
        x += col_w[k];
    }
    ch_x[ncols] = x;
    x += ch_w[ncols];
    let right_x = x;
    if n_right > 0 {
        x += term[0];
    }
    let track_x = |k: usize, net: usize| ch_x[k] + CH_PAD + TRACK * track.get(&(k, net)).copied().unwrap_or(0) as f32;

    // ---- rects and paths ----
    let rect = |e: End| -> [f32; 4] {
        let (top, h) = span_y(e);
        match e {
            End::Left(_) => [left_x, top, term[0], h],
            End::Node(u) => [col_x[level[u]], top, sizes[u][0], h],
            End::Right(_) => [right_x, top, term[0], h],
        }
    };
    let path = |e: &Edge| -> Vec<[f32; 2]> {
        let (a, b) = (rect(e.from), rect(e.to));
        let (x0, y0) = (a[0] + a[2], pin_out[&(e.from, e.net)]);
        let (x1, y1) = (b[0], pin_in[&(e.to, e.net)]);
        if direct(e) {
            let tx = track_x(col(e.from), e.net);
            vec![[x0, y0], [tx, y0], [tx, y1], [x1, y1]]
        } else {
            let (ta, tb) = (track_x(col(e.from), e.net), track_x(col(e.to) - 1, e.net));
            let gy = lane_y(row(e.to), e.net);
            vec![[x0, y0], [ta, y0], [ta, gy], [tb, gy], [tb, y1], [x1, y1]]
        }
    };
    Sheet {
        nodes: (0..n).map(|u| rect(End::Node(u))).collect(),
        left: (0..n_left).map(|i| rect(End::Left(i))).collect(),
        right: (0..n_right).map(|j| rect(End::Right(j))).collect(),
        paths: edges.iter().map(path).collect(),
        size: [x - origin[0], bottom - origin[1]],
        level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: [f32; 2] = [100.0, 28.0];
    const TERM: [f32; 2] = [80.0, 24.0];

    fn e(net: usize, from: End, to: End) -> Edge {
        Edge { net, from, to }
    }

    /// Whether two axis-parallel segments share more than a point.
    fn overlap(a: ([f32; 2], [f32; 2]), b: ([f32; 2], [f32; 2])) -> bool {
        let vertical = |s: ([f32; 2], [f32; 2])| (s.0[0] - s.1[0]).abs() < 0.01;
        let horizontal = |s: ([f32; 2], [f32; 2])| (s.0[1] - s.1[1]).abs() < 0.01;
        let shared = |a0: f32, a1: f32, b0: f32, b1: f32| a0.min(a1).max(b0.min(b1)) + 0.01 < a0.max(a1).min(b0.max(b1));
        (vertical(a) && vertical(b) && (a.0[0] - b.0[0]).abs() < 0.01 && shared(a.0[1], a.1[1], b.0[1], b.1[1]))
            || (horizontal(a) && horizontal(b) && (a.0[1] - b.0[1]).abs() < 0.01 && shared(a.0[0], a.1[0], b.0[0], b.1[0]))
    }

    /// Whether a segment passes through the inside of a rect.
    fn crosses(s: ([f32; 2], [f32; 2]), r: [f32; 4]) -> bool {
        let (lo_x, hi_x) = (s.0[0].min(s.1[0]), s.0[0].max(s.1[0]));
        let (lo_y, hi_y) = (s.0[1].min(s.1[1]), s.0[1].max(s.1[1]));
        hi_x > r[0] + 0.5 && lo_x < r[0] + r[2] - 0.5 && hi_y > r[1] + 0.5 && lo_y < r[1] + r[3] - 0.5
    }

    fn segments(p: &[[f32; 2]]) -> Vec<([f32; 2], [f32; 2])> {
        p.windows(2).map(|w| (w[0], w[1])).collect()
    }

    /// A counter: clk and a constant feed an adder and a register that loop.
    fn counter() -> (Vec<[f32; 2]>, Vec<Edge>) {
        // nodes: 0 add, 1 reg, 2 const. nets: 0 clk, 1 sum, 2 count, 3 one.
        let edges = vec![
            e(0, End::Left(0), End::Node(1)),
            e(1, End::Node(0), End::Node(1)),
            e(2, End::Node(1), End::Node(0)),
            e(3, End::Node(2), End::Node(0)),
            e(2, End::Node(1), End::Right(0)),
        ];
        (vec![NODE; 3], edges)
    }

    #[test]
    fn a_counter_flows_constant_adder_register_output() {
        let (sizes, edges) = counter();
        let s = layout(&sizes, &[false, true, false], 1, 1, TERM, &edges, [0.0, 0.0]);
        assert!(s.level[2] < s.level[0] && s.level[0] < s.level[1], "const, add, reg left to right: {:?}", s.level);
        assert!(s.left[0][0] < s.nodes[2][0] && s.nodes[1][0] < s.right[0][0], "terminals outside the columns");
        // The register's feedback into the adder is the only wire that ends left of where it starts.
        let backwards: Vec<usize> = (0..edges.len()).filter(|&i| s.paths[i].last().unwrap()[0] < s.paths[i][0][0]).collect();
        assert_eq!(backwards, vec![2], "only the loop edge runs right to left");
    }

    #[test]
    fn wires_start_and_end_on_their_ends_and_stay_orthogonal() {
        let (sizes, edges) = counter();
        let s = layout(&sizes, &[false, true, false], 1, 1, TERM, &edges, [10.0, 20.0]);
        let rect = |end: End| match end {
            End::Left(i) => s.left[i],
            End::Node(u) => s.nodes[u],
            End::Right(j) => s.right[j],
        };
        for (edge, p) in edges.iter().zip(&s.paths) {
            let (a, b) = (rect(edge.from), rect(edge.to));
            let (first, last) = (p[0], *p.last().unwrap());
            assert!((first[0] - (a[0] + a[2])).abs() < 0.01 && first[1] > a[1] && first[1] < a[1] + a[3], "leaves the driver's right side");
            assert!((last[0] - b[0]).abs() < 0.01 && last[1] > b[1] && last[1] < b[1] + b[3], "enters the sink's left side");
            for (p0, p1) in segments(p) {
                assert!((p0[0] - p1[0]).abs() < 0.01 || (p0[1] - p1[1]).abs() < 0.01, "axis-parallel segment");
            }
        }
    }

    /// A deterministic mesh with feedback, fan-out and skipped columns.
    fn mesh() -> (Vec<[f32; 2]>, usize, usize, Vec<Edge>) {
        let n = 40;
        let mut edges = Vec::new();
        let mut net = 0;
        for u in 0..n {
            // Each node drives up to three later nodes and sometimes an earlier one.
            let mut loads = vec![(u * 7 + 3) % n, (u * 11 + 5) % n, (u * 13 + 1) % n];
            loads.sort_unstable();
            loads.dedup();
            for v in loads {
                edges.push(e(net, End::Node(u), End::Node(v)));
            }
            net += 1;
        }
        for i in 0..4 {
            edges.push(e(net, End::Left(i), End::Node(i * 9)));
            edges.push(e(net, End::Left(i), End::Node(i * 9 + 2)));
            net += 1;
        }
        for j in 0..5 {
            edges.push(e(j * 6, End::Node(j * 6), End::Right(j)));
        }
        (vec![NODE; n], 4, 5, edges)
    }

    #[test]
    fn no_wire_runs_under_a_node_and_no_two_nets_share_a_segment() {
        let (sizes, n_left, n_right, edges) = mesh();
        let s = layout(&sizes, &[], n_left, n_right, TERM, &edges, [0.0, 0.0]);
        let rects: Vec<[f32; 4]> = s.nodes.iter().chain(&s.left).chain(&s.right).copied().collect();
        for (i, p) in s.paths.iter().enumerate() {
            for seg in segments(p) {
                for r in &rects {
                    assert!(!crosses(seg, *r), "edge {i} {:?} runs under {r:?}: {seg:?}", edges[i]);
                }
            }
        }
        for i in 0..edges.len() {
            for j in i + 1..edges.len() {
                if edges[i].net == edges[j].net {
                    continue;
                }
                for a in segments(&s.paths[i]) {
                    for b in segments(&s.paths[j]) {
                        assert!(!overlap(a, b), "nets {} and {} share a segment: {a:?} {b:?}", edges[i].net, edges[j].net);
                    }
                }
            }
        }
    }

    #[test]
    fn layout_is_deterministic_and_keeps_nodes_apart() {
        let (sizes, n_left, n_right, edges) = mesh();
        let a = layout(&sizes, &[], n_left, n_right, TERM, &edges, [0.0, 0.0]);
        let b = layout(&sizes, &[], n_left, n_right, TERM, &edges, [0.0, 0.0]);
        assert_eq!(a.nodes, b.nodes);
        assert_eq!(a.paths, b.paths);
        for i in 0..a.nodes.len() {
            for j in i + 1..a.nodes.len() {
                let (p, q) = (a.nodes[i], a.nodes[j]);
                let apart = p[0] + p[2] <= q[0] || q[0] + q[2] <= p[0] || p[1] + p[3] <= q[1] || q[1] + q[3] <= p[1];
                assert!(apart, "nodes {i} and {j} overlap");
            }
        }
        let far = a.nodes.iter().map(|r| r[0] + r[2]).fold(0.0, f32::max);
        assert!(a.size[0] >= far, "the sheet covers its nodes");
    }

    #[test]
    fn a_tall_node_makes_its_row_tall_for_every_column() {
        // Node 0 is a tall block driving node 1; node 2 sits under node 1's column mate.
        let sizes = vec![[190.0, 120.0], NODE, NODE, NODE];
        let edges = vec![e(0, End::Node(0), End::Node(1)), e(1, End::Node(2), End::Node(3))];
        let s = layout(&sizes, &[], 0, 0, TERM, &edges, [0.0, 0.0]);
        assert!(s.nodes[2][1] >= s.nodes[0][1] + 120.0, "the next row starts below the tall node");
        assert!(s.nodes[3][1] >= s.nodes[0][1] + 120.0);
    }

    #[test]
    fn a_shift_register_chains_left_to_right_and_a_pipeline_keeps_its_stages() {
        // Three registers in a row, then logic and a fourth register: no loop, so nothing is cut.
        let edges = vec![
            e(0, End::Left(0), End::Node(0)),
            e(1, End::Node(0), End::Node(1)),
            e(2, End::Node(1), End::Node(2)),
            e(3, End::Node(2), End::Node(3)),
            e(4, End::Node(3), End::Node(4)),
        ];
        let s = layout(&vec![NODE; 5], &[true, true, true, false, true], 1, 0, TERM, &edges, [0.0, 0.0]);
        assert_eq!(s.level, vec![0, 1, 2, 3, 4]);
        for p in &s.paths {
            assert!(p.last().unwrap()[0] > p[0][0], "every wire runs left to right");
        }
    }

    #[test]
    fn a_loop_without_a_register_is_still_cut_once() {
        let edges = vec![e(0, End::Node(0), End::Node(1)), e(1, End::Node(1), End::Node(2)), e(2, End::Node(2), End::Node(0))];
        let s = layout(&vec![NODE; 3], &[], 0, 0, TERM, &edges, [0.0, 0.0]);
        let mut cols = s.level.clone();
        cols.sort_unstable();
        assert_eq!(cols, vec![0, 1, 2], "three nodes in three columns");
        let backwards = s.paths.iter().filter(|p| p.last().unwrap()[0] < p[0][0]).count();
        assert_eq!(backwards, 1, "one closing edge");
    }

    #[test]
    fn a_driver_and_its_sink_in_one_row_join_with_a_straight_wire() {
        let edges = vec![e(0, End::Node(0), End::Node(1))];
        let s = layout(&vec![NODE; 2], &[], 0, 0, TERM, &edges, [0.0, 0.0]);
        let p = &s.paths[0];
        assert!(p.iter().all(|q| (q[1] - p[0][1]).abs() < 0.01), "one height from pin to pin: {p:?}");
    }

    #[test]
    fn a_large_sheet_lays_out_quickly() {
        // 4000 nodes in chains with cross links: the layout runs once per netlist change.
        let n = 4000;
        let mut edges = Vec::new();
        for u in 0..n {
            for v in [u + 1, u + 37, (u * 31 + 7) % n] {
                if v < n && v != u {
                    edges.push(e(u, End::Node(u), End::Node(v)));
                }
            }
        }
        let start = std::time::Instant::now();
        let s = layout(&vec![NODE; n], &[], 0, 0, TERM, &edges, [0.0, 0.0]);
        assert_eq!(s.nodes.len(), n);
        assert!(start.elapsed().as_secs() < 20, "took {:?}", start.elapsed());
    }

    #[test]
    fn an_empty_sheet_has_no_rects() {
        let s = layout(&[], &[], 0, 0, TERM, &[], [0.0, 0.0]);
        assert!(s.nodes.is_empty() && s.paths.is_empty());
    }
}
