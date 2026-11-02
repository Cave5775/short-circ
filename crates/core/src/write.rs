//! Canonical writers for `.circ` and `.part` (docs/FORMAT.md "Canonical writing").
//!
//! Hand-written emitter so key order never depends on a map type: 2-space indent, one entry per
//! line for `parts`, `wires`, `labels`, `nets`, part `pins`, view `pins`, `shapes` and `buses`,
//! every entry compact (`{"k":"v"}`, no spaces) with the key order of the model structs, final
//! newline. Optional and default fields are omitted (a `None`, an empty `props` is kept because
//! the spec examples always show it, a default `rot`/`mirror`/`r`/`anchor`/text `size`; `dir` is\n//! always written in the schematic view like the spec example and omitted when `left` in the\n//! breadboard view, where it is ignored).
//! `write(parse(write(x))) == write(x)` byte for byte.

use crate::model::{
    Anchor, Circuit, Dir, Label, NetDecl, Part, PartDef, PinDef, PinPos, Placement, Shape, Style, Theme, ViewDef,
};
use std::collections::BTreeMap;

pub fn circuit_to_string(c: &Circuit) -> String {
    let mut out = String::new();
    write_circuit(&mut out, c, 0);
    out.push('\n');
    out
}

pub fn part_to_string(p: &PartDef) -> String {
    let mut out = String::new();
    write_part_def(&mut out, p, 0);
    out.push('\n');
    out
}

// ---- primitives -------------------------------------------------------------------------

fn s(v: &str) -> String {
    serde_json::to_string(v).expect("string serialises")
}

/// Finite float without a trailing `.0`, `-0` normalised to `0`; other values as serde_json.
fn num(v: f64) -> String {
    if !v.is_finite() || v == 0.0 {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    serde_json::Number::from_f64(v).map(|n| n.to_string()).unwrap_or_else(|| "0".to_string())
}

fn pt(p: [i32; 2]) -> String {
    format!("[{},{}]", p[0], p[1])
}

fn fpt(p: [f64; 2]) -> String {
    format!("[{},{}]", num(p[0]), num(p[1]))
}

fn list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    let v: Vec<String> = items.iter().map(f).collect();
    format!("[{}]", v.join(","))
}

fn obj(fields: Vec<(&str, String)>) -> String {
    let v: Vec<String> = fields.into_iter().map(|(k, v)| format!("{}:{}", s(k), v)).collect();
    format!("{{{}}}", v.join(","))
}

fn str_map(m: &BTreeMap<String, String>) -> String {
    let v: Vec<String> = m.iter().map(|(k, v)| format!("{}:{}", s(k), s(v))).collect();
    format!("{{{}}}", v.join(","))
}

/// Compact JSON of an arbitrary value, keeping the value's own key order.
fn value(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                n.to_string()
            } else {
                num(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(x) => s(x),
        Value::Array(a) => list(a, value),
        Value::Object(o) => {
            let v: Vec<String> = o.iter().map(|(k, v)| format!("{}:{}", s(k), value(v))).collect();
            format!("{{{}}}", v.join(","))
        }
    }
}

fn pad(n: usize) -> String {
    "  ".repeat(n)
}

/// One `"key": [` block with one entry per line at depth `d + 1`; `[]` when empty.
fn block(out: &mut String, d: usize, key: &str, entries: Vec<String>, last: bool) {
    let comma = if last { "" } else { "," };
    if entries.is_empty() {
        out.push_str(&format!("{}{}: []{}\n", pad(d), s(key), comma));
        return;
    }
    out.push_str(&format!("{}{}: [\n", pad(d), s(key)));
    let n = entries.len();
    for (i, e) in entries.iter().enumerate() {
        out.push_str(&format!("{}{}{}\n", pad(d + 1), e, if i + 1 < n { "," } else { "" }));
    }
    out.push_str(&format!("{}]{}\n", pad(d), comma));
}

/// Collects the top-level members of an object, then prints them with correct commas.
struct Members {
    items: Vec<Item>,
}

enum Item {
    /// `"key": <inline text>`
    Inline(String, String),
    /// `"key": [ entries one per line ]`
    Block(String, Vec<String>),
    /// `"key": { entries one per line }`
    Map(String, Vec<(String, String)>),
    /// `"key": ` followed by a pre-rendered multi-line value (already indented for depth d + 1
    /// except its first line)
    Nested(String, String),
}

impl Members {
    fn new() -> Self {
        Members { items: Vec::new() }
    }
    fn inline(&mut self, k: &str, v: String) {
        self.items.push(Item::Inline(k.into(), v));
    }
    fn block(&mut self, k: &str, v: Vec<String>) {
        self.items.push(Item::Block(k.into(), v));
    }
    fn map(&mut self, k: &str, v: Vec<(String, String)>) {
        self.items.push(Item::Map(k.into(), v));
    }
    fn nested(&mut self, k: &str, v: String) {
        self.items.push(Item::Nested(k.into(), v));
    }

    /// Prints `{ ... }` for an object whose members sit at depth `d + 1` (no trailing newline;
    /// the closing brace is at depth `d`, the opening brace is not indented).
    fn print(self, out: &mut String, d: usize) {
        out.push_str("{\n");
        let n = self.items.len();
        for (i, it) in self.items.into_iter().enumerate() {
            let last = i + 1 == n;
            let comma = if last { "" } else { "," };
            let p = pad(d + 1);
            match it {
                Item::Inline(k, v) => out.push_str(&format!("{p}{}: {v}{comma}\n", s(&k))),
                Item::Block(k, v) => block(out, d + 1, &k, v, last),
                Item::Map(k, entries) => {
                    if entries.is_empty() {
                        out.push_str(&format!("{p}{}: {{}}{comma}\n", s(&k)));
                    } else {
                        out.push_str(&format!("{p}{}: {{\n", s(&k)));
                        let m = entries.len();
                        for (j, (ek, ev)) in entries.iter().enumerate() {
                            out.push_str(&format!(
                                "{}{}: {}{}\n",
                                pad(d + 2),
                                s(ek),
                                ev,
                                if j + 1 < m { "," } else { "" }
                            ));
                        }
                        out.push_str(&format!("{p}}}{comma}\n"));
                    }
                }
                Item::Nested(k, v) => out.push_str(&format!("{p}{}: {v}{comma}\n", s(&k))),
            }
        }
        out.push_str(&pad(d));
        out.push('}');
    }
}

// ---- circuit ----------------------------------------------------------------------------

fn write_circuit(out: &mut String, c: &Circuit, d: usize) {
    let mut m = Members::new();
    m.inline("format", s(&c.format));
    if let Some(t) = &c.title {
        m.inline("title", s(t));
    }
    m.block("parts", c.parts.iter().map(part_line).collect());
    m.block("wires", c.wires.iter().map(wire_line).collect());
    m.block("labels", c.labels.iter().map(label_line).collect());
    if !c.nets.is_empty() {
        m.block("nets", c.nets.iter().map(net_line).collect());
    }
    if let Some(t) = &c.theme {
        m.inline("theme", theme_line(t));
    }
    if c.lib.is_empty() {
        m.inline("lib", "[]".into());
    } else {
        let mut nested = String::from("[\n");
        let n = c.lib.len();
        for (i, p) in c.lib.iter().enumerate() {
            nested.push_str(&pad(d + 2));
            write_part_def(&mut nested, p, d + 2);
            nested.push_str(if i + 1 < n { ",\n" } else { "\n" });
        }
        nested.push_str(&pad(d + 1));
        nested.push(']');
        m.nested("lib", nested);
    }
    m.print(out, d);
}

fn placement(p: &Placement) -> String {
    let mut f = vec![("at", pt(p.at))];
    if p.rot != 0 {
        f.push(("rot", p.rot.to_string()));
    }
    if p.mirror {
        f.push(("mirror", "true".into()));
    }
    obj(f)
}

fn part_line(p: &Part) -> String {
    let mut f = vec![("id", s(&p.id)), ("type", s(&p.ty)), ("props", str_map(&p.props))];
    if p.hidden_label {
        f.push(("hidden_label", "true".into()));
    }
    if let Some(v) = &p.color {
        f.push(("color", s(v)));
    }
    if let Some(v) = &p.fill {
        f.push(("fill", s(v)));
    }
    if let Some(v) = &p.label_color {
        f.push(("label_color", s(v)));
    }
    if let Some(v) = &p.schematic {
        f.push(("schematic", placement(v)));
    }
    if let Some(v) = &p.breadboard {
        f.push(("breadboard", placement(v)));
    }
    obj(f)
}

fn wire_line(w: &crate::model::Wire) -> String {
    let mut f = vec![("id", s(&w.id)), ("view", s(w.view.key())), ("pts", list(&w.pts, |p| pt(*p)))];
    if let Some(c) = &w.color {
        f.push(("color", s(c)));
    }
    obj(f)
}

fn label_line(l: &Label) -> String {
    let mut f = vec![("at", pt(l.at)), ("name", s(&l.name))];
    if let Some(c) = &l.color {
        f.push(("color", s(c)));
    }
    obj(f)
}

fn net_line(n: &NetDecl) -> String {
    let mut f = Vec::new();
    if let Some(name) = &n.name {
        f.push(("name", s(name)));
    }
    f.push(("pins", list(&n.pins, |p| s(p))));
    obj(f)
}

fn theme_line(t: &Theme) -> String {
    let mut f = Vec::new();
    for (k, v) in [
        ("background", &t.background),
        ("grid", &t.grid),
        ("wire", &t.wire),
        ("part", &t.part),
        ("label", &t.label),
        ("pin", &t.pin),
        ("junction", &t.junction),
    ] {
        if let Some(v) = v {
            f.push((k, s(v)));
        }
    }
    obj(f)
}

// ---- part definition --------------------------------------------------------------------

fn write_part_def(out: &mut String, p: &PartDef, d: usize) {
    let mut m = Members::new();
    m.inline("format", s(&p.format));
    m.inline("type", s(&p.ty));
    m.inline("name", s(&p.name));
    if let Some(c) = &p.category {
        m.inline("category", s(c));
    }
    m.block("pins", p.pins.iter().map(pin_def_line).collect());
    if !p.buses.is_empty() {
        m.block("buses", p.buses.iter().map(|b| list(b, |x| s(x))).collect());
    }
    if let Some(v) = &p.schematic {
        m.nested("schematic", view_def(v, d + 1, true));
    }
    if let Some(v) = &p.breadboard {
        m.nested("breadboard", view_def(v, d + 1, false));
    }
    if let Some(v) = &p.params {
        m.inline("params", value(v));
    }
    if let Some(meta) = &p.meta {
        if !meta.is_empty() {
            m.map("meta", meta.iter().map(|(k, v)| (k.clone(), s(v))).collect());
        }
    }
    m.print(out, d);
}

fn pin_def_line(p: &PinDef) -> String {
    let mut f = vec![("id", s(&p.id))];
    if let Some(n) = &p.name {
        f.push(("name", s(n)));
    }
    obj(f)
}

fn pin_pos_line(p: &PinPos, always_dir: bool) -> String {
    let mut f = vec![("at", pt(p.at))];
    if always_dir || p.dir != Dir::default() {
        let d = match p.dir {
            Dir::Left => "left",
            Dir::Right => "right",
            Dir::Up => "up",
            Dir::Down => "down",
        };
        f.push(("dir", s(d)));
    }
    obj(f)
}

fn view_def(v: &ViewDef, d: usize, schematic: bool) -> String {
    let mut m = Members::new();
    m.inline("size", fpt(v.size));
    m.map("pins", v.pins.iter().map(|(k, p)| (k.clone(), pin_pos_line(p, schematic))).collect());
    if !v.shapes.is_empty() {
        m.block("shapes", v.shapes.iter().map(shape_line).collect());
    }
    if let Some(svg) = &v.svg {
        m.inline("svg", s(svg));
    }
    let mut out = String::new();
    m.print(&mut out, d);
    out
}

fn style_fields(f: &mut Vec<(&'static str, String)>, st: &Style) {
    if let Some(v) = &st.stroke {
        f.push(("stroke", s(v)));
    }
    if let Some(v) = &st.fill {
        f.push(("fill", s(v)));
    }
    if let Some(w) = st.width {
        f.push(("width", num(w)));
    }
}

fn shape_line(sh: &Shape) -> String {
    let mut f: Vec<(&'static str, String)> = Vec::new();
    match sh {
        Shape::Line { pts, style } => {
            f.push(("k", s("line")));
            f.push(("pts", list(pts, |p| fpt(*p))));
            style_fields(&mut f, style);
        }
        Shape::Rect { at, size, r, style } => {
            f.push(("k", s("rect")));
            f.push(("at", fpt(*at)));
            f.push(("size", fpt(*size)));
            if *r != 0.0 {
                f.push(("r", num(*r)));
            }
            style_fields(&mut f, style);
        }
        Shape::Circle { at, r, style } => {
            f.push(("k", s("circle")));
            f.push(("at", fpt(*at)));
            f.push(("r", num(*r)));
            style_fields(&mut f, style);
        }
        Shape::Arc { at, r, from, to, style } => {
            f.push(("k", s("arc")));
            f.push(("at", fpt(*at)));
            f.push(("r", num(*r)));
            f.push(("from", num(*from)));
            f.push(("to", num(*to)));
            style_fields(&mut f, style);
        }
        Shape::Poly { pts, style } => {
            f.push(("k", s("poly")));
            f.push(("pts", list(pts, |p| fpt(*p))));
            style_fields(&mut f, style);
        }
        Shape::Text { at, text, size, anchor, style } => {
            f.push(("k", s("text")));
            f.push(("at", fpt(*at)));
            f.push(("text", s(text)));
            if *size != 1.0 {
                f.push(("size", num(*size)));
            }
            if *anchor != Anchor::Start {
                f.push(("anchor", s(if *anchor == Anchor::Middle { "middle" } else { "end" })));
            }
            style_fields(&mut f, style);
        }
    }
    obj(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{View, Wire};

    fn style(stroke: Option<&str>, fill: Option<&str>, width: Option<f64>) -> Style {
        Style { stroke: stroke.map(String::from), fill: fill.map(String::from), width }
    }

    fn all_shapes() -> Vec<Shape> {
        vec![
            Shape::Line { pts: vec![[0.0, 1.0], [2.5, 1.0]], style: Style::default() },
            Shape::Rect { at: [2.0, 0.0], size: [4.0, 2.0], r: 0.0, style: style(None, Some("none"), None) },
            Shape::Rect { at: [2.0, 0.0], size: [4.0, 2.0], r: 0.5, style: style(Some("#112233"), None, Some(0.2)) },
            Shape::Circle { at: [1.0, 1.0], r: 0.75, style: style(Some("red"), Some("#fff"), None) },
            Shape::Arc { at: [3.0, 3.0], r: 1.0, from: 0.0, to: 180.0, style: Style::default() },
            Shape::Poly { pts: vec![[0.0, 0.0], [1.0, 0.0], [0.5, -0.8]], style: style(None, Some("#000000"), None) },
            Shape::Text {
                at: [4.0, -0.8],
                text: "{ref} \"q\" \\ \u{b5}".into(),
                size: 0.9,
                anchor: Anchor::Middle,
                style: Style::default(),
            },
            Shape::Text { at: [0.0, 0.0], text: "x".into(), size: 1.0, anchor: Anchor::End, style: Style::default() },
            Shape::Text { at: [0.0, 0.0], text: "y".into(), size: 1.0, anchor: Anchor::Start, style: Style::default() },
        ]
    }

    fn pin(at: Pt2, dir: Dir) -> PinPos {
        PinPos { at, dir }
    }
    type Pt2 = [i32; 2];

    fn big_part() -> PartDef {
        let mut sch_pins = BTreeMap::new();
        sch_pins.insert("a".to_string(), pin([0, 1], Dir::Left));
        sch_pins.insert("b".to_string(), pin([8, 1], Dir::Right));
        sch_pins.insert("c".to_string(), pin([4, 0], Dir::Up));
        sch_pins.insert("d".to_string(), pin([4, 2], Dir::Down));
        let mut bb_pins = BTreeMap::new();
        bb_pins.insert("a".to_string(), pin([0, 0], Dir::Left));
        bb_pins.insert("b".to_string(), pin([4, 0], Dir::Left));
        let mut meta = BTreeMap::new();
        meta.insert("source".to_string(), "fritzing".to_string());
        meta.insert("title".to_string(), "R \"x\"".to_string());
        PartDef {
            format: crate::model::FORMAT.into(),
            ty: "res".into(),
            name: "Resistor".into(),
            category: Some("passive".into()),
            pins: vec![
                PinDef { id: "a".into(), name: Some("A".into()) },
                PinDef { id: "b".into(), name: None },
                PinDef { id: "c".into(), name: None },
                PinDef { id: "d".into(), name: None },
            ],
            buses: vec![vec!["a".into(), "b".into()], vec!["c".into(), "d".into()]],
            schematic: Some(ViewDef { size: [8.0, 2.0], pins: sch_pins, shapes: all_shapes(), svg: None }),
            breadboard: Some(ViewDef {
                size: [4.0, 1.0],
                pins: bb_pins,
                shapes: vec![],
                svg: Some("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 40 10\">\n<path d=\"M0 0\"/></svg>".into()),
            }),
            params: Some(serde_json::json!({"gen":"dip","pins":8})),
            meta: Some(meta),
        }
    }

    fn big_circuit() -> Circuit {
        let mut props = BTreeMap::new();
        props.insert("value".to_string(), "330".to_string());
        props.insert("label".to_string(), "R\"1".to_string());
        Circuit {
            format: crate::model::FORMAT.into(),
            title: Some("T \u{e9}".into()),
            parts: vec![
                Part {
                    id: "R1".into(),
                    ty: "res".into(),
                    props,
                    hidden_label: true,
                    color: Some("red".into()),
                    fill: Some("none".into()),
                    label_color: Some("#abc".into()),
                    schematic: Some(Placement { at: [20, 10], rot: 90, mirror: true }),
                    breadboard: Some(Placement::at(14, -8)),
                },
                Part { id: "G".into(), ty: "gnd".into(), ..Part::default() },
            ],
            wires: vec![
                Wire { id: "w1".into(), view: View::Schematic, pts: vec![[1, 2], [3, 2], [3, -4]], color: None },
                Wire { id: "j1".into(), view: View::Breadboard, pts: vec![[1, 2], [3, 4]], color: Some("red".into()) },
            ],
            labels: vec![
                Label { at: [12, 10], name: "VCC".into(), color: None },
                Label { at: [1, 2], name: "GND".into(), color: Some("blue".into()) },
            ],
            nets: vec![
                NetDecl { name: Some("VCC".into()), pins: vec!["B1.pos".into(), "R1.a".into()] },
                NetDecl { name: None, pins: vec!["R1.b".into()] },
            ],
            lib: vec![big_part(), PartDef { ty: "bare".into(), ..PartDef::default() }],
            theme: Some(Theme { background: Some("#101010".into()), wire: Some("lime".into()), ..Theme::default() }),
        }
    }

    fn jv<T: serde::Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap()
    }

    /// Serde adds nulls for `None` options without skip attributes; compare ignoring them.
    fn strip_nulls(v: serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match v {
            Value::Object(o) => Value::Object(o.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, strip_nulls(v))).collect()),
            Value::Array(a) => Value::Array(a.into_iter().map(strip_nulls).collect()),
            o => o,
        }
    }

    #[test]
    fn circuit_roundtrip_is_byte_identical_and_structurally_equal() {
        let c = big_circuit();
        let w1 = circuit_to_string(&c);
        let back = crate::parse_circuit(&w1).expect("parses");
        let w2 = circuit_to_string(&back);
        assert_eq!(w1, w2);
        assert!(w1.ends_with("}\n"));
        assert!(!w1.contains(" \n"));
        // parse of the written text equals the original structure
        assert_eq!(strip_nulls(jv(&c)), strip_nulls(jv(&back)), "{w1}");
        // third round
        assert_eq!(w2, circuit_to_string(&crate::parse_circuit(&w2).unwrap()));
    }

    #[test]
    fn part_roundtrip_is_byte_identical_and_structurally_equal() {
        let p = big_part();
        let w1 = part_to_string(&p);
        let back = crate::parse_part(&w1).expect("parses");
        let w2 = part_to_string(&back);
        assert_eq!(w1, w2);
        assert_eq!(strip_nulls(jv(&p)), strip_nulls(jv(&back)), "{w1}");
        let bare = PartDef { ty: "bare".into(), format: crate::model::FORMAT.into(), ..PartDef::default() };
        let b1 = part_to_string(&bare);
        assert_eq!(b1, part_to_string(&crate::parse_part(&b1).unwrap()));
    }

    #[test]
    fn empty_circuit_roundtrip() {
        let c = Circuit { format: crate::model::FORMAT.into(), ..Circuit::default() };
        let w = circuit_to_string(&c);
        assert_eq!(
            w,
            "{\n  \"format\": \"short-circ/2\",\n  \"parts\": [],\n  \"wires\": [],\n  \"labels\": [],\n  \"lib\": []\n}\n"
        );
        assert_eq!(w, circuit_to_string(&crate::parse_circuit(&w).unwrap()));
    }

    #[test]
    fn layout_one_entry_per_line_compact_stable_order() {
        let w = circuit_to_string(&big_circuit());
        assert!(w.contains("\n    {\"id\":\"R1\",\"type\":\"res\",\"props\":{\"label\":\"R\\\"1\",\"value\":\"330\"},\"hidden_label\":true,\"color\":\"red\",\"fill\":\"none\",\"label_color\":\"#abc\",\"schematic\":{\"at\":[20,10],\"rot\":90,\"mirror\":true},\"breadboard\":{\"at\":[14,-8]}},\n"), "{w}");
        assert!(w.contains("\n    {\"id\":\"w1\",\"view\":\"schematic\",\"pts\":[[1,2],[3,2],[3,-4]]},\n"));
        assert!(w.contains("{\"id\":\"j1\",\"view\":\"breadboard\",\"pts\":[[1,2],[3,4]],\"color\":\"red\"}"));
        assert!(w.contains("\n    {\"at\":[12,10],\"name\":\"VCC\"},\n"));
        assert!(w.contains("\n    {\"name\":\"VCC\",\"pins\":[\"B1.pos\",\"R1.a\"]},\n"));
        assert!(w.contains("\n    {\"pins\":[\"R1.b\"]}\n"));
        // top level key order
        let order: Vec<usize> =
            ["\"format\"", "\"title\"", "\"parts\"", "\"wires\"", "\"labels\"", "\"nets\"", "\"theme\"", "\"lib\""]
                .iter()
                .map(|k| w.find(&format!("\n  {k}")).unwrap_or_else(|| panic!("missing {k}")))
                .collect();
        assert!(order.windows(2).all(|p| p[0] < p[1]), "{order:?}");
    }

    #[test]
    fn part_layout_matches_spec_example() {
        let mut pins = BTreeMap::new();
        pins.insert("a".to_string(), pin([0, 1], Dir::Left));
        pins.insert("b".to_string(), pin([8, 1], Dir::Right));
        let p = PartDef {
            format: crate::model::FORMAT.into(),
            ty: "resistor".into(),
            name: "Resistor".into(),
            category: Some("passive".into()),
            pins: vec![PinDef { id: "a".into(), name: Some("A".into()) }, PinDef { id: "b".into(), name: Some("B".into()) }],
            schematic: Some(ViewDef {
                size: [8.0, 2.0],
                pins,
                shapes: vec![
                    Shape::Line { pts: vec![[0.0, 1.0], [2.0, 1.0]], style: Style::default() },
                    Shape::Rect { at: [2.0, 0.0], size: [4.0, 2.0], r: 0.0, style: Style::default() },
                    Shape::Text {
                        at: [4.0, -0.8],
                        text: "{ref}".into(),
                        size: 0.9,
                        anchor: Anchor::Middle,
                        style: Style::default(),
                    },
                ],
                svg: None,
            }),
            ..PartDef::default()
        };
        let want = "{
  \"format\": \"short-circ/2\",
  \"type\": \"resistor\",
  \"name\": \"Resistor\",
  \"category\": \"passive\",
  \"pins\": [
    {\"id\":\"a\",\"name\":\"A\"},
    {\"id\":\"b\",\"name\":\"B\"}
  ],
  \"schematic\": {
    \"size\": [8,2],
    \"pins\": {
      \"a\": {\"at\":[0,1],\"dir\":\"left\"},
      \"b\": {\"at\":[8,1],\"dir\":\"right\"}
    },
    \"shapes\": [
      {\"k\":\"line\",\"pts\":[[0,1],[2,1]]},
      {\"k\":\"rect\",\"at\":[2,0],\"size\":[4,2]},
      {\"k\":\"text\",\"at\":[4,-0.8],\"text\":\"{ref}\",\"size\":0.9,\"anchor\":\"middle\"}
    ]
  }
}
";
        assert_eq!(part_to_string(&p), want);
        assert_eq!(part_to_string(&crate::parse_part(want).unwrap()), want);
    }

    #[test]
    fn floats_and_odd_numbers_are_stable() {
        for (v, want) in [(8.0, "8"), (-0.0, "0"), (0.1, "0.1"), (-2.5, "-2.5"), (1e20, "1e+20"), (1.0 / 3.0, "0.3333333333333333")] {
            assert_eq!(num(v), want, "{v}");
        }
        let p = PartDef {
            format: crate::model::FORMAT.into(),
            ty: "f".into(),
            schematic: Some(ViewDef {
                size: [1.0 / 3.0, -0.0],
                shapes: vec![Shape::Circle { at: [1e-7, 123456.789], r: 1e21, style: style(None, None, Some(0.12)) }],
                ..ViewDef::default()
            }),
            ..PartDef::default()
        };
        let a = part_to_string(&p);
        let b = part_to_string(&crate::parse_part(&a).unwrap());
        assert_eq!(a, b);
    }

    #[test]
    fn output_is_valid_json_with_every_shape_kind() {
        let w = part_to_string(&big_part());
        let v: serde_json::Value = serde_json::from_str(&w).unwrap();
        let kinds: Vec<&str> =
            v["schematic"]["shapes"].as_array().unwrap().iter().map(|s| s["k"].as_str().unwrap()).collect();
        for k in ["line", "rect", "circle", "arc", "poly", "text"] {
            assert!(kinds.contains(&k), "{k}");
        }
    }

    #[test]
    fn long_svg_stays_one_line() {
        let mut p = big_part();
        p.breadboard.as_mut().unwrap().svg = Some(format!("<svg>{}</svg>", "<g/>\n".repeat(500)));
        let w = part_to_string(&p);
        assert_eq!(w.lines().filter(|l| l.contains("\"svg\"")).count(), 1);
        assert_eq!(w, part_to_string(&crate::parse_part(&w).unwrap()));
    }
}
