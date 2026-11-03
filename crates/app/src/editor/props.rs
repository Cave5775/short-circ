//! What the properties panel does, as plain functions on an [`EditCtx`]: every call that changes
//! something is exactly one undo step, and a call that would change nothing records nothing.
//! Errors are messages for an inline warning next to the field.

use super::hit::{def_is_board, part_world_box};
use super::{EditCtx, EditorState, Item};
use short_circ_core::color::is_valid_color_spec;
use short_circ_core::edit::Edit;
use short_circ_core::{Circuit, Label, NetDecl, PartLookup, Placement, Pt, View};

/// The three colour fields of a part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorField {
    Color,
    Fill,
    LabelColor,
}

impl ColorField {
    pub const ALL: [ColorField; 3] = [ColorField::Color, ColorField::Fill, ColorField::LabelColor];

    pub fn key(self) -> &'static str {
        match self {
            ColorField::Color => "color",
            ColorField::Fill => "fill",
            ColorField::LabelColor => "label_color",
        }
    }
}

/// Validate colour text from a field. Empty and `default` mean "no colour set" (`Ok(None)`); a
/// colour, preset name or `none` is kept as typed (trimmed); anything else is an error.
pub fn normalize_color(text: &str) -> Result<Option<String>, String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    if is_valid_color_spec(t) {
        Ok(Some(t.to_string()))
    } else {
        Err(format!("\"{t}\" is not a colour (use #rgb, #rrggbb, #rrggbbaa, a preset name, none or default)"))
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Rename a part or wire (declared nets follow). Refuses empty and duplicate ids with a message.
pub fn rename_id(st: &mut EditorState, ctx: &mut EditCtx, old: &str, new: &str) -> Result<(), String> {
    let new = new.trim();
    if new.is_empty() {
        return Err("The id cannot be empty".into());
    }
    if new == old {
        return Ok(());
    }
    if ctx.circuit.parts.iter().any(|p| p.id == new) || ctx.circuit.wires.iter().any(|w| w.id == new) {
        return Err(format!("\"{new}\" is already used by another part or wire"));
    }
    ctx.apply(Edit::RenameId { old: old.to_string(), new: new.to_string() }).map_err(err)?;
    for item in &mut st.sel.items {
        match item {
            Item::Part(id) | Item::Wire(id) if id.as_str() == old => *id = new.to_string(),
            _ => {}
        }
    }
    Ok(())
}

/// Set (`Some`) or remove (`None`) one prop of a part.
pub fn set_prop(ctx: &mut EditCtx, id: &str, key: &str, value: Option<&str>) -> Result<(), String> {
    let part = ctx.circuit.part(id).ok_or_else(|| format!("no part \"{id}\""))?;
    if part.props.get(key).map(String::as_str) == value {
        return Ok(());
    }
    ctx.apply(Edit::SetProp { id: id.to_string(), key: key.to_string(), value: value.map(str::to_string) }).map_err(err)
}

/// Add a new prop row; the key must be non-empty and unused.
pub fn add_prop(ctx: &mut EditCtx, id: &str, key: &str, value: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("The property name cannot be empty".into());
    }
    if ctx.circuit.part(id).is_some_and(|p| p.props.contains_key(key)) {
        return Err(format!("\"{key}\" already exists"));
    }
    set_prop(ctx, id, key, Some(value))
}

/// Rename a prop key, keeping its value (one undo step).
pub fn rename_prop_key(ctx: &mut EditCtx, id: &str, old: &str, new: &str) -> Result<(), String> {
    let new = new.trim();
    if new.is_empty() {
        return Err("The property name cannot be empty".into());
    }
    if new == old {
        return Ok(());
    }
    let part = ctx.circuit.part(id).ok_or_else(|| format!("no part \"{id}\""))?;
    if part.props.contains_key(new) {
        return Err(format!("\"{new}\" already exists"));
    }
    let value = part.props.get(old).cloned().ok_or_else(|| format!("no property \"{old}\""))?;
    ctx.history.begin_group();
    let r = set_prop(ctx, id, old, None).and_then(|_| set_prop(ctx, id, new, Some(&value)));
    ctx.history.end_group();
    r
}

/// Set one colour field of a part from text (see [`normalize_color`]); invalid text is refused.
pub fn set_part_color(ctx: &mut EditCtx, id: &str, field: ColorField, text: &str) -> Result<(), String> {
    let value = normalize_color(text)?;
    let part = ctx.circuit.part(id).ok_or_else(|| format!("no part \"{id}\""))?;
    let (mut color, mut fill, mut label_color) = (part.color.clone(), part.fill.clone(), part.label_color.clone());
    let slot = match field {
        ColorField::Color => &mut color,
        ColorField::Fill => &mut fill,
        ColorField::LabelColor => &mut label_color,
    };
    if *slot == value {
        return Ok(());
    }
    *slot = value;
    ctx.apply(Edit::SetColors { id: id.to_string(), color, fill, label_color }).map_err(err)
}

pub fn set_wire_color(ctx: &mut EditCtx, id: &str, text: &str) -> Result<(), String> {
    let value = normalize_color(text)?;
    let w = ctx.circuit.wires.iter().find(|w| w.id == id).ok_or_else(|| format!("no wire \"{id}\""))?;
    if w.color == value {
        return Ok(());
    }
    ctx.apply(Edit::SetWireColor { id: id.to_string(), color: value }).map_err(err)
}

pub fn set_label_color(ctx: &mut EditCtx, index: usize, text: &str) -> Result<(), String> {
    let value = normalize_color(text)?;
    let l = ctx.circuit.labels.get(index).ok_or_else(|| "no such label".to_string())?;
    if l.color == value {
        return Ok(());
    }
    ctx.apply(Edit::SetLabelColor { index, color: value }).map_err(err)
}

pub fn set_hidden_label(ctx: &mut EditCtx, id: &str, hidden: bool) -> Result<(), String> {
    let part = ctx.circuit.part(id).ok_or_else(|| format!("no part \"{id}\""))?;
    if part.hidden_label == hidden {
        return Ok(());
    }
    ctx.apply(Edit::SetHiddenLabel { id: id.to_string(), hidden }).map_err(err)
}

/// Rename a label (there is no edit for it: the labels from that index on are removed and added
/// again, so order and index stay the same).
pub fn rename_label(ctx: &mut EditCtx, index: usize, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A label needs a name".into());
    }
    let labels: Vec<Label> = ctx.circuit.labels.clone();
    let Some(cur) = labels.get(index) else { return Err("no such label".into()) };
    if cur.name == name {
        return Ok(());
    }
    ctx.history.begin_group();
    let mut r = Ok(());
    for _ in index..labels.len() {
        r = r.and_then(|_| ctx.apply(Edit::RemoveLabel { index }).map_err(err));
    }
    for (k, l) in labels.iter().enumerate().skip(index) {
        let mut l = l.clone();
        if k == index {
            l.name = name.to_string();
        }
        r = r.and_then(|_| ctx.apply(Edit::AddLabel(l)).map_err(err));
    }
    ctx.history.end_group();
    r
}

/// Set one view's placement of a part (x, y, rot, mirror edited in the panel).
pub fn set_placement(ctx: &mut EditCtx, id: &str, view: View, pl: Placement) -> Result<(), String> {
    if !matches!(pl.rot, 0 | 90 | 180 | 270) {
        return Err("Rotation must be 0, 90, 180 or 270".into());
    }
    let part = ctx.circuit.part(id).ok_or_else(|| format!("no part \"{id}\""))?;
    if part.placement(view) == Some(&pl) {
        return Ok(());
    }
    ctx.apply(Edit::SetPlacement { id: id.to_string(), view, placement: Some(pl) }).map_err(err)
}

/// Remove the part from one view (its other view and its wires stay).
pub fn remove_from_view(ctx: &mut EditCtx, id: &str, view: View) -> Result<(), String> {
    let part = ctx.circuit.part(id).ok_or_else(|| format!("no part \"{id}\""))?;
    if part.placement(view).is_none() {
        return Ok(());
    }
    ctx.apply(Edit::SetPlacement { id: id.to_string(), view, placement: None }).map_err(err)
}

/// Put the part into `view` at a free spot near that view's origin, rotation 0.
pub fn place_in_view(ctx: &mut EditCtx, id: &str, view: View) -> Result<Pt, String> {
    let part = ctx.circuit.part(id).ok_or_else(|| format!("no part \"{id}\""))?;
    if part.placement(view).is_some() {
        return Err(format!("{id} is already placed in the {} view", view.key()));
    }
    let ty = part.ty.clone();
    let def = ctx.lib.get(&ty).ok_or_else(|| format!("unknown part type {ty:?}"))?;
    if def.view(view).is_none() {
        return Err(format!("{ty} has no {} view", view.key()));
    }
    let at = free_spot(ctx.circuit, ctx.lib, view, id);
    ctx.apply(Edit::SetPlacement { id: id.to_string(), view, placement: Some(Placement::at(at[0], at[1])) })
        .map_err(err)?;
    Ok(at)
}

/// The first position (scanning outwards from the origin, rotation 0) where the part `id` would
/// not overlap another part of its kind (boards only avoid boards) in `view`.
pub fn free_spot(c: &Circuit, lib: &dyn PartLookup, view: View, id: &str) -> Pt {
    let Some(part) = c.part(id) else { return [0, 0] };
    let Some(vd) = lib.get(&part.ty).and_then(|d| d.view(view)) else { return [0, 0] };
    let board = def_is_board(lib, &part.ty);
    let others: Vec<([f64; 2], [f64; 2])> = c
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.id != id && def_is_board(lib, &p.ty) == board)
        .filter_map(|(i, _)| part_world_box(c, lib, view, i))
        .collect();
    let (w, h) = (vd.size[0], vd.size[1]);
    for r in 0..160i32 {
        for y in 0..=r {
            let x = r - y;
            let (lo, hi) = ([x as f64, y as f64], [x as f64 + w, y as f64 + h]);
            let hit = others.iter().any(|(a, b)| lo[0] < b[0] && a[0] < hi[0] && lo[1] < b[1] && a[1] < hi[1]);
            if !hit {
                return [x, y];
            }
        }
    }
    [0, 0]
}

/// Circuit title; empty text removes it.
pub fn set_title(ctx: &mut EditCtx, text: &str) -> Result<(), String> {
    let t = text.trim();
    let new = if t.is_empty() { None } else { Some(t.to_string()) };
    if ctx.circuit.title == new {
        return Ok(());
    }
    ctx.apply(Edit::SetTitle(new)).map_err(err)
}

/// Declared nets of what is wired in `view`: every net with at least two pins of parts that are
/// not boards, named after its label when it has one. Pins are `PART.PIN` in part order.
pub fn nets_from_view(c: &Circuit, lib: &dyn PartLookup, view: View) -> Vec<NetDecl> {
    let nl = short_circ_core::Netlist::build(c, lib, view);
    nl.nets
        .iter()
        .filter_map(|net| {
            let pins: Vec<String> = net
                .pins
                .iter()
                .filter(|p| !c.part(&p.part).is_some_and(|part| def_is_board(lib, &part.ty)))
                .map(|p| format!("{}.{}", p.part, p.pin))
                .collect();
            (pins.len() >= 2).then(|| NetDecl { name: net.name.clone(), pins })
        })
        .collect()
}

/// "Declare nets from this view".
pub fn declare_nets_from_view(ctx: &mut EditCtx) -> Result<usize, String> {
    let nets = nets_from_view(ctx.circuit, ctx.lib, ctx.view);
    let n = nets.len();
    if ctx.circuit.nets == nets {
        return Ok(n);
    }
    ctx.apply(Edit::SetDeclaredNets(nets)).map_err(err)?;
    Ok(n)
}

pub fn clear_declared_nets(ctx: &mut EditCtx) -> Result<(), String> {
    if ctx.circuit.nets.is_empty() {
        return Ok(());
    }
    ctx.apply(Edit::SetDeclaredNets(Vec::new())).map_err(err)
}

pub fn delete_declared_net(ctx: &mut EditCtx, index: usize) -> Result<(), String> {
    if index >= ctx.circuit.nets.len() {
        return Err("no such net".into());
    }
    let mut nets = ctx.circuit.nets.clone();
    nets.remove(index);
    ctx.apply(Edit::SetDeclaredNets(nets)).map_err(err)
}
