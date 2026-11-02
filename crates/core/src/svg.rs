//! Static SVG export of a whole circuit, one view at a time.
//!
//! Output coordinates are grid units (the SVG user space); the root element's `width`/`height`
//! are 10 px per unit, and the `viewBox` is the content bounding box plus a 2 unit margin
//! snapped to whole units. Output is deterministic (file order, fixed number formatting).
//!
//! Colours come from `color` (the same resolver the viewer uses): explicit colours are written as
//! normalised `#rrggbb` hex (plus `*-opacity` when not opaque), `"none"` omits the element.
//!
//! Schematic drawing order: background, title, parts (each in a `<g id="REF">`), wires, junction
//! dots, pin dots, net labels. Text stays upright whatever the part rotation; `start`/`end`
//! anchors of a rotated or mirrored part are swapped when the text ends up on the other side of
//! the part centre, so labels keep pointing away from the symbol.
//!
//! Breadboard: boards first, then the other parts. A view's `svg` artwork is embedded as a nested
//! `<svg>` (position and size from the part's box, the source `viewBox` kept,
//! `preserveAspectRatio="none"`) inside a `<g transform>` that implements the placement
//! transform; `shapes` are drawn on top. Pins of parts without artwork get small hole markers.
//! Wires are round-capped jumpers, coloured per net unless they carry their own colour.

use crate::color::{self, Palette, Rgba};
use crate::model::{
    Anchor, Circuit, Part, PartDef, PartLookup, Placement, Shape, Style, View, ViewDef,
};
use crate::netlist::Netlist;
use crate::transform::{pin_world, to_world, world_bbox};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

const PX_PER_UNIT: f64 = 10.0;
const MARGIN: f64 = 2.0;
const DEFAULT_WIDTH: f64 = 0.12;
const WIRE_WIDTH: f64 = 0.15;
const JUMPER_WIDTH: f64 = 0.3;

/// Preset colours given to breadboard jumpers by net (in order of first appearance).
const JUMPER_COLORS: [&str; 10] =
    ["red", "blue", "green", "orange", "purple", "teal", "magenta", "brown", "cyan", "gold"];

/// Id of the placed part in a one-part drawing (`render_part_svg`).
pub const PART_ID: &str = "X";

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

/// Render one view of the circuit as a standalone SVG document.
pub fn render_svg(circuit: &Circuit, lib: &dyn PartLookup, dark: bool, view: View) -> String {
    render(circuit, lib, dark, view, &[])
}

/// Label for a pin: its id, plus its name when it has a different one.
pub fn pin_caption(id: &str, name: Option<&str>) -> String {
    match name.map(str::trim) {
        Some(n) if !n.is_empty() && n != id => format!("{id}:{n}"),
        _ => id.to_string(),
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// One-line description of a part view: `type - Name (category), WxH, N pin(s)`.
pub fn part_title(def: &PartDef, view: View) -> String {
    let mut s = def.ty.clone();
    if !def.name.is_empty() && def.name != def.ty {
        s.push_str(&format!(" - {}", def.name));
    }
    if let Some(c) = def.category.as_deref().filter(|c| !c.is_empty()) {
        s.push_str(&format!(" ({c})"));
    }
    let size = def.view(view).map_or([0.0, 0.0], |v| v.size);
    s.push_str(&format!(", {}x{}, {} pin(s)", fmt_num(size[0]), fmt_num(size[1]), def.pins.len()));
    s
}

/// Draw one view of a single component: the artwork, a thin outline of its declared bounding
/// box, every pin that has a position in the view labelled with its id (`id:name` when named),
/// and a title line `type - name (category), WxH, N pin(s)`. `Err` when the part has no `view`.
pub fn render_part_svg(def: &PartDef, view: View, dark: bool) -> Result<String, String> {
    if def.view(view).is_none() {
        return Err(format!("part \"{}\" has no {} view", def.ty, view.key()));
    }
    let mut d = def.clone();
    let vm = match view {
        View::Schematic => d.schematic.as_mut(),
        View::Breadboard => d.breadboard.as_mut(),
    }
    .expect("view checked above");
    vm.shapes.push(Shape::Rect {
        at: [0.0, 0.0],
        size: vm.size,
        r: 0.0,
        style: Style { stroke: Some("#8a8a8a".into()), fill: None, width: Some(0.04) },
    });
    let labels: Vec<([i32; 2], String)> = def
        .placed_pins(view)
        .map(|(p, pos)| (pos.at, pin_caption(&p.id, p.name.as_deref())))
        .collect();
    let mut part = Part { id: PART_ID.into(), ty: def.ty.clone(), hidden_label: true, ..Part::default() };
    *part.placement_mut(view) = Some(Placement::at(0, 0));
    let circuit = Circuit { title: Some(part_title(def, view)), parts: vec![part], ..Default::default() };
    let mut defs = BTreeMap::new();
    defs.insert(d.ty.clone(), d);
    Ok(render(&circuit, &defs, dark, view, &labels))
}

fn render(circuit: &Circuit, lib: &dyn PartLookup, dark: bool, view: View, extra_labels: &[([i32; 2], String)]) -> String {
    let schematic = view == View::Schematic;
    let pal = Palette::for_circuit(circuit, dark);
    let base = Palette::builtin(dark);
    let mut bb = Bounds::new();
    let mut body = String::new();

    // Parts placed in this view (breadboard: boards first so everything sits on top of them).
    let mut placed: Vec<(&Part, &Placement)> =
        circuit.parts.iter().filter_map(|p| p.placement(view).map(|pl| (p, pl))).collect();
    if !schematic {
        placed.sort_by_key(|(p, _)| !lib.get(&p.ty).is_some_and(|d| d.is_board()));
    }

    let mut embedded: HashMap<String, Option<Embedded>> = HashMap::new();
    let mut pin_points: Vec<[i32; 2]> = Vec::new();
    let mut holes: Vec<[i32; 2]> = Vec::new();
    for (part, pl) in placed {
        match lib.get(&part.ty).and_then(|def| def.view(view).map(|vd| (def, vd))) {
            Some((def, vd)) => {
                let emb = match vd.svg.as_deref() {
                    Some(src) => embedded
                        .entry(part.ty.clone())
                        .or_insert_with(|| Embedded::parse(src, vd.size))
                        .as_ref(),
                    None => None,
                };
                render_part(&mut body, part, pl, vd, emb, &pal, &mut bb);
                for (_, pos) in def.placed_pins(view) {
                    let at = pin_world(pl, pos);
                    pin_points.push(at);
                    if !schematic && vd.svg.is_none() {
                        holes.push(at);
                    }
                }
            }
            None => render_unknown(&mut body, part, pl, view, lib.get(&part.ty).is_some(), dark, &mut bb),
        }
    }

    // Wires.
    let wires: Vec<&crate::model::Wire> = circuit.wires_in(view).collect();
    let net_colors = if schematic { None } else { Some(jumper_colors(circuit, lib)) };
    for (wi, w) in wires.iter().enumerate() {
        if w.pts.len() < 2 {
            continue;
        }
        for p in &w.pts {
            bb.add([p[0] as f64, p[1] as f64]);
        }
        let pts: Vec<String> = w.pts.iter().map(|p| format!("{},{}", p[0], p[1])).collect();
        if schematic {
            let Some(color) = color::wire_color(w, &pal) else { continue };
            let _ = writeln!(
                body,
                "<polyline id=\"{}\" class=\"wire\" points=\"{}\" fill=\"none\" {} stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                esc(&w.id),
                pts.join(" "),
                attr("stroke", Some(color)),
                num(WIRE_WIDTH)
            );
        } else {
            let by_net = net_colors.as_ref().and_then(|c| c.get(wi).copied().flatten());
            let Some(color) = jumper_color(w, circuit, &pal, by_net) else { continue };
            let _ = writeln!(
                body,
                "<polyline id=\"{}\" class=\"jumper\" points=\"{}\" fill=\"none\" {} stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                esc(&w.id),
                pts.join(" "),
                attr("stroke", Some(color)),
                num(JUMPER_WIDTH)
            );
        }
    }

    if schematic {
        // Junction dots.
        for j in junctions(&wires, &pin_points) {
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
        // Net labels, then caller-supplied pin captions.
        let own = circuit.labels.iter().map(|l| (l.at, l.name.as_str(), color::label_color(l, &pal)));
        let extra = extra_labels.iter().map(|(at, name)| (*at, name.as_str(), pal.label));
        for (at_pt, name, c) in own.chain(extra) {
            let at = [at_pt[0] as f64 + 0.3, at_pt[1] as f64 - 0.3];
            if let Some(c) = c {
                let _ = writeln!(
                    body,
                    "<text class=\"label\" x=\"{}\" y=\"{}\" font-size=\"0.9\" font-weight=\"bold\" {}>{}</text>",
                    num(at[0]),
                    num(at[1]),
                    attr("fill", Some(c)),
                    esc(name)
                );
            }
            bb.add([at_pt[0] as f64, at_pt[1] as f64]);
            bb.add_text(at, name, 0.9, Anchor::Start);
        }
    } else {
        // Hole markers for parts drawn without artwork.
        for p in &holes {
            if pal.pin.is_some() {
                let _ = writeln!(
                    body,
                    "<circle class=\"hole\" cx=\"{}\" cy=\"{}\" r=\"0.15\" {}/>",
                    p[0],
                    p[1],
                    attr("fill", pal.pin)
                );
            }
        }
        for p in &pin_points {
            bb.add([p[0] as f64, p[1] as f64]);
        }
        for (at_pt, name) in extra_labels {
            let at = [at_pt[0] as f64 + 0.3, at_pt[1] as f64 - 0.3];
            if let Some(c) = pal.label {
                let _ = writeln!(
                    body,
                    "<text class=\"label\" x=\"{}\" y=\"{}\" font-size=\"0.9\" font-weight=\"bold\" {}>{}</text>",
                    num(at[0]),
                    num(at[1]),
                    attr("fill", Some(c)),
                    esc(name)
                );
            }
            bb.add([at_pt[0] as f64, at_pt[1] as f64]);
            bb.add_text(at, name, 0.9, Anchor::Start);
        }
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
// breadboard jumpers

/// Per breadboard wire (file order within the view): the preset colour of its net, assigned to
/// nets in order of first appearance among the wires.
pub fn jumper_colors(circuit: &Circuit, lib: &dyn PartLookup) -> Vec<Option<Rgba>> {
    let nl = Netlist::build(circuit, lib, View::Breadboard);
    let mut next: HashMap<usize, usize> = HashMap::new();
    nl.wire_net_idx
        .iter()
        .map(|&net| {
            let n = next.len();
            let slot = *next.entry(net).or_insert(n);
            color::parse_color(JUMPER_COLORS[slot % JUMPER_COLORS.len()])
        })
        .collect()
}

/// Jumper colour: `wire.color`, else `theme.wire` if the theme sets it, else the net colour.
pub fn jumper_color(w: &crate::model::Wire, circuit: &Circuit, pal: &Palette, by_net: Option<Rgba>) -> Option<Rgba> {
    let theme_sets = circuit
        .theme
        .as_ref()
        .and_then(|t| t.wire.as_deref())
        .is_some_and(|s| !matches!(color::classify(s), color::Spec::Unset));
    let fallback = if theme_sets { pal.wire } else { by_net.or(pal.wire) };
    color::resolve(&[w.color.as_deref()], fallback)
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

// ---------------------------------------------------------------------------------------
// embedded artwork

/// The parts of a view's `svg` document needed to nest it in the output.
struct Embedded {
    view_box: String,
    /// Namespace declarations of the source root (`xmlns:xlink="..."`), repeated on the nested
    /// element so prefixed attributes in the content keep resolving.
    namespaces: String,
    inner: String,
}

impl Embedded {
    /// `None` when the document is not well-formed SVG or contains script or foreign content
    /// (the artwork is then skipped; the part's `shapes` are still drawn).
    fn parse(src: &str, size: [f64; 2]) -> Option<Embedded> {
        let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let doc = roxmltree::Document::parse_with_options(src, opts).ok()?;
        let root = doc.root_element();
        if !root.tag_name().name().eq_ignore_ascii_case("svg") {
            return None;
        }
        for n in doc.descendants().filter(|n| n.is_element()) {
            let tag = n.tag_name().name().to_ascii_lowercase();
            if tag == "script" || tag == "foreignobject" {
                return None;
            }
            for a in n.attributes() {
                let name = a.name().to_ascii_lowercase();
                let value = a.value().trim_start().to_ascii_lowercase();
                if name.starts_with("on") || value.starts_with("javascript:") {
                    return None;
                }
            }
        }
        let number = |name: &str| -> Option<f64> {
            let v = root.attribute(name)?;
            let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(v.len());
            v[..end].parse().ok()
        };
        let view_box = match root.attribute("viewBox").map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => v.to_string(),
            None => {
                let w = number("width").unwrap_or(size[0] * 10.0);
                let h = number("height").unwrap_or(size[1] * 10.0);
                format!("0 0 {} {}", num(w), num(h))
            }
        };
        let mut namespaces = String::new();
        for ns in root.namespaces() {
            if let Some(prefix) = ns.name() {
                let _ = write!(namespaces, " xmlns:{prefix}=\"{}\"", esc(ns.uri()));
            }
        }
        let inner = match (root.first_child(), root.last_child()) {
            (Some(a), Some(b)) => src[a.range().start..b.range().end].to_string(),
            _ => String::new(),
        };
        Some(Embedded { view_box, namespaces, inner })
    }
}

/// A standalone SVG document of a view's `svg` artwork stretched over `size` grid units (the
/// source `viewBox` kept, `preserveAspectRatio="none"`), exactly as `render_svg` embeds it, with
/// the same sanitising. The document is `size` user units wide and high, so a rasteriser only
/// needs a uniform scale. `None` when the source is not well-formed SVG or has script content.
pub fn artwork_document(src: &str, size: [f64; 2]) -> Option<String> {
    let e = Embedded::parse(src, size)?;
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"><svg x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" viewBox=\"{}\" preserveAspectRatio=\"none\"{}>{}</svg></svg>",
        esc(&e.view_box),
        e.namespaces,
        e.inner,
        w = num(size[0]),
        h = num(size[1])
    ))
}

/// SVG `transform` value for a placement: `translate` when plain, else an exact `matrix`.
fn placement_transform(pl: &Placement) -> String {
    let o = to_world(pl, [0.0, 0.0]);
    if pl.rot.rem_euclid(360) == 0 && !pl.mirror {
        return format!("translate({} {})", num(o[0]), num(o[1]));
    }
    let x = to_world(pl, [1.0, 0.0]);
    let y = to_world(pl, [0.0, 1.0]);
    format!(
        "matrix({} {} {} {} {} {})",
        num(x[0] - o[0]),
        num(x[1] - o[1]),
        num(y[0] - o[0]),
        num(y[1] - o[1]),
        num(o[0]),
        num(o[1])
    )
}

fn render_part(
    out: &mut String,
    part: &Part,
    pl: &Placement,
    vd: &ViewDef,
    emb: Option<&Embedded>,
    pal: &Palette,
    bb: &mut Bounds,
) {
    let _ = writeln!(out, "<g id=\"{}\" class=\"part\" data-type=\"{}\">", esc(&part.id), esc(&part.ty));
    let (lo, hi) = world_bbox(pl, vd);
    bb.add(lo);
    bb.add(hi);
    let center = to_world(pl, [vd.size[0] / 2.0, vd.size[1] / 2.0]);
    let transformed = pl.rot.rem_euclid(360) != 0 || pl.mirror;

    if let Some(e) = emb {
        let _ = writeln!(
            out,
            "<g class=\"art\" transform=\"{}\"><svg x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" viewBox=\"{}\" preserveAspectRatio=\"none\"{}>{}</svg></g>",
            placement_transform(pl),
            num(vd.size[0]),
            num(vd.size[1]),
            esc(&e.view_box),
            e.namespaces,
            e.inner
        );
    }

    for s in &vd.shapes {
        match s {
            Shape::Line { pts, style } | Shape::Poly { pts, style } => {
                if pts.len() < 2 {
                    continue;
                }
                let w: Vec<[f64; 2]> = pts.iter().map(|p| to_world(pl, *p)).collect();
                w.iter().for_each(|p| bb.add(*p));
                let list: Vec<String> = w.iter().map(|p| format!("{},{}", num(p[0]), num(p[1]))).collect();
                if matches!(s, Shape::Poly { .. }) {
                    let _ = writeln!(out, "<polygon points=\"{}\" {}/>", list.join(" "), paint(style, part, pal, true));
                } else {
                    let _ = writeln!(out, "<polyline points=\"{}\" {}/>", list.join(" "), paint(style, part, pal, false));
                }
            }
            Shape::Rect { at, size, r, style } => {
                let (p0, p1) = (to_world(pl, *at), to_world(pl, [at[0] + size[0], at[1] + size[1]]));
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
                let c = to_world(pl, *at);
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
                    let c = to_world(pl, *at);
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
                    to_world(pl, [at[0] + r * c, at[1] + r * s])
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
                let sweep = if pl.mirror { 0 } else { 1 };
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
                let p = to_world(pl, *at);
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

/// Dashed placeholder for a part whose type is unknown, or known but without geometry for `view`.
fn render_unknown(
    out: &mut String,
    part: &Part,
    pl: &Placement,
    view: View,
    known_type: bool,
    dark: bool,
    bb: &mut Bounds,
) {
    let err = error_color(dark);
    let (x, y) = (pl.at[0] as f64, pl.at[1] as f64);
    let label = if known_type {
        format!("{}: no {} view", part.id, view.key())
    } else {
        format!("{}: {}?", part.id, part.ty)
    };
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
fn junctions(wires: &[&crate::model::Wire], pins: &[[i32; 2]]) -> Vec<[i32; 2]> {
    let mut degree: BTreeMap<[i32; 2], u32> = BTreeMap::new();
    let mut ends: BTreeMap<[i32; 2], Vec<usize>> = BTreeMap::new();
    for (wi, w) in wires.iter().enumerate() {
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
        for (wi, w) in wires.iter().enumerate() {
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
        render_svg(&c, &Library::load(&c, None), dark, View::Schematic)
    }

    fn render_bb(src: &str) -> String {
        let c = parse_circuit(src).unwrap();
        render_svg(&c, &Library::load(&c, None), false, View::Breadboard)
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
        let mut c = parse_circuit(r#"{"format":"short-circ/2"}"#).unwrap();
        let types: Vec<String> = lib.types().map(String::from).collect();
        let mut drawn = 0;
        for (i, ty) in types.iter().enumerate() {
            // Only types with a schematic view can be placed in the schematic.
            if lib.get(ty).and_then(|d| d.schematic.as_ref()).is_none() {
                continue;
            }
            drawn += 1;
            c.parts.push(Part {
                id: format!("P{i}"),
                ty: ty.clone(),
                props: [("value".to_string(), "v<&>".to_string())].into(),
                schematic: Some(Placement {
                    at: [(i as i32 % 6) * 14, (i as i32 / 6) * 14],
                    rot: [0, 90, 180, 270][i % 4],
                    mirror: i % 3 == 0,
                }),
                ..Part::default()
            });
        }
        let svg = render_svg(&c, &lib, true, View::Schematic);
        assert_balanced(&svg);
        assert!(svg.contains("v&lt;&amp;&gt;"));
        assert_eq!(svg.matches("class=\"part\"").count(), drawn);
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
        let mut p = Part { id: "R7".into(), ty: "resistor".into(), ..Part::default() };
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

    // ---- breadboard ---------------------------------------------------------------------

    const BB: &str = r##"{"format":"short-circ/2","title":"bb",
        "lib":[
          {"type":"strip","category":"board","pins":[{"id":"h1"},{"id":"h2"}],"buses":[["h1","h2"]],
           "breadboard":{"size":[2,1],"pins":{"h1":{"at":[0,0]},"h2":{"at":[1,0]}},
             "svg":"<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 20 10\"><rect id=\"b\" width=\"20\" height=\"10\" fill=\"#cccccc\"/></svg>"}},
          {"type":"res","pins":[{"id":"a"},{"id":"b"}],
           "breadboard":{"size":[4,1],"pins":{"a":{"at":[0,0]},"b":{"at":[4,0]}},
             "shapes":[{"k":"line","pts":[[0,0],[4,0]],"stroke":"#112233"},{"k":"text","at":[2,-1],"text":"{ref}","anchor":"middle"}]}}],
        "parts":[
          {"id":"R1","type":"res","breadboard":{"at":[10,5],"rot":90,"mirror":true}},
          {"id":"B1","type":"strip","breadboard":{"at":[3,4],"rot":180}}],
        "wires":[{"id":"j1","view":"breadboard","pts":[[0,0],[0,3],[4,3]]},
                 {"id":"j2","view":"breadboard","pts":[[8,0],[8,3]],"color":"#ff8800"},
                 {"id":"j3","view":"breadboard","pts":[[20,0],[20,3]]},
                 {"id":"s1","pts":[[0,0],[5,0]]}]}"##;

    #[test]
    fn breadboard_embeds_artwork_in_a_transformed_group() {
        let svg = render_bb(BB);
        assert_balanced(&svg);
        // Boards are drawn before other parts whatever the file order.
        assert!(svg.find("<g id=\"B1\"").unwrap() < svg.find("<g id=\"R1\"").unwrap());
        // B1 at (3,4) rot 180: matrix (-1 0 0 -1 3 4); nested svg uses the part size and source viewBox.
        assert!(svg.contains(
            "<g class=\"art\" transform=\"matrix(-1 0 0 -1 3 4)\"><svg x=\"0\" y=\"0\" width=\"2\" height=\"1\" viewBox=\"0 0 20 10\" preserveAspectRatio=\"none\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"><rect id=\"b\" width=\"20\" height=\"10\" fill=\"#cccccc\"/></svg></g>"
        ), "{svg}");
        // Shapes of R1 (no artwork) are drawn in world coordinates: mirror then rot 90 then translate.
        // local (0,0)->(10,5), (4,0): mirror (-4,0) -> rot90 (0,-4) -> (10,1).
        assert!(svg.contains("<polyline points=\"10,5 10,1\""), "{svg}");
        assert!(svg.contains("stroke=\"#112233\""));
        // Hole markers only for the part without artwork.
        assert_eq!(svg.matches("class=\"hole\"").count(), 2);
        // Schematic wires are not drawn.
        assert!(!svg.contains("id=\"s1\""));
        // No grid dots or junctions.
        assert!(!svg.contains("class=\"pin\"") && !svg.contains("class=\"junction\""));
    }

    #[test]
    fn breadboard_part_transform_is_exact_for_all_rotations() {
        for (rot, mirror, m) in [
            (0, false, "translate(3 4)"),
            (90, false, "matrix(0 1 -1 0 3 4)"),
            (180, false, "matrix(-1 0 0 -1 3 4)"),
            (270, false, "matrix(0 -1 1 0 3 4)"),
            (0, true, "matrix(-1 0 0 1 3 4)"),
            (90, true, "matrix(0 -1 -1 0 3 4)"),
        ] {
            assert_eq!(placement_transform(&Placement { at: [3, 4], rot, mirror }), m, "rot {rot} mirror {mirror}");
        }
        // The matrix maps the local point (1,2) like to_world does.
        let pl = Placement { at: [3, 4], rot: 90, mirror: true };
        let w = to_world(&pl, [1.0, 2.0]);
        // matrix(0 -1 -1 0 3 4): x' = 0*1 + -1*2 + 3 = 1, y' = -1*1 + 0*2 + 4 = 3
        assert_eq!(w, [1.0, 3.0]);
    }

    #[test]
    fn jumpers_are_round_capped_and_coloured_per_net() {
        let svg = render_bb(BB);
        let j = |id: &str| {
            let i = svg.find(&format!("<polyline id=\"{id}\"")).unwrap();
            &svg[i..i + svg[i..].find("/>").unwrap()]
        };
        assert!(j("j1").contains("class=\"jumper\"") && j("j1").contains("stroke-linecap=\"round\""));
        assert!(j("j1").contains("points=\"0,0 0,3 4,3\""));
        // Own colour wins; the others get distinct preset colours by net (red, blue, green...).
        assert!(j("j2").contains("stroke=\"#ff8800\""));
        assert!(j("j1").contains("stroke=\"#d93025\""), "first net is red: {}", j("j1"));
        assert!(j("j3").contains("stroke=\"#1e8e3e\""), "third net is green (j2 took blue): {}", j("j3"));
    }

    #[test]
    fn jumper_colour_follows_theme_wire_and_none() {
        let svg = render_bb(&BB.replace(r#""title":"bb","#, r##""title":"bb","theme":{"wire":"gold"},"##));
        assert!(svg.contains("id=\"j1\" class=\"jumper\" points=\"0,0 0,3 4,3\" fill=\"none\" stroke=\"#d4a017\""));
        let svg = render_bb(&BB.replace(r##""color":"#ff8800""##, r##""color":"none""##));
        assert!(!svg.contains("id=\"j2\""));
    }

    #[test]
    fn unsafe_or_broken_artwork_is_skipped_but_shapes_remain() {
        let bad = r#"{"format":"short-circ/2","lib":[{"type":"x","pins":[{"id":"a"}],"breadboard":{"size":[2,1],
            "pins":{"a":{"at":[0,0]}},"svg":"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
            "shapes":[{"k":"circle","at":[1,0],"r":0.4}]}}],"parts":[{"id":"X1","type":"x","breadboard":{"at":[0,0]}}]}"#;
        let svg = render_bb(bad);
        assert_balanced(&svg);
        assert!(!svg.contains("script") && !svg.contains("class=\"art\""));
        assert!(svg.contains("<circle cx=\"1\" cy=\"0\" r=\"0.4\""));
        let broken = bad.replace("<script>alert(1)</script></svg>", "<g></svg>");
        assert!(!render_bb(&broken).contains("class=\"art\""));
        let handler = bad.replace("<script>alert(1)</script>", "<rect onload=\\\"x()\\\"/>");
        assert!(!render_bb(&handler).contains("class=\"art\""));
    }

    #[test]
    fn artwork_without_viewbox_uses_width_and_height() {
        let src = r#"{"format":"short-circ/2","lib":[{"type":"x","pins":[{"id":"a"}],"breadboard":{"size":[2,1],
            "pins":{"a":{"at":[0,0]}},"svg":"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40px\" height=\"20\"><circle r=\"3\"/></svg>"}}],
            "parts":[{"id":"X1","type":"x","breadboard":{"at":[0,0]}}]}"#;
        assert!(render_bb(src).contains("viewBox=\"0 0 40 20\""));
    }

    #[test]
    fn unknown_and_viewless_parts_get_placeholders() {
        let svg = render_bb(
            r#"{"format":"short-circ/2","parts":[{"id":"X1","type":"nope","breadboard":{"at":[0,0]}},
            {"id":"V1","type":"vcc","breadboard":{"at":[8,0]}}]}"#,
        );
        assert_balanced(&svg);
        assert!(svg.contains("X1: nope?"));
        assert!(svg.contains("V1: no breadboard view") || svg.contains("class=\"part\" data-type=\"vcc\""));
    }

    #[test]
    fn views_draw_only_their_own_parts_and_wires() {
        let c = parse_circuit(BB).unwrap();
        let lib = Library::load(&c, None);
        let s = render_svg(&c, &lib, false, View::Schematic);
        assert!(s.contains("id=\"s1\"") && !s.contains("id=\"j1\"") && !s.contains("id=\"R1\""));
    }

    #[test]
    fn render_part_svg_labels_pins_and_outlines_the_box() {
        let d = crate::builtin::generate("dip8").unwrap();
        let svg = render_part_svg(&d, View::Schematic, false).unwrap();
        assert_balanced(&svg);
        assert_eq!(svg.matches("class=\"label\"").count(), 8, "one label per pin");
        assert!(svg.contains(">8</text>"));
        assert!(svg.contains("dip8 - DIP-8 (ic), 6x8, 8 pin(s)"));
        assert!(svg.contains("stroke=\"#8a8a8a\""), "bbox outline");
        let ground = crate::Library::builtin().get("ground").unwrap().clone();
        assert!(render_part_svg(&ground, View::Breadboard, false).unwrap_err().contains("no breadboard view"));
        // The input is not modified.
        assert_eq!(d.schematic.as_ref().unwrap().shapes.len(), crate::builtin::generate("dip8").unwrap().schematic.unwrap().shapes.len());
    }

    #[test]
    fn render_part_svg_breadboard_shows_artwork_and_captions() {
        let c = parse_circuit(BB).unwrap();
        let strip = c.lib.iter().find(|d| d.ty == "strip").unwrap();
        let svg = render_part_svg(strip, View::Breadboard, true).unwrap();
        assert_balanced(&svg);
        assert!(svg.contains("class=\"art\"") && svg.contains(">h1</text>") && svg.contains(">h2</text>"));
        assert!(svg.contains("strip (board), 2x1, 2 pin(s)"));
    }

    #[test]
    fn artwork_document_stretches_the_source_over_size() {
        let src = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 10"><rect width="40" height="10"/></svg>"#;
        let doc = artwork_document(src, [4.0, 1.0]).unwrap();
        assert!(doc.contains("width=\"4\" height=\"1\" viewBox=\"0 0 4 1\""), "{doc}");
        assert!(doc.contains("viewBox=\"0 0 40 10\" preserveAspectRatio=\"none\""), "{doc}");
        assert!(doc.contains("<rect"));
        assert!(artwork_document("<svg xmlns=\"http://www.w3.org/2000/svg\"><script/></svg>", [1.0, 1.0]).is_none());
        assert!(artwork_document("not xml", [1.0, 1.0]).is_none());
    }

    #[test]
    fn pin_captions() {
        assert_eq!(pin_caption("1", None), "1");
        assert_eq!(pin_caption("1", Some("VCC")), "1:VCC");
        assert_eq!(pin_caption("a", Some("a")), "a");
        assert_eq!(pin_caption("a", Some(" ")), "a");
    }
}
