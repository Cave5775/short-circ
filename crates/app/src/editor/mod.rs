//! The interactive editor, free of egui types.
//!
//! The egui layer (`edit_ui`) only translates input into [`Event`]s in world (grid unit)
//! coordinates and shows the [`Effect`]s it gets back; every decision (what a click means, what a
//! drag moves, what an Enter finishes) is made here, so the whole behaviour is tested headlessly by
//! feeding event sequences into an [`EditorState`] against a real `Circuit` and `Library`.
//!
//! Pieces: [`hit`] (hit testing), [`select`] (select, marquee, move, delete, rotate), [`wire`]
//! (the wire tool), [`place`] (ghost and placing), [`clipboard`] (copy, cut, paste), [`props`]
//! (what the properties panel does), [`refs`] (reference designators) and [`doc`] (new and save).

pub mod clipboard;
pub mod doc;
pub mod hit;
pub mod place;
pub mod props;
pub mod refs;
pub mod select;
pub mod vertex;
pub mod wire;

#[cfg(test)]
mod tests;

use short_circ_core::edit::{Edit, EditError, History};
use short_circ_core::{Circuit, PartLookup, Pt, View};

/// Snap a world position to the grid.
pub fn snap(pos: [f64; 2]) -> Pt {
    [pos[0].round() as i32, pos[1].round() as i32]
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[cfg(test)]
impl Mods {
    pub const NONE: Mods = Mods { shift: false, ctrl: false, alt: false };
    pub const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false };
    pub const CTRL: Mods = Mods { shift: false, ctrl: true, alt: false };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    /// A letter, lower case.
    Char(char),
}

/// The editing tools. `Place` is entered through [`Event::StartPlace`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Wire,
    Label,
    Place,
}

/// Commands that menus and keys share (Ctrl+Z, Edit > Undo, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    Rotate,
    Mirror,
    Tidy,
}

/// Input to the editor. Positions are world coordinates in grid units (not yet snapped).
#[derive(Debug, Clone)]
pub enum Event {
    PointerMove { pos: [f64; 2], mods: Mods },
    PointerDown { pos: [f64; 2], button: Button, mods: Mods },
    PointerUp { pos: [f64; 2], button: Button },
    DoubleClick { pos: [f64; 2] },
    Key { key: Key, mods: Mods },
    Command(Command),
    /// Switch tool (toolbar). `Tool::Place` is ignored here: use `StartPlace`.
    SetTool(Tool),
    /// Start placing parts of this type (part browser, Edit > Add part).
    StartPlace { ty: String },
    /// The label popup was confirmed with this net name.
    CommitLabel { name: String },
    /// The label popup was cancelled.
    CancelLabel,
    /// The active view changed (or the circuit was replaced): abort drafts, drop what is not in
    /// the new view from the selection.
    ViewChanged,
}

/// What the UI should do after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Informational message for the status bar.
    Status(String),
    /// Something was refused; the message says why.
    Refused(String),
    Repaint,
    /// The circuit changed: recompute nets, validation and everything else derived.
    CircuitChanged,
    SelectionChanged,
    ToolChanged,
    /// Ask for a net name for a label at this point; answer with `CommitLabel` / `CancelLabel`.
    PromptLabel(Pt),
    /// Show the properties panel (double click on an item).
    OpenProperties,
    /// Text for the OS clipboard (so Ctrl+V reaches the app afterwards).
    ClipboardText(String),
}

/// Pointer tolerances in grid units (the UI derives them from the zoom).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitTol {
    pub pin: f64,
    pub wire: f64,
}

impl Default for HitTol {
    fn default() -> Self {
        HitTol { pin: 0.45, wire: 0.3 }
    }
}

/// Everything the editor reads and writes for one event.
pub struct EditCtx<'a> {
    pub circuit: &'a mut Circuit,
    pub history: &'a mut History,
    pub lib: &'a dyn PartLookup,
    pub view: View,
    pub tol: HitTol,
}

impl<'a> EditCtx<'a> {
    pub fn new(circuit: &'a mut Circuit, history: &'a mut History, lib: &'a dyn PartLookup, view: View) -> Self {
        EditCtx { circuit, history, lib, view, tol: HitTol::default() }
    }

    /// Apply one edit through the history (one undo step unless a group is open).
    pub fn apply(&mut self, edit: Edit) -> Result<(), EditError> {
        self.history.apply(self.circuit, self.lib, edit)
    }

    /// Run `f` on a copy of the circuit and keep the result as ONE undo step when it changed the
    /// canonical text. For changes that are not an [`Edit`] (tidying): a no-op edit takes the
    /// history snapshot first. Returns whether anything changed.
    pub fn apply_direct(&mut self, f: impl FnOnce(&mut Circuit, &dyn PartLookup)) -> bool {
        let mut next = self.circuit.clone();
        f(&mut next, self.lib);
        let (a, b) = (short_circ_core::write::circuit_to_string(self.circuit), short_circ_core::write::circuit_to_string(&next));
        if a == b {
            return false;
        }
        let title = self.circuit.title.clone();
        if self.history.apply(self.circuit, self.lib, Edit::SetTitle(title)).is_err() {
            return false;
        }
        *self.circuit = next;
        true
    }
}

/// An item of the selection. Labels are identified by their index in `circuit.labels`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Part(String),
    Wire(String),
    Label(usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub items: Vec<Item>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, item: &Item) -> bool {
        self.items.contains(item)
    }

    pub fn set(&mut self, item: Item) {
        self.items = vec![item];
    }

    pub fn toggle(&mut self, item: Item) {
        match self.items.iter().position(|i| *i == item) {
            Some(k) => {
                self.items.remove(k);
            }
            None => self.items.push(item),
        }
    }

    pub fn add(&mut self, item: Item) {
        if !self.contains(&item) {
            self.items.push(item);
        }
    }

    pub fn parts(&self) -> Vec<String> {
        self.items.iter().filter_map(|i| if let Item::Part(p) = i { Some(p.clone()) } else { None }).collect()
    }

    pub fn wires(&self) -> Vec<String> {
        self.items.iter().filter_map(|i| if let Item::Wire(p) = i { Some(p.clone()) } else { None }).collect()
    }

    pub fn labels(&self) -> Vec<usize> {
        self.items.iter().filter_map(|i| if let Item::Label(p) = i { Some(*p) } else { None }).collect()
    }

    /// The only selected item, if exactly one is selected.
    pub fn single(&self) -> Option<&Item> {
        if self.items.len() == 1 {
            self.items.first()
        } else {
            None
        }
    }

    /// Drop items that no longer exist in `c`.
    pub fn prune(&mut self, c: &Circuit) {
        self.items.retain(|i| match i {
            Item::Part(id) => c.parts.iter().any(|p| &p.id == id),
            Item::Wire(id) => c.wires.iter().any(|w| &w.id == id),
            Item::Label(k) => *k < c.labels.len(),
        });
    }

    /// Keep only what is present in `view`.
    pub fn retain_in_view(&mut self, c: &Circuit, view: View) {
        self.items.retain(|i| match i {
            Item::Part(id) => c.part(id).is_some_and(|p| p.placement(view).is_some()),
            Item::Wire(id) => c.wires.iter().any(|w| &w.id == id && w.view == view),
            Item::Label(k) => view == View::Schematic && *k < c.labels.len(),
        });
    }
}

/// The editor: current tool, selection and the state of whatever interaction is in progress.
#[derive(Debug, Default)]
pub struct EditorState {
    pub(crate) tool: Tool,
    pub(crate) sel: Selection,
    pub(crate) drag: select::Drag,
    pub(crate) wire: Option<wire::Draft>,
    pub(crate) place: Option<place::PlaceDraft>,
    pub(crate) label_at: Option<Pt>,
    pub(crate) clip: Option<clipboard::Clip>,
    /// Last pointer position, snapped.
    pub(crate) cursor: Option<Pt>,
}

impl EditorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn selection(&self) -> &Selection {
        &self.sel
    }

    /// The marquee rectangle being dragged, `(min, max)` in world coordinates.
    pub fn marquee(&self) -> Option<([f64; 2], [f64; 2])> {
        self.drag.marquee()
    }

    pub fn has_clipboard(&self) -> bool {
        self.clip.is_some()
    }

    pub fn cursor(&self) -> Option<Pt> {
        self.cursor
    }

    /// The point a label is being named for.
    pub fn label_prompt(&self) -> Option<Pt> {
        self.label_at
    }

    /// The wire being drawn, as the canvas should show it.
    pub fn wire_preview(&self, c: &Circuit, lib: &dyn PartLookup, view: View) -> Option<wire::Preview> {
        wire::preview(self, c, lib, view)
    }

    /// The part being placed, as the canvas should show it.
    pub fn ghost(&self, c: &Circuit, lib: &dyn PartLookup, view: View) -> Option<place::Ghost> {
        place::ghost(self, c, lib, view)
    }

    /// Definition of the part being placed.
    pub fn ghost_def(&self) -> Option<&short_circ_core::PartDef> {
        place::ghost_def(self)
    }

    /// Forget everything in progress and the selection (a new circuit was loaded).
    pub fn reset(&mut self) {
        self.tool = Tool::Select;
        self.sel = Selection::default();
        self.drag = select::Drag::None;
        self.wire = None;
        self.place = None;
        self.label_at = None;
        self.cursor = None;
    }

    /// Abort drafts and go back to the Select tool (leaving Edit mode).
    pub fn cancel_all(&mut self, ctx: &mut EditCtx) {
        select::cancel_drag(self, ctx);
        self.wire = None;
        self.place = None;
        self.label_at = None;
        self.tool = Tool::Select;
    }

    /// Human readable hint for the hint bar, per tool.
    pub fn hint(&self, view: View) -> &'static str {
        match self.tool {
            Tool::Select => {
                "click: select   shift+click: add   drag: move / marquee   drag a wire end or bend: move just that point   + dot or Alt+click on a wire: add a bend   double-click a bend: remove it   Del: delete   R: rotate   M: mirror   W: wire   Ctrl+Z/Y: undo/redo"
            }
            Tool::Wire => match view {
                View::Schematic => {
                    "click: start / add corner   click pin or wire: finish   double-click or Enter: finish   Backspace: remove last   Esc: cancel   Shift: flip the L"
                }
                View::Breadboard => {
                    "click: start / add bend   click a hole or pin: finish   double-click or Enter: finish   Backspace: remove last   Esc: cancel"
                }
            },
            Tool::Label => "click a pin or wire to name its net   Esc: select tool",
            Tool::Place => "click: place   R: rotate   M: mirror   right-click or Esc: stop placing",
        }
    }

    /// Feed one event; returns what the UI should do.
    pub fn handle(&mut self, ev: Event, ctx: &mut EditCtx) -> Vec<Effect> {
        let mut fx = Vec::new();
        match ev {
            Event::PointerMove { pos, mods } => {
                self.cursor = Some(snap(pos));
                match self.tool {
                    Tool::Select => select::pointer_move(self, ctx, pos, &mut fx),
                    Tool::Wire => wire::pointer_move(self, ctx, pos, mods, &mut fx),
                    Tool::Place => place::pointer_move(self, pos, &mut fx),
                    Tool::Label => {}
                }
            }
            Event::PointerDown { pos, button, mods } => {
                self.cursor = Some(snap(pos));
                match (button, self.tool) {
                    (Button::Secondary, Tool::Select) => {}
                    (Button::Secondary, _) => self.right_click(ctx, &mut fx),
                    (Button::Primary, Tool::Select) => select::pointer_down(self, ctx, pos, mods, &mut fx),
                    (Button::Primary, Tool::Wire) => wire::pointer_down(self, ctx, pos, mods, &mut fx),
                    (Button::Primary, Tool::Place) => place::pointer_down(self, ctx, pos, &mut fx),
                    (Button::Primary, Tool::Label) => self.label_click(ctx, pos, &mut fx),
                }
            }
            Event::PointerUp { pos, button } => {
                if button == Button::Primary && self.tool == Tool::Select {
                    select::pointer_up(self, ctx, pos, &mut fx);
                }
            }
            Event::DoubleClick { pos } => match self.tool {
                Tool::Wire => wire::double_click(self, ctx, &mut fx),
                Tool::Select => select::double_click(self, ctx, pos, &mut fx),
                _ => {}
            },
            Event::Key { key, mods } => self.key(key, mods, ctx, &mut fx),
            Event::Command(cmd) => self.command(cmd, ctx, &mut fx),
            Event::SetTool(t) => self.set_tool(t, ctx, &mut fx),
            Event::StartPlace { ty } => place::start(self, ctx, &ty, &mut fx),
            Event::CommitLabel { name } => self.commit_label(ctx, &name, &mut fx),
            Event::CancelLabel => {
                self.label_at = None;
                fx.push(Effect::Repaint);
            }
            Event::ViewChanged => {
                select::cancel_drag(self, ctx);
                self.wire = None;
                self.label_at = None;
                self.sel.prune(ctx.circuit);
                self.sel.retain_in_view(ctx.circuit, ctx.view);
                if self.tool == Tool::Label && ctx.view != View::Schematic {
                    self.tool = Tool::Select;
                    fx.push(Effect::ToolChanged);
                }
                fx.push(Effect::SelectionChanged);
                fx.push(Effect::Repaint);
            }
        }
        fx
    }

    fn set_tool(&mut self, t: Tool, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
        if t == Tool::Place {
            return;
        }
        if t == Tool::Label && ctx.view != View::Schematic {
            fx.push(Effect::Refused("labels exist only in the schematic view".into()));
            return;
        }
        select::cancel_drag(self, ctx);
        self.wire = None;
        self.label_at = None;
        self.place = None;
        if self.tool != t {
            self.tool = t;
            fx.push(Effect::ToolChanged);
        }
        fx.push(Effect::Repaint);
    }

    /// Right click in a non-Select tool: cancel the draft, else back to Select.
    fn right_click(&mut self, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
        if self.wire.take().is_some() {
            fx.push(Effect::Repaint);
            return;
        }
        self.set_tool(Tool::Select, ctx, fx);
    }

    fn escape(&mut self, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
        if select::drag_active(self) {
            select::cancel_drag(self, ctx);
            fx.extend([Effect::CircuitChanged, Effect::Repaint]);
            return;
        }
        if self.label_at.take().is_some() {
            fx.push(Effect::Repaint);
            return;
        }
        if self.wire.take().is_some() {
            fx.push(Effect::Repaint);
            return;
        }
        if self.tool != Tool::Select {
            self.set_tool(Tool::Select, ctx, fx);
            return;
        }
        if !self.sel.is_empty() {
            self.sel = Selection::default();
            fx.push(Effect::SelectionChanged);
        }
        fx.push(Effect::Repaint);
    }

    fn key(&mut self, key: Key, mods: Mods, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
        use Command as C;
        if mods.ctrl {
            let cmd = match key {
                Key::Char('z') if mods.shift => Some(C::Redo),
                Key::Char('z') => Some(C::Undo),
                Key::Char('y') => Some(C::Redo),
                Key::Char('a') => Some(C::SelectAll),
                Key::Char('c') => Some(C::Copy),
                Key::Char('x') => Some(C::Cut),
                Key::Char('v') => Some(C::Paste),
                _ => None,
            };
            if let Some(cmd) = cmd {
                self.command(cmd, ctx, fx);
            }
            return;
        }
        match key {
            Key::Escape => self.escape(ctx, fx),
            Key::Enter => {
                if self.tool == Tool::Wire {
                    wire::enter(self, ctx, fx);
                }
            }
            Key::Backspace => match self.tool {
                Tool::Wire => wire::backspace(self, fx),
                Tool::Select => self.command(C::Delete, ctx, fx),
                _ => {}
            },
            Key::Delete => {
                if self.tool == Tool::Select {
                    self.command(C::Delete, ctx, fx);
                }
            }
            Key::Left | Key::Right | Key::Up | Key::Down => {
                if self.tool == Tool::Select {
                    let d = match key {
                        Key::Left => [-1, 0],
                        Key::Right => [1, 0],
                        Key::Up => [0, -1],
                        _ => [0, 1],
                    };
                    select::nudge(self, ctx, d, fx);
                }
            }
            Key::Char('r') => match self.tool {
                Tool::Place => place::rotate(self, fx),
                Tool::Select => self.command(C::Rotate, ctx, fx),
                _ => {}
            },
            Key::Char('m') => match self.tool {
                Tool::Place => place::mirror(self, fx),
                Tool::Select => self.command(C::Mirror, ctx, fx),
                _ => {}
            },
            Key::Char('w') => self.set_tool(Tool::Wire, ctx, fx),
            Key::Char('l') => self.set_tool(Tool::Label, ctx, fx),
            Key::Char('s') => self.set_tool(Tool::Select, ctx, fx),
            Key::Char(_) => {}
        }
    }

    fn command(&mut self, cmd: Command, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
        match cmd {
            Command::Undo | Command::Redo => self.undo_redo(cmd == Command::Undo, ctx, fx),
            Command::Cut => clipboard::cut(self, ctx, fx),
            Command::Copy => {
                clipboard::copy(self, ctx, fx);
            }
            Command::Paste => clipboard::paste(self, ctx, fx),
            Command::Delete => select::delete(self, ctx, fx),
            Command::SelectAll => select::select_all(self, ctx, fx),
            Command::Rotate => select::rotate(self, ctx, 90, fx),
            Command::Mirror => select::mirror(self, ctx, fx),
            Command::Tidy => select::tidy(ctx, fx),
        }
    }

    fn undo_redo(&mut self, undo: bool, ctx: &mut EditCtx, fx: &mut Vec<Effect>) {
        // While a wire is being drawn Ctrl+Z takes back the last vertex first.
        if undo && self.wire.is_some() {
            wire::backspace(self, fx);
            return;
        }
        if select::drag_active(self) {
            select::cancel_drag(self, ctx);
            fx.extend([Effect::CircuitChanged, Effect::Repaint]);
            return;
        }
        let done = if undo { ctx.history.undo(ctx.circuit) } else { ctx.history.redo(ctx.circuit) };
        if done {
            self.sel.prune(ctx.circuit);
            fx.extend([Effect::CircuitChanged, Effect::SelectionChanged, Effect::Repaint]);
            fx.push(Effect::Status(if undo { "Undo".into() } else { "Redo".into() }));
        } else {
            fx.push(Effect::Status(if undo { "Nothing to undo".into() } else { "Nothing to redo".into() }));
        }
    }

    fn label_click(&mut self, ctx: &mut EditCtx, pos: [f64; 2], fx: &mut Vec<Effect>) {
        if ctx.view != View::Schematic {
            fx.push(Effect::Refused("labels exist only in the schematic view".into()));
            return;
        }
        let at = snap(pos);
        if wire::point_is_connectable(ctx.circuit, ctx.lib, ctx.view, at) {
            self.label_at = Some(at);
            fx.extend([Effect::PromptLabel(at), Effect::Repaint]);
        } else {
            fx.push(Effect::Refused("a label must sit on a pin or on a wire".into()));
        }
    }

    fn commit_label(&mut self, ctx: &mut EditCtx, name: &str, fx: &mut Vec<Effect>) {
        let Some(at) = self.label_at.take() else { return };
        let name = name.trim();
        if name.is_empty() {
            fx.push(Effect::Refused("a label needs a name".into()));
            return;
        }
        let label = short_circ_core::Label { at, name: name.to_string(), color: None };
        match ctx.apply(Edit::AddLabel(label)) {
            Ok(()) => {
                self.sel.set(Item::Label(ctx.circuit.labels.len() - 1));
                fx.extend([Effect::CircuitChanged, Effect::SelectionChanged, Effect::Repaint]);
            }
            Err(e) => fx.push(Effect::Refused(e.to_string())),
        }
    }
}
