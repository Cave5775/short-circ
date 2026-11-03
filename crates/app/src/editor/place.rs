//! The Place tool: a ghost of a part type under the cursor, R / M turn it, a click adds it.
//! The tool stays active for repeated placing; Esc or a right click ends it.

use super::hit::pin_points;
use super::refs;
use super::{snap, EditCtx, Effect, EditorState, Item, Tool};
use short_circ_core::edit::Edit;
use short_circ_core::transform::pin_world;
use short_circ_core::{builtin, Circuit, Part, PartDef, PartLookup, Placement, Pt, View};

#[derive(Debug, Clone)]
pub struct PlaceDraft {
    pub ty: String,
    pub def: PartDef,
    pub rot: i32,
    pub mirror: bool,
    /// Snapped pointer position: the origin of the new part.
    pub at: Pt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhostPin {
    pub pin: String,
    pub at: Pt,
    /// A board hole lies under this pin (breadboard).
    pub on_hole: bool,
    /// The pin lands on another pin or on a wire.
    pub connects: bool,
}

/// What the canvas draws for the part being placed.
#[derive(Debug, Clone, PartialEq)]
pub struct Ghost {
    pub ty: String,
    pub placement: Placement,
    pub pins: Vec<GhostPin>,
    /// Why a click would be refused (the type has no geometry in this view).
    pub error: Option<String>,
}

fn no_view_message(ty: &str, view: View) -> String {
    format!("{ty} has no {} view, so it cannot be placed here", view.key())
}

pub fn start(st: &mut EditorState, ctx: &mut EditCtx, ty: &str, fx: &mut Vec<Effect>) {
    let def = ctx.lib.get(ty).cloned().or_else(|| builtin::generate(ty));
    let Some(def) = def else {
        fx.push(Effect::Refused(format!("unknown part type {ty:?}")));
        return;
    };
    if def.view(ctx.view).is_none() {
        fx.push(Effect::Refused(no_view_message(ty, ctx.view)));
        return;
    }
    super::select::cancel_drag(st, ctx);
    st.wire = None;
    st.label_at = None;
    st.tool = Tool::Place;
    let at = st.cursor.unwrap_or([0, 0]);
    st.place = Some(PlaceDraft { ty: ty.to_string(), def, rot: 0, mirror: false, at });
    fx.extend([
        Effect::ToolChanged,
        Effect::Repaint,
        Effect::Status(format!("Placing {ty}: click to place, R rotate, M mirror, Esc to stop")),
    ]);
}

pub fn pointer_move(st: &mut EditorState, pos: [f64; 2], fx: &mut Vec<Effect>) {
    if let Some(p) = &mut st.place {
        p.at = snap(pos);
        fx.push(Effect::Repaint);
    }
}

pub fn pointer_down(st: &mut EditorState, ctx: &mut EditCtx, pos: [f64; 2], fx: &mut Vec<Effect>) {
    let Some(pd) = &mut st.place else { return };
    pd.at = snap(pos);
    let view = ctx.view;
    if pd.def.view(view).is_none() {
        fx.push(Effect::Refused(no_view_message(&pd.ty, view)));
        return;
    }
    let id = refs::fresh_part_id(ctx.circuit, &pd.ty, Some(&pd.def));
    let mut part = Part { id: id.clone(), ty: pd.ty.clone(), ..Part::default() };
    if let Some(v) = refs::default_value(&pd.ty) {
        part.props.insert("value".into(), v.into());
    }
    *part.placement_mut(view) = Some(Placement { at: pd.at, rot: pd.rot, mirror: pd.mirror });
    match ctx.apply(Edit::AddPart(part)) {
        Ok(()) => {
            st.sel.set(Item::Part(id.clone()));
            fx.extend([
                Effect::CircuitChanged,
                Effect::SelectionChanged,
                Effect::Repaint,
                Effect::Status(format!("Placed {id}")),
            ]);
        }
        Err(e) => fx.push(Effect::Refused(e.to_string())),
    }
}

pub fn rotate(st: &mut EditorState, fx: &mut Vec<Effect>) {
    if let Some(p) = &mut st.place {
        p.rot = (p.rot + 90).rem_euclid(360);
        fx.push(Effect::Repaint);
    }
}

pub fn mirror(st: &mut EditorState, fx: &mut Vec<Effect>) {
    if let Some(p) = &mut st.place {
        p.mirror = !p.mirror;
        fx.push(Effect::Repaint);
    }
}

/// The ghost to draw, with the pins that land on board holes or on something else.
pub fn ghost(st: &EditorState, c: &Circuit, lib: &dyn PartLookup, view: View) -> Option<Ghost> {
    let pd = st.place.as_ref()?;
    let placement = Placement { at: pd.at, rot: pd.rot, mirror: pd.mirror };
    let Some(_) = pd.def.view(view) else {
        return Some(Ghost { ty: pd.ty.clone(), placement, pins: Vec::new(), error: Some(no_view_message(&pd.ty, view)) });
    };
    let existing = pin_points(c, lib, view);
    let pins = pd
        .def
        .placed_pins(view)
        .map(|(pin, pos)| {
            let at = pin_world(&placement, pos);
            GhostPin {
                pin: pin.id.clone(),
                at,
                on_hole: existing.iter().any(|q| q.board && q.at == at),
                connects: existing.iter().any(|q| q.at == at)
                    || c.wires_in(view).any(|w| w.pts.windows(2).any(|s| super::hit::on_segment(s[0], s[1], at))),
            }
        })
        .collect();
    Some(Ghost { ty: pd.ty.clone(), placement, pins, error: None })
}

/// The definition being placed (the canvas draws its artwork).
pub fn ghost_def(st: &EditorState) -> Option<&PartDef> {
    st.place.as_ref().map(|p| &p.def)
}
