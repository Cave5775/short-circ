//! Label visibility (refs, values, wire ids, pin names) and the legibility rule for text.
//! The placement math is pure; `draw_overlay` paints the optional wire id and pin name labels.

use crate::canvas::DrawCtx;
use eframe::egui::{Align2, Color32, FontId, Pos2};
use short_circ_core::transform::{pin_world, to_world};
use short_circ_core::{Circuit, Dir, Library, PartDef, PinDef, Placement, Pt, View};

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
pub fn inward_dir(pl: &Placement, dir: Dir) -> [f64; 2] {
    let out = match dir {
        Dir::Left => [-1.0, 0.0],
        Dir::Right => [1.0, 0.0],
        Dir::Up => [0.0, -1.0],
        Dir::Down => [0.0, 1.0],
    };
    let o = to_world(pl, [0.0, 0.0]);
    let w = to_world(pl, out);
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

pub(crate) fn paint_text(d: &DrawCtx, at: Pos2, align: Align2, text: &str, size_units: f64, color: Color32) {
    let Some(px) = text_px(size_units, d.cam.scale) else { return };
    paint_legible(d.painter, at, align, text, px, color, d.theme.bg);
}

/// WCAG relative luminance of an opaque colour, 0 (black) to 1 (white).
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.039_28 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio between two colours, 1 (identical) to 21 (black on white).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Minimum contrast text keeps against the background before its colour is corrected.
pub const MIN_CONTRAST: f32 = 3.5;

/// `color`, pushed towards black or white (whichever the background is farther from) just far
/// enough to reach `MIN_CONTRAST` against `bg`. Colours that already read are returned as is, so a
/// deliberate palette survives; only the hue-lost-in-the-background case is corrected.
pub fn legible_on(color: Color32, bg: Color32) -> Color32 {
    if contrast(color, bg) >= MIN_CONTRAST {
        return color;
    }
    let target = if luminance(bg) > 0.4 { Color32::BLACK } else { Color32::WHITE };
    for step in 1..=10 {
        let t = step as f32 / 10.0;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        let c = Color32::from_rgb(mix(color.r(), target.r()), mix(color.g(), target.g()), mix(color.b(), target.b()));
        if contrast(c, bg) >= MIN_CONTRAST {
            return c;
        }
    }
    target
}

/// Outline colour for `text`: the opposite end of the lightness scale, mostly opaque.
pub fn halo_for(text: Color32) -> Color32 {
    if luminance(text) > 0.4 {
        Color32::from_rgba_unmultiplied(0, 0, 0, 190)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 190)
    }
}

/// Draw `text` so it stays readable on whatever is under it: the colour is corrected against the
/// canvas background `bg` and a thin halo of the opposite lightness keeps it apart from part fills
/// and breadboard artwork of a similar colour.
pub(crate) fn paint_legible(painter: &eframe::egui::Painter, at: Pos2, align: Align2, text: &str, px: f32, color: Color32, bg: Color32) {
    let color = legible_on(color, bg);
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(px), Color32::PLACEHOLDER);
    let origin = align.anchor_size(at, galley.size()).min;
    let r = (px * 0.07).clamp(0.8, 1.6);
    let halo = halo_for(color);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-0.7, -0.7), (0.7, -0.7), (-0.7, 0.7), (0.7, 0.7)] {
        painter.galley(origin + eframe::egui::vec2(dx * r, dy * r), galley.clone(), halo);
    }
    painter.galley(origin, galley, color);
}

/// Paint wire ids and pin names of `view` when enabled. Cheap when both are off.
pub fn draw_overlay(d: &DrawCtx, circuit: &Circuit, lib: &Library, view: View) {
    if d.labels.wire_ids && text_px(WIRE_LABEL_SIZE, d.cam.scale).is_some() {
        for w in circuit.wires_in(view) {
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
    if d.labels.pin_names && text_px(PIN_LABEL_SIZE, d.cam.scale).is_some() {
        for part in &circuit.parts {
            let (Some(pl), Some(def)) = (part.placement(view), lib.get(&part.ty)) else { continue };
            draw_pin_names(d, pl, def, view);
        }
    }
}

fn draw_pin_names(d: &DrawCtx, pl: &Placement, def: &PartDef, view: View) {
    // Board holes are too many and too close together for labels.
    if view == View::Breadboard && def.is_board() {
        return;
    }
    for (pin, pos) in def.placed_pins(view) {
        let at = pin_world(pl, pos);
        let (off, align) = match view {
            View::Schematic => pin_label_place(inward_dir(pl, pos.dir)),
            View::Breadboard => ([0.3, -0.3], Align2::LEFT_BOTTOM),
        };
        let p = d.pt([at[0] as f64 + off[0], at[1] as f64 + off[1]]);
        paint_text(d, p, align, &pin_label_text(pin), PIN_LABEL_SIZE, d.theme.pin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(rot: i32, mirror: bool) -> Placement {
        Placement { at: [10, 10], rot, mirror }
    }

    fn pin(id: &str, name: Option<&str>) -> PinDef {
        PinDef { id: id.into(), name: name.map(String::from) }
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
    fn contrast_ratio_matches_wcag() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn similar_colour_is_pushed_apart_and_good_colour_is_kept() {
        let dark_bg = Color32::from_rgb(24, 26, 30);
        let light_bg = Color32::from_rgb(250, 250, 247);
        // Dark blue on a dark background and pale yellow on a light one are unreadable.
        for (c, bg) in [(Color32::from_rgb(30, 40, 90), dark_bg), (Color32::from_rgb(240, 235, 160), light_bg)] {
            assert!(contrast(c, bg) < MIN_CONTRAST);
            let fixed = legible_on(c, bg);
            assert!(contrast(fixed, bg) >= MIN_CONTRAST, "{c:?} on {bg:?} -> {fixed:?}");
        }
        // Colours that already read are untouched (a deliberate palette survives).
        let green = Color32::from_rgb(86, 200, 120);
        assert_eq!(legible_on(green, dark_bg), green);
        // Same colour as the background: moved to the far side, just far enough to read.
        let up = legible_on(dark_bg, dark_bg);
        assert!(contrast(up, dark_bg) >= MIN_CONTRAST && luminance(up) > luminance(dark_bg));
        let down = legible_on(light_bg, light_bg);
        assert!(contrast(down, light_bg) >= MIN_CONTRAST && luminance(down) < luminance(light_bg));
    }

    #[test]
    fn halo_is_opposite_of_the_text() {
        assert_eq!(halo_for(Color32::WHITE).to_srgba_unmultiplied()[..3], [0, 0, 0]);
        assert_eq!(halo_for(Color32::BLACK).to_srgba_unmultiplied()[..3], [255, 255, 255]);
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
