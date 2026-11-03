//! Copy, cut and paste of parts with their placement in the active view, plus the wires of that
//! view that run between copied parts (and any selected wires and labels).

use super::hit::pin_points;
use super::refs;
use super::{select, EditCtx, Effect, EditorState, Item, Selection};
use short_circ_core::edit::{fresh_id, Edit};
use short_circ_core::{Label, Part, Placement, Pt, View, Wire};

/// Each paste moves the copy by this much more than the previous one.
pub const PASTE_OFFSET: i32 = 2;

#[derive(Debug, Clone)]
pub struct Clip {
    /// The view the placement and the wires come from.
    pub view: View,
    /// Parts with only the placement in `view` kept.
    pub parts: Vec<Part>,
    pub wires: Vec<Wire>,
    pub labels: Vec<Label>,
    /// Pastes since the copy (the next one is offset by `PASTE_OFFSET * (pastes + 1)`).
    pub pastes: u32,
}

/// Copy the selection; false (with a status) when there is nothing to copy.
pub fn copy(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) -> bool {
    let view = ctx.view;
    let c = &*ctx.circuit;
    let mut parts: Vec<Part> = Vec::new();
    for id in st.sel.parts() {
        if let Some(p) = c.part(&id).filter(|p| p.placement(view).is_some()) {
            let mut q = p.clone();
            *q.placement_mut(view.other()) = None;
            parts.push(q);
        }
    }
    let pins: Vec<Pt> = pin_points(c, ctx.lib, view)
        .into_iter()
        .filter(|p| parts.iter().any(|q| q.id == c.parts[p.part].id))
        .map(|p| p.at)
        .collect();
    let sel_wires = st.sel.wires();
    let wires: Vec<Wire> = c
        .wires_in(view)
        .filter(|w| {
            sel_wires.contains(&w.id)
                || (!parts.is_empty()
                    && w.pts.len() >= 2
                    && pins.contains(&w.pts[0])
                    && pins.contains(&w.pts[w.pts.len() - 1]))
        })
        .cloned()
        .collect();
    let labels: Vec<Label> = if view == View::Schematic {
        st.sel.labels().into_iter().filter_map(|k| c.labels.get(k).cloned()).collect()
    } else {
        Vec::new()
    };
    if parts.is_empty() && wires.is_empty() && labels.is_empty() {
        fx.push(Effect::Status("Nothing selected to copy".into()));
        return false;
    }
    let text = format!(
        "short-circ: {} part(s), {} wire(s): {}",
        parts.len(),
        wires.len(),
        parts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(" ")
    );
    fx.push(Effect::Status(format!("Copied {} part(s), {} wire(s)", parts.len(), wires.len())));
    fx.push(Effect::ClipboardText(text));
    st.clip = Some(Clip { view, parts, wires, labels, pastes: 0 });
    true
}

/// Copy, then delete the selection (one undo step).
pub fn cut(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    if copy(st, ctx, fx) {
        select::delete(st, ctx, fx);
    }
}

/// Paste the clipboard into the active view as one undo step; the copies become the selection.
pub fn paste(st: &mut EditorState, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
    let Some(clip) = st.clip.as_mut() else {
        fx.push(Effect::Status("The clipboard is empty".into()));
        return;
    };
    let view = ctx.view;
    let d = PASTE_OFFSET * (clip.pastes as i32 + 1);
    let shift = |p: Pt| [p[0].saturating_add(d), p[1].saturating_add(d)];
    let mut sel = Selection::default();
    let mut skipped: Vec<String> = Vec::new();
    ctx.history.begin_group();
    for p in &clip.parts {
        let Some(pl) = p.placement(clip.view) else { continue };
        if let Some(def) = ctx.lib.get(&p.ty) {
            if def.view(view).is_none() {
                skipped.push(p.id.clone());
                continue;
            }
        }
        let mut q = p.clone();
        q.id = refs::fresh_part_id(ctx.circuit, &p.ty, ctx.lib.get(&p.ty));
        *q.placement_mut(clip.view) = None;
        *q.placement_mut(view) = Some(Placement { at: shift(pl.at), rot: pl.rot, mirror: pl.mirror });
        let id = q.id.clone();
        if ctx.apply(Edit::AddPart(q)).is_ok() {
            sel.add(Item::Part(id));
        }
    }
    if clip.view == view {
        for w in &clip.wires {
            let mut w2 = w.clone();
            w2.id = fresh_id(ctx.circuit, if view == View::Schematic { "w" } else { "j" });
            w2.pts = w.pts.iter().map(|p| shift(*p)).collect();
            let id = w2.id.clone();
            if ctx.apply(Edit::AddWire(w2)).is_ok() {
                sel.add(Item::Wire(id));
            }
        }
        for l in &clip.labels {
            let mut l2 = l.clone();
            l2.at = shift(l.at);
            if ctx.apply(Edit::AddLabel(l2)).is_ok() {
                sel.add(Item::Label(ctx.circuit.labels.len() - 1));
            }
        }
    }
    ctx.history.end_group();
    clip.pastes += 1;
    let n = sel.items.len();
    if n > 0 {
        st.sel = sel;
        fx.extend([Effect::CircuitChanged, Effect::SelectionChanged, Effect::Repaint]);
    }
    if !skipped.is_empty() {
        fx.push(Effect::Refused(format!(
            "Not pasted (no {} view for the type): {}",
            view.key(),
            skipped.join(", ")
        )));
    } else {
        fx.push(Effect::Status(format!("Pasted {n} item(s)")));
    }
}
