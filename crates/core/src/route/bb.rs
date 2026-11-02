//! Breadboard target: put boards in place, place parts onto board holes without creating
//! shorts, then join the pins of every net that ended up in several strips with jumpers.
//!
//! Placement is exhaustive over rotations and board holes. A candidate is valid when every pin of
//! the part lands on a distinct free hole, the part's bounding box overlaps no other non-board
//! part, and no strip would hold two different nets (pins that belong to no net count as nets of
//! their own, so a resistor bridges two strips and a DIP straddles the centre gap).

use super::nets::{can_exist, SrcNet};
use super::{Acc, IdGen, JumperColor, JumperStyle, RouteFailure, RouteOptions, Scope};
use crate::model::{Circuit, Part, PartLookup, Placement, Pt, View, Wire};
use crate::netlist::Netlist;
use crate::transform::{pin_world, world_bbox};
use std::collections::{BTreeMap, HashMap, HashSet};

const PALETTE: [&str; 8] = ["red", "orange", "yellow", "green", "blue", "purple", "brown", "gray"];

type FRect = ([f64; 2], [f64; 2]);

pub(crate) fn run(c: &mut Circuit, lib: &dyn PartLookup, opts: &RouteOptions, nets: Vec<SrcNet>, acc: &mut Acc) {
    let has_board = ensure_boards(c, lib, opts, acc);
    let pre = connected_before(c, lib, &nets);
    if opts.place_missing {
        place_parts(c, lib, &nets, has_board, acc);
    }
    jumpers(c, lib, opts, &nets, &pre, acc);
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
        c.parts[i].breadboard = Some(Placement::at(0, 0));
        acc.placed.push(c.parts[i].id.clone());
        return true;
    }
    if !is_board_def(lib, "breadboard_half") {
        return false;
    }
    let id = IdGen::new(c).fresh("BB");
    c.parts.push(Part { id: id.clone(), ty: "breadboard_half".into(), breadboard: Some(Placement::at(0, 0)), ..Part::default() });
    acc.placed.push(id);
    true
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

fn jumper_pts(a: Pt, b: Pt, style: &JumperStyle, obstacles: &[FRect]) -> Vec<Pt> {
    // Parts that cover an end point cannot be avoided: ignore them.
    let obs: Vec<&FRect> = obstacles.iter().filter(|r| !inside(a, r) && !inside(b, r)).collect();
    let clear = |m: Pt| !obs.iter().any(|r| seg_hits(a, m, r) || seg_hits(m, b, r));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    match style {
        JumperStyle::Straight => vec![a, b],
        JumperStyle::Orthogonal => {
            if dx == 0 || dy == 0 {
                return vec![a, b];
            }
            let (c1, c2) = ([a[0], b[1]], [b[0], a[1]]);
            if clear(c1) {
                vec![a, c1, b]
            } else if clear(c2) {
                vec![a, c2, b]
            } else {
                vec![a, b]
            }
        }
        JumperStyle::Diagonal45 => {
            if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
                return vec![a, b];
            }
            let m = dx.abs().min(dy.abs());
            let (sx, sy) = (dx.signum(), dy.signum());
            let c1 = [a[0] + sx * m, a[1] + sy * m];
            let c2 = [b[0] - sx * m, b[1] - sy * m];
            // Keep the 45 degree shape even when a part is in the way.
            if !clear(c1) && clear(c2) {
                vec![a, c2, b]
            } else {
                vec![a, c1, b]
            }
        }
    }
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
        let placed_pins: usize = groups.iter().map(|g| g.1).sum();
        if placed_pins < 2 {
            continue;
        }
        if groups.len() <= 1 {
            if !net.tainted && !(matches!(opts.scope, Scope::Unrouted) && pre[ni]) {
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
            // Best (manhattan, covered ends, a, b) over every remaining group.
            let mut best: Option<((i32, i32, Pt, Pt), usize)> = None;
            let mut too_long: Option<i32> = None;
            let mut no_hole = false;
            for (ri, &g) in remaining.iter().enumerate() {
                let gp = points(&[g], &occupied);
                if gp.is_empty() || tp.is_empty() {
                    no_hole = true;
                    continue;
                }
                let mut gbest: Option<(i32, i32, Pt, Pt)> = None;
                for &a in &tp {
                    for &b in &gp {
                        let d = (a[0] - b[0]).abs() + (a[1] - b[1]).abs();
                        let key = (d, covered(a) as i32 + covered(b) as i32, a, b);
                        if gbest.is_none_or(|g| key < g) {
                            gbest = Some(key);
                        }
                    }
                }
                let Some(k) = gbest else { continue };
                if let Some(max) = opts.max_jumper_len {
                    if k.0 > max {
                        too_long = Some(too_long.map_or(k.0, |t| t.min(k.0)));
                        continue;
                    }
                }
                if best.is_none_or(|(b, _)| k < b) {
                    best = Some((k, ri));
                }
            }
            let Some(((_, _, a, b), ri)) = best else {
                failure = Some(match (too_long, no_hole) {
                    (Some(d), _) => format!("jumper too long ({d} > {})", opts.max_jumper_len.unwrap_or(0)),
                    (None, _) => "no free hole in the strip".to_string(),
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
            let pts = jumper_pts(a, b, &opts.jumper_style, &obstacles);
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
                if !net.tainted {
                    acc.routed += 1;
                }
            }
            Some(reason) => acc.failures.push(RouteFailure { net: net.display(), reason }),
        }
    }
}
