//! Label visibility (refs, values, wire ids, pin names) and the legibility rule for text.
//! The placement math is pure; `draw_overlay` paints the optional wire id and pin name labels.

use crate::canvas::DrawCtx;
use eframe::egui::{Align2, Color32, FontId, Pos2};
use short_circ_core::transform::{pin_world, to_world};
use short_circ_core::{Circuit, Dir, Library, PartDef, PinDef, Pt};

/// Text smaller than this many pixels is not drawn at all (it would be an unreadable smudge).
pub const MIN_TEXT_PX: f32 = 7.0;
/// Size in grid units of pin name labels and wire id labels.
pub const PIN_LABEL_SIZE: f64 = 0.75;
pub const WIRE_LABEL_SIZE: f64 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelOpts {
    /// Part reference designators (`{ref}` text in part shapes).
    pub refs: bool,
    /// Part values (`{value}` text in part shapes).
    pub values: bool,
    pub wire_ids: bool,
    pub pin_names: bool,
}

impl Default for LabelOpts {
    fn default() -> Self {
        LabelOpts { refs: true, values: true, wire_ids: false, pin_names: false }
    }
}

/// Pixel height for text of `size_units` grid units at `scale` pixels per unit, or `None` when
/// it would be too small to read and should be hidden.
pub fn text_px(size_units: f64, scale: f32) -> Option<f32> {
    let px = size_units as f32 * scale;
    (px >= MIN_TEXT_PX).then_some(px)
}

/// Text shown for a pin: its name if it has a non-empty one, otherwise its id.
pub fn pin_label_text(pin: &PinDef) -> String {
    match pin.name.as_deref().map(str::trim) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => pin.id.clone(),
    }
}

/// Unit vector (in world axes) pointing from the pin into the part body, given the pin's
/// declared outward direction and the part's rotation/mirror.
pub fn inward_dir(part: &short_circ_core::Part, dir: Dir) -> [f64; 2] {
    let out = match dir {
        Dir::Left => [-1.0, 0.0],
        Dir::Right => [1.0, 0.0],
        Dir::Up => [0.0, -1.0],
        Dir::Down => [0.0, 1.0],
    };
    let o = to_world(part, [0.0, 0.0]);
    let w = to_world(part, out);
    [o[0] - w[0], o[1] - w[1]]
}

/// Offset from the pin (grid units) and alignment for its label: inside the body, just above
/// the lead for horizontal pins and beside the lead for vertical ones.
pub fn pin_label_place(inward: [f64; 2]) -> ([f64; 2], Align2) {
    if inward[0].abs() >= inward[1].abs() {
        if inward[0] >= 0.0 {
            ([0.2, -0.15], Align2::LEFT_BOTTOM)
        } else {
            ([-0.2, -0.15], Align2::RIGHT_BOTTOM)
        }
    } else if inward[1] >= 0.0 {
        ([0.25, 0.6], Align2::LEFT_CENTER)
    } else {
        ([0.25, -0.6], Align2::LEFT_CENTER)
    }
}

/// Where to put a wire's id: the middle of its longest segment, with a flag telling whether that
/// segment is horizontal (text goes above it) or vertical (text goes to its right).
pub fn wire_label_spot(pts: &[Pt]) -> Option<([f64; 2], bool)> {
    let mut best: Option<(i64, [f64; 2], bool)> = None;
    for s in pts.windows(2) {
        let (dx, dy) = ((s[1][0] - s[0][0]) as i64, (s[1][1] - s[0][1]) as i64);
        let len = dx.abs() + dy.abs();
        if len == 0 {
            continue;
        }
        if best.is_none_or(|(b, _, _)| len > b) {
            let mid = [(s[0][0] + s[1][0]) as f64 / 2.0, (s[0][1] + s[1][1]) as f64 / 2.0];
            best = Some((len, mid, dx.abs() >= dy.abs()));
        }
    }
    best.map(|(_, m, h)| (m, h))
}

fn paint_text(d: &DrawCtx, at: Pos2, align: Align2, text: &str, size_units: f64, color: Color32) {
    let Some(px) = text_px(size_units, d.view.scale) else { return };
    d.painter.text(at, align, text, FontId::proportional(px), color);
}

/// Paint wire ids and pin names when enabled. Cheap when both are off.
pub fn draw_overlay(d: &DrawCtx, circuit: &Circuit, lib: &Library) {
    if d.labels.wire_ids && text_px(WIRE_LABEL_SIZE, d.view.scale).is_some() {
        for w in &circuit.wires {
            let Some((mid, horizontal)) = wire_label_spot(&w.pts) else { continue };
            let p = d.pt(mid);
            let (at, align) = if horizontal {
                (p + eframe::egui::vec2(0.0, -d.px(0.15)), Align2::CENTER_BOTTOM)
            } else {
                (p + eframe::egui::vec2(d.px(0.2), 0.0), Align2::LEFT_CENTER)
            };
            paint_text(d, at, align, &w.id, WIRE_LABEL_SIZE, d.theme.wire);
        }
    }
    if d.labels.pin_names && text_px(PIN_LABEL_SIZE, d.view.scale).is_some() {
        for part in &circuit.parts {
            let Some(def) = lib.get(&part.ty) else { continue };
            draw_pin_names(d, part, def);
        }
    }
}

fn draw_pin_names(d: &DrawCtx, part: &short_circ_core::Part, def: &PartDef) {
    for pin in &def.pins {
        let at = pin_world(part, pin);
        let (off, align) = pin_label_place(inward_dir(part, pin.dir));
        let p = d.pt([at[0] as f64 + off[0], at[1] as f64 + off[1]]);
        paint_text(d, p, align, &pin_label_text(pin), PIN_LABEL_SIZE, d.theme.pin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use short_circ_core::Part;
    use std::collections::BTreeMap;

    fn part(rot: i32, mirror: bool) -> Part {
        Part { id: "U".into(), ty: "t".into(), at: [10, 10], rot, mirror, props: BTreeMap::new(), hidden_label: false, ..Part::default() }
    }

    fn pin(id: &str, name: Option<&str>) -> PinDef {
        PinDef { id: id.into(), at: [0, 1], dir: Dir::Left, name: name.map(String::from) }
    }

    #[test]
    fn threshold_hides_small_text() {
        assert_eq!(text_px(0.9, 5.0), None);
        assert_eq!(text_px(0.9, 7.0), None, "6.3 px is below the limit");
        assert_eq!(text_px(0.9, 10.0), Some(9.0));
        assert!(text_px(0.75, 240.0).is_some());
        // Exactly at the limit is shown.
        assert_eq!(text_px(1.0, MIN_TEXT_PX), Some(MIN_TEXT_PX));
    }

    #[test]
    fn defaults_show_refs_and_values_only() {
        let o = LabelOpts::default();
        assert!(o.refs && o.values && !o.wire_ids && !o.pin_names);
    }

    #[test]
    fn pin_text_prefers_name() {
        assert_eq!(pin_label_text(&pin("3", Some("OUT"))), "OUT");
        assert_eq!(pin_label_text(&pin("3", Some("  "))), "3");
        assert_eq!(pin_label_text(&pin("3", None)), "3");
    }

    #[test]
    fn inward_follows_rotation_and_mirror() {
        assert_eq!(inward_dir(&part(0, false), Dir::Left), [1.0, 0.0]);
        assert_eq!(inward_dir(&part(0, true), Dir::Left), [-1.0, 0.0]);
        // Rot 90 clockwise: a left lead points up, so the body is below the pin.
        assert_eq!(inward_dir(&part(90, false), Dir::Left), [0.0, 1.0]);
        assert_eq!(inward_dir(&part(180, false), Dir::Up), [0.0, -1.0]);
    }

    #[test]
    fn placement_sides() {
        assert_eq!(pin_label_place([1.0, 0.0]).1, Align2::LEFT_BOTTOM);
        assert_eq!(pin_label_place([-1.0, 0.0]).1, Align2::RIGHT_BOTTOM);
        let (off, al) = pin_label_place([0.0, 1.0]);
        assert_eq!(al, Align2::LEFT_CENTER);
        assert!(off[1] > 0.0);
        assert!(pin_label_place([0.0, -1.0]).0[1] < 0.0);
    }

    #[test]
    fn wire_label_on_longest_segment() {
        let pts: Vec<Pt> = vec![[0, 0], [2, 0], [2, 10]];
        assert_eq!(wire_label_spot(&pts), Some(([2.0, 5.0], false)));
        let flat: Vec<Pt> = vec![[0, 0], [8, 0]];
        assert_eq!(wire_label_spot(&flat), Some(([4.0, 0.0], true)));
        assert_eq!(wire_label_spot(&[[1, 1], [1, 1]]), None);
        assert_eq!(wire_label_spot(&[]), None);
    }
}
