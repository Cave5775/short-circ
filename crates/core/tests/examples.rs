//! Validates the shipped examples end to end through the public API.

use short_circ_core::{parse_circuit, validate::validate, Library, Netlist, Severity, View};

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
    let nl = Netlist::build(&c, &lib, View::Schematic);
    assert_eq!(nl.nets.len(), 3);
}

#[test]
fn colors_example_validates_without_issues_and_renders_its_colours() {
    let (c, lib) = load(include_str!("../../../examples/colors.circ"));
    let r = validate(&c, &lib);
    assert_eq!(r.errors, 0, "{:#?}", r.issues);
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
    let svg = short_circ_core::svg::render_svg(&c, &lib, false, View::Schematic);
    for hex in ["#fff8e1", "#d93025", "#1a73e8", "#00897b", "#f57c00", "#6a1b9a", "#b71c1c", "#ffe0b2", "#8d5a2b"] {
        assert!(svg.contains(hex), "{hex} missing from SVG");
    }
    // Dark and light differ only in defaults; the explicit theme/part colours are untouched.
    let dark = short_circ_core::svg::render_svg(&c, &lib, true, View::Schematic);
    assert!(dark.contains("#1a73e8") && dark.contains("#fff8e1"));
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

fn example(name: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    std::fs::read_to_string(root.join(name)).unwrap()
}

#[test]
fn led_example_holds_both_views_and_agrees_across_them() {
    let (c, lib) = load(&example("led.circ"));
    assert!(short_circ_core::view_in_use(&c, View::Schematic));
    assert!(short_circ_core::view_in_use(&c, View::Breadboard));
    let r = validate(&c, &lib);
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
}

#[test]
fn every_example_except_bad_validates_without_errors_and_is_canonical() {
    for name in ["led.circ", "colors.circ", "breadboard_demo.circ", "nets_only.circ", "555_blinker.circ"] {
        let src = example(name).replace("\r\n", "\n");
        let (c, lib) = load(&src);
        let r = validate(&c, &lib);
        assert_eq!(r.errors, 0, "{name}: {:#?}", r.issues);
        let written = short_circ_core::write::circuit_to_string(&c);
        assert_eq!(written, src, "{name} is not in canonical form (run --upgrade)");
    }
}

#[test]
fn nets_only_example_has_declared_nets_and_no_placements() {
    let (c, lib) = load(&example("nets_only.circ"));
    assert!(c.wires.is_empty() && !c.nets.is_empty());
    assert!(c.parts.iter().all(|p| p.schematic.is_none() && p.breadboard.is_none()));
    assert_eq!(validate(&c, &lib).errors, 0);
}

#[test]
fn blinker_example_is_fully_wired_in_both_views() {
    let (c, lib) = load(&example("555_blinker.circ"));
    // No net-open / net-merged / unplaced-part: both views realise every declared net.
    let r = validate(&c, &lib);
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
    assert!(short_circ_core::view_in_use(&c, View::Schematic));
    assert!(short_circ_core::view_in_use(&c, View::Breadboard));
}

/// The circuit skill inlines docs/FORMAT.md so Claude has the whole format when the skill loads.
/// If this fails, run `powershell -File scripts/sync-skill.ps1`.
#[test]
fn skill_contains_the_current_format_spec() {
    // Read at run time (not include_str!) so a stale build cache can never hide drift.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let spec = std::fs::read_to_string(root.join("docs/FORMAT.md")).unwrap();
    let skill = std::fs::read_to_string(root.join(".claude/skills/circuit/SKILL.md")).unwrap();
    let strip = |l: &str| l.trim_start_matches('#').trim().to_string();
    let skill_lines: std::collections::HashSet<String> =
        skill.lines().map(|l| strip(l.trim_end())).collect();
    let missing: Vec<&str> = spec
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !strip(l).is_empty() && !skill_lines.contains(&strip(l)))
        .take(5)
        .collect();
    assert!(
        missing.is_empty(),
        "SKILL.md is out of date with docs/FORMAT.md (run scripts/sync-skill.ps1); first missing lines: {missing:?}"
    );
}
