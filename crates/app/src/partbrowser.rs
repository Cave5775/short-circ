//! "Part browser" window: lists every resolvable part type with a preview that draws the part
//! with its pin ids and names, so imported or hand-written components can be checked by eye.

use crate::canvas::{self, DrawCtx, Highlight, Theme, View};
use crate::labels::LabelOpts;
use crate::partview;
use eframe::egui::{self, RichText, Sense, Vec2};
use short_circ_core::{Library, PartDef};

#[derive(Default)]
pub struct PartBrowser {
    pub open: bool,
    filter: String,
    /// Text box for generated families such as `dip8` or `header4`.
    family: String,
    family_error: Option<String>,
    selected: Option<String>,
    show_grid: bool,
}

/// Case-insensitive substring filter over type names.
pub fn filter_types<'a>(types: impl Iterator<Item = &'a str>, filter: &str) -> Vec<String> {
    let f = filter.trim().to_ascii_lowercase();
    types.filter(|t| f.is_empty() || t.to_ascii_lowercase().contains(&f)).map(String::from).collect()
}

/// Validate the family box: a type that is not in the library must be a generated one.
pub fn check_family(lib: &Library, text: &str) -> Result<String, String> {
    let ty = text.trim();
    if ty.is_empty() {
        return Err("type a name such as dip8 or header4".into());
    }
    match partview::resolve_in(lib, ty) {
        Some(_) => Ok(ty.to_string()),
        None => Err(format!("{ty:?} is not a known part or generated family (dip4..dip40 even, header1..header40)")),
    }
}

impl PartBrowser {
    pub fn new() -> Self {
        PartBrowser { show_grid: true, ..Default::default() }
    }

    /// Show the window. `lib` is the open circuit's library (or the default one).
    pub fn show(&mut self, ctx: &egui::Context, lib: &Library, dark: bool) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Part browser").open(&mut open).default_size([780.0, 600.0]).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(190.0);
                    self.list_column(ui, lib);
                });
                ui.separator();
                ui.vertical(|ui| {
                    let def = self.selected.as_deref().and_then(|t| partview::resolve_in(lib, t));
                    match def {
                        Some(def) => self.detail(ui, &def, dark),
                        None => {
                            ui.label("Select a part type on the left.");
                        }
                    }
                });
            });
        });
        self.open = open;
    }

    fn list_column(&mut self, ui: &mut egui::Ui, lib: &Library) {
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter..."));
        let types = filter_types(lib.types(), &self.filter);
        ui.label(format!("{} part type(s)", types.len()));
        egui::ScrollArea::vertical().id_salt("part_types").max_height(300.0).auto_shrink([false, false]).show(ui, |ui| {
            for t in &types {
                let on = self.selected.as_deref() == Some(t.as_str());
                if ui.selectable_label(on, t).clicked() {
                    self.selected = Some(t.clone());
                }
            }
        });
        ui.separator();
        ui.label("Generated family");
        let resp = ui.add(egui::TextEdit::singleline(&mut self.family).hint_text("dip8, header4").desired_width(120.0));
        let go = ui.button("Show").clicked() || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if go {
            match check_family(lib, &self.family) {
                Ok(t) => {
                    self.selected = Some(t);
                    self.family_error = None;
                }
                Err(e) => self.family_error = Some(e),
            }
        }
        if let Some(e) = &self.family_error {
            ui.add(egui::Label::new(RichText::new(e).small().color(egui::Color32::from_rgb(230, 90, 90))).wrap());
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, def: &PartDef, dark: bool) {
        ui.heading(&def.ty);
        egui::Grid::new("part_detail").num_columns(2).show(ui, |ui| {
            ui.label("name");
            ui.label(if def.name.is_empty() { "-" } else { def.name.as_str() });
            ui.end_row();
            ui.label("category");
            ui.label(def.category.as_deref().filter(|c| !c.is_empty()).unwrap_or("-"));
            ui.end_row();
            ui.label("size");
            ui.label(format!("{} x {} grid units", def.size[0], def.size[1]));
            ui.end_row();
            ui.label("pins");
            ui.label(def.pins.len().to_string());
            ui.end_row();
        });
        ui.checkbox(&mut self.show_grid, "Grid");
        self.preview(ui, def, dark);
        if !def.pins.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical().id_salt("part_pins").max_height(140.0).show(ui, |ui| {
                egui::Grid::new("part_pin_table").striped(true).show(ui, |ui| {
                    for h in ["pin", "name", "at", "dir"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for p in &def.pins {
                        ui.monospace(&p.id);
                        ui.label(p.name.as_deref().unwrap_or(""));
                        ui.monospace(format!("({}, {})", p.at[0], p.at[1]));
                        ui.label(format!("{:?}", p.dir).to_lowercase());
                        ui.end_row();
                    }
                });
            });
        }
    }

    fn preview(&self, ui: &mut egui::Ui, def: &PartDef, dark: bool) {
        let size = Vec2::new(ui.available_width().clamp(260.0, 520.0), 260.0);
        let (resp, painter) = ui.allocate_painter(size, Sense::hover());
        let rect = resp.rect;
        let theme = Theme::new(dark);
        painter.rect_filled(rect, 2.0, theme.bg);
        let circuit = partview::single_part_circuit(def);
        let lib = Library::load(&circuit, None);
        let Some((lo, hi)) = canvas::circuit_bounds(&circuit, &lib) else { return };
        let mut view = View::default();
        view.fit([lo[0] - 1.0, lo[1] - 2.0], [hi[0] + 3.0, hi[1] + 1.0], rect.size(), 10.0);
        let d = DrawCtx { painter: &painter, origin: rect.min, view, theme, labels: LabelOpts::default() };
        if self.show_grid {
            canvas::draw_grid(&d, rect.size());
        }
        canvas::draw_circuit(&d, &circuit, &lib, &[], &Highlight::default(), None);
        if let Some(pos) = resp.hover_pos() {
            let w = view.to_world(pos - rect.min);
            if let Some(h) = canvas::pin_at(&circuit, &lib, w, 0.6) {
                let text = def
                    .pins
                    .iter()
                    .find(|p| p.id == h.pin)
                    .map(|p| format!("pin {}{}  at ({}, {})", p.id, p.name.as_deref().map(|n| format!(" {n}")).unwrap_or_default(), p.at[0], p.at[1]))
                    .unwrap_or(h.pin);
                canvas::draw_tooltip(&painter, rect, pos, &text, &theme);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_is_case_insensitive_substring() {
        let all = ["resistor", "led", "dip8", "Header4"];
        assert_eq!(filter_types(all.iter().copied(), ""), all);
        assert_eq!(filter_types(all.iter().copied(), " RES "), vec!["resistor"]);
        assert_eq!(filter_types(all.iter().copied(), "d"), vec!["led", "dip8", "Header4"]);
        assert!(filter_types(all.iter().copied(), "zzz").is_empty());
    }

    #[test]
    fn family_box_accepts_generated_and_library_types() {
        let lib = Library::builtin();
        assert_eq!(check_family(&lib, " dip8 ").unwrap(), "dip8");
        assert_eq!(check_family(&lib, "header12").unwrap(), "header12");
        assert_eq!(check_family(&lib, "resistor").unwrap(), "resistor");
        assert!(check_family(&lib, "dip3").is_err());
        assert!(check_family(&lib, "").is_err());
        assert!(check_family(&lib, "dip99").is_err());
    }

    #[test]
    fn builtin_library_lists_resistor() {
        let lib = Library::builtin();
        assert!(filter_types(lib.types(), "resis").contains(&"resistor".to_string()));
    }
}
