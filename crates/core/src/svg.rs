//! Static SVG export of a whole circuit.
//!
//! Output coordinates are grid units (the SVG user space); the root element's `width`/`height`
//! are 10 px per unit, and the `viewBox` is the content bounding box plus a 2 unit margin
//! snapped to whole units. Output is deterministic (file order, fixed number formatting).
//!
//! Colours come from `color` (the same resolver the viewer uses): explicit colours are written as
//! normalised `#rrggbb` hex (plus `*-opacity` when not opaque), `"none"` omits the element.
//!
//! Drawing order: background, title, parts (each in a `<g id="REF">`), wires, junction dots,
//! pin dots, net labels. Text stays upright whatever the part rotation; `start`/`end` anchors
//! of a rotated or mirrored part are swapped when the text ends up on the other side of the
//! part centre, so labels keep pointing away from the symbol.

use crate::color::{self, Palette, Rgba};
use crate::model::{Anchor, Circuit, Part, PartDef, Shape, Style};
use crate::resolve::Library;
use crate::svgimport;
use crate::transform::{pin_world, to_world, world_bbox};
use std::collections::BTreeMap;
use std::fmt::Write;

const PX_PER_UNIT: f64 = 10.0;
const MARGIN: f64 = 2.0;
const DEFAULT_WIDTH: f64 = 0.12;
const WIRE_WIDTH: f64 = 0.15;

/// Colour of the dashed placeholder for unknown part types.
fn error_color(dark: bool) -> &'static str {
    if dark {
        "#ff5252"
    } else {
        "#d32f2f"
    }
}

/// `name="#rrggbb"` (plus `name-opacity` if translucent), or `name="none"`.
fn attr(name: &str, c: Option<Rgba>) -> String {
    match c {
        None => format!("{name}=\"none\""),
        Some(c) if c.a == 255 => format!("{name}=\"#{:02x}{:02x}{:02x}\"", c.r, c.g, c.b),
        Some(c) => format!(
            "{name}=\"#{:02x}{:02x}{:02x}\" {name}-opacity=\"{}\"",
            c.r,
            c.g,
            c.b,
            num(c.a as f64 / 255.0)
        ),
    }
}

/// Render the whole circuit as a standalone SVG document.
pub fn render_svg(circuit: &Circuit, lib: &Library, dark: bool) -> String {
    let pal = Palette::for_circuit(circuit, dark);
    let base = Palette::builtin(dark);
    let mut bb = Bounds::new();
    let mut body = String::new();

    // Parts.
    let mut pin_points: Vec<[i32; 2]> = Vec::new();
    for part in &circuit.parts {
        match lib.get(&part.ty) {
            Some(def) => {
                render_part(&mut body, part, def, &pal, &mut bb);
                for p in &def.pins {
                    pin_points.push(pin_world(part, p));
                }
            }
            None => render_unknown(&mut body, part, dark, &mut bb),
        }
    }

    // Wires.
    for w in &circuit.wires {
        if w.pts.len() < 2 {
            continue;
        }
        for p in &w.pts {
            bb.add([p[0] as f64, p[1] as f64]);
        }
        let Some(color) = color::wire_color(w, &pal) else { continue };
        let pts: Vec<String> = w.pts.iter().map(|p| format!("{},{}", p[0], p[1])).collect();
        let _ = writeln!(
            body,
            "<polyline id=\"{}\" class=\"wire\" points=\"{}\" fill=\"none\" {} stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
            esc(&w.id),
            pts.join(" "),
            attr("stroke", Some(color)),
            num(WIRE_WIDTH)
        );
    }

    // Junction dots.
    for j in junctions(circuit, &pin_points) {
        let Some(c) = color::junction_color(circuit, j, &pal) else { continue };
        let _ = writeln!(
            body,
            "<circle class=\"junction\" cx=\"{}\" cy=\"{}\" r=\"0.3\" {}/>",
            j[0],
            j[1],
            attr("fill", Some(c))
        );
    }

    // Pin dots.
    for p in &pin_points {
        if pal.pin.is_some() {
            let _ = writeln!(
                body,
                "<circle class=\"pin\" cx=\"{}\" cy=\"{}\" r=\"0.18\" {}/>",
                p[0],
                p[1],
                attr("fill", pal.pin)
            );
        }
        bb.add([p[0] as f64, p[1] as f64]);
    }

    // Net labels.
    for l in &circuit.labels {
        let at = [l.at[0] as f64 + 0.3, l.at[1] as f64 - 0.3];
        if let Some(c) = color::label_color(l, &pal) {
            let _ = writeln!(
                body,
                "<text class=\"label\" x=\"{}\" y=\"{}\" font-size=\"0.9\" font-weight=\"bold\" {}>{}</text>",
                num(at[0]),
                num(at[1]),
                attr("fill", Some(c)),
                esc(&l.name)
            );
        }
        bb.add([l.at[0] as f64, l.at[1] as f64]);
        bb.add_text(at, &l.name, 0.9, Anchor::Start);
    }

    // Title goes above the content.
    let mut head = String::new();
    if bb.is_empty() {
        bb.add([0.0, 0.0]);
        bb.add([8.0, 4.0]);
    }
    if let Some(t) = circuit.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let at = [bb.min[0], bb.min[1] - 1.2];
        let _ = writeln!(
            head,
            "<text class=\"title\" x=\"{}\" y=\"{}\" font-size=\"1.4\" font-weight=\"bold\" {}>{}</text>",
            num(at[0]),
            num(at[1]),
            attr("fill", pal.part.or(base.part)),
            esc(t)
        );
        bb.add_text(at, t, 1.4, Anchor::Start);
    }

    let vx = (bb.min[0] - MARGIN).floor();
    let vy = (bb.min[1] - MARGIN).floor();
    let vw = (bb.max[0] + MARGIN).ceil() - vx;
    let vh = (bb.max[1] + MARGIN).ceil() - vy;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" font-family=\"sans-serif\">",
        num(vw * PX_PER_UNIT),
        num(vh * PX_PER_UNIT),
        num(vx),
        num(vy),
        num(vw),
        num(vh)
    );
    if let Some(t) = circuit.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = writeln!(out, "<title>{}</title>", esc(t));
    }
    if let Some(bg) = pal.background {
        let _ = writeln!(
            out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" {}/>",
            num(vx),
            num(vy),
            num(vw),
            num(vh),
            attr("fill", Some(bg))
        );
    }
    out.push_str(&head);
    out.push_str(&body);
    out.push_str("</svg>\n");
    out
}

// ---------------------------------------------------------------------------------------
// helpers

struct Bounds {
    min: [f64; 2],
    max: [f64; 2],
}

impl Bounds {
    fn new() -> Self {
        Bounds { min: [f64::INFINITY; 2], max: [f64::NEG_INFINITY; 2] }
    }
    fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }
    fn add(&mut self, p: [f64; 2]) {
        self.min = [self.min[0].min(p[0]), self.min[1].min(p[1])];
        self.max = [self.max[0].max(p[0]), self.max[1].max(p[1])];
    }
    /// Rough text extent: 0.6 em per character, ascent `size`, descent 0.3 em.
    fn add_text(&mut self, at: [f64; 2], text: &str, size: f64, anchor: Anchor) {
        let w = 0.6 * size * text.chars().count() as f64;
        let (x0, x1) = match anchor {
            Anchor::Start => (at[0], at[0] + w),
            Anchor::Middle => (at[0] - w / 2.0, at[0] + w / 2.0),
            Anchor::End => (at[0] - w, at[0]),
        };
        self.add([x0, at[1] - size]);
        self.add([x1, at[1] + 0.3 * size]);
    }
}

/// Fixed-precision number without trailing zeros and without "-0".
fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let r = (v * 1000.0).round() / 1000.0;
    let mut s = format!("{:.3}", if r == 0.0 { 0.0 } else { r });
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

/// Presentation attributes of a shape of `part`. `closed` shapes may be filled.
fn paint(st: &Style, part: &Part, pal: &Palette, closed: bool) -> String {
    let fill = if closed { color::fill_color(st, part) } else { None };
    format!(
        "{} {} stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
        attr("fill", fill),
        attr("stroke", color::stroke_color(st, part, pal)),
        num(st.width.unwrap_or(DEFAULT_WIDTH))
    )
}

/// Expand `{ref}`, `{value}` and `{prop:name}`. Unknown keys are kept verbatim.
fn expand(t: &str, part: &Part, hide_ref: bool) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            rest = &rest[i..];
            break;
        };
        let key = &rest[i + 1..i + j];
        if key == "ref" {
            if !hide_ref {
                out.push_str(&part.id);
            }
        } else if key == "value" {
            out.push_str(part.props.get("value").map_or("", |s| s.as_str()));
        } else if let Some(name) = key.strip_prefix("prop:") {
            out.push_str(part.props.get(name).map_or("", |s| s.as_str()));
        } else {
            out.push_str(&rest[i..i + j + 1]);
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

fn svg_anchor(a: Anchor) -> &'static str {
    match a {
        Anchor::Start => "start",
        Anchor::Middle => "middle",
        Anchor::End => "end",
    }
}

fn angle_in_sweep(from: f64, delta: f64, theta: f64) -> bool {
    (theta - from).rem_euclid(360.0) <= delta
}

fn render_part(out: &mut String, part: &Part, def: &PartDef, pal: &Palette, bb: &mut Bounds) {
    let _ = writeln!(out, "<g id=\"{}\" class=\"part\" data-type=\"{}\">", esc(&part.id), esc(&part.ty));
    let (lo, hi) = world_bbox(part, def);
    bb.add(lo);
    bb.add(hi);
    let center = to_world(part, [def.size[0] / 2.0, def.size[1] / 2.0]);
    let transformed = part.rot.rem_euclid(360) != 0 || part.mirror;

    let fallback;
    let shapes: &[Shape] = if def.shapes.is_empty() {
        fallback = def.svg.as_deref().map(|d| svgimport::path_to_shapes(d, 1.0).unwrap_or_default());
        fallback.as_deref().unwrap_or(&[])
    } else {
        &def.shapes
    };

    for s in shapes {
        match s {
            Shape::Line { pts, style } | Shape::Poly { pts, style } => {
                if pts.len() < 2 {
                    continue;
                }
                let w: Vec<[f64; 2]> = pts.iter().map(|p| to_world(part, *p)).collect();
                w.iter().for_each(|p| bb.add(*p));
                let list: Vec<String> = w.iter().map(|p| format!("{},{}", num(p[0]), num(p[1]))).collect();
                if matches!(s, Shape::Poly { .. }) {
                    let _ = writeln!(out, "<polygon points=\"{}\" {}/>", list.join(" "), paint(style, part, pal, true));
                } else {
                    let _ = writeln!(out, "<polyline points=\"{}\" {}/>", list.join(" "), paint(style, part, pal, false));
                }
            }
            Shape::Rect { at, size, r, style } => {
                let (p0, p1) = (to_world(part, *at), to_world(part, [at[0] + size[0], at[1] + size[1]]));
                let (x, y) = (p0[0].min(p1[0]), p0[1].min(p1[1]));
                let (w, h) = ((p1[0] - p0[0]).abs(), (p1[1] - p0[1]).abs());
                bb.add([x, y]);
                bb.add([x + w, y + h]);
                let rr = if *r > 0.0 { format!(" rx=\"{}\"", num(*r)) } else { String::new() };
                let _ = writeln!(
                    out,
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{} {}/>",
                    num(x),
                    num(y),
                    num(w),
                    num(h),
                    rr,
                    paint(style, part, pal, true)
                );
            }
            Shape::Circle { at, r, style } => {
                let c = to_world(part, *at);
                bb.add([c[0] - r, c[1] - r]);
                bb.add([c[0] + r, c[1] + r]);
                let _ = writeln!(
                    out,
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" {}/>",
                    num(c[0]),
                    num(c[1]),
                    num(*r),
                    paint(style, part, pal, true)
                );
            }
            Shape::Arc { at, r, from, to, style } => {
                let delta = (to - from).rem_euclid(360.0);
                if delta < 1e-9 && (to - from).abs() < 1e-9 {
                    continue;
                }
                if delta < 1e-9 {
                    // Full turn.
                    let c = to_world(part, *at);
                    bb.add([c[0] - r, c[1] - r]);
                    bb.add([c[0] + r, c[1] + r]);
                    let _ = writeln!(
                        out,
                        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" {}/>",
                        num(c[0]),
                        num(c[1]),
                        num(*r),
                        paint(style, part, pal, false)
                    );
                    continue;
                }
                let pt = |deg: f64| {
                    let (s, c) = deg.to_radians().sin_cos();
                    to_world(part, [at[0] + r * c, at[1] + r * s])
                };
                let (a, b) = (pt(*from), pt(*to));
                bb.add(a);
                bb.add(b);
                for card in [0.0, 90.0, 180.0, 270.0] {
                    if angle_in_sweep(*from, delta, card) {
                        bb.add(pt(card));
                    }
                }
                let large = if delta > 180.0 { 1 } else { 0 };
                let sweep = if part.mirror { 0 } else { 1 };
                let _ = writeln!(
                    out,
                    "<path d=\"M{} {} A{} {} 0 {} {} {} {}\" {}/>",
                    num(a[0]),
                    num(a[1]),
                    num(*r),
                    num(*r),
                    large,
                    sweep,
                    num(b[0]),
                    num(b[1]),
                    paint(style, part, pal, false)
                );
            }
            Shape::Text { at, text, size, anchor, style } => {
                let raw = expand(text, part, part.hidden_label);
                let shown = raw.trim();
                if shown.is_empty() {
                    continue;
                }
                let p = to_world(part, *at);
                let mut anchor = *anchor;
                if transformed {
                    let dx = p[0] - center[0];
                    anchor = match anchor {
                        Anchor::Start if dx < -0.5 => Anchor::End,
                        Anchor::End if dx > 0.5 => Anchor::Start,
                        a => a,
                    };
                }
                bb.add_text(p, shown, *size, anchor);
                let Some(fill) = color::text_color(style, part, pal) else { continue };
                let _ = writeln!(
                    out,
                    "<text x=\"{}\" y=\"{}\" font-size=\"{}\" text-anchor=\"{}\" {}>{}</text>",
                    num(p[0]),
                    num(p[1]),
                    num(*size),
                    svg_anchor(anchor),
                    attr("fill", Some(fill)),
                    esc(shown)
                );
            }
        }
    }
    out.push_str("</g>\n");
}

fn render_unknown(out: &mut String, part: &Part, dark: bool, bb: &mut Bounds) {
    let err = error_color(dark);
    let (x, y) = (part.at[0] as f64, part.at[1] as f64);
    let label = format!("{}: {}?", part.id, part.ty);
    let _ = writeln!(out, "<g id=\"{}\" class=\"part unknown\" data-type=\"{}\">", esc(&part.id), esc(&part.ty));
    let _ = writeln!(
        out,
        "<rect x=\"{}\" y=\"{}\" width=\"4\" height=\"2\" fill=\"none\" stroke=\"{}\" stroke-width=\"0.15\" stroke-dasharray=\"0.4 0.3\"/>",
        num(x),
        num(y),
        err
    );
    let _ = writeln!(
        out,
        "<text x=\"{}\" y=\"{}\" font-size=\"0.8\" text-anchor=\"middle\" fill=\"{}\">{}</text>",
        num(x + 2.0),
        num(y + 1.3),
        err,
        esc(&label)
    );
    out.push_str("</g>\n");
    bb.add([x, y]);
    bb.add([x + 4.0, y + 2.0]);
    bb.add_text([x + 2.0, y + 1.3], &label, 0.8, Anchor::Middle);
}

/// Points where three or more wire arms (or pins) meet. Arms: a wire endpoint counts 1, an
/// interior vertex 2, a pin 1, and a wire running straight through a point where another
/// wire ends counts 2 (T junction). Crossings without a vertex never produce a dot.
fn junctions(circuit: &Circuit, pins: &[[i32; 2]]) -> Vec<[i32; 2]> {
    let mut degree: BTreeMap<[i32; 2], u32> = BTreeMap::new();
    let mut ends: BTreeMap<[i32; 2], Vec<usize>> = BTreeMap::new();
    for (wi, w) in circuit.wires.iter().enumerate() {
        if w.pts.len() < 2 {
            continue;
        }
        let last = w.pts.len() - 1;
        for (i, p) in w.pts.iter().enumerate() {
            if i == 0 || i == last {
                *degree.entry(*p).or_default() += 1;
                ends.entry(*p).or_default().push(wi);
            } else {
                *degree.entry(*p).or_default() += 2;
            }
        }
    }
    for p in pins {
        if let Some(d) = degree.get_mut(p) {
            *d += 1;
        }
    }
    let keys: Vec<[i32; 2]> = degree.keys().copied().collect();
    for p in keys {
        for (wi, w) in circuit.wires.iter().enumerate() {
            let passes = w.pts.windows(2).any(|s| strictly_inside(p, s[0], s[1]));
            let other_end = ends.get(&p).is_some_and(|v| v.iter().any(|e| *e != wi));
            if passes && other_end {
                *degree.get_mut(&p).expect("key exists") += 2;
            }
        }
    }
    degree.into_iter().filter(|(_, d)| *d >= 3).map(|(p, _)| p).collect()
}

/// `p` lies on segment `a`-`b` but is neither endpoint.
fn strictly_inside(p: [i32; 2], a: [i32; 2], b: [i32; 2]) -> bool {
    if p == a || p == b {
        return false;
    }
    let (px, py, ax, ay, bx, by) =
        (p[0] as i64, p[1] as i64, a[0] as i64, a[1] as i64, b[0] as i64, b[1] as i64);
    (bx - ax) * (py - ay) - (by - ay) * (px - ax) == 0
        && px >= ax.min(bx)
        && px <= ax.max(bx)
        && py >= ay.min(by)
        && py <= ay.max(by)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_circuit, Library};

    fn render(src: &str, dark: bool) -> String {
        let c = parse_circuit(src).unwrap();
        render_svg(&c, &Library::load(&c, None), dark)
    }

    /// Minimal well-formedness check: every open tag is closed in order.
    fn assert_balanced(svg: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = svg;
        while let Some(i) = rest.find('<') {
            let j = rest[i..].find('>').expect("unterminated tag") + i;
            let tag = &rest[i + 1..j];
            rest = &rest[j + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                assert_eq!(stack.pop().as_deref(), Some(name.trim()), "mismatched close");
            } else if !tag.ends_with('/') {
                stack.push(tag.split_whitespace().next().unwrap().to_string());
            }
        }
        assert!(stack.is_empty(), "unclosed: {stack:?}");
    }

    #[test]
    fn led_circuit_renders() {
        let svg = render(include_str!("../../../examples/led.circ"), false);
        assert!(svg.starts_with("<svg"));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert_balanced(&svg);
        for id in ["B1", "R1", "D1", "w1", "w2", "w3"] {
            assert!(svg.contains(&format!("id=\"{id}\"")), "{id}");
        }
        assert!(svg.contains(">GND</text>"));
        assert!(svg.contains(">Battery, resistor and LED loop</text>"));
        assert!(svg.contains(">330</text>"), "value expansion");
        // Rotated LED: anode pin (0,1) -> (25,6), cathode (4,1) -> (25,10).
        assert!(svg.contains("class=\"pin\" cx=\"25\" cy=\"6\""));
        assert!(svg.contains("class=\"pin\" cx=\"25\" cy=\"10\""));
        // Deterministic.
        assert_eq!(svg, render(include_str!("../../../examples/led.circ"), false));
        assert_ne!(svg, render(include_str!("../../../examples/led.circ"), true));
    }

    #[test]
    fn all_builtin_parts_render_balanced() {
        let lib = Library::builtin();
        let mut c = parse_circuit(r#"{"format":"short-circ/1"}"#).unwrap();
        let types: Vec<String> = lib.types().map(String::from).collect();
        for (i, ty) in types.iter().enumerate() {
            c.parts.push(Part {
                id: format!("P{i}"),
                ty: ty.clone(),
                at: [(i as i32 % 6) * 14, (i as i32 / 6) * 14],
                rot: [0, 90, 180, 270][i % 4],
                mirror: i % 3 == 0,
                props: [("value".to_string(), "v<&>".to_string())].into(),
                ..Part::default()
            });
        }
        let svg = render_svg(&c, &lib, true);
        assert_balanced(&svg);
        assert!(svg.contains("v&lt;&amp;&gt;"));
        assert_eq!(svg.matches("class=\"part\"").count(), types.len());
    }

    #[test]
    fn generated_dip_renders() {
        let svg = render(
            r#"{"format":"short-circ/1","parts":[{"id":"U1","type":"dip8","at":[2,2],"props":{"value":"NE555"}}]}"#,
            false,
        );
        assert_balanced(&svg);
        assert_eq!(svg.matches("class=\"pin\"").count(), 8);
        assert!(svg.contains(">NE555</text>") && svg.contains(">U1</text>"));
        assert!(svg.contains(" A0.6 0.6 0 0 1 "), "notch arc");
    }

    #[test]
    fn unknown_type_is_red_dashed_and_xml_is_escaped() {
        let svg = render(
            r#"{"format":"short-circ/1","title":"a<b & \"c\"","parts":[{"id":"X1","type":"nope<1>","at":[3,3]}]}"#,
            false,
        );
        assert_balanced(&svg);
        assert!(svg.contains("stroke-dasharray") && svg.contains("#d32f2f"));
        assert!(svg.contains("X1: nope&lt;1&gt;?"));
        assert!(svg.contains("a&lt;b &amp; &quot;c&quot;"));
        assert!(svg.contains("id=\"X1\""));
    }

    #[test]
    fn junction_dots_only_where_three_arms_meet() {
        let t = r#"{"format":"short-circ/1","wires":[
            {"id":"a","pts":[[0,0],[10,0]]},{"id":"b","pts":[[5,0],[5,5]]},
            {"id":"c","pts":[[20,0],[30,0]]},{"id":"d","pts":[[25,-3],[25,3]]},
            {"id":"e","pts":[[40,0],[45,0],[45,5]]}]}"#;
        let svg = render(t, false);
        assert_balanced(&svg);
        assert_eq!(svg.matches("class=\"junction\"").count(), 1, "only the T at (5,0)");
        assert!(svg.contains("class=\"junction\" cx=\"5\" cy=\"0\""));
    }

    #[test]
    fn empty_circuit_has_viewbox() {
        let svg = render(r#"{"format":"short-circ/1"}"#, false);
        assert_balanced(&svg);
        assert!(svg.contains("viewBox=\""));
    }

    #[test]
    fn text_expansion() {
        let mut p = Part {
            id: "R7".into(),
            ty: "resistor".into(),
            at: [0, 0],
            rot: 0,
            mirror: false,
            props: Default::default(),
            ..Part::default()
        };
        p.props.insert("tol".into(), "1%".into());
        assert_eq!(expand("{ref} {value}|{prop:tol}|{prop:zz}|{x}|{", &p, false), "R7 |1%||{x}|{");
        assert_eq!(expand("{ref}!", &p, true), "!");
    }

    const COLORED: &str = r##"{"format":"short-circ/1","title":"T",
        "theme":{"background":"#102030","wire":"teal","pin":"#abcdef","label":"gold","junction":"pink"},
        "lib":[{"type":"box","size":[4,2],"pins":[{"id":"a","at":[0,1]}],"shapes":[
            {"k":"rect","at":[0,0],"size":[4,2]},
            {"k":"circle","at":[2,1],"r":0.5,"stroke":"#00ff00","fill":"#0000ff"},
            {"k":"line","pts":[[0,1],[1,1]]},
            {"k":"text","at":[2,3],"text":"{ref}","size":1}]}],
        "parts":[{"id":"P1","type":"box","at":[0,0],"color":"red","fill":"yellow","label_color":"#123456"},
                 {"id":"P2","type":"box","at":[10,0]}],
        "wires":[{"id":"w1","pts":[[4,1],[8,1]],"color":"#ff8800"},{"id":"w2","pts":[[6,1],[6,5]]},
                 {"id":"w3","pts":[[20,1],[24,1]],"color":"none"}],
        "labels":[{"at":[8,1],"name":"A","color":"#010203"},{"at":[6,5],"name":"B"}]}"##;

    #[test]
    fn explicit_colours_reach_the_svg_and_follow_precedence() {
        let svg = render(COLORED, false);
        assert_balanced(&svg);
        // Theme background, normalised hex.
        assert!(svg.contains("height=\"") && svg.contains("fill=\"#102030\"/>"), "{svg}");
        // Wires: own colour, theme colour, and "none" omits the element.
        assert!(svg.contains("id=\"w1\" class=\"wire\" points=\"4,1 8,1\" fill=\"none\" stroke=\"#ff8800\""));
        assert!(svg.contains("id=\"w2\" class=\"wire\" points=\"6,1 6,5\" fill=\"none\" stroke=\"#00897b\""));
        assert!(!svg.contains("id=\"w3\""));
        // Part P1: part.color / part.fill apply to the rect; the circle keeps its own colours.
        let p1 = &svg[svg.find("<g id=\"P1\"").unwrap()..svg.find("<g id=\"P2\"").unwrap()];
        assert!(p1.contains("<rect") && p1.contains("fill=\"#f2c200\" stroke=\"#d93025\""));
        assert!(p1.contains("<circle cx=\"2\" cy=\"1\" r=\"0.5\" fill=\"#0000ff\" stroke=\"#00ff00\""));
        assert!(p1.contains("<polyline") && p1.contains("fill=\"none\" stroke=\"#d93025\""));
        assert!(p1.contains("fill=\"#123456\">P1</text>"));
        // Part P2 has no colours: unfilled rect in the light default part colour, text likewise.
        let p2 = &svg[svg.find("<g id=\"P2\"").unwrap()..];
        assert!(p2.contains("<rect") && p2.contains("fill=\"none\" stroke=\"#1e2024\""));
        assert!(p2.contains("fill=\"#1e2024\">P2</text>"));
        // Pins, labels, junction dots.
        assert!(svg.contains("class=\"pin\" cx=\"0\" cy=\"1\" r=\"0.18\" fill=\"#abcdef\""));
        assert!(svg.contains("class=\"label\"") && svg.contains("fill=\"#010203\">A</text>"));
        assert!(svg.contains("fill=\"#d4a017\">B</text>"), "theme.label gold");
        assert!(svg.contains("class=\"junction\" cx=\"6\" cy=\"1\" r=\"0.3\" fill=\"#f48fb1\""), "theme.junction pink");
    }

    #[test]
    fn junction_takes_wire_colour_and_none_hides_things() {
        let t = r##"{"format":"short-circ/1","theme":{"background":"none","pin":"none"},"wires":[
            {"id":"a","pts":[[0,0],[10,0]],"color":"#ff0000"},{"id":"b","pts":[[5,0],[5,5]]}]}"##;
        let svg = render(t, false);
        assert!(svg.contains("class=\"junction\" cx=\"5\" cy=\"0\" r=\"0.3\" fill=\"#ff0000\""));
        assert!(!svg.contains("<rect"), "background none: no rect");
        assert!(!svg.contains("class=\"pin\""));
    }

    #[test]
    fn explicit_black_is_not_inverted_in_dark_mode() {
        let t = r##"{"format":"short-circ/1","wires":[{"id":"a","pts":[[0,0],[4,0]],"color":"black"}]}"##;
        assert!(render(t, true).contains("stroke=\"#000000\""));
        assert!(render(t, false).contains("stroke=\"#000000\""));
    }

    #[test]
    fn translucent_colour_uses_opacity_attribute() {
        let t = r##"{"format":"short-circ/1","wires":[{"id":"a","pts":[[0,0],[4,0]],"color":"#ff000080"}]}"##;
        let svg = render(t, false);
        assert!(svg.contains("stroke=\"#ff0000\" stroke-opacity=\"0.502\""), "{svg}");
    }

    #[test]
    fn arc_flags_and_numbers() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(0.12345), "0.123");
        assert_eq!(num(2.5), "2.5");
    }
}
