//! Single-part views: resolving a type by name, a one-part circuit for the part browser, and the
//! `--render-part` command (the drawing itself is `short_circ_core::svg::render_part_svg`).

use short_circ_core::{builtin, Circuit, Library, Part, PartDef, Placement, View};
use std::path::Path;

/// Id of the placed part in the one-part circuit (the one `render_part_svg` uses too).
pub use short_circ_core::svg::PART_ID;

/// Resolve a type name the way a circuit would: `parts/` next to `circuit_dir`, the exe's
/// `parts/`, built-ins, then generated families (`dipN`, `headerN`).
pub fn resolve_type(ty: &str, circuit_dir: Option<&Path>) -> Option<PartDef> {
    let probe = Circuit { parts: vec![placed(ty, View::Schematic)], ..Default::default() };
    Library::load(&probe, circuit_dir).get(ty).cloned()
}

/// Resolve against an existing library, falling back to the generated families.
pub fn resolve_in(lib: &Library, ty: &str) -> Option<PartDef> {
    lib.get(ty).cloned().or_else(|| builtin::generate(ty))
}

fn placed(ty: &str, view: View) -> Part {
    let mut p = Part { id: PART_ID.into(), ty: ty.into(), hidden_label: true, ..Part::default() };
    *p.placement_mut(view) = Some(Placement::at(0, 0));
    p
}

/// A circuit holding exactly `def` (inlined, so no resolution is needed to draw it) placed at the
/// origin of `view`.
pub fn preview_circuit(def: &PartDef, view: View) -> Circuit {
    Circuit { parts: vec![placed(&def.ty, view)], lib: vec![def.clone()], ..Default::default() }
}

/// `--render-part TYPE OUT.svg [--view V] [--dark] [--circuit-dir DIR]`. Exit 0 written, 1 unknown
/// type, a part without that view, or write error.
pub fn run(ty: &str, out: &Path, dark: bool, circuit_dir: Option<&Path>, view: View) -> i32 {
    let Some(def) = resolve_type(ty, circuit_dir) else {
        eprintln!("error: unknown part type {ty:?} (see --list-parts; dipN and headerN are generated)");
        return 1;
    };
    let svg = match short_circ_core::svg::render_part_svg(&def, view, dark) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    match std::fs::write(out, svg) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: cannot write {}: {e}", out.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_and_builtin_types_resolve() {
        assert_eq!(resolve_type("dip8", None).unwrap().pins.len(), 8);
        assert_eq!(resolve_type("header4", None).unwrap().pins.len(), 4);
        assert_eq!(resolve_type("resistor", None).unwrap().pins.len(), 2);
        assert!(resolve_type("nonsense_part", None).is_none());
        assert!(resolve_type("dip7", None).is_none(), "odd DIP is not a family member");
        let lib = Library::builtin();
        assert!(resolve_in(&lib, "dip14").is_some());
        assert!(resolve_in(&lib, "resistor").is_some());
        assert!(resolve_in(&lib, "zzz").is_none());
    }

    #[test]
    fn preview_circuit_places_one_inlined_part_in_the_view() {
        let def = resolve_type("dip8", None).unwrap();
        for view in View::ALL {
            let c = preview_circuit(&def, view);
            assert_eq!(c.parts.len(), 1);
            assert_eq!(c.parts[0].ty, "dip8");
            assert_eq!(c.lib.len(), 1, "definition is inlined");
            assert!(c.parts[0].placement(view).is_some() && c.parts[0].placement(view.other()).is_none());
            assert!(c.parts[0].hidden_label);
        }
    }

    #[test]
    fn render_part_writes_svg_with_pin_labels() {
        let dir = std::env::temp_dir().join(format!("short-circ-partview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("p.svg");
        let sch = View::Schematic;
        assert_eq!(run("dip8", &out, false, None, sch), 0);
        let svg = std::fs::read_to_string(&out).unwrap();
        assert!(svg.starts_with("<svg"));
        assert_eq!(svg.matches("class=\"label\"").count(), 8, "one label per pin");
        assert!(svg.contains(">8</text>"));
        assert_eq!(run("no_such_type", &out, true, None, sch), 1);
        assert_eq!(run("dip8", &dir.join("no/such/dir/p.svg"), false, None, sch), 1, "write failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_view_is_exit_1_and_present_view_renders() {
        let dir = std::env::temp_dir().join(format!("short-circ-partview-views-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("v.svg");
        // A board has only a breadboard view, ground only a schematic one.
        assert_eq!(run("breadboard_half", &out, false, None, View::Schematic), 1);
        assert_eq!(run("ground", &out, false, None, View::Breadboard), 1);
        assert_eq!(run("led", &out, false, None, View::Breadboard), 0);
        assert!(std::fs::read_to_string(&out).unwrap().contains("class=\"art\""), "artwork embedded");
        assert_eq!(run("breadboard_half", &out, true, None, View::Breadboard), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn circuit_dir_parts_are_found() {
        let dir = std::env::temp_dir().join(format!("short-circ-partview-dir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("parts")).unwrap();
        std::fs::write(
            dir.join("parts").join("widget.part"),
            r#"{"format":"short-circ/1","type":"widget","size":[4,2],"pins":[{"id":"p","at":[0,1],"name":"IN"}]}"#,
        )
        .unwrap();
        assert!(resolve_type("widget", None).is_none());
        let d = resolve_type("widget", Some(&dir)).unwrap();
        let svg = short_circ_core::svg::render_part_svg(&d, View::Schematic, false).unwrap();
        assert!(svg.contains(">p:IN</text>"), "{svg}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
