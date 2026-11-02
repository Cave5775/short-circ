//! Two views: breadboard connectivity with a hand-built board, and the cross-view / per-view
//! validation rules, each triggered and not triggered.
//!
//! Test parts (all inline `lib`, so no file access and nothing depends on the shipped parts):
//! - `board`: a breadboard-only carrier with three vertical strips of three holes each,
//!   strip A = holes a0..a2 at x=0, B = b0..b2 at x=2, C = c0..c2 at x=4 (y = 0..2), one bus
//!   per strip;
//! - `r2`: a two-pin part with both views (schematic 4x2, pins a(0,1) b(4,1); breadboard 2x1,
//!   pins a(0,0) b(2,0));
//! - `led2`: like `r2` with pins anode / cathode;
//! - `sym`: a one-pin schematic-only part.

use short_circ_core::{
    parse_circuit, validate, validate_view, Circuit, Issue, Library, Netlist, Report, Severity, View,
};

const LIB: &str = r##"[
  {"type":"board","name":"Test board","category":"board",
   "pins":[{"id":"a0"},{"id":"a1"},{"id":"a2"},{"id":"b0"},{"id":"b1"},{"id":"b2"},{"id":"c0"},{"id":"c1"},{"id":"c2"}],
   "buses":[["a0","a1","a2"],["b0","b1","b2"],["c0","c1","c2"]],
   "breadboard":{"size":[5,3],"pins":{
      "a0":{"at":[0,0]},"a1":{"at":[0,1]},"a2":{"at":[0,2]},
      "b0":{"at":[2,0]},"b1":{"at":[2,1]},"b2":{"at":[2,2]},
      "c0":{"at":[4,0]},"c1":{"at":[4,1]},"c2":{"at":[4,2]}},
      "svg":"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 50 30\"><rect width=\"50\" height=\"30\" fill=\"#e8e0c8\"/></svg>"}},
  {"type":"r2","name":"Two pin","pins":[{"id":"a"},{"id":"b"}],
   "schematic":{"size":[4,2],"pins":{"a":{"at":[0,1],"dir":"left"},"b":{"at":[4,1],"dir":"right"}},
      "shapes":[{"k":"rect","at":[0.5,0],"size":[3,2]}]},
   "breadboard":{"size":[2,1],"pins":{"a":{"at":[0,0]},"b":{"at":[2,0]}},"shapes":[{"k":"line","pts":[[0,0],[2,0]]}]}},
  {"type":"led2","name":"Two pin LED","pins":[{"id":"anode"},{"id":"cathode"}],
   "schematic":{"size":[4,2],"pins":{"anode":{"at":[0,1],"dir":"left"},"cathode":{"at":[4,1],"dir":"right"}},
      "shapes":[{"k":"rect","at":[0.5,0],"size":[3,2]}]},
   "breadboard":{"size":[2,1],"pins":{"anode":{"at":[0,0]},"cathode":{"at":[2,0]}},"shapes":[{"k":"line","pts":[[0,0],[2,0]]}]}},
  {"type":"sym","name":"Symbol only","pins":[{"id":"p"}],
   "schematic":{"size":[2,2],"pins":{"p":{"at":[0,1],"dir":"left"}},"shapes":[{"k":"circle","at":[1,1],"r":0.5}]}}
]"##;

fn circuit(parts: &str, wires: &str, extra: &str) -> Circuit {
    let src = format!(
        r#"{{"format":"short-circ/2","lib":{LIB},"parts":[{parts}],"wires":[{wires}]{extra}}}"#
    );
    parse_circuit(&src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn run(c: &Circuit) -> Report {
    validate(c, &Library::load(c, None))
}

fn of<'a>(r: &'a Report, rule: &str) -> Vec<&'a Issue> {
    r.issues.iter().filter(|i| i.rule == rule).collect()
}

fn board(at: &str) -> String {
    format!(r#"{{"id":"B1","type":"board","breadboard":{{"at":{at}}}}}"#)
}

// ---- hand-built breadboard -------------------------------------------------------------

/// Resistor R1 across strips A and B, LED D1 across strips B and C, jumper A - C.
fn loop_on_board(extra_wires: &str) -> Circuit {
    let parts = format!(
        r#"{},
        {{"id":"R1","type":"r2","props":{{"value":"330"}},"breadboard":{{"at":[0,1]}}}},
        {{"id":"D1","type":"led2","breadboard":{{"at":[2,2]}}}}"#,
        board("[0,0]")
    );
    let wires = format!(
        r#"{{"id":"j1","view":"breadboard","pts":[[4,0],[4,-2],[0,-2],[0,0]],"color":"red"}}{extra_wires}"#
    );
    circuit(&parts, &wires, "")
}

#[test]
fn resistor_led_and_jumper_on_a_board_make_two_nets() {
    let c = loop_on_board("");
    let lib = Library::load(&c, None);
    let nl = Netlist::build(&c, &lib, View::Breadboard);
    // A (with R1.a) is jumpered to C (with D1.cathode); B joins R1.b and D1.anode.
    assert_eq!(nl.net_of_pin("R1", "a"), nl.net_of_pin("D1", "cathode"));
    assert_eq!(nl.net_of_pin("R1", "b"), nl.net_of_pin("D1", "anode"));
    assert_ne!(nl.net_of_pin("R1", "a"), nl.net_of_pin("R1", "b"));
    // The strips are buses: every hole of a strip is on the strip's net.
    for hole in ["a0", "a1", "a2", "c0", "c1", "c2"] {
        assert_eq!(nl.net_of_pin("B1", hole), nl.net_of_pin("R1", "a"), "{hole}");
    }
    for hole in ["b0", "b1", "b2"] {
        assert_eq!(nl.net_of_pin("B1", hole), nl.net_of_pin("R1", "b"), "{hole}");
    }
    assert_eq!(nl.net_of_wire("j1"), nl.net_of_pin("R1", "a"));
    assert_eq!(nl.nets.len(), 2);
    // Pin positions: R1 at (0,1): a (0,1) b (2,1); D1 at (2,2): anode (2,2) cathode (4,2).
    let at = |part: &str, pin: &str| nl.nets.iter().flat_map(|n| &n.pins).find(|p| p.part == part && p.pin == pin).unwrap().at;
    assert_eq!(at("R1", "a"), [0, 1]);
    assert_eq!(at("R1", "b"), [2, 1]);
    assert_eq!(at("D1", "anode"), [2, 2]);
    assert_eq!(at("D1", "cathode"), [4, 2]);
    // No schematic geometry is placed.
    assert!(Netlist::build(&c, &lib, View::Schematic).nets.is_empty());
}

#[test]
fn clean_board_circuit_has_no_issues() {
    let r = run(&loop_on_board(""));
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
}

#[test]
fn a_jumper_passing_over_a_hole_does_not_connect_but_may_dangle() {
    // j2 starts in C (c1), bends at (2,1) (R1.b / b1) and ends in free space.
    let c = loop_on_board(r#",{"id":"j2","view":"breadboard","pts":[[4,1],[2,1],[2,-4]]}"#);
    let lib = Library::load(&c, None);
    let nl = Netlist::build(&c, &lib, View::Breadboard);
    assert_eq!(nl.net_of_wire("j2"), nl.net_of_pin("B1", "c1"));
    assert_ne!(nl.net_of_wire("j2"), nl.net_of_pin("R1", "b"), "an interior vertex connects nothing");
    let r = run(&c);
    let dangling = of(&r, "dangling-wire");
    assert_eq!(dangling.len(), 1, "{:#?}", r.issues);
    assert_eq!((dangling[0].ids.as_slice(), dangling[0].at, dangling[0].view), (&["j2".to_string()][..], Some([2, -4]), Some(View::Breadboard)));
}

#[test]
fn legs_floating_over_nothing_are_unconnected_but_boards_are_exempt() {
    // R1 away from the board: both legs float.
    let parts = format!(
        r#"{},{{"id":"R1","type":"r2","breadboard":{{"at":[20,20]}}}}"#,
        board("[0,0]")
    );
    let r = run(&circuit(&parts, "", ""));
    let un = of(&r, "unconnected-pin");
    assert_eq!(un.len(), 2, "{:#?}", r.issues);
    assert!(un.iter().all(|i| i.ids == ["R1"] && i.view == Some(View::Breadboard)));
}

#[test]
fn breadboard_part_overlap_ignores_boards_and_applies_to_components() {
    let parts = format!(
        r#"{},{{"id":"R1","type":"r2","breadboard":{{"at":[0,1]}}}},{{"id":"R2","type":"r2","breadboard":{{"at":[1,1]}}}}"#,
        board("[0,0]")
    );
    let r = run(&circuit(&parts, "", ""));
    let ov = of(&r, "part-overlap");
    assert_eq!(ov.len(), 1, "{:#?}", r.issues);
    assert_eq!((ov[0].ids.as_slice(), ov[0].view), (&["R1".to_string(), "R2".to_string()][..], Some(View::Breadboard)));
}

// ---- pin-conflict ----------------------------------------------------------------------

#[test]
fn pin_conflict_two_legs_in_one_hole() {
    let parts = format!(
        r#"{},{{"id":"R1","type":"r2","breadboard":{{"at":[0,1]}}}},{{"id":"R2","type":"r2","breadboard":{{"at":[0,1]}}}}"#,
        board("[0,0]")
    );
    let r = run(&circuit(&parts, "", ""));
    let pc = of(&r, "pin-conflict");
    assert_eq!(pc.len(), 2, "R1.a/R2.a share (0,1) and R1.b/R2.b share (2,1): {:#?}", r.issues);
    assert!(pc.iter().all(|i| i.severity == Severity::Error && i.view == Some(View::Breadboard)));
    assert!(pc.iter().any(|i| i.at == Some([0, 1]) && i.ids == ["R1", "R2"]));
}

#[test]
fn pin_conflict_not_raised_for_a_leg_in_a_hole_or_in_the_schematic() {
    // A leg sitting on a board hole is the normal case.
    assert!(of(&run(&loop_on_board("")), "pin-conflict").is_empty());
    // In the schematic, pins at the same point simply connect.
    let c = circuit(
        r#"{"id":"R1","type":"r2","schematic":{"at":[0,0]}},{"id":"R2","type":"r2","schematic":{"at":[4,0]}}"#,
        "",
        "",
    );
    assert!(of(&run(&c), "pin-conflict").is_empty());
}

// ---- no-view ---------------------------------------------------------------------------

#[test]
fn no_view_when_a_part_is_placed_where_its_type_has_no_geometry() {
    // `sym` is schematic-only.
    let c = circuit(r#"{"id":"S1","type":"sym","breadboard":{"at":[0,0]}}"#, "", "");
    let r = run(&c);
    let nv = of(&r, "no-view");
    assert_eq!(nv.len(), 1, "{:#?}", r.issues);
    assert_eq!((nv[0].severity, nv[0].view, nv[0].ids.as_slice()), (Severity::Error, Some(View::Breadboard), &["S1".to_string()][..]));
    // The board has no schematic view either.
    let c = circuit(r#"{"id":"B1","type":"board","schematic":{"at":[0,0]}}"#, "", "");
    let nv = of(&run(&c), "no-view").len();
    assert_eq!(nv, 1);
}

#[test]
fn no_view_not_raised_when_the_view_exists() {
    let c = circuit(
        r#"{"id":"S1","type":"sym","schematic":{"at":[0,0]}},{"id":"R1","type":"r2","schematic":{"at":[6,0]},"breadboard":{"at":[0,1]}},
        {"id":"B1","type":"board","breadboard":{"at":[0,0]}}"#,
        "",
        "",
    );
    assert!(of(&run(&c), "no-view").is_empty());
}

// ---- bad-net ---------------------------------------------------------------------------

#[test]
fn bad_net_flags_unknown_parts_and_pins() {
    let c = circuit(
        r#"{"id":"R1","type":"r2","schematic":{"at":[0,0]}},{"id":"X1","type":"nope","schematic":{"at":[8,0]}}"#,
        "",
        r#","nets":[{"name":"N1","pins":["R1.a","R1.zz","Q9.a","junk"]},{"pins":["X1.anything","R1.b"]}]"#,
    );
    let r = run(&c);
    let bad: Vec<_> = of(&r, "bad-net");
    let ids: Vec<&str> = bad.iter().map(|i| i.ids[0].as_str()).collect();
    assert_eq!(ids.len(), 3, "{:#?}", r.issues);
    for want in ["R1.zz", "Q9.a", "junk"] {
        assert!(ids.contains(&want), "{want} in {ids:?}");
    }
    assert!(bad.iter().all(|i| i.severity == Severity::Error && i.view.is_none() && i.at.is_none()));
    assert!(bad[0].message.contains("N1") || bad.iter().any(|i| i.message.contains("N1")));
}

#[test]
fn bad_net_not_raised_for_valid_references() {
    let c = circuit(
        r#"{"id":"R1","type":"r2","schematic":{"at":[0,0]}},{"id":"D1","type":"led2","schematic":{"at":[8,0]}}"#,
        "",
        r#","nets":[{"name":"N","pins":["R1.b","D1.anode"]}]"#,
    );
    assert!(of(&run(&c), "bad-net").is_empty());
    // Part ids may contain dots.
    let c = circuit(
        r#"{"id":"R.1","type":"r2","schematic":{"at":[0,0]}}"#,
        "",
        r#","nets":[{"pins":["R.1.a"]}]"#,
    );
    assert!(of(&run(&c), "bad-net").is_empty());
}

// ---- cross-view ------------------------------------------------------------------------

/// R1 and R2 in both views. Schematic: R1.b and R2.a coincide at (4,1). Breadboard: R1 at (0,1)
/// (a in strip A, b in strip B), R2 at `r2_bb` (default (2,2): a in strip B, b in strip C).
fn two_resistors(r2_bb: &str, bb_wires: &str, extra: &str) -> Circuit {
    let parts = format!(
        r#"{},
        {{"id":"R1","type":"r2","schematic":{{"at":[0,0]}},"breadboard":{{"at":[0,1]}}}},
        {{"id":"R2","type":"r2","schematic":{{"at":[4,0]}},"breadboard":{{"at":{r2_bb}}}}}"#,
        board("[0,0]")
    );
    circuit(&parts, bb_wires, extra)
}

#[test]
fn net_open_when_the_breadboard_does_not_join_what_the_schematic_joins() {
    // R2 in strip C / free space: R1.b (strip B) and R2.a (strip C) are not joined.
    let r = run(&two_resistors("[4,0]", "", ""));
    let open = of(&r, "net-open");
    assert_eq!(open.len(), 1, "{:#?}", r.issues);
    let o = open[0];
    assert_eq!((o.severity, o.view, o.at), (Severity::Warning, None, None));
    assert_eq!(o.ids, ["R1.b", "R2.a"]);
    assert!(o.message.contains("breadboard"), "{}", o.message);
    assert!(of(&r, "net-merged").is_empty());
}

#[test]
fn net_open_not_raised_when_joined_by_a_strip_or_a_jumper() {
    // R2 at (2,2): R2.a in strip B with R1.b.
    let r = run(&two_resistors("[2,2]", "", ""));
    assert!(of(&r, "net-open").is_empty() && of(&r, "net-merged").is_empty(), "{:#?}", r.issues);
    // Or a jumper from strip B (2,0) to R2.a's hole in strip C (4,0).
    let r = run(&two_resistors("[4,0]", r#"{"id":"j1","view":"breadboard","pts":[[2,0],[4,0]]}"#, ""));
    assert!(of(&r, "net-open").is_empty(), "{:#?}", r.issues);
}

#[test]
fn net_merged_when_the_breadboard_joins_different_nets() {
    // Jumper strip A (R1.a) to strip C (R2.b): two schematic nets become one.
    let r = run(&two_resistors("[2,2]", r#"{"id":"j1","view":"breadboard","pts":[[0,0],[4,0]]}"#, ""));
    let m = of(&r, "net-merged");
    assert_eq!(m.len(), 1, "{:#?}", r.issues);
    assert_eq!((m[0].severity, m[0].view, m[0].at), (Severity::Error, None, None));
    assert_eq!(m[0].ids, ["R1.a", "R2.b"]);
    assert!(!r.ok);
}

#[test]
fn net_merged_not_raised_without_the_extra_connection() {
    let r = run(&two_resistors("[2,2]", "", ""));
    assert!(of(&r, "net-merged").is_empty(), "{:#?}", r.issues);
    // A jumper that stays inside one schematic net is fine too (strip B to strip B).
    let r = run(&two_resistors("[2,2]", r#"{"id":"j1","view":"breadboard","pts":[[2,0],[2,2]]}"#, ""));
    assert!(of(&r, "net-merged").is_empty(), "{:#?}", r.issues);
}

#[test]
fn declared_nets_are_the_reference_and_only_named_pins_are_constrained() {
    let nets = r#","nets":[{"name":"MID","pins":["R1.b","R2.a"]}]"#;
    // Satisfied in both views (schematic coincidence, breadboard strip B).
    let r = run(&two_resistors("[2,2]", "", nets));
    assert!(of(&r, "net-open").is_empty() && of(&r, "net-merged").is_empty(), "{:#?}", r.issues);
    // Breadboard not routed: net-open, named after the declared net.
    let r = run(&two_resistors("[4,0]", "", nets));
    let open = of(&r, "net-open");
    assert_eq!(open.len(), 1, "{:#?}", r.issues);
    assert!(open[0].message.contains("MID") && open[0].message.contains("breadboard"), "{}", open[0].message);
    // R1.a and R2.b are not named: joining them (strip A to strip C) is not a net-merged.
    let r = run(&two_resistors("[2,2]", r#"{"id":"j1","view":"breadboard","pts":[[0,0],[4,0]]}"#, nets));
    assert!(of(&r, "net-merged").is_empty(), "{:#?}", r.issues);
}

#[test]
fn declared_nets_also_hold_the_schematic_to_account() {
    // Declared: R1.a with R2.b. The schematic draws no such connection.
    let nets = r#","nets":[{"pins":["R1.a","R2.b"]}]"#;
    let r = run(&two_resistors("[2,2]", "", nets));
    let open = of(&r, "net-open");
    assert!(open.iter().any(|i| i.message.contains("schematic")), "{:#?}", r.issues);
    // Declared nets that overlap are one net.
    let nets = r#","nets":[{"pins":["R1.a","R1.b"]},{"pins":["R1.b","R2.a"]}]"#;
    let r = run(&two_resistors("[2,2]", "", nets));
    assert!(of(&r, "net-merged").is_empty());
}

#[test]
fn boards_and_one_view_parts_are_ignored_by_the_net_comparison() {
    // D1 is placed in the breadboard only and wired to R2's net there; the board's strips join
    // many pins. Neither raises net-open / net-merged.
    let parts = format!(
        r#"{},
        {{"id":"R1","type":"r2","schematic":{{"at":[0,0]}},"breadboard":{{"at":[0,1]}}}},
        {{"id":"R2","type":"r2","schematic":{{"at":[4,0]}},"breadboard":{{"at":[2,2]}}}},
        {{"id":"D1","type":"led2","breadboard":{{"at":[4,0]}}}}"#,
        board("[0,0]")
    );
    let r = run(&circuit(&parts, "", ""));
    assert!(of(&r, "net-open").is_empty() && of(&r, "net-merged").is_empty(), "{:#?}", r.issues);
}

#[test]
fn unplaced_part_when_a_view_in_use_lacks_a_placement() {
    // The breadboard is in use (board, R1, R2); R3 is only in the schematic.
    let parts = format!(
        r#"{},
        {{"id":"R1","type":"r2","schematic":{{"at":[0,0]}},"breadboard":{{"at":[0,1]}}}},
        {{"id":"R3","type":"r2","schematic":{{"at":[0,6]}}}}"#,
        board("[0,0]")
    );
    let r = run(&circuit(&parts, "", ""));
    let un = of(&r, "unplaced-part");
    assert_eq!(un.len(), 1, "{:#?}", r.issues);
    assert_eq!((un[0].severity, un[0].view, un[0].ids.as_slice()), (Severity::Warning, None, &["R3".to_string()][..]));
    assert!(un[0].message.contains("breadboard"));
}

#[test]
fn unplaced_part_not_raised_when_the_view_is_unused_or_the_type_has_no_such_geometry() {
    // No breadboard placement or wire anywhere: the view is not in use.
    let c = circuit(r#"{"id":"R1","type":"r2","schematic":{"at":[0,0]}}"#, "", "");
    assert!(of(&run(&c), "unplaced-part").is_empty());
    // A breadboard wire alone puts the view in use...
    let c = circuit(
        r#"{"id":"R1","type":"r2","schematic":{"at":[0,0]}}"#,
        r#"{"id":"j","view":"breadboard","pts":[[0,0],[3,0]]}"#,
        "",
    );
    assert_eq!(of(&run(&c), "unplaced-part").len(), 1);
    // ...but a schematic-only type has nothing to place there.
    let c = circuit(
        r#"{"id":"S1","type":"sym","schematic":{"at":[0,0]}}"#,
        r#"{"id":"j","view":"breadboard","pts":[[0,0],[3,0]]}"#,
        "",
    );
    assert!(of(&run(&c), "unplaced-part").is_empty());
}

#[test]
fn schematic_only_circuits_get_no_cross_view_issues() {
    let c = circuit(
        r#"{"id":"R1","type":"r2","schematic":{"at":[0,0]}},{"id":"R2","type":"r2","schematic":{"at":[4,0]}}"#,
        r#"{"id":"w","pts":[[0,1],[-3,1]]}"#,
        "",
    );
    let r = run(&c);
    for rule in ["net-open", "net-merged", "unplaced-part", "no-view", "pin-conflict", "bad-net"] {
        assert!(of(&r, rule).is_empty(), "{rule}: {:#?}", r.issues);
    }
}

// ---- view selection --------------------------------------------------------------------

#[test]
fn validate_view_limits_the_rules_to_one_view() {
    // Breadboard has a dangling jumper, schematic a dangling wire.
    let c = circuit(
        "",
        r#"{"id":"w","pts":[[0,0],[5,0]]},{"id":"j","view":"breadboard","pts":[[0,0],[5,5]]}"#,
        "",
    );
    let lib = Library::load(&c, None);
    let s = validate_view(&c, &lib, View::Schematic);
    let b = validate_view(&c, &lib, View::Breadboard);
    assert!(of(&s, "dangling-wire").iter().all(|i| i.ids == ["w"] && i.view == Some(View::Schematic)));
    assert!(of(&b, "dangling-wire").iter().all(|i| i.ids == ["j"] && i.view == Some(View::Breadboard)));
    assert_eq!(of(&s, "dangling-wire").len(), 2);
    assert_eq!(of(&b, "dangling-wire").len(), 2);
    assert_eq!(of(&validate(&c, &lib), "dangling-wire").len(), 4);
}

#[test]
fn schematic_only_rules_do_not_fire_in_the_breadboard() {
    // Crossing, overlapping and diagonal jumpers are fine on a breadboard.
    let c = circuit(
        "",
        r#"{"id":"a","view":"breadboard","pts":[[0,0],[10,10]]},{"id":"b","view":"breadboard","pts":[[0,10],[10,0]]},
        {"id":"c","view":"breadboard","pts":[[0,0],[10,0]]},{"id":"d","view":"breadboard","pts":[[3,0],[12,0]]}"#,
        "",
    );
    let r = run(&c);
    for rule in ["wire-crossing", "wire-overlap", "wire-diagonal", "wire-through-pin", "empty-net", "net-short"] {
        assert!(of(&r, rule).is_empty(), "{rule}: {:#?}", r.issues);
    }
}

#[test]
fn bad_rotation_is_reported_per_view_with_that_views_location() {
    let c = circuit(
        r#"{"id":"R1","type":"r2","schematic":{"at":[1,2]},"breadboard":{"at":[3,4],"rot":45}}"#,
        "",
        "",
    );
    let r = run(&c);
    let br = of(&r, "bad-rotation");
    assert_eq!(br.len(), 1, "{:#?}", r.issues);
    assert_eq!((br[0].view, br[0].at), (Some(View::Breadboard), Some([3, 4])));
}

#[test]
fn breadboard_view_in_a_v2_report_serialises_the_view() {
    let r = run(&two_resistors("[4,0]", "", ""));
    let json = serde_json::to_value(&r).unwrap();
    let issues = json["issues"].as_array().unwrap();
    let open = issues.iter().find(|i| i["rule"] == "net-open").unwrap();
    assert!(open.get("view").is_none(), "cross-view issues have no view key");
    let bad_rot = run(&circuit(r#"{"id":"R1","type":"r2","breadboard":{"at":[0,0],"rot":45}}"#, "", ""));
    let j = serde_json::to_value(&bad_rot).unwrap();
    assert_eq!(j["issues"][0]["view"], "breadboard");
}

// ---- unconnected-pin exemptions, breadboard degenerate jumpers ---------------------------

#[test]
fn unconnected_pin_skips_pins_in_a_bus_and_boards() {
    // A link part whose two pins are one bus: neither is reported even with no wire. A plain
    // two-pin part is reported twice.
    let c = parse_circuit(
        r#"{"format":"short-circ/2","lib":[{"type":"link","pins":[{"id":"a"},{"id":"b"}],"buses":[["a","b"]],
          "schematic":{"size":[4,1],"pins":{"a":{"at":[0,0]},"b":{"at":[4,0]}},"shapes":[{"k":"line","pts":[[0,0],[4,0]]}]}}],
          "parts":[{"id":"K1","type":"link","schematic":{"at":[0,0]}}]}"#,
    )
    .unwrap();
    assert!(of(&run(&c), "unconnected-pin").is_empty());
    let c = circuit(r#"{"id":"R1","type":"r2","schematic":{"at":[0,0]}}"#, "", "");
    assert_eq!(of(&run(&c), "unconnected-pin").len(), 2);
    // Board holes are never reported.
    let c = circuit(&board("[0,0]"), "", "");
    assert!(of(&run(&c), "unconnected-pin").is_empty());
}

#[test]
fn degenerate_jumpers_are_reported_in_the_breadboard() {
    let c = circuit(
        "",
        r#"{"id":"a","view":"breadboard","pts":[[3,3],[3,3]]},{"id":"b","view":"breadboard","pts":[[0,0],[5,0],[5,0],[9,0]]},
        {"id":"c","view":"breadboard","pts":[[0,5],[9,5]]}"#,
        "",
    );
    let r = run(&c);
    let ids: Vec<&str> = of(&r, "wire-degenerate").iter().map(|i| i.ids[0].as_str()).collect();
    assert!(ids.contains(&"a") && ids.contains(&"b") && !ids.contains(&"c"), "{ids:?}");
    assert!(of(&r, "wire-degenerate").iter().all(|i| i.view == Some(View::Breadboard)));
}
