//! The Wire tool.
//!
//! A click starts a wire at the snapped point, every further click adds a vertex. The wire ends on
//! a double click, Enter, or a click on a pin or on an existing wire point; Esc cancels,
//! Backspace takes the last vertex back.
//!
//! Schematic: every segment is horizontal or vertical. Moving the cursor away from the last
//! vertex previews an L of two segments. **Rule:** the L runs horizontal-first when the cursor is
//! at least as far away horizontally as vertically (`|dx| >= |dy|`), vertical-first otherwise;
//! Shift flips the choice. A path that would run over an existing wire (or itself) for a positive
//! length, or pass through a pin without a vertex there, is shown red and its click is refused
//! (the validator would call it `wire-overlap` / `wire-through-pin`). Ending on the interior of
//! another wire is a legal T-junction.
//!
//! Breadboard: segments go in any direction between grid points; a wire ends on a pin or hole
//! (a green end dot) or on a free point (orange).

use super::hit::{on_segment, pin_points};
use super::{snap, EditCtx, Effect, EditorState, Mods};
use short_circ_core::edit::{fresh_id, tidy_with, Edit};
use short_circ_core::{Circuit, PartLookup, Pt, View, Wire};

#[derive(Debug, Clone)]
pub struct Draft {
    /// Committed vertices.
    pub pts: Vec<Pt>,
    /// Snapped pointer.
    pub cursor: Pt,
    /// Shift is held: flip the L.
    pub flip: bool,
}

/// What the canvas draws for the wire being drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Committed vertices plus the route to the cursor.
    pub pts: Vec<Pt>,
    /// Why a click would be refused (draw it red).
    pub error: Option<String>,
    pub start_on_pin: bool,
    pub end_on_pin: bool,
}

/// Points to add after `a` to reach `b` (the last one is `b`; empty when they are equal).
/// Schematic: straight when aligned, else an L, horizontal-first when `|dx| >= |dy|` (Shift
/// flips). Breadboard: straight.
pub fn route(view: View, a: Pt, b: Pt, flip: bool) -> Vec<Pt> {
    if a == b {
        return Vec::new();
    }
    if view == View::Breadboard || a[0] == b[0] || a[1] == b[1] {
        return vec![b];
    }
    let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
    let horizontal_first = (dx >= dy) != flip;
    let corner = if horizontal_first { [b[0], a[1]] } else { [a[0], b[1]] };
    vec![corner, b]
}

fn cross(u: [i64; 2], v: [i64; 2]) -> i64 {
    u[0] * v[1] - u[1] * v[0]
}

fn sub(a: Pt, b: Pt) -> [i64; 2] {
    [a[0] as i64 - b[0] as i64, a[1] as i64 - b[1] as i64]
}

/// Two non-degenerate segments are collinear and share a stretch of positive length.
pub fn overlap(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    if a == b || c == d {
        return false;
    }
    let u = sub(b, a);
    if cross(u, sub(c, a)) != 0 || cross(u, sub(d, a)) != 0 {
        return false;
    }
    let axis = if u[0] != 0 { 0 } else { 1 };
    let (a0, a1) = (a[axis].min(b[axis]), a[axis].max(b[axis]));
    let (c0, c1) = (c[axis].min(d[axis]), c[axis].max(d[axis]));
    a0.max(c0) < a1.min(c1)
}

/// Is `pts` an acceptable wire here? Only the schematic is restricted (see the module doc).
pub fn check_path(c: &Circuit, lib: &dyn PartLookup, view: View, pts: &[Pt]) -> Result<(), String> {
    if view != View::Schematic {
        return Ok(());
    }
    let segs: Vec<(Pt, Pt)> = pts.windows(2).map(|w| (w[0], w[1])).filter(|(a, b)| a != b).collect();
    let pins = pin_points(c, lib, view);
    for (a, b) in &segs {
        for pin in &pins {
            if pin.at != *a && pin.at != *b && on_segment(*a, *b, pin.at) {
                return Err(format!("it would pass through pin {}.{}", c.parts[pin.part].id, pin.pin));
            }
        }
        for w in c.wires_in(view) {
            for s in w.pts.windows(2) {
                if overlap(*a, *b, s[0], s[1]) {
                    return Err(format!("it would overlap wire {}", w.id));
                }
            }
        }
    }
    for i in 0..segs.len() {
        for j in i + 1..segs.len() {
            if overlap(segs[i].0, segs[i].1, segs[j].0, segs[j].1) {
                return Err("it would run over itself".to_string());
            }
        }
    }
    Ok(())
}

fn pin_at_point(c: &Circuit, lib: &dyn PartLookup, view: View, p: Pt) -> bool {
    pin_points(c, lib, view).iter().any(|q| q.at == p)
}

fn on_any_wire(c: &Circuit, view: View, p: Pt) -> bool {
    c.wires_in(view).any(|w| w.pts.windows(2).any(|s| on_segment(s[0], s[1], p)) || w.pts.first() == Some(&p))
}

/// A point a label or a wire can attach to: a pin, or a point on a wire.
pub fn point_is_connectable(c: &Circuit, lib: &dyn PartLookup, view: View, p: Pt) -> bool {
    pin_at_point(c, lib, view, p) || on_any_wire(c, view, p)
}

/// A click here ends the wire: a pin or hole, or an existing wire (schematic: any point of it,
/// breadboard: one of its end points).
pub fn is_terminal(c: &Circuit, lib: &dyn PartLookup, view: View, p: Pt) -> bool {
    if pin_at_point(c, lib, view, p) {
        return true;
    }
    match view {
        View::Schematic => on_any_wire(c, view, p),
        View::Breadboard => c.wires_in(view).any(|w| w.pts.first() == Some(&p) || w.pts.last() == Some(&p)),
    }
}

/// The wire in progress as the canvas should draw it.
pub fn preview(st: &EditorState, c: &Circuit, lib: &dyn PartLookup, view: View) -> Option<Preview> {
    let d = st.wire.as_ref()?;
    let last = *d.pts.last()?;
    let mut pts = d.pts.clone();
    pts.extend(route(view, last, d.cursor, d.flip));
    let error = check_path(c, lib, view, &pts).err();
    let pins = pin_points(c, lib, view);
    let on_pin = |p: Pt| pins.iter().any(|q| q.at == p);
    Some(Preview { start_on_pin: on_pin(pts[0]), end_on_pin: on_pin(*pts.last().unwrap_or(&pts[0])), pts, error })
}

pub fn pointer_move(st: &mut EditorState, _ctx: &mut EditCtx, pos: [f64; 2], mods: Mods, fx: &mut Vec<Effect>) {
    if let Some(d) = &mut st.wire {
        d.cursor = snap(pos);
        d.flip = mods.shift;
    }
    fx.push(Effect::Repaint);
}

pub fn pointer_down(st: &mut EditorState, ctx: &mut EditCtx, pos: [f64; 2], mods: Mods, fx: &mut Vec<Effect>) {
    let p = snap(pos);
    if st.wire.is_none() {
        st.wire = Some(Draft { pts: vec![p], cursor: p, flip: mods.shift });
        fx.push(Effect::Repaint);
        return;
    }
    let d = st.wire.as_mut().expect("checked above");
    let last = *d.pts.last().expect("a draft has a vertex");
    if p == last {
        return;
    }
    let mut cand = d.pts.clone();
    cand.extend(route(ctx.view, last, p, mods.shift));
    if let Err(e) = check_path(ctx.circuit, ctx.lib, ctx.view, &cand) {
        fx.push(Effect::Refused(format!("Wire refused: {e}")));
        return;
    }
    d.pts = cand;
    d.cursor = p;
    if is_terminal(ctx.circuit, ctx.lib, ctx.view, p) {
        finish(st, ctx, fx);
    } else {
        fx.push(Effect::Repaint);
    }
}

pub fn double_click(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    finish_quietly(st, ctx, fx, true);
}

pub fn enter(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    finish_quietly(st, ctx, fx, false);
}

fn finish_quietly(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>, quiet: bool) {
    match &st.wire {
        Some(d) if distinct(&d.pts).len() >= 2 => finish(st, ctx, fx),
        Some(_) => {
            st.wire = None;
            if !quiet {
                fx.push(Effect::Status("A wire needs at least two points".into()));
            }
            fx.push(Effect::Repaint);
        }
        None => {}
    }
}

pub fn backspace(st: &mut EditorState, fx: &mut Vec<Effect>) {
    if let Some(d) = &mut st.wire {
        d.pts.pop();
        if let Some(last) = d.pts.last() {
            d.cursor = *last;
        } else {
            st.wire = None;
        }
        fx.push(Effect::Repaint);
    }
}

fn distinct(pts: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(pts.len());
    for p in pts {
        if out.last() != Some(p) {
            out.push(*p);
        }
    }
    out
}

/// Add the wire as one undo step: `AddWire` with a fresh id (`w1..` schematic, `j1..` breadboard),
/// then the same tidying `tidy_with` does, limited to the new wire.
fn finish(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    let Some(d) = st.wire.take() else { return };
    let pts = distinct(&d.pts);
    if pts.len() < 2 {
        fx.push(Effect::Repaint);
        return;
    }
    let view = ctx.view;
    let id = fresh_id(ctx.circuit, if view == View::Schematic { "w" } else { "j" });
    ctx.history.begin_group();
    let added = ctx.apply(Edit::AddWire(Wire { id: id.clone(), view, pts, color: None }));
    if added.is_ok() {
        let mut tmp = ctx.circuit.clone();
        tidy_with(&mut tmp, ctx.lib, view);
        let tidy = tmp.wires.iter().find(|w| w.id == id).map(|w| w.pts.clone());
        let now = ctx.circuit.wires.iter().find(|w| w.id == id).map(|w| w.pts.clone());
        if let (Some(t), Some(n)) = (tidy, now) {
            if t != n && t.len() >= 2 {
                let _ = ctx.apply(Edit::SetWirePts { id: id.clone(), pts: t });
            }
        }
    }
    ctx.history.end_group();
    match added {
        Ok(()) => {
            fx.extend([Effect::CircuitChanged, Effect::Repaint, Effect::Status(format!("Added wire {id}"))]);
        }
        Err(e) => fx.push(Effect::Refused(e.to_string())),
    }
}
