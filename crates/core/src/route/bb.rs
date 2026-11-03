//! Breadboard target: put boards in place, place parts onto board holes without creating
//! shorts, then join the pins of every net that ended up in several strips with jumpers.
//!
//! Placement is exhaustive over rotations and board holes. A candidate is valid when every pin of
//! the part lands on a distinct free hole, the part's bounding box overlaps no other non-board
//! part, and no strip would hold two different nets (pins that belong to no net count as nets of
//! their own, so a resistor bridges two strips and a DIP straddles the centre gap).

use super::nets::{can_exist, SrcNet};
use super::{report_unplaced, Acc, IdGen, JumperColor, JumperStyle, RouteFailure, RouteOptions, Scope};
use crate::model::{Circuit, Part, PartLookup, Placement, Pt, View, Wire};
use crate::netlist::Netlist;
use crate::transform::{pin_world, world_bbox};
use std::collections::{BTreeMap, HashMap, HashSet};

const PALETTE: [&str; 8] = ["red", "orange", "yellow", "green", "blue", "purple", "brown", "gray"];

type FRect = ([f64; 2], [f64; 2]);

pub(crate) fn run(c: &mut Circuit, lib: &dyn PartLookup, opts: &RouteOptions, nets: Vec<SrcNet>, acc: &mut Acc) {
    if opts.move_parts && opts.place_missing {
        lift_parts(c, lib, &nets);
    }
    let has_board = ensure_boards(c, lib, opts, acc);
    let pre = connected_before(c, lib, &nets);
    if opts.place_missing {
        place_parts(c, lib, &nets, has_board, acc);
    }
    jumpers(c, lib, opts, &nets, &pre, acc);
}

/// `move_parts`: take the components that appear in the nets off the board so placement starts
/// from scratch. Boards stay where they are. The breadboard wires go too: their ends were on the
/// old pin positions and would be left hanging.
fn lift_parts(c: &mut Circuit, lib: &dyn PartLookup, nets: &[SrcNet]) {
    let in_nets: HashSet<&str> = nets.iter().flat_map(|n| n.pins.iter().map(|(p, _)| p.as_str())).collect();
    let mut any = false;
    for p in &mut c.parts {
        let board = lib.get(&p.ty).is_some_and(|d| d.is_board());
        if !board && p.breadboard.is_some() && in_nets.contains(p.id.as_str()) {
            p.breadboard = None;
            any = true;
        }
    }
    if any {
        c.wires.retain(|w| w.view != View::Breadboard);
    }
}

fn is_board_def(lib: &dyn PartLookup, ty: &str) -> bool {
    lib.get(ty).is_some_and(|d| d.is_board() && d.breadboard.is_some())
}

/// Make sure there is a board in the breadboard view when parts are to be placed. Returns
/// whether a placed board exists afterwards.
fn ensure_boards(c: &mut Circuit, lib: &dyn PartLookup, opts: &RouteOptions, acc: &mut Acc) -> bool {
    if c.parts.iter().any(|p| p.breadboard.is_some() && is_board_def(lib, &p.ty)) {
        return true;
    }
    if !opts.place_missing {
        return false;
    }
    if !c.parts.iter().any(|p| p.breadboard.is_none() && can_exist(lib, p, View::Breadboard)) {
        return false;
    }
    if let Some(i) = c.parts.iter().position(|p| p.breadboard.is_none() && is_board_def(lib, &p.ty)) {
        let ty = c.parts[i].ty.clone();
        let Some((at, id)) = board_origin(c, lib, &ty, &c.parts[i].id.clone(), acc) else { return false };
        c.parts[i].breadboard = Some(Placement::at(at[0], at[1]));
        acc.placed.push(id);
        return true;
    }
    if !is_board_def(lib, "breadboard_half") {
        return false;
    }
    let id = IdGen::new(c).fresh("BB");
    let Some((at, id)) = board_origin(c, lib, "breadboard_half", &id, acc) else { return false };
    c.parts.push(Part { id: id.clone(), ty: "breadboard_half".into(), breadboard: Some(Placement::at(at[0], at[1])), ..Part::default() });
    acc.placed.push(id);
    true
}

/// Where to put a new board of type `ty`. Loose legs already in the breadboard view (parts placed
/// without a board) must not land on its holes, because a strip would then join their nets: the
/// origin (0,0) is used when that is clean, else the board is moved clear of everything placed
/// (right, below, left, above). When no position is clean the board is added at (0,0) only if
/// that causes no new validation error; otherwise a failure is reported and `None` returned.
fn board_origin(c: &Circuit, lib: &dyn PartLookup, ty: &str, id: &str, acc: &mut Acc) -> Option<(Pt, String)> {
    let def = lib.get(ty)?;
    let vd = def.breadboard.as_ref()?;
    let holes: Vec<Pt> = def.placed_pins(View::Breadboard).map(|(_, pos)| pin_world(&Placement::at(0, 0), pos)).collect();
    // Points where something already sits in the breadboard view: legs and jumper ends.
    let nl = Netlist::build(c, lib, View::Breadboard);
    let mut blocked: HashSet<Pt> = nl.geom.pins.iter().map(|p| p.at).collect();
    for pts in &nl.geom.wire_pts {
        blocked.extend(pts.first().copied());
        blocked.extend(pts.last().copied());
    }
    let clean = |o: Pt| !holes.iter().any(|h| blocked.contains(&[h[0] + o[0], h[1] + o[1]]));
    if blocked.is_empty() || clean([0, 0]) {
        return Some(([0, 0], id.to_string()));
    }
    let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
    for p in &blocked {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    for (_, (a, b)) in part_rects(c, lib) {
        for k in 0..2 {
            lo[k] = lo[k].min(a[k].floor() as i32);
            hi[k] = hi[k].max(b[k].ceil() as i32);
        }
    }
    let size = [vd.size[0].ceil() as i32, vd.size[1].ceil() as i32];
    let gap = 2;
    let candidates = [
        [hi[0] + gap, 0],
        [0, hi[1] + gap],
        [lo[0] - gap - size[0], 0],
        [0, lo[1] - gap - size[1]],
        [hi[0] + gap, hi[1] + gap],
    ];
    if let Some(o) = candidates.into_iter().find(|&o| clean(o)) {
        return Some((o, id.to_string()));
    }
    // Last resort: the first position, if it adds no error.
    let before = error_keys(c, lib);
    let mut trial = c.clone();
    trial.parts.push(Part { id: id.to_string(), ty: ty.to_string(), breadboard: Some(Placement::at(0, 0)), ..Part::default() });
    if error_keys(&trial, lib).iter().all(|k| before.contains(k)) {
        return Some(([0, 0], id.to_string()));
    }
    acc.failures.push(RouteFailure {
        net: id.to_string(),
        reason: "no clear place for a board: parts already in the breadboard view would be shorted by its strips".into(),
    });
    None
}

/// The validation errors of a circuit as comparable keys (rule and sorted ids).
fn error_keys(c: &Circuit, lib: &dyn PartLookup) -> Vec<String> {
    crate::validate::validate(c, lib)
        .issues
        .iter()
        .filter(|i| i.severity == crate::model::Severity::Error)
        .map(|i| {
            let mut ids = i.ids.clone();
            ids.sort();
            format!("{} {}", i.rule, ids.join(","))
        })
        .collect()
}

/// Per net: every usable pin is placed and all of them are already one net in the breadboard view.
fn connected_before(c: &Circuit, lib: &dyn PartLookup, nets: &[SrcNet]) -> Vec<bool> {
    let nl = Netlist::build(c, lib, View::Breadboard);
    nets.iter()
        .map(|n| {
            let ids: Vec<Option<usize>> = n.pins.iter().map(|(p, q)| nl.net_of_pin(p, q)).collect();
            ids.iter().all(|i| i.is_some()) && ids.windows(2).all(|w| w[0] == w[1])
        })
        .collect()
}

fn frect_overlap(a: &FRect, b: &FRect) -> bool {
    a.0[0] < b.1[0] && b.0[0] < a.1[0] && a.0[1] < b.1[1] && b.0[1] < a.1[1]
}

/// World bounding boxes of the non-board parts placed in the breadboard view.
fn part_rects(c: &Circuit, lib: &dyn PartLookup) -> Vec<(String, FRect)> {
    let mut v = Vec::new();
    for p in &c.parts {
        let Some(pl) = p.breadboard.as_ref() else { continue };
        let Some(def) = lib.get(&p.ty) else { continue };
        if def.is_board() {
            continue;
        }
        if let Some(vd) = def.breadboard.as_ref() {
            v.push((p.id.clone(), world_bbox(pl, vd)));
        }
    }
    v
}

// ---- placement ---------------------------------------------------------------------------

fn place_parts(c: &mut Circuit, lib: &dyn PartLookup, nets: &[SrcNet], has_board: bool, acc: &mut Acc) {
    let mut todo: Vec<usize> = c
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.breadboard.is_none() && can_exist(lib, p, View::Breadboard))
        .map(|(i, _)| i)
        .collect();
    if todo.is_empty() {
        return;
    }
    if !has_board {
        for &i in &todo {
            acc.failures.push(RouteFailure { net: c.parts[i].id.clone(), reason: "no breadboard available".into() });
        }
        return;
    }
    let nl = Netlist::build(c, lib, View::Breadboard);
    let geom = &nl.geom;

    // Holes (board pins) with their strip, and what is already in use.
    let mut holes: BTreeMap<Pt, usize> = BTreeMap::new();
    let mut occupied: HashSet<Pt> = HashSet::new();
    for (i, p) in geom.pins.iter().enumerate() {
        if geom.pin_board[i] {
            holes.entry(p.at).or_insert(nl.pin_net_idx[i]);
        } else {
            occupied.insert(p.at);
        }
    }
    for pts in &geom.wire_pts {
        if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
            occupied.insert(*a);
            occupied.insert(*b);
        }
    }

    // Net keys: source net index, or a neutral negative key per pin without a net.
    let mut key_map: HashMap<(String, String), i64> = HashMap::new();
    for (ni, n) in nets.iter().enumerate() {
        for (p, q) in &n.pins {
            key_map.insert((p.clone(), q.clone()), ni as i64);
        }
    }
    let mut neutral = 0i64;
    let mut key_of = |part: &str, pin: &str| -> i64 {
        if let Some(&k) = key_map.get(&(part.to_string(), pin.to_string())) {
            return k;
        }
        neutral -= 1;
        key_map.insert((part.to_string(), pin.to_string()), neutral);
        neutral
    };
    let mut class_keys: HashMap<usize, Vec<i64>> = HashMap::new();
    let mut net_placed: HashMap<i64, usize> = HashMap::new();
    for (i, p) in geom.pins.iter().enumerate() {
        if geom.pin_board[i] {
            continue;
        }
        let k = key_of(&p.part, &p.pin);
        let e = class_keys.entry(nl.pin_net_idx[i]).or_default();
        if !e.contains(&k) {
            e.push(k);
        }
        if k >= 0 {
            *net_placed.entry(k).or_insert(0) += 1;
        }
    }

    let mut rects = part_rects(c, lib);
    let mut centers: HashMap<String, [f64; 2]> =
        rects.iter().map(|(id, r)| (id.clone(), [(r.0[0] + r.1[0]) / 2.0, (r.0[1] + r.1[1]) / 2.0])).collect();
    // Centre of the first board, the pull of the first part.
    let board_center = c
        .parts
        .iter()
        .find_map(|p| {
            let pl = p.breadboard.as_ref()?;
            let def = lib.get(&p.ty)?;
            if !def.is_board() {
                return None;
            }
            let (a, b) = world_bbox(pl, def.breadboard.as_ref()?);
            Some([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0])
        })
        .unwrap_or([0.0, 0.0]);

    // Nets of each part (by id), to rank parts and find connected neighbours.
    let mut part_nets: HashMap<&str, Vec<usize>> = HashMap::new();
    for (ni, n) in nets.iter().enumerate() {
        for (p, _) in &n.pins {
            let e = part_nets.entry(p.as_str()).or_default();
            if !e.contains(&ni) {
                e.push(ni);
            }
        }
    }

    while !todo.is_empty() {
        // Most pins first, then the most nets that already have a pin on the board.
        let rank = |i: usize| -> (usize, usize, std::cmp::Reverse<usize>) {
            let p = &c.parts[i];
            let pins = lib.get(&p.ty).map(|d| d.placed_pins(View::Breadboard).count()).unwrap_or(0);
            let conn = part_nets
                .get(p.id.as_str())
                .map(|v| v.iter().filter(|&&n| net_placed.get(&(n as i64)).copied().unwrap_or(0) > 0).count())
                .unwrap_or(0);
            (pins, conn, std::cmp::Reverse(i))
        };
        let pos = (0..todo.len()).max_by_key(|&k| rank(todo[k])).expect("non-empty");
        let i = todo.remove(pos);
        let part = &c.parts[i];
        let def = lib.get(&part.ty).expect("resolved");
        let vd = def.breadboard.as_ref().expect("breadboard view");
        let pins: Vec<(String, crate::model::PinPos)> =
            def.placed_pins(View::Breadboard).map(|(p, pos)| (p.id.clone(), *pos)).collect();
        let keys: Vec<i64> = pins.iter().map(|(id, _)| key_of(&part.id, id)).collect();

        // Pull target: connected placed parts, else every placed part, else the board centre.
        let mut conn_ids: Vec<&str> = Vec::new();
        for &n in part_nets.get(part.id.as_str()).map(|v| v.as_slice()).unwrap_or(&[]) {
            for (p, _) in &nets[n].pins {
                if p != &part.id && centers.contains_key(p) && !conn_ids.contains(&p.as_str()) {
                    conn_ids.push(p.as_str());
                }
            }
        }
        let mut sum = [0.0, 0.0];
        let mut cnt = 0.0;
        let pool: Vec<[f64; 2]> = if conn_ids.is_empty() {
            let mut ids: Vec<&String> = centers.keys().collect();
            ids.sort();
            ids.iter().map(|k| centers[*k]).collect()
        } else {
            conn_ids.iter().map(|k| centers[*k]).collect()
        };
        for c in &pool {
            sum[0] += c[0];
            sum[1] += c[1];
            cnt += 1.0;
        }
        let pull = if cnt > 0.0 { [sum[0] / cnt, sum[1] / cnt] } else { board_center };

        let mut best: Option<(f64, Placement)> = None;
        for rot in [0, 90, 180, 270] {
            let pl0 = Placement { at: [0, 0], rot, mirror: false };
            let offs: Vec<Pt> = pins.iter().map(|(_, pos)| pin_world(&pl0, pos)).collect();
            if offs.is_empty() {
                continue;
            }
            let (bmin, bmax) = world_bbox(&pl0, vd);
            for &h in holes.keys() {
                let origin = [h[0] - offs[0][0], h[1] - offs[0][1]];
                let mut pts: Vec<(Pt, usize)> = Vec::with_capacity(offs.len());
                let mut ok = true;
                for off in &offs {
                    let p = [origin[0] + off[0], origin[1] + off[1]];
                    match holes.get(&p) {
                        Some(&cls) if !occupied.contains(&p) && !pts.iter().any(|(q, _)| *q == p) => pts.push((p, cls)),
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                let lo = [bmin[0] + origin[0] as f64, bmin[1] + origin[1] as f64];
                let hi = [bmax[0] + origin[0] as f64, bmax[1] + origin[1] as f64];
                if rects.iter().any(|(_, r)| frect_overlap(&(lo, hi), r)) {
                    continue;
                }
                // No strip may end up with two different nets.
                let mut per_class: Vec<(usize, Vec<i64>)> = Vec::new();
                for (j, (_, cls)) in pts.iter().enumerate() {
                    match per_class.iter_mut().find(|(c, _)| c == cls) {
                        Some((_, v)) => v.push(keys[j]),
                        None => per_class.push((*cls, vec![keys[j]])),
                    }
                }
                let short = per_class.iter().any(|(cls, new)| {
                    let mut all: Vec<i64> = class_keys.get(cls).cloned().unwrap_or_default();
                    all.extend(new);
                    all.sort_unstable();
                    all.dedup();
                    all.len() > 1
                });
                if short {
                    continue;
                }
                // Score: nets that cannot join without a jumper, weighted, plus distance.
                let mut joins = 0;
                let mut seen: Vec<i64> = Vec::new();
                for (j, &k) in keys.iter().enumerate() {
                    if k < 0 || seen.contains(&k) {
                        continue;
                    }
                    seen.push(k);
                    if net_placed.get(&k).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                    let joined = keys.iter().enumerate().any(|(m, &km)| {
                        km == k && class_keys.get(&pts[m].1).is_some_and(|v| v.contains(&k))
                    });
                    let _ = j;
                    if !joined {
                        joins += 1;
                    }
                }
                let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
                let dist = (center[0] - pull[0]).abs() + (center[1] - pull[1]).abs();
                let score = joins as f64 * 100.0
                    + dist
                    + origin[0] as f64 * 0.001
                    + origin[1] as f64 * 0.0001
                    + (rot / 90) as f64 * 0.00001;
                if best.as_ref().is_none_or(|(s, _)| score < *s) {
                    best = Some((score, Placement { at: origin, rot, mirror: false }));
                }
            }
        }
        let id = part.id.clone();
        match best {
            None => {
                acc.failures.push(RouteFailure { net: id, reason: "no free spot on the board".into() });
            }
            Some((_, pl)) => {
                let (bmin, bmax) = world_bbox(&pl, vd);
                for (j, (_, pos)) in pins.iter().enumerate() {
                    let p = pin_world(&pl, pos);
                    occupied.insert(p);
                    let cls = holes[&p];
                    let e = class_keys.entry(cls).or_default();
                    if !e.contains(&keys[j]) {
                        e.push(keys[j]);
                    }
                    if keys[j] >= 0 {
                        *net_placed.entry(keys[j]).or_insert(0) += 1;
                    }
                }
                rects.push((id.clone(), (bmin, bmax)));
                centers.insert(id.clone(), [(bmin[0] + bmax[0]) / 2.0, (bmin[1] + bmax[1]) / 2.0]);
                c.parts[i].breadboard = Some(pl);
                acc.placed.push(id);
            }
        }
    }
}

// ---- jumpers -----------------------------------------------------------------------------

/// `seg a-b` passes through the open interior of the rectangle.
fn seg_hits(a: Pt, b: Pt, r: &FRect) -> bool {
    let (ax, ay) = (a[0] as f64, a[1] as f64);
    let (dx, dy) = (b[0] as f64 - ax, b[1] as f64 - ay);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, ax - r.0[0]), (dx, r.1[0] - ax), (-dy, ay - r.0[1]), (dy, r.1[1] - ay)] {
        if p == 0.0 {
            if q <= 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return false;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return false;
                }
                t1 = t1.min(t);
            }
        }
    }
    t0 < t1
}

fn inside(p: Pt, r: &FRect) -> bool {
    let (x, y) = (p[0] as f64, p[1] as f64);
    x > r.0[0] && x < r.1[0] && y > r.0[1] && y < r.1[1]
}

/// The shapes of the jumper style from `a` to `b`, best first. Every one obeys the style:
/// straight is a single segment, orthogonal has horizontal and vertical segments only (never a
/// diagonal, even when a part is in the way), 45 degree uses only 0/45/90 degree segments.
fn style_shapes(a: Pt, b: Pt, style: &JumperStyle) -> Vec<Vec<Pt>> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let ortho = || -> Vec<Vec<Pt>> {
        if dx == 0 || dy == 0 {
            vec![vec![a, b]]
        } else {
            vec![vec![a, [a[0], b[1]], b], vec![a, [b[0], a[1]], b]]
        }
    };
    match style {
        JumperStyle::Straight => vec![vec![a, b]],
        JumperStyle::Orthogonal => ortho(),
        JumperStyle::Diagonal45 => {
            if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
                return vec![vec![a, b]];
            }
            let m = dx.abs().min(dy.abs());
            let (sx, sy) = (dx.signum(), dy.signum());
            vec![vec![a, [a[0] + sx * m, a[1] + sy * m], b], vec![a, [b[0] - sx * m, b[1] - sy * m], b]]
        }
    }
}

/// Candidate pairs per group kept when jumpers must not cross: the best one may be boxed in.
const TRIES_PER_GROUP: usize = 8;
/// Candidates (best first) that get the grid search when no plain shape works.
const DETOUR_TRIES: usize = 2;
/// Grid cells around the two ends the detour search may use, and the biggest search area.
const DETOUR_MARGINS: [i32; 2] = [6, 16];
const DETOUR_MAX_CELLS: i64 = 20_000;

fn orient(a: Pt, b: Pt, c: Pt) -> i64 {
    ((b[0] as i64 - a[0] as i64) * (c[1] as i64 - a[1] as i64) - (b[1] as i64 - a[1] as i64) * (c[0] as i64 - a[0] as i64)).signum()
}

fn in_box(a: Pt, b: Pt, p: Pt) -> bool {
    p[0] >= a[0].min(b[0]) && p[0] <= a[0].max(b[0]) && p[1] >= a[1].min(b[1]) && p[1] <= a[1].max(b[1])
}

/// The closed segments `p1-p2` and `p3-p4` share a point (crossing, touching or overlapping).
fn seg_touch(p1: Pt, p2: Pt, p3: Pt, p4: Pt) -> bool {
    let (d1, d2, d3, d4) = (orient(p3, p4, p1), orient(p3, p4, p2), orient(p1, p2, p3), orient(p1, p2, p4));
    (d1 * d2 < 0 && d3 * d4 < 0)
        || (d1 == 0 && in_box(p3, p4, p1))
        || (d2 == 0 && in_box(p3, p4, p2))
        || (d3 == 0 && in_box(p1, p2, p3))
        || (d4 == 0 && in_box(p1, p2, p4))
}

/// The breadboard wires placed so far: every segment, and the points where a wire begins or ends
/// (a bend is not an end).
#[derive(Default, Clone)]
struct WireMap {
    segs: Vec<(Pt, Pt)>,
    ends: HashSet<Pt>,
    /// Points a jumper must not run over although no wire is there yet: legs standing off the
    /// board, which a later jumper has to reach.
    reserved: HashSet<Pt>,
}

impl WireMap {
    fn add(&mut self, pts: &[Pt]) {
        self.segs.extend(pts.windows(2).filter(|w| w[0] != w[1]).map(|w| (w[0], w[1])));
        self.ends.extend(pts.first().into_iter().chain(pts.last()).copied());
    }

    /// The part of the map that can matter inside the box `lo..=hi` (one cell of slack).
    fn near(&self, lo: Pt, hi: Pt) -> WireMap {
        let segs = self
            .segs
            .iter()
            .copied()
            .filter(|(r, s)| r[0].max(s[0]) >= lo[0] - 1 && r[0].min(s[0]) <= hi[0] + 1 && r[1].max(s[1]) >= lo[1] - 1 && r[1].min(s[1]) <= hi[1] + 1)
            .collect();
        WireMap { segs, ends: self.ends.clone(), reserved: self.reserved.clone() }
    }
}

/// The segment `p-q` meets a wire or runs over a reserved point. A wire that ends exactly on one
/// of the jumper's own end points `ends` is already part of that junction and does not count; a
/// bend there does.
fn blocked(p: Pt, q: Pt, wires: &WireMap, ends: [Pt; 2]) -> bool {
    wires.segs.iter().any(|&(r, s)| {
        let own = ends.iter().any(|e| wires.ends.contains(e) && (*e == r || *e == s));
        !own && seg_touch(p, q, r, s)
    }) || wires.reserved.iter().any(|r| !ends.contains(r) && orient(p, q, *r) == 0 && in_box(p, q, *r))
}

fn path_crosses(path: &[Pt], wires: &WireMap, ends: [Pt; 2]) -> bool {
    path.windows(2).any(|w| blocked(w[0], w[1], wires, ends))
}

/// Points of the jumper from `a` to `b`. The shapes of the style are tried in order and the first
/// that is allowed wins: it must keep off the other wires unless crossings are allowed, and off
/// component bodies when `avoid_parts` is set (otherwise a shape clear of components is only
/// preferred). When none is allowed, a straight style also tries right-angle shapes, and then a
/// search on the grid goes around the wires and components; that search only makes right
/// angles, so an orthogonal jumper is orthogonal in every case. `None` when nothing is allowed.
fn jumper_path(a: Pt, b: Pt, opts: &RouteOptions, obstacles: &[FRect], wires: &WireMap, search: bool) -> Option<Vec<Pt>> {
    // A component that covers an end point cannot be avoided: ignore it.
    let bodies: Vec<&FRect> = obstacles.iter().filter(|r| !inside(a, r) && !inside(b, r)).collect();
    let over_part = |p: &[Pt]| p.windows(2).any(|w| bodies.iter().any(|r| seg_hits(w[0], w[1], r)));
    let crosses = |p: &[Pt]| !opts.allow_crossings && path_crosses(p, wires, [a, b]);
    let primary = style_shapes(a, b, &opts.jumper_style);
    // Right angles are the fallback of the other styles; an orthogonal jumper has no other kind.
    let fallback = if opts.jumper_style == JumperStyle::Orthogonal { Vec::new() } else { style_shapes(a, b, &JumperStyle::Orthogonal) };
    let clear = |s: &&Vec<Pt>| !crosses(s) && !over_part(s);
    let allowed = |s: &&Vec<Pt>| !crosses(s);
    if let Some(s) = primary.iter().find(clear) {
        return Some(s.clone());
    }
    if !opts.avoid_parts {
        // Over a component is allowed: take the style's own shape rather than bending it.
        if let Some(s) = primary.iter().find(allowed) {
            return Some(s.clone());
        }
    }
    if let Some(s) = fallback.iter().find(clear) {
        return Some(s.clone());
    }
    if !opts.avoid_parts {
        if let Some(s) = fallback.iter().find(allowed) {
            return Some(s.clone());
        }
    }
    if search {
        return detour(a, b, obstacles, wires, opts.avoid_parts);
    }
    None
}

/// Cheapest orthogonal path from `a` to `b` whose unit steps meet no wire of `wires`; bends cost
/// extra, and steps over component bodies are either forbidden (`avoid_parts`) or cost extra.
/// `None` when walled in.
fn detour(a: Pt, b: Pt, obstacles: &[FRect], wires: &WireMap, avoid_parts: bool) -> Option<Vec<Pt>> {
    // Close to the ends first (short jumpers); a wider search only when that is walled in.
    DETOUR_MARGINS.iter().find_map(|&m| detour_within(a, b, obstacles, wires, avoid_parts, m))
}

fn detour_within(a: Pt, b: Pt, obstacles: &[FRect], wires: &WireMap, avoid_parts: bool, margin: i32) -> Option<Vec<Pt>> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let (lo, hi) = ([a[0].min(b[0]) - margin, a[1].min(b[1]) - margin], [a[0].max(b[0]) + margin, a[1].max(b[1]) + margin]);
    if (hi[0] - lo[0] + 1) as i64 * (hi[1] - lo[1] + 1) as i64 > DETOUR_MAX_CELLS {
        return None;
    }
    let near = wires.near(lo, hi);
    let body: Vec<&FRect> = obstacles.iter().filter(|r| !inside(a, r) && !inside(b, r)).collect();
    const STEP: i32 = 10;
    const BEND: i32 = 6;
    const OVER_PART: i32 = 4;
    const DIRS: [[i32; 2]; 4] = [[1, 0], [-1, 0], [0, 1], [0, -1]];
    let mut dist: HashMap<(Pt, u8), i32> = HashMap::new();
    let mut from: HashMap<(Pt, u8), (Pt, u8)> = HashMap::new();
    let mut heap = BinaryHeap::new();
    // A*: a step costs at least STEP, so STEP times the Manhattan distance never overestimates.
    let h = |p: Pt| STEP * ((p[0] - b[0]).abs() + (p[1] - b[1]).abs());
    for d in 0..4u8 {
        dist.insert((a, d), 0);
        heap.push(Reverse((h(a), 0, a, d)));
    }
    let mut goal: Option<(Pt, u8)> = None;
    while let Some(Reverse((_, cost, p, d))) = heap.pop() {
        if dist.get(&(p, d)).is_some_and(|&best| best < cost) {
            continue;
        }
        if p == b {
            goal = Some((p, d));
            break;
        }
        for (nd, step) in DIRS.iter().enumerate() {
            let q = [p[0] + step[0], p[1] + step[1]];
            if q[0] < lo[0] || q[0] > hi[0] || q[1] < lo[1] || q[1] > hi[1] || blocked(p, q, &near, [a, b]) {
                continue;
            }
            if avoid_parts && body.iter().any(|r| seg_hits(p, q, r)) {
                continue;
            }
            let turn = if p != a && nd as u8 != d { BEND } else { 0 };
            let over = if body.iter().any(|r| inside(q, r)) { OVER_PART } else { 0 };
            let next = cost + STEP + turn + over;
            let key = (q, nd as u8);
            if dist.get(&key).is_none_or(|&best| next < best) {
                dist.insert(key, next);
                from.insert(key, (p, d));
                heap.push(Reverse((next + h(q), next, q, nd as u8)));
            }
        }
    }
    let mut cur = goal?;
    let mut rev = vec![cur.0];
    while let Some(&prev) = from.get(&cur) {
        rev.push(prev.0);
        cur = prev;
    }
    rev.reverse();
    // Keep the corners only.
    let mut out: Vec<Pt> = vec![rev[0]];
    for i in 1..rev.len() - 1 {
        let (p, q, r) = (rev[i - 1], rev[i], rev[i + 1]);
        if orient(p, q, r) != 0 {
            out.push(q);
        }
    }
    out.push(*rev.last()?);
    Some(out)
}

fn jumpers(c: &mut Circuit, lib: &dyn PartLookup, opts: &RouteOptions, nets: &[SrcNet], pre: &[bool], acc: &mut Acc) {
    let nl = Netlist::build(c, lib, View::Breadboard);
    let geom = &nl.geom;
    let mut occupied: HashSet<Pt> = HashSet::new();
    let mut board_pts: HashSet<Pt> = HashSet::new();
    let mut class_holes: HashMap<usize, Vec<Pt>> = HashMap::new();
    let mut class_loose: HashMap<usize, Vec<Pt>> = HashMap::new();
    for (i, p) in geom.pins.iter().enumerate() {
        if geom.pin_board[i] {
            board_pts.insert(p.at);
            class_holes.entry(nl.pin_net_idx[i]).or_default().push(p.at);
        } else {
            occupied.insert(p.at);
        }
    }
    for (i, p) in geom.pins.iter().enumerate() {
        if !geom.pin_board[i] && !board_pts.contains(&p.at) {
            class_loose.entry(nl.pin_net_idx[i]).or_default().push(p.at);
        }
    }
    for v in class_holes.values_mut() {
        v.sort_unstable();
    }
    for pts in &geom.wire_pts {
        if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
            occupied.insert(*a);
            occupied.insert(*b);
        }
    }
    let obstacles: Vec<FRect> = part_rects(c, lib).into_iter().map(|(_, r)| r).collect();
    let covered = |p: Pt| obstacles.iter().any(|r| inside(p, r));

    let mut ids = IdGen::new(c);
    let mut colored = 0usize;
    // Every segment of the breadboard wires placed so far (existing and new): jumpers keep off
    // them unless crossings are allowed.
    let mut wire_map = WireMap::default();
    for w in c.wires.iter().filter(|w| w.view == View::Breadboard) {
        wire_map.add(&w.pts);
    }
    wire_map.reserved = class_loose.values().flatten().copied().collect();
    // Usable attach points of a set of strips: free holes plus loose pins.
    let points = |classes: &[usize], occupied: &HashSet<Pt>| -> Vec<Pt> {
        let mut v: Vec<Pt> = Vec::new();
        for cls in classes {
            if let Some(h) = class_holes.get(cls) {
                v.extend(h.iter().copied().filter(|p| !occupied.contains(p)));
            }
            if let Some(l) = class_loose.get(cls) {
                v.extend(l.iter().copied());
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    };

    for (ni, net) in nets.iter().enumerate() {
        // Groups: the strips (netlist nets) the placed pins sit in.
        let mut groups: Vec<(usize, usize)> = Vec::new(); // (class, pin count)
        for (p, q) in &net.pins {
            if let Some(cls) = nl.net_of_pin(p, q) {
                match groups.iter_mut().find(|(g, _)| *g == cls) {
                    Some((_, n)) => *n += 1,
                    None => groups.push((cls, 1)),
                }
            }
        }
        // Usable pins whose part is not placed (--no-place, or no spot found): reported, and the
        // net is not routed even when its other pins get wired.
        let incomplete =
            report_unplaced(acc, net, View::Breadboard, net.pins.iter().filter(|(p, q)| nl.net_of_pin(p, q).is_none()));
        let placed_pins: usize = groups.iter().map(|g| g.1).sum();
        if placed_pins < 2 {
            continue;
        }
        if groups.len() <= 1 {
            if !net.tainted && !incomplete && !(matches!(opts.scope, Scope::Unrouted) && pre[ni]) {
                acc.routed += 1;
            }
            continue;
        }
        let seed = (0..groups.len()).max_by_key(|&g| (groups[g].1, std::cmp::Reverse(g))).unwrap_or(0);
        let mut tree: Vec<usize> = vec![groups[seed].0];
        let mut remaining: Vec<usize> = (0..groups.len()).filter(|&g| g != seed).map(|g| groups[g].0).collect();
        let mut failure: Option<String> = None;
        let mut colour: Option<String> = None;
        while !remaining.is_empty() {
            let tp = points(&tree, &occupied);
            // Candidate (manhattan, covered ends, a, b) pairs, best first, over every remaining
            // group. With crossings allowed the best pair always works; without, a few runners-up
            // per group are kept for when the best one cannot avoid the wires already placed.
            let mut cands: Vec<((i32, i32, Pt, Pt), usize)> = Vec::new();
            let mut too_long: Option<i32> = None;
            let mut no_hole = false;
            let keep = if opts.allow_crossings && !opts.avoid_parts { 1 } else { TRIES_PER_GROUP };
            // Without crossings an end must not sit under another wire (a bend or the middle of one).
            let clear_of_wires = |v: Vec<Pt>| -> Vec<Pt> {
                // (Also keeps the ends of the best pairs from sitting under another wire.)
                if opts.allow_crossings {
                    v
                } else {
                    v.into_iter().filter(|p| !wire_map.segs.iter().any(|&(r, s)| seg_touch(*p, *p, r, s) && !wire_map.ends.contains(p))).collect()
                }
            };
            let tp = clear_of_wires(tp);
            for (ri, &g) in remaining.iter().enumerate() {
                let gp = clear_of_wires(points(&[g], &occupied));
                if gp.is_empty() || tp.is_empty() {
                    no_hole = true;
                    continue;
                }
                let mut keys: Vec<(i32, i32, Pt, Pt)> = Vec::with_capacity(tp.len() * gp.len());
                for &a in &tp {
                    for &b in &gp {
                        let d = (a[0] - b[0]).abs() + (a[1] - b[1]).abs();
                        keys.push((d, covered(a) as i32 + covered(b) as i32, a, b));
                    }
                }
                keys.sort_unstable();
                let Some(shortest) = keys.first().map(|k| k.0) else { continue };
                if let Some(max) = opts.max_jumper_len {
                    if shortest > max {
                        too_long = Some(too_long.map_or(shortest, |t| t.min(shortest)));
                        continue;
                    }
                }
                let max = opts.max_jumper_len.unwrap_or(i32::MAX);
                cands.extend(keys.into_iter().filter(|k| k.0 <= max).take(keep).map(|k| (k, ri)));
            }
            cands.sort_unstable();
            // Plain shapes for every candidate first; the grid search only for the best few.
            let chosen = cands
                .iter()
                .find_map(|((_, _, a, b), ri)| jumper_path(*a, *b, opts, &obstacles, &wire_map, false).map(|pts| (*a, *b, *ri, pts)))
                .or_else(|| {
                    cands.iter().take(DETOUR_TRIES).find_map(|((_, _, a, b), ri)| {
                        jumper_path(*a, *b, opts, &obstacles, &wire_map, true).map(|pts| (*a, *b, *ri, pts))
                    })
                });
            let Some((a, b, ri, pts)) = chosen else {
                failure = Some(match (too_long, no_hole, cands.is_empty()) {
                    (Some(d), _, true) => format!("jumper too long ({d} > {})", opts.max_jumper_len.unwrap_or(0)),
                    (_, _, false) => {
                        let mut what: Vec<&str> = Vec::new();
                        if !opts.allow_crossings {
                            what.push("another wire");
                        }
                        if opts.avoid_parts {
                            what.push("a component");
                        }
                        format!("every jumper route would cross {} (relax that option to join this net)", what.join(" or "))
                    }
                    _ => "no free hole in the strip".to_string(),
                });
                break;
            };
            let colour = colour.get_or_insert_with(|| match &opts.jumper_color {
                JumperColor::Single(s) => s.clone(),
                JumperColor::ByNet => {
                    let col = PALETTE[colored % PALETTE.len()].to_string();
                    colored += 1;
                    col
                }
            });
            wire_map.add(&pts);
            let id = ids.fresh("j");
            c.wires.push(Wire { id: id.clone(), view: View::Breadboard, pts, color: Some(colour.clone()) });
            acc.wires_added.push(id);
            occupied.insert(a);
            occupied.insert(b);
            let g = remaining.remove(ri);
            tree.push(g);
        }
        match failure {
            None => {
                if !net.tainted && !incomplete {
                    acc.routed += 1;
                }
            }
            Some(reason) => acc.failures.push(RouteFailure { net: net.display(), reason }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_touch_when_they_cross_meet_or_overlap() {
        assert!(seg_touch([0, 0], [4, 4], [0, 4], [4, 0]), "proper crossing");
        assert!(seg_touch([0, 0], [4, 0], [4, 0], [4, 4]), "shared end");
        assert!(seg_touch([0, 0], [4, 0], [2, 0], [6, 0]), "collinear overlap");
        assert!(seg_touch([0, 0], [4, 0], [2, -2], [2, 0]), "end on the interior");
        assert!(!seg_touch([0, 0], [4, 0], [5, 0], [8, 0]), "collinear gap");
        assert!(!seg_touch([0, 0], [4, 0], [0, 1], [4, 1]), "parallel");
        assert!(!seg_touch([0, 0], [4, 4], [3, 0], [4, 1]), "apart");
    }

    fn map(wires: &[&[Pt]]) -> WireMap {
        let mut m = WireMap::default();
        for w in wires {
            m.add(w);
        }
        m
    }

    #[test]
    fn a_wire_ending_on_the_jumper_end_is_exempt_but_a_bend_there_is_not() {
        let a = [0, 0];
        let b = [10, 0];
        let ends = [a, b];
        // A leg that ends at `b` is the same junction, not a crossing.
        assert!(!blocked(a, b, &map(&[&[b, [10, 5]]]), ends));
        // The same point as a bend of another wire (which then overlaps it) is a collision.
        assert!(blocked(a, b, &map(&[&[[10, 6], [10, 0], [14, 0]]]), ends));
        assert!(blocked(a, b, &map(&[&[[5, -3], [5, 3]]]), ends));
    }

    #[test]
    fn detour_goes_around_a_wall_and_never_touches_it() {
        // A long vertical wire stands between the ends.
        let wall = map(&[&[[5, -6], [5, 6]]]);
        let (a, b) = ([0, 0], [10, 0]);
        let path = detour(a, b, &[], &wall, false).expect("a way around exists");
        assert_eq!((path[0], *path.last().unwrap()), (a, b));
        assert!(!path_crosses(&path, &wall, [a, b]), "{path:?}");
        assert!(path.len() > 2, "not a straight line: {path:?}");
        // Orthogonal steps only.
        assert!(path.windows(2).all(|w| w[0][0] == w[1][0] || w[0][1] == w[1][1]), "{path:?}");
    }

    #[test]
    fn detour_gives_up_when_walled_in() {
        // The target sits inside a closed box of wires.
        let (a, b) = ([0, 0], [20, 0]);
        let ring = map(&[&[[18, -2], [22, -2], [22, 2], [18, 2], [18, -2]]]);
        assert!(detour(a, b, &[], &ring, false).is_none());
    }

    #[test]
    fn jumper_path_respects_the_crossing_switch() {
        let wall = map(&[&[[5, -6], [5, 6]]]);
        let (a, b) = ([0, 0], [10, 0]);
        let mut o = RouteOptions { jumper_style: JumperStyle::Straight, allow_crossings: true, ..RouteOptions::default() };
        assert_eq!(jumper_path(a, b, &o, &[], &wall, true), Some(vec![a, b]), "crossings allowed: the style's shape");
        o.allow_crossings = false;
        let p = jumper_path(a, b, &o, &[], &wall, true).unwrap();
        assert!(p.len() > 2 && !path_crosses(&p, &wall, [a, b]), "{p:?}");
    }

    fn orthogonal(p: &[Pt]) -> bool {
        p.windows(2).all(|w| w[0][0] == w[1][0] || w[0][1] == w[1][1])
    }

    #[test]
    fn orthogonal_jumpers_are_never_diagonal_even_with_parts_in_the_way() {
        // Parts sit on both L shapes between the ends, so neither corner is clear.
        let (a, b) = ([0, 0], [10, 8]);
        let parts: Vec<FRect> = vec![([-1.0, 2.0], [3.0, 6.0]), ([7.0, 2.0], [11.0, 6.0])];
        for avoid in [false, true] {
            for style in [JumperStyle::Orthogonal, JumperStyle::Straight, JumperStyle::Diagonal45] {
                let o = RouteOptions { jumper_style: style.clone(), avoid_parts: avoid, ..RouteOptions::default() };
                let p = jumper_path(a, b, &o, &parts, &WireMap::default(), true).expect("a route exists");
                assert_eq!((p[0], *p.last().unwrap()), (a, b));
                if style == JumperStyle::Orthogonal {
                    assert!(orthogonal(&p), "avoid {avoid}: {p:?}");
                }
                if avoid {
                    assert!(!p.windows(2).any(|w| parts.iter().any(|r| seg_hits(w[0], w[1], r))), "{style:?}: {p:?}");
                }
            }
        }
    }

    #[test]
    fn a_part_covering_an_end_does_not_block_the_jumper() {
        // The end b sits under the part: it cannot be avoided and must not make routing fail.
        let (a, b) = ([0, 0], [6, 0]);
        let parts: Vec<FRect> = vec![([4.0, -2.0], [8.0, 2.0])];
        let o = RouteOptions { avoid_parts: true, ..RouteOptions::default() };
        assert_eq!(jumper_path(a, b, &o, &parts, &WireMap::default(), true), Some(vec![a, b]));
    }

    #[test]
    fn avoid_parts_off_takes_the_first_shape_over_a_part_when_nothing_is_clear() {
        let (a, b) = ([0, 0], [10, 0]);
        let parts: Vec<FRect> = vec![([4.0, -1.0], [6.0, 1.0])];
        let off = RouteOptions { avoid_parts: false, jumper_style: JumperStyle::Straight, ..RouteOptions::default() };
        assert_eq!(jumper_path(a, b, &off, &parts, &WireMap::default(), true), Some(vec![a, b]), "straight over the part");
        let on = RouteOptions { avoid_parts: true, ..off };
        let p = jumper_path(a, b, &on, &parts, &WireMap::default(), true).unwrap();
        assert!(!p.windows(2).any(|w| parts.iter().any(|r| seg_hits(w[0], w[1], r))), "{p:?}");
    }
}
