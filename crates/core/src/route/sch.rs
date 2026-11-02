//! Schematic target: place missing parts on a tidy grid, route nets with an orthogonal A* on the
//! integer grid, fall back to same-name labels for nets that cannot be wired.
//!
//! Routing model. The grid is the bounding region of everything plus a margin. Each cell is
//! either free, a part body (parts expanded by `clearance`), or occupied by wires/pins of
//! one or two nets (two only at a crossing). A path may only end on a cell of its own net (the
//! growing tree: pins and wires already joined), never passes through any other occupied cell
//! except for perpendicular crossings of a straight foreign segment when crossings are allowed.
//! Because occupied cells are never entered, new wires can neither overlap nor touch existing
//! ones except at the intended end points.

use super::nets::{def_for, SrcNet};
use super::{part_index, part_rect, Acc, IdGen, Prefer, Rect, RouteFailure, RouteOptions, Scope};
use crate::model::{Circuit, Dir, Label, PartLookup, Placement, Pt, View, Wire};
use crate::netlist::Netlist;
use crate::transform::{pin_world, to_world};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

pub(crate) fn run(c: &mut Circuit, lib: &dyn PartLookup, opts: &RouteOptions, nets: Vec<SrcNet>, acc: &mut Acc) {
    if opts.place_missing {
        place_missing(c, lib, &nets, opts, acc);
    }
    route(c, lib, opts, &nets, acc);
}

// ---- placement ---------------------------------------------------------------------------

fn placed_rects(c: &Circuit, lib: &dyn PartLookup) -> Vec<Rect> {
    let mut v = Vec::new();
    for p in &c.parts {
        let Some(pl) = p.schematic.as_ref() else { continue };
        let Some(def) = lib.get(&p.ty) else { continue };
        if def.is_board() {
            continue;
        }
        if let Some(vd) = def.schematic.as_ref() {
            v.push(part_rect(pl, vd));
        }
    }
    v
}

/// Size of a part's schematic body in whole grid units (rounded up).
fn body_size(lib: &dyn PartLookup, ty: &str) -> (i32, i32) {
    let vd = lib.get(ty).and_then(|d| d.schematic.as_ref());
    match vd {
        Some(vd) => ((vd.size[0].ceil() as i32).max(1), (vd.size[1].ceil() as i32).max(1)),
        None => (1, 1),
    }
}

fn place_missing(c: &mut Circuit, lib: &dyn PartLookup, nets: &[SrcNet], opts: &RouteOptions, acc: &mut Acc) {
    let todo: Vec<usize> = c
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.schematic.is_none() && def_for(lib, p, View::Schematic).is_some())
        .map(|(i, _)| i)
        .collect();
    if todo.is_empty() {
        return;
    }
    // The gap between bodies: at least 4, wide enough for both clearance zones, and with room
    // for routing channels that grows with the number of nets (a 1-cell channel carries one wire).
    let tracks = (2 + nets.len() as i32 / 6).min(8);
    let margin = (2 * opts.clearance + 2 + tracks).max(4);
    let mut rects = placed_rects(c, lib);
    let idx = part_index(c);

    // Connectivity between parts (shared nets), and the nets of each part.
    let mut adj: HashMap<usize, HashMap<usize, u32>> = HashMap::new();
    let mut part_nets: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ni, n) in nets.iter().enumerate() {
        let mut ps: Vec<usize> = Vec::new();
        for (p, _) in &n.pins {
            if let Some(&i) = idx.get(p) {
                if !ps.contains(&i) {
                    ps.push(i);
                }
            }
        }
        for &p in &ps {
            part_nets.entry(p).or_default().push(ni);
        }
        let mut link = |a: usize, b: usize| {
            *adj.entry(a).or_default().entry(b).or_insert(0) += 1;
            *adj.entry(b).or_default().entry(a).or_insert(0) += 1;
        };
        if ps.len() <= 8 {
            for i in 0..ps.len() {
                for j in i + 1..ps.len() {
                    link(ps[i], ps[j]);
                }
            }
        } else {
            for w in ps.windows(2) {
                link(w[0], w[1]);
            }
        }
    }

    let is_single = |i: usize| lib.get(&c.parts[i].ty).is_some_and(|d| d.placed_pins(View::Schematic).count() == 1);
    let mains: Vec<usize> = todo.iter().copied().filter(|&i| !is_single(i)).collect();
    let singles: Vec<usize> = todo.iter().copied().filter(|&i| is_single(i)).collect();

    // BFS order from the most connected part, so connected parts land next to each other.
    let weight = |i: usize| adj.get(&i).map(|m| m.values().sum::<u32>()).unwrap_or(0);
    let mut visited: HashSet<usize> = HashSet::new();
    let mut order: Vec<usize> = Vec::new();
    loop {
        let Some(start) = mains.iter().copied().filter(|i| !visited.contains(i)).max_by_key(|&i| (weight(i), Reverse(i))) else {
            break;
        };
        visited.insert(start);
        let mut q = VecDeque::from([start]);
        while let Some(p) = q.pop_front() {
            order.push(p);
            let mut nb: Vec<(u32, usize)> = adj
                .get(&p)
                .map(|m| m.iter().map(|(&j, &w)| (w, j)).collect())
                .unwrap_or_default();
            nb.sort_by_key(|&(w, j)| (Reverse(w), j));
            for (_, j) in nb {
                if mains.contains(&j) && visited.insert(j) {
                    q.push_back(j);
                }
            }
        }
    }

    // Grid layout, left to right in rows.
    let total_area: f64 = order
        .iter()
        .chain(singles.iter())
        .map(|&i| {
            let (w, h) = body_size(lib, &c.parts[i].ty);
            ((w + margin) * (h + margin)) as f64
        })
        .sum();
    let width_limit = ((total_area.sqrt() * 1.6).ceil() as i32).max(40);
    let (x_start, y_start) = if rects.is_empty() {
        (4, 4)
    } else {
        (rects.iter().map(|r| r.x0).min().unwrap_or(0), rects.iter().map(|r| r.y1).max().unwrap_or(0) + margin + 2)
    };
    let mut cursor = (x_start, y_start);
    let mut row_h = 0;
    let mut grid_place = |c: &mut Circuit, rects: &mut Vec<Rect>, i: usize, acc: &mut Acc| {
        let (w, h) = body_size(lib, &c.parts[i].ty);
        let (mut cx, mut cy) = cursor;
        loop {
            if cx > x_start && cx + w > x_start + width_limit {
                cx = x_start;
                cy += row_h + margin;
                row_h = 0;
            }
            let r = Rect { x0: cx, y0: cy, x1: cx + w, y1: cy + h };
            if let Some(o) = rects.iter().find(|o| r.overlaps(&o.inflate(margin))) {
                cx = o.x1 + margin;
                continue;
            }
            break;
        }
        let vd = lib.get(&c.parts[i].ty).and_then(|d| d.schematic.as_ref()).expect("schematic view");
        let r0 = part_rect(&Placement::at(0, 0), vd);
        c.parts[i].schematic = Some(Placement::at(cx - r0.x0, cy - r0.y0));
        rects.push(Rect { x0: cx, y0: cy, x1: cx + w, y1: cy + h });
        cursor = (cx + w + margin, cy);
        row_h = row_h.max(h);
        acc.placed.push(c.parts[i].id.clone());
    };
    for &i in &order {
        grid_place(c, &mut rects, i, acc);
    }

    // Single-pin parts (power symbols) go beside the first part they connect to.
    let mut leftovers: Vec<usize> = Vec::new();
    for &i in &singles {
        let pin_id = match lib.get(&c.parts[i].ty).and_then(|d| d.placed_pins(View::Schematic).next()) {
            Some((p, _)) => p.id.clone(),
            None => continue,
        };
        let me = c.parts[i].id.clone();
        let net = nets.iter().find(|n| n.pins.iter().any(|(p, q)| *p == me && *q == pin_id));
        // First other part of the net that is already placed in the schematic.
        let target = net.and_then(|n| {
            n.pins.iter().find_map(|(p, q)| {
                if *p == me {
                    return None;
                }
                let part = c.parts.get(*idx.get(p)?)?;
                let pl = part.schematic.as_ref()?;
                let vd = lib.get(&part.ty)?.schematic.as_ref()?;
                let pos = vd.pins.get(q)?;
                Some(pin_world(pl, pos))
            })
        });
        let Some(tp) = target else {
            leftovers.push(i);
            continue;
        };
        let vd = lib.get(&c.parts[i].ty).and_then(|d| d.schematic.as_ref()).expect("schematic view");
        let r0 = part_rect(&Placement::at(0, 0), vd);
        let local_pin = vd.pins.get(&pin_id).map(|p| p.at).unwrap_or([0, 0]);
        let (w, h) = (r0.x1 - r0.x0, r0.y1 - r0.y0);
        let mut best: Option<(i32, i32, i32)> = None; // (cost, x, y) origin
        let radius = 3 * margin + 12;
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                let ox = tp[0] + dx;
                let oy = tp[1] + dy;
                let r = Rect { x0: ox + r0.x0, y0: oy + r0.y0, x1: ox + r0.x0 + w, y1: oy + r0.y0 + h };
                if rects.iter().any(|o| r.overlaps(&o.inflate(margin))) {
                    continue;
                }
                let pw = [ox + local_pin[0], oy + local_pin[1]];
                let dist = (pw[0] - tp[0]).abs() + (pw[1] - tp[1]).abs();
                let aligned = pw[0] == tp[0] || pw[1] == tp[1];
                let cost = dist + if aligned { 0 } else { 2 };
                let key = (cost, ox, oy);
                if best.is_none_or(|b| key < b) {
                    best = Some(key);
                }
            }
        }
        match best {
            Some((_, ox, oy)) => {
                c.parts[i].schematic = Some(Placement::at(ox, oy));
                rects.push(Rect { x0: ox + r0.x0, y0: oy + r0.y0, x1: ox + r0.x0 + w, y1: oy + r0.y0 + h });
                acc.placed.push(me);
            }
            None => leftovers.push(i),
        }
    }
    for i in leftovers {
        grid_place(c, &mut rects, i, acc);
    }
}

// ---- routing grid ------------------------------------------------------------------------

const NONE: u32 = u32::MAX;
const K_FREE: u8 = 0;
const K_H: u8 = 1;
const K_V: u8 = 2;
const K_OTHER: u8 = 3;
const DX: [i32; 4] = [1, 0, -1, 0];
const DY: [i32; 4] = [0, 1, 0, -1];

/// Immutable part of the grid.
struct Fixed {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    /// Part bodies expanded by the clearance.
    body: Vec<bool>,
    /// Cells leading straight out of a pin through its own clearance zone: the pin index.
    lane: Vec<u32>,
}

impl Fixed {
    fn idx(&self, x: i32, y: i32) -> Option<usize> {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            None
        } else {
            Some(((y - self.y0) * self.w + (x - self.x0)) as usize)
        }
    }

    fn xy(&self, i: usize) -> (i32, i32) {
        let i = i as i32;
        (i % self.w + self.x0, i / self.w + self.y0)
    }

    fn pt(&self, i: usize) -> Pt {
        let (x, y) = self.xy(i);
        [x, y]
    }
}

/// Mutable occupancy, cloned per routing attempt.
#[derive(Clone)]
struct State {
    /// Owner (netlist net id) of the cell, -1 free, -2 blocked label.
    occ1: Vec<i32>,
    /// Second owner at a crossing.
    occ2: Vec<i32>,
    kind: Vec<u8>,
    /// Union-find over netlist net ids (conductors joined by new wires).
    parent: Vec<u32>,
    /// Cells of each conductor root.
    cells: Vec<Vec<u32>>,
}

impl State {
    fn find(&self, mut x: usize) -> usize {
        while self.parent[x] as usize != x {
            x = self.parent[x] as usize;
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) -> usize {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return ra;
        }
        let (big, small) = if self.cells[ra].len() >= self.cells[rb].len() { (ra, rb) } else { (rb, ra) };
        let moved = std::mem::take(&mut self.cells[small]);
        self.cells[big].extend(moved);
        self.parent[small] = big as u32;
        big
    }

    fn mark(&mut self, cell: usize, label: usize, kind: u8) {
        let l = self.find(label);
        if self.occ1[cell] == -1 {
            self.occ1[cell] = l as i32;
            self.kind[cell] = kind;
            self.cells[l].push(cell as u32);
        } else if self.occ1[cell] >= 0 && self.find(self.occ1[cell] as usize) == l {
            self.kind[cell] = K_OTHER;
        } else if self.occ1[cell] >= 0 && self.occ2[cell] == -1 {
            self.occ2[cell] = l as i32;
            self.kind[cell] = K_OTHER;
            self.cells[l].push(cell as u32);
        }
    }
}

struct Scratch {
    dist: Vec<i32>,
    parent: Vec<u32>,
    stamp: Vec<u32>,
    goal: Vec<u32>,
    gen: u32,
    heap: BinaryHeap<Reverse<(i32, u32)>>,
}

impl Scratch {
    fn new(cells: usize) -> Scratch {
        Scratch {
            dist: vec![0; cells * 5],
            parent: vec![NONE; cells * 5],
            stamp: vec![0; cells * 5],
            goal: vec![0; cells],
            gen: 0,
            heap: BinaryHeap::new(),
        }
    }
}

struct Costs {
    bend: i32,
    cross: i32,
    allow_cross: bool,
}

/// Bresenham-style lattice cells of a segment (both ends included).
fn line_cells(a: Pt, b: Pt) -> Vec<Pt> {
    let (mut x, mut y) = (a[0], a[1]);
    let dx = (b[0] - a[0]).abs();
    let dy = -(b[1] - a[1]).abs();
    let sx = if a[0] < b[0] { 1 } else { -1 };
    let sy = if a[1] < b[1] { 1 } else { -1 };
    let mut err = dx + dy;
    let mut out = vec![[x, y]];
    while (x, y) != (b[0], b[1]) {
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        out.push([x, y]);
    }
    out
}

/// One net to route in the current run.
struct Work {
    net: usize,
    /// Initial groups (pins already connected in the target view), as geometry pin indexes.
    groups: Vec<Vec<usize>>,
    /// Netlist net id of each group.
    glabel: Vec<usize>,
    hpwl: i32,
}

struct NetResult {
    paths: Vec<Vec<Pt>>,
    /// Groups connected to the tree (including the tree seed).
    tree: Vec<usize>,
    /// Groups left unconnected with the reason.
    stuck: Vec<(usize, String)>,
}

fn route(c: &mut Circuit, lib: &dyn PartLookup, opts: &RouteOptions, nets: &[SrcNet], acc: &mut Acc) {
    let nl = Netlist::build(c, lib, View::Schematic);
    let geom = &nl.geom;
    let mut pin_idx: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, p) in geom.pins.iter().enumerate() {
        pin_idx.entry((p.part.as_str(), p.pin.as_str())).or_insert(i);
    }
    let mut pin_src = vec![-1i32; geom.pins.len()];
    for (ni, n) in nets.iter().enumerate() {
        for (p, q) in &n.pins {
            if let Some(&i) = pin_idx.get(&(p.as_str(), q.as_str())) {
                pin_src[i] = ni as i32;
            }
        }
    }

    // Which nets need work.
    let mut works: Vec<Work> = Vec::new();
    for (ni, n) in nets.iter().enumerate() {
        let pins: Vec<usize> =
            n.pins.iter().filter_map(|(p, q)| pin_idx.get(&(p.as_str(), q.as_str())).copied()).collect();
        if pins.len() < 2 {
            continue;
        }
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut glabel: Vec<usize> = Vec::new();
        for &p in &pins {
            let l = nl.pin_net_idx[p];
            match glabel.iter().position(|&x| x == l) {
                Some(g) => groups[g].push(p),
                None => {
                    glabel.push(l);
                    groups.push(vec![p]);
                }
            }
        }
        if groups.len() == 1 {
            if !matches!(opts.scope, Scope::Unrouted) && !n.tainted {
                acc.routed += 1;
            }
            continue;
        }
        let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
        for &p in &pins {
            let at = geom.pins[p].at;
            for k in 0..2 {
                lo[k] = lo[k].min(at[k]);
                hi[k] = hi[k].max(at[k]);
            }
        }
        works.push(Work { net: ni, groups, glabel, hpwl: (hi[0] - lo[0]) + (hi[1] - lo[1]) });
    }
    if works.is_empty() {
        return;
    }

    // Part rectangles and region.
    let clearance = opts.clearance.max(0);
    let mut prect: HashMap<&str, Rect> = HashMap::new();
    let mut rects: Vec<Rect> = Vec::new();
    for p in &c.parts {
        let Some(pl) = p.schematic.as_ref() else { continue };
        let Some(def) = lib.get(&p.ty) else { continue };
        if def.is_board() {
            continue;
        }
        let Some(vd) = def.schematic.as_ref() else { continue };
        let r = part_rect(pl, vd).inflate(clearance);
        prect.insert(p.id.as_str(), r);
        rects.push(r);
    }
    let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
    let mut grow = |p: Pt| {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    };
    for r in &rects {
        grow([r.x0, r.y0]);
        grow([r.x1, r.y1]);
    }
    for p in &geom.pins {
        grow(p.at);
    }
    for pts in &geom.wire_pts {
        for p in pts {
            grow(*p);
        }
    }
    for l in &c.labels {
        grow(l.at);
    }
    let margin = 8 + clearance;
    let (x0, y0) = (lo[0] - margin, lo[1] - margin);
    let (w, h) = (hi[0] - lo[0] + 2 * margin + 1, hi[1] - lo[1] + 2 * margin + 1);
    let ncell = (w as i64 * h as i64) as usize;
    if ncell > 40_000_000 {
        for wk in &works {
            acc.failures.push(RouteFailure { net: nets[wk.net].display(), reason: "layout too large to route".into() });
        }
        return;
    }
    let mut fx = Fixed { x0, y0, w, h, body: vec![false; ncell], lane: vec![NONE; ncell] };
    for r in &rects {
        for y in r.y0..=r.y1 {
            for x in r.x0..=r.x1 {
                if let Some(i) = fx.idx(x, y) {
                    fx.body[i] = true;
                }
            }
        }
    }
    // Lanes: the cells straight out of a pin until the clearance zone of its own part ends.
    let parts_by_id = part_index(c);
    for (pi, pin) in geom.pins.iter().enumerate() {
        let Some(&part_i) = parts_by_id.get(&pin.part) else { continue };
        let part = &c.parts[part_i];
        let (Some(pl), Some(def)) = (part.schematic.as_ref(), lib.get(&part.ty)) else { continue };
        let Some(pos) = def.schematic.as_ref().and_then(|v| v.pins.get(&pin.pin)) else { continue };
        let Some(rect) = prect.get(pin.part.as_str()) else { continue };
        let dv = match pos.dir {
            Dir::Left => [-1.0, 0.0],
            Dir::Right => [1.0, 0.0],
            Dir::Up => [0.0, -1.0],
            Dir::Down => [0.0, 1.0],
        };
        let wv = to_world(&Placement { at: [0, 0], rot: pl.rot, mirror: pl.mirror }, dv);
        let (dx, dy) = (wv[0].round() as i32, wv[1].round() as i32);
        if pin_src[pi] < 0 {
            continue;
        }
        let mut cells: Vec<usize> = Vec::new();
        let mut exited = false;
        for k in 1..=(clearance + 3) {
            let p = [pin.at[0] + dx * k, pin.at[1] + dy * k];
            if let Some(i) = fx.idx(p[0], p[1]) {
                cells.push(i);
            }
            if !rect.contains(p) {
                // The first cell outside the zone is the pin's escape cell: reserved for its net
                // so foreign wires cannot wall the pin in.
                exited = true;
                break;
            }
        }
        if exited {
            for i in cells {
                if fx.lane[i] == NONE {
                    fx.lane[i] = pi as u32;
                }
            }
        }
    }

    // Occupancy: pins, existing wires, loose labels.
    let nlab = nl.nets.len();
    let mut base = State {
        occ1: vec![-1; ncell],
        occ2: vec![-1; ncell],
        kind: vec![K_FREE; ncell],
        parent: (0..nlab as u32).collect(),
        cells: vec![Vec::new(); nlab],
    };
    for (i, p) in geom.pins.iter().enumerate() {
        if let Some(cell) = fx.idx(p.at[0], p.at[1]) {
            base.mark(cell, nl.pin_net_idx[i], K_OTHER);
        }
    }
    for (wi, pts) in geom.wire_pts.iter().enumerate() {
        let label = nl.wire_net_idx[wi];
        for pair in pts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a == b {
                continue;
            }
            let ortho = a[0] == b[0] || a[1] == b[1];
            let cells = line_cells(a, b);
            let n = cells.len();
            for (k, p) in cells.iter().enumerate() {
                let Some(cell) = fx.idx(p[0], p[1]) else { continue };
                let kind = if !ortho || k == 0 || k == n - 1 {
                    K_OTHER
                } else if a[1] == b[1] {
                    K_H
                } else {
                    K_V
                };
                base.mark(cell, label, kind);
            }
        }
        // Isolated points (single-point wires) still occupy their cell.
        for p in pts {
            if let Some(cell) = fx.idx(p[0], p[1]) {
                base.mark(cell, label, K_OTHER);
            }
        }
    }
    for l in &c.labels {
        if let Some(cell) = fx.idx(l.at[0], l.at[1]) {
            if base.occ1[cell] == -1 {
                base.occ1[cell] = -2;
                base.kind[cell] = K_OTHER;
            }
        }
    }

    // Order nets: short ones first.
    works.sort_by_key(|wk| (wk.hpwl, wk.net));
    let costs = Costs {
        bend: match opts.prefer {
            Prefer::Bends => 6,
            Prefer::Length => 1,
        },
        cross: 8,
        allow_cross: opts.allow_crossings,
    };
    let mut scratch = Scratch::new(ncell);
    // Failed nets are promoted for the next attempt (history of failures decides the order,
    // shorter nets first among equals).
    let mut order: Vec<usize> = (0..works.len()).collect();
    let mut priority = vec![0u32; works.len()];
    let mut best: Option<(usize, Vec<NetResult>)> = None;
    for _attempt in 0..8 {
        let mut st = base.clone();
        let mut results: Vec<Option<NetResult>> = (0..works.len()).map(|_| None).collect();
        for &wi in &order {
            let r = route_net(&fx, &mut st, &mut scratch, &costs, &works[wi], &pin_src, geom, nets);
            results[wi] = Some(r);
        }
        let results: Vec<NetResult> = results.into_iter().map(|r| r.expect("routed")).collect();
        let failed: Vec<usize> = (0..works.len()).filter(|&i| !results[i].stuck.is_empty()).collect();
        let nfail = failed.len();
        if best.as_ref().is_none_or(|(n, _)| nfail < *n) {
            best = Some((nfail, results));
        }
        if nfail == 0 {
            break;
        }
        for &f in &failed {
            priority[f] += 1;
        }
        order.sort_by_key(|&i| (Reverse(priority[i]), i));
    }
    let (_, results) = best.expect("at least one attempt");

    // Emit wires in the order of the nets, then labels / failures for stuck nets.
    let mut ids = IdGen::new(c);
    let mut existing_names: HashSet<String> = c.labels.iter().map(|l| l.name.clone()).collect();
    existing_names.extend(nets.iter().filter_map(|n| n.name.clone()));
    let mut taken: HashMap<String, usize> = HashMap::new();
    let label_nets: Vec<(String, Option<usize>)> = c.labels.iter().map(|l| (l.name.clone(), nl.net_at(l.at))).collect();
    let mut new_labels: Vec<Label> = Vec::new();
    for (wk, res) in works.iter().zip(&results) {
        for pts in &res.paths {
            let id = ids.fresh("w");
            c.wires.push(Wire { id: id.clone(), view: View::Schematic, pts: pts.clone(), color: None });
            acc.wires_added.push(id);
        }
        let net = &nets[wk.net];
        if res.stuck.is_empty() {
            if !net.tainted {
                acc.routed += 1;
            }
            continue;
        }
        if !opts.label_fallback {
            let reason = res.stuck[0].1.clone();
            acc.failures.push(RouteFailure { net: net.display(), reason });
            continue;
        }
        // Label fallback: same name on the tree and on every stuck group.
        let mut group_ids: Vec<usize> = res.tree.clone();
        group_ids.extend(res.stuck.iter().map(|(g, _)| *g));
        let net_ids: HashSet<usize> = group_ids.iter().map(|&g| wk.glabel[g]).collect();
        let present: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for &g in &group_ids {
                for (n, _) in &nl.labels[wk.glabel[g]] {
                    if !v.contains(n) {
                        v.push(n.clone());
                    }
                }
            }
            v
        };
        let name = if let Some(n) = present.first() {
            n.clone()
        } else if let Some(n) = &net.name {
            n.clone()
        } else {
            let mut k = 1;
            loop {
                let cand = format!("N{k}");
                if !existing_names.contains(&cand) && !taken.contains_key(&cand) {
                    break cand;
                }
                k += 1;
            }
        };
        let foreign = label_nets.iter().any(|(n, at)| *n == name && at.is_some_and(|a| !net_ids.contains(&a)));
        let other_fallback = taken.get(&name).is_some_and(|&k| k != wk.net);
        if present.len() > 1 || foreign || other_fallback {
            let reason = if present.len() > 1 {
                format!("pins carry different labels ({})", present.join(", "))
            } else {
                format!("label name {name} is already used by a different net")
            };
            acc.failures.push(RouteFailure { net: net.display(), reason });
            continue;
        }
        taken.insert(name.clone(), wk.net);
        existing_names.insert(name.clone());
        let has_name = |g: usize| nl.labels[wk.glabel[g]].iter().any(|(n, _)| *n == name);
        // The connected tree needs one label (unless it already carries the name); every stuck
        // group gets its own.
        let mut need: Vec<usize> = Vec::new();
        if !res.tree.iter().any(|&g| has_name(g)) {
            need.push(res.tree[0]);
        }
        need.extend(res.stuck.iter().map(|(g, _)| *g).filter(|&g| !has_name(g)));
        for g in need {
            let pin = wk.groups[g][0];
            let at = geom.pins[pin].at;
            if !new_labels.iter().any(|l| l.at == at && l.name == name) {
                new_labels.push(Label { at, name: name.clone(), color: None });
            }
        }
        if !net.tainted {
            acc.routed += 1;
        }
    }
    c.labels.extend(new_labels);
}

fn manhattan(a: Pt, b: Pt) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs()
}

#[allow(clippy::too_many_arguments)]
fn route_net(
    fx: &Fixed,
    st: &mut State,
    sc: &mut Scratch,
    costs: &Costs,
    wk: &Work,
    pin_src: &[i32],
    geom: &crate::netlist::Geom,
    nets: &[SrcNet],
) -> NetResult {
    let _ = nets;
    // Seed: the group with most pins (first on ties).
    let seed = (0..wk.groups.len()).max_by_key(|&g| (wk.groups[g].len(), Reverse(g))).unwrap_or(0);
    let mut tree = vec![seed];
    let mut remaining: Vec<usize> = (0..wk.groups.len()).filter(|&g| g != seed).collect();
    let mut paths: Vec<Vec<Pt>> = Vec::new();
    let mut reasons: HashMap<usize, String> = HashMap::new();
    let mut failed_now: HashSet<usize> = HashSet::new();
    loop {
        if remaining.is_empty() {
            break;
        }
        let tree_root = st.find(wk.glabel[seed]);
        // Distance of each remaining group to the tree (pins to tree cells).
        let tcells: Vec<Pt> = st.cells[tree_root].iter().map(|&c| fx.pt(c as usize)).collect();
        let mut cand: Vec<(i32, usize)> = remaining
            .iter()
            .filter(|g| !failed_now.contains(g))
            .map(|&g| {
                let d = wk.groups[g]
                    .iter()
                    .map(|&p| tcells.iter().map(|&t| manhattan(geom.pins[p].at, t)).min().unwrap_or(0))
                    .min()
                    .unwrap_or(0);
                (d, g)
            })
            .collect();
        cand.sort();
        let Some(&(_, g)) = cand.first() else { break };
        let start_root = st.find(wk.glabel[g]);
        let starts: Vec<u32> =
            st.cells[start_root].iter().copied().filter(|&c| st.occ2[c as usize] == -1).collect();
        let goals: Vec<u32> = st.cells[tree_root]
            .iter()
            .copied()
            .filter(|&c| st.occ2[c as usize] == -1 && st.occ1[c as usize] >= 0 && st.find(st.occ1[c as usize] as usize) == tree_root)
            .collect();
        let found = if goals.is_empty() || starts.is_empty() {
            None
        } else {
            search(fx, st, sc, costs, &starts, &goals, wk.net as i32, pin_src, (start_root, tree_root))
        };
        match found {
            Some(cells) => {
                let root = st.union(start_root, tree_root);
                // Mark the new wire.
                let n = cells.len();
                let mut pts: Vec<Pt> = Vec::new();
                for (k, &cell) in cells.iter().enumerate() {
                    let p = fx.pt(cell as usize);
                    let (din, dout) = (
                        if k > 0 { Some(dir_of(fx.pt(cells[k - 1] as usize), p)) } else { None },
                        if k + 1 < n { Some(dir_of(p, fx.pt(cells[k + 1] as usize))) } else { None },
                    );
                    let vertex = din.is_none() || dout.is_none() || din != dout;
                    let kind = if vertex {
                        K_OTHER
                    } else if din.is_some_and(|d| d % 2 == 0) {
                        K_H
                    } else {
                        K_V
                    };
                    st.mark(cell as usize, root, kind);
                    if vertex {
                        pts.push(p);
                    }
                }
                paths.push(pts);
                remaining.retain(|&x| x != g);
                tree.push(g);
                failed_now.clear();
            }
            None => {
                let enclosed = starts.iter().all(|&s| {
                    let (x, y) = fx.xy(s as usize);
                    (0..4).all(|d| match fx.idx(x + DX[d], y + DY[d]) {
                        None => true,
                        Some(i) => {
                            let free = st.occ1[i] == -1
                                && if fx.lane[i] != NONE { pin_src[fx.lane[i] as usize] == wk.net as i32 } else { !fx.body[i] };
                            let is_goal = goals.contains(&(i as u32));
                            !free && !is_goal
                        }
                    })
                });
                let pin = &geom.pins[wk.groups[g][0]];
                let reason = if enclosed {
                    format!("pin enclosed: {}.{}", pin.part, pin.pin)
                } else {
                    format!("no path: {}.{}", pin.part, pin.pin)
                };
                reasons.insert(g, reason);
                failed_now.insert(g);
            }
        }
    }
    let stuck = remaining
        .into_iter()
        .map(|g| {
            let r = reasons.remove(&g).unwrap_or_else(|| "no path".to_string());
            (g, r)
        })
        .collect();
    NetResult { paths, tree, stuck }
}

fn dir_of(a: Pt, b: Pt) -> usize {
    if b[0] > a[0] {
        0
    } else if b[1] > a[1] {
        1
    } else if b[0] < a[0] {
        2
    } else {
        3
    }
}

/// Multi-source A* from `starts` to any of `goals`. Returns the cell path (every lattice cell
/// from a start to a goal, endpoints included).
#[allow(clippy::too_many_arguments)]
fn search(
    fx: &Fixed,
    st: &State,
    sc: &mut Scratch,
    costs: &Costs,
    starts: &[u32],
    goals: &[u32],
    cur_net: i32,
    pin_src: &[i32],
    roots: (usize, usize),
) -> Option<Vec<u32>> {
    sc.gen += 1;
    let gen = sc.gen;
    let (mut gx0, mut gy0, mut gx1, mut gy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &g in goals {
        sc.goal[g as usize] = gen;
        let (x, y) = fx.xy(g as usize);
        gx0 = gx0.min(x);
        gx1 = gx1.max(x);
        gy0 = gy0.min(y);
        gy1 = gy1.max(y);
    }
    let heur = |x: i32, y: i32| (gx0 - x).max(0) + (x - gx1).max(0) + (gy0 - y).max(0) + (y - gy1).max(0);
    sc.heap.clear();
    for &s in starts {
        let state = s as usize * 5 + 4;
        sc.stamp[state] = gen;
        sc.dist[state] = 0;
        sc.parent[state] = NONE;
        let (x, y) = fx.xy(s as usize);
        sc.heap.push(Reverse((heur(x, y), state as u32)));
    }
    let free = |i: usize| -> bool {
        st.occ1[i] == -1
            && if fx.lane[i] != NONE { pin_src[fx.lane[i] as usize] == cur_net } else { !fx.body[i] }
    };
    let crossable = |i: usize, d: usize| -> bool {
        let o = st.occ1[i];
        if o < 0 || st.occ2[i] != -1 {
            return false;
        }
        let r = st.find(o as usize);
        if r == roots.0 || r == roots.1 {
            return false;
        }
        match st.kind[i] {
            K_H => d % 2 == 1,
            K_V => d % 2 == 0,
            _ => false,
        }
    };
    // Small penalty for cells that touch a part zone or a foreign wire, so wires keep some room
    // and do not wall in each other's pins.
    let near_foreign = |i: usize| -> bool {
        let (x, y) = fx.xy(i);
        (0..4).any(|d| match fx.idx(x + DX[d], y + DY[d]) {
            None => false,
            Some(j) => {
                if st.occ1[j] >= 0 {
                    let r = st.find(st.occ1[j] as usize);
                    r != roots.0 && r != roots.1
                } else {
                    fx.body[j] && fx.lane[j] == NONE
                }
            }
        })
    };
    let mut found: Option<usize> = None;
    while let Some(Reverse((f, s))) = sc.heap.pop() {
        let s = s as usize;
        let cell = s / 5;
        let d = s % 5;
        let g = sc.dist[s];
        let (x, y) = fx.xy(cell);
        if f != g + heur(x, y) {
            continue;
        }
        if sc.goal[cell] == gen {
            found = Some(s);
            break;
        }
        for nd in 0..4usize {
            if d < 4 && nd == (d + 2) % 4 {
                continue;
            }
            let (x2, y2) = (x + DX[nd], y + DY[nd]);
            let Some(c2) = fx.idx(x2, y2) else { continue };
            let pen = if d < 4 && d != nd { costs.bend } else { 0 };
            let mut target: Option<(usize, i32)> = None;
            if sc.goal[c2] == gen || free(c2) {
                target = Some((c2, 1 + pen));
            } else if costs.allow_cross {
                let (mut cx, mut cy) = (x2, y2);
                let mut k = 0;
                while let Some(ci) = fx.idx(cx, cy) {
                    if !crossable(ci, nd) {
                        break;
                    }
                    k += 1;
                    cx += DX[nd];
                    cy += DY[nd];
                }
                if k >= 1 {
                    if let Some(li) = fx.idx(cx, cy) {
                        if sc.goal[li] == gen || free(li) {
                            target = Some((li, k + 1 + pen + costs.cross));
                        }
                    }
                }
            }
            let Some((tc, mut step)) = target else { continue };
            if sc.goal[tc] != gen && near_foreign(tc) {
                step += 1;
            }
            let ns = tc * 5 + nd;
            let ng = g + step;
            if sc.stamp[ns] != gen || ng < sc.dist[ns] {
                sc.stamp[ns] = gen;
                sc.dist[ns] = ng;
                sc.parent[ns] = s as u32;
                let (tx, ty) = fx.xy(tc);
                sc.heap.push(Reverse((ng + heur(tx, ty), ns as u32)));
            }
        }
    }
    let end = found?;
    // Reconstruct, filling the cells between consecutive states (jumps are straight).
    let mut chain: Vec<usize> = Vec::new();
    let mut s = end;
    loop {
        chain.push(s / 5);
        let p = sc.parent[s];
        if p == NONE {
            break;
        }
        s = p as usize;
    }
    chain.reverse();
    let mut cells: Vec<u32> = vec![chain[0] as u32];
    for w in chain.windows(2) {
        let (a, b) = (fx.pt(w[0]), fx.pt(w[1]));
        let d = dir_of(a, b);
        let (mut x, mut y) = (a[0], a[1]);
        while [x, y] != b {
            x += DX[d];
            y += DY[d];
            cells.push(fx.idx(x, y).expect("in grid") as u32);
        }
    }
    Some(cells)
}
