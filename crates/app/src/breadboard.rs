//! Drawing of the breadboard view: boards first, then the parts (rasterised artwork, else their
//! `shapes`, else a placeholder box), pin dots, and the jumper wires on top.

use crate::canvas::{self, to_color32, DrawCtx, Highlight};
use crate::labels;
use crate::tex::TexCache;
use eframe::egui::{self, epaint, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke};
use short_circ_core::color::{Palette, Rgba};
use short_circ_core::transform::{pin_world, to_world, world_bbox};
use short_circ_core::{Circuit, Library, Part, PartDef, Placement, View};
use std::collections::HashSet;

/// Width of a jumper wire in grid units.
pub const JUMPER_WIDTH: f64 = 0.25;
/// Radius of the dot at each end of a jumper.
const JUMPER_END_R: f64 = 0.22;
/// Radius of the dot drawn at a pin of a real (non-board) part.
const PIN_R: f64 = 0.16;
/// How much everything not on the highlighted net fades.
const DIM: f32 = 0.3;

/// Indices of the parts placed in the breadboard view, boards first (stable otherwise), so
/// everything else sits on top of them.
pub fn draw_order(circuit: &Circuit, lib: &Library) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..circuit.parts.len()).filter(|&i| circuit.parts[i].breadboard.is_some()).collect();
    idx.sort_by_key(|&i| !lib.get(&circuit.parts[i].ty).is_some_and(PartDef::is_board));
    idx
}

/// The four screen corners of a part's box (local `[0,0]-[w,h]`) under its placement; the
/// textured quad is drawn between them, so mirror and rotation come out of the same transform
/// as everything else.
fn quad(d: &DrawCtx, pl: &Placement, size: [f64; 2]) -> [Pos2; 4] {
    [[0.0, 0.0], [size[0], 0.0], [size[0], size[1]], [0.0, size[1]]].map(|c| d.pt(to_world(pl, c)))
}

/// Add a textured quad (corners in the order top-left, top-right, bottom-right, bottom-left of the
/// unplaced box) to the painter.
fn draw_texture(d: &DrawCtx, tex: egui::TextureId, corners: [Pos2; 4]) {
    let uv = [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)];
    let mut mesh = epaint::Mesh::with_texture(tex);
    for (p, t) in corners.into_iter().zip(uv) {
        mesh.vertices.push(epaint::Vertex { pos: p, uv: t, color: Color32::WHITE });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    d.painter.add(Shape::mesh(mesh));
}

/// Dashed grey box with the part id: a part whose type has neither artwork nor shapes.
fn draw_placeholder(d: &DrawCtx, part: &Part, pl: &Placement, size: [f64; 2]) {
    let c = quad(d, pl, size);
    let pts = vec![c[0], c[1], c[2], c[3], c[0]];
    let stroke = Stroke::new(1.5, d.theme.grid_major);
    d.painter.extend(Shape::dashed_line(&pts, stroke, 6.0, 4.0));
    let center = d.pt(to_world(pl, [size[0] / 2.0, size[1] / 2.0]));
    let px = d.px(0.9).clamp(9.0, 40.0);
    d.painter.text(center, Align2::CENTER_CENTER, &part.id, FontId::proportional(px), d.theme.fg);
}

fn faded(c: Color32, dim: bool) -> Color32 {
    if dim {
        c.gamma_multiply(DIM)
    } else {
        c
    }
}

/// Jumper colour as drawn: its own, the theme's, else the net's preset (see core `svg`).
pub fn jumper_color32(w: &short_circ_core::Wire, circuit: &Circuit, pal: &Palette, by_net: Option<Rgba>) -> Option<Color32> {
    short_circ_core::svg::jumper_color(w, circuit, pal, by_net).map(to_color32)
}

/// Draw the breadboard view. `jumpers` is the net colour of each breadboard wire in file order
/// (`core_api::jumper_colors`). `hl` is the net to emphasise; everything else on the wiring
/// level fades while it is non-empty.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    d: &DrawCtx,
    tex: &mut TexCache,
    circuit: &Circuit,
    lib: &Library,
    jumpers: &[Option<Rgba>],
    hl: &Highlight,
    selected: Option<usize>,
) {
    let view = View::Breadboard;
    let t = &d.theme;
    let pal = Palette::for_circuit(circuit, t.dark);
    let dim = !hl.is_empty();
    let ppp = d.painter.ctx().pixels_per_point();
    let visible = d.painter.clip_rect().expand(8.0);
    let order = draw_order(circuit, lib);

    // Parts: boards first.
    for &i in &order {
        let part = &circuit.parts[i];
        let Some(pl) = part.breadboard.as_ref() else { continue };
        let Some(def) = lib.get(&part.ty) else {
            canvas::draw_unknown(d, part, pl);
            continue;
        };
        let Some(vd) = def.view(view) else {
            canvas::draw_unknown(d, part, pl);
            continue;
        };
        let (lo, hi) = world_bbox(pl, vd);
        let on_screen = Rect::from_two_pos(d.pt(lo), d.pt(hi)).intersects(visible);
        let mut drawn = false;
        if on_screen {
            if vd.svg.is_some() {
                let long_px = d.px(vd.size[0].max(vd.size[1])) * ppp;
                if let Some(id) = tex.get(d.painter.ctx(), &part.ty, vd, long_px) {
                    draw_texture(d, id, quad(d, pl, vd.size));
                    drawn = true;
                }
            }
            if !vd.shapes.is_empty() {
                canvas::draw_shapes(d, &pal, part, pl, &vd.shapes);
                drawn = true;
            }
            if !drawn {
                draw_placeholder(d, part, pl, vd.size);
            }
        }
        if selected == Some(i) {
            canvas::draw_selection(d, lo, hi);
        }
    }

    // Part names above the parts (not for boards).
    for &i in &order {
        draw_caption(d, circuit, lib, i, &pal);
    }

    // Pin dots of real parts; holes of boards are part of the artwork and only light up.
    let lit_pin = |part: &str, pin: &str| hl.pins.iter().any(|(p, n)| p == part && n == pin);
    let r = d.px(PIN_R).max(1.8);
    for &i in &order {
        let part = &circuit.parts[i];
        let (Some(pl), Some(def)) = (part.breadboard.as_ref(), lib.get(&part.ty)) else { continue };
        if def.is_board() {
            continue;
        }
        for (pin, pos) in def.placed_pins(view) {
            let at = pin_world(pl, pos);
            let c = d.pt([at[0] as f64, at[1] as f64]);
            if lit_pin(&part.id, &pin.id) {
                d.painter.circle_filled(c, r * 1.9, t.highlight);
                d.painter.circle_stroke(c, r * 1.9, Stroke::new(1.0, t.fg.gamma_multiply(0.7)));
            } else if let Some(pc) = pal.pin {
                d.painter.circle_filled(c, r, faded(to_color32(pc), dim));
            }
        }
    }
    draw_lit_holes(d, circuit, lib, hl);

    // Jumpers on top.
    let width = d.px(JUMPER_WIDTH).max(2.0);
    let mut wi = 0;
    for w in circuit.wires.iter().filter(|w| w.view == view) {
        let by_net = jumpers.get(wi).copied().flatten();
        wi += 1;
        let Some(base) = jumper_color32(w, circuit, &pal, by_net) else { continue };
        if w.pts.len() < 2 {
            continue;
        }
        let lit = hl.wires.iter().any(|x| x == &w.id);
        let pts: Vec<Pos2> = w.pts.iter().map(|p| d.pt([p[0] as f64, p[1] as f64])).collect();
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        let er = d.px(JUMPER_END_R).max(width * 0.6);
        if lit {
            let glow = Stroke::new(width + 5.0, t.highlight.gamma_multiply(0.85));
            d.painter.add(Shape::line(pts.clone(), glow));
            for p in [first, last] {
                d.painter.circle_filled(p, er + 2.5, glow.color);
            }
        }
        let col = faded(base, dim && !lit);
        d.painter.add(Shape::line(pts.clone(), Stroke::new(width, col)));
        // Round caps and end dots with a darker rim.
        let rim = Stroke::new(1.0, col.gamma_multiply(0.55));
        for p in [first, last] {
            d.painter.circle_filled(p, width * 0.5, col);
            d.painter.circle_filled(p, er, col);
            d.painter.circle_stroke(p, er, rim);
        }
        // Rounded joints at bends so thick lines do not show gaps.
        for p in &pts[1..pts.len() - 1] {
            d.painter.circle_filled(*p, width * 0.5, col);
        }
    }

    labels::draw_overlay(d, circuit, lib, view);
}

/// Highlight rings on the board holes of the highlighted net.
fn draw_lit_holes(d: &DrawCtx, circuit: &Circuit, lib: &Library, hl: &Highlight) {
    if hl.pins.is_empty() {
        return;
    }
    let boards: HashSet<&str> = circuit
        .parts
        .iter()
        .filter(|p| p.breadboard.is_some() && lib.get(&p.ty).is_some_and(PartDef::is_board))
        .map(|p| p.id.as_str())
        .collect();
    let r = d.px(0.3).max(3.0);
    for (part_id, pin_id) in &hl.pins {
        if !boards.contains(part_id.as_str()) {
            continue;
        }
        let Some(part) = circuit.part(part_id) else { continue };
        let (Some(pl), Some(def)) = (part.breadboard.as_ref(), lib.get(&part.ty)) else { continue };
        let Some(pos) = def.breadboard.as_ref().and_then(|v| v.pins.get(pin_id)) else { continue };
        let at = pin_world(pl, pos);
        let c = d.pt([at[0] as f64, at[1] as f64]);
        d.painter.circle_filled(c, r, d.theme.highlight.gamma_multiply(0.55));
        d.painter.circle_stroke(c, r, Stroke::new(1.2, d.theme.highlight));
    }
}

/// `R1 330` above a non-board part, when the label options ask for it and it is legible.
fn draw_caption(d: &DrawCtx, circuit: &Circuit, lib: &Library, i: usize, pal: &Palette) {
    const SIZE: f64 = 0.9;
    let part = &circuit.parts[i];
    let (Some(pl), Some(def)) = (part.breadboard.as_ref(), lib.get(&part.ty)) else { return };
    let Some(vd) = def.view(View::Breadboard) else { return };
    if def.is_board() || labels::text_px(SIZE, d.cam.scale).is_none() {
        return;
    }
    let mut text = String::new();
    if d.labels.refs && !part.hidden_label {
        text.push_str(&part.id);
    }
    if d.labels.values {
        if let Some(v) = part.props.get("value").filter(|v| !v.is_empty()) {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(v);
        }
    }
    if text.is_empty() {
        return;
    }
    let (lo, hi) = world_bbox(pl, vd);
    let at = d.pt([(lo[0] + hi[0]) / 2.0, lo[1] - 0.25]);
    let Some(col) = pal.part.or(pal.label) else { return };
    // Dark text on a light background and the other way round come from the theme palette.
    labels::paint_text(d, at, Align2::CENTER_BOTTOM, &text, SIZE, to_color32(col));
}

/// Pixel size of the pointer hit radius for a pin in the breadboard view, in grid units: about
/// 9 px but never more than a hole pitch allows (holes are 1 unit apart).
pub fn pin_hit_tolerance(px_per_unit: f64) -> f64 {
    (9.0 * px_per_unit).clamp(0.2, 0.45)
}

/// Hit radius for jumper wires in grid units: 6 px, at least the wire's own half width.
pub fn wire_hit_tolerance(px_per_unit: f64) -> f64 {
    (6.0 * px_per_unit).max(JUMPER_WIDTH / 2.0 + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> (Circuit, Library) {
        let c = short_circ_core::parse_circuit(include_str!("../../../examples/breadboard_demo.circ")).unwrap();
        let lib = Library::load(&c, None);
        (c, lib)
    }

    #[test]
    fn boards_are_drawn_before_parts() {
        let (c, lib) = demo();
        let order = draw_order(&c, &lib);
        assert_eq!(order.len(), c.parts.len(), "every demo part is placed in the breadboard view");
        let first = &c.parts[order[0]];
        assert!(lib.get(&first.ty).unwrap().is_board(), "{} first", first.id);
        let boards = order.iter().take_while(|&&i| lib.get(&c.parts[i].ty).unwrap().is_board()).count();
        assert!(boards >= 1 && order[boards..].iter().all(|&i| !lib.get(&c.parts[i].ty).unwrap().is_board()));
    }

    #[test]
    fn schematic_only_parts_are_not_in_the_order() {
        let mut c = short_circ_core::parse_circuit(include_str!("../../../examples/led.circ")).unwrap();
        let lib = Library::load(&c, None);
        assert!(draw_order(&c, &lib).is_empty());
        c.parts[0].breadboard = Some(Placement::at(1, 1));
        assert_eq!(draw_order(&c, &lib), vec![0]);
    }

    #[test]
    fn jumper_colour_prefers_own_then_theme_then_net() {
        let (mut c, lib) = demo();
        let nets = crate::core_api::jumper_colors(&c, &lib);
        let pal = Palette::for_circuit(&c, true);
        let wires: Vec<_> = c.wires_in(View::Breadboard).cloned().collect();
        assert!(wires.len() >= 2);
        let by_net = nets[0];
        let own = jumper_color32(&wires[0], &c, &pal, by_net).unwrap();
        // Two jumpers on different nets get different preset colours unless they carry their own.
        let other = jumper_color32(&wires[1], &c, &pal, nets[1]).unwrap();
        if wires[0].color.is_none() && wires[1].color.is_none() {
            assert_ne!(own, other);
        }
        let mut w = wires[0].clone();
        w.color = Some("#102030".into());
        assert_eq!(jumper_color32(&w, &c, &pal, by_net), Some(Color32::from_rgb(0x10, 0x20, 0x30)));
        w.color = None;
        c.theme = Some(short_circ_core::Theme { wire: Some("#405060".into()), ..Default::default() });
        let pal = Palette::for_circuit(&c, true);
        assert_eq!(jumper_color32(&w, &c, &pal, by_net), Some(Color32::from_rgb(0x40, 0x50, 0x60)), "theme beats the net colour");
        w.color = Some("none".into());
        assert_eq!(jumper_color32(&w, &c, &pal, by_net), None, "none draws nothing");
    }

    #[test]
    fn hit_tolerances_stay_below_the_hole_pitch() {
        assert_eq!(pin_hit_tolerance(1.0 / 2.0), 0.45);
        assert!(pin_hit_tolerance(1.0 / 100.0) >= 0.2 && pin_hit_tolerance(1.0 / 100.0) < 0.45);
        assert!(wire_hit_tolerance(1.0 / 200.0) > JUMPER_WIDTH / 2.0);
    }

    #[test]
    fn faded_only_when_dimmed() {
        let c = Color32::from_rgb(200, 100, 50);
        assert_eq!(faded(c, false), c);
        assert!(faded(c, true).r() < c.r());
    }
}
