//! Validates the shipped examples end to end through the public API.

use short_circ_core::{parse_circuit, validate::validate, Library, Netlist, Severity};

fn load(src: &str) -> (short_circ_core::Circuit, Library) {
    let c = parse_circuit(src).unwrap();
    let lib = Library::load(&c, None);
    (c, lib)
}

#[test]
fn led_example_validates_without_errors() {
    let (c, lib) = load(include_str!("../../../examples/led.circ"));
    let r = validate(&c, &lib);
    assert_eq!(r.errors, 0, "{:#?}", r.issues);
    assert!(r.issues.iter().all(|i| i.severity == Severity::Warning));
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
    let nl = Netlist::build(&c, &lib);
    assert_eq!(nl.nets.len(), 3);
}

#[test]
fn bad_example_reports_the_deliberate_faults() {
    let (c, lib) = load(include_str!("../../../examples/bad.circ"));
    let r = validate(&c, &lib);
    let has = |rule: &str, ids: &[&str]| {
        r.issues.iter().any(|i| i.rule == rule && i.ids.iter().map(|s| s.as_str()).eq(ids.iter().copied()))
    };
    assert!(has("wire-overlap", &["w1", "w2"]));
    assert!(has("wire-crossing", &["w3", "w4"]));
    assert!(has("dangling-wire", &["w5"]));
    assert!(has("part-overlap", &["R1", "R2"]));
    assert!(!r.ok);
}
