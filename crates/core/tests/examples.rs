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
fn colors_example_validates_without_issues_and_renders_its_colours() {
    let (c, lib) = load(include_str!("../../../examples/colors.circ"));
    let r = validate(&c, &lib);
    assert_eq!(r.errors, 0, "{:#?}", r.issues);
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
    let svg = short_circ_core::svg::render_svg(&c, &lib, false);
    for hex in ["#fff8e1", "#d93025", "#1a73e8", "#00897b", "#f57c00", "#6a1b9a", "#b71c1c", "#ffe0b2", "#8d5a2b"] {
        assert!(svg.contains(hex), "{hex} missing from SVG");
    }
    // Dark and light differ only in defaults; the explicit theme/part colours are untouched.
    let dark = short_circ_core::svg::render_svg(&c, &lib, true);
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
