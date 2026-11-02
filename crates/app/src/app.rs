//! The egui application: menus, panels, file loading and the interactive canvas.

use crate::canvas::{self, DrawCtx, Highlight, Theme, View};
use crate::core_api::{self, Nets};
use crate::labels::LabelOpts;
use crate::nets_panel::{self, NetKey};
use crate::partbrowser::PartBrowser;
use crate::recent::Recent;
use crate::{reload, window};
use eframe::egui::{self, Color32, PointerButton, RichText, Sense, Vec2};
use short_circ_core::{fzp, Circuit, Library, Pt, Report, Severity};
use std::path::{Path, PathBuf};

/// Read-only colour display: a swatch and the hex code (plus the name as written if it differs).
fn color_value(ui: &mut egui::Ui, spec: &str) {
    ui.horizontal(|ui| match short_circ_core::color::parse_color(spec) {
        Some(c) => {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, canvas::to_color32(c));
            let edge = ui.visuals().widgets.noninteractive.fg_stroke.color;
            ui.painter().rect_stroke(r, 2.0, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
            let hex = c.to_hex();
            if spec.trim().eq_ignore_ascii_case(&hex) {
                ui.label(hex);
            } else {
                ui.label(format!("{hex} ({})", spec.trim()));
            }
        }
        None => {
            ui.label(spec);
        }
    });
}

struct Loaded {
    circuit: Circuit,
    lib: Library,
    path: PathBuf,
    nets: Nets,
    report: Report,
    junctions: Vec<Pt>,
    /// File system state when this version was read (for live reload).
    stamp: reload::Stamp,
}

enum Action {
    OpenDialog,
    Open(PathBuf),
    ExportSvg,
    FitView,
    ReloadNow,
    ImportFritzing,
}

pub struct ShortCircApp {
    loaded: Option<Loaded>,
    error: Option<String>,
    /// Informational message (e.g. result of a Fritzing import), shown in a dismissable banner.
    import_status: Option<String>,
    recent: Recent,
    view: View,
    canvas_size: Vec2,
    pending_fit: bool,
    show_grid: bool,
    dark: bool,
    applied_dark: Option<bool>,
    show_issues: bool,
    show_about: bool,
    selected: Option<usize>,
    /// Issue location to pan to on the next frame.
    focus: Option<Pt>,
    /// Marker location and start time (egui clock).
    flash: Option<(Pt, f64)>,
    actions: Vec<Action>,
    auto_reload: bool,
    /// Egui clock time of the last file system poll.
    last_poll: f64,
    reloads: u32,
    /// Short message for the hint bar and the time it was set.
    status: Option<(String, f64)>,
    labels: LabelOpts,
    show_nets: bool,
    /// Net kept highlighted after a click.
    locked: Option<NetKey>,
    /// World box to fit on the next frame (net focus).
    fit_box: Option<([f64; 2], [f64; 2])>,
    browser: PartBrowser,
    default_lib: Library,
    show_help: bool,
    window_placed: bool,
}

const STATUS_SECS: f32 = 3.0;
const HINT: &str = "drag: pan   wheel: zoom   click pin/wire: lock net   Esc: clear   F: fit   G: grid   R: reload   Ctrl+O: open";
const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+O", "open a circuit file"),
    ("F", "fit the circuit to the window"),
    ("R", "reload the file now"),
    ("G", "show or hide the grid"),
    ("Esc", "clear the selection and the locked net"),
    ("click pin or wire", "lock that net's highlight"),
    ("click empty space", "clear selection and locked net"),
    ("drag / wheel", "pan / zoom"),
];

/// Read and analyse a circuit file. The file system stamp is taken first so a write that
/// happens while reading is noticed by the next poll.
fn build_loaded(path: &Path) -> Result<Loaded, String> {
    let stamp = reload::stamp(path);
    let circuit = core_api::load_circuit(path)?;
    let lib = Library::load(&circuit, path.parent());
    let nets = core_api::build_nets(&circuit, &lib);
    let report = core_api::validate(&circuit, &lib);
    let junctions = canvas::junctions(&circuit.wires);
    Ok(Loaded { circuit, lib, path: path.to_path_buf(), nets, report, junctions, stamp })
}

impl ShortCircApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut app = ShortCircApp {
            loaded: None,
            error: None,
            import_status: None,
            recent: Recent::load(),
            view: View::default(),
            canvas_size: Vec2::new(800.0, 600.0),
            pending_fit: false,
            show_grid: true,
            dark: true,
            applied_dark: None,
            show_issues: true,
            show_about: false,
            selected: None,
            focus: None,
            flash: None,
            actions: Vec::new(),
            auto_reload: true,
            last_poll: 0.0,
            reloads: 0,
            status: None,
            labels: LabelOpts::default(),
            show_nets: false,
            locked: None,
            fit_box: None,
            browser: PartBrowser::new(),
            default_lib: Library::load(&Circuit::default(), None),
            show_help: false,
            window_placed: false,
        };
        if let Some(p) = initial {
            app.load_file(&cc.egui_ctx, &p);
        }
        app
    }

    /// Load a circuit. On failure the previous circuit stays and a banner is shown.
    fn load_file(&mut self, ctx: &egui::Context, path: &Path) {
        match build_loaded(path) {
            Ok(loaded) => {
                self.loaded = Some(loaded);
                self.error = None;
                self.selected = None;
                self.locked = None;
                self.flash = None;
                self.pending_fit = true;
                self.recent.add(path);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("short-circ - {name}")));
            }
            Err(msg) => self.error = Some(msg),
        }
    }

    /// Re-read the open file and its parts folder, keeping view, selection and locked net.
    /// On failure the previous circuit stays and the error banner is shown.
    fn reload_current(&mut self, ctx: &egui::Context) {
        let Some(old) = &self.loaded else { return };
        let path = old.path.clone();
        let sel_id = self.selected.and_then(|i| old.circuit.parts.get(i)).map(|p| p.id.clone());
        match build_loaded(&path) {
            Ok(new) => {
                self.selected = sel_id.and_then(|id| new.circuit.parts.iter().position(|p| p.id == id));
                if self.locked.as_ref().is_some_and(|k| k.resolve(&new.nets).is_none()) {
                    self.locked = None;
                }
                self.flash = None;
                self.loaded = Some(new);
                self.error = None;
                self.reloads += 1;
                let msg = format!("Reloaded ({})", self.reloads);
                self.set_status(ctx, msg);
            }
            Err(msg) => {
                if let Some(l) = &mut self.loaded {
                    l.stamp = reload::stamp(&path);
                }
                self.error = Some(msg);
                self.set_status(ctx, "Reload failed, showing the previous version".into());
            }
        }
    }

    fn set_status(&mut self, ctx: &egui::Context, msg: String) {
        self.status = Some((msg, ctx.input(|i| i.time)));
        ctx.request_repaint_after(std::time::Duration::from_secs_f32(STATUS_SECS + 0.1));
    }

    /// Poll the file system about every 400 ms and reload when the file or parts folder changed.
    fn poll_reload(&mut self, ctx: &egui::Context) {
        let Some(l) = &self.loaded else { return };
        if !self.auto_reload {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(reload::POLL_INTERVAL));
        let now = ctx.input(|i| i.time);
        if !reload::poll_due(now, self.last_poll) {
            return;
        }
        self.last_poll = now;
        if reload::should_reload(&l.stamp, &reload::stamp(&l.path)) {
            self.reload_current(ctx);
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_dark != Some(self.dark) {
            ctx.set_visuals(if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() });
            self.applied_dark = Some(self.dark);
        }
    }

    fn run_actions(&mut self, ctx: &egui::Context) {
        for a in std::mem::take(&mut self.actions) {
            match a {
                Action::OpenDialog => {
                    let pick = rfd::FileDialog::new()
                        .add_filter("short-circ circuit", &["circ"])
                        .add_filter("All files", &["*"])
                        .pick_file();
                    if let Some(p) = pick {
                        self.load_file(ctx, &p);
                    }
                }
                Action::Open(p) => self.load_file(ctx, &p),
                Action::FitView => self.pending_fit = true,
                Action::ReloadNow => self.reload_current(ctx),
                Action::ExportSvg => self.export_svg(),
                Action::ImportFritzing => self.import_fritzing(),
            }
        }
    }

    /// File > Import Fritzing part: convert a .fzpz/.fzp into `parts/` next to the current
    /// circuit (or next to the exe), then rebuild the library so the new type resolves.
    fn import_fritzing(&mut self) {
        let Some(src) = rfd::FileDialog::new().add_filter("Fritzing part", &["fzpz", "fzp"]).pick_file() else {
            return;
        };
        let dir = match self.loaded.as_ref().and_then(|l| l.path.parent()) {
            Some(d) => d.join("parts"),
            None => std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("parts")))
                .unwrap_or_else(|| PathBuf::from("parts")),
        };
        match fzp::import_to_dir(&src, &dir, &fzp::ImportOptions::default()) {
            Ok((part, warnings, out)) => {
                if let Some(l) = &mut self.loaded {
                    l.lib = Library::load(&l.circuit, l.path.parent());
                    l.nets = core_api::build_nets(&l.circuit, &l.lib);
                    l.report = core_api::validate(&l.circuit, &l.lib);
                }
                let mut msg = format!("Imported {} ({} pins) to {}", part.ty, part.pins.len(), out.display());
                for w in &warnings {
                    msg.push_str(&format!("\nwarning: {w}"));
                }
                self.import_status = Some(msg);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("cannot import {}: {e}", src.display())),
        }
    }

    fn status_banner(&mut self, ui: &mut egui::Ui) {
        let mut dismiss = false;
        if let Some(msg) = &self.import_status {
            egui::Panel::top("status_banner").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(msg.as_str()).wrap());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Dismiss").clicked() {
                            dismiss = true;
                        }
                    });
                });
            });
        }
        if dismiss {
            self.import_status = None;
        }
    }

    fn export_svg(&mut self) {
        let Some(l) = &self.loaded else { return };
        let stem = l.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "circuit".into());
        let Some(out) = rfd::FileDialog::new().add_filter("SVG image", &["svg"]).set_file_name(format!("{stem}.svg")).save_file()
        else {
            return;
        };
        let result = core_api::render_svg(&l.circuit, &l.lib, self.dark)
            .and_then(|svg| std::fs::write(&out, svg).map_err(|e| format!("cannot write {}: {e}", out.display())));
        if let Err(e) = result {
            self.error = Some(e);
        }
    }

    fn handle_global_input(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.actions.push(Action::Open(p));
        }
        let open = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O));
        if open {
            self.actions.push(Action::OpenDialog);
        }
        // Single-key shortcuts are off while a text box has the keyboard.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let key = |k: egui::Key| ctx.input(|i| i.key_pressed(k) && !i.modifiers.any());
        if key(egui::Key::F) {
            self.actions.push(Action::FitView);
        }
        if key(egui::Key::R) {
            self.actions.push(Action::ReloadNow);
        }
        if key(egui::Key::G) {
            self.show_grid = !self.show_grid;
        }
        if key(egui::Key::Escape) {
            self.locked = None;
            self.selected = None;
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open...").clicked() {
                    self.actions.push(Action::OpenDialog);
                }
                ui.menu_button("Open Recent", |ui| {
                    if self.recent.items.is_empty() {
                        ui.add_enabled(false, egui::Button::new("(none)"));
                    }
                    for p in self.recent.items.clone() {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.button(name).on_hover_text(p.display().to_string()).clicked() {
                            self.actions.push(Action::Open(p));
                        }
                    }
                });
                let can_export = self.loaded.is_some() && core_api::svg_available();
                if ui.add_enabled(can_export, egui::Button::new("Export SVG...")).clicked() {
                    self.actions.push(Action::ExportSvg);
                }
                if ui.button("Import Fritzing part...").clicked() {
                    self.actions.push(Action::ImportFritzing);
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Fit to window  (F)").clicked() {
                    self.actions.push(Action::FitView);
                }
                if ui.add_enabled(self.loaded.is_some(), egui::Button::new("Reload now  (R)")).clicked() {
                    self.actions.push(Action::ReloadNow);
                }
                ui.checkbox(&mut self.auto_reload, "Auto-reload");
                ui.checkbox(&mut self.show_grid, "Grid  (G)");
                ui.checkbox(&mut self.dark, "Dark theme");
                ui.separator();
                ui.checkbox(&mut self.show_issues, "Issues panel");
                ui.checkbox(&mut self.show_nets, "Nets panel");
                ui.checkbox(&mut self.browser.open, "Part browser");
                ui.separator();
                ui.label("Labels");
                ui.checkbox(&mut self.labels.refs, "Part references");
                ui.checkbox(&mut self.labels.values, "Part values");
                ui.checkbox(&mut self.labels.wire_ids, "Wire ids");
                ui.checkbox(&mut self.labels.pin_names, "Pin names");
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Shortcuts").clicked() {
                    self.show_help = true;
                }
                if ui.button("About").clicked() {
                    self.show_about = true;
                }
            });
        });
    }

    fn banner(&mut self, ui: &mut egui::Ui) {
        let mut dismiss = false;
        if let Some(msg) = &self.error {
            egui::Panel::top("error_banner").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Error").strong().color(Color32::from_rgb(230, 80, 80)));
                    ui.add(egui::Label::new(msg.as_str()).wrap());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Dismiss").clicked() {
                            dismiss = true;
                        }
                    });
                });
            });
        }
        if dismiss {
            self.error = None;
        }
    }

    fn issues_panel(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let mut jump: Option<(Option<Pt>, Vec<String>)> = None;
        egui::Panel::bottom("issues").resizable(true).default_size(140.0).show(ui, |ui| {
            let Some(l) = &self.loaded else {
                ui.label("No circuit loaded.");
                return;
            };
            let r = &l.report;
            ui.horizontal(|ui| {
                ui.strong("Issues");
                if r.issues.is_empty() {
                    ui.label("none found");
                } else {
                    ui.label(format!("{} error(s), {} warning(s)", r.errors, r.warnings));
                }
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for issue in &r.issues {
                    let (word, color) = match issue.severity {
                        Severity::Error => ("error", theme.error),
                        Severity::Warning => ("warning", theme.warning),
                    };
                    let resp = ui
                        .horizontal(|ui| {
                            ui.label(RichText::new(word).color(color).strong());
                            ui.label(RichText::new(&issue.rule).monospace());
                            ui.add(egui::Label::new(issue.message.as_str()).sense(Sense::click()))
                        })
                        .inner;
                    if resp.clicked() {
                        jump = Some((issue.at, issue.ids.clone()));
                    }
                    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            });
        });
        if let Some((at, ids)) = jump {
            let pt = at.or_else(|| self.loaded.as_ref().and_then(|l| canvas::locate_ids(&l.circuit, &ids)));
            self.focus = pt;
        }
    }

    fn info_panel(&mut self, ui: &mut egui::Ui) {
        let (Some(l), Some(i)) = (&self.loaded, self.selected) else { return };
        let Some(part) = l.circuit.parts.get(i) else { return };
        let def = l.lib.get(&part.ty);
        egui::Panel::right("info").default_size(230.0).show(ui, |ui| {
            ui.heading(&part.id);
            egui::Grid::new("part_info").num_columns(2).show(ui, |ui| {
                ui.label("type");
                ui.label(match def {
                    Some(d) if !d.name.is_empty() => format!("{} ({})", part.ty, d.name),
                    Some(_) => part.ty.clone(),
                    None => format!("{} (unknown)", part.ty),
                });
                ui.end_row();
                ui.label("position");
                ui.label(format!("({}, {})", part.at[0], part.at[1]));
                ui.end_row();
                ui.label("rotation");
                ui.label(format!("{} deg{}", part.rot, if part.mirror { ", mirrored" } else { "" }));
                ui.end_row();
                for (k, v) in &part.props {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                }
                for (k, v) in [("color", &part.color), ("fill", &part.fill), ("label_color", &part.label_color)] {
                    if let Some(spec) = v {
                        ui.label(k);
                        color_value(ui, spec);
                        ui.end_row();
                    }
                }
            });
            if let Some(d) = def {
                if !d.pins.is_empty() {
                    ui.separator();
                    ui.strong("Pins");
                    egui::Grid::new("pin_info").num_columns(3).show(ui, |ui| {
                        for pin in &d.pins {
                            let w = short_circ_core::transform::pin_world(part, pin);
                            ui.label(&pin.id);
                            ui.label(format!("({}, {})", w[0], w[1]));
                            ui.label(l.nets.of_pin(&part.id, &pin.id).map(|n| n.name.as_str()).unwrap_or(""));
                            ui.end_row();
                        }
                    });
                }
            }
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let theme = Theme::for_circuit(self.dark, self.loaded.as_ref().map(|l| &l.circuit));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.bg);
        self.canvas_size = rect.size();
        let now = ui.input(|i| i.time);

        // View changes: fit, focus, pan, zoom.
        if let Some((lo, hi)) = self.fit_box.take() {
            self.view.fit(lo, hi, rect.size(), 30.0);
        }
        if self.pending_fit {
            self.pending_fit = false;
            match self.loaded.as_ref().and_then(|l| canvas::circuit_bounds(&l.circuit, &l.lib)) {
                Some((lo, hi)) => self.view.fit(lo, hi, rect.size(), 40.0),
                None => self.view = View::default(),
            }
        }
        if let Some(pt) = self.focus.take() {
            self.view.center_on([pt[0] as f64, pt[1] as f64], rect.size());
            self.flash = Some((pt, now));
        }
        if resp.dragged_by(PointerButton::Primary) || resp.dragged_by(PointerButton::Middle) {
            self.view.pan(resp.drag_delta());
        }
        if resp.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom * (scroll * 0.0025).exp();
            if let Some(pos) = resp.hover_pos() {
                if (factor - 1.0).abs() > 1e-6 {
                    self.view.zoom_at(pos - rect.min, factor);
                }
            }
        }
        let view = self.view;

        // Hover, click and the locked net.
        let mut hl = Highlight::default();
        let mut tip: Option<(egui::Pos2, String)> = None;
        if let Some(l) = &self.loaded {
            if let Some(net) = self.locked.as_ref().and_then(|k| k.resolve(&l.nets)) {
                nets_panel::add_highlight(&mut hl, net);
            }
        }
        if let (Some(l), Some(pos)) = (&self.loaded, resp.hover_pos()) {
            let w = view.to_world(pos - rect.min);
            let px = 1.0 / view.scale as f64;
            let pin_hit = canvas::pin_at(&l.circuit, &l.lib, w, (9.0 * px).max(0.3));
            let wire_hit = if pin_hit.is_none() { canvas::wire_at(&l.circuit, w, 6.0 * px) } else { None };
            if let Some(h) = &pin_hit {
                let part = &l.circuit.parts[h.part];
                let mut text = format!("{}.{}", part.id, h.pin);
                if let Some(net) = l.nets.of_pin(&part.id, &h.pin) {
                    if !net.name.is_empty() {
                        text.push_str(&format!("  [{}]", net.name));
                    }
                    nets_panel::add_highlight(&mut hl, net);
                } else {
                    hl.pins.push((part.id.clone(), h.pin.clone()));
                }
                tip = Some((pos, text));
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            } else if let Some(wi) = wire_hit {
                let id = &l.circuit.wires[wi].id;
                match l.nets.of_wire(id) {
                    Some(net) => {
                        nets_panel::add_highlight(&mut hl, net);
                        tip = Some((pos, format!("{id}  [{}]", net.name)));
                    }
                    None => {
                        hl.wires.push(id.clone());
                        tip = Some((pos, id.clone()));
                    }
                }
            }
            if resp.clicked() {
                // A click on a pin or wire locks its net; on a part selects it; elsewhere clears.
                if let Some(h) = pin_hit {
                    self.locked = Some(NetKey::Pin(l.circuit.parts[h.part].id.clone(), h.pin.clone()));
                    self.selected = Some(h.part);
                } else if let Some(wi) = wire_hit {
                    self.locked = Some(NetKey::Wire(l.circuit.wires[wi].id.clone()));
                } else if let Some(pi) = canvas::part_at(&l.circuit, &l.lib, w, 0.3) {
                    self.selected = Some(pi);
                } else {
                    self.locked = None;
                    self.selected = None;
                }
            }
        }

        // Paint.
        let d = DrawCtx { painter: &painter, origin: rect.min, view, theme, labels: self.labels };
        if self.show_grid {
            canvas::draw_grid(&d, rect.size());
        }
        if let Some(l) = &self.loaded {
            canvas::draw_circuit(&d, &l.circuit, &l.lib, &l.junctions, &hl, self.selected);
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Open a .circ file (File > Open, Ctrl+O) or drop one here",
                egui::FontId::proportional(16.0),
                theme.grid_major,
            );
        }
        if let Some((at, start)) = self.flash {
            let age = (now - start) as f32;
            if age < canvas::FLASH_SECS {
                canvas::draw_flash(&d, at, age);
                ui.ctx().request_repaint();
            } else {
                self.flash = None;
            }
        }
        if let Some((pos, text)) = tip {
            canvas::draw_tooltip(&painter, rect, pos, &text, &theme);
        }
        if ui.input(|i| !i.raw.hovered_files.is_empty()) {
            painter.rect_filled(rect, 0.0, theme.select.gamma_multiply(0.15));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to open",
                egui::FontId::proportional(22.0),
                theme.select,
            );
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About short-circ").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.heading(crate::cli::version_line());
            ui.label("Viewer and checker for short-circ circuit files.");
            ui.label("Drag to pan, wheel to zoom, F to fit.");
            ui.label("Hover a pin to see its name and net; click a pin or wire to lock its net.");
        });
        self.show_about = open;
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new("Shortcuts").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            egui::Grid::new("shortcuts").num_columns(2).show(ui, |ui| {
                for (k, what) in SHORTCUTS {
                    ui.monospace(*k);
                    ui.label(*what);
                    ui.end_row();
                }
            });
        });
        self.show_help = open;
    }

    fn hint_bar(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        if self.status.as_ref().is_some_and(|(_, t)| now - t > STATUS_SECS as f64) {
            self.status = None;
        }
        egui::Panel::bottom("hint").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some((msg, _)) = &self.status {
                    ui.label(RichText::new(msg.as_str()).strong().color(Color32::from_rgb(90, 190, 120)));
                    ui.separator();
                }
                ui.label(RichText::new(HINT).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(if self.auto_reload { "auto-reload on" } else { "auto-reload off" }).small());
                });
            });
        });
    }

    fn nets_side_panel(&mut self, ui: &mut egui::Ui) {
        let mut pick = None;
        egui::Panel::left("nets").resizable(true).default_size(230.0).show(ui, |ui| match &self.loaded {
            Some(l) => {
                let locked = self.locked.as_ref().and_then(|k| k.resolve(&l.nets));
                pick = nets_panel::show(ui, &l.nets, locked);
            }
            None => {
                ui.heading("Nets");
                ui.label("No circuit loaded.");
            }
        });
        if let Some(key) = pick {
            if let Some(l) = &self.loaded {
                if let Some(net) = key.resolve(&l.nets) {
                    self.fit_box = nets_panel::net_fit_bounds(net, &l.circuit);
                }
            }
            self.locked = Some(key);
        }
    }
}

impl eframe::App for ShortCircApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        if !self.window_placed {
            self.window_placed = window::place_once(&ctx);
        }
        self.handle_global_input(&ctx);
        self.poll_reload(&ctx);
        self.run_actions(&ctx);
        let theme = Theme::new(self.dark);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        self.banner(ui);
        self.status_banner(ui);
        self.hint_bar(ui);
        if self.show_issues {
            self.issues_panel(ui, &theme);
        }
        self.info_panel(ui);
        if self.show_nets {
            self.nets_side_panel(ui);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.canvas(ui));
        let lib = self.loaded.as_ref().map(|l| &l.lib).unwrap_or(&self.default_lib);
        self.browser.show(&ctx, lib, self.dark);
        self.about_window(&ctx);
        self.help_window(&ctx);
        self.run_actions(&ctx);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"{"format":"short-circ/1","parts":[{"id":"R1","type":"resistor","at":[0,0],"props":{"value":"100"}}]}"#;

    #[test]
    fn rebuild_after_edit_sees_new_content_and_stamp() {
        let dir = std::env::temp_dir().join(format!("short-circ-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.circ");
        std::fs::write(&path, SRC).unwrap();
        let first = build_loaded(&path).unwrap();
        assert_eq!(first.circuit.parts[0].props["value"], "100");
        assert!(!reload::should_reload(&first.stamp, &reload::stamp(&path)));
        std::fs::write(&path, SRC.replace("\"100\"", "\"4700\"")).unwrap();
        assert!(reload::should_reload(&first.stamp, &reload::stamp(&path)), "edit is detected");
        let second = build_loaded(&path).unwrap();
        assert_eq!(second.circuit.parts[0].props["value"], "4700");
        // A broken file is an error (the app keeps the old circuit) and is detected as a change.
        std::fs::write(&path, "{ not json").unwrap();
        assert!(reload::should_reload(&second.stamp, &reload::stamp(&path)));
        assert!(build_loaded(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}