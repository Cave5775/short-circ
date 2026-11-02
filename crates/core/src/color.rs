//! Colours: parsing, presets and the single resolver shared by SVG export and the viewer.
//!
//! A colour *spec* is a string: `#rgb`, `#rrggbb`, `#rrggbbaa` (case-insensitive hex), a preset
//! name (see [`preset_names`]), `"none"` (draw nothing) or `"default"` (use the theme default
//! for that kind of object). [`parse_color`] only accepts real colours; `"none"`, `"default"`,
//! the empty string and anything unparseable are handled by [`classify`] and [`resolve`].
//!
//! Precedence, highest first (docs/FORMAT.md "Colours"):
//! 1. the object's own colour (`wire.color`, `label.color`);
//! 2. for shapes inside a part: the shape's own `stroke` / `fill`, then `part.color` /
//!    `part.fill` / `part.label_color`;
//! 3. `circuit.theme` for that kind;
//! 4. the built-in light or dark default.
//!
//! A spec that is `"default"`, empty or unparseable is skipped and the next level decides.
//! `"none"` stops the search and means "draw nothing". Explicit colours are never adjusted for
//! the dark theme.

use crate::model::{Circuit, Label, Part, Style, Theme, Wire};

/// 8 bit RGBA, straight (non-premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba { r, g, b, a }
    }

    /// Lower-case `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// Linear blend of the RGB channels: `t = 0` is `self`, `t = 1` is `other`. Result is opaque.
    pub fn mix(self, other: Rgba, t: f32) -> Rgba {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
        Rgba::rgb(m(self.r, other.r), m(self.g, other.g), m(self.b, other.b))
    }
}

const PRESETS: [(&str, Rgba); 18] = [
    ("black", Rgba::rgb(0x00, 0x00, 0x00)),
    ("white", Rgba::rgb(0xff, 0xff, 0xff)),
    ("gray", Rgba::rgb(0x80, 0x80, 0x80)),
    ("silver", Rgba::rgb(0xc0, 0xc0, 0xc0)),
    ("red", Rgba::rgb(0xd9, 0x30, 0x25)),
    ("orange", Rgba::rgb(0xf5, 0x7c, 0x00)),
    ("yellow", Rgba::rgb(0xf2, 0xc2, 0x00)),
    ("green", Rgba::rgb(0x1e, 0x8e, 0x3e)),
    ("lime", Rgba::rgb(0x84, 0xcc, 0x16)),
    ("cyan", Rgba::rgb(0x00, 0xac, 0xc1)),
    ("blue", Rgba::rgb(0x1a, 0x73, 0xe8)),
    ("navy", Rgba::rgb(0x1a, 0x23, 0x7e)),
    ("purple", Rgba::rgb(0x8e, 0x24, 0xaa)),
    ("magenta", Rgba::rgb(0xd8, 0x1b, 0x9a)),
    ("pink", Rgba::rgb(0xf4, 0x8f, 0xb1)),
    ("brown", Rgba::rgb(0x8d, 0x5a, 0x2b)),
    ("teal", Rgba::rgb(0x00, 0x89, 0x7b)),
    ("gold", Rgba::rgb(0xd4, 0xa0, 0x17)),
];

/// The preset colour names and their values, in documentation order.
pub fn preset_names() -> &'static [(&'static str, Rgba)] {
    &PRESETS
}

/// Parse a real colour: hex (`#rgb`, `#rrggbb`, `#rrggbbaa`) or a preset name, case-insensitive,
/// surrounding whitespace ignored. `"grey"` is accepted as an alias of `"gray"`.
/// `"none"`, `"default"` and the empty string return `None` (see [`classify`]).
pub fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
        return match hex.len() {
            3 => Some(Rgba::rgb(v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
            6 => Some(Rgba::rgb(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
            8 => Some(Rgba::new(v(0, 2)?, v(2, 2)?, v(4, 2)?, v(6, 2)?)),
            _ => None,
        };
    }
    let name = s.to_ascii_lowercase();
    let name = if name == "grey" { "gray" } else { name.as_str() };
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// True for a colour, `"none"` or `"default"` (case-insensitive). The empty string is not valid.
pub fn is_valid_color_spec(s: &str) -> bool {
    let t = s.trim();
    t.eq_ignore_ascii_case("none") || t.eq_ignore_ascii_case("default") || parse_color(t).is_some()
}

/// What a colour spec asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spec {
    /// Absent, `"default"`, empty or unparseable: defer to the next precedence level.
    Unset,
    /// `"none"`: draw nothing.
    None,
    Color(Rgba),
}

pub fn classify(s: &str) -> Spec {
    if s.trim().eq_ignore_ascii_case("none") {
        Spec::None
    } else {
        parse_color(s).map_or(Spec::Unset, Spec::Color)
    }
}

/// Walk `chain` (highest precedence first; `None` entries are skipped) and return the first
/// colour, or `None` (draw nothing) at the first `"none"`; `fallback` if every level is unset.
pub fn resolve(chain: &[Option<&str>], fallback: Option<Rgba>) -> Option<Rgba> {
    for s in chain.iter().flatten() {
        match classify(s) {
            Spec::Color(c) => return Some(c),
            Spec::None => return None,
            Spec::Unset => {}
        }
    }
    fallback
}

/// Circuit-wide colours after applying `circuit.theme` over the built-in defaults.
/// `None` means "draw nothing" for that kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub background: Option<Rgba>,
    pub grid: Option<Rgba>,
    pub wire: Option<Rgba>,
    pub part: Option<Rgba>,
    pub label: Option<Rgba>,
    pub pin: Option<Rgba>,
    /// `Unset` follows the colour of the wires meeting at the junction.
    pub junction: Spec,
}

impl Palette {
    /// The built-in light or dark defaults, every kind set.
    pub fn builtin(dark: bool) -> Palette {
        if dark {
            Palette {
                dark,
                background: Some(Rgba::rgb(24, 26, 30)),
                grid: Some(Rgba::rgb(92, 98, 110)),
                wire: Some(Rgba::rgb(86, 200, 120)),
                part: Some(Rgba::rgb(222, 224, 228)),
                label: Some(Rgba::rgb(0xff, 0xb8, 0x6c)),
                pin: Some(Rgba::rgb(240, 170, 60)),
                junction: Spec::Unset,
            }
        } else {
            Palette {
                dark,
                background: Some(Rgba::rgb(250, 250, 247)),
                grid: Some(Rgba::rgb(168, 172, 182)),
                wire: Some(Rgba::rgb(20, 130, 60)),
                part: Some(Rgba::rgb(30, 32, 36)),
                label: Some(Rgba::rgb(0xa0, 0x40, 0x00)),
                pin: Some(Rgba::rgb(200, 110, 0)),
                junction: Spec::Unset,
            }
        }
    }

    /// Built-in defaults overridden by `theme` (precedence levels 3 and 4).
    pub fn new(theme: Option<&Theme>, dark: bool) -> Palette {
        let base = Palette::builtin(dark);
        let Some(t) = theme else { return base };
        let kind = |spec: &Option<String>, def: Option<Rgba>| resolve(&[spec.as_deref()], def);
        Palette {
            dark,
            background: kind(&t.background, base.background),
            grid: kind(&t.grid, base.grid),
            wire: kind(&t.wire, base.wire),
            part: kind(&t.part, base.part),
            label: kind(&t.label, base.label),
            pin: kind(&t.pin, base.pin),
            junction: t.junction.as_deref().map_or(Spec::Unset, classify),
        }
    }

    pub fn for_circuit(circuit: &Circuit, dark: bool) -> Palette {
        Palette::new(circuit.theme.as_ref(), dark)
    }
}

/// Wire colour: `wire.color`, else the palette.
pub fn wire_color(w: &Wire, pal: &Palette) -> Option<Rgba> {
    resolve(&[w.color.as_deref()], pal.wire)
}

/// Net label colour: `label.color`, else the palette.
pub fn label_color(l: &Label, pal: &Palette) -> Option<Rgba> {
    resolve(&[l.color.as_deref()], pal.label)
}

/// Outline colour of a shape inside `part`: shape `stroke`, `part.color`, theme part colour.
pub fn stroke_color(style: &Style, part: &Part, pal: &Palette) -> Option<Rgba> {
    resolve(&[style.stroke.as_deref(), part.color.as_deref()], pal.part)
}

/// Fill of a closed shape inside `part`: shape `fill`, `part.fill`, else no fill.
pub fn fill_color(style: &Style, part: &Part) -> Option<Rgba> {
    resolve(&[style.fill.as_deref(), part.fill.as_deref()], None)
}

/// Colour of a text shape inside `part`: shape `fill`, shape `stroke`, `part.label_color`, theme
/// part colour. For text a shape-level `"none"` is ignored (text is never invisible by accident).
pub fn text_color(style: &Style, part: &Part, pal: &Palette) -> Option<Rgba> {
    fn not_none(s: &Option<String>) -> Option<&str> {
        s.as_deref().filter(|v| !v.trim().eq_ignore_ascii_case("none"))
    }
    resolve(&[not_none(&style.fill), not_none(&style.stroke), part.label_color.as_deref()], pal.part)
}

/// Colour of the junction dot at `at`: `theme.junction` if set, else the colour of the first wire
/// (file order) that touches the point, else the palette wire colour.
pub fn junction_color(circuit: &Circuit, at: [i32; 2], pal: &Palette) -> Option<Rgba> {
    match pal.junction {
        Spec::Color(c) => return Some(c),
        Spec::None => return None,
        Spec::Unset => {}
    }
    let mut touched = false;
    for w in &circuit.wires {
        if wire_touches(w, at) {
            touched = true;
            if let Some(c) = wire_color(w, pal) {
                return Some(c);
            }
        }
    }
    if touched {
        None
    } else {
        pal.wire
    }
}

fn wire_touches(w: &Wire, p: [i32; 2]) -> bool {
    w.pts.contains(&p)
        || w.pts.windows(2).any(|s| {
            let (a, b) = (s[0], s[1]);
            let cross = (b[0] - a[0]) as i64 * (p[1] - a[1]) as i64 - (b[1] - a[1]) as i64 * (p[0] - a[0]) as i64;
            cross == 0
                && p[0] >= a[0].min(b[0])
                && p[0] <= a[0].max(b[0])
                && p[1] >= a[1].min(b[1])
                && p[1] <= a[1].max(b[1])
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_circuit;

    #[test]
    fn parses_hex_forms() {
        assert_eq!(parse_color("#f00"), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(parse_color("#0A5"), Some(Rgba::rgb(0, 170, 85)));
        assert_eq!(parse_color("#1b7F3b"), Some(Rgba::rgb(0x1b, 0x7f, 0x3b)));
        assert_eq!(parse_color("#11223344"), Some(Rgba::new(0x11, 0x22, 0x33, 0x44)));
        assert_eq!(parse_color("  #ABCDEF "), Some(Rgba::rgb(0xab, 0xcd, 0xef)));
    }

    #[test]
    fn parses_every_preset_case_insensitively() {
        assert_eq!(preset_names().len(), 18);
        for (name, c) in preset_names() {
            assert_eq!(parse_color(name), Some(*c), "{name}");
            assert_eq!(parse_color(&name.to_uppercase()), Some(*c), "{name}");
            assert_eq!(parse_color(&format!(" {name} ")), Some(*c));
            assert_eq!(parse_color(&c.to_hex()), Some(*c), "hex round trip {name}");
        }
        assert_eq!(parse_color("Grey"), parse_color("gray"));
        assert_eq!(parse_color("black"), Some(Rgba::rgb(0, 0, 0)));
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "", " ", "none", "default", "nope", "#", "#12", "#1234", "#12345", "#1234567", "#123456789", "#ggg",
            "#+1+2+3", "ff0000", "rgb(1,2,3)", "#\u{e9}\u{e9}\u{e9}", "re d",
        ] {
            assert_eq!(parse_color(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn valid_specs() {
        for ok in ["red", "#abc", "NONE", "Default", " none ", "#aabbccdd"] {
            assert!(is_valid_color_spec(ok), "{ok:?}");
        }
        for bad in ["", "reddish", "#12", "transparent"] {
            assert!(!is_valid_color_spec(bad), "{bad:?}");
        }
    }

    #[test]
    fn hex_output_and_mix() {
        assert_eq!(Rgba::rgb(1, 2, 255).to_hex(), "#0102ff");
        assert_eq!(Rgba::new(1, 2, 3, 128).to_hex(), "#01020380");
        let m = Rgba::rgb(0, 0, 0).mix(Rgba::rgb(100, 200, 50), 0.5);
        assert_eq!(m, Rgba::rgb(50, 100, 25));
    }

    #[test]
    fn classify_and_resolve() {
        assert_eq!(classify("default"), Spec::Unset);
        assert_eq!(classify(""), Spec::Unset);
        assert_eq!(classify("bogus"), Spec::Unset);
        assert_eq!(classify("None"), Spec::None);
        let fb = Some(Rgba::rgb(9, 9, 9));
        let red = Some(Rgba::rgb(0xd9, 0x30, 0x25));
        assert_eq!(resolve(&[], fb), fb);
        assert_eq!(resolve(&[None, Some("default"), Some("bogus")], fb), fb);
        assert_eq!(resolve(&[Some("red"), Some("blue")], fb), red);
        assert_eq!(resolve(&[Some("default"), Some("red")], fb), red);
        assert_eq!(resolve(&[Some("none"), Some("red")], fb), None);
    }

    fn circuit(src: &str) -> Circuit {
        parse_circuit(src).unwrap()
    }

    #[test]
    fn theme_overrides_builtin_defaults() {
        let c = circuit(
            r##"{"format":"short-circ/1","theme":{"background":"#102030","wire":"orange","pin":"none","part":"default","junction":"red"}}"##,
        );
        let light = Palette::for_circuit(&c, false);
        let base = Palette::builtin(false);
        assert_eq!(light.background, Some(Rgba::rgb(0x10, 0x20, 0x30)));
        assert_eq!(light.wire, parse_color("orange"));
        assert_eq!(light.pin, None);
        assert_eq!(light.part, base.part, "default keeps the built-in");
        assert_eq!(light.label, base.label);
        assert_eq!(light.junction, Spec::Color(parse_color("red").unwrap()));
        assert_ne!(Palette::builtin(true), base);
        assert_eq!(Palette::new(None, true), Palette::builtin(true));
    }

    #[test]
    fn precedence_wire_and_label() {
        let c = circuit(
            r##"{"format":"short-circ/1","theme":{"wire":"blue","label":"teal"},
            "wires":[{"id":"a","pts":[[0,0],[5,0]],"color":"red"},{"id":"b","pts":[[0,1],[5,1]]},
                     {"id":"c","pts":[[0,2],[5,2]],"color":"none"},{"id":"d","pts":[[0,3],[5,3]],"color":"default"},
                     {"id":"e","pts":[[0,4],[5,4]],"color":"oops"}],
            "labels":[{"at":[0,0],"name":"X","color":"#00ff00"},{"at":[1,0],"name":"Y"}]}"##,
        );
        let pal = Palette::for_circuit(&c, false);
        let blue = parse_color("blue");
        assert_eq!(wire_color(&c.wires[0], &pal), parse_color("red"));
        assert_eq!(wire_color(&c.wires[1], &pal), blue);
        assert_eq!(wire_color(&c.wires[2], &pal), None);
        assert_eq!(wire_color(&c.wires[3], &pal), blue);
        assert_eq!(wire_color(&c.wires[4], &pal), blue);
        assert_eq!(label_color(&c.labels[0], &pal), Some(Rgba::rgb(0, 255, 0)));
        assert_eq!(label_color(&c.labels[1], &pal), parse_color("teal"));
        let plain = Palette::builtin(true);
        assert_eq!(wire_color(&c.wires[1], &plain), plain.wire);
    }

    #[test]
    fn precedence_part_shapes() {
        let pal = Palette::new(Some(&Theme { part: Some("navy".into()), ..Theme::default() }), false);
        let navy = parse_color("navy");
        let red = parse_color("red");
        let blue = parse_color("blue");
        let gold = parse_color("gold");
        let plain = Part::default();
        let colored = Part {
            color: Some("red".into()),
            fill: Some("gold".into()),
            label_color: Some("blue".into()),
            ..Part::default()
        };
        let none_style = Style::default();
        let own = Style { stroke: Some("#00ff00".into()), fill: Some("#0000ff".into()), width: None };
        let no_fill = Style { fill: Some("none".into()), ..Style::default() };

        // theme.part is the fallback; part.color beats it; the shape's own stroke beats part.color.
        assert_eq!(stroke_color(&none_style, &plain, &pal), navy);
        assert_eq!(stroke_color(&none_style, &colored, &pal), red);
        assert_eq!(stroke_color(&own, &colored, &pal), Some(Rgba::rgb(0, 255, 0)));
        // Fill: nothing by default, part.fill applies, shape fill beats it, "none" cancels it.
        assert_eq!(fill_color(&none_style, &plain), None);
        assert_eq!(fill_color(&none_style, &colored), gold);
        assert_eq!(fill_color(&own, &colored), Some(Rgba::rgb(0, 0, 255)));
        assert_eq!(fill_color(&no_fill, &colored), None);
        // Text: label_color beats the theme; the shape's own fill/stroke beat label_color.
        assert_eq!(text_color(&none_style, &plain, &pal), navy);
        assert_eq!(text_color(&none_style, &colored, &pal), blue);
        assert_eq!(text_color(&own, &colored, &pal), Some(Rgba::rgb(0, 0, 255)));
        assert_eq!(text_color(&no_fill, &colored, &pal), blue, "shape-level none is ignored for text");
        // part "none" removes the outline.
        let hidden = Part { color: Some("none".into()), ..Part::default() };
        assert_eq!(stroke_color(&none_style, &hidden, &pal), None);
    }

    #[test]
    fn junction_follows_wire_colour_unless_theme_sets_it() {
        let src = |theme: &str| {
            circuit(&format!(
                r##"{{"format":"short-circ/1",{theme}"wires":[
                {{"id":"a","pts":[[0,0],[10,0]],"color":"red"}},{{"id":"b","pts":[[5,0],[5,5]]}}]}}"##
            ))
        };
        let c = src("");
        let pal = Palette::for_circuit(&c, false);
        assert_eq!(junction_color(&c, [5, 0], &pal), parse_color("red"), "T on the red wire");
        assert_eq!(junction_color(&c, [50, 50], &pal), pal.wire, "no wire there: default");
        let c = src(r##""theme":{"junction":"#123456"},"##);
        let pal = Palette::for_circuit(&c, false);
        assert_eq!(junction_color(&c, [5, 0], &pal), Some(Rgba::rgb(0x12, 0x34, 0x56)));
        let c = src(r##""theme":{"junction":"none"},"##);
        assert_eq!(junction_color(&c, [5, 0], &Palette::for_circuit(&c, false)), None);
    }

    #[test]
    fn old_files_still_load() {
        let c = circuit(r#"{"format":"short-circ/1","parts":[{"id":"R1","type":"resistor","at":[0,0]}]}"#);
        assert!(c.theme.is_none() && c.parts[0].color.is_none() && c.parts[0].fill.is_none());
        assert!(c.parts[0].label_color.is_none());
    }
}
