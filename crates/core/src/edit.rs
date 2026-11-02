//! Editing commands with grouped undo/redo (docs/FORMAT.md "Editing").
//!
//! - [`Edit`] is a command enum; [`apply`] runs one on a [`Circuit`]. A command either succeeds
//!   or leaves the circuit untouched (everything is validated before the first mutation).
//! - Moving, rotating, mirroring or re-placing a part **rubber-bands** wires: a wire's first and
//!   last point, in the same view, that sit exactly on a pin of a moved part before the change
//!   follow that pin to its new world position. Interior vertices, other views, labels and
//!   other points never move. Removing a part leaves its wires (the validator flags dangling).
//! - [`History`] keeps whole-circuit snapshots with groups (a drag, a paste or an autoroute run
//!   is one undo step) and dirty tracking.

use crate::model::{Circuit, Label, NetDecl, Part, PartLookup, Placement, Pt, View, Wire};
use crate::transform::pin_world;
use std::collections::VecDeque;

/// One editing command. Geometry commands carry the `View` they act in.
#[derive(Debug, Clone)]
pub enum Edit {
    /// Add a part; its id must be unused among parts and wires.
    AddPart(Part),
    /// Remove a part. Its wires and labels stay.
    RemovePart { id: String },
    /// Set (`Some`) or remove (`None`) the part's placement in `view`. Rubber-bands when the part
    /// was already placed in that view.
    SetPlacement { id: String, view: View, placement: Option<Placement> },
    /// Move the part's origin to `to` (rot and mirror unchanged).
    MovePart { id: String, view: View, to: Pt },
    /// Move several parts by the same delta in one command (wires between them move both ends).
    MoveBy { ids: Vec<String>, view: View, delta: [i32; 2] },
    /// Rotate clockwise by `degrees` (a multiple of 90, negative = counter-clockwise) about the
    /// part origin.
    Rotate { id: String, view: View, degrees: i32 },
    /// Toggle the mirror flag (mirror is applied before rotation).
    Mirror { id: String, view: View },
    /// Set (`Some`) or remove (`None`) a prop.
    SetProp { id: String, key: String, value: Option<String> },
    /// Set all three colour specs of a part; `None` clears that one.
    SetColors { id: String, color: Option<String>, fill: Option<String>, label_color: Option<String> },
    SetHiddenLabel { id: String, hidden: bool },
    /// Add a wire (>= 2 points, id unused among parts and wires).
    AddWire(Wire),
    RemoveWire { id: String },
    SetWirePts { id: String, pts: Vec<Pt> },
    SetWireColor { id: String, color: Option<String> },
    /// Move vertex `index` of a wire.
    MoveWirePoint { id: String, index: usize, to: Pt },
    /// Split a wire at a vertex (interior) or on a segment. The first half keeps `id`, the second
    /// gets a fresh unique id and follows it in file order; both keep view and colour.
    SplitWire { id: String, at: Pt },
    AddLabel(Label),
    MoveLabel { index: usize, to: Pt },
    RemoveLabel { index: usize },
    SetLabelColor { index: usize, color: Option<String> },
    SetDeclaredNets(Vec<NetDecl>),
    SetTitle(Option<String>),
    /// Rename a part or wire id; declared nets that mention the part follow.
    RenameId { old: String, new: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The id is empty or already used by a part or a wire.
    DuplicateId(String),
    UnknownPart(String),
    UnknownWire(String),
    /// The part has no placement in that view.
    NotPlaced { id: String, view: View },
    /// Rotation is not a multiple of 90 (for `SetPlacement`: not in {0, 90, 180, 270}).
    BadRotation(i32),
    /// A wire needs at least 2 points.
    TooFewPoints(String),
    BadIndex { what: &'static str, index: usize },
    /// `SplitWire`: the point is not on the wire.
    NotOnWire { id: String, at: Pt },
    /// `SplitWire`: the point is the first or last point, nothing to split off.
    AtEndpoint { id: String, at: Pt },
    /// An empty id was given.
    EmptyId,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::DuplicateId(id) => write!(f, "id \"{id}\" is empty or already in use"),
            EditError::UnknownPart(id) => write!(f, "no part \"{id}\""),
            EditError::UnknownWire(id) => write!(f, "no wire \"{id}\""),
            EditError::NotPlaced { id, view } => write!(f, "part \"{id}\" is not placed in the {} view", view.key()),
            EditError::BadRotation(r) => write!(f, "bad rotation {r} (must be a multiple of 90)"),
            EditError::TooFewPoints(id) => write!(f, "wire \"{id}\" needs at least 2 points"),
            EditError::BadIndex { what, index } => write!(f, "{what} index {index} out of range"),
            EditError::NotOnWire { id, at } => write!(f, "point ({},{}) is not on wire \"{id}\"", at[0], at[1]),
            EditError::AtEndpoint { id, at } => {
                write!(f, "point ({},{}) is an end of wire \"{id}\", nothing to split", at[0], at[1])
            }
            EditError::EmptyId => write!(f, "empty id"),
        }
    }
}

impl std::error::Error for EditError {}

// ---- ids ----------------------------------------------------------------------------------

fn id_in_use(c: &Circuit, id: &str) -> bool {
    c.parts.iter().any(|p| p.id == id) || c.wires.iter().any(|w| w.id == id)
}

/// Smallest unused `prefix + n` (n = 1, 2, ...) among part and wire ids: `R1`, `R2`, ... / `w1`...
pub fn fresh_id(c: &Circuit, prefix: &str) -> String {
    (1u32..)
        .map(|n| format!("{prefix}{n}"))
        .find(|id| !id_in_use(c, id))
        .expect("an unused id exists")
}

// ---- apply --------------------------------------------------------------------------------

fn part_idx(c: &Circuit, id: &str) -> Result<usize, EditError> {
    c.parts.iter().position(|p| p.id == id).ok_or_else(|| EditError::UnknownPart(id.to_string()))
}

fn wire_idx(c: &Circuit, id: &str) -> Result<usize, EditError> {
    c.wires.iter().position(|w| w.id == id).ok_or_else(|| EditError::UnknownWire(id.to_string()))
}

fn placed(c: &Circuit, id: &str, view: View) -> Result<(usize, Placement), EditError> {
    let i = part_idx(c, id)?;
    match c.parts[i].placement(view) {
        Some(pl) => Ok((i, pl.clone())),
        None => Err(EditError::NotPlaced { id: id.to_string(), view }),
    }
}

fn add(a: Pt, d: [i32; 2]) -> Pt {
    [a[0].saturating_add(d[0]), a[1].saturating_add(d[1])]
}

/// World pin positions of the part with placement `pl` in `view`, `(pin id, position)`.
fn pin_positions(lib: &dyn PartLookup, ty: &str, view: View, pl: &Placement) -> Vec<(String, Pt)> {
    match lib.get(ty).and_then(|d| d.view(view)) {
        Some(vd) => vd.pins.iter().map(|(id, pos)| (id.clone(), pin_world(pl, pos))).collect(),
        None => Vec::new(),
    }
}

/// Give parts new placements in `view` and rubber-band wire ends. `changes` is `(part index, new
/// placement)`, all validated by the caller.
fn relocate(c: &mut Circuit, lib: &dyn PartLookup, view: View, changes: Vec<(usize, Option<Placement>)>) {
    // (old world pos -> new world pos) for every pin of a part that stays placed.
    let mut moves: Vec<(Pt, Pt)> = Vec::new();
    for (i, new) in &changes {
        let part = &c.parts[*i];
        if let (Some(old), Some(new)) = (part.placement(view), new) {
            let before = pin_positions(lib, &part.ty, view, old);
            let after = pin_positions(lib, &part.ty, view, new);
            for ((_, b), (_, a)) in before.iter().zip(after.iter()) {
                if b != a && !moves.iter().any(|(ob, _)| ob == b) {
                    moves.push((*b, *a));
                }
            }
        }
    }
    for (i, new) in changes {
        *c.parts[i].placement_mut(view) = new;
    }
    if moves.is_empty() {
        return;
    }
    let target = |p: Pt| moves.iter().find(|(o, _)| *o == p).map(|(_, n)| *n);
    for w in c.wires.iter_mut().filter(|w| w.view == view) {
        let n = w.pts.len();
        if n < 2 {
            continue;
        }
        // Both ends are looked up against the old positions, never chained.
        let first = target(w.pts[0]);
        let last = target(w.pts[n - 1]);
        if let Some(p) = first {
            w.pts[0] = p;
        }
        if let Some(p) = last {
            w.pts[n - 1] = p;
        }
    }
}

fn norm_rot(r: i32) -> Result<i32, EditError> {
    if r % 90 != 0 {
        return Err(EditError::BadRotation(r));
    }
    Ok(r.rem_euclid(360))
}

fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    let (ax, ay, bx, by, px, py) =
        (a[0] as i64, a[1] as i64, b[0] as i64, b[1] as i64, p[0] as i64, p[1] as i64);
    (bx - ax) * (py - ay) - (by - ay) * (px - ax) == 0
        && px >= ax.min(bx)
        && px <= ax.max(bx)
        && py >= ay.min(by)
        && py <= ay.max(by)
}

/// Apply one command. On error the circuit is unchanged.
pub fn apply(c: &mut Circuit, lib: &dyn PartLookup, edit: Edit) -> Result<(), EditError> {
    match edit {
        Edit::AddPart(p) => {
            if p.id.is_empty() || id_in_use(c, &p.id) {
                return Err(EditError::DuplicateId(p.id));
            }
            for v in View::ALL {
                if let Some(pl) = p.placement(v) {
                    norm_rot(pl.rot)?;
                }
            }
            c.parts.push(p);
        }
        Edit::RemovePart { id } => {
            let i = part_idx(c, &id)?;
            c.parts.remove(i);
        }
        Edit::SetPlacement { id, view, placement } => {
            let i = part_idx(c, &id)?;
            if let Some(pl) = &placement {
                if !matches!(pl.rot, 0 | 90 | 180 | 270) {
                    return Err(EditError::BadRotation(pl.rot));
                }
            }
            relocate(c, lib, view, vec![(i, placement)]);
        }
        Edit::MovePart { id, view, to } => {
            let (i, mut pl) = placed(c, &id, view)?;
            pl.at = to;
            relocate(c, lib, view, vec![(i, Some(pl))]);
        }
        Edit::MoveBy { ids, view, delta } => {
            let mut changes: Vec<(usize, Option<Placement>)> = Vec::new();
            for id in &ids {
                let (i, mut pl) = placed(c, id, view)?;
                if changes.iter().any(|(j, _)| *j == i) {
                    continue;
                }
                pl.at = add(pl.at, delta);
                changes.push((i, Some(pl)));
            }
            relocate(c, lib, view, changes);
        }
        Edit::Rotate { id, view, degrees } => {
            let step = norm_rot(degrees)?;
            let (i, mut pl) = placed(c, &id, view)?;
            pl.rot = (pl.rot + step).rem_euclid(360);
            relocate(c, lib, view, vec![(i, Some(pl))]);
        }
        Edit::Mirror { id, view } => {
            let (i, mut pl) = placed(c, &id, view)?;
            pl.mirror = !pl.mirror;
            relocate(c, lib, view, vec![(i, Some(pl))]);
        }
        Edit::SetProp { id, key, value } => {
            let i = part_idx(c, &id)?;
            match value {
                Some(v) => {
                    c.parts[i].props.insert(key, v);
                }
                None => {
                    c.parts[i].props.remove(&key);
                }
            }
        }
        Edit::SetColors { id, color, fill, label_color } => {
            let i = part_idx(c, &id)?;
            let p = &mut c.parts[i];
            p.color = color;
            p.fill = fill;
            p.label_color = label_color;
        }
        Edit::SetHiddenLabel { id, hidden } => {
            let i = part_idx(c, &id)?;
            c.parts[i].hidden_label = hidden;
        }
        Edit::AddWire(w) => {
            if w.id.is_empty() || id_in_use(c, &w.id) {
                return Err(EditError::DuplicateId(w.id));
            }
            if w.pts.len() < 2 {
                return Err(EditError::TooFewPoints(w.id));
            }
            c.wires.push(w);
        }
        Edit::RemoveWire { id } => {
            let i = wire_idx(c, &id)?;
            c.wires.remove(i);
        }
        Edit::SetWirePts { id, pts } => {
            let i = wire_idx(c, &id)?;
            if pts.len() < 2 {
                return Err(EditError::TooFewPoints(id));
            }
            c.wires[i].pts = pts;
        }
        Edit::SetWireColor { id, color } => {
            let i = wire_idx(c, &id)?;
            c.wires[i].color = color;
        }
        Edit::MoveWirePoint { id, index, to } => {
            let i = wire_idx(c, &id)?;
            match c.wires[i].pts.get_mut(index) {
                Some(p) => *p = to,
                None => return Err(EditError::BadIndex { what: "wire point", index }),
            }
        }
        Edit::SplitWire { id, at } => {
            let i = wire_idx(c, &id)?;
            let pts = c.wires[i].pts.clone();
            let n = pts.len();
            let (first, second): (Vec<Pt>, Vec<Pt>) = if let Some(k) = pts.iter().position(|p| *p == at) {
                if k == 0 || k == n - 1 {
                    return Err(EditError::AtEndpoint { id, at });
                }
                (pts[..=k].to_vec(), pts[k..].to_vec())
            } else if let Some(s) = (0..n - 1).find(|&s| on_segment(pts[s], pts[s + 1], at)) {
                let mut a = pts[..=s].to_vec();
                a.push(at);
                let mut b = vec![at];
                b.extend_from_slice(&pts[s + 1..]);
                (a, b)
            } else {
                return Err(EditError::NotOnWire { id, at });
            };
            let new_id = fresh_id(c, "w");
            let mut w2 = c.wires[i].clone();
            w2.id = new_id;
            w2.pts = second;
            c.wires[i].pts = first;
            c.wires.insert(i + 1, w2);
        }
        Edit::AddLabel(l) => c.labels.push(l),
        Edit::MoveLabel { index, to } => match c.labels.get_mut(index) {
            Some(l) => l.at = to,
            None => return Err(EditError::BadIndex { what: "label", index }),
        },
        Edit::RemoveLabel { index } => {
            if index >= c.labels.len() {
                return Err(EditError::BadIndex { what: "label", index });
            }
            c.labels.remove(index);
        }
        Edit::SetLabelColor { index, color } => match c.labels.get_mut(index) {
            Some(l) => l.color = color,
            None => return Err(EditError::BadIndex { what: "label", index }),
        },
        Edit::SetDeclaredNets(nets) => c.nets = nets,
        Edit::SetTitle(t) => c.title = t,
        Edit::RenameId { old, new } => {
            if new.is_empty() {
                return Err(EditError::EmptyId);
            }
            let part = c.parts.iter().position(|p| p.id == old);
            let wire = c.wires.iter().position(|w| w.id == old);
            if part.is_none() && wire.is_none() {
                return Err(EditError::UnknownPart(old));
            }
            if old == new {
                return Ok(());
            }
            if id_in_use(c, &new) {
                return Err(EditError::DuplicateId(new));
            }
            if let Some(i) = part {
                c.parts[i].id = new.clone();
                let prefix = format!("{old}.");
                for net in &mut c.nets {
                    for pin in &mut net.pins {
                        if let Some(rest) = pin.strip_prefix(&prefix) {
                            *pin = format!("{new}.{rest}");
                        }
                    }
                }
            }
            if let Some(i) = wire {
                c.wires[i].id = new;
            }
        }
    }
    Ok(())
}

// ---- tidy ---------------------------------------------------------------------------------

/// Remove consecutive duplicate points and merge collinear consecutive segments of every wire in
/// `view`; wires left with fewer than 2 distinct points are deleted. A vertex that another wire
/// or a label of the schematic touches is kept (merging it would change connectivity). Use
/// [`tidy_with`] to also protect part pins.
pub fn tidy(c: &mut Circuit, view: View) {
    tidy_impl(c, None, view);
}

/// [`tidy`] that additionally keeps vertices lying on a pin of a placed part (schematic only).
pub fn tidy_with(c: &mut Circuit, lib: &dyn PartLookup, view: View) {
    tidy_impl(c, Some(lib), view);
}

fn collinear_same_dir(a: Pt, b: Pt, c: Pt) -> bool {
    let (abx, aby) = ((b[0] as i64) - (a[0] as i64), (b[1] as i64) - (a[1] as i64));
    let (bcx, bcy) = ((c[0] as i64) - (b[0] as i64), (c[1] as i64) - (b[1] as i64));
    abx * bcy - aby * bcx == 0 && abx * bcx + aby * bcy > 0
}

fn tidy_impl(c: &mut Circuit, lib: Option<&dyn PartLookup>, view: View) {
    // Points that must stay vertices: schematic only (breadboard interior points connect nothing).
    let mut anchors: Vec<Pt> = Vec::new();
    if view == View::Schematic {
        anchors.extend(c.labels.iter().map(|l| l.at));
        if let Some(lib) = lib {
            for p in &c.parts {
                if let Some(pl) = p.placement(view) {
                    anchors.extend(pin_positions(lib, &p.ty, view, pl).into_iter().map(|(_, q)| q));
                }
            }
        }
    }
    let others: Vec<(usize, Vec<Pt>)> = c
        .wires
        .iter()
        .enumerate()
        .filter(|(_, w)| w.view == view)
        .map(|(i, w)| (i, w.pts.clone()))
        .collect();
    for (wi, w) in c.wires.iter_mut().enumerate() {
        if w.view != view {
            continue;
        }
        let mut pts: Vec<Pt> = Vec::with_capacity(w.pts.len());
        for p in &w.pts {
            if pts.last() != Some(p) {
                pts.push(*p);
            }
        }
        if view == View::Schematic {
            let mut out: Vec<Pt> = Vec::with_capacity(pts.len());
            for (k, p) in pts.iter().enumerate() {
                let interior = k > 0 && k + 1 < pts.len();
                if interior {
                    let prev = *out.last().expect("first point kept");
                    let touched = anchors.contains(p)
                        || others.iter().any(|(j, o)| *j != wi && o.contains(p));
                    if !touched && collinear_same_dir(prev, *p, pts[k + 1]) {
                        continue;
                    }
                }
                out.push(*p);
            }
            pts = out;
        } else {
            let mut out: Vec<Pt> = Vec::with_capacity(pts.len());
            for (k, p) in pts.iter().enumerate() {
                if k > 0 && k + 1 < pts.len() && collinear_same_dir(*out.last().expect("first kept"), *p, pts[k + 1]) {
                    continue;
                }
                out.push(*p);
            }
            pts = out;
        }
        w.pts = pts;
    }
    c.wires.retain(|w| w.view != view || w.pts.len() >= 2);
}

// ---- history ------------------------------------------------------------------------------

const MAX_UNDO: usize = 200;

/// Undo/redo over whole-circuit snapshots, with groups and dirty tracking.
#[derive(Debug, Default)]
pub struct History {
    /// `(snapshot before the step, id of that state)`, oldest first.
    undo: VecDeque<(Circuit, u64)>,
    redo: Vec<(Circuit, u64)>,
    /// Id of the current state; every applied edit gives a new one.
    cur: u64,
    next: u64,
    /// `cur` when the document was last saved (`None`: that state is gone).
    saved: Option<u64>,
    group_depth: u32,
    /// The open group already took its snapshot.
    group_snapped: bool,
}

impl History {
    pub fn new() -> Self {
        History { cur: 0, next: 1, saved: Some(0), ..Default::default() }
    }

    fn fresh(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Apply `edit`, recording a snapshot first (once per group). On error nothing is recorded
    /// and the circuit is unchanged.
    pub fn apply(&mut self, circuit: &mut Circuit, lib: &dyn PartLookup, edit: Edit) -> Result<(), EditError> {
        let snap = circuit.clone();
        if let Err(e) = apply(circuit, lib, edit) {
            *circuit = snap;
            return Err(e);
        }
        let need_snapshot = self.group_depth == 0 || !self.group_snapped;
        if need_snapshot {
            self.undo.push_back((snap, self.cur));
            if self.undo.len() > MAX_UNDO {
                self.undo.pop_front();
            }
            if self.group_depth > 0 {
                self.group_snapped = true;
            }
        }
        self.redo.clear();
        self.cur = self.fresh();
        Ok(())
    }

    /// Start a group: until the matching `end_group`, all edits are one undo step. Nestable.
    pub fn begin_group(&mut self) {
        if self.group_depth == 0 {
            self.group_snapped = false;
        }
        self.group_depth += 1;
    }

    pub fn end_group(&mut self) {
        self.group_depth = self.group_depth.saturating_sub(1);
        if self.group_depth == 0 {
            self.group_snapped = false;
        }
    }

    pub fn undo(&mut self, circuit: &mut Circuit) -> bool {
        self.group_depth = 0;
        self.group_snapped = false;
        match self.undo.pop_back() {
            Some((snap, id)) => {
                let now = std::mem::replace(circuit, snap);
                self.redo.push((now, self.cur));
                self.cur = id;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, circuit: &mut Circuit) -> bool {
        self.group_depth = 0;
        self.group_snapped = false;
        match self.redo.pop() {
            Some((snap, id)) => {
                let now = std::mem::replace(circuit, snap);
                self.undo.push_back((now, self.cur));
                self.cur = id;
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Forget all history (e.g. after loading a file). The dirty flag is left as it is; call
    /// [`History::mark_clean`] afterwards when the new content is the saved state.
    pub fn clear(&mut self) {
        let was_dirty = self.dirty();
        self.undo.clear();
        self.redo.clear();
        self.group_depth = 0;
        self.group_snapped = false;
        self.cur = self.fresh();
        self.saved = if was_dirty { None } else { Some(self.cur) };
    }

    /// The current state differs from the one at the last `mark_clean` (or from creation).
    pub fn dirty(&self) -> bool {
        self.saved != Some(self.cur)
    }

    /// Record the current state as the saved one.
    pub fn mark_clean(&mut self) {
        self.saved = Some(self.cur);
    }
}

#[cfg(test)]
mod tests;
