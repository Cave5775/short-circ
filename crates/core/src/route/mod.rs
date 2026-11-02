//! Autoroute and port: realise the nets of one view as placement and wires in the other.
//! Contract for docs/FORMAT.md "Autoroute and port". Pure: takes a circuit, returns a new one,
//! so the GUI can preview, commit as one undo step, or discard.
//!
//! Owned by the router stream; this file holds only the shared types until then.

use crate::model::{Circuit, PartLookup, View};

/// Where the nets to realise come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The netlist of the other view (e.g. porting the schematic to the breadboard).
    #[default]
    OtherView,
    /// The circuit's declared `nets`.
    Declared,
}

/// Which nets to route.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    /// Every net with two or more pins.
    #[default]
    All,
    /// Only nets that are not yet fully connected in the target view.
    Unrouted,
    /// Only nets containing one of these `PART.PIN` references.
    Only(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Prefer {
    /// Fewest bends first, then length.
    #[default]
    Bends,
    /// Shortest length first, then bends.
    Length,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JumperStyle {
    Straight,
    #[default]
    Orthogonal,
    Diagonal45,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JumperColor {
    /// A distinct preset colour per net.
    #[default]
    ByNet,
    /// One colour spec for every jumper.
    Single(String),
}

/// Every option is also a CLI flag (docs/FORMAT.md) and a toggle in the GUI dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteOptions {
    /// The view to produce. The source is the other view unless `source` is `Declared`.
    pub to: View,
    pub source: Source,
    pub scope: Scope,
    /// Place parts that are missing from the target view before routing.
    pub place_missing: bool,
    /// Keep wires already in the target view (they count as routed connections and as
    /// obstacles). When false they are removed first.
    pub keep_wires: bool,
    /// Schematic: allow wires to cross (perpendicular, no junction) instead of detouring.
    pub allow_crossings: bool,
    pub prefer: Prefer,
    /// Schematic: grid units kept free around part bodies of other parts.
    pub clearance: i32,
    /// Schematic: a net that cannot be routed gets same-name labels at its pins instead.
    pub label_fallback: bool,
    pub jumper_style: JumperStyle,
    pub jumper_color: JumperColor,
    /// Breadboard: longest jumper allowed, in grid units; `None` is unlimited.
    pub max_jumper_len: Option<i32>,
}

impl Default for RouteOptions {
    fn default() -> Self {
        RouteOptions {
            to: View::Breadboard,
            source: Source::OtherView,
            scope: Scope::All,
            place_missing: true,
            keep_wires: true,
            allow_crossings: false,
            prefer: Prefer::Bends,
            clearance: 1,
            label_fallback: true,
            jumper_style: JumperStyle::Orthogonal,
            jumper_color: JumperColor::ByNet,
            max_jumper_len: None,
        }
    }
}

/// A net that could not be realised and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFailure {
    /// Net name if it has one, else its first pin reference.
    pub net: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct RouteOutcome {
    /// The circuit with placements and wires added (or removed, see `keep_wires`).
    pub circuit: Circuit,
    /// Ids of parts that were placed.
    pub placed: Vec<String>,
    /// Ids of wires that were added.
    pub wires_added: Vec<String>,
    /// Nets realised completely.
    pub routed: usize,
    pub failures: Vec<RouteFailure>,
}

/// Port the circuit into `opts.to`. Never fails as a whole: unroutable nets are reported in
/// `failures` and the rest is still produced.
pub fn port(circuit: &Circuit, lib: &dyn PartLookup, opts: &RouteOptions) -> RouteOutcome {
    let _ = (lib, opts);
    RouteOutcome {
        circuit: circuit.clone(),
        placed: Vec::new(),
        wires_added: Vec::new(),
        routed: 0,
        failures: vec![RouteFailure { net: String::new(), reason: "autoroute is not implemented yet".into() }],
    }
}
