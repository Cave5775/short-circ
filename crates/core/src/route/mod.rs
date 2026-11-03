//! Autoroute and port: realise the nets of one view as placement and wires in the other.
//! Contract: docs/FORMAT.md "Autoroute and port". Pure: takes a circuit, returns a new one,
//! so the GUI can preview, commit as one undo step, or discard.
//!
//! Layout of this module:
//! - `nets`: the nets to realise (other view or declared) restricted to pins that can exist in
//!   the target view,
//! - `sch`: schematic target (grid placement of missing parts, orthogonal A* routing with
//!   crossings and label fallback),
//! - `bb`: breadboard target (placement of parts onto board holes without shorts, jumpers),
//! - `cli`: the `--autoroute` command line front end.

pub mod bb;
pub mod cli;
pub mod nets;
pub mod sch;

#[cfg(test)]
mod tests;

use crate::model::{Circuit, PartLookup, Placement, Pt, View, ViewDef};
use crate::transform::world_bbox;
use std::collections::{HashMap, HashSet};

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

/// Running totals shared by the two targets.
#[derive(Debug, Default)]
pub(crate) struct Acc {
    pub placed: Vec<String>,
    pub wires_added: Vec<String>,
    pub routed: usize,
    pub failures: Vec<RouteFailure>,
}

/// Largest `--clearance` (grid units); larger values only waste time and could overflow.
pub const MAX_CLEARANCE: i32 = 100;
/// Largest `--max-jumper` (grid units); the coordinate range is smaller than this anyway.
pub const MAX_JUMPER: i32 = 1_000_000;

/// Port the circuit into `opts.to`. Never fails as a whole: unroutable nets are reported in
/// `failures` and the rest is still produced. Option values outside their documented range
/// (`clearance` 0..=[`MAX_CLEARANCE`], `max_jumper_len` at most [`MAX_JUMPER`]) are clamped, so no
/// caller can overflow the grid arithmetic.
pub fn port(circuit: &Circuit, lib: &dyn PartLookup, opts: &RouteOptions) -> RouteOutcome {
    let clamped = RouteOptions {
        clearance: opts.clearance.clamp(0, MAX_CLEARANCE),
        max_jumper_len: opts.max_jumper_len.map(|m| m.min(MAX_JUMPER)),
        ..opts.clone()
    };
    let opts = &clamped;
    let mut c = circuit.clone();
    let mut acc = Acc::default();
    if !opts.keep_wires {
        if opts.to == View::Schematic && !c.labels.is_empty() {
            // Labels that sat on a removed wire would be debris; labels on pins stay.
            let before = crate::netlist::Netlist::build(&c, lib, View::Schematic);
            c.wires.retain(|w| w.view != opts.to);
            let after = crate::netlist::Netlist::build(&c, lib, View::Schematic);
            let keep: Vec<bool> =
                c.labels.iter().map(|l| before.net_at(l.at).is_none() || after.net_at(l.at).is_some()).collect();
            let mut k = keep.into_iter();
            c.labels.retain(|_| k.next().unwrap_or(true));
        } else {
            c.wires.retain(|w| w.view != opts.to);
        }
    }
    let nets = nets::source_nets(&c, lib, opts, &mut acc.failures);
    match opts.to {
        View::Schematic => sch::run(&mut c, lib, opts, nets, &mut acc),
        View::Breadboard => bb::run(&mut c, lib, opts, nets, &mut acc),
    }
    RouteOutcome { circuit: c, placed: acc.placed, wires_added: acc.wires_added, routed: acc.routed, failures: acc.failures }
}

// ---- shared helpers ----------------------------------------------------------------------

/// Report the usable pins of `net` that have no placement in `view` (a net with such a pin is
/// never counted as routed). A part for which a failure was already reported (named by its id,
/// e.g. "no free spot on the board") is not reported again. Returns whether any pin was unplaced.
pub(crate) fn report_unplaced<'a>(
    acc: &mut Acc,
    net: &nets::SrcNet,
    view: View,
    unplaced: impl IntoIterator<Item = &'a (String, String)>,
) -> bool {
    let mut any = false;
    for (part, pin) in unplaced {
        any = true;
        if acc.failures.iter().any(|f| f.net == *part) {
            continue;
        }
        acc.failures.push(RouteFailure { net: net.display(), reason: format!("{part}.{pin} is not placed in the {} view", view.key()) });
    }
    any
}

/// Integer rectangle on the lattice (closed on all sides for point tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn inflate(self, d: i32) -> Rect {
        Rect { x0: self.x0 - d, y0: self.y0 - d, x1: self.x1 + d, y1: self.y1 + d }
    }

    /// Overlap with positive area.
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    pub fn contains(&self, p: Pt) -> bool {
        p[0] >= self.x0 && p[0] <= self.x1 && p[1] >= self.y0 && p[1] <= self.y1
    }
}

/// World bounding box of a placed view rounded outwards to the lattice.
pub(crate) fn part_rect(pl: &Placement, vd: &ViewDef) -> Rect {
    let (mn, mx) = world_bbox(pl, vd);
    Rect { x0: mn[0].floor() as i32, y0: mn[1].floor() as i32, x1: mx[0].ceil() as i32, y1: mx[1].ceil() as i32 }
}

/// Same sequence as `edit::fresh_id` (smallest unused `prefix + n` among part and wire ids) but
/// without rescanning the circuit for every id.
pub(crate) struct IdGen {
    used: HashSet<String>,
    next: HashMap<String, u32>,
}

impl IdGen {
    pub fn new(c: &Circuit) -> IdGen {
        let used = c.parts.iter().map(|p| p.id.clone()).chain(c.wires.iter().map(|w| w.id.clone())).collect();
        IdGen { used, next: HashMap::new() }
    }

    pub fn fresh(&mut self, prefix: &str) -> String {
        let n = self.next.entry(prefix.to_string()).or_insert(1);
        loop {
            let id = format!("{prefix}{n}");
            *n += 1;
            if self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

/// Part indexes by id (first occurrence wins).
pub(crate) fn part_index(c: &Circuit) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for (i, p) in c.parts.iter().enumerate() {
        m.entry(p.id.clone()).or_insert(i);
    }
    m
}
