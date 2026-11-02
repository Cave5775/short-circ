//! SVG subset reader (on `roxmltree`) that turns artwork into a [`PartDef`].
//!
//! Supported elements: `line`, `rect` (with `rx`/`ry`), `circle`, `ellipse` (a circle when
//! `rx == ry` and the transform is a uniform scale, otherwise a 32-gon), `polyline`,
//! `polygon`, `path` (commands `M L H V C S Q T A Z`, absolute and relative; curves are
//! flattened to polylines), `text`, `use` (references to any element by id), and
//! `g`/`svg`/`a` containers. `transform` attributes (`translate`, `scale`, `rotate`, `matrix`,
//! `skewX`, `skewY`) compose down the tree. Styling comes from presentation attributes, `<style>`
//! rules (type, `.class`, `#id` selectors, comma lists, last compound of descendant selectors)
//! and `style=""`, in that order of precedence, inherited down groups. `defs`, `clipPath`,
//! `mask`, `symbol`, `style`, `title`, ... and `display:none` subtrees are not drawn.
//! [`import_svg`] ignores root `width`/`height`/`viewBox` (user units are pixels); [`read_scene`]
//! in physical mode honours them (units in/mm/cm/pt/pc/px) and also reports the bounding box of
//! every element with an `id`, which the Fritzing importer uses to place pins.
//!
//! Styling (decisions for schematic use, not full SVG semantics): a missing `fill` means no
//! fill (SVG would say black); a missing or black `stroke` means "theme foreground" (the
//! `Style.stroke` field is left `None`); a black fill is kept as `#000000`. Any other colour
//! string is passed through. `stroke-width` is scaled like geometry.
//!
//! Scaling: coordinates are divided by `px_per_unit` (10 by default, i.e. 10 px = 1 grid
//! unit), the geometry bounding box (text anchors excluded unless nothing else exists) is
//! translated to start at (0,0), and `size` is its ceiling. Pins are supplied by the caller in
//! the *resulting* part-local grid coordinates; `size` grows to include them if necessary and
//! `dir` is inferred from the nearest bbox edge.

use crate::model::{Anchor, Dir, PartDef, PinDef, Shape, Style, FORMAT};
use std::collections::BTreeMap;

pub const DEFAULT_PX_PER_UNIT: f64 = 10.0;

/// Import with the default scale of 10 px per grid unit.
pub fn import_svg(svg: &str, ty: &str, pins: &[(String, i32, i32)]) -> Result<PartDef, String> {
    import_svg_scaled(svg, ty, pins, DEFAULT_PX_PER_UNIT)
}

/// Import with an explicit `px_per_unit` (> 0).
pub fn import_svg_scaled(
    svg: &str,
    ty: &str,
    pins: &[(String, i32, i32)],
    px_per_unit: f64,
) -> Result<PartDef, String> {
    if !(px_per_unit.is_finite() && px_per_unit > 0.0) {
        return Err("scale (px per grid unit) must be a positive number".into());
    }
    if ty.trim().is_empty() {
        return Err("part type name must not be empty".into());
    }
    let raw = read_scene(svg, false)?.shapes;
    if raw.is_empty() {
        return Err("no drawable shapes found in SVG".into());
    }

    // Bounding box in px over geometry; text anchors only when there is nothing else.
    let mut bb = BBox::new();
    for s in &raw {
        bb.add_shape(s, false);
    }
    if bb.is_empty() {
        for s in &raw {
            bb.add_shape(s, true);
        }
    }
    let (minx, miny) = (bb.min[0], bb.min[1]);
    let k = 1.0 / px_per_unit;
    let map = |p: [f64; 2]| [r3((p[0] - minx) * k), r3((p[1] - miny) * k)];
    let shapes: Vec<Shape> = raw.into_iter().map(|s| map_shape(s, &map, k)).collect();

    let mut w = r3((bb.max[0] - minx) * k).ceil().max(1.0);
    let mut h = r3((bb.max[1] - miny) * k).ceil().max(1.0);
    let mut out_pins: Vec<PinDef> = Vec::new();
    for (id, x, y) in pins {
        if out_pins.iter().any(|p| &p.id == id) {
            return Err(format!("duplicate pin id {id}"));
        }
        w = w.max(*x as f64);
        h = h.max(*y as f64);
        out_pins.push(PinDef { id: id.clone(), at: [*x, *y], dir: Dir::Left, name: None });
    }
    for p in &mut out_pins {
        p.dir = infer_dir(p.at, w, h);
    }

    Ok(PartDef {
        format: FORMAT.to_string(),
        ty: ty.to_string(),
        name: ty.to_string(),
        category: Some("imported".to_string()),
        size: [w, h],
        pins: out_pins,
        shapes,
        params: None,
        svg: None,
        meta: None,
    })
}

/// Convert SVG path data (`d` attribute) into shapes, coordinates divided by `px_per_unit`
/// and otherwise untransformed. Closed subpaths become `Poly`, open ones `Line`. Used for
/// `PartDef.svg` fallbacks.
pub fn path_to_shapes(d: &str, px_per_unit: f64) -> Result<Vec<Shape>, String> {
    let k = if px_per_unit > 0.0 { 1.0 / px_per_unit } else { 1.0 };
    let mut out = Vec::new();
    for sub in parse_path(d)? {
        let pts: Vec<[f64; 2]> = sub.pts.iter().map(|p| [r3(p[0] * k), r3(p[1] * k)]).collect();
        if sub.closed && pts.len() >= 3 {
            out.push(Shape::Poly { pts, style: Style::default() });
        } else if pts.len() >= 2 {
            out.push(Shape::Line { pts, style: Style::default() });
        }
    }
    Ok(out)
}

fn r3(v: f64) -> f64 {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// Direction a pin at `at` leads out of a `w` x `h` box: toward the nearest edge.
pub fn infer_dir(at: [i32; 2], w: f64, h: f64) -> Dir {
    let (x, y) = (at[0] as f64, at[1] as f64);
    let cands = [(x, Dir::Left), (w - x, Dir::Right), (y, Dir::Up), (h - y, Dir::Down)];
    let mut best = cands[0];
    for c in cands {
        if c.0 < best.0 {
            best = c;
        }
    }
    best.1
}

// ---------------------------------------------------------------------------------------
// Bounding box and shape mapping

struct BBox {
    min: [f64; 2],
    max: [f64; 2],
}

impl BBox {
    fn new() -> Self {
        BBox { min: [f64::INFINITY; 2], max: [f64::NEG_INFINITY; 2] }
    }
    fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }
    fn add(&mut self, p: [f64; 2]) {
        self.min = [self.min[0].min(p[0]), self.min[1].min(p[1])];
        self.max = [self.max[0].max(p[0]), self.max[1].max(p[1])];
    }
    fn add_shape(&mut self, s: &Shape, with_text: bool) {
        match s {
            Shape::Line { pts, .. } | Shape::Poly { pts, .. } => pts.iter().for_each(|p| self.add(*p)),
            Shape::Rect { at, size, .. } => {
                self.add(*at);
                self.add([at[0] + size[0], at[1] + size[1]]);
            }
            Shape::Circle { at, r, .. } | Shape::Arc { at, r, .. } => {
                self.add([at[0] - r, at[1] - r]);
                self.add([at[0] + r, at[1] + r]);
            }
            Shape::Text { at, .. } => {
                if with_text {
                    self.add(*at)
                }
            }
        }
    }
}

fn scale_style(mut st: Style, k: f64) -> Style {
    st.width = st.width.map(|w| r3(w * k));
    st
}

fn map_shape(s: Shape, f: &dyn Fn([f64; 2]) -> [f64; 2], k: f64) -> Shape {
    match s {
        Shape::Line { pts, style } => {
            Shape::Line { pts: pts.into_iter().map(f).collect(), style: scale_style(style, k) }
        }
        Shape::Poly { pts, style } => {
            Shape::Poly { pts: pts.into_iter().map(f).collect(), style: scale_style(style, k) }
        }
        Shape::Rect { at, size, r, style } => Shape::Rect {
            at: f(at),
            size: [r3(size[0] * k), r3(size[1] * k)],
            r: r3(r * k),
            style: scale_style(style, k),
        },
        Shape::Circle { at, r, style } => {
            Shape::Circle { at: f(at), r: r3(r * k), style: scale_style(style, k) }
        }
        Shape::Arc { at, r, from, to, style } => {
            Shape::Arc { at: f(at), r: r3(r * k), from, to, style: scale_style(style, k) }
        }
        Shape::Text { at, text, size, anchor, style } => {
            Shape::Text { at: f(at), text, size: r3(size * k), anchor, style }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Affine matrices: [a b c d e f], x' = a x + c y + e, y' = b x + d y + f

type M = [f64; 6];
const IDENT: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(p: &M, c: &M) -> M {
    [
        p[0] * c[0] + p[2] * c[1],
        p[1] * c[0] + p[3] * c[1],
        p[0] * c[2] + p[2] * c[3],
        p[1] * c[2] + p[3] * c[3],
        p[0] * c[4] + p[2] * c[5] + p[4],
        p[1] * c[4] + p[3] * c[5] + p[5],
    ]
}

fn apply(m: &M, p: [f64; 2]) -> [f64; 2] {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

fn det_scale(m: &M) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
}

fn axis_aligned(m: &M) -> bool {
    m[1].abs() < 1e-9 && m[2].abs() < 1e-9
}

fn uniform(m: &M) -> bool {
    axis_aligned(m) && (m[0].abs() - m[3].abs()).abs() < 1e-9 * m[0].abs().max(1.0)
}

fn parse_transform(s: &str) -> M {
    let mut m = IDENT;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let a = numbers(&rest[open + 1..open + close]);
        let g = |i: usize, d: f64| a.get(i).copied().unwrap_or(d);
        let t: M = match (name, a.len()) {
            ("translate", n) if n >= 1 => [1.0, 0.0, 0.0, 1.0, a[0], g(1, 0.0)],
            ("scale", n) if n >= 1 => [a[0], 0.0, 0.0, g(1, a[0]), 0.0, 0.0],
            ("rotate", n) if n >= 1 => {
                let (sn, cs) = a[0].to_radians().sin_cos();
                let r: M = [cs, sn, -sn, cs, 0.0, 0.0];
                if n >= 3 {
                    let to: M = [1.0, 0.0, 0.0, 1.0, a[1], a[2]];
                    let back: M = [1.0, 0.0, 0.0, 1.0, -a[1], -a[2]];
                    mul(&mul(&to, &r), &back)
                } else {
                    r
                }
            }
            ("matrix", 6) => [a[0], a[1], a[2], a[3], a[4], a[5]],
            ("skewX", n) if n >= 1 => [1.0, 0.0, a[0].to_radians().tan(), 1.0, 0.0, 0.0],
            ("skewY", n) if n >= 1 => [1.0, a[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => IDENT,
        };
        m = mul(&m, &t);
        rest = &rest[open + close + 1..];
    }
    m
}

// ---------------------------------------------------------------------------------------
// Number scanner

struct Sc<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Sc<'a> {
    fn new(s: &'a str) -> Self {
        Sc { b: s.as_bytes(), i: 0 }
    }
    fn skip_sep(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',') {
            self.i += 1;
        }
    }
    fn at_end(&mut self) -> bool {
        self.skip_sep();
        self.i >= self.b.len()
    }
    fn number(&mut self) -> Option<f64> {
        self.skip_sep();
        let b = self.b;
        let s = self.i;
        let mut j = s;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let mut digits = false;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
            digits = true;
        }
        if j < b.len() && b[j] == b'.' {
            j += 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
                digits = true;
            }
        }
        if !digits {
            return None;
        }
        if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
            let mut k = j + 1;
            if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                k += 1;
            }
            if k < b.len() && b[k].is_ascii_digit() {
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                j = k;
            }
        }
        let v = std::str::from_utf8(&b[s..j]).ok()?.parse::<f64>().ok()?;
        self.i = j;
        Some(v)
    }
    /// Arc flag: a single `0` or `1`, possibly glued to the next number.
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        match self.b.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Some(false)
            }
            Some(b'1') => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
}

fn numbers(s: &str) -> Vec<f64> {
    let mut sc = Sc::new(s);
    let mut v = Vec::new();
    while let Some(n) = sc.number() {
        v.push(n);
    }
    v
}

/// Leading number of a length such as `12`, `3.5px`, `2mm` (the unit is ignored).
fn length(s: &str) -> Option<f64> {
    Sc::new(s).number()
}

// ---------------------------------------------------------------------------------------
// Path data

struct Sub {
    pts: Vec<[f64; 2]>,
    closed: bool,
}

fn parse_path(d: &str) -> Result<Vec<Sub>, String> {
    let mut sc = Sc::new(d);
    let mut subs: Vec<Sub> = Vec::new();
    let mut cur_sub: Option<Sub> = None;
    let mut cur = [0.0f64, 0.0];
    let mut start = [0.0f64, 0.0];
    let mut last_c: Option<[f64; 2]> = None;
    let mut last_q: Option<[f64; 2]> = None;
    let mut cmd: u8 = 0;
    let err = |c: u8| format!("bad path data near command '{}'", c as char);

    macro_rules! num {
        () => {
            sc.number().ok_or_else(|| err(cmd))?
        };
    }

    while !sc.at_end() {
        let c = sc.b[sc.i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            sc.i += 1;
        } else if cmd == 0 {
            return Err("path data must start with a command".into());
        } else if cmd == b'M' {
            cmd = b'L';
        } else if cmd == b'm' {
            cmd = b'l';
        } else if cmd == b'z' || cmd == b'Z' {
            return Err(err(cmd));
        }
        let rel = cmd.is_ascii_lowercase();
        let prev_c = last_c.take();
        let prev_q = last_q.take();
        let (ox, oy) = if rel { (cur[0], cur[1]) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = [num!() + ox, num!() + oy];
                if let Some(s) = cur_sub.take() {
                    subs.push(s);
                }
                cur_sub = Some(Sub { pts: vec![p], closed: false });
                cur = p;
                start = p;
            }
            b'Z' => {
                if let Some(mut s) = cur_sub.take() {
                    s.closed = true;
                    subs.push(s);
                }
                cur = start;
            }
            up @ (b'L' | b'H' | b'V' | b'C' | b'S' | b'Q' | b'T' | b'A') => {
                let from = cur;
                let sub = cur_sub.get_or_insert_with(|| Sub { pts: vec![from], closed: false });
                match up {
                    b'L' => {
                        cur = [num!() + ox, num!() + oy];
                        sub.pts.push(cur);
                    }
                    b'H' => {
                        cur = [num!() + ox, cur[1]];
                        sub.pts.push(cur);
                    }
                    b'V' => {
                        cur = [cur[0], num!() + oy];
                        sub.pts.push(cur);
                    }
                    b'C' | b'S' => {
                        let c1 = if up == b'C' {
                            [num!() + ox, num!() + oy]
                        } else {
                            prev_c.map_or(cur, |c| [2.0 * cur[0] - c[0], 2.0 * cur[1] - c[1]])
                        };
                        let c2 = [num!() + ox, num!() + oy];
                        let p = [num!() + ox, num!() + oy];
                        for i in 1..=16 {
                            let t = i as f64 / 16.0;
                            let u = 1.0 - t;
                            let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                            sub.pts.push([
                                a * cur[0] + b * c1[0] + c * c2[0] + e * p[0],
                                a * cur[1] + b * c1[1] + c * c2[1] + e * p[1],
                            ]);
                        }
                        last_c = Some(c2);
                        cur = p;
                    }
                    b'Q' | b'T' => {
                        let c1 = if up == b'Q' {
                            [num!() + ox, num!() + oy]
                        } else {
                            prev_q.map_or(cur, |c| [2.0 * cur[0] - c[0], 2.0 * cur[1] - c[1]])
                        };
                        let p = [num!() + ox, num!() + oy];
                        for i in 1..=12 {
                            let t = i as f64 / 12.0;
                            let u = 1.0 - t;
                            let (a, b, c) = (u * u, 2.0 * u * t, t * t);
                            sub.pts.push([
                                a * cur[0] + b * c1[0] + c * p[0],
                                a * cur[1] + b * c1[1] + c * p[1],
                            ]);
                        }
                        last_q = Some(c1);
                        cur = p;
                    }
                    _ => {
                        let (rx, ry, rot) = (num!(), num!(), num!());
                        let fa = sc.flag().ok_or_else(|| err(cmd))?;
                        let fs = sc.flag().ok_or_else(|| err(cmd))?;
                        let p = [num!() + ox, num!() + oy];
                        sub.pts.extend(arc_points(cur, rx, ry, rot, fa, fs, p));
                        cur = p;
                    }
                }
            }
            _ => return Err(err(cmd)),
        }
    }
    if let Some(s) = cur_sub.take() {
        subs.push(s);
    }
    Ok(subs)
}

/// SVG endpoint arc -> polyline points (start excluded, end included). Appendix F.6.5.
fn arc_points(p1: [f64; 2], rx: f64, ry: f64, phi_deg: f64, fa: bool, fs: bool, p2: [f64; 2]) -> Vec<[f64; 2]> {
    if p1 == p2 {
        return vec![];
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        return vec![p2];
    }
    let (sp, cp) = phi_deg.to_radians().sin_cos();
    let (dx, dy) = ((p1[0] - p2[0]) / 2.0, (p1[1] - p2[1]) / 2.0);
    let x1p = cp * dx + sp * dy;
    let y1p = -sp * dx + cp * dy;
    let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let sign = if fa == fs { -1.0 } else { 1.0 };
    let coef = if den == 0.0 { 0.0 } else { sign * (num / den).max(0.0).sqrt() };
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cp * cxp - sp * cyp + (p1[0] + p2[0]) / 2.0;
    let cy = sp * cxp + cp * cyp + (p1[1] + p2[1]) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let (ux, uy) = ((x1p - cxp) / rx, (y1p - cyp) / ry);
    let (vx, vy) = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let th1 = ang(1.0, 0.0, ux, uy);
    let mut dth = ang(ux, uy, vx, vy);
    if !fs && dth > 0.0 {
        dth -= std::f64::consts::TAU;
    } else if fs && dth < 0.0 {
        dth += std::f64::consts::TAU;
    }
    let steps = ((dth.abs() / (std::f64::consts::PI / 16.0)).ceil() as usize).max(4);
    let mut out = Vec::with_capacity(steps);
    for i in 1..=steps {
        if i == steps {
            out.push(p2);
            break;
        }
        let t = th1 + dth * i as f64 / steps as f64;
        let (st, ct) = t.sin_cos();
        out.push([cp * rx * ct - sp * ry * st + cx, sp * rx * ct + cp * ry * st + cy]);
    }
    out
}
// ---------------------------------------------------------------------------------------
// XML (roxmltree), CSS and units

type Props = Vec<(String, String)>;

fn get<'a>(props: &'a [(String, String)], k: &str) -> Option<&'a str> {
    props.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn fnum(props: &[(String, String)], k: &str) -> f64 {
    get(props, k).and_then(length).unwrap_or(0.0)
}

/// Bounding box `[minx, miny, maxx, maxy]`.
pub type Bb = [f64; 4];

fn bb_union(a: Option<Bb>, b: Option<Bb>) -> Option<Bb> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (a, None) => a,
        (None, b) => b,
    }
}

fn shape_bb(s: &Shape) -> Option<Bb> {
    let mut b = BBox::new();
    b.add_shape(s, false);
    if b.is_empty() {
        None
    } else {
        Some([b.min[0], b.min[1], b.max[0], b.max[1]])
    }
}

/// Bounding box of the geometry of `shapes` (text excluded).
pub fn shapes_bbox(shapes: &[Shape]) -> Option<Bb> {
    shapes.iter().fold(None, |acc, s| bb_union(acc, shape_bb(s)))
}

/// Move a shape by `(dx, dy)`.
pub fn translate_shape(s: Shape, dx: f64, dy: f64) -> Shape {
    map_shape(s, &|p| [r3(p[0] + dx), r3(p[1] + dy)], 1.0)
}

/// A selector reduced to its last compound: optional tag, optional id, classes.
struct Sel {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

struct CssRule {
    sel: Sel,
    spec: (usize, usize, usize),
    order: usize,
    decls: Props,
}

impl Sel {
    fn matches(&self, name: &str, id: Option<&str>, classes: &[&str]) -> bool {
        self.tag.as_deref().is_none_or(|t| t == name)
            && self.id.as_deref().is_none_or(|i| Some(i) == id)
            && self.classes.iter().all(|c| classes.contains(&c.as_str()))
    }
}

fn parse_compound(s: &str) -> Option<Sel> {
    if s.contains(':') || s.contains('[') || s.contains('*') && s.len() > 1 {
        return None;
    }
    let mut sel = Sel { tag: None, id: None, classes: Vec::new() };
    let mut cur = String::new();
    let mut kind = 't';
    let flush = |kind: char, cur: &mut String, sel: &mut Sel| {
        if !cur.is_empty() {
            match kind {
                't' if cur != "*" => sel.tag = Some(cur.clone()),
                '#' => sel.id = Some(cur.clone()),
                '.' => sel.classes.push(cur.clone()),
                _ => {}
            }
            cur.clear();
        }
    };
    for ch in s.chars() {
        if ch == '.' || ch == '#' {
            flush(kind, &mut cur, &mut sel);
            kind = ch;
        } else {
            cur.push(ch);
        }
    }
    flush(kind, &mut cur, &mut sel);
    Some(sel)
}

fn parse_decls(body: &str) -> Props {
    let mut out = Vec::new();
    for d in body.split(';') {
        if let Some((k, v)) = d.split_once(':') {
            let v = v.replace("!important", "");
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    out
}

/// Read `selector { decl; ... }` rules (class, id and type selectors, comma lists; the last
/// compound of a descendant selector is used; pseudo classes and attribute selectors are
/// ignored). `@media` and similar blocks are read as if they always applied.
fn parse_css(css: &str, rules: &mut Vec<CssRule>) {
    let mut s = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        s.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    s.push_str(rest);
    let mut rest = s.as_str();
    while let Some(open) = rest.find('{') {
        let head = rest[..open].trim();
        let mut depth = 1;
        let mut j = open + 1;
        let b = rest.as_bytes();
        while j < b.len() && depth > 0 {
            match b[j] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        let body = &rest[open + 1..j.saturating_sub(1).max(open + 1)];
        rest = &rest[j.min(rest.len())..];
        if head.starts_with('@') {
            parse_css(body, rules);
            continue;
        }
        let decls = parse_decls(body);
        for part in head.split(',') {
            let last = part.split(|c: char| c.is_whitespace() || c == '>' || c == '+' || c == '~').rfind(|t| !t.is_empty());
            let Some(sel) = last.and_then(parse_compound) else { continue };
            let spec = (sel.id.iter().count(), sel.classes.len(), sel.tag.iter().count());
            rules.push(CssRule { sel, spec, order: rules.len(), decls: decls.clone() });
        }
    }
}

/// Add `xmlns:prefix` declarations for undeclared prefixes (some exports omit `xlink`).
fn fix_namespaces(svg: &str) -> String {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let mut src = svg.trim_start_matches('\u{feff}').to_string();
    let mut added: Vec<String> = Vec::new();
    for _ in 0..8 {
        let err = roxmltree::Document::parse_with_options(&src, opts).err();
        let Some(roxmltree::Error::UnknownNamespace(prefix, _)) = err else { break };
        if added.contains(&prefix) {
            break;
        }
        let Some(i) = src.find("<svg") else { break };
        src.insert_str(i + 4, &format!(" xmlns:{prefix}=\"urn:short-circ:{prefix}\""));
        added.push(prefix);
    }
    src
}

/// Result of [`read_scene`].
#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// Drawn shapes: pixels in raw mode, grid units (0.1 in) in physical mode.
    pub shapes: Vec<Shape>,
    /// Bounding box (same units) of every element with an `id`, groups included.
    pub ids: BTreeMap<String, Bb>,
    /// Declared root size in inches, when `width` and `height` are both absolute lengths.
    pub declared_in: Option<[f64; 2]>,
    /// Dots per inch assumed for unit-less lengths (physical mode).
    pub dpi: f64,
    pub warnings: Vec<String>,
}

/// Parse an SVG document. In raw mode root `width`/`height`/`viewBox` are ignored and user
/// units are returned as they are. In physical mode the root size is honoured (units `in`,
/// `mm`, `cm`, `pt`, `pc`, `px`; unit-less lengths use 72 dpi for Illustrator files, 96 for
/// Inkscape 0.92+ and 90 otherwise), the result is in grid units, and SVG defaults apply
/// (missing fill is black, missing stroke is none).
pub fn read_scene(svg: &str, physical: bool) -> Result<Scene, String> {
    let src = fix_namespaces(svg);
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(&src, opts).map_err(|e| format!("invalid SVG: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err("not an SVG document (no <svg> element)".into());
    }
    let mut rules = Vec::new();
    for st in doc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "style") {
        let css: String = st.children().filter_map(|c| c.text()).collect();
        parse_css(&css, &mut rules);
    }
    let mut ids = std::collections::HashMap::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        if let Some(id) = n.attribute("id") {
            ids.entry(id).or_insert(n);
        }
    }
    let dpi = detect_dpi(svg);
    let (ctm, declared_in) = if physical { root_matrix(root, dpi) } else { (IDENT, None) };
    let inh = if physical {
        Inh {
            stroke: Some("none".into()),
            fill: Some("#000000".into()),
            width: Some(1.0),
            font: 16.0,
            anchor: Anchor::Start,
            visible: true,
        }
    } else {
        Inh { stroke: None, fill: None, width: None, font: 10.0, anchor: Anchor::Start, visible: true }
    };
    let mut w = Walker {
        ids,
        rules,
        out: Vec::new(),
        bbs: BTreeMap::new(),
        budget: 200_000,
        warnings: Vec::new(),
    };
    w.walk(root, &Ctx { ctm, inh, hidden: false }, 0, false);
    let mut shapes = w.out;
    if physical {
        // Flattened curves are far finer than the 0.1 in grid needs (tolerance 0.005 unit).
        for s in &mut shapes {
            let (pts, min) = match s {
                Shape::Line { pts, .. } => (pts, 2),
                Shape::Poly { pts, .. } => (pts, 3),
                _ => continue,
            };
            let simple = simplify(pts, 0.005);
            if simple.len() >= min {
                *pts = simple;
            }
        }
    }
    Ok(Scene { shapes, ids: w.bbs, declared_in, dpi, warnings: w.warnings })
}

/// Douglas-Peucker simplification keeping both end points; consecutive duplicates are dropped.
fn simplify(pts: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    let mut v: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
    for p in pts {
        if v.last().is_none_or(|q| (q[0] - p[0]).hypot(q[1] - p[1]) > 1e-6) {
            v.push(*p);
        }
    }
    if v.len() <= 2 {
        return v;
    }
    let mut keep = vec![false; v.len()];
    keep[0] = true;
    keep[v.len() - 1] = true;
    let mut stack = vec![(0, v.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (v[a], v[b]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len = dx.hypot(dy);
        let mut worst = (0.0, a);
        for (i, p) in v.iter().enumerate().take(b).skip(a + 1) {
            let d = if len < 1e-12 {
                (p[0] - pa[0]).hypot(p[1] - pa[1])
            } else {
                ((p[0] - pa[0]) * dy - (p[1] - pa[1]) * dx).abs() / len
            };
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    v.into_iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| p).collect()
}

fn detect_dpi(svg: &str) -> f64 {
    let mut end = svg.len().min(3000);
    while !svg.is_char_boundary(end) {
        end -= 1;
    }
    let head = &svg[..end];
    if head.contains("Illustrator") {
        return 72.0;
    }
    if let Some(i) = head.find("inkscape:version=\"") {
        let v = &head[i + 18..];
        let mut it = v.split(|c: char| !c.is_ascii_digit() && c != '.').next().unwrap_or("").split('.');
        let major: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if major > 0 || minor >= 92 {
            return 96.0;
        }
    }
    90.0
}

/// A length in inches; unit-less and `px` use `dpi`. Percentages and font-relative units
/// give `None`.
fn length_in(s: &str, dpi: f64) -> Option<f64> {
    let mut sc = Sc::new(s);
    let v = sc.number()?;
    match s[sc.i..].trim() {
        "" | "px" => Some(v / dpi),
        "in" => Some(v),
        "mm" => Some(v / 25.4),
        "cm" => Some(v / 2.54),
        "pt" => Some(v / 72.0),
        "pc" => Some(v / 6.0),
        _ => None,
    }
}

/// Matrix from root user units to grid units, and the declared size in inches.
fn root_matrix(root: roxmltree::Node, dpi: f64) -> (M, Option<[f64; 2]>) {
    let w = root.attribute("width").and_then(|s| length_in(s, dpi));
    let h = root.attribute("height").and_then(|s| length_in(s, dpi));
    let vb = root
        .attribute("viewBox")
        .map(numbers)
        .filter(|v| v.len() == 4 && v[2] > 0.0 && v[3] > 0.0);
    let fallback = 10.0 / dpi;
    let (s, org) = match &vb {
        Some(v) => {
            let s = match (w.map(|w| w * 10.0 / v[2]), h.map(|h| h * 10.0 / v[3])) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => fallback,
            };
            (s, [v[0], v[1]])
        }
        None => (fallback, [0.0, 0.0]),
    };
    let declared = match (w, h) {
        (Some(w), Some(h)) => Some([w, h]),
        _ => None,
    };
    ([s, 0.0, 0.0, s, -s * org[0], -s * org[1]], declared)
}

// ---------------------------------------------------------------------------------------
// Tree walk

#[derive(Clone)]
struct Inh {
    stroke: Option<String>,
    fill: Option<String>,
    width: Option<f64>,
    font: f64,
    anchor: Anchor,
    visible: bool,
}

#[derive(Clone)]
struct Ctx {
    ctm: M,
    inh: Inh,
    hidden: bool,
}

const SKIP: &[&str] = &[
    "defs", "clipPath", "mask", "symbol", "style", "title", "desc", "metadata", "marker", "pattern",
    "linearGradient", "radialGradient", "filter", "script", "foreignObject", "namedview",
];

const NAMED_COLORS: &[(&str, &str)] = &[
    ("black", "#000000"),
    ("white", "#ffffff"),
    ("red", "#ff0000"),
    ("green", "#008000"),
    ("lime", "#00ff00"),
    ("blue", "#0000ff"),
    ("yellow", "#ffff00"),
    ("cyan", "#00ffff"),
    ("aqua", "#00ffff"),
    ("magenta", "#ff00ff"),
    ("fuchsia", "#ff00ff"),
    ("gray", "#808080"),
    ("grey", "#808080"),
    ("silver", "#c0c0c0"),
    ("maroon", "#800000"),
    ("olive", "#808000"),
    ("navy", "#000080"),
    ("purple", "#800080"),
    ("teal", "#008080"),
    ("orange", "#ffa500"),
    ("brown", "#a52a2a"),
    ("pink", "#ffc0cb"),
    ("gold", "#ffd700"),
    ("darkgray", "#a9a9a9"),
    ("darkgrey", "#a9a9a9"),
    ("lightgray", "#d3d3d3"),
    ("lightgrey", "#d3d3d3"),
];

/// Normalise a CSS colour to lowercase `#rrggbb` when it is a hex colour, `rgb()` triple or
/// common colour name; anything else is returned unchanged.
pub fn norm_color(c: &str) -> String {
    let t = c.trim();
    let l = t.to_ascii_lowercase();
    if let Some(h) = l.strip_prefix('#') {
        if h.chars().all(|x| x.is_ascii_hexdigit()) {
            match h.len() {
                6 => return l,
                3 => return format!("#{}", h.chars().flat_map(|x| [x, x]).collect::<String>()),
                _ => {}
            }
        }
        return l;
    }
    if let Some(args) = l.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        if parts.len() == 3 {
            let ch = |p: &str| -> Option<u8> {
                match p.strip_suffix('%') {
                    Some(n) => n.trim().parse::<f64>().ok().map(|v| (v * 2.55).round().clamp(0.0, 255.0) as u8),
                    None => p.parse::<f64>().ok().map(|v| v.round().clamp(0.0, 255.0) as u8),
                }
            };
            if let (Some(r), Some(g), Some(b)) = (ch(parts[0]), ch(parts[1]), ch(parts[2])) {
                return format!("#{r:02x}{g:02x}{b:02x}");
            }
        }
    }
    NAMED_COLORS.iter().find(|(n, _)| *n == l).map_or_else(|| t.to_string(), |(_, h)| h.to_string())
}

fn is_black(c: &str) -> bool {
    norm_color(c) == "#000000"
}

/// Style for an emitted shape, and whether it would be visible at all.
fn make_style(inh: &Inh, scale: f64, closed: bool) -> (Style, bool) {
    let stroke_none = inh.stroke.as_deref().is_some_and(|s| s == "none" || s.starts_with("url("));
    let stroke = match inh.stroke.as_deref() {
        None => None,
        Some(_) if stroke_none => Some("none".to_string()),
        Some(c) if is_black(c) || c.eq_ignore_ascii_case("currentcolor") => None,
        Some(c) => Some(norm_color(c)),
    };
    let fill = if closed {
        match inh.fill.as_deref() {
            None | Some("none") | Some("transparent") => None,
            Some(c) if c.starts_with("url(") => None,
            Some(c) if c.eq_ignore_ascii_case("currentcolor") => Some("#000000".to_string()),
            Some(c) => Some(norm_color(c)),
        }
    } else {
        None
    };
    let visible = !(stroke_none && fill.is_none());
    let width = if stroke_none { None } else { inh.width.map(|w| w * scale) };
    (Style { stroke, fill, width }, visible)
}

fn update_inh(inh: &mut Inh, props: &[(String, String)]) {
    let clean = |s: &str| s.trim().to_string();
    if let Some(v) = get(props, "stroke").filter(|v| *v != "inherit") {
        inh.stroke = Some(clean(v));
    }
    if let Some(v) = get(props, "fill").filter(|v| *v != "inherit") {
        inh.fill = Some(clean(v));
    }
    let zero = |k: &str| get(props, k).and_then(length).is_some_and(|o| o <= 0.0);
    if zero("fill-opacity") {
        inh.fill = Some("none".into());
    }
    if zero("stroke-opacity") {
        inh.stroke = Some("none".into());
    }
    if let Some(w) = get(props, "stroke-width").and_then(length) {
        inh.width = Some(w);
    }
    if let Some(f) = get(props, "font-size").and_then(length) {
        inh.font = f;
    }
    match get(props, "text-anchor").map(str::trim) {
        Some("middle") => inh.anchor = Anchor::Middle,
        Some("end") => inh.anchor = Anchor::End,
        Some("start") => inh.anchor = Anchor::Start,
        _ => {}
    }
    match get(props, "visibility").map(str::trim) {
        Some("hidden") | Some("collapse") => inh.visible = false,
        Some("visible") => inh.visible = true,
        _ => {}
    }
}

struct Walker<'a, 'i> {
    ids: std::collections::HashMap<&'a str, roxmltree::Node<'a, 'i>>,
    rules: Vec<CssRule>,
    out: Vec<Shape>,
    bbs: BTreeMap<String, Bb>,
    budget: usize,
    warnings: Vec<String>,
}

impl<'a, 'i> Walker<'a, 'i> {
    fn warn(&mut self, msg: &str) {
        if !self.warnings.iter().any(|w| w == msg) {
            self.warnings.push(msg.to_string());
        }
    }

    /// Properties of an element: presentation attributes, then matching stylesheet rules (by
    /// specificity, then order), then the `style` attribute.
    fn props_of(&self, node: roxmltree::Node<'a, 'i>) -> Props {
        let mut props: Props = node
            .attributes()
            .filter(|a| a.namespace().is_none() || a.name() == "href")
            .map(|a| (a.name().to_string(), a.value().to_string()))
            .collect();
        if !self.rules.is_empty() {
            let classes: Vec<&str> = get(&props, "class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
            let mut m: Vec<&CssRule> = self
                .rules
                .iter()
                .filter(|r| r.sel.matches(node.tag_name().name(), node.attribute("id"), &classes))
                .collect();
            m.sort_by_key(|r| (r.spec, r.order));
            let extra: Props = m.iter().flat_map(|r| r.decls.iter().cloned()).collect();
            props.extend(extra);
        }
        if let Some(st) = node.attribute("style") {
            props.extend(parse_decls(st));
        }
        props
    }

    /// Walk one element; returns the bounding box of its drawn geometry (text excluded).
    fn walk(&mut self, node: roxmltree::Node<'a, 'i>, parent: &Ctx, depth: usize, force: bool) -> Option<Bb> {
        if !node.is_element() {
            return None;
        }
        let name = node.tag_name().name();
        if !force && SKIP.contains(&name) {
            return None;
        }
        if depth > 64 || self.budget == 0 {
            return None;
        }
        self.budget -= 1;
        let props = self.props_of(node);
        let mut ctx = parent.clone();
        let opacity0 = get(&props, "opacity").and_then(length).is_some_and(|o| o <= 0.0);
        ctx.hidden = ctx.hidden || opacity0 || get(&props, "display").map(str::trim) == Some("none");
        if let Some(t) = get(&props, "transform") {
            ctx.ctm = mul(&ctx.ctm, &parse_transform(t));
        }
        if name == "svg" && depth > 0 {
            let (x, y) = (fnum(&props, "x"), fnum(&props, "y"));
            let mut t: M = [1.0, 0.0, 0.0, 1.0, x, y];
            let vb = get(&props, "viewBox").map(numbers).filter(|v| v.len() == 4 && v[2] > 0.0 && v[3] > 0.0);
            let (w, h) = (get(&props, "width").and_then(length), get(&props, "height").and_then(length));
            if let (Some(v), Some(w), Some(h)) = (vb, w, h) {
                let (sx, sy) = (w / v[2], h / v[3]);
                t = mul(&t, &[sx, 0.0, 0.0, sy, -sx * v[0], -sy * v[1]]);
            }
            ctx.ctm = mul(&ctx.ctm, &t);
        }
        update_inh(&mut ctx.inh, &props);
        let show = !ctx.hidden && ctx.inh.visible;

        let mut bb: Option<Bb> = None;
        match name {
            "text" => {
                if show {
                    let content: String = node.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect();
                    emit_text(&props, &content, &ctx, &mut self.out);
                }
                return None;
            }
            "image" => {
                if show {
                    self.warn("embedded raster <image> ignored");
                }
                return None;
            }
            "use" => {
                let href = get(&props, "href").map(|h| h.trim().trim_start_matches('#').to_string());
                let target = href.and_then(|h| self.ids.get(h.as_str()).copied());
                let mut c2 = ctx.clone();
                c2.ctm = mul(&c2.ctm, &[1.0, 0.0, 0.0, 1.0, fnum(&props, "x"), fnum(&props, "y")]);
                if let Some(t) = target {
                    bb = self.walk(t, &c2, depth + 1, true);
                }
            }
            _ => {
                let mut local: Vec<(Shape, bool)> = Vec::new();
                emit(name, &props, &ctx, &mut local);
                for (s, vis) in local {
                    bb = bb_union(bb, shape_bb(&s));
                    if vis && show {
                        self.out.push(s);
                    }
                }
                if let Some(f) = get(&props, "fill") {
                    if f.starts_with("url(") && show {
                        self.warn("gradient or pattern fill replaced by no fill");
                    }
                }
                for c in node.children().filter(|c| c.is_element()) {
                    let b = self.walk(c, &ctx, depth + 1, false);
                    bb = bb_union(bb, b);
                }
            }
        }
        if let (Some(id), Some(b)) = (node.attribute("id"), bb) {
            self.bbs.entry(id.to_string()).or_insert(b);
        }
        bb
    }
}

fn emit_text(props: &[(String, String)], content: &str, ctx: &Ctx, out: &mut Vec<Shape>) {
    let text = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return;
    }
    let at = apply(&ctx.ctm, [fnum(props, "x"), fnum(props, "y")]);
    let size = ctx.inh.font * det_scale(&ctx.ctm);
    let fill = ctx.inh.fill.as_deref().filter(|f| !is_black(f) && *f != "none" && !f.starts_with("url(")).map(norm_color);
    let style = Style { stroke: None, fill, width: None };
    out.push(Shape::Text { at, text, size, anchor: ctx.inh.anchor, style });
}

fn push_poly(out: &mut Vec<(Shape, bool)>, pts: &[[f64; 2]], closed: bool, ctx: &Ctx) {
    let scale = det_scale(&ctx.ctm);
    let mut closed = closed && pts.len() >= 3;
    if !closed && pts.len() >= 3 {
        // An open path with a fill and no stroke is filled as if it were closed.
        let (st, _) = make_style(&ctx.inh, scale, true);
        closed = st.fill.is_some() && st.stroke.as_deref() == Some("none");
    }
    let (style, visible) = make_style(&ctx.inh, scale, closed);
    let pts: Vec<[f64; 2]> = pts.iter().map(|p| apply(&ctx.ctm, *p)).collect();
    if pts.len() < 2 {
        return;
    }
    out.push((if closed { Shape::Poly { pts, style } } else { Shape::Line { pts, style } }, visible));
}

fn ellipse_pts(c: [f64; 2], rx: f64, ry: f64) -> Vec<[f64; 2]> {
    (0..32)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / 32.0;
            [c[0] + rx * t.cos(), c[1] + ry * t.sin()]
        })
        .collect()
}

fn emit(name: &str, props: &[(String, String)], ctx: &Ctx, out: &mut Vec<(Shape, bool)>) {
    let n = |k: &str| fnum(props, k);
    let m = &ctx.ctm;
    match name {
        "line" => push_poly(out, &[[n("x1"), n("y1")], [n("x2"), n("y2")]], false, ctx),
        "polyline" | "polygon" => {
            let v = numbers(get(props, "points").unwrap_or(""));
            let pts: Vec<[f64; 2]> = v.windows(2).step_by(2).map(|c| [c[0], c[1]]).collect();
            push_poly(out, &pts, name == "polygon", ctx);
        }
        "rect" => {
            let (x, y, w, h) = (n("x"), n("y"), n("width"), n("height"));
            if w <= 0.0 || h <= 0.0 {
                // Zero-size rectangles (terminal markers) still have a position.
                let p = apply(m, [x, y]);
                out.push((Shape::Rect { at: p, size: [0.0, 0.0], r: 0.0, style: Style::default() }, false));
                return;
            }
            if axis_aligned(m) {
                let (style, visible) = make_style(&ctx.inh, det_scale(m), true);
                let (p0, p1) = (apply(m, [x, y]), apply(m, [x + w, y + h]));
                let size = [(p1[0] - p0[0]).abs(), (p1[1] - p0[1]).abs()];
                let rx = get(props, "rx").and_then(length).or_else(|| get(props, "ry").and_then(length)).unwrap_or(0.0);
                let r = (rx * m[0].abs()).min(size[0].min(size[1]) / 2.0).max(0.0);
                out.push((Shape::Rect { at: [p0[0].min(p1[0]), p0[1].min(p1[1])], size, r, style }, visible));
            } else {
                push_poly(out, &[[x, y], [x + w, y], [x + w, y + h], [x, y + h]], true, ctx);
            }
        }
        "circle" | "ellipse" => {
            let (rx, ry) = if name == "circle" { (n("r"), n("r")) } else { (n("rx"), n("ry")) };
            if rx <= 0.0 || ry <= 0.0 {
                return;
            }
            let c = [n("cx"), n("cy")];
            if uniform(m) && (rx - ry).abs() < 1e-9 {
                let (style, visible) = make_style(&ctx.inh, det_scale(m), true);
                out.push((Shape::Circle { at: apply(m, c), r: rx * m[0].abs(), style }, visible));
            } else {
                push_poly(out, &ellipse_pts(c, rx, ry), true, ctx);
            }
        }
        "path" => {
            let Ok(subs) = parse_path(get(props, "d").unwrap_or("")) else { return };
            for s in subs {
                push_poly(out, &s.pts, s.closed, ctx);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RES: &str = r##"<?xml version="1.0"?>
<!-- hand written -->
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40">
  <defs><rect x="0" y="0" width="999" height="999"/></defs>
  <g transform="translate(10,10)" stroke="#336699">
    <line x1="0" y1="10" x2="20" y2="10"/>
    <rect x="20" y="0" width="40" height="20" fill="none" stroke-width="2"/>
    <line x1="60" y1="10" x2="80" y2="10" stroke="black"/>
    <circle cx="40" cy="10" r="5" fill="none"/>
    <text x="40" y="-2" font-size="9" text-anchor="middle">R &amp; <tspan>X</tspan></text>
  </g>
</svg>"##;

    fn pins() -> Vec<(String, i32, i32)> {
        vec![("a".into(), 0, 1), ("b".into(), 8, 1)]
    }

    #[test]
    fn imports_basic_shapes_and_normalises_bbox() {
        let d = import_svg(RES, "myres", &pins()).unwrap();
        assert_eq!(d.ty, "myres");
        assert_eq!(d.size, [8.0, 2.0]);
        assert_eq!(d.pins.len(), 2);
        assert_eq!(d.pins[0].dir, Dir::Left);
        assert_eq!(d.pins[1].dir, Dir::Right);
        assert_eq!(d.shapes.len(), 5, "defs content must be skipped");
        match &d.shapes[0] {
            Shape::Line { pts, style } => {
                assert_eq!(pts, &vec![[0.0, 1.0], [2.0, 1.0]]);
                assert_eq!(style.stroke.as_deref(), Some("#336699"));
            }
            s => panic!("{s:?}"),
        }
        match &d.shapes[1] {
            Shape::Rect { at, size, style, .. } => {
                assert_eq!((*at, *size), ([2.0, 0.0], [4.0, 2.0]));
                assert_eq!(style.width, Some(0.2));
            }
            s => panic!("{s:?}"),
        }
        match &d.shapes[2] {
            Shape::Line { pts, style } => {
                assert_eq!(pts[1], [8.0, 1.0]);
                assert_eq!(style.stroke, None, "black stroke maps to theme foreground");
            }
            s => panic!("{s:?}"),
        }
        match &d.shapes[3] {
            Shape::Circle { at, r, .. } => assert_eq!((*at, *r), ([4.0, 1.0], 0.5)),
            s => panic!("{s:?}"),
        }
        match &d.shapes[4] {
            Shape::Text { at, text, size, anchor, .. } => {
                assert_eq!(text, "R & X");
                assert_eq!(*anchor, Anchor::Middle);
                assert_eq!(*size, 0.9);
                assert_eq!(*at, [4.0, -0.2]);
            }
            s => panic!("{s:?}"),
        }
        // The result must serialise to a valid .part.
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(crate::parse_part(&json).unwrap().shapes.len(), 5);
    }

    #[test]
    fn no_pins_yields_no_pins() {
        let d = import_svg(RES, "x", &[]).unwrap();
        assert!(d.pins.is_empty());
        assert_eq!(d.size, [8.0, 2.0]);
    }

    #[test]
    fn custom_scale_and_pin_growth() {
        let d = import_svg_scaled(RES, "x", &[("p".into(), 0, 3)], 20.0).unwrap();
        assert_eq!(d.size, [4.0, 3.0], "bbox 80x20 px at 20 px/unit, grown to include the pin");
        assert_eq!(d.pins[0].dir, Dir::Left);
        assert!(import_svg_scaled(RES, "x", &[], 0.0).is_err());
    }

    #[test]
    fn duplicate_pins_and_non_svg_are_errors() {
        let dup = vec![("a".to_string(), 0, 1), ("a".to_string(), 1, 1)];
        assert!(import_svg(RES, "x", &dup).is_err());
        assert!(import_svg("<html><line x1='0' y1='0' x2='1' y2='1'/></html>", "x", &[]).is_err());
        assert!(import_svg("<svg></svg>", "x", &[]).is_err());
    }

    #[test]
    fn polygon_polyline_and_ellipse() {
        let svg = r#"<svg><polygon points="0,0 20,0 10,20" fill="red"/><polyline points="0 30 20 30"/>
            <ellipse cx="10" cy="10" rx="10" ry="5" fill="none"/><ellipse cx="50" cy="10" rx="4" ry="4"/></svg>"#;
        let d = import_svg(svg, "x", &[]).unwrap();
        assert!(matches!(&d.shapes[0], Shape::Poly { pts, style } if pts.len() == 3 && style.fill.as_deref() == Some("#ff0000")));
        assert!(matches!(&d.shapes[1], Shape::Line { pts, .. } if pts.len() == 2));
        assert!(matches!(&d.shapes[2], Shape::Poly { pts, .. } if pts.len() == 32));
        assert!(matches!(&d.shapes[3], Shape::Circle { r, .. } if *r == 0.4));
    }

    #[test]
    fn nested_transforms_compose() {
        let svg = r#"<svg><g transform="translate(10 0)"><g transform="scale(2)"><line x1="0" y1="0" x2="10" y2="5"/></g></g></svg>"#;
        let d = import_svg(svg, "x", &[]).unwrap();
        match &d.shapes[0] {
            Shape::Line { pts, .. } => assert_eq!(pts, &vec![[0.0, 0.0], [2.0, 1.0]]),
            s => panic!("{s:?}"),
        }
        let m = parse_transform("translate(5,6) rotate(90)");
        let p = apply(&m, [1.0, 0.0]);
        assert!((p[0] - 5.0).abs() < 1e-9 && (p[1] - 7.0).abs() < 1e-9);
    }

    #[test]
    fn path_lines_close_and_relative() {
        let s = path_to_shapes("M0 0 H10 V10 Z M20 0 l5 5 5-5", 1.0).unwrap();
        assert_eq!(s.len(), 2);
        assert!(matches!(&s[0], Shape::Poly { pts, .. } if pts == &vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]));
        assert!(matches!(&s[1], Shape::Line { pts, .. } if pts == &vec![[20.0, 0.0], [25.0, 5.0], [30.0, 0.0]]));
    }

    #[test]
    fn path_curves_flatten_to_endpoints() {
        for d in ["M0 0 C0 10 10 10 10 0", "M0 0 S5 10 10 0", "M0 0 Q5 10 10 0", "M0 0 Q5 10 10 0 T20 0"] {
            let s = path_to_shapes(d, 1.0).unwrap();
            match &s[0] {
                Shape::Line { pts, .. } => {
                    assert!(pts.len() > 8, "{d}");
                    let last = pts.last().unwrap();
                    assert!(last[1].abs() < 1e-9 && (last[0] == 10.0 || last[0] == 20.0), "{d}: {last:?}");
                }
                s => panic!("{s:?}"),
            }
        }
    }

    #[test]
    fn path_arc_is_circular() {
        // Quarter circle from (10,0) to (0,10), centre (0,0), clockwise on screen.
        let s = path_to_shapes("M10 0 A10 10 0 0 1 0 10", 1.0).unwrap();
        let Shape::Line { pts, .. } = &s[0] else { panic!() };
        assert!(pts.len() >= 5);
        for p in pts {
            let rad = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((rad - 10.0).abs() < 0.01, "{p:?}");
            assert!(p[0] > -0.01 && p[1] > -0.01);
        }
        assert_eq!(*pts.last().unwrap(), [0.0, 10.0]);
        // Compact flags without separators.
        assert!(path_to_shapes("M10 0a10 10 0 0110-10", 1.0).is_ok());
        assert!(path_to_shapes("L", 1.0).is_err());
        assert!(path_to_shapes("10 10", 1.0).is_err());
    }

    #[test]
    fn style_attribute_and_hidden() {
        let svg = r#"<svg><line x1="0" y1="0" x2="10" y2="0" style="stroke:#ff0000;stroke-width:3"/>
            <line x1="0" y1="5" x2="10" y2="5" display="none"/><rect x="0" y="0" width="1" height="1" stroke="none"/></svg>"#;
        let d = import_svg(svg, "x", &[]).unwrap();
        assert_eq!(d.shapes.len(), 1);
        let Shape::Line { style, .. } = &d.shapes[0] else { panic!() };
        assert_eq!(style.stroke.as_deref(), Some("#ff0000"));
        assert_eq!(style.width, Some(0.3));
    }

    const CSS_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg"><style type="text/css">
        /* Illustrator style export */
        .st0{fill:none;stroke:#000000;stroke-width:2}
        .st1,.st2{fill:#FF0000;stroke:none}
        rect.st2{fill:rgb(0, 128, 255)}
        #special{stroke:#00ff00 !important}
        @media print { .st3{stroke:blue} }
        </style>
        <polyline class="st0" points="0,0 20,0"/>
        <rect class="st1" x="0" y="10" width="10" height="10"/>
        <rect class="st1 st2" x="20" y="10" width="10" height="10"/>
        <line id="special" class="st0" x1="0" y1="30" x2="20" y2="30"/>
        <line class="st3" x1="0" y1="40" x2="20" y2="40"/>
        <line class="st0" style="stroke:#112233" x1="0" y1="50" x2="20" y2="50"/></svg>"##;

    #[test]
    fn css_class_rules_apply_with_specificity_and_precedence() {
        let sc = read_scene(CSS_SVG, false).unwrap();
        assert_eq!(sc.shapes.len(), 6);
        let Shape::Line { style, .. } = &sc.shapes[0] else { panic!() };
        assert_eq!((style.stroke.as_deref(), style.width), (None, Some(2.0)), "black stroke is the theme colour");
        let Shape::Rect { style, .. } = &sc.shapes[1] else { panic!() };
        assert_eq!((style.fill.as_deref(), style.stroke.as_deref()), (Some("#ff0000"), Some("none")));
        let Shape::Rect { style, .. } = &sc.shapes[2] else { panic!() };
        assert_eq!(style.fill.as_deref(), Some("#0080ff"), "rect.st2 is more specific than .st1");
        let Shape::Line { style, .. } = &sc.shapes[3] else { panic!() };
        assert_eq!(style.stroke.as_deref(), Some("#00ff00"), "#id beats class");
        let Shape::Line { style, .. } = &sc.shapes[4] else { panic!() };
        assert_eq!(style.stroke.as_deref(), Some("#0000ff"), "@media content is read");
        let Shape::Line { style, .. } = &sc.shapes[5] else { panic!() };
        assert_eq!(style.stroke.as_deref(), Some("#112233"), "style attribute beats classes");
    }

    #[test]
    fn colours_are_normalised_to_hex() {
        assert_eq!(norm_color("#ABC"), "#aabbcc");
        assert_eq!(norm_color("RGB(255, 0, 0)"), "#ff0000");
        assert_eq!(norm_color("rgb(100%,0%,0%)"), "#ff0000");
        assert_eq!(norm_color("Red"), "#ff0000");
        assert_eq!(norm_color("url(#g)"), "url(#g)");
    }

    #[test]
    fn use_references_and_group_styles() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
            <defs><g id="sym"><line x1="0" y1="0" x2="10" y2="0"/></g></defs>
            <g stroke="#ff0000" stroke-width="4" fill="none">
              <use xlink:href="#sym" x="5" y="7"/>
              <use href="#sym" transform="translate(0 20)" stroke="#0000ff"/>
            </g></svg>"##;
        let sc = read_scene(svg, false).unwrap();
        assert_eq!(sc.shapes.len(), 2);
        let Shape::Line { pts, style } = &sc.shapes[0] else { panic!() };
        assert_eq!(pts, &vec![[5.0, 7.0], [15.0, 7.0]]);
        assert_eq!((style.stroke.as_deref(), style.width), (Some("#ff0000"), Some(4.0)));
        let Shape::Line { pts, style } = &sc.shapes[1] else { panic!() };
        assert_eq!(pts[0], [0.0, 20.0]);
        assert_eq!(style.stroke.as_deref(), Some("#0000ff"), "use element overrides the inherited style");
    }

    #[test]
    fn physical_units_and_element_boxes() {
        // 25.4 mm = 1 in = 10 grid units; viewBox 0..25.4.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="25.4mm" height="12.7mm" viewBox="0 0 25.4 12.7">
            <g id="grp" transform="translate(2.54 0)"><rect id="r" x="0" y="0" width="2.54" height="5.08" fill="none" stroke="#000"/></g></svg>"##;
        let sc = read_scene(svg, true).unwrap();
        assert_eq!(sc.declared_in, Some([1.0, 0.5]));
        let r = sc.ids["r"];
        for (a, b) in r.iter().zip([1.0, 0.0, 2.0, 2.0]) {
            assert!((a - b).abs() < 1e-9, "{r:?}");
        }
        assert_eq!(sc.ids["grp"], r);
        // Legacy unit-less sizes: Illustrator 72 dpi, otherwise 90 dpi; px with viewBox scale.
        let ai = r##"<!-- Generator: Adobe Illustrator 15 --><svg width="72px" height="72px" viewBox="0 0 72 72"><line id="l" x1="0" y1="0" x2="72" y2="0" stroke="#000"/></svg>"##;
        assert!((read_scene(ai, true).unwrap().ids["l"][2] - 10.0).abs() < 1e-9);
        let legacy = r##"<svg width="90" height="90"><line id="l" x1="0" y1="0" x2="90" y2="0" stroke="#000"/></svg>"##;
        assert!((read_scene(legacy, true).unwrap().ids["l"][2] - 10.0).abs() < 1e-9);
        let no_vb = r##"<svg width="1in" height="1in"><line id="l" x1="0" y1="0" x2="90" y2="0" stroke="#000"/></svg>"##;
        assert!((read_scene(no_vb, true).unwrap().ids["l"][2] - 10.0).abs() < 1e-9);
        // Stroke widths scale with the geometry (0.254 mm = 0.1 unit).
        let w = r##"<svg width="25.4mm" height="25.4mm" viewBox="0 0 25.4 25.4"><line x1="0" y1="1" x2="5" y2="1" stroke="#f00" stroke-width="0.254"/></svg>"##;
        let Shape::Line { style, .. } = &read_scene(w, true).unwrap().shapes[0] else { panic!() };
        assert!((style.width.unwrap() - 0.1).abs() < 1e-3);
    }

    #[test]
    fn physical_mode_uses_svg_defaults() {
        // No fill attribute means black, no stroke attribute means none.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1in" height="1in" viewBox="0 0 10 10">
            <path d="M0 0 L5 0 L5 5 Z"/><line x1="0" y1="9" x2="9" y2="9"/>
            <path d="M0 6 L3 6 L3 8" fill="#ffffff"/></svg>"##;
        let sc = read_scene(svg, true).unwrap();
        assert_eq!(sc.shapes.len(), 2, "the unstroked line is invisible");
        let Shape::Poly { style, .. } = &sc.shapes[0] else { panic!() };
        assert_eq!((style.fill.as_deref(), style.stroke.as_deref()), (Some("#000000"), Some("none")));
        assert!(matches!(&sc.shapes[1], Shape::Poly { style, .. } if style.fill.as_deref() == Some("#ffffff")), "open filled path is closed");
        assert!(sc.ids.is_empty());
    }

    #[test]
    fn physical_mode_simplifies_flattened_curves() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1in" height="1in" viewBox="0 0 10 10">
            <path d="M0 5 H8 C8.1 5 8.2 5.05 8.2 5.1" stroke="#f00" fill="none"/>
            <path d="M0 8 H4 L8 8" stroke="#f00" fill="none"/></svg>"##;
        let sc = read_scene(svg, true).unwrap();
        let Shape::Line { pts, .. } = &sc.shapes[0] else { panic!() };
        assert!(pts.len() <= 6, "{} points", pts.len());
        assert_eq!(pts[0], [0.0, 5.0]);
        let Shape::Line { pts, .. } = &sc.shapes[1] else { panic!() };
        assert_eq!(pts.len(), 2, "collinear points are dropped");
        assert_eq!(*pts.last().unwrap(), [8.0, 8.0]);
    }

    #[test]
    fn invisible_elements_keep_their_boxes() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg"><rect id="t" x="1" y="2" width="0.5" height="0.5" fill="none" stroke="none"/>
            <rect id="z" x="4" y="4" width="0" height="0"/><g display="none"><line id="h" x1="0" y1="0" x2="3" y2="0" stroke="#000"/></g>
            <line x1="0" y1="0" x2="10" y2="0" stroke="#000" opacity="0"/></svg>"##;
        let sc = read_scene(svg, false).unwrap();
        assert!(sc.shapes.is_empty());
        assert_eq!(sc.ids["t"], [1.0, 2.0, 1.5, 2.5]);
        assert_eq!(sc.ids["z"], [4.0, 4.0, 4.0, 4.0]);
        assert_eq!(sc.ids["h"], [0.0, 0.0, 3.0, 0.0]);
    }

    #[test]
    fn doctype_entities_and_missing_prefixes_parse() {
        let svg = "<?xml version=\"1.0\"?><!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\" [<!ENTITY ns_svg \"http://www.w3.org/2000/svg\">]><svg xmlns=\"&ns_svg;\"><line stroke=\"#f00\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\" inkscape:foo=\"x\"/></svg>";
        assert_eq!(read_scene(svg, false).unwrap().shapes.len(), 1);
        assert!(read_scene("<svg><g></svg>", false).is_err());
    }
}
