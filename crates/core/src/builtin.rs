//! Built-in components embedded in the executable: one `include_str!` per file in /parts,
//! plus the parametric families `dipN` and `headerN` produced by [`generate`].

use crate::model::{Anchor, Dir, PartDef, PinDef, Shape, Style, FORMAT};

pub const BUILTIN: &[(&str, &str)] = &[
    ("resistor", include_str!("../../../parts/resistor.part")),
    ("battery", include_str!("../../../parts/battery.part")),
    ("led", include_str!("../../../parts/led.part")),
    ("capacitor", include_str!("../../../parts/capacitor.part")),
    ("capacitor_polarized", include_str!("../../../parts/capacitor_polarized.part")),
    ("diode", include_str!("../../../parts/diode.part")),
    ("zener", include_str!("../../../parts/zener.part")),
    ("inductor", include_str!("../../../parts/inductor.part")),
    ("switch", include_str!("../../../parts/switch.part")),
    ("pushbutton", include_str!("../../../parts/pushbutton.part")),
    ("ground", include_str!("../../../parts/ground.part")),
    ("vcc", include_str!("../../../parts/vcc.part")),
    ("potentiometer", include_str!("../../../parts/potentiometer.part")),
    ("npn", include_str!("../../../parts/npn.part")),
    ("pnp", include_str!("../../../parts/pnp.part")),
    ("nmos", include_str!("../../../parts/nmos.part")),
    ("pmos", include_str!("../../../parts/pmos.part")),
    ("opamp", include_str!("../../../parts/opamp.part")),
    ("crystal", include_str!("../../../parts/crystal.part")),
    ("fuse", include_str!("../../../parts/fuse.part")),
    ("buzzer", include_str!("../../../parts/buzzer.part")),
    ("voltage_source_ac", include_str!("../../../parts/voltage_source_ac.part")),
    ("connector2", include_str!("../../../parts/connector2.part")),
];

/// Parametric families by type name, e.g. "dip8", "header4". Returns None if `ty` is not one.
///
/// `dipN` (N even, 4..=40): two columns of N/2 pins each on a 2-unit pitch, columns 6 units
/// apart (x = 0 and x = 6). Pin ids "1".."N" are numbered anticlockwise as seen on a real
/// package with the notch at the top: pin 1 is top-left, numbers increase going down the
/// left column, then pin N/2+1 is bottom-right and numbers increase going up the right
/// column to pin N at top-right. With n = N/2 the pin k (1..=n) sits at (0, 2k-1) and pin
/// n+k sits at (6, 2n-2k+1). Size is [6, N]. Body rectangle x 1..5, full height, notch is a
/// half circle at the top edge and pin 1 is marked with a small circle.
///
/// `headerN` (N 1..=40): a single vertical column of pins "1".."N" at x = 0, y = 2k-1 (2-unit
/// pitch), pin 1 at the top. Size is [3, 2N]. Body rectangle x 1..3, each pin gets a lead
/// and a small square at x = 1.6..2.4.
pub fn generate(ty: &str) -> Option<PartDef> {
    if let Some(n) = family_count(ty, "dip") {
        if (4..=40).contains(&n) && n % 2 == 0 {
            return Some(dip(n));
        }
    } else if let Some(n) = family_count(ty, "header") {
        if (1..=40).contains(&n) {
            return Some(header(n));
        }
    }
    None
}

/// `prefix` followed by plain decimal digits only (no sign, no leading zero).
fn family_count(ty: &str, prefix: &str) -> Option<i32> {
    let rest = ty.strip_prefix(prefix)?;
    if rest.is_empty() || rest.starts_with('0') || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn line(pts: &[[f64; 2]]) -> Shape {
    Shape::Line { pts: pts.to_vec(), style: Style::default() }
}

fn text(at: [f64; 2], t: &str, anchor: Anchor) -> Shape {
    Shape::Text { at, text: t.to_string(), size: 0.9, anchor, style: Style::default() }
}

fn pin(id: i32, at: [i32; 2], dir: Dir) -> PinDef {
    PinDef { id: id.to_string(), at, dir, name: None }
}

fn dip(n: i32) -> PartDef {
    let half = n / 2;
    let mut pins = Vec::new();
    let mut shapes = Vec::new();
    for k in 1..=half {
        let y = 2 * k - 1;
        pins.push(pin(k, [0, y], Dir::Left));
        shapes.push(line(&[[0.0, y as f64], [1.0, y as f64]]));
        let y2 = 2 * half - 2 * k + 1;
        pins.push(pin(half + k, [6, y2], Dir::Right));
        shapes.push(line(&[[5.0, y2 as f64], [6.0, y2 as f64]]));
    }
    shapes.push(Shape::Rect { at: [1.0, 0.0], size: [4.0, n as f64], r: 0.0, style: Style::default() });
    shapes.push(Shape::Arc { at: [3.0, 0.0], r: 0.6, from: 0.0, to: 180.0, style: Style::default() });
    shapes.push(Shape::Circle { at: [1.7, 0.9], r: 0.2, style: Style::default() });
    shapes.push(text([3.0, -0.6], "{ref}", Anchor::Middle));
    shapes.push(text([3.0, n as f64 / 2.0 + 0.3], "{value}", Anchor::Middle));
    PartDef {
        format: FORMAT.to_string(),
        ty: format!("dip{n}"),
        name: format!("DIP-{n}"),
        category: Some("ic".to_string()),
        size: [6.0, n as f64],
        pins,
        shapes,
        params: Some(serde_json::json!({"gen": "dip", "pins": n})),
        svg: None,
        meta: None,
    }
}

fn header(n: i32) -> PartDef {
    let mut pins = Vec::new();
    let mut shapes = vec![Shape::Rect {
        at: [1.0, 0.0],
        size: [2.0, 2.0 * n as f64],
        r: 0.0,
        style: Style::default(),
    }];
    for k in 1..=n {
        let y = (2 * k - 1) as f64;
        pins.push(pin(k, [0, 2 * k - 1], Dir::Left));
        shapes.push(line(&[[0.0, y], [1.6, y]]));
        shapes.push(Shape::Rect {
            at: [1.6, y - 0.4],
            size: [0.8, 0.8],
            r: 0.0,
            style: Style::default(),
        });
    }
    shapes.push(text([2.0, -0.6], "{ref}", Anchor::Middle));
    shapes.push(text([2.0, 2.0 * n as f64 + 1.1], "{value}", Anchor::Middle));
    PartDef {
        format: FORMAT.to_string(),
        ty: format!("header{n}"),
        name: format!("Header 1x{n}"),
        category: Some("connector".to_string()),
        size: [3.0, 2.0 * n as f64],
        pins,
        shapes,
        params: Some(serde_json::json!({"gen": "header", "pins": n})),
        svg: None,
        meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn check_invariants(d: &PartDef) {
        assert!(d.size[0] > 0.0 && d.size[1] > 0.0, "{}: empty bbox", d.ty);
        assert!(!d.pins.is_empty(), "{}: no pins", d.ty);
        let mut seen = HashSet::new();
        for p in &d.pins {
            assert!(seen.insert(p.id.clone()), "{}: duplicate pin id {}", d.ty, p.id);
            let (x, y) = (p.at[0] as f64, p.at[1] as f64);
            assert!(
                x >= 0.0 && y >= 0.0 && x <= d.size[0] && y <= d.size[1],
                "{}: pin {} at {:?} outside bbox {:?}",
                d.ty,
                p.id,
                p.at,
                d.size
            );
        }
        assert!(!d.shapes.is_empty(), "{}: no shapes", d.ty);
    }

    #[test]
    fn every_builtin_parses_and_is_consistent() {
        let mut types = HashSet::new();
        for (name, src) in BUILTIN {
            let d = crate::parse_part(src).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(&d.ty, name, "type must match table key");
            assert_eq!(d.ty, d.ty.to_lowercase());
            assert_eq!(d.format, FORMAT);
            assert!(!d.name.is_empty(), "{name}: missing display name");
            assert!(types.insert(d.ty.clone()), "duplicate builtin {name}");
            check_invariants(&d);
        }
        assert!(BUILTIN.len() >= 23);
    }

    #[test]
    fn dip8_geometry() {
        let d = generate("dip8").unwrap();
        assert_eq!(d.ty, "dip8");
        assert_eq!(d.size, [6.0, 8.0]);
        assert_eq!(d.pins.len(), 8);
        let at = |id: &str| d.pins.iter().find(|p| p.id == id).unwrap().at;
        assert_eq!(at("1"), [0, 1]);
        assert_eq!(at("4"), [0, 7]);
        assert_eq!(at("5"), [6, 7]);
        assert_eq!(at("8"), [6, 1]);
        check_invariants(&d);
        for n in (4..=40).step_by(2) {
            check_invariants(&generate(&format!("dip{n}")).unwrap());
        }
    }

    #[test]
    fn header4_geometry() {
        let d = generate("header4").unwrap();
        assert_eq!(d.size, [3.0, 8.0]);
        assert_eq!(d.pins.len(), 4);
        assert_eq!(d.pins[0].id, "1");
        assert_eq!(d.pins[3].at, [0, 7]);
        check_invariants(&d);
        for n in 1..=40 {
            check_invariants(&generate(&format!("header{n}")).unwrap());
        }
    }

    #[test]
    fn generate_rejects_non_families() {
        for t in ["dip", "dip7", "dip2", "dip42", "dip08", "header0", "header41", "header+4", "resistor", "dipx", ""] {
            assert!(generate(t).is_none(), "{t}");
        }
    }

    #[test]
    fn generated_parts_round_trip_json() {
        let d = generate("dip14").unwrap();
        let s = serde_json::to_string(&d).unwrap();
        let back = crate::parse_part(&s).unwrap();
        assert_eq!(back.pins.len(), 14);
    }
}
