//! Canvas: camera, hit testing, junction detection and drawing of the schematic view.
//! The breadboard view is drawn by `breadboard`; both share the helpers here.
//! The math is kept free of any egui context so it can be unit tested.

use crate::labels::{self, LabelOpts};
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use short_circ_core::color::{self, Palette, Rgba, Spec};
use short_circ_core::transform::{pin_world, to_world, world_bbox};
use short_circ_core::{
    Anchor, Circuit, Library, Part, PartDef, PartLookup, Placement, Pt, Shape as Sh, Style, View as ModelView, Wire,
};

/// Grid units per world unit are 1:1; this is pixels per grid unit.
pub const MIN_SCALE: f32 = 2.0;
pub const MAX_SCALE: f32 = 240.0;
/// Local size of the placeholder box drawn for unknown part types.
pub const UNKNOWN_SIZE: [f64; 2] = [6.0, 2.0];

// ---------------------------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------------------------

/// Maps world (grid unit) coordinates to canvas-relative pixels: `p = origin + w * scale`.
/// (Not to be confused with `short_circ_core::View`, the schematic / breadboard layout.)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub origin: Vec2,
    pub scale: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { origin: Vec2::new(40.0, 40.0), scale: 20.0 }
    }
}

impl Camera {
    /// World point to a position relative to the canvas top-left.
    pub fn to_local(self, w: [f64; 2]) -> Vec2 {
        Vec2::new(self.origin.x + w[0] as f32 * self.scale, self.origin.y + w[1] as f32 * self.scale)
    }

    /// Canvas-relative position back to world coordinates.
    pub fn to_world(self, local: Vec2) -> [f64; 2] {
        [((local.x - self.origin.x) / self.scale) as f64, ((local.y - self.origin.y) / self.scale) as f64]
    }

    pub fn to_screen(self, rect_min: Pos2, w: [f64; 2]) -> Pos2 {
        rect_min + self.to_local(w)
    }

    pub fn pan(&mut self, delta: Vec2) {
        self.origin += delta;
    }

    /// Multiply the scale by `factor` keeping the world point under `cursor` (canvas-relative) fixed.
    pub fn zoom_at(&mut self, cursor: Vec2, factor: f32) {
        let new_scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let k = new_scale / self.scale;
        self.origin = cursor - (cursor - self.origin) * k;
        self.scale = new_scale;
    }

    /// Fit the world box `[min, max]` into a canvas of `size` with a pixel margin.
    pub fn fit(&mut self, min: [f64; 2], max: [f64; 2], size: Vec2, margin: f32) {
        let bw = ((max[0] - min[0]) as f32).max(1.0);
        let bh = ((max[1] - min[1]) as f32).max(1.0);
        let sx = (size.x - 2.0 * margin).max(10.0) / bw;
        let sy = (size.y - 2.0 * margin).max(10.0) / bh;
        self.scale = sx.min(sy).clamp(MIN_SCALE, MAX_SCALE);
        let c = [(min[0] + max[0]) as f32 / 2.0, (min[1] + max[1]) as f32 / 2.0];
        self.origin = Vec2::new(size.x / 2.0 - c[0] * self.scale, size.y / 2.0 - c[1] * self.scale);
    }

    /// Pan so the world point `w` is at the center of a canvas of `size`.
    pub fn center_on(&mut self, w: [f64; 2], size: Vec2) {
        self.origin = Vec2::new(size.x / 2.0 - w[0] as f32 * self.scale, size.y / 2.0 - w[1] as f32 * self.scale);
    }
}

// ---------------------------------------------------------------------------------------------
// Pure geometry helpers
// ---------------------------------------------------------------------------------------------

/// Expand `{ref}`, `{value}` and `{prop:name}` in part text. Unknown or unterminated
/// braces are kept literally; a missing property expands to the empty string.
#[cfg(test)]
pub fn expand_text(text: &str, part: &Part) -> String {
    expand_text_with(text, part, true, true)
}

/// Like [`expand_text`] but `{ref}` and `{value}` expand to nothing when their flag is off
/// (`{ref}` is also empty for a part with `hidden_label`).
pub fn expand_text_with(text: &str, part: &Part, show_ref: bool, show_value: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let key = &after[..j];
        let val = match key {
            "ref" if !show_ref || part.hidden_label => Some(String::new()),
            "value" if !show_value => Some(String::new()),
            "ref" => Some(part.id.clone()),
            "value" => Some(part.props.get("value").cloned().unwrap_or_default()),
            _ => key.strip_prefix("prop:").map(|n| part.props.get(n).cloned().unwrap_or_default()),
        };
        match val {
            Some(v) => out.push_str(&v),
            None => {
                out.push('{');
                out.push_str(key);
                out.push('}');
            }
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// Closed outline of a (rounded) rect in local coordinates, clockwise starting at the top left.
pub fn rect_outline(at: [f64; 2], size: [f64; 2], r: f64) -> Vec<[f64; 2]> {
    let (x, y, w, h) = (at[0], at[1], size[0], size[1]);
    let r = r.min(w.abs() / 2.0).min(h.abs() / 2.0);
    if r <= 0.0 {
        return vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    }
    let mut pts = Vec::new();
    let corners = [([x + r, y + r], 180.0), ([x + w - r, y + r], 270.0), ([x + w - r, y + h - r], 0.0), ([x + r, y + h - r], 90.0)];
    for (c, start) in corners {
        for k in 0..=6 {
            let a = (start + 15.0 * k as f64).to_radians();
            pts.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
        }
    }
    pts
}

/// Points of an arc, degrees clockwise from +x (y down), `to` wrapped to be above `from`.
pub fn arc_points(at: [f64; 2], r: f64, from: f64, to: f64, step_deg: f64) -> Vec<[f64; 2]> {
    let mut to = to;
    while to <= from {
        to += 360.0;
    }
    let sweep = (to - from).min(360.0);
    let n = ((sweep / step_deg.max(1.0)).ceil() as usize).max(2);
    (0..=n)
        .map(|k| {
            let a = (from + sweep * k as f64 / n as f64).to_radians();
            [at[0] + r * a.cos(), at[1] + r * a.sin()]
        })
        .collect()
}

fn on_segment_interior(p: Pt, a: Pt, b: Pt) -> bool {
    if p == a || p == b {
        return false;
    }
    let cross = (b[0] - a[0]) as i64 * (p[1] - a[1]) as i64 - (b[1] - a[1]) as i64 * (p[0] - a[0]) as i64;
    cross == 0
        && p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// Points where three or more wire segment ends meet: a vertex joins two segments, an endpoint one,
/// and a point inside another segment (T-junction) counts that segment twice. Schematic wires
/// only (breadboard jumpers never join sideways).
pub fn junctions(wires: &[Wire]) -> Vec<Pt> {
    let segs: Vec<(Pt, Pt)> = wires
        .iter()
        .filter(|w| w.view == ModelView::Schematic)
        .flat_map(|w| w.pts.windows(2).map(|s| (s[0], s[1])).collect::<Vec<_>>())
        .filter(|(a, b)| a != b)
        .collect();
    let mut cands: Vec<Pt> = segs.iter().flat_map(|(a, b)| [*a, *b]).collect();
    cands.sort_unstable();
    cands.dedup();
    cands
        .into_iter()
        .filter(|p| {
            let degree: usize = segs
                .iter()
                .map(|(a, b)| {
                    if a == p || b == p {
                        1
                    } else if on_segment_interior(*p, *a, *b) {
                        2
                    } else {
                        0
                    }
                })
                .sum();
            degree >= 3
        })
        .collect()
}

fn corners(pl: &Placement, size: [f64; 2]) -> [[f64; 2]; 4] {
    [[0.0, 0.0], [size[0], 0.0], [size[0], size[1]], [0.0, size[1]]].map(|c| to_world(pl, c))
}

/// World bounding box of a part placed with `pl` (placeholder box for unknown types and for
/// types without geometry in the view).
pub fn part_box(pl: &Placement, def: Option<&PartDef>, view: ModelView) -> ([f64; 2], [f64; 2]) {
    match def.and_then(|d| d.view(view)) {
        Some(vd) => world_bbox(pl, vd),
        None => {
            let cs = corners(pl, UNKNOWN_SIZE);
            let mut min = cs[0];
            let mut max = cs[0];
            for c in cs {
                min = [min[0].min(c[0]), min[1].min(c[1])];
                max = [max[0].max(c[0]), max[1].max(c[1])];
            }
            (min, max)
        }
    }
}

/// Bounding box of everything in `view` of the circuit, or None if it is empty.
pub fn circuit_bounds(circuit: &Circuit, lib: &Library, view: ModelView) -> Option<([f64; 2], [f64; 2])> {
    let mut acc: Option<([f64; 2], [f64; 2])> = None;
    let mut add = |lo: [f64; 2], hi: [f64; 2]| {
        acc = Some(match acc {
            None => (lo, hi),
            Some((a, b)) => ([a[0].min(lo[0]), a[1].min(lo[1])], [b[0].max(hi[0]), b[1].max(hi[1])]),
        });
    };
    for p in &circuit.parts {
        if let Some(pl) = p.placement(view) {
            let (lo, hi) = part_box(pl, lib.get(&p.ty), view);
            add(lo, hi);
        }
    }
    for w in circuit.wires_in(view) {
        for pt in &w.pts {
            add([pt[0] as f64, pt[1] as f64], [pt[0] as f64, pt[1] as f64]);
        }
    }
    if view == ModelView::Schematic {
        for l in &circuit.labels {
            add([l.at[0] as f64, l.at[1] as f64], [l.at[0] as f64, l.at[1] as f64]);
        }
    }
    acc
}

// ---------------------------------------------------------------------------------------------
// Hit testing (world coordinates, tolerances in grid units)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinHit {
    pub part: usize,
    pub pin: String,
    pub at: Pt,
    /// The pin is a hole of a board part.
    pub board: bool,
}

/// The pin of a part placed in `view` closest to `w` within `tol`. At equal distance a pin of a
/// real part beats a board hole (a leg sitting in a hole).
pub fn pin_at(circuit: &Circuit, lib: &dyn PartLookup, view: ModelView, w: [f64; 2], tol: f64) -> Option<PinHit> {
    let mut best: Option<((f64, bool), PinHit)> = None;
    for (i, part) in circuit.parts.iter().enumerate() {
        let (Some(pl), Some(def)) = (part.placement(view), lib.get(&part.ty)) else { continue };
        let board = def.is_board();
        for (pin, pos) in def.placed_pins(view) {
            let at = pin_world(pl, pos);
            let (dx, dy) = (at[0] as f64 - w[0], at[1] as f64 - w[1]);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= tol && best.as_ref().is_none_or(|(k, _)| (d, board) < *k) {
                best = Some(((d, board), PinHit { part: i, pin: pin.id.clone(), at, board }));
            }
        }
    }
    best.map(|(_, h)| h)
}

fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) };
    ((p[0] - (a[0] + t * dx)).powi(2) + (p[1] - (a[1] + t * dy)).powi(2)).sqrt()
}

/// Index (into `circuit.wires`) of the wire of `view` closest to `w` within `tol`.
pub fn wire_at(circuit: &Circuit, view: ModelView, w: [f64; 2], tol: f64) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, wire) in circuit.wires.iter().enumerate() {
        if wire.view != view {
            continue;
        }
        for s in wire.pts.windows(2) {
            let d = dist_to_segment(w, [s[0][0] as f64, s[0][1] as f64], [s[1][0] as f64, s[1][1] as f64]);
            if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
            }
        }
    }
    best.map(|(_, i)| i)
}

/// Index of the smallest part placed in `view` whose box (grown by `tol`) contains `w`.
pub fn part_at(circuit: &Circuit, lib: &dyn PartLookup, view: ModelView, w: [f64; 2], tol: f64) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, part) in circuit.parts.iter().enumerate() {
        let Some(pl) = part.placement(view) else { continue };
        let (lo, hi) = part_box(pl, lib.get(&part.ty), view);
        if w[0] >= lo[0] - tol && w[0] <= hi[0] + tol && w[1] >= lo[1] - tol && w[1] <= hi[1] + tol {
            let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
            if best.is_none_or(|(ba, _)| area < ba) {
                best = Some((area, i));
            }
        }
    }
    best.map(|(_, i)| i)
}

/// A representative world point in `view` for a list of ids: a part id (its origin there), a
/// `PART.PIN` reference (the part's origin), or a wire id (its first point). Ids of things that
/// are not in `view` are skipped.
pub fn locate_ids(circuit: &Circuit, ids: &[String], view: ModelView) -> Option<Pt> {
    for id in ids {
        let part_id = id.split_once('.').map_or(id.as_str(), |(p, _)| p);
        let part = circuit.parts.iter().find(|p| p.id == *id).or_else(|| circuit.part(part_id));
        if let Some(pl) = part.and_then(|p| p.placement(view)) {
            return Some(pl.at);
        }
        if let Some(w) = circuit.wires.iter().find(|w| &w.id == id && w.view == view) {
            if let Some(pt) = w.pts.first() {
                return Some(*pt);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Colors and theme
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub bg: Color32,
    pub fg: Color32,
    /// Built-in wire and pin colours; drawing uses `short_circ_core::color::Palette` instead.
    #[allow(dead_code)]
    pub wire: Color32,
    pub grid_minor: Color32,
    pub grid_major: Color32,
    #[allow(dead_code)]
    pub pin: Color32,
    pub highlight: Color32,
    pub select: Color32,
    pub error: Color32,
    pub warning: Color32,
}

impl Theme {
    pub fn new(dark: bool) -> Self {
        if dark {
            Theme {
                dark,
                bg: Color32::from_rgb(24, 26, 30),
                fg: Color32::from_rgb(222, 224, 228),
                wire: Color32::from_rgb(86, 200, 120),
                grid_minor: Color32::from_rgb(54, 58, 66),
                grid_major: Color32::from_rgb(92, 98, 110),
                pin: Color32::from_rgb(240, 170, 60),
                highlight: Color32::from_rgb(255, 214, 10),
                select: Color32::from_rgb(90, 165, 255),
                error: Color32::from_rgb(240, 80, 80),
                warning: Color32::from_rgb(235, 175, 40),
            }
        } else {
            Theme {
                dark,
                bg: Color32::from_rgb(250, 250, 247),
                fg: Color32::from_rgb(30, 32, 36),
                wire: Color32::from_rgb(20, 130, 60),
                grid_minor: Color32::from_rgb(214, 216, 220),
                grid_major: Color32::from_rgb(168, 172, 182),
                pin: Color32::from_rgb(200, 110, 0),
                highlight: Color32::from_rgb(225, 140, 0),
                select: Color32::from_rgb(0, 100, 220),
                error: Color32::from_rgb(200, 30, 30),
                warning: Color32::from_rgb(190, 120, 0),
            }
        }
    }

    /// Like `new`, with the circuit's `theme.background` and `theme.grid` applied. A background of
    /// "none" keeps the viewer background; a grid of "none" hides the grid.
    pub fn for_circuit(dark: bool, circuit: Option<&Circuit>) -> Self {
        let mut t = Theme::new(dark);
        let Some(c) = circuit.filter(|c| c.theme.is_some()) else { return t };
        let pal = Palette::for_circuit(c, dark);
        if let Some(bg) = pal.background {
            t.bg = to_color32(bg);
        }
        match pal.grid {
            Some(g) => {
                let bg = pal.background.unwrap_or(Palette::builtin(dark).background.unwrap_or(g));
                t.grid_major = to_color32(g);
                t.grid_minor = to_color32(bg.mix(g, 0.45));
            }
            None => {
                t.grid_major = Color32::TRANSPARENT;
                t.grid_minor = Color32::TRANSPARENT;
            }
        }
        t
    }
}

/// Core colour to egui colour. Colours resolve in `short_circ_core::color`, shared with SVG export.
pub fn to_color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Stroke of `width` in `c`, or no stroke when the colour resolved to "none".
fn stroke_of(width: f32, c: Option<Rgba>) -> Stroke {
    c.map_or(Stroke::NONE, |c| Stroke::new(width, to_color32(c)))
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

/// What to emphasize this frame (hovered or locked net).
#[derive(Debug, Clone, Default)]
pub struct Highlight {
    pub wires: Vec<String>,
    /// (part id, pin id)
    pub pins: Vec<(String, String)>,
}

impl Highlight {
    pub fn is_empty(&self) -> bool {
        self.wires.is_empty() && self.pins.is_empty()
    }
}

pub struct DrawCtx<'a> {
    pub painter: &'a Painter,
    pub origin: Pos2,
    pub cam: Camera,
    pub theme: Theme,
    pub labels: LabelOpts,
}

impl DrawCtx<'_> {
    pub(crate) fn pt(&self, w: [f64; 2]) -> Pos2 {
        self.cam.to_screen(self.origin, w)
    }

    pub(crate) fn px(&self, units: f64) -> f32 {
        units as f32 * self.cam.scale
    }
}

/// Draw background grid dots: one per unit, stronger every ten.
pub fn draw_grid(d: &DrawCtx, size: Vec2) {
    let s = d.cam.scale;
    let lo = d.cam.to_world(Vec2::ZERO);
    let hi = d.cam.to_world(size);
    let (x0, x1) = (lo[0].floor() as i64, hi[0].ceil() as i64);
    let (y0, y1) = (lo[1].floor() as i64, hi[1].ceil() as i64);
    let minor = s >= 8.0;
    let major = s * 10.0 >= 12.0;
    if !minor && !major {
        return;
    }
    for gy in y0..=y1 {
        for gx in x0..=x1 {
            let is_major = gx.rem_euclid(10) == 0 && gy.rem_euclid(10) == 0;
            if !(is_major && major || minor) {
                continue;
            }
            let p = d.pt([gx as f64, gy as f64]);
            if is_major {
                d.painter.circle_filled(p, 1.6, d.theme.grid_major);
            } else {
                d.painter.rect_filled(Rect::from_center_size(p, Vec2::splat(1.5)), 0.0, d.theme.grid_minor);
            }
        }
    }
}

/// Draw the schematic view of a circuit.
pub fn draw_schematic(
    d: &DrawCtx,
    circuit: &Circuit,
    lib: &Library,
    junctions: &[Pt],
    hl: &Highlight,
    selected: Option<usize>,
) {
    let view = ModelView::Schematic;
    let t = &d.theme;
    let pal = Palette::for_circuit(circuit, t.dark);
    // Wires
    for w in circuit.wires_in(view) {
        let lit = hl.wires.iter().any(|x| x == &w.id);
        let color = if lit { Some(t.highlight) } else { color::wire_color(w, &pal).map(to_color32) };
        let Some(color) = color else { continue };
        let width = (d.px(0.16)).max(1.5) * if lit { 1.7 } else { 1.0 };
        let pts: Vec<Pos2> = w.pts.iter().map(|p| d.pt([p[0] as f64, p[1] as f64])).collect();
        if pts.len() >= 2 {
            d.painter.add(Shape::line(pts, Stroke::new(width, color)));
        }
    }
    // Parts
    for (i, part) in circuit.parts.iter().enumerate() {
        let Some(pl) = part.placement(view) else { continue };
        match lib.get(&part.ty).and_then(|def| def.view(view)) {
            Some(vd) => draw_shapes(d, &pal, part, pl, &vd.shapes),
            None => draw_unknown(d, part, pl),
        }
        if selected == Some(i) {
            let (lo, hi) = part_box(pl, lib.get(&part.ty), view);
            draw_selection(d, lo, hi);
        }
    }
    // Junction dots
    let jr = d.px(0.28).max(2.5);
    // Without per-wire colours every junction takes the palette colour (skips a scan per dot).
    let uniform = circuit.wires_in(view).all(|w| w.color.is_none());
    for j in junctions {
        let c = match (uniform, pal.junction) {
            (_, Spec::Color(c)) => Some(c),
            (_, Spec::None) => None,
            (true, Spec::Unset) => pal.wire,
            (false, Spec::Unset) => color::junction_color(circuit, *j, &pal),
        };
        if let Some(c) = c {
            d.painter.circle_filled(d.pt([j[0] as f64, j[1] as f64]), jr, to_color32(c));
        }
    }
    // Pin markers
    for part in &circuit.parts {
        let (Some(pl), Some(def)) = (part.placement(view), lib.get(&part.ty)) else { continue };
        for (pin, pos) in def.placed_pins(view) {
            let at = pin_world(pl, pos);
            let c = d.pt([at[0] as f64, at[1] as f64]);
            let lit = hl.pins.iter().any(|(p, n)| p == &part.id && n == &pin.id);
            if lit {
                d.painter.circle_filled(c, (d.px(0.3)).max(4.0), t.highlight);
            } else if let Some(pc) = pal.pin {
                let h = d.px(0.12).max(2.0);
                d.painter.rect_stroke(
                    Rect::from_center_size(c, Vec2::splat(h * 2.0)),
                    0.0,
                    Stroke::new(1.0, to_color32(pc)),
                    egui::StrokeKind::Middle,
                );
            }
        }
    }
    // Labels
    for l in &circuit.labels {
        let Some(c) = color::label_color(l, &pal) else { continue };
        let p = d.pt([l.at[0] as f64, l.at[1] as f64]);
        draw_text(d, p + Vec2::new(d.px(0.2), -d.px(0.2)), &l.name, 0.9, Anchor::Start, to_color32(c));
    }
    labels::draw_overlay(d, circuit, lib, view);
}

/// Outline around the selected part's world box.
pub fn draw_selection(d: &DrawCtx, lo: [f64; 2], hi: [f64; 2]) {
    let r = Rect::from_min_max(d.pt([lo[0] - 0.4, lo[1] - 0.4]), d.pt([hi[0] + 0.4, hi[1] + 0.4]));
    d.painter.rect_stroke(r, 3.0, Stroke::new(1.5, d.theme.select), egui::StrokeKind::Outside);
}

pub fn draw_text(d: &DrawCtx, at: Pos2, text: &str, size_units: f64, anchor: Anchor, color: Color32) {
    let Some(px) = labels::text_px(size_units, d.cam.scale) else { return };
    if text.is_empty() {
        return;
    }
    let align = match anchor {
        Anchor::Start => Align2::LEFT_BOTTOM,
        Anchor::Middle => Align2::CENTER_BOTTOM,
        Anchor::End => Align2::RIGHT_BOTTOM,
    };
    labels::paint_legible(d.painter, at + Vec2::new(0.0, px * 0.15), align, text, px, color, d.theme.bg);
}

fn draw_poly(d: &DrawCtx, pal: &Palette, part: &Part, pl: &Placement, pts: &[[f64; 2]], closed: bool, style: &Style) {
    if pts.len() < 2 {
        return;
    }
    let sp: Vec<Pos2> = pts.iter().map(|p| d.pt(to_world(pl, *p))).collect();
    let width = d.px(style.width.unwrap_or(0.12)).max(1.0);
    let stroke = stroke_of(width, color::stroke_color(style, part, pal));
    let fill = if closed { color::fill_color(style, part).map(to_color32) } else { None };
    if let Some(fill) = fill {
        d.painter.add(Shape::convex_polygon(sp, fill, stroke));
    } else if closed {
        d.painter.add(Shape::closed_line(sp, stroke));
    } else {
        d.painter.add(Shape::line(sp, stroke));
    }
}

/// Draw the vector `shapes` of one view of a part placed with `pl` (schematic symbols, and the
/// breadboard fallback for parts without artwork).
pub fn draw_shapes(d: &DrawCtx, pal: &Palette, part: &Part, pl: &Placement, shapes: &[Sh]) {
    for shape in shapes {
        match shape {
            Sh::Line { pts, style } => draw_poly(d, pal, part, pl, pts, false, style),
            Sh::Poly { pts, style } => draw_poly(d, pal, part, pl, pts, true, style),
            Sh::Rect { at, size, r, style } => draw_poly(d, pal, part, pl, &rect_outline(*at, *size, *r), true, style),
            Sh::Circle { at, r, style } => {
                let c = d.pt(to_world(pl, *at));
                let width = d.px(style.width.unwrap_or(0.12)).max(1.0);
                if let Some(fill) = color::fill_color(style, part) {
                    d.painter.add(Shape::circle_filled(c, d.px(*r), to_color32(fill)));
                }
                let stroke = stroke_of(width, color::stroke_color(style, part, pal));
                d.painter.add(Shape::circle_stroke(c, d.px(*r), stroke));
            }
            Sh::Arc { at, r, from, to, style } => {
                let step = (60.0 / (d.px(*r).max(1.0) as f64).sqrt()).clamp(3.0, 15.0);
                draw_poly(d, pal, part, pl, &arc_points(*at, *r, *from, *to, step), false, style);
            }
            Sh::Text { at, text, size, anchor, style } => {
                let Some(c) = color::text_color(style, part, pal) else { continue };
                let p = d.pt(to_world(pl, *at));
                let shown = expand_text_with(text, part, d.labels.refs, d.labels.values);
                draw_text(d, p, &shown, *size, *anchor, to_color32(c));
            }
        }
    }
}

/// Dashed red placeholder box with the type name: unknown type or no geometry in this view.
pub fn draw_unknown(d: &DrawCtx, part: &Part, pl: &Placement) {
    let cs = corners(pl, UNKNOWN_SIZE);
    let mut pts: Vec<Pos2> = cs.iter().map(|c| d.pt(*c)).collect();
    pts.push(pts[0]);
    let stroke = Stroke::new(1.5, d.theme.error);
    d.painter.extend(Shape::dashed_line(&pts, stroke, 6.0, 4.0));
    let center = d.pt(to_world(pl, [UNKNOWN_SIZE[0] / 2.0, UNKNOWN_SIZE[1] / 2.0]));
    let px = d.px(0.9).clamp(9.0, 40.0);
    d.painter.text(center, Align2::CENTER_CENTER, &part.ty, FontId::proportional(px), d.theme.error);
}

/// Expanding ring used to mark an issue location. `age` is seconds since the click.
pub fn draw_flash(d: &DrawCtx, at: Pt, age: f32) {
    let c = d.pt([at[0] as f64, at[1] as f64]);
    let fade = (1.0 - age / FLASH_SECS).clamp(0.0, 1.0);
    let pulse = (age * 6.0).sin().abs();
    let color = d.theme.error.gamma_multiply(0.35 + 0.65 * fade);
    d.painter.circle_stroke(c, 10.0 + 14.0 * pulse, Stroke::new(2.5, color));
    d.painter.circle_stroke(c, 4.0, Stroke::new(2.0, color));
}

pub const FLASH_SECS: f32 = 2.5;

/// Small tooltip box near the pointer, kept inside `bounds`.
pub fn draw_tooltip(painter: &Painter, bounds: Rect, near: Pos2, text: &str, theme: &Theme) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(13.0), theme.fg);
    let size = galley.size() + Vec2::splat(10.0);
    let mut min = near + Vec2::new(14.0, 16.0);
    if min.x + size.x > bounds.max.x {
        min.x = near.x - size.x - 8.0;
    }
    if min.y + size.y > bounds.max.y {
        min.y = near.y - size.y - 8.0;
    }
    let r = Rect::from_min_size(min, size);
    painter.rect_filled(r, 3.0, theme.bg);
    painter.rect_stroke(r, 3.0, Stroke::new(1.0, theme.grid_major), egui::StrokeKind::Inside);
    painter.galley(min + Vec2::splat(5.0), galley, theme.fg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn wire(id: &str, pts: &[Pt]) -> Wire {
        Wire { id: id.into(), pts: pts.to_vec(), ..Wire::default() }
    }

    fn part_with(props: &[(&str, &str)]) -> Part {
        Part {
            id: "R1".into(),
            ty: "resistor".into(),
            schematic: Some(Placement::at(0, 0)),
            props: props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(),
            ..Part::default()
        }
    }

    #[test]
    fn camera_roundtrip() {
        let v = Camera { origin: Vec2::new(13.0, -7.0), scale: 25.0 };
        let w = v.to_world(v.to_local([3.0, 4.5]));
        assert!((w[0] - 3.0).abs() < 1e-4 && (w[1] - 4.5).abs() < 1e-4);
    }

    #[test]
    fn zoom_keeps_cursor_fixed() {
        let mut v = Camera::default();
        let cursor = Vec2::new(300.0, 200.0);
        let before = v.to_world(cursor);
        v.zoom_at(cursor, 1.7);
        let after = v.to_world(cursor);
        assert!((before[0] - after[0]).abs() < 1e-3 && (before[1] - after[1]).abs() < 1e-3);
        assert!(v.scale > 20.0);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut v = Camera::default();
        v.zoom_at(Vec2::ZERO, 1e6);
        assert_eq!(v.scale, MAX_SCALE);
        v.zoom_at(Vec2::ZERO, 1e-6);
        assert_eq!(v.scale, MIN_SCALE);
    }

    #[test]
    fn fit_contains_bounds() {
        let mut v = Camera::default();
        let size = Vec2::new(800.0, 600.0);
        v.fit([10.0, 5.0], [30.0, 25.0], size, 20.0);
        for corner in [[10.0, 5.0], [30.0, 25.0]] {
            let p = v.to_local(corner);
            assert!(p.x >= 19.9 && p.x <= 780.1 && p.y >= 19.9 && p.y <= 580.1, "{p:?}");
        }
        let c = v.to_local([20.0, 15.0]);
        assert!((c.x - 400.0).abs() < 0.1 && (c.y - 300.0).abs() < 0.1);
    }

    #[test]
    fn center_on_point() {
        let mut v = Camera::default();
        v.center_on([7.0, 9.0], Vec2::new(400.0, 300.0));
        let p = v.to_local([7.0, 9.0]);
        assert!((p.x - 200.0).abs() < 0.01 && (p.y - 150.0).abs() < 0.01);
    }

    #[test]
    fn text_expansion() {
        let p = part_with(&[("value", "330"), ("tol", "5%")]);
        assert_eq!(expand_text("{ref}", &p), "R1");
        assert_eq!(expand_text("{ref} = {value} ohm", &p), "R1 = 330 ohm");
        assert_eq!(expand_text("{prop:tol}/{prop:none}", &p), "5%/");
        assert_eq!(expand_text("{bogus} {value", &p), "{bogus} {value");
        assert_eq!(expand_text("plain", &p), "plain");
        assert_eq!(expand_text("{value}", &part_with(&[])), "");
    }

    #[test]
    fn junction_t_and_corner_and_cross() {
        // T: second wire ends in the middle of the first.
        let t = junctions(&[wire("a", &[[0, 0], [10, 0]]), wire("b", &[[5, 0], [5, 5]])]);
        assert_eq!(t, vec![[5, 0]]);
        // Plain corner of one wire: no dot.
        assert!(junctions(&[wire("a", &[[0, 0], [5, 0], [5, 5]])]).is_empty());
        // Two wires meeting end to end: no dot.
        assert!(junctions(&[wire("a", &[[0, 0], [5, 0]]), wire("b", &[[5, 0], [9, 0]])]).is_empty());
        // Pure crossing: no dot.
        assert!(junctions(&[wire("a", &[[0, 5], [10, 5]]), wire("b", &[[5, 0], [5, 10]])]).is_empty());
    }

    #[test]
    fn junction_three_ends_and_vertex() {
        let three = junctions(&[
            wire("a", &[[0, 0], [5, 0]]),
            wire("b", &[[5, 0], [5, 5]]),
            wire("c", &[[5, 0], [9, 0]]),
        ]);
        assert_eq!(three, vec![[5, 0]]);
        // A polyline vertex (2) plus another wire's endpoint (1).
        let v = junctions(&[wire("a", &[[0, 0], [5, 0], [5, 5]]), wire("b", &[[5, 0], [9, 0]])]);
        assert_eq!(v, vec![[5, 0]]);
    }

    #[test]
    fn breadboard_jumpers_make_no_junctions() {
        let mut a = wire("a", &[[0, 0], [5, 0]]);
        let mut b = wire("b", &[[5, 0], [5, 5]]);
        let mut c = wire("c", &[[5, 0], [9, 0]]);
        assert_eq!(junctions(&[a.clone(), b.clone(), c.clone()]), vec![[5, 0]]);
        for w in [&mut a, &mut b, &mut c] {
            w.view = ModelView::Breadboard;
        }
        assert!(junctions(&[a, b, c]).is_empty());
    }

    #[test]
    fn rounded_rect_outline() {
        assert_eq!(rect_outline([0.0, 0.0], [4.0, 2.0], 0.0).len(), 4);
        let r = rect_outline([0.0, 0.0], [4.0, 2.0], 0.5);
        assert_eq!(r.len(), 28);
        for p in r {
            assert!(p[0] >= -1e-9 && p[0] <= 4.0 + 1e-9 && p[1] >= -1e-9 && p[1] <= 2.0 + 1e-9);
        }
        // Oversized radius is clamped to half the short side.
        let c = rect_outline([0.0, 0.0], [4.0, 2.0], 10.0);
        assert!(c.iter().all(|p| p[1] >= -1e-9 && p[1] <= 2.0 + 1e-9));
    }

    #[test]
    fn arc_endpoints() {
        let a = arc_points([1.0, 1.0], 2.0, 0.0, 90.0, 10.0);
        assert!((a[0][0] - 3.0).abs() < 1e-9 && (a[0][1] - 1.0).abs() < 1e-9);
        let l = a.last().unwrap();
        assert!((l[0] - 1.0).abs() < 1e-9 && (l[1] - 3.0).abs() < 1e-9);
        // to <= from wraps around.
        let w = arc_points([0.0, 0.0], 1.0, 270.0, 90.0, 30.0);
        assert!(w.len() > 3);
    }

    #[test]
    fn colors() {
        use short_circ_core::color::parse_color;
        assert_eq!(parse_color("#ff0000").map(to_color32), Some(Color32::from_rgb(255, 0, 0)));
        assert_eq!(parse_color("#0f08").map(|c| c.a), None, "4 digit hex is not a form");
        assert_eq!(parse_color("#00ff0080").map(to_color32), Some(Color32::from_rgba_unmultiplied(0, 255, 0, 128)));
        assert_eq!(parse_color("Black").map(to_color32), Some(Color32::BLACK));
        assert_eq!(parse_color("nonsense"), None);
    }

    #[test]
    fn circuit_theme_overrides_background_and_grid() {
        let plain = Theme::for_circuit(true, None);
        assert_eq!(plain.bg, Theme::new(true).bg);
        let mut c = Circuit {
            theme: Some(short_circ_core::Theme {
                background: Some("#102030".into()),
                grid: Some("none".into()),
                ..Default::default()
            }),
            ..Circuit::default()
        };
        let t = Theme::for_circuit(false, Some(&c));
        assert_eq!(t.bg, Color32::from_rgb(0x10, 0x20, 0x30));
        assert_eq!(t.grid_major, Color32::TRANSPARENT);
        c.theme = Some(short_circ_core::Theme { background: Some("none".into()), ..Default::default() });
        assert_eq!(Theme::for_circuit(false, Some(&c)).bg, Theme::new(false).bg, "none keeps the viewer background");
    }

    #[test]
    fn hit_testing_on_example() {
        let c = short_circ_core::parse_circuit(include_str!("../../core/tests/fixtures/v1/led.circ")).unwrap();
        let lib = Library::load(&c, None);
        let sch = ModelView::Schematic;
        // R1 at (14,6): pin a is local (0,1) -> world (14,7).
        let h = pin_at(&c, &lib, sch, [14.1, 7.1], 0.5).unwrap();
        assert_eq!((c.parts[h.part].id.as_str(), h.pin.as_str(), h.board), ("R1", "a", false));
        assert!(pin_at(&c, &lib, sch, [18.0, 3.0], 0.5).is_none());
        assert_eq!(part_at(&c, &lib, sch, [18.0, 7.0], 0.2), Some(1));
        assert_eq!(wire_at(&c, sch, [11.0, 8.5], 0.3), Some(0));
        assert!(wire_at(&c, sch, [40.0, 40.0], 0.3).is_none());
        let (lo, hi) = circuit_bounds(&c, &lib, sch).unwrap();
        assert!(lo[0] <= 10.0 && hi[0] >= 26.0);
        assert_eq!(locate_ids(&c, &["w2".into()], sch), Some([22, 7]));
        assert_eq!(locate_ids(&c, &["R1".into()], sch), Some([14, 6]));
        assert_eq!(locate_ids(&c, &["R1.a".into()], sch), Some([14, 6]), "PART.PIN refs find the part");
        // Nothing of this v1 file is in the breadboard view.
        let bb = ModelView::Breadboard;
        assert!(pin_at(&c, &lib, bb, [14.1, 7.1], 0.5).is_none());
        assert!(wire_at(&c, bb, [11.0, 8.5], 0.3).is_none());
        assert!(part_at(&c, &lib, bb, [18.0, 7.0], 0.2).is_none());
        assert!(circuit_bounds(&c, &lib, bb).is_none());
        assert!(locate_ids(&c, &["R1".into()], bb).is_none());
    }

    #[test]
    fn breadboard_hits_prefer_legs_over_holes_and_small_over_boards() {
        let c = short_circ_core::parse_circuit(include_str!("../../../examples/breadboard_demo.circ")).unwrap();
        let lib = Library::load(&c, None);
        let bb = ModelView::Breadboard;
        // R1 leg `a` sits in hole i10 of the board at (11,16).
        let h = pin_at(&c, &lib, bb, [11.05, 16.0], 0.45).unwrap();
        assert_eq!((c.parts[h.part].id.as_str(), h.pin.as_str(), h.board), ("R1", "a", false));
        // An empty hole is a board pin.
        let hole = pin_at(&c, &lib, bb, [5.0, 6.0], 0.45).unwrap();
        assert!(hole.board && c.parts[hole.part].id == "BB1", "{hole:?}");
        // On the resistor body the part wins over the big board.
        let p = part_at(&c, &lib, bb, [13.0, 16.0], 0.3).unwrap();
        assert_eq!(c.parts[p].id, "R1");
        // Jumpers hit-test within the breadboard view only.
        assert_eq!(c.wires[wire_at(&c, bb, [5.5, 23.3], 0.3).unwrap()].id, "j1");
        assert!(wire_at(&c, ModelView::Schematic, [5.5, 23.3], 0.3).is_none());
    }

    #[test]
    fn unknown_part_has_box() {
        let pl = Placement { at: [10, 10], rot: 90, mirror: false };
        let (lo, hi) = part_box(&pl, None, ModelView::Schematic);
        assert_eq!(lo, [8.0, 10.0]);
        assert_eq!(hi, [10.0, 16.0]);
        // A type without geometry in the view gets the placeholder too.
        let lib = Library::builtin();
        let (lo2, hi2) = part_box(&pl, lib.get("breadboard_half"), ModelView::Schematic);
        assert_eq!((lo2, hi2), (lo, hi));
    }
}
