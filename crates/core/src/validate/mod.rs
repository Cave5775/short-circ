//! Circuit validation, implementing the rule table in docs/FORMAT.md.
//!
//! - circuit-wide rules (`schema`, `duplicate-id`, `unknown-type`, `bad-color`, `bad-net`) live
//!   here;
//! - per-view rules (`view.rs`) run once for every view in use and carry `view`;
//! - cross-view rules (`cross.rs`: `net-open`, `net-merged`, `unplaced-part`) compare the views.

mod cross;
mod view;

use crate::color::is_valid_color_spec;
use crate::model::{Circuit, Issue, PartLookup, Pt, Report, Severity, Shape, View, FORMAT};
use std::collections::{BTreeMap, HashMap};

pub(crate) fn issue(rule: &str, severity: Severity, ids: Vec<String>, at: Option<Pt>, message: String) -> Issue {
    Issue { rule: rule.to_string(), severity, ids, at, message, view: None }
}

pub(crate) fn fmt(p: Pt) -> String {
    format!("({},{})", p[0], p[1])
}

/// A view is "in use" when at least one part is placed in it or at least one wire is drawn in it.
pub fn view_in_use(circuit: &Circuit, view: View) -> bool {
    circuit.parts.iter().any(|p| p.placement(view).is_some()) || circuit.wires.iter().any(|w| w.view == view)
}

/// Where to point at for a part in a circuit-wide issue: its schematic placement, else its
/// breadboard placement.
fn part_at(p: &crate::model::Part) -> Option<Pt> {
    p.schematic.as_ref().or(p.breadboard.as_ref()).map(|pl| pl.at)
}

/// `bad-color` (warning): every colour spec that is not a colour, "none" or "default".
fn check_colors(circuit: &Circuit, issues: &mut Vec<Issue>) {
    let mut bad = |id: &str, at: Option<Pt>, what: String, value: &Option<String>| {
        if let Some(v) = value.as_deref().filter(|v| !is_valid_color_spec(v)) {
            issues.push(issue(
                "bad-color",
                Severity::Warning,
                vec![id.to_string()],
                at,
                format!(
                    "{what} {v:?} is not a colour (use #rgb, #rrggbb, #rrggbbaa, a preset name, \"none\" or \"default\")"
                ),
            ));
        }
    };
    for p in &circuit.parts {
        bad(&p.id, part_at(p), format!("part {} color", p.id), &p.color);
        bad(&p.id, part_at(p), format!("part {} fill", p.id), &p.fill);
        bad(&p.id, part_at(p), format!("part {} label_color", p.id), &p.label_color);
    }
    for w in &circuit.wires {
        bad(&w.id, w.pts.first().copied(), format!("wire {} color", w.id), &w.color);
    }
    for l in &circuit.labels {
        bad(&l.name, Some(l.at), format!("label {} color", l.name), &l.color);
    }
    if let Some(t) = &circuit.theme {
        for (name, v) in [
            ("background", &t.background),
            ("grid", &t.grid),
            ("wire", &t.wire),
            ("part", &t.part),
            ("label", &t.label),
            ("pin", &t.pin),
            ("junction", &t.junction),
        ] {
            bad("theme", None, format!("theme.{name}"), v);
        }
    }
    for def in &circuit.lib {
        for view in View::ALL {
            let Some(vd) = def.view(view) else { continue };
            // Schematic keeps the v1 wording; other views name the view.
            let scope = if view == View::Schematic { String::new() } else { format!("{} ", view.key()) };
            for (i, s) in vd.shapes.iter().enumerate() {
                let st = match s {
                    Shape::Line { style, .. }
                    | Shape::Rect { style, .. }
                    | Shape::Circle { style, .. }
                    | Shape::Arc { style, .. }
                    | Shape::Poly { style, .. }
                    | Shape::Text { style, .. } => style,
                };
                bad(&def.ty, None, format!("lib part {} {scope}shape {} stroke", def.ty, i), &st.stroke);
                bad(&def.ty, None, format!("lib part {} {scope}shape {} fill", def.ty, i), &st.fill);
            }
        }
    }
}

/// `bad-net` (error): a declared net names a part or pin that does not exist. A pin of a part
/// whose type is unknown cannot be checked (`unknown-type` reports that).
fn check_declared_nets(circuit: &Circuit, lib: &dyn PartLookup, issues: &mut Vec<Issue>) {
    for (n, d) in circuit.nets.iter().enumerate() {
        let label = d.name.clone().unwrap_or_else(|| format!("#{}", n + 1));
        for r in &d.pins {
            if !pin_ref_exists(circuit, lib, r) {
                issues.push(issue(
                    "bad-net",
                    Severity::Error,
                    vec![r.clone()],
                    None,
                    format!("declared net {label} names {r:?}, which is not a part and pin of this circuit (use PART.PIN)"),
                ));
            }
        }
    }
}

/// `PART.PIN` names a part of the circuit and a logical pin of its type (ids may contain dots,
/// so every split point is tried).
fn pin_ref_exists(circuit: &Circuit, lib: &dyn PartLookup, r: &str) -> bool {
    r.match_indices('.').any(|(i, _)| {
        let (part, pin) = (&r[..i], &r[i + 1..]);
        circuit.parts.iter().filter(|p| p.id == part).any(|p| match lib.get(&p.ty) {
            Some(def) => def.pins.iter().any(|d| d.id == pin),
            None => true,
        })
    })
}

/// Circuit-wide rules: `schema`, `duplicate-id`, `unknown-type`, `bad-color`, `bad-net`.
fn check_circuit(circuit: &Circuit, lib: &dyn PartLookup, issues: &mut Vec<Issue>) {
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
        .map(|p| (p.id.as_str(), part_at(p).unwrap_or([0, 0])))
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

    // unknown-type
    for p in &circuit.parts {
        if lib.get(&p.ty).is_none() {
            issues.push(issue(
                "unknown-type",
                Severity::Error,
                vec![p.id.clone()],
                part_at(p),
                format!("part {} has unknown type \"{}\"", p.id, p.ty),
            ));
        }
    }

    check_colors(circuit, issues);
    check_declared_nets(circuit, lib, issues);
}

fn finish(mut issues: Vec<Issue>) -> Report {
    issues.sort_by(|a, b| {
        (&a.rule, &a.ids, a.at, &a.message, a.view).cmp(&(&b.rule, &b.ids, b.at, &b.message, b.view))
    });
    Report::from_issues(issues)
}

/// Every rule: the circuit-wide rules, the per-view rules for each view in use, and the
/// cross-view group.
pub fn validate(circuit: &Circuit, lib: &dyn PartLookup) -> Report {
    let mut issues = Vec::new();
    check_circuit(circuit, lib, &mut issues);
    for v in View::ALL {
        if view_in_use(circuit, v) {
            view::check(circuit, lib, v, &mut issues);
        }
    }
    cross::check(circuit, lib, &mut issues);
    finish(issues)
}

/// The circuit-wide rules plus the per-view rules of `view` (whether or not it is in use), and
/// no cross-view checks.
pub fn validate_view(circuit: &Circuit, lib: &dyn PartLookup, view: View) -> Report {
    let mut issues = Vec::new();
    check_circuit(circuit, lib, &mut issues);
    view::check(circuit, lib, view, &mut issues);
    finish(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_circuit, Library};

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
    fn bad_colors_warn_on_every_object_kind() {
        let r = run(&format!(
            r##"{{{HEAD},"theme":{{"wire":"nope","background":"#123456"}},
            "lib":[{{"type":"box","size":[2,2],"shapes":[{{"k":"rect","at":[0,0],"size":[2,2],"stroke":"bad1","fill":"none"}}]}}],
            "parts":[{{"id":"P1","type":"box","at":[0,0],"color":"#12","fill":"red","label_color":"nonee"}}],
            "wires":[{{"id":"w1","pts":[[5,0],[8,0]],"color":"reddish"}},{{"id":"w2","pts":[[5,2],[8,2]],"color":"default"}}],
            "labels":[{{"at":[5,0],"name":"N","color":"#12345"}}]}}"##
        ));
        let warn = |ids: &[&str], needle: &str| {
            r.issues.iter().any(|i| {
                i.rule == "bad-color"
                    && i.severity == Severity::Warning
                    && i.ids.iter().map(|s| s.as_str()).eq(ids.iter().copied())
                    && i.message.contains(needle)
            })
        };
        assert!(warn(&["P1"], "part P1 color \"#12\""), "{:?}", r.issues);
        assert!(warn(&["P1"], "label_color \"nonee\""));
        assert!(!r.issues.iter().any(|i| i.message.contains("fill \"red\"")));
        assert!(warn(&["w1"], "wire w1 color \"reddish\""));
        assert!(warn(&["N"], "label N color"));
        assert!(warn(&["theme"], "theme.wire \"nope\""));
        assert!(warn(&["box"], "shape 0 stroke \"bad1\""));
        assert_eq!(r.issues.iter().filter(|i| i.rule == "bad-color").count(), 6);
        assert_eq!(r.errors, 0, "bad-color is only a warning");
    }

    #[test]
    fn good_colors_do_not_warn() {
        let r = run(&format!(
            r##"{{{HEAD},"theme":{{"wire":"red","grid":"none","pin":"default","label":"#abc"}},
            "wires":[{{"id":"w1","pts":[[5,0],[8,0]],"color":"#AABBCCDD"}},{{"id":"w2","pts":[[5,2],[8,2]],"color":"None"}}]}}"##
        ));
        assert!(!r.issues.iter().any(|i| i.rule == "bad-color"), "{:?}", r.issues);
    }

    #[test]
    fn led_example_has_no_errors() {
        let c = parse_circuit(include_str!("../../../../examples/led.circ")).unwrap();
        let r = validate(&c, &Library::load(&c, None));
        assert_eq!(r.errors, 0, "{:#?}", r.issues);
        assert!(r.ok);
    }

    #[test]
    fn bad_example_reports_expected_issues() {
        let c = parse_circuit(include_str!("../../../../examples/bad.circ")).unwrap();
        let r = validate(&c, &Library::load(&c, None));
        assert!(!r.ok);
        assert!(has(&r, "wire-overlap", &["w1", "w2"]), "{:#?}", r.issues);
        assert!(has(&r, "wire-crossing", &["w3", "w4"]));
        assert!(has(&r, "dangling-wire", &["w5"]));
        assert!(has(&r, "part-overlap", &["R1", "R2"]));
        let ov = r.issues.iter().find(|i| i.rule == "wire-overlap").unwrap();
        assert_eq!(ov.at, Some([3, 6]));
        assert!(ov.message.contains("(3,6)") && ov.message.contains("(5,6)"));
        assert_eq!(ov.view, Some(View::Schematic));
        let cr = r.issues.iter().find(|i| i.rule == "wire-crossing").unwrap();
        assert_eq!(cr.at, Some([20, 5]));
        assert_eq!(cr.severity, Severity::Warning);
    }

    #[test]
    fn report_is_deterministic_and_sorted() {
        let c = parse_circuit(include_str!("../../../../examples/bad.circ")).unwrap();
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
        let dup = r.issues.iter().find(|i| i.rule == "duplicate-id").unwrap();
        assert_eq!(dup.at, Some([0, 20]), "first duplicate: the second part");
        assert_eq!(dup.view, None);
        let rot = r.issues.iter().find(|i| i.rule == "bad-rotation").unwrap();
        assert_eq!((rot.at, rot.view), (Some([0, 20]), Some(View::Schematic)));
    }

    #[test]
    fn schema_wrong_format() {
        let mut c = parse_circuit(r#"{"format":"short-circ/2"}"#).unwrap();
        c.format = "other/9".into();
        let r = validate(&c, &Library::load(&c, None));
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

    #[test]
    fn validate_view_runs_only_that_view_and_circuit_wide_rules() {
        let c = parse_circuit(
            r#"{"format":"short-circ/2","parts":[{"id":"X1","type":"nope"}],
            "wires":[{"id":"a","pts":[[0,0],[5,0]]},{"id":"j","view":"breadboard","pts":[[0,0],[5,5]]}]}"#,
        )
        .unwrap();
        let lib = Library::load(&c, None);
        let s = validate_view(&c, &lib, View::Schematic);
        assert!(has(&s, "unknown-type", &["X1"]));
        assert!(has(&s, "dangling-wire", &["a"]));
        assert!(!has(&s, "dangling-wire", &["j"]));
        let b = validate_view(&c, &lib, View::Breadboard);
        assert!(has(&b, "dangling-wire", &["j"]) && !has(&b, "dangling-wire", &["a"]));
        assert!(b.issues.iter().filter(|i| i.rule == "dangling-wire").all(|i| i.view == Some(View::Breadboard)));
        let all = validate(&c, &lib);
        assert!(has(&all, "dangling-wire", &["a"]) && has(&all, "dangling-wire", &["j"]));
    }

    #[test]
    fn views_in_use() {
        let c = parse_circuit(r#"{"format":"short-circ/2"}"#).unwrap();
        assert!(!view_in_use(&c, View::Schematic) && !view_in_use(&c, View::Breadboard));
        let c = parse_circuit(r#"{"format":"short-circ/2","wires":[{"id":"j","view":"breadboard","pts":[[0,0],[1,0]]}]}"#)
            .unwrap();
        assert!(!view_in_use(&c, View::Schematic) && view_in_use(&c, View::Breadboard));
        let c = parse_circuit(r#"{"format":"short-circ/2","parts":[{"id":"R1","type":"r","schematic":{"at":[0,0]}}]}"#)
            .unwrap();
        assert!(view_in_use(&c, View::Schematic) && !view_in_use(&c, View::Breadboard));
    }
}
