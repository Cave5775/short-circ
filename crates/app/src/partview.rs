//! Single-part views: a one-part circuit with every pin labelled. Used by the `--render-part`
//! command and by the part browser preview, so imported components can be checked visually.

use crate::core_api;
use short_circ_core::{builtin, Circuit, Label, Library, Part, PartDef, Shape, Style};
use std::collections::BTreeMap;
use std::path::Path;

/// Id of the placed part in the one-part circuit.
pub const PART_ID: &str = "X";

/// Resolve a type name the way a circuit would: `parts/` next to `circuit_dir`, the exe's
/// `parts/`, built-ins, then generated families (`dipN`, `headerN`).
pub fn resolve_type(ty: &str, circuit_dir: Option<&Path>) -> Option<PartDef> {
    let probe = Circuit { parts: vec![placed(ty)], ..Default::default() };
    Library::load(&probe, circuit_dir).get(ty).cloned()
}

/// Resolve against an existing library, falling back to the generated families.
pub fn resolve_in(lib: &Library, ty: &str) -> Option<PartDef> {
    lib.get(ty).cloned().or_else(|| builtin::generate(ty))
}

fn placed(ty: &str) -> Part {
    Part { id: PART_ID.into(), ty: ty.into(), at: [0, 0], rot: 0, mirror: false, props: BTreeMap::new(), hidden_label: true }
}

/// Label for a pin: its id, plus its name when it has a different one.
pub fn pin_caption(id: &str, name: Option<&str>) -> String {
    match name.map(str::trim) {
        Some(n) if !n.is_empty() && n != id => format!("{id}:{n}"),
        _ => id.to_string(),
    }
}

/// One-line description shown as the title: `type - Name (category) WxH, N pins`.
pub fn describe(def: &PartDef) -> String {
    let mut s = def.ty.clone();
    if !def.name.is_empty() && def.name != def.ty {
        s.push_str(&format!(" - {}", def.name));
    }
    if let Some(c) = def.category.as_deref().filter(|c| !c.is_empty()) {
        s.push_str(&format!(" ({c})"));
    }
    s.push_str(&format!(", {}x{}, {} pin(s)", fmt_num(def.size[0]), fmt_num(def.size[1]), def.pins.len()));
    s
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// A circuit holding exactly `def` (inlined, so no resolution is needed to draw it) placed at
/// the origin, with a net label at every pin carrying the pin id/name, a thin outline of the
/// declared bounding box, and a title describing the part.
pub fn single_part_circuit(def: &PartDef) -> Circuit {
    let mut def = def.clone();
    def.shapes.push(Shape::Rect {
        at: [0.0, 0.0],
        size: def.size,
        r: 0.0,
        style: Style { stroke: Some("#8a8a8a".into()), fill: None, width: Some(0.04) },
    });
    let labels = def
        .pins
        .iter()
        .map(|p| Label { at: p.at, name: pin_caption(&p.id, p.name.as_deref()) })
        .collect();
    Circuit {
        title: Some(describe(&def)),
        parts: vec![placed(&def.ty)],
        labels,
        lib: vec![def],
        ..Default::default()
    }
}

/// `--render-part TYPE OUT.svg [--dark] [--circuit-dir DIR]`. Exit 0 written, 1 unknown type
/// or write error.
pub fn run(ty: &str, out: &Path, dark: bool, circuit_dir: Option<&Path>) -> i32 {
    let Some(def) = resolve_type(ty, circuit_dir) else {
        eprintln!("error: unknown part type {ty:?} (see --list-parts; dipN and headerN are generated)");
        return 1;
    };
    let circuit = single_part_circuit(&def);
    let lib = Library::load(&circuit, circuit_dir);
    let svg = match core_api::render_svg(&circuit, &lib, dark) {
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
    fn one_part_circuit_labels_every_pin() {
        let def = resolve_type("dip8", None).unwrap();
        let c = single_part_circuit(&def);
        assert_eq!(c.parts.len(), 1);
        assert_eq!(c.parts[0].ty, "dip8");
        assert_eq!(c.labels.len(), 8);
        assert_eq!(c.labels[0].name, "1");
        assert_eq!(c.labels[0].at, def.pins[0].at);
        assert_eq!(c.lib.len(), 1, "definition is inlined");
        assert_eq!(c.lib[0].shapes.len(), def.shapes.len() + 1, "bbox outline added");
        assert!(c.title.as_deref().unwrap().starts_with("dip8 - DIP-8 (ic), 6x8, 8 pin(s)"));
        // The original is untouched.
        assert_eq!(def.shapes.len(), c.lib[0].shapes.len() - 1);
    }

    #[test]
    fn captions_include_names() {
        assert_eq!(pin_caption("1", None), "1");
        assert_eq!(pin_caption("1", Some("VCC")), "1:VCC");
        assert_eq!(pin_caption("a", Some("a")), "a");
        assert_eq!(pin_caption("a", Some(" ")), "a");
    }

    #[test]
    fn describe_formats_size() {
        let mut d = resolve_type("resistor", None).unwrap();
        d.size = [8.0, 2.5];
        d.category = None;
        assert!(describe(&d).contains(", 8x2.5, 2 pin(s)"));
    }

    #[test]
    fn render_part_writes_svg_with_pin_labels() {
        let dir = std::env::temp_dir().join(format!("short-circ-partview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("p.svg");
        assert_eq!(run("dip8", &out, false, None), 0);
        let svg = std::fs::read_to_string(&out).unwrap();
        assert!(svg.starts_with("<svg"));
        assert_eq!(svg.matches("class=\"label\"").count(), 8, "one label per pin");
        assert!(svg.contains(">8</text>"));
        assert_eq!(run("no_such_type", &out, true, None), 1);
        assert_eq!(run("dip8", &dir.join("no/such/dir/p.svg"), false, None), 1, "write failure");
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
        assert_eq!(single_part_circuit(&d).labels[0].name, "p:IN");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
