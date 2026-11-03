use super::*;
use crate::edit::fresh_id;
use crate::model::{Circuit, Report, Severity, View};
use crate::netlist::Netlist;
use crate::{parse_circuit, validate, write, Library};
use std::collections::BTreeMap;

// ---- helpers -----------------------------------------------------------------------------

fn errors(r: &Report) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for i in r.issues.iter().filter(|i| i.severity == Severity::Error) {
        let mut ids = i.ids.clone();
        ids.sort();
        *m.entry(format!("{} {}", i.rule, ids.join(","))).or_insert(0) += 1;
    }
    m
}

fn view_data(c: &Circuit, view: View) -> serde_json::Value {
    let parts: Vec<serde_json::Value> = c
        .parts
        .iter()
        .filter_map(|p| p.placement(view).map(|pl| serde_json::json!([p.id, pl])))
        .collect();
    let wires: Vec<&crate::model::Wire> = c.wires_in(view).collect();
    let labels = if view == View::Schematic { serde_json::to_value(&c.labels).unwrap() } else { serde_json::Value::Null };
    serde_json::json!({"parts": parts, "wires": wires, "labels": labels})
}

fn opts(to: View) -> RouteOptions {
    RouteOptions { to, ..RouteOptions::default() }
}

/// Port with every output guarantee of FORMAT.md asserted. Returns the outcome.
fn checked(c: &Circuit, o: &RouteOptions) -> RouteOutcome {
    let lib = Library::load(c, None);
    let before = errors(&validate(c, &lib));
    let out = port(c, &lib, o);
    let lib2 = Library::load(&out.circuit, None);
    let report = validate(&out.circuit, &lib2);
    let after = errors(&report);
    for (k, n) in &after {
        let was = before.get(k).copied().unwrap_or(0);
        assert!(*n <= was, "new error {k:?} (x{n}, was {was}); issues: {:#?}", report.issues);
    }
    // The other view is untouched.
    let other = o.to.other();
    assert_eq!(view_data(c, other), view_data(&out.circuit, other), "other view changed");
    // Pins of existing placements in the target view did not move.
    for p in &c.parts {
        if let Some(pl) = p.placement(o.to) {
            assert_eq!(out.circuit.part(&p.id).unwrap().placement(o.to), Some(pl), "part {} moved", p.id);
        }
    }
    // Accounting.
    for id in &out.placed {
        assert!(out.circuit.part(id).unwrap().placement(o.to).is_some(), "placed {id} has no placement");
        assert!(c.part(id).is_none_or(|p| p.placement(o.to).is_none()), "{id} was already placed");
    }
    // A wire that absorbed another one is listed in wires_added under its old id.
    // (After --replace the old wires are gone, so their ids may be handed out again.)
    let new_wires = out.wires_added.iter().filter(|id| !o.keep_wires || !c.wires.iter().any(|w| &w.id == *id && w.view == o.to)).count();
    let expected = c.wires.len() + new_wires - removed_wires(c, &out.circuit, o);
    if out.wires_merged == 0 {
        assert_eq!(out.circuit.wires.len(), expected, "wires_added accuracy");
    } else {
        // Each join removes one wire, possibly one that was added in the same run.
        assert!(out.circuit.wires.len() + out.wires_merged >= expected, "wires_added accuracy after merging");
    }
    for id in &out.wires_added {
        assert!(out.circuit.wires.iter().any(|w| &w.id == id && w.view == o.to));
    }
    // Every net reported as routed is connected in the target view.
    let mut dummy = Vec::new();
    let nets = nets::source_nets(c, &lib, o, &mut dummy);
    let nl = Netlist::build(&out.circuit, &lib2, o.to);
    let failed: Vec<&str> = out.failures.iter().map(|f| f.net.as_str()).collect();
    let mut connected = 0;
    for n in &nets {
        let ids: Vec<Option<usize>> = n.pins.iter().map(|(p, q)| nl.net_of_pin(p, q)).collect();
        let placed: Vec<usize> = ids.iter().flatten().copied().collect();
        if placed.len() < 2 {
            continue;
        }
        // A net with a usable pin that is not placed in the target view is never routed.
        if placed.len() < ids.len() {
            for ((p, q), id) in n.pins.iter().zip(&ids) {
                let named = out.failures.iter().any(|f| f.net == n.display() && f.reason.contains(&format!("{p}.{q} is not placed")));
                let by_part = out.failures.iter().any(|f| f.net == *p);
                assert!(id.is_some() || named || by_part, "unplaced {p}.{q} is not reported: {:?}", out.failures);
            }
            continue;
        }
        let ok = placed.windows(2).all(|w| w[0] == w[1]);
        if failed.contains(&n.display().as_str()) {
            continue;
        }
        assert!(ok, "net {} reported routed but not connected in {}", n.display(), o.to.key());
        connected += 1;
    }
    if !matches!(o.scope, Scope::Unrouted) {
        assert_eq!(out.routed, connected, "routed count; failures {:?}", out.failures);
    }
    // Deterministic.
    let again = port(c, &lib, o);
    assert_eq!(write::circuit_to_string(&again.circuit), write::circuit_to_string(&out.circuit), "not deterministic");
    out
}

fn removed_wires(before: &Circuit, after: &Circuit, o: &RouteOptions) -> usize {
    if o.keep_wires {
        0
    } else {
        before.wires_in(o.to).count().min(before.wires.len() + after.wires.len())
    }
}

fn led_declared() -> Circuit {
    parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"B1","type":"battery","props":{"value":"9V"}},
          {"id":"R1","type":"resistor","props":{"value":"330"}},
          {"id":"D1","type":"led"}],
        "nets":[{"name":"VCC","pins":["B1.pos","R1.a"]},{"pins":["R1.b","D1.anode"]},{"name":"GND","pins":["D1.cathode","B1.neg"]}]}"#,
    )
    .unwrap()
}

fn part_mut<'a>(c: &'a mut Circuit, id: &str) -> &'a mut crate::model::Part {
    c.parts.iter_mut().find(|p| p.id == id).unwrap()
}

fn led_example() -> Circuit {
    parse_circuit(include_str!("../../tests/fixtures/v1/led.circ")).unwrap()
}

fn declared(o: View) -> RouteOptions {
    RouteOptions { to: o, source: Source::Declared, ..RouteOptions::default() }
}

/// Deterministic pseudo random numbers.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// `parts` random parts and up to `nets` nets of 2 or 3 pins, as declared nets.
fn stress(parts: usize, nets: usize, seed: u64) -> Circuit {
    let types: [(&str, &[&str]); 6] = [
        ("resistor", &["a", "b"]),
        ("led", &["anode", "cathode"]),
        ("capacitor", &["a", "b"]),
        ("diode", &["anode", "cathode"]),
        ("npn", &["base", "collector", "emitter"]),
        ("dip8", &["1", "2", "3", "4", "5", "6", "7", "8"]),
    ];
    let mut rng = Lcg(seed);
    let mut ps = Vec::new();
    let mut pins: Vec<String> = Vec::new();
    for i in 0..parts {
        let (ty, pn) = types[if i == 0 { 5 } else { (rng.next() % 5) as usize }];
        let id = format!("P{}", i + 1);
        ps.push(format!(r#"{{"id":"{id}","type":"{ty}"}}"#));
        for p in pn {
            pins.push(format!("{id}.{p}"));
        }
    }
    // Fisher-Yates.
    for i in (1..pins.len()).rev() {
        let j = (rng.next() as usize) % (i + 1);
        pins.swap(i, j);
    }
    let mut ns = Vec::new();
    let mut at = 0;
    for k in 0..nets {
        let size = if k % 3 == 2 { 3 } else { 2 };
        if at + size > pins.len() {
            break;
        }
        let list: Vec<String> = pins[at..at + size].iter().map(|p| format!("\"{p}\"")).collect();
        at += size;
        ns.push(format!(r#"{{"pins":[{}]}}"#, list.join(",")));
    }
    parse_circuit(&format!(
        r#"{{"format":"short-circ/2","parts":[{}],"nets":[{}]}}"#,
        ps.join(","),
        ns.join(",")
    ))
    .unwrap()
}

// ---- nets --------------------------------------------------------------------------------

#[test]
fn declared_nets_with_unknown_refs_fail_but_route_the_rest() {
    let mut c = led_declared();
    c.nets.push(crate::model::NetDecl { name: Some("X".into()), pins: vec!["R1.a".into(), "ZZ.q".into(), "D1.cathode".into()] });
    let out = checked(&c, &declared(View::Breadboard));
    assert!(out.failures.iter().any(|f| f.net == "X" && f.reason.contains("ZZ.q")), "{:?}", out.failures);
}

#[test]
fn source_nets_skip_unusable_pins() {
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[10,10]}},
          {"id":"G1","type":"ground","schematic":{"at":[30,10]}}],
        "nets":[{"pins":["R1.a","G1.gnd"]},{"pins":["R1.b","G1.gnd"]}]}"#,
    )
    .unwrap();
    let lib = Library::load(&c, None);
    let mut f = Vec::new();
    // ground has no breadboard view: nets shrink to one usable pin and vanish
    let n = nets::source_nets(&c, &lib, &declared(View::Breadboard), &mut f);
    assert!(n.is_empty(), "{n:?}");
}

// ---- schematic ---------------------------------------------------------------------------

#[test]
fn schematic_from_declared_nets_places_and_routes_led() {
    let out = checked(&led_declared(), &declared(View::Schematic));
    assert_eq!(out.placed.len(), 3);
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 3);
    assert!(!out.wires_added.is_empty());
    assert!(out.circuit.labels.is_empty(), "no label fallback needed");
}

#[test]
fn schematic_round_trip_from_breadboard() {
    // schematic -> breadboard, then drop the schematic and port the breadboard back.
    let bb = checked(&led_example(), &opts(View::Breadboard)).circuit;
    let mut c = bb.clone();
    c.wires.retain(|w| w.view == View::Breadboard);
    c.labels.clear();
    for p in &mut c.parts {
        p.schematic = None;
    }
    let out = checked(&c, &opts(View::Schematic));
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 3);
    // The new schematic has the same three nets as the original.
    let lib = Library::load(&out.circuit, None);
    let nl = Netlist::build(&out.circuit, &lib, View::Schematic);
    assert_eq!(nl.net_of_pin("B1", "pos"), nl.net_of_pin("R1", "a"));
    assert_eq!(nl.net_of_pin("R1", "b"), nl.net_of_pin("D1", "anode"));
    assert_eq!(nl.net_of_pin("D1", "cathode"), nl.net_of_pin("B1", "neg"));
    assert_ne!(nl.net_of_pin("R1", "a"), nl.net_of_pin("R1", "b"));
}

#[test]
fn placed_parts_never_overlap_and_existing_ones_stay() {
    let mut c = stress(10, 12, 7);
    // Pre-place two parts.
    c.parts[1].schematic = Some(crate::model::Placement::at(20, 20));
    c.parts[2].schematic = Some(crate::model::Placement::at(26, 20));
    let out = checked(&c, &declared(View::Schematic));
    assert_eq!(out.placed.len(), 8);
    assert!(!out.placed.contains(&"P2".to_string()));
}

#[test]
fn power_symbols_are_placed_beside_their_part() {
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[10,10]}},
          {"id":"R2","type":"resistor"},
          {"id":"G1","type":"ground"}],
        "nets":[{"pins":["R1.b","R2.a"]},{"pins":["R2.b","G1.gnd"]}]}"#,
    )
    .unwrap();
    let out = checked(&c, &declared(View::Schematic));
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    let lib = Library::load(&out.circuit, None);
    let nl = Netlist::build(&out.circuit, &lib, View::Schematic);
    assert_eq!(nl.net_of_pin("R2", "b"), nl.net_of_pin("G1", "gnd"));
    // G1 is within a few units of R2's pin b.
    let g = out.circuit.part("G1").unwrap().schematic.as_ref().unwrap().at;
    let r = out.circuit.part("R2").unwrap().schematic.as_ref().unwrap().at;
    assert!((g[0] - r[0]).abs() + (g[1] - r[1]).abs() < 25, "{g:?} vs {r:?}");
}

/// Every pin of every part placed in the schematic must be clear of foreign wires and labels:
/// not on a segment, not on a label point.
fn assert_pins_clear_of(c: &Circuit, wires: &[&str], labels: &[crate::model::Pt]) {
    let lib = Library::load(c, None);
    let nl = Netlist::build(c, &lib, View::Schematic);
    for p in &nl.geom.pins {
        for w in c.wires.iter().filter(|w| wires.contains(&w.id.as_str())) {
            for s in w.pts.windows(2) {
                assert!(!crate::netlist::on_segment(s[0], s[1], p.at), "{}.{} at {:?} lies on wire {}", p.part, p.pin, p.at, w.id);
            }
        }
        assert!(!labels.contains(&p.at), "{}.{} at {:?} lies on a label", p.part, p.pin, p.at);
    }
}

#[test]
fn placed_parts_keep_clear_of_existing_wires_and_labels() {
    // Regression: only part rectangles were obstacles, so R2 (first free spot: (10,20), pins at
    // (10,21) and (18,21)) landed on a wire below the lowest part and on a label.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[10,10]}},
          {"id":"R2","type":"resistor"},{"id":"G1","type":"ground"}],
        "wires":[{"id":"w1","pts":[[4,21],[24,21]]},{"id":"w2","pts":[[4,23],[4,30]]}],
        "labels":[{"at":[4,26],"name":"SIG"},{"at":[24,21],"name":"EDGE"}],
        "nets":[{"pins":["R1.b","R2.a"]},{"pins":["R2.b","G1.gnd"]}]}"#,
    )
    .unwrap();
    let out = checked(&c, &declared(View::Schematic));
    assert_pins_clear_of(&out.circuit, &["w1", "w2"], &[[4, 26], [24, 21]]);
    let r2 = out.circuit.part("R2").unwrap().schematic.as_ref().unwrap().at;
    assert_ne!(r2, [10, 20], "the spot on the wire is not used");
    // The existing wires and labels are untouched.
    assert_eq!(out.circuit.wires[0].pts, c.wires[0].pts);
    assert!(out.circuit.labels.iter().filter(|l| l.name == "SIG" || l.name == "EDGE").count() == 2);
    // A wire running through the places next to a pin keeps single-pin parts clear as well.
    let g = out.circuit.part("G1").unwrap().schematic.as_ref().unwrap().at;
    assert!(!on_any_segment(&out.circuit, g), "{g:?}");
}

fn on_any_segment(c: &Circuit, p: crate::model::Pt) -> bool {
    c.wires_in(View::Schematic).filter(|w| w.id == "w1" || w.id == "w2").any(|w| w.pts.windows(2).any(|s| crate::netlist::on_segment(s[0], s[1], p)))
}

#[test]
fn multi_pin_net_is_one_tree() {
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[10,10]}},
          {"id":"R2","type":"resistor","schematic":{"at":[10,20]}},
          {"id":"R3","type":"resistor","schematic":{"at":[10,30]}},
          {"id":"R4","type":"resistor","schematic":{"at":[30,20]}}],
        "nets":[{"pins":["R1.b","R2.b","R3.b","R4.a"]}]}"#,
    )
    .unwrap();
    let mut o = declared(View::Schematic);
    o.merge_wires = false;
    let out = checked(&c, &o);
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 1);
    assert!(out.wires_added.len() >= 3);
    // Merging joins the branches that end on the same pin into one wire each; the net is intact.
    let merged = checked(&c, &declared(View::Schematic));
    assert!(merged.failures.is_empty(), "{:?}", merged.failures);
    assert!(merged.wires_merged > 0 && merged.circuit.wires.len() < out.circuit.wires.len(), "{} vs {}", merged.circuit.wires.len(), out.circuit.wires.len());
}

#[test]
fn partial_routing_is_completed_and_kept() {
    // R1.b and R2.b are already wired; R3.b joins them (T-junction or pin end), the old wire stays.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[10,10]}},
          {"id":"R2","type":"resistor","schematic":{"at":[10,30]}},
          {"id":"R3","type":"resistor","schematic":{"at":[40,20]}}],
        "wires":[{"id":"old","pts":[[18,11],[24,11],[24,31],[18,31]]}],
        "nets":[{"pins":["R1.b","R2.b","R3.a"]}]}"#,
    )
    .unwrap();
    let out = checked(&c, &declared(View::Schematic));
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 1);
    assert_eq!(out.wires_added.len(), 1, "one new wire joins R3 to the existing tree");
    assert_eq!(out.circuit.wires[0].id, "old");
    assert_eq!(out.circuit.wires[0].pts, c.wires[0].pts);
    // Scope unrouted still sees the net as unrouted (R3.a is not connected yet) and completes it.
    let mut o = declared(View::Schematic);
    o.scope = Scope::Unrouted;
    assert_eq!(checked(&c, &o).wires_added.len(), 1);
}

#[test]
fn replace_removes_old_wires_and_unrouted_skips_connected_nets() {
    let c = led_example();
    // The example schematic is complete: scope unrouted adds nothing; the nets are in other view.
    let lib = Library::load(&c, None);
    let mut o = declared(View::Schematic);
    o.scope = Scope::Unrouted;
    let out = port(&c, &lib, &o);
    assert!(out.wires_added.is_empty());
    assert_eq!(out.circuit.wires.len(), c.wires.len());
    // With --replace the old wires are gone and routing starts over: still valid.
    let mut c2 = led_example();
    c2.nets = vec![
        crate::model::NetDecl { name: None, pins: vec!["B1.pos".into(), "R1.a".into()] },
        crate::model::NetDecl { name: None, pins: vec!["R1.b".into(), "D1.anode".into()] },
        crate::model::NetDecl { name: None, pins: vec!["D1.cathode".into(), "B1.neg".into()] },
    ];
    let mut o = declared(View::Schematic);
    o.keep_wires = false;
    let out = checked(&c2, &o);
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    // The example's R1.b sits on D1's bounding box (its own wire runs through the body), so
    // that net cannot be wired around the bodies and falls back to labels N1.
    assert_eq!(out.routed, 3);
    assert!(out.wires_added.len() >= 2, "{:?}", out.wires_added);
    assert_eq!(out.circuit.labels.iter().filter(|l| l.name == "N1").count(), 2, "{:?}", out.circuit.labels);
}

#[test]
fn only_scope_routes_just_the_named_nets() {
    let c = led_declared();
    let mut o = declared(View::Schematic);
    o.scope = Scope::Only(vec!["R1.b".into()]);
    let out = checked(&c, &o);
    assert_eq!(out.routed, 1);
    let lib = Library::load(&out.circuit, None);
    let nl = Netlist::build(&out.circuit, &lib, View::Schematic);
    assert_eq!(nl.net_of_pin("R1", "b"), nl.net_of_pin("D1", "anode"));
    assert_ne!(nl.net_of_pin("R1", "a"), nl.net_of_pin("B1", "pos"));
    // unknown reference is reported
    o.scope = Scope::Only(vec!["Q9.x".into()]);
    let out = port(&c, &lib, &o);
    assert!(out.failures.iter().any(|f| f.net == "Q9.x"));
}

/// A resistor R1 whose pin `a` is walled in by three more resistors (their clearance zones leave
/// no way out), and a far away partner.
fn enclosed_circuit() -> Circuit {
    parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[20,20]}},
          {"id":"W1","type":"resistor","schematic":{"at":[11,20],"rot":0}},
          {"id":"W2","type":"resistor","schematic":{"at":[11,17]}},
          {"id":"W3","type":"resistor","schematic":{"at":[11,23]}},
          {"id":"R2","type":"resistor","schematic":{"at":[50,50]}}],
        "nets":[{"name":"SIG","pins":["R1.a","R2.b"]}]}"#,
    )
    .unwrap()
}

#[test]
fn enclosed_pin_falls_back_to_labels_or_fails() {
    let c = enclosed_circuit();
    let out = checked(&c, &declared(View::Schematic));
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 1);
    assert_eq!(out.circuit.labels.len(), 2, "{:?}", out.circuit.labels);
    assert!(out.circuit.labels.iter().all(|l| l.name == "SIG"));
    let mut o = declared(View::Schematic);
    o.label_fallback = false;
    let out = checked(&c, &o);
    assert_eq!(out.failures.len(), 1);
    assert_eq!(out.failures[0].net, "SIG");
    assert!(out.failures[0].reason.contains("enclosed") || out.failures[0].reason.contains("no path"), "{:?}", out.failures);
    assert!(out.circuit.labels.is_empty());
}

#[test]
fn unnamed_failed_net_gets_a_fresh_label_name() {
    let mut c = enclosed_circuit();
    c.nets[0].name = None;
    c.labels.push(crate::model::Label { at: [60, 60], name: "N1".into(), color: None });
    let out = checked(&c, &declared(View::Schematic));
    let new: Vec<_> = out.circuit.labels.iter().filter(|l| l.at != [60, 60]).collect();
    assert_eq!(new.len(), 2);
    assert!(new.iter().all(|l| l.name == "N2"), "{new:?}");
}

#[test]
fn crossings_only_when_allowed() {
    // Two nets whose straight connections must cross: R1.b -> R4.a and R2.b -> R3.a laid out as
    // an X, boxed in so that detours are impossible without crossing.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[0,0],"rot":90}},
          {"id":"R2","type":"resistor","schematic":{"at":[40,0],"rot":90}},
          {"id":"R3","type":"resistor","schematic":{"at":[0,40],"rot":90}},
          {"id":"R4","type":"resistor","schematic":{"at":[40,40],"rot":90}}],
        "nets":[{"pins":["R1.b","R4.a"]},{"pins":["R2.b","R3.a"]}]}"#,
    )
    .unwrap();
    // Without crossings: planar detour (or labels): no wire-crossing at all.
    let lib = Library::load(&c, None);
    let out = checked(&c, &declared(View::Schematic));
    let rep = validate(&out.circuit, &Library::load(&out.circuit, None));
    assert!(!rep.issues.iter().any(|i| i.rule == "wire-crossing"), "{:?}", rep.issues);
    // With crossings allowed it still validates (crossings are warnings) and routes both nets.
    let mut o = declared(View::Schematic);
    o.allow_crossings = true;
    let out = checked(&c, &o);
    assert!(out.failures.is_empty());
    assert_eq!(out.routed, 2);
    let _ = lib;
}

#[test]
fn crossing_is_used_when_the_only_way() {
    // A long vertical wire (its own net) stands between R1 and R2. With crossings allowed the
    // new wire crosses it perpendicularly (one wire-crossing warning); without, it detours
    // around the wire's end and no crossing appears.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[0,10]}},
          {"id":"R2","type":"resistor","schematic":{"at":[30,10]}}],
        "wires":[{"id":"wall","pts":[[20,-60],[20,80]]}],
        "nets":[{"pins":["R1.b","R2.a"]}]}"#,
    )
    .unwrap();
    let crossings = |o: &RouteOptions| {
        let out = checked(&c, o);
        assert!(out.failures.is_empty(), "{:?}", out.failures);
        assert_eq!(out.routed, 1);
        assert!(out.circuit.labels.is_empty());
        let rep = validate(&out.circuit, &Library::load(&out.circuit, None));
        rep.issues.iter().filter(|i| i.rule == "wire-crossing").count()
    };
    let mut o = declared(View::Schematic);
    assert_eq!(crossings(&o), 0);
    o.allow_crossings = true;
    assert_eq!(crossings(&o), 1);
}

#[test]
fn dense_stress_circuits_validate_in_both_targets() {
    for seed in 1..=4u64 {
        let c = stress(12, 15, seed);
        let sch = checked(&c, &declared(View::Schematic));
        assert_eq!(sch.placed.len(), 12);
        assert!(!sch.wires_added.is_empty());
        let bb = checked(&c, &declared(View::Breadboard));
        assert!(bb.placed.len() >= 10, "seed {seed}: {:?}", bb.failures);
        // with crossings and length preference as well
        let mut o = declared(View::Schematic);
        o.allow_crossings = true;
        o.prefer = Prefer::Length;
        checked(&c, &o);
    }
}

#[test]
fn sixty_parts_eighty_nets_route() {
    // Debug builds are an order of magnitude slower: use a smaller circuit there.
    let (parts, nets) = if cfg!(debug_assertions) { (30, 40) } else { (60, 80) };
    let c = stress_big(parts, nets, 3);
    let lib = Library::load(&c, None);
    let t = std::time::Instant::now();
    let out = port(&c, &lib, &declared(View::Schematic));
    let took = t.elapsed();
    eprintln!("{parts} parts / {nets} nets: {took:?}, {} wires, {} failures, {} labels", out.wires_added.len(), out.failures.len(), out.circuit.labels.len());
    if !cfg!(debug_assertions) {
        assert!(took.as_secs_f64() < 5.0, "too slow: {took:?}");
    }
    checked(&c, &declared(View::Schematic));
    assert_eq!(out.placed.len(), parts);
}

/// Ladder-like circuit: part i connects to parts i+1..i+3 (local nets, like a real design).
fn stress_local(parts: usize, nets: usize, seed: u64) -> Circuit {
    let mut rng = Lcg(seed);
    let mut ps = Vec::new();
    for i in 0..parts {
        let ty = ["resistor", "capacitor", "led", "diode"][(rng.next() % 4) as usize];
        ps.push(format!(r#"{{"id":"P{}","type":"{ty}"}}"#, i + 1));
    }
    let pin = |i: usize, which: u64| {
        let ty_is_pol = ps[i].contains("led") || ps[i].contains("diode");
        let names = if ty_is_pol { ["anode", "cathode"] } else { ["a", "b"] };
        format!("P{}.{}", i + 1, names[which as usize])
    };
    let mut used = std::collections::HashSet::new();
    let mut ns = Vec::new();
    let mut tries = 0;
    while ns.len() < nets && tries < 2000 {
        tries += 1;
        let i = (rng.next() as usize) % parts;
        let j = i + 1 + (rng.next() as usize) % 3;
        if j >= parts {
            continue;
        }
        let (a, b) = (pin(i, rng.next() % 2), pin(j, rng.next() % 2));
        if used.contains(&a) || used.contains(&b) {
            continue;
        }
        used.insert(a.clone());
        used.insert(b.clone());
        ns.push(format!(r#"{{"pins":["{a}","{b}"]}}"#));
    }
    parse_circuit(&format!(r#"{{"format":"short-circ/2","parts":[{}],"nets":[{}]}}"#, ps.join(","), ns.join(","))).unwrap()
}

#[test]
fn local_nets_route_mostly_with_wires() {
    let c = stress_local(60, 55, 11);
    let t = std::time::Instant::now();
    let out = checked(&c, &declared(View::Schematic));
    let n = c.nets.len();
    eprintln!("local 60 parts / {n} nets: {:?}, {} wires, {} labels", t.elapsed(), out.wires_added.len(), out.circuit.labels.len());
    // Most nets are plain wires (labels come in pairs).
    assert!(out.circuit.labels.len() / 2 * 4 <= n, "too many label fallbacks: {}", out.circuit.labels.len());
}

/// Many two-pin parts, nets of 2..4 pins.
fn stress_big(parts: usize, nets: usize, seed: u64) -> Circuit {
    let mut rng = Lcg(seed);
    let mut ps = Vec::new();
    let mut pins: Vec<String> = Vec::new();
    for i in 0..parts {
        let ty = ["resistor", "capacitor", "led", "diode"][(rng.next() % 4) as usize];
        let id = format!("P{}", i + 1);
        ps.push(format!(r#"{{"id":"{id}","type":"{ty}"}}"#));
        let (a, b) = if ty == "led" || ty == "diode" { ("anode", "cathode") } else { ("a", "b") };
        pins.push(format!("{id}.{a}"));
        pins.push(format!("{id}.{b}"));
    }
    for i in (1..pins.len()).rev() {
        let j = (rng.next() as usize) % (i + 1);
        pins.swap(i, j);
    }
    let mut ns = Vec::new();
    let mut at = 0;
    for k in 0..nets {
        let size = 2 + (k % 3 == 2) as usize;
        if at + size > pins.len() {
            break;
        }
        let list: Vec<String> = pins[at..at + size].iter().map(|p| format!("\"{p}\"")).collect();
        at += size;
        ns.push(format!(r#"{{"pins":[{}]}}"#, list.join(",")));
    }
    parse_circuit(&format!(r#"{{"format":"short-circ/2","parts":[{}],"nets":[{}]}}"#, ps.join(","), ns.join(","))).unwrap()
}

// ---- breadboard --------------------------------------------------------------------------

#[test]
fn breadboard_from_led_example() {
    let c = led_example();
    let out = checked(&c, &opts(View::Breadboard));
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 3);
    // one half board was added at the origin
    let board = out.circuit.parts.iter().find(|p| p.ty == "breadboard_half").expect("board");
    assert_eq!(board.breadboard.as_ref().unwrap().at, [0, 0]);
    assert_eq!(out.placed.len(), 4);
    assert!(out.placed.contains(&board.id));
    // jumpers have ids j1.. and colours
    assert!(out.wires_added.iter().all(|w| w.starts_with('j')));
    assert!(out.circuit.wires.iter().filter(|w| w.view == View::Breadboard).all(|w| w.color.is_some()));
}

#[test]
fn breadboard_from_declared_nets() {
    let out = checked(&led_declared(), &declared(View::Breadboard));
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 3);
}

#[test]
fn existing_board_is_used_and_no_second_board_added() {
    let mut c = led_declared();
    c.parts.push(crate::model::Part {
        id: "MY".into(),
        ty: "breadboard_mini".into(),
        breadboard: Some(crate::model::Placement::at(5, 5)),
        ..Default::default()
    });
    let out = checked(&c, &declared(View::Breadboard));
    assert_eq!(out.circuit.parts.iter().filter(|p| p.ty.starts_with("breadboard")).count(), 1);
    assert!(out.failures.is_empty(), "{:?}", out.failures);
}

#[test]
fn dip_straddles_the_centre_gap_and_resistor_bridges_strips() {
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"U1","type":"dip8"},{"id":"R1","type":"resistor"}],
        "nets":[{"pins":["U1.1","R1.a"]},{"pins":["U1.8","R1.b"]}]}"#,
    )
    .unwrap();
    let out = checked(&c, &declared(View::Breadboard));
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    let lib = Library::load(&out.circuit, None);
    let nl = Netlist::build(&out.circuit, &lib, View::Breadboard);
    // every pin of U1 sits in its own strip: the 8 nets are distinct
    let mut strips: Vec<Option<usize>> = (1..=8).map(|k| nl.net_of_pin("U1", &k.to_string())).collect();
    strips.sort();
    strips.dedup();
    assert_eq!(strips.len(), 8);
    assert_ne!(nl.net_of_pin("R1", "a"), nl.net_of_pin("R1", "b"));
    // IC pins 1 and 8 are on opposite sides of the gap: pins 1 (bottom row) and 8 (top row) are 3 apart
    let u = out.circuit.part("U1").unwrap().breadboard.as_ref().unwrap();
    let def = lib.get("dip8").unwrap();
    let p1 = crate::transform::pin_world(u, &def.breadboard.as_ref().unwrap().pins["1"]);
    let p8 = crate::transform::pin_world(u, &def.breadboard.as_ref().unwrap().pins["8"]);
    assert_eq!((p1[0] - p8[0]).abs().max((p1[1] - p8[1]).abs()), 3);
}

#[test]
fn nets_sharing_a_strip_get_no_jumper() {
    // R1.b - R2.a: the best placement puts both legs in one strip.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[{"id":"R1","type":"resistor"},{"id":"R2","type":"resistor"}],
        "nets":[{"pins":["R1.b","R2.a"]}]}"#,
    )
    .unwrap();
    let out = checked(&c, &declared(View::Breadboard));
    assert!(out.wires_added.is_empty(), "{:?}", out.wires_added);
    assert_eq!(out.routed, 1);
}

#[test]
fn bb_stress_placement_has_no_shorts() {
    for seed in 1..=5u64 {
        let c = stress(12, 15, seed);
        let out = checked(&c, &declared(View::Breadboard));
        // no strip holds two different declared nets (checked via net-merged by `checked`, and
        // explicitly: every breadboard net holds pins of one declared net)
        let lib = Library::load(&out.circuit, None);
        let nl = Netlist::build(&out.circuit, &lib, View::Breadboard);
        for net in &nl.nets {
            let mut decl: Vec<usize> = Vec::new();
            for p in &net.pins {
                if let Some(i) = c.nets.iter().position(|d| d.pins.contains(&format!("{}.{}", p.part, p.pin))) {
                    if !decl.contains(&i) {
                        decl.push(i);
                    }
                }
            }
            assert!(decl.len() <= 1, "seed {seed}: short between declared nets {decl:?}");
            // pins of unnetted pins never share a strip with another part pin of a different net
            let non_board: Vec<_> = net.pins.iter().filter(|p| !p.part.starts_with("BB")).collect();
            let unnetted = non_board.iter().filter(|p| !c.nets.iter().any(|d| d.pins.contains(&format!("{}.{}", p.part, p.pin)))).count();
            assert!(unnetted == 0 || non_board.len() == 1, "seed {seed}: neutral pin shares a strip: {non_board:?}");
        }
    }
}

#[test]
fn jumper_styles_end_on_holes_and_are_valid() {
    for style in [JumperStyle::Straight, JumperStyle::Orthogonal, JumperStyle::Diagonal45] {
        for seed in 1..=3u64 {
            let c = stress(8, 10, seed);
            let mut o = declared(View::Breadboard);
            o.jumper_style = style.clone();
            // The shapes of the styles; detours around other wires are tested separately.
            o.allow_crossings = true;
            o.avoid_parts = false;
            let out = checked(&c, &o);
            let lib = Library::load(&out.circuit, None);
            let nl = Netlist::build(&out.circuit, &lib, View::Breadboard);
            for w in out.circuit.wires_in(View::Breadboard) {
                // both ends touch a pin (hole or leg)
                for end in [w.pts[0], *w.pts.last().unwrap()] {
                    assert!(nl.net_at(end).is_some_and(|n| !nl.nets[n].pins.is_empty()), "{style:?}: {} end {end:?}", w.id);
                }
                match style {
                    JumperStyle::Straight => assert_eq!(w.pts.len(), 2),
                    JumperStyle::Orthogonal => assert!(w.pts.len() <= 3),
                    JumperStyle::Diagonal45 => {
                        assert!(w.pts.len() <= 3);
                        for s in w.pts.windows(2) {
                            let (dx, dy) = ((s[1][0] - s[0][0]).abs(), (s[1][1] - s[0][1]).abs());
                            assert!(dx == 0 || dy == 0 || dx == dy, "{:?}", w.pts);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn jumper_colours() {
    let c = stress(8, 10, 2);
    let mut o = declared(View::Breadboard);
    o.jumper_color = JumperColor::Single("magenta".into());
    let out = checked(&c, &o);
    assert!(out.circuit.wires.iter().all(|w| w.color.as_deref() == Some("magenta")));
    let out = checked(&c, &declared(View::Breadboard));
    let cols: std::collections::BTreeSet<_> = out.circuit.wires.iter().filter_map(|w| w.color.clone()).collect();
    assert!(cols.len() >= 2 || out.wires_added.len() < 2, "{cols:?}");
    assert_eq!(out.circuit.wires.first().and_then(|w| w.color.as_deref()), Some("red"));
}

/// Pairs of breadboard wires of `c` that touch or cross anywhere except at a shared end point.
fn crossing_jumpers(c: &Circuit) -> Vec<(String, String)> {
    fn orient(a: crate::model::Pt, b: crate::model::Pt, p: crate::model::Pt) -> i64 {
        ((b[0] - a[0]) as i64 * (p[1] - a[1]) as i64 - (b[1] - a[1]) as i64 * (p[0] - a[0]) as i64).signum()
    }
    let wires: Vec<&crate::model::Wire> = c.wires_in(View::Breadboard).collect();
    let mut out = Vec::new();
    for (i, w) in wires.iter().enumerate() {
        for v in &wires[i + 1..] {
            let hit = w.pts.windows(2).any(|s| {
                v.pts.windows(2).any(|t| {
                    let (d1, d2) = (orient(t[0], t[1], s[0]) * orient(t[0], t[1], s[1]), orient(s[0], s[1], t[0]) * orient(s[0], s[1], t[1]));
                    let proper = d1 < 0 && d2 < 0;
                    let on = |a: crate::model::Pt, b: crate::model::Pt, p: crate::model::Pt| {
                        orient(a, b, p) == 0
                            && p[0] >= a[0].min(b[0])
                            && p[0] <= a[0].max(b[0])
                            && p[1] >= a[1].min(b[1])
                            && p[1] <= a[1].max(b[1])
                    };
                    let touch = [(t[0], s), (t[1], s)].iter().any(|(p, s)| on(s[0], s[1], *p) && ![s[0], s[1]].contains(p))
                        || [(s[0], t), (s[1], t)].iter().any(|(p, t)| on(t[0], t[1], *p) && ![t[0], t[1]].contains(p));
                    proper || touch
                })
            });
            if hit {
                out.push((w.id.clone(), v.id.clone()));
            }
        }
    }
    out
}

#[test]
fn breadboard_jumpers_cross_only_when_allowed() {
    let mut crossed_when_allowed = 0;
    for seed in 1..=2u64 {
        let c = stress(10, 14, seed);
        for style in [JumperStyle::Straight, JumperStyle::Orthogonal, JumperStyle::Diagonal45] {
            let mut o = declared(View::Breadboard);
            o.jumper_style = style.clone();
            assert!(!o.allow_crossings, "off by default");
            let out = checked(&c, &o);
            let wires: Vec<_> = out.circuit.wires_in(View::Breadboard).map(|w| (w.id.clone(), w.pts.clone())).collect();
            assert_eq!(crossing_jumpers(&out.circuit), vec![], "seed {seed} {style:?}: jumpers cross; failures {:?}; {wires:?}", out.failures);
            o.allow_crossings = true;
            let free = checked(&c, &o);
            crossed_when_allowed += crossing_jumpers(&free.circuit).len();
        }
    }
    assert!(crossed_when_allowed > 0, "the stress circuits never cross, so the test proves nothing");
}

/// Bounding boxes of the non-board parts placed in the breadboard view of `c`.
fn bb_boxes(c: &Circuit, lib: &Library) -> Vec<(String, [f64; 2], [f64; 2])> {
    c.parts
        .iter()
        .filter_map(|p| {
            let (pl, def) = (p.breadboard.as_ref()?, lib.get(&p.ty)?);
            if def.is_board() {
                return None;
            }
            let (lo, hi) = crate::transform::world_bbox(pl, def.breadboard.as_ref()?);
            Some((p.id.clone(), lo, hi))
        })
        .collect()
}

/// Length of the segment inside the open interior of the box (sampled every 1/8 unit).
fn seg_inside_len(a: crate::model::Pt, b: crate::model::Pt, lo: [f64; 2], hi: [f64; 2]) -> f64 {
    let len = (((b[0] - a[0]) as f64).powi(2) + ((b[1] - a[1]) as f64).powi(2)).sqrt();
    let n = ((len * 8.0).ceil() as i32).max(1);
    let inside = (0..n)
        .filter(|k| {
            let t = (*k as f64 + 0.5) / n as f64;
            let (x, y) = (a[0] as f64 + (b[0] - a[0]) as f64 * t, a[1] as f64 + (b[1] - a[1]) as f64 * t);
            x > lo[0] + 1e-9 && x < hi[0] - 1e-9 && y > lo[1] + 1e-9 && y < hi[1] - 1e-9
        })
        .count();
    len * inside as f64 / n as f64
}

fn seg_through(a: crate::model::Pt, b: crate::model::Pt, lo: [f64; 2], hi: [f64; 2]) -> bool {
    seg_inside_len(a, b, lo, hi) > 0.0
}

#[test]
fn orthogonal_jumpers_are_orthogonal_whatever_the_other_options_say() {
    for seed in 1..=2u64 {
        let c = stress(10, 14, seed);
        for crossings in [false, true] {
            for avoid in [false, true] {
                let mut o = declared(View::Breadboard);
                o.allow_crossings = crossings;
                o.avoid_parts = avoid;
                let out = checked(&c, &o);
                for w in out.circuit.wires_in(View::Breadboard) {
                    assert!(
                        w.pts.windows(2).all(|s| s[0][0] == s[1][0] || s[0][1] == s[1][1]),
                        "seed {seed} crossings {crossings} avoid {avoid}: {} {:?}",
                        w.id,
                        w.pts
                    );
                }
            }
        }
    }
}

#[test]
fn jumpers_keep_off_components_unless_allowed() {
    let mut over_when_allowed = 0;
    for seed in 1..=3u64 {
        let c = stress(10, 14, seed);
        let mut o = declared(View::Breadboard);
        o.allow_crossings = true;
        let out = checked(&c, &o);
        let lib = Library::load(&out.circuit, None);
        let boxes = bb_boxes(&out.circuit, &lib);
        let nl = Netlist::build(&out.circuit, &lib, View::Breadboard);
        let _ = &nl;
        for w in out.circuit.wires_in(View::Breadboard) {
            let ends = [w.pts[0], *w.pts.last().unwrap()];
            for (id, lo, hi) in &boxes {
                let covers = |p: crate::model::Pt| p[0] as f64 > lo[0] && (p[0] as f64) < hi[0] && p[1] as f64 > lo[1] && (p[1] as f64) < hi[1];
                // A component that covers an end may hold the jumper only for the shortest way out.
                let exit = |p: crate::model::Pt| {
                    let (x, y) = (p[0] as f64, p[1] as f64);
                    (x - lo[0]).min(hi[0] - x).min(y - lo[1]).min(hi[1] - y)
                };
                // (A walled-in end may have to leave by a second side: a little slack for that.)
                let held = ends.iter().filter(|e| covers(**e)).map(|e| exit(*e)).sum::<f64>();
                let allowed = if held > 0.0 { held + 3.0 } else { 0.01 };
                let inside: f64 = w.pts.windows(2).map(|s| seg_inside_len(s[0], s[1], *lo, *hi)).sum();
                assert!(inside <= allowed, "seed {seed}: {} runs {inside} units inside {id} (allowed {allowed}): {:?}", w.id, w.pts);
            }
        }
        // And the switch is real: allowing wires over components never routes fewer nets.
        o.avoid_parts = false;
        let free = checked(&c, &o);
        assert!(free.routed >= out.routed, "seed {seed}: {} vs {}", free.routed, out.routed);
        let lib = Library::load(&free.circuit, None);
        let boxes = bb_boxes(&free.circuit, &lib);
        over_when_allowed += free
            .circuit
            .wires_in(View::Breadboard)
            .filter(|w| boxes.iter().any(|(_, lo, hi)| w.pts.windows(2).any(|s| seg_through(s[0], s[1], *lo, *hi))))
            .count();
    }
    assert!(over_when_allowed > 0, "no stress circuit ever puts a jumper over a component, so the test proves nothing");
}

#[test]
fn placed_components_move_only_when_allowed() {
    // Everything placed once; then R1 is moved off the board by hand.
    let base = checked(&led_declared(), &declared(View::Breadboard)).circuit;
    let mut c = base.clone();
    let far = crate::model::Placement::at(60, 60);
    part_mut(&mut c, "R1").breadboard = Some(far.clone());
    let lib = Library::load(&c, None);

    let kept = port(&c, &lib, &declared(View::Breadboard));
    assert_eq!(kept.circuit.part("R1").unwrap().breadboard.as_ref(), Some(&far), "never moved by default");

    let mut o = declared(View::Breadboard);
    o.move_parts = true;
    let moved = checked_moved(&c, &o);
    assert!(moved.failures.is_empty(), "{:?}", moved.failures);
    assert_ne!(moved.circuit.part("R1").unwrap().breadboard.as_ref(), Some(&far), "R1 was placed again, on the board");
    assert_eq!(moved.routed, 3);
    // Components that were placed are placed again: all three non-board parts are listed.
    for id in ["B1", "R1", "D1"] {
        assert!(moved.placed.iter().any(|p| p == id), "{id} in {:?}", moved.placed);
    }
    // Without place_missing nothing can be placed again, so nothing is lifted.
    o.place_missing = false;
    let still = port(&c, &lib, &o);
    assert_eq!(still.circuit.part("R1").unwrap().breadboard.as_ref(), Some(&far));
}

/// Like `checked` for runs that move parts: the guarantees that do not mention fixed placements.
fn checked_moved(c: &Circuit, o: &RouteOptions) -> RouteOutcome {
    let lib = Library::load(c, None);
    let out = port(c, &lib, o);
    let lib2 = Library::load(&out.circuit, None);
    let report = validate(&out.circuit, &lib2);
    assert_eq!(errors(&report), BTreeMap::new(), "{:#?}", report.issues);
    assert_eq!(view_data(c, o.to.other()), view_data(&out.circuit, o.to.other()), "other view changed");
    out
}

#[test]
fn autoroute_joins_wires_that_meet_end_to_end() {
    // R1.b - R2.a is wired in two separate pieces that meet end to end at (24,11).
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor","schematic":{"at":[10,10]}},
          {"id":"R2","type":"resistor","schematic":{"at":[30,10]}},
          {"id":"R3","type":"resistor","schematic":{"at":[50,10]}}],
        "wires":[{"id":"w1","view":"schematic","pts":[[18,11],[24,11]]},
                 {"id":"w2","view":"schematic","pts":[[24,11],[30,11]]}],
        "nets":[{"pins":["R1.b","R2.a","R3.a"]}]}"#,
    )
    .unwrap();
    let mut o = declared(View::Schematic);
    o.merge_wires = false;
    let apart = checked(&c, &o);
    assert_eq!(apart.wires_merged, 0);
    let joined = checked(&c, &declared(View::Schematic));
    assert!(joined.failures.is_empty(), "{:?}", joined.failures);
    assert!(joined.wires_merged > 0, "wires: {:?}", apart.circuit.wires);
    assert!(joined.circuit.wires.len() < apart.circuit.wires.len());
    assert_eq!(joined.circuit.wires.iter().filter(|w| w.pts.contains(&[18, 11]) && w.pts.contains(&[30, 11])).count(), 1, "one wire from R1.b to R2.a");
    // Same connectivity.
    let lib = Library::load(&joined.circuit, None);
    let nl = Netlist::build(&joined.circuit, &lib, View::Schematic);
    assert_eq!(nl.net_of_pin("R1", "b"), nl.net_of_pin("R3", "a"));
    assert_eq!(nl.net_of_pin("R1", "b"), nl.net_of_pin("R2", "a"));
    // A second run changes nothing.
    let lib2 = Library::load(&joined.circuit, None);
    let again = port(&joined.circuit, &lib2, &declared(View::Schematic));
    assert_eq!(write::circuit_to_string(&again.circuit), write::circuit_to_string(&joined.circuit));
}

#[test]
fn a_jumper_that_cannot_avoid_the_wires_is_reported_not_drawn_across() {
    let mut c = led_declared();
    c.parts.push(crate::model::Part {
        id: "BB1".into(),
        ty: "breadboard_mini".into(),
        breadboard: Some(crate::model::Placement::at(0, 0)),
        ..Default::default()
    });
    let lib = Library::load(&c, None);
    // Without crossings every outcome is crossing-free; whatever could not be joined is listed.
    let out = port(&c, &lib, &declared(View::Breadboard));
    assert_eq!(crossing_jumpers(&out.circuit), vec![]);
    for f in &out.failures {
        assert!(!f.reason.is_empty());
    }
}

#[test]
fn max_jumper_length_is_honoured() {
    // Pins 1 and 4 of a DIP on the same board sit in different strips; with a tiny limit the
    // jumper cannot be drawn.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[{"id":"U1","type":"dip8"}],
        "nets":[{"name":"A","pins":["U1.1","U1.4"]}]}"#,
    )
    .unwrap();
    let mut o = declared(View::Breadboard);
    o.max_jumper_len = Some(1);
    let out = checked(&c, &o);
    assert_eq!(out.failures.len(), 1, "{:?}", out.failures);
    assert!(out.failures[0].reason.contains("too long"), "{:?}", out.failures);
    assert_eq!(out.routed, 0);
    o.max_jumper_len = Some(40);
    let out = checked(&c, &o);
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 1);
}

#[test]
fn no_place_routes_only_existing_pins() {
    let c = led_example();
    let mut o = opts(View::Breadboard);
    o.place_missing = false;
    let out = checked(&c, &o);
    assert!(out.placed.is_empty());
    assert!(out.wires_added.is_empty());
    assert!(out.circuit.parts.iter().all(|p| p.breadboard.is_none()));
}

#[test]
fn unplaced_pins_are_reported_and_their_nets_are_not_counted_as_routed() {
    // Regression: nets with two or more usable pins but fewer than two placed ones were skipped
    // silently, and a 3-pin net with two placed pins counted as routed.
    let placed_bb = checked(&led_example(), &opts(View::Breadboard)).circuit;
    let mut c = placed_bb.clone();
    part_mut(&mut c, "D1").breadboard = None;
    let mut o = opts(View::Breadboard);
    o.place_missing = false;
    let out = checked(&c, &o);
    let reasons: Vec<(&str, &str)> = out.failures.iter().map(|f| (f.net.as_str(), f.reason.as_str())).collect();
    assert_eq!(out.failures.len(), 2, "{reasons:?}");
    assert!(out.failures.iter().any(|f| f.reason == "D1.anode is not placed in the breadboard view"), "{reasons:?}");
    assert!(out.failures.iter().any(|f| f.reason == "D1.cathode is not placed in the breadboard view"), "{reasons:?}");
    assert_eq!(out.routed, 1, "only the B1-R1 net is complete");

    // The same in the schematic.
    let mut c = led_declared();
    part_mut(&mut c, "B1").schematic = Some(crate::model::Placement::at(10, 10));
    part_mut(&mut c, "R1").schematic = Some(crate::model::Placement::at(30, 10));
    let mut o = declared(View::Schematic);
    o.place_missing = false;
    let out = checked(&c, &o);
    assert_eq!(out.failures.len(), 2, "{:?}", out.failures);
    assert!(out.failures.iter().all(|f| f.reason.starts_with("D1.") && f.reason.ends_with(" is not placed in the schematic view")), "{:?}", out.failures);
    assert_eq!(out.routed, 1);

    // A 3-pin net whose third part cannot be placed (reported once, by part): not routed.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"MY","type":"breadboard_mini","breadboard":{"at":[0,0]}},
          {"id":"U1","type":"dip40"},{"id":"R1","type":"resistor"},{"id":"R2","type":"resistor"}],
        "nets":[{"name":"N","pins":["R1.b","R2.a","U1.1"]}]}"#,
    )
    .unwrap();
    let out = checked(&c, &declared(View::Breadboard));
    assert_eq!(out.failures.len(), 1, "{:?}", out.failures);
    assert!(out.failures[0].net == "U1" && out.failures[0].reason.contains("no free spot"));
    assert_eq!(out.routed, 0, "two of three pins placed is not a routed net");
}

/// R1 and R2 stand loose in the breadboard view (no board yet) with their legs exactly where the
/// holes of a half board at the origin would be: a strip would join R1.a and R2.a, which belong
/// to different nets. R3 is missing, so the run has to add a board.
fn loose_legs_circuit(lib: &Library) -> Circuit {
    let hole = |name: &str| lib.get("breadboard_half").unwrap().breadboard.as_ref().unwrap().pins[name].at;
    let (a3, c3) = (hole("a3"), hole("c3"));
    let mut c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"R1","type":"resistor"},{"id":"R2","type":"resistor"},{"id":"R3","type":"resistor"}],
        "nets":[{"name":"N1","pins":["R1.a","R3.a"]},{"name":"N2","pins":["R2.a","R3.b"]}]}"#,
    )
    .unwrap();
    // The resistor's pin a is at local (0,1).
    part_mut(&mut c, "R1").breadboard = Some(crate::model::Placement::at(a3[0], a3[1] - 1));
    part_mut(&mut c, "R2").breadboard = Some(crate::model::Placement::at(c3[0], c3[1] - 1));
    // Placed in both views, so the cross-view check compares their nets.
    part_mut(&mut c, "R1").schematic = Some(crate::model::Placement::at(10, 10));
    part_mut(&mut c, "R2").schematic = Some(crate::model::Placement::at(10, 30));
    c
}

#[test]
fn a_new_board_keeps_clear_of_loose_legs() {
    // Regression: the board was always added at (0,0), so legs that already stood on its holes
    // were joined by the strips (net-merged errors in the output).
    let lib = Library::builtin();
    let c = loose_legs_circuit(&lib);
    let before = validate(&c, &lib);
    assert!(!before.issues.iter().any(|i| i.rule == "net-merged"), "{:?}", before.issues);
    let out = checked(&c, &declared(View::Breadboard));
    let board = out.circuit.parts.iter().find(|p| p.ty == "breadboard_half").expect("a board was added");
    let at = board.breadboard.as_ref().unwrap().at;
    assert_ne!(at, [0, 0], "the first position would short the loose legs");
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(out.routed, 2);
    let report = validate(&out.circuit, &Library::load(&out.circuit, None));
    assert!(!report.issues.iter().any(|i| i.rule == "net-merged"), "{:?}", report.issues);
    // The legs stay where they were.
    assert_eq!(out.circuit.part("R1").unwrap().breadboard, c.part("R1").unwrap().breadboard);
    // Without loose legs the board still goes to the origin.
    let out = checked(&led_declared(), &declared(View::Breadboard));
    let board = out.circuit.parts.iter().find(|p| p.ty == "breadboard_half").unwrap();
    assert_eq!(board.breadboard.as_ref().unwrap().at, [0, 0]);
}

#[test]
fn second_run_is_a_no_op() {
    let c = led_example();
    let first = checked(&c, &opts(View::Breadboard));
    let mut o = opts(View::Breadboard);
    o.scope = Scope::Unrouted;
    let second = checked(&first.circuit, &o);
    assert!(second.placed.is_empty());
    assert!(second.wires_added.is_empty());
    assert_eq!(write::circuit_to_string(&second.circuit), write::circuit_to_string(&first.circuit));
    // and with scope all nothing new gets drawn either
    let third = checked(&first.circuit, &opts(View::Breadboard));
    assert!(third.wires_added.is_empty());
    assert_eq!(third.routed, 3);
}

#[test]
fn part_that_does_not_fit_is_reported() {
    // A mini board has 17 columns: a 40 pin DIP cannot straddle it.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"MY","type":"breadboard_mini","breadboard":{"at":[0,0]}},
          {"id":"U1","type":"dip40"},{"id":"R1","type":"resistor"}],
        "nets":[{"pins":["U1.1","R1.a"]}]}"#,
    )
    .unwrap();
    let out = checked(&c, &declared(View::Breadboard));
    assert!(out.failures.iter().any(|f| f.net == "U1" && f.reason.contains("no free spot")), "{:?}", out.failures);
    assert!(out.placed.contains(&"R1".to_string()));
}

// ---- examples ----------------------------------------------------------------------------

#[test]
fn examples_port_in_both_directions_without_new_errors() {
    for name in ["led", "colors", "bad"] {
        let src = std::fs::read_to_string(format!("{}/../../examples/{name}.circ", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let c = parse_circuit(&src).unwrap();
        checked(&c, &opts(View::Breadboard));
        let mut o = opts(View::Schematic);
        o.keep_wires = false;
        checked(&c, &o);
        // and through declared nets of the schematic
        let mut d = c.clone();
        d.nets = vec![];
        checked(&d, &opts(View::Breadboard));
    }
}

#[test]
fn fresh_ids_follow_edit_fresh_id() {
    let out = checked(&led_declared(), &declared(View::Schematic));
    let mut c = out.circuit.clone();
    let w = fresh_id(&c, "w");
    assert!(!c.wires.iter().any(|x| x.id == w));
    c.wires.clear();
    assert_eq!(fresh_id(&c, "w"), "w1");
}

fn astable_555() -> Circuit {
    parse_circuit(
        r#"{"format":"short-circ/2","parts":[
          {"id":"U1","type":"dip8"},{"id":"R1","type":"resistor"},{"id":"R2","type":"resistor"},
          {"id":"R3","type":"resistor"},{"id":"C1","type":"capacitor"},{"id":"C2","type":"capacitor"},
          {"id":"B1","type":"battery"},{"id":"D1","type":"led"}],
        "nets":[
          {"name":"GND","pins":["U1.1","C1.b","C2.b","B1.neg","D1.cathode"]},
          {"name":"VCC","pins":["U1.8","U1.4","R1.a","B1.pos"]},
          {"name":"DIS","pins":["U1.7","R1.b","R2.a"]},
          {"name":"TRIG","pins":["U1.2","U1.6","R2.b","C1.a"]},
          {"name":"CV","pins":["U1.5","C2.a"]},
          {"name":"OUT","pins":["U1.3","R3.a"]},
          {"name":"LED","pins":["R3.b","D1.anode"]}]}"#,
    )
    .unwrap()
}

#[test]
fn astable_555_ports_to_both_views() {
    let c = astable_555();
    let sch = checked(&c, &declared(View::Schematic));
    eprintln!("555 schematic: wires {} labels {} failures {:?}", sch.wires_added.len(), sch.circuit.labels.len(), sch.failures);
    assert!(sch.failures.is_empty());
    // Some nets are wired, the rest (planar-impossible around the IC) get labels.
    assert!(sch.wires_added.len() >= 4, "{}", sch.wires_added.len());
    // With crossings and wires over components allowed every net can be joined.
    let mut permissive = declared(View::Breadboard);
    permissive.allow_crossings = true;
    permissive.avoid_parts = false;
    let bb = checked(&c, &permissive);
    eprintln!("555 breadboard: jumpers {} failures {:?} placed {}", bb.wires_added.len(), bb.failures, bb.placed.len());
    assert!(bb.failures.is_empty(), "{:?}", bb.failures);
    // With the strict defaults some nets may not fit; each one is reported with a reason, and
    // the wires that are drawn keep to the rules.
    let strict = checked(&c, &declared(View::Breadboard));
    assert!(strict.failures.iter().all(|f| f.reason.contains("would cross") || f.reason.contains("free hole")), "{:?}", strict.failures);
    assert!(strict.routed + strict.failures.len() >= bb.routed, "{} + {} vs {}", strict.routed, strict.failures.len(), bb.routed);
}
