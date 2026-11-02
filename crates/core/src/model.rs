//! Data model for `.circ` and `.part` files. See docs/FORMAT.md (the contract).
//! Pure data + serde; no logic beyond defaults. Other modules build on this.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT: &str = "short-circ/1";

/// Integer grid point. 1 unit = 0.1 inch. +y is down.
pub type Pt = [i32; 2];

fn default_format() -> String {
    FORMAT.to_string()
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
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Inline component definitions; highest resolution priority.
    #[serde(default)]
    pub lib: Vec<PartDef>,
    /// Circuit-wide colour defaults, see docs/FORMAT.md "Colours".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Part {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub at: Pt,
    #[serde(default)]
    pub rot: i32,
    #[serde(default)]
    pub mirror: bool,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Wire {
    pub id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PartDef {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    /// Bounding box [w,h] from local origin.
    pub size: [f64; 2],
    #[serde(default)]
    pub pins: Vec<PinDef>,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    /// Reserved generator params, e.g. {"gen":"dip","pins":8}.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
    /// Raw SVG path data, converted to shapes at load if `shapes` is empty.
    #[serde(default)]
    pub svg: Option<String>,
    /// Free-form string metadata (title, family, source properties of imported parts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinDef {
    pub id: String,
    pub at: Pt,
    #[serde(default)]
    pub dir: Dir,
    #[serde(default)]
    pub name: Option<String>,
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
    pub at: Option<Pt>,
    pub message: String,
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
