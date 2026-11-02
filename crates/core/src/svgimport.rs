//! Tiny dependency-free SVG subset reader that turns artwork into a [`PartDef`].
//!
//! Supported elements: `line`, `rect` (with `rx`/`ry`), `circle`, `ellipse` (a circle when
//! `rx == ry` and the transform is a uniform scale, otherwise a 32-gon), `polyline`,
//! `polygon`, `path` (commands `M L H V C S Q T A Z`, absolute and relative; curves are
//! flattened to polylines), `text`, and `g`/`svg`/`a` containers. `transform` attributes
//! (`translate`, `scale`, `rotate`, `matrix`, `skewX`, `skewY`) compose down the tree.
//! `defs`, `clipPath`, `mask`, `symbol`, `style`, `title`, ... and `display:none` subtrees
//! are skipped. Root `width`/`height`/`viewBox` are ignored: user units are taken as pixels.
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
    let raw = read_shapes(svg)?;
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

fn infer_dir(at: [i32; 2], w: f64, h: f64) -> Dir {
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
// Minimal XML tokenizer

enum Tag {
    Open { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    Close,
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match tail.find(';') {
            Some(j) if j <= 10 => {
                let ent = &tail[1..j];
                let rep = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => ent
                        .strip_prefix("#x")
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                match rep {
                    Some(c) => {
                        out.push(c);
                        rest = &tail[j + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &tail[1..];
                    }
                }
            }
            _ => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn next_tag(src: &str, mut pos: usize) -> Option<(Tag, usize)> {
    let b = src.as_bytes();
    loop {
        let lt = pos + src[pos..].find('<')?;
        let rest = &src[lt..];
        let skip_to = |end: &str| -> usize {
            rest.find(end).map_or(src.len(), |e| lt + e + end.len())
        };
        if rest.starts_with("<!--") {
            pos = skip_to("-->");
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            pos = skip_to("]]>");
            continue;
        }
        if rest.starts_with("<?") {
            pos = skip_to("?>");
            continue;
        }
        if rest.starts_with("<!") {
            pos = skip_to(">");
            continue;
        }
        if rest.starts_with("</") {
            return Some((Tag::Close, skip_to(">")));
        }
        // Open tag.
        let mut i = lt + 1;
        let ns = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
            i += 1;
        }
        let full = &src[ns..i];
        let name = full.rsplit(':').next().unwrap_or(full).to_string();
        let mut attrs = Vec::new();
        let mut self_closing = false;
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= b.len() {
                return None;
            }
            if b[i] == b'>' {
                i += 1;
                break;
            }
            if b[i] == b'/' {
                self_closing = true;
                i += 1;
                continue;
            }
            let ks = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
                i += 1;
            }
            let key = src[ks..i].to_string();
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let mut val = String::new();
            if i < b.len() && b[i] == b'=' {
                i += 1;
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                    let q = b[i];
                    i += 1;
                    let vs = i;
                    while i < b.len() && b[i] != q {
                        i += 1;
                    }
                    val = decode_entities(&src[vs..i.min(b.len())]);
                    i = (i + 1).min(b.len());
                } else {
                    let vs = i;
                    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                        i += 1;
                    }
                    val = src[vs..i].to_string();
                }
            }
            if key.is_empty() {
                // Defensive: avoid an infinite loop on stray characters.
                i += 1;
                continue;
            }
            let key = key.rsplit(':').next().unwrap_or(&key).to_string();
            attrs.push((key, val));
        }
        return Some((Tag::Open { name, attrs, self_closing }, i));
    }
}

fn get<'a>(props: &'a [(String, String)], k: &str) -> Option<&'a str> {
    props.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn fnum(props: &[(String, String)], k: &str) -> f64 {
    get(props, k).and_then(length).unwrap_or(0.0)
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

fn is_black(c: &str) -> bool {
    matches!(c.to_ascii_lowercase().as_str(), "black" | "#000" | "#000000" | "rgb(0,0,0)" | "rgb(0, 0, 0)")
}

/// Style for an emitted shape, or None when it would be invisible.
fn make_style(inh: &Inh, scale: f64, closed: bool) -> Option<Style> {
    let stroke_none = inh.stroke.as_deref().is_some_and(|s| s == "none" || s.starts_with("url("));
    let stroke = match inh.stroke.as_deref() {
        None => None,
        Some(_) if stroke_none => Some("none".to_string()),
        Some(c) if is_black(c) => None,
        Some(c) => Some(c.to_string()),
    };
    let fill = if closed {
        match inh.fill.as_deref() {
            None | Some("none") | Some("transparent") => None,
            Some(c) if c.starts_with("url(") => None,
            Some(c) if is_black(c) => Some("#000000".to_string()),
            Some(c) => Some(c.to_string()),
        }
    } else {
        None
    };
    if stroke_none && fill.is_none() {
        return None;
    }
    Some(Style { stroke, fill, width: inh.width.map(|w| w * scale) })
}

fn update_inh(inh: &mut Inh, props: &[(String, String)]) {
    let clean = |s: &str| s.trim().to_string();
    if let Some(v) = get(props, "stroke").filter(|v| *v != "inherit") {
        inh.stroke = Some(clean(v));
    }
    if let Some(v) = get(props, "fill").filter(|v| *v != "inherit") {
        inh.fill = Some(clean(v));
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
}

fn read_shapes(svg: &str) -> Result<Vec<Shape>, String> {
    let mut out = Vec::new();
    let root = Ctx {
        ctm: IDENT,
        inh: Inh { stroke: None, fill: None, width: None, font: 10.0, anchor: Anchor::Start },
        hidden: false,
    };
    let mut stack = vec![root];
    let mut saw_svg = false;
    let mut pos = 0;
    while let Some((tag, end)) = next_tag(svg, pos) {
        pos = end;
        let (name, attrs, self_closing) = match tag {
            Tag::Close => {
                if stack.len() > 1 {
                    stack.pop();
                }
                continue;
            }
            Tag::Open { name, attrs, self_closing } => (name, attrs, self_closing),
        };
        if name == "svg" {
            saw_svg = true;
        }
        let mut props = attrs;
        if let Some(st) = get(&props, "style").map(str::to_string) {
            for decl in st.split(';') {
                if let Some((k, v)) = decl.split_once(':') {
                    props.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
        }
        let mut ctx = stack.last().cloned().expect("stack never empty");
        ctx.hidden = ctx.hidden || SKIP.contains(&name.as_str()) || get(&props, "display").map(str::trim) == Some("none");
        if let Some(t) = get(&props, "transform") {
            ctx.ctm = mul(&ctx.ctm, &parse_transform(t));
        }
        update_inh(&mut ctx.inh, &props);

        if name == "text" {
            if self_closing {
                continue;
            }
            let rel = svg[pos..].find("</text").unwrap_or(svg.len() - pos);
            let content = &svg[pos..pos + rel];
            pos = svg[pos + rel..].find('>').map_or(svg.len(), |g| pos + rel + g + 1);
            if !ctx.hidden {
                emit_text(&props, content, &ctx, &mut out);
            }
            continue;
        }
        if !ctx.hidden {
            emit(&name, &props, &ctx, &mut out);
        }
        if !self_closing {
            stack.push(ctx);
        }
    }
    if !saw_svg {
        return Err("not an SVG document (no <svg> element)".into());
    }
    Ok(out)
}

fn emit_text(props: &[(String, String)], content: &str, ctx: &Ctx, out: &mut Vec<Shape>) {
    // Strip nested tags (tspan, ...), decode entities, collapse whitespace.
    let mut plain = String::new();
    let mut in_tag = false;
    for ch in content.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => plain.push(c),
            _ => {}
        }
    }
    let plain = decode_entities(&plain);
    let text = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return;
    }
    let at = apply(&ctx.ctm, [fnum(props, "x"), fnum(props, "y")]);
    let size = ctx.inh.font * det_scale(&ctx.ctm);
    let style = Style {
        stroke: None,
        fill: ctx.inh.fill.as_deref().filter(|f| !is_black(f) && *f != "none").map(str::to_string),
        width: None,
    };
    out.push(Shape::Text { at, text, size, anchor: ctx.inh.anchor, style });
}

fn push_poly(out: &mut Vec<Shape>, pts: &[[f64; 2]], closed: bool, ctx: &Ctx) {
    let closed = closed && pts.len() >= 3;
    let Some(style) = make_style(&ctx.inh, det_scale(&ctx.ctm), closed) else { return };
    let pts: Vec<[f64; 2]> = pts.iter().map(|p| apply(&ctx.ctm, *p)).collect();
    if pts.len() < 2 {
        return;
    }
    out.push(if closed { Shape::Poly { pts, style } } else { Shape::Line { pts, style } });
}

fn ellipse_pts(c: [f64; 2], rx: f64, ry: f64) -> Vec<[f64; 2]> {
    (0..32)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / 32.0;
            [c[0] + rx * t.cos(), c[1] + ry * t.sin()]
        })
        .collect()
}

fn emit(name: &str, props: &[(String, String)], ctx: &Ctx, out: &mut Vec<Shape>) {
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
                return;
            }
            if axis_aligned(m) {
                let Some(style) = make_style(&ctx.inh, det_scale(m), true) else { return };
                let (p0, p1) = (apply(m, [x, y]), apply(m, [x + w, y + h]));
                let size = [(p1[0] - p0[0]).abs(), (p1[1] - p0[1]).abs()];
                let rx = get(props, "rx").and_then(length).or_else(|| get(props, "ry").and_then(length)).unwrap_or(0.0);
                let r = (rx * m[0].abs()).min(size[0].min(size[1]) / 2.0).max(0.0);
                out.push(Shape::Rect { at: [p0[0].min(p1[0]), p0[1].min(p1[1])], size, r, style });
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
                let Some(style) = make_style(&ctx.inh, det_scale(m), true) else { return };
                out.push(Shape::Circle { at: apply(m, c), r: rx * m[0].abs(), style });
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
        assert!(matches!(&d.shapes[0], Shape::Poly { pts, style } if pts.len() == 3 && style.fill.as_deref() == Some("red")));
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
}
