//! Per-view validation rules (docs/FORMAT.md rule table, scopes "per view", "schematic",
//! "breadboard").
//!
//! Wire geometry uses exact integer arithmetic. Candidate wire pairs come from a spatial hash
//! over segments (see `netlist::Geom`), so cost scales with local density, not wire count.

use super::{fmt, issue};
use crate::model::{Circuit, Issue, PartLookup, Pt, Severity, View};
use crate::netlist::{on_segment, sub, Netlist, Occ};
use crate::transform::world_bbox;
use std::collections::{BTreeMap, HashMap, HashSet};

fn cross(u: [i64; 2], v: [i64; 2]) -> i64 {
    u[0] * v[1] - u[1] * v[0]
}

fn dot(u: [i64; 2], v: [i64; 2]) -> i64 {
    u[0] * v[0] + u[1] * v[1]
}

/// Collinear stretch shared by segments `a-b` and `c-d`, as `(start, end)` ordered
/// lexicographically, if it has positive length. Both segments must be non-degenerate.
fn collinear_overlap(a: Pt, b: Pt, c: Pt, d: Pt) -> Option<(Pt, Pt)> {
    let u = sub(b, a);
    if cross(u, sub(c, a)) != 0 || cross(u, sub(d, a)) != 0 {
        return None;
    }
    let len = dot(u, u);
    // Positions along `a-b`, scaled by |u|.
    let (tc, td) = (dot(sub(c, a), u), dot(sub(d, a), u));
    let ((lo_p, lo_t), (hi_p, hi_t)) = if tc <= td { ((c, tc), (d, td)) } else { ((d, td), (c, tc)) };
    let (lo, lo_pt) = if lo_t >= 0 { (lo_t, lo_p) } else { (0, a) };
    let (hi, hi_pt) = if hi_t <= len { (hi_t, hi_p) } else { (len, b) };
    if hi <= lo {
        return None;
    }
    Some(if lo_pt <= hi_pt { (lo_pt, hi_pt) } else { (hi_pt, lo_pt) })
}

/// Proper crossing of `a-b` and `c-d` strictly inside both segments. Returns the crossing
/// point rounded to the grid and whether it is exactly on a grid point.
fn proper_crossing(a: Pt, b: Pt, c: Pt, d: Pt) -> Option<(Pt, bool)> {
    let (u, v) = (sub(b, a), sub(d, c));
    let den = cross(u, v);
    if den == 0 {
        return None;
    }
    let s = |x: i64| x.signum();
    if s(cross(u, sub(c, a))) * s(cross(u, sub(d, a))) >= 0 {
        return None;
    }
    if s(cross(v, sub(a, c))) * s(cross(v, sub(b, c))) >= 0 {
        return None;
    }
    // P = a + u * num/den
    let num = cross(sub(c, a), v) as i128;
    let den = den as i128;
    let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
    let mut exact = true;
    let mut p = [0i32; 2];
    for k in 0..2 {
        let n = a[k] as i128 * den + u[k] as i128 * num;
        if n.rem_euclid(den) != 0 {
            exact = false;
        }
        p[k] = (2 * n + den).div_euclid(2 * den) as i32;
    }
    Some((p, exact))
}

/// Run every per-view rule for `view`. Each issue carries `view`.
pub(super) fn check(circuit: &Circuit, lib: &dyn PartLookup, view: View, out: &mut Vec<Issue>) {
    let mut issues: Vec<Issue> = Vec::new();
    let schematic = view == View::Schematic;

    // bad-rotation, no-view
    for p in &circuit.parts {
        let Some(pl) = p.placement(view) else { continue };
        if !matches!(pl.rot, 0 | 90 | 180 | 270) {
            issues.push(issue(
                "bad-rotation",
                Severity::Error,
                vec![p.id.clone()],
                Some(pl.at),
                format!("part {} has rotation {}, expected 0, 90, 180 or 270", p.id, pl.rot),
            ));
        }
        if let Some(def) = lib.get(&p.ty) {
            if def.view(view).is_none() {
                issues.push(issue(
                    "no-view",
                    Severity::Error,
                    vec![p.id.clone()],
                    Some(pl.at),
                    format!("part {} is placed in the {} view but type \"{}\" has no {} geometry", p.id, view.key(), p.ty, view.key()),
                ));
            }
        }
    }

    let nl = Netlist::build(circuit, lib, view);
    let geom = &nl.geom;

    // Per-wire checks: wire-degenerate (every view), wire-diagonal (schematic).
    for (wi, pts) in geom.wire_pts.iter().enumerate() {
        let id = &geom.wire_ids[wi];
        let distinct = pts.iter().collect::<HashSet<_>>().len();
        let zero_seg = pts.windows(2).position(|w| w[0] == w[1]);
        if pts.len() < 2 || distinct < 2 {
            issues.push(issue(
                "wire-degenerate",
                Severity::Error,
                vec![id.clone()],
                pts.first().copied(),
                format!("wire {id} has fewer than 2 distinct points"),
            ));
        } else if let Some(i) = zero_seg {
            issues.push(issue(
                "wire-degenerate",
                Severity::Error,
                vec![id.clone()],
                Some(pts[i]),
                format!("wire {id} has a zero-length segment at {}", fmt(pts[i])),
            ));
        }
        if schematic {
            let diag: Vec<_> = pts.windows(2).filter(|w| w[0][0] != w[1][0] && w[0][1] != w[1][1]).collect();
            if let Some(w) = diag.first() {
                issues.push(issue(
                    "wire-diagonal",
                    Severity::Warning,
                    vec![id.clone()],
                    Some(w[0]),
                    format!(
                        "wire {id} has {} non-orthogonal segment(s), first from {} to {}",
                        diag.len(),
                        fmt(w[0]),
                        fmt(w[1])
                    ),
                ));
            }
        }
    }

    if schematic {
        schematic_wire_pairs(&nl, &mut issues);
        wire_through_pin(&nl, &mut issues);
    }

    // dangling-wire: one issue per end that touches nothing.
    let mut label_count: HashMap<&str, usize> = HashMap::new();
    let mut labels_at: HashMap<Pt, Vec<&str>> = HashMap::new();
    if schematic {
        for l in &circuit.labels {
            *label_count.entry(l.name.as_str()).or_insert(0) += 1;
            labels_at.entry(l.at).or_default().push(l.name.as_str());
        }
    }
    for (wi, pts) in geom.wire_pts.iter().enumerate() {
        if pts.len() < 2 {
            continue;
        }
        let last = pts.len() - 1;
        for vi in [0, last] {
            let p = pts[vi];
            let adjacent_seg = if vi == 0 { 0 } else { last - 1 };
            let touches_occ = geom.pt_map[&p].iter().any(|o| match *o {
                Occ::Pin(_) => true,
                // A breadboard wire's two end points are never "the same polyline touching".
                Occ::Vertex(w, i) => w != wi || (schematic && i.abs_diff(vi) > 1),
            });
            let touches_seg = || {
                geom.near(p).iter().any(|&si| {
                    let s = &geom.segs[si as usize];
                    (s.wire != wi || s.idx != adjacent_seg) && on_segment(s.a, s.b, p)
                })
            };
            let touches_label = || labels_at.get(&p).is_some_and(|v| v.iter().any(|n| label_count[n] >= 2));
            if !(touches_occ || touches_seg() || touches_label()) {
                let id = &geom.wire_ids[wi];
                issues.push(issue(
                    "dangling-wire",
                    Severity::Warning,
                    vec![id.clone()],
                    Some(p),
                    format!("wire {id} has an endpoint at {} that touches nothing", fmt(p)),
                ));
            }
        }
    }

    // unconnected-pin (board pins and pins in a bus with other pins are exempt)
    for (i, pin) in geom.pins.iter().enumerate() {
        if geom.pin_board[i] || geom.pin_in_bus[i] {
            continue;
        }
        let net = &nl.nets[nl.pin_net_idx[i]];
        if net.pins.len() == 1 && net.wires.is_empty() {
            issues.push(issue(
                "unconnected-pin",
                Severity::Warning,
                vec![pin.part.clone()],
                Some(pin.at),
                format!("pin {}.{} at {} is not connected", pin.part, pin.pin, fmt(pin.at)),
            ));
        }
    }

    if schematic {
        // empty-net, net-short
        let first_pt: HashMap<&str, Pt> = geom
            .wire_ids
            .iter()
            .zip(&geom.wire_pts)
            .filter_map(|(id, pts)| pts.first().map(|p| (id.as_str(), *p)))
            .collect();
        for net in &nl.nets {
            if net.pins.is_empty() {
                let mut ids = net.wires.clone();
                ids.sort();
                let at = net.wires.first().and_then(|w| first_pt.get(w.as_str()).copied());
                issues.push(issue(
                    "empty-net",
                    Severity::Warning,
                    ids.clone(),
                    at,
                    format!("net with wire(s) {} has no pins", ids.join(", ")),
                ));
            }
            let labels = &nl.labels[net.id];
            if let Some((first, _)) = labels.first() {
                if let Some((other, at)) = labels.iter().find(|(n, _)| n != first) {
                    let mut names: Vec<String> = labels.iter().map(|(n, _)| n.clone()).collect();
                    names.sort();
                    names.dedup();
                    issues.push(issue(
                        "net-short",
                        Severity::Error,
                        names.clone(),
                        Some(*at),
                        format!("labels {} are joined on one net (\"{}\" meets \"{}\")", names.join(", "), first, other),
                    ));
                }
            }
        }
    } else {
        pin_conflict(&nl, &mut issues);
    }

    part_overlap(circuit, lib, view, &mut issues);

    for i in &mut issues {
        i.view = Some(view);
    }
    out.append(&mut issues);
}

/// wire-overlap / wire-crossing over candidate segment pairs from the spatial hash.
fn schematic_wire_pairs(nl: &Netlist, issues: &mut Vec<Issue>) {
    let geom = &nl.geom;
    let mut overlaps: HashMap<(usize, usize), (Pt, Pt)> = HashMap::new();
    let mut crossings: HashMap<(usize, usize), Pt> = HashMap::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    for cell in geom.cells() {
        for (i, &s1) in cell.iter().enumerate() {
            for &s2 in &cell[i + 1..] {
                let (x, y) = (&geom.segs[s1 as usize], &geom.segs[s2 as usize]);
                if x.wire == y.wire {
                    continue;
                }
                if x.a[0].max(x.b[0]) < y.a[0].min(y.b[0])
                    || y.a[0].max(y.b[0]) < x.a[0].min(x.b[0])
                    || x.a[1].max(x.b[1]) < y.a[1].min(y.b[1])
                    || y.a[1].max(y.b[1]) < x.a[1].min(x.b[1])
                {
                    continue;
                }
                if !seen.insert((s1.min(s2), s1.max(s2))) {
                    continue;
                }
                let key = (x.wire.min(y.wire), x.wire.max(y.wire));
                if let Some(ov) = collinear_overlap(x.a, x.b, y.a, y.b) {
                    let e = overlaps.entry(key).or_insert(ov);
                    *e = (*e).min(ov);
                } else if let Some((p, exact)) = proper_crossing(x.a, x.b, y.a, y.b) {
                    // A pin or another wire's vertex exactly there already forms a junction.
                    if exact && geom.pt_map.contains_key(&p) {
                        continue;
                    }
                    let e = crossings.entry(key).or_insert(p);
                    *e = (*e).min(p);
                }
            }
        }
    }
    let pair_ids = |k: (usize, usize)| {
        let mut ids = vec![geom.wire_ids[k.0].clone(), geom.wire_ids[k.1].clone()];
        ids.sort();
        ids
    };
    for (k, (s, e)) in &overlaps {
        let ids = pair_ids(*k);
        let msg = format!("{} and {} overlap between {} and {}", ids[0], ids[1], fmt(*s), fmt(*e));
        issues.push(issue("wire-overlap", Severity::Error, ids, Some(*s), msg));
    }
    for (k, p) in &crossings {
        let ids = pair_ids(*k);
        let msg = format!("{} and {} cross at {} without a junction", ids[0], ids[1], fmt(*p));
        issues.push(issue("wire-crossing", Severity::Warning, ids, Some(*p), msg));
    }
}

/// wire-through-pin (schematic).
fn wire_through_pin(nl: &Netlist, issues: &mut Vec<Issue>) {
    let geom = &nl.geom;
    let mut through: HashSet<(usize, usize)> = HashSet::new();
    for (pi, pin) in geom.pins.iter().enumerate() {
        let p = pin.at;
        for &si in geom.near(p) {
            let s = &geom.segs[si as usize];
            if p == s.a || p == s.b || !on_segment(s.a, s.b, p) {
                continue;
            }
            let has_vertex = geom.pt_map[&p].iter().any(|o| matches!(o, Occ::Vertex(w, _) if *w == s.wire));
            if has_vertex || !through.insert((s.wire, pi)) {
                continue;
            }
            let wid = &geom.wire_ids[s.wire];
            issues.push(issue(
                "wire-through-pin",
                Severity::Error,
                vec![wid.clone(), pin.part.clone()],
                Some(p),
                format!("wire {wid} passes through pin {}.{} at {} without a vertex", pin.part, pin.pin, fmt(p)),
            ));
        }
    }
}

/// pin-conflict (breadboard): pins of two or more non-board parts at one point.
fn pin_conflict(nl: &Netlist, issues: &mut Vec<Issue>) {
    let geom = &nl.geom;
    let mut at: BTreeMap<Pt, Vec<usize>> = BTreeMap::new();
    for (i, pin) in geom.pins.iter().enumerate() {
        if !geom.pin_board[i] {
            at.entry(pin.at).or_default().push(i);
        }
    }
    for (p, pins) in at {
        if pins.len() < 2 {
            continue;
        }
        let refs: Vec<String> = pins.iter().map(|&i| format!("{}.{}", geom.pins[i].part, geom.pins[i].pin)).collect();
        let mut ids: Vec<String> = pins.iter().map(|&i| geom.pins[i].part.clone()).collect();
        ids.sort();
        ids.dedup();
        issues.push(issue(
            "pin-conflict",
            Severity::Error,
            ids,
            Some(p),
            format!("pins {} are all at {}", refs.join(", "), fmt(p)),
        ));
    }
}

/// part-overlap: sweep over x-sorted bounding boxes of the non-board parts placed in `view`.
fn part_overlap(circuit: &Circuit, lib: &dyn PartLookup, view: View, issues: &mut Vec<Issue>) {
    let mut boxes: Vec<(&str, [f64; 2], [f64; 2])> = Vec::new();
    for p in &circuit.parts {
        let Some(pl) = p.placement(view) else { continue };
        let Some(def) = lib.get(&p.ty) else { continue };
        let Some(vd) = def.view(view) else { continue };
        if def.is_board() {
            continue;
        }
        let (min, max) = world_bbox(pl, vd);
        boxes.push((p.id.as_str(), min, max));
    }
    boxes.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            if b.1[0] >= a.2[0] {
                break;
            }
            let lo = [a.1[0].max(b.1[0]), a.1[1].max(b.1[1])];
            let hi = [a.2[0].min(b.2[0]), a.2[1].min(b.2[1])];
            if lo[0] < hi[0] && lo[1] < hi[1] {
                let mut ids = vec![a.0.to_string(), b.0.to_string()];
                ids.sort();
                let at = [lo[0].round() as i32, lo[1].round() as i32];
                issues.push(issue(
                    "part-overlap",
                    Severity::Error,
                    ids.clone(),
                    Some(at),
                    format!("parts {} and {} overlap at {}", ids[0], ids[1], fmt(at)),
                ));
            }
        }
    }
}
