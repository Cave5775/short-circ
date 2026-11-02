//! Data model for `.circ` and `.part` files, format `short-circ/2`. See docs/FORMAT.md (the contract).
//! Pure data + serde; no logic beyond defaults and small accessors. Other modules build on this.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT: &str = "short-circ/2";
/// Files with this tag are upgraded on load by `migrate`; nothing else knows about v1.
pub const FORMAT_V1: &str = "short-circ/1";

/// Integer grid point. 1 unit = 0.1 inch. +y is down. Same grid in every view.
pub type Pt = [i32; 2];

fn default_format() -> String {
    FORMAT.to_string()
}

/// A layout of the circuit. Parts are logical; each view places them separately.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum View {
    /// Symbolic diagram.
    #[default]
    Schematic,
    /// Realistic diagram: real part artwork on breadboards.
    Breadboard,
}

impl View {
    pub const ALL: [View; 2] = [View::Schematic, View::Breadboard];

    /// Lowercase name used in files and on the command line.
    pub fn key(self) -> &'static str {
        match self {
            View::Schematic => "schematic",
            View::Breadboard => "breadboard",
        }
    }

    pub fn other(self) -> View {
        match self {
            View::Schematic => View::Breadboard,
            View::Breadboard => View::Schematic,
        }
    }

    pub fn parse(s: &str) -> Option<View> {
        match s.trim().to_ascii_lowercase().as_str() {
            "schematic" | "sch" => Some(View::Schematic),
            "breadboard" | "bb" => Some(View::Breadboard),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Circuit {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    /// Net labels. Schematic view only.
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Optional view-independent connection intent, see docs/FORMAT.md "Declared nets".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<NetDecl>,
    /// Inline component definitions; highest resolution priority.
    #[serde(default)]
    pub lib: Vec<PartDef>,
    /// Circuit-wide colour defaults, see docs/FORMAT.md "Colours".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
}

impl Circuit {
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }

    /// Wires drawn in `view`, in file order.
    pub fn wires_in(&self, view: View) -> impl Iterator<Item = &Wire> {
        self.wires.iter().filter(move |w| w.view == view)
    }
}

/// A declared net: pins that must be connected, named `PART.PIN` (e.g. `R1.a`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct NetDecl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub pins: Vec<String>,
}

/// Circuit-wide colour overrides per object kind. Each value is a colour spec (`#rgb`,
/// `#rrggbb`, `#rrggbbaa`, a preset name), `"none"` (draw nothing) or `"default"`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Theme {
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub grid: Option<String>,
    #[serde(default)]
    pub wire: Option<String>,
    #[serde(default)]
    pub part: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub pin: Option<String>,
    #[serde(default)]
    pub junction: Option<String>,
}

/// Where and how one part sits in one view. Transform: mirror, then clockwise rotation, then
/// translation by `at` (docs/FORMAT.md "Part transform").
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Placement {
    pub at: Pt,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rot: i32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mirror: bool,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl Placement {
    pub fn at(x: i32, y: i32) -> Placement {
        Placement { at: [x, y], rot: 0, mirror: false }
    }
}

/// A part instance. Logical (`id`, `type`, `props`, colours) plus one optional placement per
/// view; a part with no placement in a view does not appear there.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Part {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub props: BTreeMap<String, String>,
    #[serde(default)]
    pub hidden_label: bool,
    /// Stroke colour for every shape of this part without its own `stroke`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Fill colour for closed shapes of this part without their own `fill`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    /// Colour of the part's text shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schematic: Option<Placement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breadboard: Option<Placement>,
}

impl Part {
    pub fn placement(&self, view: View) -> Option<&Placement> {
        match view {
            View::Schematic => self.schematic.as_ref(),
            View::Breadboard => self.breadboard.as_ref(),
        }
    }

    pub fn placement_mut(&mut self, view: View) -> &mut Option<Placement> {
        match view {
            View::Schematic => &mut self.schematic,
            View::Breadboard => &mut self.breadboard,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Wire {
    pub id: String,
    /// The view this wire is drawn in. Wires never cross views.
    #[serde(default)]
    pub view: View,
    pub pts: Vec<Pt>,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Label {
    pub at: Pt,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A component: logical identity once (`pins`, `buses`), geometry per view.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PartDef {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub name: String,
    /// Free text; `"board"` marks a breadboard-like carrier (see docs/FORMAT.md "Boards").
    #[serde(default)]
    pub category: Option<String>,
    /// Logical pins in file order. Geometry lives in the views.
    #[serde(default)]
    pub pins: Vec<PinDef>,
    /// Groups of pin ids that are internally connected (breadboard strips, link jumpers).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schematic: Option<ViewDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breadboard: Option<ViewDef>,
    /// Reserved generator params, e.g. {"gen":"dip","pins":8}.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
    /// Free-form string metadata (title, family, source properties of imported parts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<BTreeMap<String, String>>,
}

impl PartDef {
    pub fn view(&self, view: View) -> Option<&ViewDef> {
        match view {
            View::Schematic => self.schematic.as_ref(),
            View::Breadboard => self.breadboard.as_ref(),
        }
    }

    pub fn is_board(&self) -> bool {
        self.category.as_deref().is_some_and(|c| c.eq_ignore_ascii_case("board"))
    }

    /// Pins that have a position in `view`, in `pins` order, with that position.
    pub fn placed_pins(&self, view: View) -> impl Iterator<Item = (&PinDef, &PinPos)> {
        let v = self.view(view);
        self.pins.iter().filter_map(move |p| v.and_then(|v| v.pins.get(&p.id)).map(|pos| (p, pos)))
    }
}

/// Geometry of a part in one view. Schematic: `shapes` (native, themable). Breadboard: `svg`
/// (a complete SVG document drawn at `size`), with `shapes` as fallback when `svg` is absent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ViewDef {
    /// Bounding box [w,h] in grid units from the local origin.
    pub size: [f64; 2],
    /// Pin positions by pin id. Positions are integers.
    #[serde(default)]
    pub pins: BTreeMap<String, PinPos>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<Shape>,
    /// Complete SVG document stretched over `size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinDef {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PinPos {
    pub at: Pt,
    /// Direction the pin leads out of the body (label placement). Ignored in the breadboard view.
    #[serde(default, skip_serializing_if = "is_default_dir")]
    pub dir: Dir,
}

fn is_default_dir(d: &Dir) -> bool {
    *d == Dir::default()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

/// Anything that can resolve a part type to its definition (`Library` implements it).
/// Editing, routing and validation take this so they are testable without file access.
pub trait PartLookup {
    fn get(&self, ty: &str) -> Option<&PartDef>;
}

/// Style fields shared by every shape.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Style {
    #[serde(default)]
    pub stroke: Option<String>,
    #[serde(default)]
    pub fill: Option<String>,
    #[serde(default)]
    pub width: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum Shape {
    Line {
        pts: Vec<[f64; 2]>,
        #[serde(flatten)]
        style: Style,
    },
    Rect {
        at: [f64; 2],
        size: [f64; 2],
        #[serde(default)]
        r: f64,
        #[serde(flatten)]
        style: Style,
    },
    Circle {
        at: [f64; 2],
        r: f64,
        #[serde(flatten)]
        style: Style,
    },
    Arc {
        at: [f64; 2],
        r: f64,
        from: f64,
        to: f64,
        #[serde(flatten)]
        style: Style,
    },
    Poly {
        pts: Vec<[f64; 2]>,
        #[serde(flatten)]
        style: Style,
    },
    Text {
        at: [f64; 2],
        text: String,
        #[serde(default = "default_text_size")]
        size: f64,
        #[serde(default)]
        anchor: Anchor,
        #[serde(flatten)]
        style: Style,
    },
}

fn default_text_size() -> f64 {
    1.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    #[default]
    Start,
    Middle,
    End,
}

/// Severity of a validation issue.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One validation finding. Stable JSON shape, see FORMAT.md "Report format".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub rule: String,
    pub severity: Severity,
    pub ids: Vec<String>,
    /// `None` for issues about the whole circuit.
    pub at: Option<Pt>,
    pub message: String,
    /// The view the issue was found in; `None` for cross-view or whole-circuit issues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<View>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn from_issues(issues: Vec<Issue>) -> Self {
        let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
        let warnings = issues.len() - errors;
        Report { ok: errors == 0, errors, warnings, issues }
    }
}
