//! Tools > Autoroute...: the dialog around `route::port`.
//!
//! The dialog binds every `RouteOptions` field to a control, previews the result on a worker
//! thread (the circuit and library are cloned, so a large circuit never freezes the window),
//! and hands the outcome back to the app, which commits it as ONE undo step through the editor's
//! history (`commit`). Everything that decides something is a plain function here (defaults from
//! the active view and selection, scope from a part, the equivalent command line, what blocks the
//! buttons, the commit); the egui code below only draws them.

use crate::breadboard;
use crate::canvas::{self, DrawCtx, Highlight};
use crate::core_api;
use crate::edit_ui;
use crate::editor::EditCtx;
use crate::tex::TexCache;
use eframe::egui::{self, Color32, RichText};
use short_circ_core::color::{parse_color, Rgba};
use short_circ_core::edit::History;
use short_circ_core::route::cli::options_to_args;
use short_circ_core::route::{port, JumperColor, JumperStyle, Prefer, RouteOptions, RouteOutcome, Scope, Source};
use short_circ_core::write::circuit_to_string;
use short_circ_core::{view_in_use, Circuit, Library, PartLookup, Pt, View};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

// ---- decisions (plain functions) -------------------------------------------------------------

/// The view to produce when the dialog opens: the other view of the active tab when the active
/// view has parts (working in the schematic, port to the breadboard and the other way round); the
/// active view itself when only the other one has parts (it is the one to fill); else the
/// default target.
pub fn default_to(circuit: &Circuit, active: View) -> View {
    if view_in_use(circuit, active) {
        active.other()
    } else if view_in_use(circuit, active.other()) {
        active
    } else {
        RouteOptions::default().to
    }
}

/// Options when the dialog is first opened: the defaults, the target from [`default_to`], and
/// the declared nets as the source for a circuit that has nets but nothing placed anywhere.
pub fn default_options(circuit: &Circuit, active: View) -> RouteOptions {
    let mut o = RouteOptions { to: default_to(circuit, active), ..RouteOptions::default() };
    if !View::ALL.iter().any(|v| view_in_use(circuit, *v)) && !circuit.nets.is_empty() {
        o.source = Source::Declared;
    }
    o
}

/// `PART.PIN` of every pin of a part, for `Scope::Only` (nets of the selected part). Boards give
/// nothing (hundreds of holes are not "the part's nets"), as does an unknown part or type.
pub fn selected_pin_refs(circuit: &Circuit, lib: &dyn PartLookup, part_id: &str) -> Vec<String> {
    let Some(part) = circuit.part(part_id) else { return Vec::new() };
    match lib.get(&part.ty) {
        Some(def) if !def.is_board() => def.pins.iter().map(|p| format!("{}.{}", part.id, p.id)).collect(),
        _ => Vec::new(),
    }
}

/// Keep a `Scope::Only` in step with the selection: it follows the selected part's pins and falls
/// back to `All` when nothing usable is selected. Other scopes are unchanged.
pub fn sync_scope(scope: &Scope, refs: &[String]) -> Scope {
    match scope {
        Scope::Only(_) if refs.is_empty() => Scope::All,
        Scope::Only(_) => Scope::Only(refs.to_vec()),
        other => other.clone(),
    }
}

/// The options with everything that does not apply to the target view reset to its default, so
/// the command line shows only what matters.
pub fn normalized(o: &RouteOptions) -> RouteOptions {
    let d = RouteOptions::default();
    let mut o = o.clone();
    match o.to {
        View::Schematic => {
            o.jumper_style = d.jumper_style;
            o.jumper_color = d.jumper_color;
            o.max_jumper_len = d.max_jumper_len;
        }
        View::Breadboard => {
            o.allow_crossings = d.allow_crossings;
            o.prefer = d.prefer;
            o.clearance = d.clearance;
            o.label_fallback = d.label_fallback;
        }
    }
    o
}

fn quote(s: &str) -> String {
    if s.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// `short-circ.exe --autoroute IN OUT <flags>` for these options. `--to` is always written so the
/// target is explicit; the other flags are the ones that differ from the defaults.
pub fn command_line(o: &RouteOptions, file: Option<&str>) -> String {
    let o = normalized(o);
    let mut args = options_to_args(&o);
    if !args.iter().any(|a| a == "--to") {
        args.splice(0..0, ["--to".to_string(), o.to.key().to_string()]);
    }
    let mut s = format!("short-circ.exe --autoroute {} routed.circ", quote(file.unwrap_or("file.circ")));
    for a in &args {
        s.push(' ');
        s.push_str(&quote(a));
    }
    s
}

/// Why Preview and Apply are disabled, if they are.
pub fn blocked_reason(c: &Circuit, o: &RouteOptions) -> Option<&'static str> {
    if c.parts.is_empty() {
        return Some("The circuit is empty: there is nothing to route.");
    }
    let placed = View::ALL.iter().any(|v| view_in_use(c, *v));
    if !placed && !(o.source == Source::Declared && !c.nets.is_empty()) {
        return Some("Both views are empty: place some parts first.");
    }
    None
}

/// Only a real colour makes a useful jumper colour (the command line also takes `none`).
pub fn valid_jumper_color(s: &str) -> bool {
    parse_color(s).is_some()
}

/// The status line after Apply; same words as the `--autoroute` summary.
pub fn summary_line(o: &RouteOutcome) -> String {
    format!(
        "Autoroute: placed {} part(s), added {} wire(s), routed {} net(s), {} failed",
        o.placed.len(),
        o.wires_added.len(),
        o.routed,
        o.failures.len()
    )
}

/// Commit the outcome as ONE undo step through the editor's history (the same mechanism as the
/// tidy command). Returns whether the circuit changed; when it did not, nothing is recorded.
pub fn commit(circuit: &mut Circuit, history: &mut History, lib: &dyn PartLookup, outcome: &Circuit, view: View) -> bool {
    let mut e = EditCtx::new(circuit, history, lib, view);
    e.apply_direct(|c, _| *c = outcome.clone())
}

// ---- the preview ----------------------------------------------------------------------------

/// A finished `port` run with what drawing it needs.
pub struct PreviewData {
    /// The options it was run with.
    pub opts: RouteOptions,
    pub outcome: RouteOutcome,
    /// Library that resolves every type of the outcome (it may have added a breadboard).
    pub lib: Library,
    pub junctions: Vec<Pt>,
    pub jumpers: Vec<Option<Rgba>>,
    /// The outcome differs from the circuit it was made from.
    pub changed: bool,
}

impl PreviewData {
    /// The view the preview replaces.
    pub fn to(&self) -> View {
        self.opts.to
    }

    pub fn summary(&self) -> String {
        summary_line(&self.outcome)
    }

    /// New parts' pins and new wires, drawn in the accent colour.
    pub fn highlight(&self) -> Highlight {
        let view = self.to();
        let mut hl = Highlight { wires: self.outcome.wires_added.clone(), pins: Vec::new() };
        for id in &self.outcome.placed {
            let Some(part) = self.outcome.circuit.part(id) else { continue };
            let Some(def) = self.lib.get(&part.ty) else { continue };
            if def.is_board() {
                continue;
            }
            hl.pins.extend(def.placed_pins(view).map(|(pin, _)| (part.id.clone(), pin.id.clone())));
        }
        hl
    }
}

/// What the worker thread runs (and what Apply runs without a preview): `port` on the given
/// circuit, then everything derived that drawing the result needs.
pub fn compute_preview(circuit: &Circuit, lib: &Library, dir: Option<&Path>, opts: &RouteOptions) -> PreviewData {
    let outcome = port(circuit, lib, opts);
    let changed = circuit_to_string(circuit) != circuit_to_string(&outcome.circuit);
    let plib = Library::load(&outcome.circuit, dir);
    let junctions = canvas::junctions(&outcome.circuit.wires);
    let jumpers = core_api::jumper_colors(&outcome.circuit, &plib);
    PreviewData { opts: opts.clone(), outcome, lib: plib, junctions, jumpers, changed }
}

type Msg = (u64, Result<Box<PreviewData>, String>);

struct Running {
    gen: u64,
    /// Apply as soon as the result arrives (Apply pressed without a fresh preview).
    apply: bool,
}

/// What the dialog needs to know about the document each frame.
pub struct Env<'a> {
    pub circuit: &'a Circuit,
    pub lib: &'a Library,
    pub dir: Option<&'a Path>,
    pub file_name: Option<&'a str>,
    /// Id of the selected part, if exactly one is selected.
    pub selected: Option<&'a str>,
}

/// State of the dialog.
pub struct AutorouteUi {
    pub open: bool,
    opts: RouteOptions,
    initialised: bool,
    /// Remembered while the "Max jumper length" box is off.
    max_len: i32,
    /// Remembered while the jumper colour is "By net".
    single_color: String,
    color_err: Option<String>,
    gen: u64,
    running: Option<Running>,
    preview: Option<Box<PreviewData>>,
    /// A preview or run was discarded because the options (or the document) changed.
    stale: bool,
    error: Option<String>,
    /// Target of a preview nobody looked at yet.
    arrived: Option<View>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
}

impl Default for AutorouteUi {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        AutorouteUi {
            open: false,
            opts: RouteOptions::default(),
            initialised: false,
            max_len: 12,
            single_color: "teal".into(),
            color_err: None,
            gen: 0,
            running: None,
            preview: None,
            stale: false,
            error: None,
            arrived: None,
            tx,
            rx,
        }
    }
}

impl AutorouteUi {
    #[cfg(test)]
    pub fn options(&self) -> &RouteOptions {
        &self.opts
    }

    /// Replace the options; a change discards the preview like a click on a control does.
    #[cfg(test)]
    pub fn set_options(&mut self, o: RouteOptions) {
        if o != self.opts {
            self.opts = o;
            self.invalidate();
        }
    }

    /// Open the dialog for this document. The first time everything starts from the defaults;
    /// later the options are kept, except that the target follows the active view again and a
    /// "selected part" scope is dropped.
    pub fn open(&mut self, circuit: &Circuit, active: View) {
        if !self.initialised {
            self.opts = default_options(circuit, active);
            self.initialised = true;
        } else {
            self.opts.to = default_to(circuit, active);
            self.opts.scope = sync_scope(&self.opts.scope, &[]);
            if self.opts.source == Source::Declared && circuit.nets.is_empty() {
                self.opts.source = Source::OtherView;
            }
        }
        self.cancel();
        self.preview = None;
        self.stale = false;
        self.error = None;
        self.open = true;
    }

    /// Close the dialog: the preview goes away and a running job is ignored.
    pub fn close(&mut self) {
        self.open = false;
        self.cancel();
        self.preview = None;
        self.arrived = None;
        self.stale = false;
    }

    /// Ignore the running job: its result, when it arrives, is dropped.
    pub fn cancel(&mut self) {
        if self.running.take().is_some() {
            self.gen += 1;
        }
    }

    /// The options or the document changed: drop the preview and any running job. The user
    /// presses Preview again.
    pub fn invalidate(&mut self) {
        let had = self.preview.is_some() || self.running.is_some();
        self.cancel();
        self.preview = None;
        self.arrived = None;
        self.stale = self.stale || had;
    }

    /// A preview is held: the canvas shows it and editing is blocked.
    pub fn holds_preview(&self) -> bool {
        self.preview.is_some()
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    #[cfg(test)]
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    pub fn preview(&self) -> Option<&PreviewData> {
        self.preview.as_deref()
    }

    /// The preview if it replaces `view`.
    pub fn preview_for(&self, view: View) -> Option<&PreviewData> {
        self.preview().filter(|p| p.to() == view)
    }

    /// Run `port` on a worker thread with the current options. `repaint` is woken when the
    /// result is in. With `apply` the result is handed to the app by [`poll`](Self::poll)
    /// instead of being held as a preview.
    pub fn start(&mut self, repaint: Option<egui::Context>, circuit: &Circuit, lib: &Library, dir: Option<&Path>, apply: bool) {
        self.cancel();
        self.gen += 1;
        let gen = self.gen;
        self.running = Some(Running { gen, apply });
        self.preview = None;
        self.stale = false;
        self.error = None;
        let (c, l, d, o, tx) = (circuit.clone(), lib.clone(), dir.map(Path::to_path_buf), self.opts.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compute_preview(&c, &l, d.as_deref(), &o)));
            let msg = run.map(Box::new).map_err(|_| "the autorouter stopped unexpectedly".to_string());
            let _ = tx.send((gen, msg));
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    /// Take in results of the worker. Returns the data to apply when the job was started with
    /// `apply`.
    pub fn poll(&mut self) -> Option<Box<PreviewData>> {
        while let Ok((gen, res)) = self.rx.try_recv() {
            if let Some(d) = self.deliver(gen, res) {
                return Some(d);
            }
        }
        None
    }

    /// A result arrived. Results of cancelled or superseded jobs are ignored.
    fn deliver(&mut self, gen: u64, res: Result<Box<PreviewData>, String>) -> Option<Box<PreviewData>> {
        let apply = match &self.running {
            Some(r) if r.gen == gen => r.apply,
            _ => return None,
        };
        self.running = None;
        match res {
            Err(e) => {
                self.error = Some(e);
                None
            }
            Ok(d) if apply => Some(d),
            Ok(d) => {
                self.arrived = Some(d.to());
                self.preview = Some(d);
                None
            }
        }
    }

    /// The target view of a preview that arrived since the last call (the app shows that view).
    pub fn take_arrived(&mut self) -> Option<View> {
        self.arrived.take()
    }

    /// The held preview, for Apply.
    pub fn take_preview(&mut self) -> Option<Box<PreviewData>> {
        self.preview.take()
    }

    // ---- drawing ------------------------------------------------------------------------

    /// Draw the preview in `view`: the outcome circuit with new parts boxed and new wires and pins
    /// in the accent colour. Returns false when there is nothing to draw for this view.
    pub fn draw_preview(&self, d: &DrawCtx, tex: &mut TexCache, view: View) -> bool {
        let Some(p) = self.preview_for(view) else { return false };
        let c = &p.outcome.circuit;
        let hl = p.highlight();
        match view {
            View::Schematic => canvas::draw_schematic(d, c, &p.lib, &p.junctions, &hl, None),
            View::Breadboard => breadboard::draw(d, tex, c, &p.lib, &p.jumpers, &hl, None),
        }
        for id in &p.outcome.placed {
            let Some(part) = c.part(id) else { continue };
            let Some(pl) = part.placement(view) else { continue };
            let (lo, hi) = canvas::part_box(pl, p.lib.get(&part.ty), view);
            canvas::draw_selection(d, lo, hi);
        }
        true
    }

    /// Banner above the canvas while a preview is held.
    pub fn banner(&self, ui: &mut egui::Ui) {
        if !self.holds_preview() {
            return;
        }
        egui::Panel::top("autoroute_banner").show(ui, |ui| {
            ui.label(RichText::new("Autoroute preview - Apply or Close in the dialog").strong().color(Color32::from_rgb(235, 175, 40)));
        });
    }

    /// The dialog. Returns the outcome to commit when Apply was pressed (with a fresh preview) or
    /// finished running; the app commits it and calls [`close`](Self::close).
    pub fn show(&mut self, ctx: &egui::Context, env: Option<&Env>) -> Option<Box<PreviewData>> {
        if !self.open {
            return None;
        }
        let mut applied = self.poll();
        let before = self.opts.clone();
        let mut open = true;
        let mut want = Want::None;
        egui::Window::new("Autoroute").open(&mut open).collapsible(false).default_width(440.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(ctx.content_rect().height() - 90.0).show(ui, |ui| match env {
                Some(env) => want = self.body(ui, env),
                None => {
                    ui.label("No circuit is open.");
                    if ui.button("Close").clicked() {
                        want = Want::Close;
                    }
                }
            });
        });
        if self.opts != before {
            self.invalidate();
        }
        if !open {
            want = Want::Close;
        }
        match (want, env) {
            (Want::Close, _) => self.close(),
            (Want::Cancel, _) => self.cancel(),
            (Want::Preview, Some(env)) => self.start(Some(ctx.clone()), env.circuit, env.lib, env.dir, false),
            (Want::Apply, Some(env)) => match self.take_preview() {
                Some(p) => applied = Some(p),
                None => self.start(Some(ctx.clone()), env.circuit, env.lib, env.dir, true),
            },
            _ => {}
        }
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        applied
    }

    fn body(&mut self, ui: &mut egui::Ui, env: &Env) -> Want {
        let c = env.circuit;
        let refs = env.selected.map(|id| selected_pin_refs(c, env.lib, id)).unwrap_or_default();
        let has_nets = !c.nets.is_empty();
        if !has_nets && self.opts.source == Source::Declared {
            self.opts.source = Source::OtherView;
        }
        let to = self.opts.to;
        let blocked = blocked_reason(c, &self.opts);
        if let Some(why) = blocked {
            ui.label(RichText::new(why).color(Color32::from_rgb(230, 90, 90)));
            ui.add_space(4.0);
        }

        // Target and source.
        ui.horizontal(|ui| {
            ui.label("Target view:");
            ui.radio_value(&mut self.opts.to, View::Schematic, "Schematic");
            ui.radio_value(&mut self.opts.to, View::Breadboard, "Breadboard");
        });
        ui.horizontal(|ui| {
            ui.label("Nets from:");
            ui.radio_value(&mut self.opts.source, Source::OtherView, format!("Other view ({})", to.other().key()));
            let declared = ui.add_enabled(has_nets, egui::RadioButton::new(self.opts.source == Source::Declared, "Declared nets"));
            if declared.clicked() {
                self.opts.source = Source::Declared;
            }
            declared.on_disabled_hover_text("This circuit has no declared nets (the \"nets\" list of the file)");
        });

        // Scope.
        let sel_text = match (env.selected, refs.is_empty()) {
            (Some(id), false) => format!("Only nets of the selected part ({id})"),
            _ => "Only nets of the selected part".to_string(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label("Scope:");
            let mut scope = self.opts.scope.clone();
            if ui.radio(matches!(scope, Scope::All), "All nets").clicked() {
                scope = Scope::All;
            }
            if ui.radio(matches!(scope, Scope::Unrouted), "Only unrouted nets").clicked() {
                scope = Scope::Unrouted;
            }
            let only = ui.add_enabled(!refs.is_empty(), egui::RadioButton::new(matches!(scope, Scope::Only(_)), sel_text));
            if only.clicked() {
                scope = Scope::Only(refs.clone());
            }
            only.on_disabled_hover_text("Select a part (not a board) in the canvas first");
            self.opts.scope = scope;
        });
        self.opts.scope = sync_scope(&self.opts.scope, &refs);

        ui.separator();
        ui.checkbox(&mut self.opts.place_missing, "Place missing parts")
            .on_hover_text("Place parts that exist in the other view but not in the target view");
        ui.checkbox(&mut self.opts.keep_wires, "Keep existing wires in the target view")
            .on_hover_text("Existing wires count as routed connections and as obstacles");
        if !self.opts.keep_wires {
            ui.label(
                RichText::new(format!("Warning: the {} view's existing wires will be removed first.", to.key()))
                    .color(Color32::from_rgb(235, 175, 40)),
            );
        }

        // Options of the chosen target only.
        match to {
            View::Schematic => {
                ui.separator();
                ui.checkbox(&mut self.opts.allow_crossings, "Allow wire crossings").on_hover_text("Wires may cross instead of detouring");
                ui.checkbox(&mut self.opts.label_fallback, "Label nets that cannot be routed")
                    .on_hover_text("A net that cannot be wired gets same-name labels at its pins");
                ui.horizontal(|ui| {
                    ui.label("Clearance:");
                    ui.add(egui::DragValue::new(&mut self.opts.clearance).range(0..=6).suffix(" units"))
                        .on_hover_text("Grid units kept free around other parts");
                });
                ui.horizontal(|ui| {
                    ui.label("Prefer:");
                    ui.radio_value(&mut self.opts.prefer, Prefer::Bends, "Fewer bends");
                    ui.radio_value(&mut self.opts.prefer, Prefer::Length, "Shorter wires");
                });
            }
            View::Breadboard => {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Jumper style:");
                    ui.radio_value(&mut self.opts.jumper_style, JumperStyle::Straight, "Straight");
                    ui.radio_value(&mut self.opts.jumper_style, JumperStyle::Orthogonal, "Orthogonal");
                    ui.radio_value(&mut self.opts.jumper_style, JumperStyle::Diagonal45, "45-degree");
                });
                self.jumper_color_ui(ui);
                ui.horizontal(|ui| {
                    let mut on = self.opts.max_jumper_len.is_some();
                    if ui.checkbox(&mut on, "Max jumper length").clicked() {
                        self.opts.max_jumper_len = on.then_some(self.max_len);
                    }
                    if let Some(n) = &mut self.opts.max_jumper_len {
                        ui.add(egui::DragValue::new(n).range(1..=200).suffix(" units"));
                        self.max_len = *n;
                    }
                });
            }
        }

        ui.separator();
        ui.label("Equivalent command line:");
        let cmd = command_line(&self.opts, env.file_name);
        ui.add(egui::TextEdit::multiline(&mut cmd.as_str()).desired_width(f32::INFINITY).desired_rows(2).font(egui::TextStyle::Monospace));

        // Buttons.
        ui.separator();
        let mut want = Want::None;
        ui.horizontal(|ui| {
            let running = self.running.is_some();
            let can = blocked.is_none() && !running;
            if ui.add_enabled(can, egui::Button::new("Preview")).on_hover_text("Run the autorouter and show the result on the canvas").clicked() {
                want = Want::Preview;
            }
            if ui.add_enabled(can, egui::Button::new("Apply")).on_hover_text("Commit the result as one undo step").clicked() {
                want = Want::Apply;
            }
            if ui.button("Close").clicked() {
                want = Want::Close;
            }
            if running {
                ui.spinner();
                ui.label("Routing...");
                if ui.button("Cancel").clicked() {
                    want = Want::Cancel;
                }
            }
        });

        self.result_ui(ui);
        want
    }

    fn jumper_color_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Jumper colour:");
            let single = matches!(self.opts.jumper_color, JumperColor::Single(_));
            if ui.radio(!single, "By net").clicked() {
                self.opts.jumper_color = JumperColor::ByNet;
            }
            if ui.radio(single, "Single colour").clicked() {
                self.opts.jumper_color = JumperColor::Single(self.single_color.clone());
            }
            if single {
                let current = self.single_color.clone();
                if let Some(text) = edit_ui::color_edit(ui, "autoroute:jumper", Some(&current), false) {
                    let t = text.trim().to_string();
                    if valid_jumper_color(&t) {
                        self.single_color = t.clone();
                        self.opts.jumper_color = JumperColor::Single(t);
                        self.color_err = None;
                    } else {
                        self.color_err = Some(format!("\"{t}\" is not a colour (#rrggbb or a preset name)"));
                    }
                }
            }
        });
        if let (true, Some(e)) = (matches!(self.opts.jumper_color, JumperColor::Single(_)), &self.color_err) {
            ui.label(RichText::new(e.as_str()).small().color(Color32::from_rgb(230, 90, 90)));
        }
    }

    fn result_ui(&self, ui: &mut egui::Ui) {
        if let Some(e) = &self.error {
            ui.separator();
            ui.label(RichText::new(e.as_str()).color(Color32::from_rgb(230, 90, 90)));
        }
        if self.running.is_some() {
            return;
        }
        let Some(p) = &self.preview else {
            if self.stale {
                ui.separator();
                ui.label(RichText::new("The options changed: press Preview again.").weak());
            }
            return;
        };
        ui.separator();
        ui.strong("Preview");
        let o = &p.outcome;
        egui::Grid::new("autoroute_counts").num_columns(2).show(ui, |ui| {
            for (name, n) in [("Parts placed", o.placed.len()), ("Wires added", o.wires_added.len()), ("Nets routed", o.routed), ("Nets failed", o.failures.len())] {
                ui.label(name);
                ui.label(n.to_string());
                ui.end_row();
            }
        });
        if !p.changed {
            ui.label(RichText::new("The autorouter would change nothing.").weak());
        }
        if !o.failures.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Failures").color(Color32::from_rgb(230, 90, 90)));
            egui::ScrollArea::vertical().id_salt("autoroute_failures").max_height(130.0).show(ui, |ui| {
                for f in &o.failures {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&f.net).monospace().strong());
                        ui.label(&f.reason);
                    });
                }
            });
        }
        ui.label(RichText::new("The canvas shows the result; new parts are boxed and new wires highlighted.").small().weak());
    }
}

enum Want {
    None,
    Preview,
    Apply,
    Cancel,
    Close,
}

/// Where the part in the active view is selected: the part id to offer for "nets of the selected
/// part" (used by the app to fill [`Env::selected`]).
pub fn selected_id(circuit: &Circuit, index: Option<usize>) -> Option<&str> {
    index.and_then(|i| circuit.parts.get(i)).map(|p| p.id.as_str())
}

/// The file name shown in the command line.
pub fn file_label(path: Option<&PathBuf>) -> Option<String> {
    path.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use short_circ_core::route::cli::parse_options;

    fn load(src: &str) -> (Circuit, Library) {
        let c = short_circ_core::parse_circuit(src).unwrap();
        let lib = Library::load(&c, None);
        (c, lib)
    }

    fn led() -> (Circuit, Library) {
        load(include_str!("../../../examples/led.circ"))
    }

    fn nets_only() -> (Circuit, Library) {
        load(include_str!("../../../examples/nets_only.circ"))
    }

    /// Run the worker's job to completion on this thread.
    fn wait_for_result(ui: &mut AutorouteUi) -> Option<Box<PreviewData>> {
        for _ in 0..600 {
            if let Some(d) = ui.poll() {
                return Some(d);
            }
            if ui.running.is_none() {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the worker did not finish");
    }

    /// led.circ is wired in both views already; without its jumper the breadboard has work to do.
    fn led_without_jumper() -> (Circuit, Library) {
        let (mut c, lib) = led();
        c.wires.retain(|w| w.id != "j1");
        (c, lib)
    }


    #[test]
    fn target_defaults_to_the_other_view_of_the_active_tab() {
        let (c, _) = led();
        assert_eq!(default_to(&c, View::Schematic), View::Breadboard);
        assert_eq!(default_to(&c, View::Breadboard), View::Schematic);
        assert_eq!(default_options(&c, View::Breadboard).to, View::Schematic);
        assert_eq!(default_options(&c, View::Schematic), RouteOptions::default());
        // Only the schematic has parts: from the breadboard tab the (empty) active view is the
        // one to fill; from the schematic tab the other view is.
        let (mut sch, _) = led();
        for p in &mut sch.parts {
            p.breadboard = None;
        }
        sch.wires.retain(|w| w.view == View::Schematic);
        assert_eq!(default_to(&sch, View::Schematic), View::Breadboard);
        assert_eq!(default_to(&sch, View::Breadboard), View::Breadboard);
        // Nothing placed anywhere: the default target, and declared nets when there are some.
        let (n, _) = nets_only();
        assert_eq!(default_to(&n, View::Schematic), RouteOptions::default().to);
        assert_eq!(default_options(&n, View::Schematic).source, Source::Declared);
        assert_eq!(default_options(&Circuit::default(), View::Schematic).source, Source::OtherView);
    }

    #[test]
    fn scope_follows_the_selected_part() {
        let (c, lib) = led();
        assert_eq!(selected_pin_refs(&c, &lib, "R1"), vec!["R1.a".to_string(), "R1.b".to_string()]);
        let led_refs = selected_pin_refs(&c, &lib, "D1");
        assert_eq!(led_refs.len(), 2);
        assert!(led_refs.iter().all(|r| r.starts_with("D1.")));
        assert!(selected_pin_refs(&c, &lib, "BB1").is_empty(), "a board has no nets of its own");
        assert!(selected_pin_refs(&c, &lib, "nope").is_empty());
        assert_eq!(selected_id(&c, Some(1)), Some("R1"));
        assert_eq!(selected_id(&c, Some(99)), None);
        assert_eq!(selected_id(&c, None), None);
        let refs = selected_pin_refs(&c, &lib, "R1");
        assert_eq!(sync_scope(&Scope::Only(vec!["x.y".into()]), &refs), Scope::Only(refs.clone()));
        assert_eq!(sync_scope(&Scope::Only(vec!["x.y".into()]), &[]), Scope::All);
        assert_eq!(sync_scope(&Scope::Unrouted, &[]), Scope::Unrouted);
        assert_eq!(sync_scope(&Scope::All, &refs), Scope::All);
    }

    #[test]
    fn the_selected_part_scope_routes_only_its_nets() {
        let (c, lib) = led();
        let refs = selected_pin_refs(&c, &lib, "R1");
        let all = compute_preview(&c, &lib, None, &RouteOptions { keep_wires: false, ..RouteOptions::default() });
        let only = compute_preview(&c, &lib, None, &RouteOptions { keep_wires: false, scope: Scope::Only(refs), ..RouteOptions::default() });
        assert!(only.outcome.routed < all.outcome.routed, "{} vs {}", only.outcome.routed, all.outcome.routed);
    }

    #[test]
    fn irrelevant_options_are_left_out_of_the_command_line() {
        let o = RouteOptions {
            to: View::Breadboard,
            allow_crossings: true,
            clearance: 3,
            prefer: Prefer::Length,
            label_fallback: false,
            jumper_style: JumperStyle::Straight,
            ..RouteOptions::default()
        };
        assert_eq!(command_line(&o, Some("my led.circ")), "short-circ.exe --autoroute \"my led.circ\" routed.circ --to breadboard --jumper straight");
        let o = RouteOptions {
            to: View::Schematic,
            jumper_color: JumperColor::Single("#112233".into()),
            max_jumper_len: Some(5),
            allow_crossings: true,
            scope: Scope::Only(vec!["R1.a".into(), "R1.b".into()]),
            ..RouteOptions::default()
        };
        let line = command_line(&o, None);
        assert_eq!(line, "short-circ.exe --autoroute file.circ routed.circ --to schematic --only R1.a,R1.b --crossings");
        // What is shown is what the CLI parses back (the normalised options).
        let args: Vec<String> = line.split_whitespace().skip(4).map(String::from).collect();
        assert_eq!(parse_options(&args), Ok(normalized(&o)));
        let single = RouteOptions { jumper_color: JumperColor::Single("teal".into()), ..RouteOptions::default() };
        assert!(command_line(&single, None).ends_with("--to breadboard --jumper-color teal"));
    }

    #[test]
    fn empty_circuits_block_the_buttons() {
        let o = RouteOptions::default();
        assert!(blocked_reason(&Circuit::default(), &o).unwrap().contains("empty"));
        let (mut c, _) = led();
        assert!(blocked_reason(&c, &o).is_none());
        for p in &mut c.parts {
            p.schematic = None;
            p.breadboard = None;
        }
        c.wires.clear();
        assert!(blocked_reason(&c, &o).unwrap().contains("Both views"));
        // Declared nets are a source of their own.
        let (n, _) = nets_only();
        assert!(blocked_reason(&n, &RouteOptions { source: Source::Declared, ..o.clone() }).is_none());
        assert!(blocked_reason(&n, &o).is_some());
    }

    #[test]
    fn jumper_colours_must_be_real_colours() {
        assert!(valid_jumper_color("teal") && valid_jumper_color(" #12ab34 "));
        assert!(!valid_jumper_color("none") && !valid_jumper_color("default") && !valid_jumper_color("nocolour") && !valid_jumper_color(""));
    }

    #[test]
    fn preview_runs_on_a_worker_and_does_not_touch_the_circuit() {
        let (c, lib) = led_without_jumper();
        let before = circuit_to_string(&c);
        let mut ui = AutorouteUi::default();
        ui.open(&c, View::Schematic);
        assert_eq!(ui.options().to, View::Breadboard);
        ui.start(None, &c, &lib, None, false);
        assert!(ui.is_running() && !ui.holds_preview());
        assert!(wait_for_result(&mut ui).is_none(), "a preview is held, not handed over");
        assert!(!ui.is_running() && ui.holds_preview());
        let p = ui.preview_for(View::Breadboard).unwrap();
        assert!(ui.preview_for(View::Schematic).is_none());
        assert_eq!(p.to(), View::Breadboard);
        assert_eq!(circuit_to_string(&c), before, "the real circuit is untouched");
        assert_eq!(p.summary(), "Autoroute: placed 0 part(s), added 1 wire(s), routed 3 net(s), 0 failed");
        assert!(p.changed);
        // New wires are highlighted; the wires that were already there are not.
        let hl = p.highlight();
        assert_eq!(hl.wires, p.outcome.wires_added);
        assert_eq!(hl.wires.len(), 1);
        assert!(["w1", "w2", "w3"].iter().all(|w| !hl.wires.iter().any(|x| x == w)));
    }

    #[test]
    fn changing_an_option_discards_the_preview_and_marks_it_stale() {
        let (c, lib) = led();
        let mut ui = AutorouteUi::default();
        ui.open(&c, View::Schematic);
        ui.start(None, &c, &lib, None, false);
        wait_for_result(&mut ui);
        assert!(ui.holds_preview() && !ui.is_stale());
        let mut o = ui.options().clone();
        ui.set_options(o.clone());
        assert!(ui.holds_preview(), "setting the same options keeps the preview");
        o.keep_wires = false;
        ui.set_options(o);
        assert!(!ui.holds_preview() && ui.is_stale());
        // Running again clears the stale mark.
        ui.start(None, &c, &lib, None, false);
        assert!(!ui.is_stale());
        wait_for_result(&mut ui);
        // Changing options while a job runs ignores that job.
        ui.start(None, &c, &lib, None, false);
        let mut o = ui.options().clone();
        o.clearance = 4;
        ui.set_options(o);
        assert!(!ui.is_running() && ui.is_stale());
        assert!(wait_for_result(&mut ui).is_none());
        assert!(!ui.holds_preview());
    }

    #[test]
    fn a_cancelled_job_cannot_deliver_a_late_result() {
        let (c, lib) = led();
        let mut ui = AutorouteUi::default();
        ui.open(&c, View::Schematic);
        ui.start(None, &c, &lib, None, false);
        let gen = ui.gen;
        ui.cancel();
        assert!(!ui.is_running());
        // The worker finishes later and its message arrives anyway.
        let late = Ok(Box::new(compute_preview(&c, &lib, None, ui.options())));
        assert!(ui.deliver(gen, late).is_none());
        assert!(!ui.holds_preview());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(ui.poll().is_none());
        assert!(!ui.holds_preview() && ui.error.is_none());
        // A result of an older job does not satisfy a newer one either.
        ui.start(None, &c, &lib, None, false);
        let stale = Ok(Box::new(compute_preview(&c, &lib, None, ui.options())));
        assert!(ui.deliver(gen, stale).is_none() && ui.is_running());
        wait_for_result(&mut ui);
        assert!(ui.holds_preview());
    }

    #[test]
    fn apply_without_a_preview_hands_the_result_over() {
        let (c, lib) = led_without_jumper();
        let mut ui = AutorouteUi::default();
        ui.open(&c, View::Schematic);
        ui.start(None, &c, &lib, None, true);
        let d = wait_for_result(&mut ui).expect("handed over for applying");
        assert!(!ui.holds_preview());
        assert!(d.changed);
    }

    #[test]
    fn closing_drops_the_preview_and_reopening_keeps_the_options() {
        let (c, lib) = led();
        let mut ui = AutorouteUi::default();
        ui.open(&c, View::Schematic);
        let mut o = ui.options().clone();
        o.place_missing = false;
        o.scope = Scope::Only(vec!["R1.a".into()]);
        o.source = Source::Declared;
        ui.set_options(o);
        ui.start(None, &c, &lib, None, false);
        wait_for_result(&mut ui);
        ui.close();
        assert!(!ui.open && !ui.holds_preview());
        ui.open(&c, View::Breadboard);
        assert!(!ui.options().place_missing, "kept");
        assert_eq!(ui.options().to, View::Schematic, "follows the active view again");
        assert_eq!(ui.options().scope, Scope::All, "a part scope does not outlive the selection");
        assert_eq!(ui.options().source, Source::OtherView, "no declared nets in led.circ");
    }

    #[test]
    fn commit_is_one_undo_step_and_undo_restores_the_exact_circuit() {
        let (mut c, lib) = led_without_jumper();
        let before = circuit_to_string(&c);
        let mut h = History::new();
        let d = compute_preview(&c, &lib, None, &RouteOptions::default());
        assert!(d.changed);
        assert!(commit(&mut c, &mut h, &lib, &d.outcome.circuit, d.to()));
        assert_eq!(circuit_to_string(&c), circuit_to_string(&d.outcome.circuit));
        assert!(h.dirty() && h.can_undo());
        assert!(h.undo(&mut c));
        assert_eq!(circuit_to_string(&c), before);
        assert!(!h.can_undo(), "exactly one step");
        assert!(h.redo(&mut c));
        assert_eq!(circuit_to_string(&c), circuit_to_string(&d.outcome.circuit));
    }

    #[test]
    fn committing_an_unchanged_outcome_records_nothing() {
        let (mut c, lib) = led();
        let same = c.clone();
        let mut h = History::new();
        assert!(!commit(&mut c, &mut h, &lib, &same, View::Breadboard));
        assert!(!h.can_undo() && !h.dirty());
    }

    #[test]
    fn declared_nets_port_places_parts_and_commits_as_one_step() {
        let (mut c, lib) = nets_only();
        let before = circuit_to_string(&c);
        let opts = RouteOptions { to: View::Schematic, source: Source::Declared, ..RouteOptions::default() };
        let d = compute_preview(&c, &lib, None, &opts);
        assert_eq!(d.outcome.placed.len(), c.parts.len());
        assert!(!d.outcome.wires_added.is_empty());
        assert!(d.changed);
        let mut h = History::new();
        assert!(commit(&mut c, &mut h, &lib, &d.outcome.circuit, View::Schematic));
        assert!(c.parts.iter().all(|p| p.schematic.is_some()));
        assert!(h.undo(&mut c));
        assert_eq!(circuit_to_string(&c), before);
        assert!(!h.can_undo());
    }

    #[test]
    fn failures_are_reported_for_types_the_library_cannot_resolve() {
        let (mut c, lib) = nets_only();
        c.parts[1].ty = "no_such_part".into(); // U1
        let opts = RouteOptions { to: View::Schematic, source: Source::Declared, ..RouteOptions::default() };
        let d = compute_preview(&c, &lib, None, &opts);
        assert!(!d.outcome.failures.is_empty());
        assert!(d.outcome.failures.iter().all(|f| !f.net.is_empty() && f.reason.contains("unknown part or pin")), "{:?}", d.outcome.failures);
        // The nets that do not involve U1 are still routed, and the result is a normal preview.
        assert!(d.outcome.routed >= 1 && d.changed);
    }

    #[test]
    fn the_preview_library_resolves_a_board_the_router_added() {
        let (c, lib) = nets_only();
        let opts = RouteOptions { source: Source::Declared, ..RouteOptions::default() };
        let d = compute_preview(&c, &lib, None, &opts);
        let board = d.outcome.circuit.parts.iter().find(|p| d.lib.get(&p.ty).is_some_and(|t| t.is_board()));
        assert!(board.is_some(), "a breadboard part was added and resolves");
        assert_eq!(d.jumpers.len(), d.outcome.circuit.wires.iter().filter(|w| w.view == View::Breadboard).count());
    }
}
