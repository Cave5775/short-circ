//! Every file in /parts must be a valid `short-circ/2` component: parses into the model, pin ids
//! are unique, view pins are logical pins at integer positions inside the view, SVG artwork
//! matches the view size, and boards (breadboards) are consistent. Needs only the data model.

use short_circ_core::{PartDef, ViewDef};
use std::collections::HashSet;
use std::path::PathBuf;

fn parts() -> Vec<(String, PartDef)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../parts");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).expect("parts dir") {
        let path = e.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("part") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(&path).unwrap();
        let def: PartDef = serde_json::from_str(&src).unwrap_or_else(|e| panic!("{name}: {e}"));
        out.push((name, def));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn check_view(name: &str, def: &PartDef, view: &str, v: &ViewDef, pin_ids: &HashSet<&str>) {
    assert!(v.size[0] > 0.0 && v.size[1] > 0.0, "{name} {view}: empty size");
    for (id, p) in &v.pins {
        assert!(pin_ids.contains(id.as_str()), "{name} {view}: pin key {id} is not a logical pin");
        assert!(
            p.at[0] >= 0 && p.at[1] >= 0 && p.at[0] as f64 <= v.size[0] && p.at[1] as f64 <= v.size[1],
            "{name} {view}: pin {id} at {:?} outside size {:?}",
            p.at,
            v.size
        );
    }
    if let Some(svg) = &v.svg {
        assert!(svg.starts_with("<svg"), "{name} {view}: svg does not start with <svg");
        assert!(svg.trim_end().ends_with("</svg>"), "{name} {view}: svg not closed");
        if view == "breadboard" {
            let vb = format!("viewBox='0 0 {} {}'", (v.size[0] * 10.0) as i64, (v.size[1] * 10.0) as i64);
            assert!(svg.contains(&vb), "{name}: expected {vb} (10 viewBox units per grid unit)");
        }
    }
    let _ = def;
}

#[test]
fn every_part_file_is_valid_v2() {
    let all = parts();
    assert!(all.len() >= 26, "expected at least 26 parts, found {}", all.len());
    for (name, d) in &all {
        assert_eq!(d.format, "short-circ/2", "{name}");
        assert_eq!(format!("{}.part", d.ty), *name, "{name}: type must match the file name");
        assert!(!d.name.is_empty(), "{name}: missing name");
        assert!(d.category.is_some(), "{name}: missing category");
        assert!(!d.pins.is_empty(), "{name}: no pins");
        let mut seen = HashSet::new();
        for p in &d.pins {
            assert!(seen.insert(p.id.as_str()), "{name}: duplicate pin {}", p.id);
        }
        assert!(d.schematic.is_some() || d.breadboard.is_some(), "{name}: no view");
        if let Some(v) = &d.schematic {
            check_view(name, d, "schematic", v, &seen);
        }
        if let Some(v) = &d.breadboard {
            check_view(name, d, "breadboard", v, &seen);
        }
        // every logical pin is placed in at least one view
        for p in &d.pins {
            let placed = d.schematic.as_ref().is_some_and(|v| v.pins.contains_key(&p.id))
                || d.breadboard.as_ref().is_some_and(|v| v.pins.contains_key(&p.id));
            assert!(placed, "{name}: pin {} has no position in any view", p.id);
        }
        for bus in &d.buses {
            assert!(bus.len() >= 2, "{name}: bus with fewer than two pins");
            for id in bus {
                assert!(seen.contains(id.as_str()), "{name}: bus member {id} is not a pin");
            }
        }
    }
}

#[test]
fn schematic_pins_keep_their_v1_ids_and_positions() {
    // spot checks of pins that must not move during the upgrade
    let all = parts();
    let get = |t: &str| &all.iter().find(|(_, d)| d.ty == t).unwrap().1;
    let s = |t: &str, pin: &str| get(t).schematic.as_ref().unwrap().pins[pin].at;
    assert_eq!(s("resistor", "a"), [0, 1]);
    assert_eq!(s("resistor", "b"), [8, 1]);
    assert_eq!(s("battery", "pos"), [1, 0]);
    assert_eq!(s("npn", "collector"), [4, 0]);
    assert_eq!(s("opamp", "v-"), [4, 6]);
    assert_eq!(s("potentiometer", "wiper"), [4, 0]);
    assert_eq!(s("ground", "gnd"), [1, 0]);
}

#[test]
fn power_symbols_have_no_breadboard_view() {
    let all = parts();
    for t in ["ground", "vcc"] {
        let d = &all.iter().find(|(_, d)| d.ty == t).unwrap().1;
        assert!(d.schematic.is_some() && d.breadboard.is_none(), "{t}");
    }
}

#[test]
fn physical_parts_have_a_breadboard_view() {
    let all = parts();
    for t in [
        "resistor", "led", "capacitor", "capacitor_polarized", "diode", "zener", "switch", "pushbutton",
        "potentiometer", "npn", "pnp", "nmos", "pmos", "battery", "connector2", "buzzer", "crystal", "fuse",
        "inductor", "opamp", "voltage_source_ac",
    ] {
        let d = &all.iter().find(|(_, d)| d.ty == t).unwrap().1;
        let b = d.breadboard.as_ref().unwrap_or_else(|| panic!("{t}: no breadboard view"));
        assert!(b.svg.is_some(), "{t}");
        assert!(!b.pins.is_empty(), "{t}");
        // parts with a schematic keep it
        assert!(d.schematic.is_some(), "{t}");
    }
}

#[test]
fn pushbutton_joins_the_always_connected_legs() {
    let all = parts();
    let d = &all.iter().find(|(_, d)| d.ty == "pushbutton").unwrap().1;
    assert_eq!(d.buses, vec![vec!["a", "a2"], vec!["b", "b2"]]);
    assert_eq!(d.breadboard.as_ref().unwrap().pins.len(), 4);
    assert_eq!(d.schematic.as_ref().unwrap().pins.len(), 2);
}

#[test]
fn boards_are_consistent() {
    let all = parts();
    for (ty, cols, tie_points, rail_holes) in
        [("breadboard_mini", 17usize, 170usize, 0usize), ("breadboard_half", 30, 400, 25), ("breadboard_full", 63, 830, 50)]
    {
        let d = &all.iter().find(|(_, d)| d.ty == ty).unwrap().1;
        assert!(d.is_board(), "{ty}");
        assert!(d.schematic.is_none(), "{ty}: boards have no schematic view");
        assert_eq!(d.pins.len(), tie_points, "{ty}: tie points");
        let v = d.breadboard.as_ref().unwrap();
        assert_eq!(v.pins.len(), tie_points, "{ty}: placed pins");
        let ids: HashSet<&str> = d.pins.iter().map(|p| p.id.as_str()).collect();
        // every bus member is a pin of the breadboard view, no pin in two buses
        let mut in_bus = HashSet::new();
        for bus in &d.buses {
            for id in bus {
                assert!(v.pins.contains_key(id), "{ty}: bus member {id} not in the breadboard view");
                assert!(in_bus.insert(id.as_str()), "{ty}: {id} is in two buses");
            }
        }
        assert_eq!(in_bus, ids, "{ty}: every pin belongs to a bus");
        let rails = if rail_holes > 0 { 4 } else { 0 };
        assert_eq!(d.buses.len(), cols * 2 + rails, "{ty}: bus count");
        // column halves have 5 holes, rails have rail_holes
        for bus in &d.buses {
            assert!(bus.len() == 5 || bus.len() == rail_holes, "{ty}: odd bus size {}", bus.len());
        }
        // holes are on distinct grid points
        let pts: HashSet<[i32; 2]> = v.pins.values().map(|p| p.at).collect();
        assert_eq!(pts.len(), tie_points, "{ty}: two holes share a point");
        // spacing: a-e consecutive, gap of 3 between e and f
        let y = |id: &str| v.pins[id].at[1];
        assert_eq!(y("b1") - y("a1"), 1);
        assert_eq!(y("e1") - y("d1"), 1);
        assert_eq!(y("f1") - y("e1"), 3);
        assert_eq!(v.pins["a2"].at[0] - v.pins["a1"].at[0], 1);
        assert!(d.meta.as_ref().is_some_and(|m| m.contains_key("pin_naming")), "{ty}: meta.pin_naming");
        // strip bus: a-e of one column
        assert!(d.buses.contains(&vec!["a1".into(), "b1".into(), "c1".into(), "d1".into(), "e1".into()]));
        assert!(d.buses.contains(&vec!["f1".into(), "g1".into(), "h1".into(), "i1".into(), "j1".into()]));
        if rail_holes > 0 {
            let last = format!("t+{rail_holes}");
            assert!(ids.contains(last.as_str()) && ids.contains("t-1") && ids.contains("b+1") && ids.contains("b-1"));
            assert_eq!(y("t-1") - y("t+1"), 1);
            assert!(y("t-1") < y("a1") && y("j1") < y("b-1"), "rails above and below the strips");
        }
    }
}
