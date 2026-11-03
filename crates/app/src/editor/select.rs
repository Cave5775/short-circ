//! The Select tool: click, shift-click, marquee, drag-move, nudge, delete, rotate, mirror, tidy.

use super::hit;
use super::{EditCtx, Effect, EditorState, Item, Mods, Selection};
use short_circ_core::edit::Edit;
use short_circ_core::{Pt, View};

/// Smallest marquee (grid units) that is a drag rather than a click.
const CLICK_SLOP: f64 = 0.25;

#[derive(Debug, Default)]
pub enum Drag {
    #[default]
    None,
    Marquee {
        start: [f64; 2],
        cur: [f64; 2],
        shift: bool,
        /// Selection to extend (shift) or empty.
        base: Selection,
        /// Board under the press: a plain click selects it.
        board: Option<String>,
    },
    /// Moving the selection inside one history group.
    Move {
        start: [f64; 2],
        /// Delta already applied to the circuit.
        applied: [i32; 2],
        /// Some edit was applied (the group has a snapshot).
        touched: bool,
        /// On a click without movement, shrink the selection to this item.
        pending: Option<Item>,
    },
}

impl Drag {
    pub fn marquee(&self) -> Option<([f64; 2], [f64; 2])> {
        match self {
            Drag::Marquee { start, cur, .. } => {
                Some(([start[0].min(cur[0]), start[1].min(cur[1])], [start[0].max(cur[0]), start[1].max(cur[1])]))
            }
            _ => None,
        }
    }
}

pub fn drag_active(st: &EditorState) -> bool {
    !matches!(st.drag, Drag::None)
}

/// Abort a drag: a move is taken back completely, a marquee just disappears.
pub fn cancel_drag(st: &mut EditorState, ctx: &mut EditCtx) {
    match std::mem::take(&mut st.drag) {
        Drag::Move { touched, .. } => {
            ctx.history.end_group();
            if touched {
                ctx.history.undo(ctx.circuit);
                st.sel.prune(ctx.circuit);
            }
        }
        Drag::Marquee { .. } | Drag::None => {}
    }
}

pub fn pointer_down(st: &mut EditorState, ctx: &mut EditCtx, pos: [f64; 2], mods: Mods, fx: &mut Vec<Effect>) {
    cancel_drag(st, ctx);
    match hit::hit_item(ctx.circuit, ctx.lib, ctx.view, pos, ctx.tol) {
        Some(item) => {
            if mods.shift {
                st.sel.toggle(item);
                fx.extend([Effect::SelectionChanged, Effect::Repaint]);
                return;
            }
            let was_selected = st.sel.contains(&item);
            if !was_selected {
                st.sel.set(item.clone());
                fx.push(Effect::SelectionChanged);
            }
            let pending = (was_selected && st.sel.items.len() > 1).then_some(item);
            ctx.history.begin_group();
            st.drag = Drag::Move { start: pos, applied: [0, 0], touched: false, pending };
        }
        None => {
            let board = hit::board_under(ctx.circuit, ctx.lib, ctx.view, pos).map(|i| ctx.circuit.parts[i].id.clone());
            let base = if mods.shift { st.sel.clone() } else { Selection::default() };
            st.drag = Drag::Marquee { start: pos, cur: pos, shift: mods.shift, base, board };
        }
    }
    fx.push(Effect::Repaint);
}

pub fn pointer_move(st: &mut EditorState, ctx: &mut EditCtx, pos: [f64; 2], fx: &mut Vec<Effect>) {
    let mut drag = std::mem::take(&mut st.drag);
    match &mut drag {
        Drag::Move { start, applied, touched, .. } => {
            let target = [(pos[0] - start[0]).round() as i32, (pos[1] - start[1]).round() as i32];
            if target != *applied {
                let delta = [target[0] - applied[0], target[1] - applied[1]];
                match apply_move(ctx, &st.sel, delta) {
                    Ok(()) => {
                        *applied = target;
                        *touched = true;
                        fx.push(Effect::CircuitChanged);
                    }
                    Err(e) => fx.push(Effect::Refused(e)),
                }
            }
            fx.push(Effect::Repaint);
        }
        Drag::Marquee { cur, .. } => {
            *cur = pos;
            fx.push(Effect::Repaint);
        }
        Drag::None => {}
    }
    st.drag = drag;
}

pub fn pointer_up(st: &mut EditorState, ctx: &mut EditCtx, pos: [f64; 2], fx: &mut Vec<Effect>) {
    match std::mem::take(&mut st.drag) {
        Drag::Move { touched, applied, pending, .. } => {
            ctx.history.end_group();
            if touched && applied == [0, 0] {
                // Dragged away and back: leave no undo step behind.
                ctx.history.undo(ctx.circuit);
                st.sel.prune(ctx.circuit);
                fx.push(Effect::CircuitChanged);
            } else if !touched {
                if let Some(item) = pending {
                    st.sel.set(item);
                    fx.push(Effect::SelectionChanged);
                }
            }
        }
        Drag::Marquee { start, shift, base, board, .. } => {
            let (dx, dy) = ((pos[0] - start[0]).abs(), (pos[1] - start[1]).abs());
            if dx < CLICK_SLOP && dy < CLICK_SLOP {
                match board {
                    Some(id) => {
                        let item = Item::Part(id);
                        if shift {
                            st.sel.toggle(item);
                        } else {
                            st.sel.set(item);
                        }
                    }
                    None if !shift => st.sel = Selection::default(),
                    None => {}
                }
            } else {
                let lo = [start[0].min(pos[0]), start[1].min(pos[1])];
                let hi = [start[0].max(pos[0]), start[1].max(pos[1])];
                let mut sel = base;
                for item in hit::items_inside(ctx.circuit, ctx.lib, ctx.view, lo, hi) {
                    sel.add(item);
                }
                st.sel = sel;
            }
            fx.extend([Effect::SelectionChanged, Effect::Repaint]);
        }
        Drag::None => {}
    }
}

pub fn double_click(st: &mut EditorState, ctx: &mut EditCtx, pos: [f64; 2], fx: &mut Vec<Effect>) {
    if let Some(item) = hit::hit_item(ctx.circuit, ctx.lib, ctx.view, pos, ctx.tol) {
        if !st.sel.contains(&item) {
            st.sel.set(item);
            fx.push(Effect::SelectionChanged);
        }
        fx.push(Effect::OpenProperties);
    }
}

/// Move the selected parts, wires and labels of the active view by `delta` (grid units). Parts go
/// through `MoveBy` (wire ends on their pins follow); selected wires move as a whole (the points a
/// rubber band already moved stay); labels follow, and so do labels sitting on a moved pin.
pub fn apply_move(ctx: &mut EditCtx, sel: &Selection, delta: [i32; 2]) -> Result<(), String> {
    let view = ctx.view;
    let part_ids: Vec<String> = sel
        .parts()
        .into_iter()
        .filter(|id| ctx.circuit.part(id).is_some_and(|p| p.placement(view).is_some()))
        .collect();
    let wire_ids: Vec<String> =
        sel.wires().into_iter().filter(|id| ctx.circuit.wires.iter().any(|w| &w.id == id && w.view == view)).collect();
    let sel_labels: Vec<usize> = if view == View::Schematic {
        sel.labels().into_iter().filter(|k| *k < ctx.circuit.labels.len()).collect()
    } else {
        Vec::new()
    };
    // Labels on a pin of a moved part go with it.
    let mut label_moves = sel_labels.clone();
    if view == View::Schematic && !part_ids.is_empty() {
        let pins: Vec<Pt> = hit::pin_points(ctx.circuit, ctx.lib, view)
            .into_iter()
            .filter(|p| part_ids.contains(&ctx.circuit.parts[p.part].id))
            .map(|p| p.at)
            .collect();
        for (k, l) in ctx.circuit.labels.iter().enumerate() {
            if pins.contains(&l.at) && !label_moves.contains(&k) {
                label_moves.push(k);
            }
        }
    }
    let before: Vec<(String, Vec<Pt>)> = wire_ids
        .iter()
        .filter_map(|id| ctx.circuit.wires.iter().find(|w| &w.id == id).map(|w| (id.clone(), w.pts.clone())))
        .collect();

    if !part_ids.is_empty() {
        ctx.apply(Edit::MoveBy { ids: part_ids, view, delta }).map_err(|e| e.to_string())?;
    }
    for (id, old) in before {
        let Some(w) = ctx.circuit.wires.iter().find(|w| w.id == id) else { continue };
        let pts: Vec<Pt> = old
            .iter()
            .zip(w.pts.iter())
            .map(|(o, n)| if o == n { [o[0].saturating_add(delta[0]), o[1].saturating_add(delta[1])] } else { *n })
            .collect();
        ctx.apply(Edit::SetWirePts { id, pts }).map_err(|e| e.to_string())?;
    }
    for k in label_moves {
        let at = ctx.circuit.labels[k].at;
        let to = [at[0].saturating_add(delta[0]), at[1].saturating_add(delta[1])];
        ctx.apply(Edit::MoveLabel { index: k, to }).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Arrow key: move the selection by one unit, one undo step.
pub fn nudge(st: &mut EditorState, ctx: &mut EditCtx, delta: [i32; 2], fx: &mut Vec<Effect>) {
    if st.sel.is_empty() {
        return;
    }
    ctx.history.begin_group();
    let r = apply_move(ctx, &st.sel, delta);
    ctx.history.end_group();
    match r {
        Ok(()) => fx.extend([Effect::CircuitChanged, Effect::Repaint]),
        Err(e) => fx.push(Effect::Refused(e)),
    }
}

pub fn select_all(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    st.sel.items = hit::all_items(ctx.circuit, ctx.view);
    fx.extend([Effect::SelectionChanged, Effect::Repaint]);
}

/// Delete the selection (parts leave every view; their wires stay and show up as dangling).
pub fn delete(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    if st.sel.is_empty() {
        fx.push(Effect::Status("nothing selected".into()));
        return;
    }
    let (parts, wires) = (st.sel.parts(), st.sel.wires());
    let mut labels = st.sel.labels();
    labels.sort_unstable_by(|a, b| b.cmp(a));
    let shared = parts.iter().filter(|id| {
        ctx.circuit.part(id).is_some_and(|p| p.placement(ctx.view.other()).is_some())
    });
    let also_other = shared.count();
    ctx.history.begin_group();
    let mut n = 0;
    for k in labels {
        n += ctx.apply(Edit::RemoveLabel { index: k }).is_ok() as usize;
    }
    for id in wires {
        n += ctx.apply(Edit::RemoveWire { id }).is_ok() as usize;
    }
    for id in &parts {
        n += ctx.apply(Edit::RemovePart { id: id.clone() }).is_ok() as usize;
    }
    ctx.history.end_group();
    st.sel = Selection::default();
    let mut msg = format!("Deleted {n} item(s)");
    if also_other > 0 {
        msg.push_str(&format!(" ({also_other} part(s) removed from the {} view too)", ctx.view.other().key()));
    }
    fx.extend([Effect::CircuitChanged, Effect::SelectionChanged, Effect::Repaint, Effect::Status(msg)]);
}

fn selected_parts_in_view(st: &EditorState, ctx: &EditCtx) -> Vec<String> {
    st.sel
        .parts()
        .into_iter()
        .filter(|id| ctx.circuit.part(id).is_some_and(|p| p.placement(ctx.view).is_some()))
        .collect()
}

/// Rotate every selected part by `degrees` about its own origin, one undo step.
pub fn rotate(st: &mut EditorState, ctx: &mut EditCtx, degrees: i32, fx: &mut Vec<Effect>) {
    let ids = selected_parts_in_view(st, ctx);
    if ids.is_empty() {
        fx.push(Effect::Status("select a part to rotate".into()));
        return;
    }
    let view = ctx.view;
    ctx.history.begin_group();
    for id in ids {
        let _ = ctx.apply(Edit::Rotate { id, view, degrees });
    }
    ctx.history.end_group();
    fx.extend([Effect::CircuitChanged, Effect::Repaint]);
}

pub fn mirror(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    let ids = selected_parts_in_view(st, ctx);
    if ids.is_empty() {
        fx.push(Effect::Status("select a part to mirror".into()));
        return;
    }
    let view = ctx.view;
    ctx.history.begin_group();
    for id in ids {
        let _ = ctx.apply(Edit::Mirror { id, view });
    }
    ctx.history.end_group();
    fx.extend([Effect::CircuitChanged, Effect::Repaint]);
}

/// Edit > Tidy wires: merge collinear segments of the active view as one undo step.
pub fn tidy(ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    let view = ctx.view;
    if ctx.apply_direct(|c, lib| short_circ_core::edit::tidy_with(c, lib, view)) {
        fx.extend([Effect::CircuitChanged, Effect::Repaint, Effect::Status("Tidied the wires".into())]);
    } else {
        fx.push(Effect::Status("Nothing to tidy".into()));
    }
}
