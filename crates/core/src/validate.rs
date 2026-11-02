//! Circuit validation, implementing the rule table in docs/FORMAT.md.
//!
//! Wire geometry uses exact integer arithmetic. Candidate wire pairs come from a spatial hash
//! over segments (see `netlist::Geom`), so cost scales with local density, not wire count.

use crate::model::{Circuit, Issue, Pt, Report, Severity, FORMAT};
use crate::netlist::{on_segment, sub, Netlist, Occ};
use crate::resolve::Library;
use crate::transform::world_bbox;
use std::collections::{BTreeMap, HashMap, HashSet};

fn issue(rule: &str, severity: Severity, ids: Vec<String>, at: Option<Pt>, message: String) -> Issue {
    Issue { rule: rule.to_string(), severity, ids, at, message }
}

fn fmt(p: Pt) -> String {
    format!("({},{})", p[0], p[1])
}

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

pub fn validate(circuit: &Circuit, lib: &Library) -> Report {
    let mut issues: Vec<Issue> = Vec::new();
    let nl = Netlist::build(circuit, lib);
    let geom = &nl.geom;

    // schema
    if circuit.format != FORMAT {
        issues.push(issue(
            "schema",
            Severity::Error,
            vec![],
            None,
            format!("unsupported format \"{}\", expected \"{}\"", circuit.format, FORMAT),
        ));
    }

    // duplicate-id (parts and wires share one id space)
    let mut first_seen: HashMap<&str, usize> = HashMap::new();
    let mut dup_at: BTreeMap<&str, Pt> = BTreeMap::new();
    let mut dup_count: HashMap<&str, usize> = HashMap::new();
    let entries = circuit
        .parts
        .iter()
        .map(|p| (p.id.as_str(), p.at))
        .chain(circuit.wires.iter().map(|w| (w.id.as_str(), w.pts.first().copied().unwrap_or([0, 0]))));
    for (n, (id, at)) in entries.enumerate() {
        *dup_count.entry(id).or_insert(0) += 1;
        if first_seen.insert(id, n).is_some() {
            dup_at.entry(id).or_insert(at);
        }
    }
    for (id, at) in dup_at {
        issues.push(issue(
            "duplicate-id",
            Severity::Error,
            vec![id.to_string()],
            Some(at),
            format!("id {} is used {} times", id, dup_count[id]),
        ));
    }

    // unknown-type, bad-rotation
    for p in &circuit.parts {
        if lib.get(&p.ty).is_none() {
            issues.push(issue(
                "unknown-type",
                Severity::Error,
                vec![p.id.clone()],
                Some(p.at),
                format!("part {} has unknown type \"{}\"", p.id, p.ty),
            ));
        }
        if !matches!(p.rot, 0 | 90 | 180 | 270) {
            issues.push(issue(
                "bad-rotation",
                Severity::Error,
                vec![p.id.clone()],
                Some(p.at),
                format!("part {} has rotation {}, expected 0, 90, 180 or 270", p.id, p.rot),
            ));
        }
    }

    // Per-wire checks: wire-diagonal, wire-degenerate.
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
        let diag: Vec<_> = pts.windows(2).filter(|w| w[0][0] != w[1][0] && w[0][1] != w[1][1]).collect();
        if let Some(w) = diag.first() {
            issues.push(issue(
                "wire-diagonal",
                Severity::Warning,
                vec![id.clone()],
                Some(w[0]),
                format!("wire {id} has {} non-orthogonal segment(s), first from {} to {}", diag.len(), fmt(w[0]), fmt(w[1])),
            ));
        }
    }

    // wire-overlap / wire-crossing over candidate segment pairs from the spatial hash.
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

    // wire-through-pin
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

    // dangling-wire: one issue per endpoint that touches nothing.
    let mut label_count: HashMap<&str, usize> = HashMap::new();
    let mut labels_at: HashMap<Pt, Vec<&str>> = HashMap::new();
    for l in &circuit.labels {
        *label_count.entry(l.name.as_str()).or_insert(0) += 1;
        labels_at.entry(l.at).or_default().push(l.name.as_str());
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
                Occ::Vertex(w, i) => w != wi || i.abs_diff(vi) > 1,
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

    // unconnected-pin, empty-net, net-short
    let first_pt: HashMap<&str, Pt> = geom
        .wire_ids
        .iter()
        .zip(&geom.wire_pts)
        .filter_map(|(id, pts)| pts.first().map(|p| (id.as_str(), *p)))
        .collect();
    for net in &nl.nets {
        if net.pins.len() == 1 && net.wires.is_empty() {
            let pin = &net.pins[0];
            issues.push(issue(
                "unconnected-pin",
                Severity::Warning,
                vec![pin.part.clone()],
                Some(pin.at),
                format!("pin {}.{} at {} is not connected", pin.part, pin.pin, fmt(pin.at)),
            ));
        }
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

    // part-overlap: sweep over x-sorted bounding boxes.
    let mut boxes: Vec<(&str, [f64; 2], [f64; 2])> = Vec::new();
    for p in &circuit.parts {
        if let Some(def) = lib.get(&p.ty) {
            let (min, max) = world_bbox(p, def);
            boxes.push((p.id.as_str(), min, max));
        }
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

    issues.sort_by(|a, b| {
        (&a.rule, &a.ids, a.at, &a.message).cmp(&(&b.rule, &b.ids, b.at, &b.message))
    });
    Report::from_issues(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_circuit;

    fn run(src: &str) -> Report {
        let c = parse_circuit(src).unwrap();
        validate(&c, &Library::load(&c, None))
    }

    fn rules(r: &Report) -> Vec<(&str, Vec<&str>)> {
        r.issues
            .iter()
            .map(|i| (i.rule.as_str(), i.ids.iter().map(|s| s.as_str()).collect()))
            .collect()
    }

    fn has(r: &Report, rule: &str, ids: &[&str]) -> bool {
        rules(r).iter().any(|(ru, i)| *ru == rule && i == ids)
    }

    const HEAD: &str = r#""format":"short-circ/1""#;

    #[test]
    fn led_example_has_no_errors() {
        let c = parse_circuit(include_str!("../../../examples/led.circ")).unwrap();
        let r = validate(&c, &Library::load(&c, None));
        assert_eq!(r.errors, 0, "{:#?}", r.issues);
        assert!(r.ok);
    }

    #[test]
    fn bad_example_reports_expected_issues() {
        let c = parse_circuit(include_str!("../../../examples/bad.circ")).unwrap();
        let r = validate(&c, &Library::load(&c, None));
        assert!(!r.ok);
        assert!(has(&r, "wire-overlap", &["w1", "w2"]), "{:#?}", r.issues);
        assert!(has(&r, "wire-crossing", &["w3", "w4"]));
        assert!(has(&r, "dangling-wire", &["w5"]));
        assert!(has(&r, "part-overlap", &["R1", "R2"]));
        let ov = r.issues.iter().find(|i| i.rule == "wire-overlap").unwrap();
        assert_eq!(ov.at, Some([3, 6]));
        assert!(ov.message.contains("(3,6)") && ov.message.contains("(5,6)"));
        let cr = r.issues.iter().find(|i| i.rule == "wire-crossing").unwrap();
        assert_eq!(cr.at, Some([20, 5]));
        assert_eq!(cr.severity, Severity::Warning);
    }

    #[test]
    fn report_is_deterministic_and_sorted() {
        let c = parse_circuit(include_str!("../../../examples/bad.circ")).unwrap();
        let lib = Library::load(&c, None);
        let a = serde_json::to_string(&validate(&c, &lib)).unwrap();
        let b = serde_json::to_string(&validate(&c, &lib)).unwrap();
        assert_eq!(a, b);
        let r = validate(&c, &lib);
        assert!(r.issues.windows(2).all(|w| w[0].rule <= w[1].rule));
    }

    #[test]
    fn overlap_vertical_and_diagonal() {
        let r = run(&format!(
            r#"{{{HEAD},"wires":[
            {{"id":"a","pts":[[0,0],[0,10]]}},{{"id":"b","pts":[[0,4],[0,20]]}},
            {{"id":"c","pts":[[10,10],[20,20]]}},{{"id":"d","pts":[[14,14],[30,30]]}},
            {{"id":"e","pts":[[40,0],[40,5]]}},{{"id":"f","pts":[[40,5],[40,9]]}}]}}"#
        ));
        assert!(has(&r, "wire-overlap", &["a", "b"]));
        assert!(has(&r, "wire-overlap", &["c", "d"]));
        // e and f only touch end to end: no overlap.
        assert!(!has(&r, "wire-overlap", &["e", "f"]));
        let cd = r.issues.iter().find(|i| i.rule == "wire-overlap" && i.ids[0] == "c").unwrap();
        assert_eq!(cd.at, Some([14, 14]));
    }

    #[test]
    fn t_junction_is_not_a_crossing() {
        let r = run(&format!(
            r#"{{{HEAD},"wires":[{{"id":"h","pts":[[0,5],[10,5]]}},{{"id":"v","pts":[[5,5],[5,10]]}},
            {{"id":"v2","pts":[[5,0],[5,5]]}}]}}"#
        ));
        assert!(!rules(&r).iter().any(|(ru, _)| *ru == "wire-crossing"));
        assert!(!rules(&r).iter().any(|(ru, _)| *ru == "wire-overlap"));
    }

    #[test]
    fn diagonal_crossing_and_warning() {
        let r = run(&format!(
            r#"{{{HEAD},"wires":[{{"id":"a","pts":[[0,0],[10,10]]}},{{"id":"b","pts":[[0,10],[10,0]]}}]}}"#
        ));
        assert!(has(&r, "wire-crossing", &["a", "b"]));
        assert!(has(&r, "wire-diagonal", &["a"]));
        assert!(r.issues.iter().filter(|i| i.rule == "wire-diagonal").all(|i| i.severity == Severity::Warning));
    }

    #[test]
    fn wire_through_pin_detected_but_vertex_is_fine() {
        // R1 pins at (0,1) and (8,1).
        let r = run(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}}],
            "wires":[{{"id":"w","pts":[[-5,1],[10,1]]}}]}}"#
        ));
        assert!(has(&r, "wire-through-pin", &["w", "R1"]));
        let r = run(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}}],
            "wires":[{{"id":"w","pts":[[-5,1],[0,1],[0,9],[8,9],[8,1],[10,1]]}}]}}"#
        ));
        assert!(!rules(&r).iter().any(|(ru, _)| *ru == "wire-through-pin"));
    }

    #[test]
    fn degenerate_wires() {
        let r = run(&format!(
            r#"{{{HEAD},"wires":[{{"id":"a","pts":[[3,3]]}},{{"id":"b","pts":[[3,3],[3,3]]}},
            {{"id":"c","pts":[[0,0],[5,0],[5,0],[9,0]]}},{{"id":"d","pts":[[0,50],[5,50]]}}]}}"#
        ));
        assert!(has(&r, "wire-degenerate", &["a"]));
        assert!(has(&r, "wire-degenerate", &["b"]));
        assert!(has(&r, "wire-degenerate", &["c"]));
        assert!(!has(&r, "wire-degenerate", &["d"]));
    }

    #[test]
    fn ids_types_and_rotation() {
        let r = run(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}},{{"id":"R1","type":"nope","at":[0,20],"rot":45}}],
            "wires":[{{"id":"R1","pts":[[0,40],[5,40]]}}]}}"#
        ));
        assert!(has(&r, "duplicate-id", &["R1"]));
        assert!(has(&r, "unknown-type", &["R1"]));
        assert!(has(&r, "bad-rotation", &["R1"]));
        assert_eq!(r.issues.iter().filter(|i| i.rule == "duplicate-id").count(), 1);
    }

    #[test]
    fn schema_wrong_format() {
        let r = run(r#"{"format":"other/9"}"#);
        assert!(has(&r, "schema", &[]));
        assert!(!r.ok);
    }

    #[test]
    fn unconnected_pin_and_empty_net() {
        let r = run(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}}],
            "wires":[{{"id":"w","pts":[[0,1],[0,5]]}},{{"id":"x","pts":[[30,30],[35,30]]}}]}}"#
        ));
        // R1.a has a wire; R1.b is alone.
        let un: Vec<_> = r.issues.iter().filter(|i| i.rule == "unconnected-pin").collect();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].at, Some([8, 1]));
        assert!(has(&r, "empty-net", &["x"]));
        assert!(has(&r, "dangling-wire", &["x"]));
    }

    #[test]
    fn net_short_on_label_conflict() {
        let r = run(&format!(
            r#"{{{HEAD},"wires":[{{"id":"a","pts":[[0,0],[5,0]]}}],
            "labels":[{{"at":[0,0],"name":"VCC"}},{{"at":[5,0],"name":"GND"}}]}}"#
        ));
        assert!(has(&r, "net-short", &["GND", "VCC"]));
        // Same name twice is fine.
        let r = run(&format!(
            r#"{{{HEAD},"wires":[{{"id":"a","pts":[[0,0],[5,0]]}}],
            "labels":[{{"at":[0,0],"name":"VCC"}},{{"at":[5,0],"name":"VCC"}}]}}"#
        ));
        assert!(!rules(&r).iter().any(|(ru, _)| *ru == "net-short"));
    }

    #[test]
    fn rotated_parts_use_rotated_bbox_for_overlap() {
        // Two LEDs: unrotated bbox 4x2 at (0,0) = (0..4,0..2); rot 90 at (3,0) = (1..3... ) check overlap.
        let r = run(&format!(
            r#"{{{HEAD},"parts":[{{"id":"D1","type":"led","at":[0,0]}},{{"id":"D2","type":"led","at":[10,0],"rot":90}},
            {{"id":"D3","type":"led","at":[14,0]}}]}}"#
        ));
        // D2 rot 90 at (10,0) spans x 8..10, y 0..4; D3 spans 14..18: no overlaps.
        assert!(!rules(&r).iter().any(|(ru, _)| *ru == "part-overlap"));
        let r = run(&format!(
            r#"{{{HEAD},"parts":[{{"id":"D1","type":"led","at":[9,0]}},{{"id":"D2","type":"led","at":[10,0],"rot":90}}]}}"#
        ));
        assert!(has(&r, "part-overlap", &["D1", "D2"]));
    }

    #[test]
    fn touching_bboxes_do_not_overlap() {
        let r = run(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}},{{"id":"R2","type":"resistor","at":[8,0]}}]}}"#
        ));
        assert!(!rules(&r).iter().any(|(ru, _)| *ru == "part-overlap"));
    }

    #[test]
    fn scales_to_ten_thousand_wires() {
        // 10k short, disjoint horizontal wires on a grid plus a few crossings.
        let mut wires = String::new();
        for i in 0..10_000 {
            let (x, y) = ((i % 100) * 10, (i / 100) * 4);
            if i > 0 {
                wires.push(',');
            }
            wires.push_str(&format!(r#"{{"id":"w{i}","pts":[[{x},{y}],[{},{y}]]}}"#, x + 6));
        }
        wires.push_str(r#",{"id":"vx","pts":[[3,-5],[3,500]]}"#);
        let src = format!(r#"{{{HEAD},"wires":[{wires}]}}"#);
        let t = std::time::Instant::now();
        let r = run(&src);
        assert!(t.elapsed().as_secs() < 20, "too slow: {:?}", t.elapsed());
        // The vertical wire crosses the first wire of each of the 100 rows.
        assert_eq!(r.issues.iter().filter(|i| i.rule == "wire-crossing").count(), 100);
    }
}
