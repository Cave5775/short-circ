//! The egui side of Edit mode: a thin translator from egui input to `editor::Event`, the overlays
//! (selection, marquee, wire preview, ghost), the tools bar, the properties panel and the label
//! popup. No decision lives here; see `crate::editor` for the behaviour and its tests.

use crate::canvas::{self, Camera, DrawCtx, Highlight};
use crate::editor::props::{self, ColorField};
use crate::editor::vertex;
use crate::editor::{
    hit, Button, Command, EditCtx, EditorState, Event, Item, Key, Mods, Selection, Tool,
};
use crate::tex::TexCache;
use crate::{breadboard, partbrowser};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Shape, Stroke, Vec2};
use short_circ_core::color::parse_color;
use short_circ_core::{builtin, Circuit, Library, Part, Placement, View};

/// What a panel frame did.
#[derive(Debug, Default)]
pub struct PanelOut {
    /// The circuit changed (recompute derived data).
    pub changed: bool,
    pub status: Option<String>,
}

/// One change requested by the properties panel; applied after the layout pass.
enum Act {
    Rename(String, String),
    SetProp(String, String, Option<String>),
    AddProp(String, String, String),
    RenameKey(String, String, String),
    PartColor(String, ColorField, String),
    Hidden(String, bool),
    Placement(String, View, Placement),
    RemoveFromView(String, View),
    PlaceIn(String, View),
    WireColor(String, String),
    LabelName(usize, String),
    LabelColor(usize, String),
    Title(String),
    DeclareNets,
    ClearNets,
    DeleteNet(usize),
    DeleteSelection,
    /// A field's text could not be used (shown inline).
    Fail(String),
}

pub struct EditUi {
    pub editor: EditorState,
    /// Properties panel visible (double click on an item opens it again).
    pub show_props: bool,
    /// Text of the label popup.
    pub label_text: String,
    /// The view the editor was last told about.
    pub last_view: View,
    /// A press that started in the canvas is still down.
    pressed: bool,
    last_pos: Option<[f64; 2]>,
    ghost_cache: Option<(String, Library)>,
    add_filter: String,
    new_key: String,
    new_value: String,
    /// Inline error of the properties panel: the field it belongs to and the message.
    err: Option<(String, String)>,
    err_sel: Selection,
    label_focus: bool,
}

impl Default for EditUi {
    fn default() -> Self {
        EditUi {
            editor: EditorState::new(),
            show_props: true,
            label_text: String::new(),
            last_view: View::Schematic,
            pressed: false,
            last_pos: None,
            ghost_cache: None,
            add_filter: String::new(),
            new_key: String::new(),
            new_value: String::new(),
            err: None,
            err_sel: Selection::default(),
            label_focus: false,
        }
    }
}

fn mods_of(m: egui::Modifiers) -> Mods {
    Mods { shift: m.shift, ctrl: m.command, alt: m.alt }
}

fn map_key(k: egui::Key, m: egui::Modifiers) -> Option<Key> {
    use egui::Key as K;
    if m.alt {
        return None;
    }
    let key = match k {
        K::Escape => Key::Escape,
        K::Enter => Key::Enter,
        K::Backspace => Key::Backspace,
        K::Delete => Key::Delete,
        K::ArrowLeft => Key::Left,
        K::ArrowRight => Key::Right,
        K::ArrowUp => Key::Up,
        K::ArrowDown => Key::Down,
        _ => {
            let name = k.name();
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    };
    // Letters with Ctrl: only the editor's own chords (copy, cut and paste arrive as events).
    if m.command {
        return matches!(key, Key::Char('z' | 'y' | 'a')).then_some(key);
    }
    Some(key)
}

/// Key presses (and copy / cut / paste events) of this frame as editor events. Nothing while a
/// text box has the keyboard.
pub fn key_events(ctx: &egui::Context) -> Vec<Event> {
    if ctx.egui_wants_keyboard_input() {
        return Vec::new();
    }
    ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    map_key(*key, *modifiers).map(|k| Event::Key { key: k, mods: mods_of(*modifiers) })
                }
                egui::Event::Copy => Some(Event::Command(Command::Copy)),
                egui::Event::Cut => Some(Event::Command(Command::Cut)),
                egui::Event::Paste(_) => Some(Event::Command(Command::Paste)),
                _ => None,
            })
            .collect()
    })
}

fn swatch(ui: &mut egui::Ui, spec: Option<&str>) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
    let edge = ui.visuals().widgets.noninteractive.fg_stroke.color;
    match spec.and_then(parse_color) {
        Some(c) => {
            ui.painter().rect_filled(r, 2.0, canvas::to_color32(c));
        }
        None => {
            ui.painter().line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.0, edge));
        }
    }
    ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
}

/// A single-line text box that reports its text once, when focus leaves it (Enter or click
/// elsewhere) and the text differs from `current`; Esc abandons the edit.
fn text_commit(ui: &mut egui::Ui, salt: &str, current: &str, width: f32) -> Option<String> {
    let id = ui.make_persistent_id(salt);
    let store = id.with("text");
    let focused = ui.memory(|m| m.has_focus(id));
    let mut text = if focused {
        ui.data_mut(|d| d.get_temp::<String>(store)).unwrap_or_else(|| current.to_string())
    } else {
        current.to_string()
    };
    let resp = ui.add(egui::TextEdit::singleline(&mut text).id(id).desired_width(width));
    if resp.has_focus() || resp.changed() {
        let t = text.clone();
        ui.data_mut(|d| d.insert_temp(store, t));
    }
    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if resp.lost_focus() {
        ui.data_mut(|d| d.remove_temp::<String>(store));
        if !esc && text != current {
            return Some(text);
        }
    }
    None
}

/// The colour field of the properties panel, also used by the Autoroute dialog: a swatch, a hex /
/// name box and a preset combo, in the current layout. Returns the new text when the user typed
/// one (on Enter or focus loss) or picked a preset; `extras` adds the `default` and `none`
/// choices that only make sense for part colours.
pub(crate) fn color_edit(ui: &mut egui::Ui, key: &str, current: Option<&str>, extras: bool) -> Option<String> {
    let mut out = None;
    swatch(ui, current);
    if let Some(text) = text_commit(ui, &format!("{key}:text"), current.unwrap_or(""), 84.0) {
        out = Some(text);
    }
    egui::ComboBox::from_id_salt(format!("{key}:combo")).selected_text("presets").width(64.0).show_ui(ui, |ui| {
        for (n, _) in short_circ_core::color::preset_names() {
            if ui.selectable_label(current.is_some_and(|c| c.eq_ignore_ascii_case(n)), *n).clicked() {
                out = Some(n.to_string());
            }
        }
        if extras {
            if ui.selectable_label(false, "default").clicked() {
                out = Some("default".into());
            }
            if ui.selectable_label(current.is_some_and(|c| c.eq_ignore_ascii_case("none")), "none").clicked() {
                out = Some("none".into());
            }
        }
    });
    out
}

fn item_key(sel: &Selection) -> String {
    match sel.single() {
        Some(Item::Part(i)) => format!("p:{i}"),
        Some(Item::Wire(i)) => format!("w:{i}"),
        Some(Item::Label(i)) => format!("l:{i}"),
        None => format!("n:{}", sel.items.len()),
    }
}

impl EditUi {
    // ---- input ------------------------------------------------------------------------------

    /// Pointer events of this frame for the canvas `rect`, in world coordinates.
    pub fn pointer_events(&mut self, ui: &egui::Ui, rect: Rect, resp: &egui::Response, cam: Camera) -> Vec<Event> {
        let mut out = Vec::new();
        let (latest, down1, down2, up1, mods) = ui.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_pressed(),
                i.pointer.button_pressed(egui::PointerButton::Secondary),
                i.pointer.primary_released(),
                mods_of(i.modifiers),
            )
        });
        let hovered = resp.hovered();
        let Some(p) = latest else { return out };
        let w = cam.to_world(p - rect.min);
        if (hovered || self.pressed) && self.last_pos != Some(w) {
            self.last_pos = Some(w);
            out.push(Event::PointerMove { pos: w, mods });
        }
        if hovered && down1 {
            self.pressed = true;
            out.push(Event::PointerDown { pos: w, button: Button::Primary, mods });
        }
        if hovered && down2 {
            out.push(Event::PointerDown { pos: w, button: Button::Secondary, mods });
        }
        if self.pressed && up1 {
            self.pressed = false;
            out.push(Event::PointerUp { pos: w, button: Button::Primary });
        }
        if hovered && resp.double_clicked() {
            out.push(Event::DoubleClick { pos: w });
        }
        out
    }

    // ---- overlays ---------------------------------------------------------------------------

    /// Selection, marquee and the wire in progress, drawn over the canvas.
    pub fn draw_overlays(&self, d: &DrawCtx, circuit: &Circuit, lib: &Library, view: View) {
        let t = &d.theme;
        let sel = self.editor.selection();
        for item in &sel.items {
            match item {
                Item::Part(id) => {
                    if let Some(i) = circuit.parts.iter().position(|p| &p.id == id) {
                        if let Some((lo, hi)) = hit::part_world_box(circuit, lib, view, i) {
                            canvas::draw_selection(d, lo, hi);
                        }
                    }
                }
                Item::Wire(id) => {
                    if let Some(w) = circuit.wires.iter().find(|w| &w.id == id) {
                        let pts: Vec<Pos2> = w.pts.iter().map(|p| d.pt([p[0] as f64, p[1] as f64])).collect();
                        let width = d.px(0.3).max(3.0);
                        d.painter.add(Shape::line(pts.clone(), Stroke::new(width, t.select.gamma_multiply(0.55))));
                        // Handles only when this is the one selected wire (what a press grabs).
                        if sel.single().is_some() {
                            let h = d.px(0.22).max(4.0);
                            for (_, mid, _) in vertex::bend_handles(&w.pts) {
                                let c = d.pt(mid);
                                d.painter.circle_filled(c, h * 0.8, t.bg.gamma_multiply(0.9));
                                d.painter.circle_stroke(c, h * 0.8, Stroke::new(1.2, t.select));
                                let arm = h * 0.45;
                                d.painter.line_segment([c - Vec2::new(arm, 0.0), c + Vec2::new(arm, 0.0)], Stroke::new(1.2, t.select));
                                d.painter.line_segment([c - Vec2::new(0.0, arm), c + Vec2::new(0.0, arm)], Stroke::new(1.2, t.select));
                            }
                            for p in &pts {
                                let r = Rect::from_center_size(*p, Vec2::splat(h * 1.6));
                                d.painter.rect_filled(r, 1.0, t.select);
                                d.painter.rect_stroke(r, 1.0, Stroke::new(1.0, t.bg), egui::StrokeKind::Outside);
                            }
                        }
                    }
                }
                Item::Label(k) => {
                    if let (Some(l), View::Schematic) = (circuit.labels.get(*k), view) {
                        let (lo, hi) = hit::label_box(l);
                        canvas::draw_selection(d, lo, hi);
                    }
                }
            }
        }
        if let Some((lo, hi)) = self.editor.marquee() {
            let r = Rect::from_min_max(d.pt(lo), d.pt(hi));
            d.painter.rect_filled(r, 0.0, t.select.gamma_multiply(0.12));
            d.painter.rect_stroke(r, 0.0, Stroke::new(1.0, t.select), egui::StrokeKind::Inside);
        }
        if let Some(p) = self.editor.wire_preview(circuit, lib, view) {
            let bad = p.error.is_some();
            let color = if bad { t.error } else { t.select };
            let pts: Vec<Pos2> = p.pts.iter().map(|q| d.pt([q[0] as f64, q[1] as f64])).collect();
            let width = if view == View::Breadboard { d.px(breadboard::JUMPER_WIDTH).max(2.0) } else { d.px(0.16).max(1.5) };
            d.painter.add(Shape::line(pts.clone(), Stroke::new(width, color.gamma_multiply(0.85))));
            let r = d.px(0.28).max(3.5);
            let end_color = |on_pin: bool| {
                if bad {
                    t.error
                } else if view == View::Breadboard {
                    if on_pin {
                        Color32::from_rgb(40, 190, 90)
                    } else {
                        Color32::from_rgb(245, 140, 0)
                    }
                } else {
                    t.select
                }
            };
            d.painter.circle_filled(pts[0], r, end_color(p.start_on_pin));
            if let Some(last) = pts.last() {
                d.painter.circle_filled(*last, r, end_color(p.end_on_pin));
            }
            if let Some(msg) = &p.error {
                if let Some(last) = pts.last() {
                    d.painter.text(*last + Vec2::new(10.0, -10.0), egui::Align2::LEFT_BOTTOM, msg, egui::FontId::proportional(12.0), t.error);
                }
            }
        } else if matches!(self.editor.tool(), Tool::Wire | Tool::Label) {
            if let Some(c) = self.editor.cursor() {
                d.painter.circle_stroke(d.pt([c[0] as f64, c[1] as f64]), d.px(0.25).max(3.0), Stroke::new(1.2, t.select));
            }
        }
    }

    /// The part being placed: its artwork faded, plus rings on the pins that land on a hole
    /// (breadboard) or on something else.
    pub fn draw_ghost(&mut self, d: &DrawCtx, tex: &mut TexCache, circuit: &Circuit, lib: &Library, view: View) {
        let Some(g) = self.editor.ghost(circuit, lib, view) else { return };
        let Some(def) = self.editor.ghost_def().cloned() else { return };
        let Some(at) = self.editor.cursor() else { return };
        if let Some(msg) = &g.error {
            d.painter.text(
                d.pt([at[0] as f64 + 0.5, at[1] as f64 - 0.5]),
                egui::Align2::LEFT_BOTTOM,
                msg,
                egui::FontId::proportional(13.0),
                d.theme.error,
            );
            return;
        }
        if self.ghost_cache.as_ref().is_none_or(|(ty, _)| *ty != g.ty) {
            let probe = Circuit { lib: vec![def.clone()], ..Circuit::default() };
            self.ghost_cache = Some((g.ty.clone(), Library::load(&probe, None)));
        }
        let Some((_, glib)) = &self.ghost_cache else { return };
        let mut part = Part { id: "ghost".into(), ty: g.ty.clone(), hidden_label: true, ..Part::default() };
        *part.placement_mut(view) = Some(g.placement.clone());
        let one = Circuit { parts: vec![part], lib: vec![def], ..Circuit::default() };
        let mut faded = d.painter.clone();
        faded.set_opacity(0.55);
        let fd = DrawCtx { painter: &faded, origin: d.origin, cam: d.cam, theme: d.theme, labels: d.labels };
        match view {
            View::Schematic => canvas::draw_schematic(&fd, &one, glib, &[], &Highlight::default(), None),
            View::Breadboard => breadboard::draw(&fd, tex, &one, glib, &[], &Highlight::default(), None),
        }
        let ok = Color32::from_rgb(40, 190, 90);
        for p in &g.pins {
            if p.on_hole || p.connects {
                let c = d.pt([p.at[0] as f64, p.at[1] as f64]);
                let r = d.px(0.34).max(4.0);
                d.painter.circle_filled(c, r, ok.gamma_multiply(0.35));
                d.painter.circle_stroke(c, r, Stroke::new(1.5, ok));
            }
        }
    }

    // ---- tools bar and menus ----------------------------------------------------------------

    /// The row of tool buttons in Edit mode; returns the events the buttons caused.
    pub fn tools_bar(&mut self, ui: &mut egui::Ui, view: View, can_undo: bool, can_redo: bool) -> Vec<Event> {
        let mut out = Vec::new();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Edit").strong());
            let tool = self.editor.tool();
            if ui.add(egui::Button::selectable(tool == Tool::Select, "Select")).on_hover_text("Esc / S").clicked() {
                out.push(Event::SetTool(Tool::Select));
            }
            if ui.add(egui::Button::selectable(tool == Tool::Wire, "Wire")).on_hover_text("W").clicked() {
                out.push(Event::SetTool(Tool::Wire));
            }
            let label = ui.add_enabled(view == View::Schematic, egui::Button::selectable(tool == Tool::Label, "Label"));
            if label.on_hover_text("L (schematic only)").clicked() {
                out.push(Event::SetTool(Tool::Label));
            }
            if tool == Tool::Place {
                ui.add(egui::Button::selectable(true, "Place"));
            }
            ui.separator();
            if ui.add_enabled(can_undo, egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                out.push(Event::Command(Command::Undo));
            }
            if ui.add_enabled(can_redo, egui::Button::new("Redo")).on_hover_text("Ctrl+Y").clicked() {
                out.push(Event::Command(Command::Redo));
            }
        });
        out
    }

    /// Edit > Add part: a filterable list of the library's types (a typed `dip8` or `header4`
    /// works too). Returns the chosen type.
    pub fn add_part_menu(&mut self, ui: &mut egui::Ui, lib: &Library) -> Option<String> {
        let mut picked = None;
        ui.menu_button("Add part", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.add_filter).hint_text("filter, or dip8 / header4"));
            let types = partbrowser::filter_types(lib.types(), &self.add_filter);
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for t in &types {
                    let suffix = lib.get(t).map(|d| crate::core_api::view_names(d).replace("schematic", "sch").replace("breadboard", "bb"));
                    let text = format!("{t}  ({})", suffix.unwrap_or_default());
                    if ui.button(text).clicked() {
                        picked = Some(t.clone());
                        ui.close();
                    }
                }
                let typed = self.add_filter.trim();
                if !typed.is_empty() && !types.iter().any(|t| t == typed) && builtin::generate(typed).is_some() {
                    if ui.button(format!("Use {typed}")).clicked() {
                        picked = Some(typed.to_string());
                        ui.close();
                    }
                }
            });
        });
        picked
    }

    /// The popup asking for a net name; returns the confirming or cancelling event.
    pub fn label_prompt(&mut self, ctx: &egui::Context) -> Option<Event> {
        let at = self.editor.label_prompt()?;
        let mut out = None;
        egui::Window::new("Net label")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Name of the net at ({}, {})", at[0], at[1]));
                let resp = ui.add(egui::TextEdit::singleline(&mut self.label_text).hint_text("VCC, GND, ..."));
                if !self.label_focus {
                    resp.request_focus();
                    self.label_focus = true;
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() || enter {
                        out = Some(Event::CommitLabel { name: std::mem::take(&mut self.label_text) });
                    }
                    if ui.button("Cancel").clicked() || esc {
                        out = Some(Event::CancelLabel);
                    }
                });
            });
        if out.is_some() {
            self.label_focus = false;
            self.label_text.clear();
        }
        out
    }

    // ---- properties panel -------------------------------------------------------------------

    fn field_error(&self, ui: &mut egui::Ui, key: &str) {
        if let Some((k, msg)) = &self.err {
            if k == key {
                ui.add(egui::Label::new(RichText::new(msg).small().color(Color32::from_rgb(230, 90, 90))).wrap());
            }
        }
    }

    fn color_row(&self, ui: &mut egui::Ui, acts: &mut Vec<(String, Act)>, key: &str, name: &str, current: Option<&str>, mk: &dyn Fn(String) -> Act) {
        ui.horizontal(|ui| {
            ui.label(name);
            if let Some(text) = color_edit(ui, key, current, true) {
                acts.push((key.to_string(), mk(text)));
            }
        });
        self.field_error(ui, key);
    }

    /// The right-hand panel of Edit mode. Returns whether the circuit changed.
    pub fn props_panel(&mut self, ui: &mut egui::Ui, e: &mut EditCtx) -> PanelOut {
        let mut out = PanelOut::default();
        if !self.show_props {
            return out;
        }
        let sel = self.editor.selection().clone();
        if sel != self.err_sel {
            self.err = None;
            self.err_sel = sel.clone();
        }
        let sk = item_key(&sel);
        let view = e.view;
        let mut acts: Vec<(String, Act)> = Vec::new();
        egui::Panel::right("props").default_size(270.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match sel.single() {
                Some(Item::Part(id)) => {
                    if let Some(p) = e.circuit.part(id).cloned() {
                        self.part_ui(ui, &mut acts, e, &p, view, &sk);
                    }
                }
                Some(Item::Wire(id)) => {
                    if let Some(w) = e.circuit.wires.iter().find(|w| &w.id == id).cloned() {
                        ui.heading("Wire");
                        ui.horizontal(|ui| {
                            ui.label("id");
                            if let Some(t) = text_commit(ui, &format!("{sk}:id"), &w.id, 110.0) {
                                acts.push((format!("{sk}:id"), Act::Rename(w.id.clone(), t)));
                            }
                        });
                        self.field_error(ui, &format!("{sk}:id"));
                        ui.label(format!("view: {}, {} points", w.view.key(), w.pts.len()));
                        let wid = w.id.clone();
                        self.color_row(ui, &mut acts, &format!("{sk}:color"), "colour", w.color.as_deref(), &move |t| Act::WireColor(wid.clone(), t));
                        if ui.button("Delete wire").clicked() {
                            acts.push((sk.clone(), Act::DeleteSelection));
                        }
                    }
                }
                Some(Item::Label(k)) => {
                    if let Some(l) = e.circuit.labels.get(*k).cloned() {
                        ui.heading("Label");
                        let k = *k;
                        ui.horizontal(|ui| {
                            ui.label("name");
                            if let Some(t) = text_commit(ui, &format!("{sk}:name"), &l.name, 110.0) {
                                acts.push((format!("{sk}:name"), Act::LabelName(k, t)));
                            }
                        });
                        self.field_error(ui, &format!("{sk}:name"));
                        ui.label(format!("at ({}, {})", l.at[0], l.at[1]));
                        self.color_row(ui, &mut acts, &format!("{sk}:color"), "colour", l.color.as_deref(), &move |t| Act::LabelColor(k, t));
                        if ui.button("Delete label").clicked() {
                            acts.push((sk.clone(), Act::DeleteSelection));
                        }
                    }
                }
                None if sel.is_empty() => self.doc_ui(ui, &mut acts, e),
                None => {
                    ui.heading(format!("{} items selected", sel.items.len()));
                    ui.label("R rotates, M mirrors, arrows nudge, drag moves.");
                    if ui.button("Delete selection").clicked() {
                        acts.push((sk.clone(), Act::DeleteSelection));
                    }
                }
            });
        });
        for (key, act) in acts {
            match self.run(e, act) {
                Ok(msg) => {
                    out.changed = true;
                    self.err = None;
                    if msg.is_some() {
                        out.status = msg;
                    }
                }
                Err(m) => self.err = Some((key, m)),
            }
        }
        out
    }

    fn run(&mut self, e: &mut EditCtx, act: Act) -> Result<Option<String>, String> {
        let v = e.view;
        match act {
            Act::Rename(old, new) => props::rename_id(&mut self.editor, e, &old, &new)?,
            Act::SetProp(id, k, val) => props::set_prop(e, &id, &k, val.as_deref())?,
            Act::AddProp(id, k, val) => props::add_prop(e, &id, &k, &val)?,
            Act::RenameKey(id, old, new) => props::rename_prop_key(e, &id, &old, &new)?,
            Act::PartColor(id, f, t) => props::set_part_color(e, &id, f, &t)?,
            Act::Hidden(id, h) => props::set_hidden_label(e, &id, h)?,
            Act::Placement(id, view, pl) => props::set_placement(e, &id, view, pl)?,
            Act::RemoveFromView(id, view) => props::remove_from_view(e, &id, view)?,
            Act::PlaceIn(id, view) => {
                let at = props::place_in_view(e, &id, view)?;
                return Ok(Some(format!("Placed {id} at ({}, {}) in the {} view", at[0], at[1], view.key())));
            }
            Act::WireColor(id, t) => props::set_wire_color(e, &id, &t)?,
            Act::LabelName(k, t) => props::rename_label(e, k, &t)?,
            Act::LabelColor(k, t) => props::set_label_color(e, k, &t)?,
            Act::Title(t) => props::set_title(e, &t)?,
            Act::Fail(m) => return Err(m),
            Act::DeclareNets => {
                let n = props::declare_nets_from_view(e)?;
                return Ok(Some(format!("Declared {n} net(s) from the {} view", v.key())));
            }
            Act::ClearNets => props::clear_declared_nets(e)?,
            Act::DeleteNet(i) => props::delete_declared_net(e, i)?,
            Act::DeleteSelection => {
                let mut ed = std::mem::take(&mut self.editor);
                let fx = ed.handle(Event::Command(Command::Delete), e);
                self.editor = ed;
                return Ok(fx.into_iter().find_map(|f| if let crate::editor::Effect::Status(s) = f { Some(s) } else { None }));
            }
        }
        Ok(None)
    }

    fn doc_ui(&self, ui: &mut egui::Ui, acts: &mut Vec<(String, Act)>, e: &EditCtx) {
        ui.heading("Circuit");
        ui.horizontal(|ui| {
            ui.label("title");
            if let Some(t) = text_commit(ui, "doc:title", e.circuit.title.as_deref().unwrap_or(""), 170.0) {
                acts.push(("doc:title".into(), Act::Title(t)));
            }
        });
        self.field_error(ui, "doc:title");
        ui.separator();
        ui.strong("Declared nets");
        if e.circuit.nets.is_empty() {
            ui.label(RichText::new("none (the schematic is the reference)").weak());
        }
        for (i, n) in e.circuit.nets.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("x").on_hover_text("delete this net").clicked() {
                    acts.push(("doc:nets".into(), Act::DeleteNet(i)));
                }
                let name = n.name.as_deref().unwrap_or("-");
                ui.add(egui::Label::new(format!("{name}: {}", n.pins.join(" "))).wrap());
            });
        }
        ui.horizontal(|ui| {
            if ui.button(format!("Declare nets from this view")).on_hover_text("nets with two or more pins of parts, named by their label").clicked() {
                acts.push(("doc:nets".into(), Act::DeclareNets));
            }
            if ui.add_enabled(!e.circuit.nets.is_empty(), egui::Button::new("Clear")).clicked() {
                acts.push(("doc:nets".into(), Act::ClearNets));
            }
        });
        self.field_error(ui, "doc:nets");
        ui.separator();
        ui.label(RichText::new("Select something to edit its properties.").weak());
    }

    fn part_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<(String, Act)>, e: &EditCtx, p: &Part, view: View, sk: &str) {
        ui.heading(&p.id);
        let def = e.lib.get(&p.ty);
        ui.horizontal(|ui| {
            ui.label("id");
            if let Some(t) = text_commit(ui, &format!("{sk}:id"), &p.id, 110.0) {
                acts.push((format!("{sk}:id"), Act::Rename(p.id.clone(), t)));
            }
        });
        self.field_error(ui, &format!("{sk}:id"));
        ui.horizontal(|ui| {
            ui.label("type");
            ui.label(match def {
                Some(d) if !d.name.is_empty() => format!("{} ({})", p.ty, d.name),
                Some(_) => p.ty.clone(),
                None => format!("{} (unknown)", p.ty),
            });
        });
        ui.separator();
        // The value is the prominent field.
        ui.horizontal(|ui| {
            ui.label(RichText::new("value").strong());
            let cur = p.props.get("value").map(String::as_str).unwrap_or("");
            if let Some(t) = text_commit(ui, &format!("{sk}:value"), cur, 150.0) {
                let v = (!t.trim().is_empty()).then(|| t.trim().to_string());
                acts.push((format!("{sk}:value"), Act::SetProp(p.id.clone(), "value".into(), v)));
            }
        });
        ui.strong("Properties");
        for (k, v) in p.props.iter().filter(|(k, _)| k.as_str() != "value") {
            ui.horizontal(|ui| {
                if let Some(t) = text_commit(ui, &format!("{sk}:key:{k}"), k, 70.0) {
                    acts.push((format!("{sk}:props"), Act::RenameKey(p.id.clone(), k.clone(), t)));
                }
                if let Some(t) = text_commit(ui, &format!("{sk}:val:{k}"), v, 90.0) {
                    acts.push((format!("{sk}:props"), Act::SetProp(p.id.clone(), k.clone(), Some(t))));
                }
                if ui.small_button("x").on_hover_text("delete property").clicked() {
                    acts.push((format!("{sk}:props"), Act::SetProp(p.id.clone(), k.clone(), None)));
                }
            });
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_key).hint_text("name").desired_width(70.0));
            ui.add(egui::TextEdit::singleline(&mut self.new_value).hint_text("value").desired_width(90.0));
            if ui.button("Add").clicked() {
                acts.push((format!("{sk}:props"), Act::AddProp(p.id.clone(), self.new_key.clone(), self.new_value.clone())));
                if !self.new_key.trim().is_empty() && !p.props.contains_key(self.new_key.trim()) {
                    self.new_key.clear();
                    self.new_value.clear();
                }
            }
        });
        self.field_error(ui, &format!("{sk}:props"));
        ui.separator();
        ui.strong("Colours");
        for f in ColorField::ALL {
            let cur = match f {
                ColorField::Color => p.color.as_deref(),
                ColorField::Fill => p.fill.as_deref(),
                ColorField::LabelColor => p.label_color.as_deref(),
            };
            let id = p.id.clone();
            self.color_row(ui, acts, &format!("{sk}:{}", f.key()), f.key(), cur, &move |t| Act::PartColor(id.clone(), f, t));
        }
        let mut hidden = p.hidden_label;
        if ui.checkbox(&mut hidden, "hidden_label").changed() {
            acts.push((format!("{sk}:hidden"), Act::Hidden(p.id.clone(), hidden)));
        }
        ui.separator();
        ui.strong("Placement");
        for v in [view, view.other()] {
            let has_geometry = def.is_some_and(|d| d.view(v).is_some());
            match p.placement(v) {
                Some(pl) => {
                    ui.label(RichText::new(format!("{} view{}", v.key(), if v == view { " (this view)" } else { "" })).italics());
                    let key = format!("{sk}:{}", v.key());
                    ui.horizontal(|ui| {
                        let mut np = pl.clone();
                        let mut changed = false;
                        for (axis, name) in [(0usize, "x"), (1, "y")] {
                            ui.label(name);
                            let cur = pl.at[axis].to_string();
                            if let Some(t) = text_commit(ui, &format!("{key}:{name}"), &cur, 40.0) {
                                if let Ok(n) = t.trim().parse::<i32>() {
                                    np.at[axis] = n;
                                    changed = true;
                                } else {
                                    acts.push((key.clone(), Act::Fail(format!("\"{}\" is not a whole number", t.trim()))));
                                }
                            }
                        }
                        ui.label("rot");
                        egui::ComboBox::from_id_salt(format!("{key}:rot")).selected_text(pl.rot.to_string()).width(48.0).show_ui(ui, |ui| {
                            for r in [0, 90, 180, 270] {
                                if ui.selectable_label(pl.rot == r, r.to_string()).clicked() {
                                    np.rot = r;
                                    changed = true;
                                }
                            }
                        });
                        let mut m = pl.mirror;
                        if ui.checkbox(&mut m, "mirror").changed() {
                            np.mirror = m;
                            changed = true;
                        }
                        if changed {
                            acts.push((key.clone(), Act::Placement(p.id.clone(), v, np)));
                        }
                    });
                    let label = if v == view { "Remove from this view".to_string() } else { format!("Remove from {} view", v.key()) };
                    if ui.button(label).clicked() {
                        acts.push((key.clone(), Act::RemoveFromView(p.id.clone(), v)));
                    }
                    self.field_error(ui, &key);
                }
                None if has_geometry => {
                    let key = format!("{sk}:{}", v.key());
                    if ui.button(format!("Place in {} view", v.key())).clicked() {
                        acts.push((key.clone(), Act::PlaceIn(p.id.clone(), v)));
                    }
                    self.field_error(ui, &key);
                }
                None => {
                    ui.label(RichText::new(format!("{} view: the type has no geometry for it", v.key())).weak());
                }
            }
        }
        ui.separator();
        if ui.button("Delete part").clicked() {
            acts.push((sk.to_string(), Act::DeleteSelection));
        }
    }
}
