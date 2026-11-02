//! v1 files keep loading and behave exactly as they did before format v2.
//!
//! The fixtures under `tests/fixtures/v1/` are copied verbatim from commit 490b984 (the last
//! v1-only revision): `examples/{led,bad,colors}.circ` and `parts/*.part`. The expected nets,
//! issues and SVG output below were recorded by running that old build (`expected/*.svg` are its
//! output, the nets and issue lines were printed by its `Netlist`/`validate`).

use short_circ_core::svg::render_svg;
use short_circ_core::{
    parse_circuit, parse_part, validate, validate_view, Library, Netlist, Severity, View, FORMAT,
};

const LED: &str = include_str!("fixtures/v1/led.circ");
const BAD: &str = include_str!("fixtures/v1/bad.circ");
const COLORS: &str = include_str!("fixtures/v1/colors.circ");

fn lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// `net ID name=NAME pins=[PART.PIN@x,y ...] wires=[...]`, pins sorted.
fn nets(src: &str, view: View) -> Vec<String> {
    let c = parse_circuit(src).unwrap();
    let nl = Netlist::build(&c, &Library::load(&c, None), view);
    nl.nets
        .iter()
        .map(|n| {
            let mut pins: Vec<String> =
                n.pins.iter().map(|p| format!("{}.{}@{},{}", p.part, p.pin, p.at[0], p.at[1])).collect();
            pins.sort();
            format!("net {} name={:?} pins=[{}] wires=[{}]", n.id, n.name, pins.join(" "), n.wires.join(" "))
        })
        .collect()
}

/// `RULE Severity ids=[..] at=Some([x, y]) msg=MESSAGE`, in report order.
fn issues(src: &str) -> (Vec<String>, usize, usize) {
    let c = parse_circuit(src).unwrap();
    let r = validate(&c, &Library::load(&c, None));
    let lines = r
        .issues
        .iter()
        .map(|i| format!("{} {:?} ids={:?} at={:?} msg={}", i.rule, i.severity, i.ids, i.at, i.message))
        .collect();
    (lines, r.errors, r.warnings)
}

fn lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_string).collect()
}

#[test]
fn led_nets_and_report_are_unchanged() {
    assert_eq!(
        nets(LED, View::Schematic),
        lines(
            r#"net 0 name=None pins=[B1.pos@11,10 R1.a@14,7] wires=[w1]
net 1 name=Some("GND") pins=[B1.neg@11,14 D1.cathode@25,10] wires=[w3]
net 2 name=None pins=[D1.anode@25,6 R1.b@22,7] wires=[w2]"#
        )
    );
    assert_eq!(issues(LED), (vec![], 0, 0));
    // No breadboard view exists in a v1 file.
    assert!(nets(LED, View::Breadboard).is_empty());
}

#[test]
fn bad_nets_and_report_are_unchanged() {
    assert_eq!(
        nets(BAD, View::Schematic),
        lines(
            r#"net 0 name=None pins=[R1.a@5,6] wires=[w1 w2]
net 1 name=None pins=[R1.b@13,6] wires=[]
net 2 name=None pins=[R2.a@8,6] wires=[]
net 3 name=None pins=[R2.b@16,6] wires=[]
net 4 name=None pins=[] wires=[w3]
net 5 name=None pins=[] wires=[w4]
net 6 name=None pins=[] wires=[w5]"#
        )
    );
    let (got, errors, warnings) = issues(BAD);
    let expected = lines(
        r#"dangling-wire Warning ids=["w1"] at=Some([0, 6]) msg=wire w1 has an endpoint at (0,6) that touches nothing
dangling-wire Warning ids=["w2"] at=Some([10, 6]) msg=wire w2 has an endpoint at (10,6) that touches nothing
dangling-wire Warning ids=["w3"] at=Some([20, 0]) msg=wire w3 has an endpoint at (20,0) that touches nothing
dangling-wire Warning ids=["w3"] at=Some([20, 10]) msg=wire w3 has an endpoint at (20,10) that touches nothing
dangling-wire Warning ids=["w4"] at=Some([15, 5]) msg=wire w4 has an endpoint at (15,5) that touches nothing
dangling-wire Warning ids=["w4"] at=Some([25, 5]) msg=wire w4 has an endpoint at (25,5) that touches nothing
dangling-wire Warning ids=["w5"] at=Some([30, 30]) msg=wire w5 has an endpoint at (30,30) that touches nothing
dangling-wire Warning ids=["w5"] at=Some([34, 30]) msg=wire w5 has an endpoint at (34,30) that touches nothing
empty-net Warning ids=["w3"] at=Some([20, 0]) msg=net with wire(s) w3 has no pins
empty-net Warning ids=["w4"] at=Some([15, 5]) msg=net with wire(s) w4 has no pins
empty-net Warning ids=["w5"] at=Some([30, 30]) msg=net with wire(s) w5 has no pins
part-overlap Error ids=["R1", "R2"] at=Some([8, 5]) msg=parts R1 and R2 overlap at (8,5)
unconnected-pin Warning ids=["R1"] at=Some([13, 6]) msg=pin R1.b at (13,6) is not connected
unconnected-pin Warning ids=["R2"] at=Some([8, 6]) msg=pin R2.a at (8,6) is not connected
unconnected-pin Warning ids=["R2"] at=Some([16, 6]) msg=pin R2.b at (16,6) is not connected
wire-crossing Warning ids=["w3", "w4"] at=Some([20, 5]) msg=w3 and w4 cross at (20,5) without a junction
wire-overlap Error ids=["w1", "w2"] at=Some([3, 6]) msg=w1 and w2 overlap between (3,6) and (5,6)
wire-through-pin Error ids=["w2", "R1"] at=Some([5, 6]) msg=wire w2 passes through pin R1.a at (5,6) without a vertex
wire-through-pin Error ids=["w2", "R2"] at=Some([8, 6]) msg=wire w2 passes through pin R2.a at (8,6) without a vertex"#,
    );
    assert_eq!(got, expected);
    assert_eq!((errors, warnings), (4, 15));
}

#[test]
fn colors_nets_and_report_are_unchanged() {
    assert_eq!(
        nets(COLORS, View::Schematic),
        lines(
            r#"net 0 name=Some("VCC") pins=[B1.pos@5,8 R1.a@10,5] wires=[w1]
net 1 name=Some("GND") pins=[B1.neg@5,12 D1.cathode@36,5 R3.b@20,16] wires=[w4]
net 2 name=None pins=[R1.b@18,5 R2.a@22,5 R3.a@20,8] wires=[w2 w5]
net 3 name=None pins=[D1.anode@32,5 R2.b@30,5] wires=[w3]"#
        )
    );
    assert_eq!(issues(COLORS), (vec![], 0, 0));
}

#[test]
fn per_view_issues_of_a_v1_file_are_schematic_and_the_view_filter_agrees() {
    let c = parse_circuit(BAD).unwrap();
    let lib = Library::load(&c, None);
    let all = validate(&c, &lib);
    assert!(all.issues.iter().all(|i| i.view == Some(View::Schematic)), "{:#?}", all.issues);
    let s = validate_view(&c, &lib, View::Schematic);
    assert_eq!(serde_json::to_string(&all).unwrap(), serde_json::to_string(&s).unwrap());
    // The breadboard view of a v1 file is empty: nothing to report.
    assert!(validate_view(&c, &lib, View::Breadboard).issues.is_empty());
    assert!(all.issues.iter().any(|i| i.severity == Severity::Error));
}

#[test]
fn svg_output_is_byte_identical_to_the_old_build() {
    let golden = [
        ("led", LED, include_str!("fixtures/v1/expected/led.svg"), include_str!("fixtures/v1/expected/led-dark.svg")),
        ("bad", BAD, include_str!("fixtures/v1/expected/bad.svg"), include_str!("fixtures/v1/expected/bad-dark.svg")),
        (
            "colors",
            COLORS,
            include_str!("fixtures/v1/expected/colors.svg"),
            include_str!("fixtures/v1/expected/colors-dark.svg"),
        ),
    ];
    for (name, src, light, dark) in golden {
        let c = parse_circuit(src).unwrap();
        let lib = Library::load(&c, None);
        assert_eq!(lf(&render_svg(&c, &lib, false, View::Schematic)), lf(light), "{name} light");
        assert_eq!(lf(&render_svg(&c, &lib, true, View::Schematic)), lf(dark), "{name} dark");
    }
}

const V1_PARTS: &[(&str, &str)] = &[
    ("battery", include_str!("fixtures/v1/parts/battery.part")),
    ("buzzer", include_str!("fixtures/v1/parts/buzzer.part")),
    ("capacitor", include_str!("fixtures/v1/parts/capacitor.part")),
    ("capacitor_polarized", include_str!("fixtures/v1/parts/capacitor_polarized.part")),
    ("connector2", include_str!("fixtures/v1/parts/connector2.part")),
    ("crystal", include_str!("fixtures/v1/parts/crystal.part")),
    ("diode", include_str!("fixtures/v1/parts/diode.part")),
    ("fuse", include_str!("fixtures/v1/parts/fuse.part")),
    ("ground", include_str!("fixtures/v1/parts/ground.part")),
    ("inductor", include_str!("fixtures/v1/parts/inductor.part")),
    ("led", include_str!("fixtures/v1/parts/led.part")),
    ("nmos", include_str!("fixtures/v1/parts/nmos.part")),
    ("npn", include_str!("fixtures/v1/parts/npn.part")),
    ("opamp", include_str!("fixtures/v1/parts/opamp.part")),
    ("pmos", include_str!("fixtures/v1/parts/pmos.part")),
    ("pnp", include_str!("fixtures/v1/parts/pnp.part")),
    ("potentiometer", include_str!("fixtures/v1/parts/potentiometer.part")),
    ("pushbutton", include_str!("fixtures/v1/parts/pushbutton.part")),
    ("resistor", include_str!("fixtures/v1/parts/resistor.part")),
    ("switch", include_str!("fixtures/v1/parts/switch.part")),
    ("vcc", include_str!("fixtures/v1/parts/vcc.part")),
    ("voltage_source_ac", include_str!("fixtures/v1/parts/voltage_source_ac.part")),
    ("zener", include_str!("fixtures/v1/parts/zener.part")),
];

#[test]
fn every_v1_part_file_loads_as_a_schematic_part() {
    assert_eq!(V1_PARTS.len(), 23);
    for (name, src) in V1_PARTS {
        let raw: serde_json::Value = serde_json::from_str(src).unwrap();
        assert_eq!(raw["format"], "short-circ/1", "{name} is a v1 fixture");
        let d = parse_part(src).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&d.ty, name);
        assert_eq!(d.format, FORMAT);
        let s = d.schematic.as_ref().unwrap_or_else(|| panic!("{name}: no schematic view"));
        assert!(d.breadboard.is_none());
        // Every v1 pin keeps its id, order, name and position.
        let raw_pins = raw["pins"].as_array().unwrap();
        assert_eq!(d.pins.len(), raw_pins.len(), "{name}");
        for (def, raw) in d.pins.iter().zip(raw_pins) {
            assert_eq!(def.id, raw["id"].as_str().unwrap());
            assert_eq!(def.name.as_deref(), raw["name"].as_str());
            let pos = s.pins[&def.id];
            assert_eq!(pos.at, [raw["at"][0].as_i64().unwrap() as i32, raw["at"][1].as_i64().unwrap() as i32]);
        }
        assert_eq!(s.size, [raw["size"][0].as_f64().unwrap(), raw["size"][1].as_f64().unwrap()]);
        assert_eq!(s.shapes.len(), raw["shapes"].as_array().map_or(0, Vec::len), "{name}");
        assert!(s.svg.is_none());
    }
}

#[test]
fn builtin_library_resolves_every_v1_part_type() {
    let lib = Library::builtin();
    for (name, _) in V1_PARTS {
        let d = lib.get(name).unwrap_or_else(|| panic!("{name} not built in"));
        assert!(d.schematic.is_some(), "{name}");
    }
}

#[test]
fn v1_circuit_with_inline_lib_round_trips_through_the_upgrade() {
    let src = r##"{"format":"short-circ/1","lib":[{"type":"box","size":[4,2],"pins":[{"id":"a","at":[0,1],"dir":"left"},{"id":"b","at":[4,1],"dir":"right"}],
        "shapes":[{"k":"rect","at":[0,0],"size":[4,2]}]}],
        "parts":[{"id":"P1","type":"box","at":[0,0]},{"id":"P2","type":"box","at":[8,0],"rot":180}],
        "wires":[{"id":"w","pts":[[4,1],[8,1]]}],"labels":[{"at":[4,1],"name":"N"}]}"##;
    let c = parse_circuit(src).unwrap();
    let nl = Netlist::build(&c, &Library::load(&c, None), View::Schematic);
    // P1.b (4,1), P2.a: (0,1) rot 180 -> (-0,-1) + (8,0) = (8,-1); w joins P1.b with (8,1) which is nothing.
    assert_eq!(nl.net_of_pin("P1", "b"), nl.net_of_wire("w"));
    assert_eq!(nl.nets[nl.net_of_wire("w").unwrap()].name.as_deref(), Some("N"));
    let r = validate(&c, &Library::load(&c, None));
    assert!(r.issues.iter().any(|i| i.rule == "dangling-wire" && i.ids == ["w"]));
}
