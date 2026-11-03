//! Headless tests of the editor: event sequences against a real circuit and the built-in library.

use super::*;
use crate::editor::hit;
use crate::editor::props::{self, ColorField};
use crate::editor::refs::ref_prefix;
use short_circ_core::edit::History;
use short_circ_core::transform::pin_world;
use short_circ_core::{Library, Netlist, Placement, Severity, View};
use std::collections::BTreeSet;

/// A circuit, its history, a library and an editor, driven by plain events.
struct Rig {
    c: Circuit,
    h: History,
    lib: Library,
    view: View,
    ed: EditorState,
    last: Vec<Effect>,
}

impl Rig {
    fn new(view: View) -> Rig {
        Rig::with(Circuit::default(), view)
    }

    fn with(c: Circuit, view: View) -> Rig {
        let lib = Library::load(&c, None);
        Rig { c, h: History::new(), lib, view, ed: EditorState::new(), last: Vec::new() }
    }

    fn from_json(src: &str, view: View) -> Rig {
        Rig::with(short_circ_core::parse_circuit(src).unwrap(), view)
    }

    fn ev(&mut self, e: Event) -> Vec<Effect> {
        let mut ctx = EditCtx::new(&mut self.c, &mut self.h, &self.lib, self.view);
        let fx = self.ed.handle(e, &mut ctx);
        self.last = fx.clone();
        fx
    }

    fn mv(&mut self, x: f64, y: f64) -> Vec<Effect> {
        self.ev(Event::PointerMove { pos: [x, y], mods: Mods::NONE })
    }

    fn down(&mut self, x: f64, y: f64) -> Vec<Effect> {
        self.ev(Event::PointerDown { pos: [x, y], button: Button::Primary, mods: Mods::NONE })
    }

    fn down_mods(&mut self, x: f64, y: f64, mods: Mods) -> Vec<Effect> {
        self.ev(Event::PointerDown { pos: [x, y], button: Button::Primary, mods })
    }

    fn up(&mut self, x: f64, y: f64) -> Vec<Effect> {
        self.ev(Event::PointerUp { pos: [x, y], button: Button::Primary })
    }

    fn click(&mut self, x: f64, y: f64) -> Vec<Effect> {
        self.mv(x, y);
        let mut fx = self.down(x, y);
        fx.extend(self.up(x, y));
        self.last = fx.clone();
        fx
    }

    fn key(&mut self, k: Key) -> Vec<Effect> {
        self.ev(Event::Key { key: k, mods: Mods::NONE })
    }

    fn ctrl(&mut self, c: char) -> Vec<Effect> {
        self.ev(Event::Key { key: Key::Char(c), mods: Mods::CTRL })
    }

    fn cmd(&mut self, c: Command) -> Vec<Effect> {
        self.ev(Event::Command(c))
    }

    fn drag(&mut self, from: (f64, f64), to: (f64, f64)) {
        self.mv(from.0, from.1);
        self.down(from.0, from.1);
        // A few intermediate moves, like a real pointer.
        for k in 1..=4 {
            let t = k as f64 / 4.0;
            self.mv(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        }
        self.up(to.0, to.1);
    }

    /// Start placing `ty`, click at `(x, y)`, stop placing; returns the new id.
    fn place(&mut self, ty: &str, x: i32, y: i32) -> String {
        self.ev(Event::StartPlace { ty: ty.to_string() });
        assert_eq!(self.ed.tool(), Tool::Place, "placing {ty}: {:?}", self.last);
        self.mv(x as f64, y as f64);
        self.down(x as f64, y as f64);
        self.up(x as f64, y as f64);
        let id = match self.ed.selection().single() {
            Some(Item::Part(id)) => id.clone(),
            other => panic!("placing {ty} selected {other:?}: {:?}", self.last),
        };
        self.key(Key::Escape);
        assert_eq!(self.ed.tool(), Tool::Select);
        id
    }

    /// Draw one wire with the Wire tool by clicking the points in turn.
    fn wire(&mut self, pts: &[(i32, i32)]) {
        self.key(Key::Char('w'));
        for (x, y) in pts {
            self.click(*x as f64, *y as f64);
        }
        self.key(Key::Escape);
    }

    fn part(&self, id: &str) -> &short_circ_core::Part {
        self.c.part(id).unwrap_or_else(|| panic!("no part {id}"))
    }

    fn at(&self, id: &str) -> Pt {
        self.part(id).placement(self.view).expect("placed").at
    }

    fn wire_pts(&self, id: &str) -> Vec<Pt> {
        self.c.wires.iter().find(|w| w.id == id).unwrap_or_else(|| panic!("no wire {id}")).pts.clone()
    }

    fn errors(&self) -> Vec<String> {
        short_circ_core::validate(&self.c, &self.lib)
            .issues
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| format!("{} {:?}: {}", i.rule, i.view, i.message))
            .collect()
    }

    /// Nets of `view` with two or more pins of non-board parts, as sorted pin sets.
    fn nets(&self, view: View) -> BTreeSet<Vec<String>> {
        props::nets_from_view(&self.c, &self.lib, view)
            .into_iter()
            .map(|n| {
                let mut p = n.pins;
                p.sort();
                p
            })
            .collect()
    }

    fn ctx(&mut self) -> EditCtx<'_> {
        EditCtx::new(&mut self.c, &mut self.h, &self.lib, self.view)
    }

    fn can_undo(&self) -> bool {
        self.h.can_undo()
    }

    fn fingerprint(&self) -> String {
        short_circ_core::write::circuit_to_string(&self.c)
    }

    fn has_refusal(&self) -> bool {
        self.last.iter().any(|e| matches!(e, Effect::Refused(_)))
    }
}

fn set(items: &[Item]) -> Vec<Item> {
    items.to_vec()
}

fn part(id: &str) -> Item {
    Item::Part(id.to_string())
}

fn wire_item(id: &str) -> Item {
    Item::Wire(id.to_string())
}

/// R1 (14,9), D1 (26,9) joined by w1 (22,10)-(26,10); B1 at (2,9) with w2 from its pos pin.
const BASE: &str = r#"{"format":"short-circ/2","parts":[
 {"id":"R1","type":"resistor","props":{"value":"330"},"schematic":{"at":[14,9]}},
 {"id":"D1","type":"led","props":{},"schematic":{"at":[26,9]}},
 {"id":"B1","type":"battery","props":{"value":"9V"},"schematic":{"at":[2,9]}}],
 "wires":[{"id":"w1","view":"schematic","pts":[[22,10],[26,10]]},
          {"id":"w2","view":"schematic","pts":[[3,9],[3,6],[14,6],[14,10]]}],
 "labels":[{"at":[3,9],"name":"VCC"}]}"#;

fn base() -> Rig {
    Rig::from_json(BASE, View::Schematic)
}

// ---- reference designators ------------------------------------------------------------------

#[test]
fn ref_prefixes() {
    let cases = [
        ("resistor", "R"),
        ("capacitor", "C"),
        ("capacitor_polarized", "C"),
        ("led", "D"),
        ("diode", "D"),
        ("zener", "D"),
        ("battery", "B"),
        ("voltage_source_ac", "B"),
        ("inductor", "L"),
        ("switch", "SW"),
        ("pushbutton", "SW"),
        ("npn", "Q"),
        ("pnp", "Q"),
        ("nmos", "Q"),
        ("pmos", "Q"),
        ("opamp", "U"),
        ("dip8", "U"),
        ("dip40", "U"),
        ("connector2", "J"),
        ("header4", "J"),
        ("crystal", "Y"),
        ("fuse", "F"),
        ("buzzer", "BZ"),
        ("potentiometer", "RV"),
        ("ground", "GND"),
        ("vcc", "VCC"),
        ("breadboard_half", "BB"),
        ("breadboard_mini", "BB"),
        ("something_else", "P"),
        ("dipx", "P"),
    ];
    for (ty, want) in cases {
        assert_eq!(ref_prefix(ty), want, "{ty}");
    }
    // Boards by category too (imported boards have arbitrary type names).
    let lib = Library::builtin();
    assert_eq!(refs::ref_prefix_for("my_board", lib.get("breadboard_half")), "BB");
}

#[test]
fn placed_parts_get_unique_designators_across_parts_and_wires() {
    let mut r = Rig::new(View::Schematic);
    assert_eq!(r.place("resistor", 0, 0), "R1");
    assert_eq!(r.place("resistor", 0, 6), "R2");
    assert_eq!(r.place("ground", 20, 0), "GND1");
    assert_eq!(r.place("vcc", 30, 0), "VCC1");
    assert_eq!(r.place("led", 40, 0), "D1");
    // A wire named like a part prefix blocks that id.
    r.c.wires.push(short_circ_core::Wire { id: "R3".into(), view: View::Schematic, pts: vec![[0, 0], [1, 0]], color: None });
    assert_eq!(r.place("resistor", 0, 12), "R4");
    assert_eq!(r.place("dip8", 50, 0), "U1");
}

// ---- select ---------------------------------------------------------------------------------

#[test]
fn click_selects_part_wire_and_label_and_empty_clears() {
    let mut r = base();
    r.click(18.0, 10.0); // R1 body
    assert_eq!(r.ed.selection().items, set(&[part("R1")]));
    r.click(24.0, 10.0); // wire w1
    assert_eq!(r.ed.selection().items, set(&[wire_item("w1")]));
    r.click(26.0, 10.0); // pin D1.anode selects the part, not the wire end
    assert_eq!(r.ed.selection().items, set(&[part("D1")]));
    // The label's text sits up and to the right of its point.
    r.click(4.5, 8.2);
    assert_eq!(r.ed.selection().items, set(&[Item::Label(0)]));
    r.click(40.0, 40.0);
    assert!(r.ed.selection().is_empty());
    assert!(!r.can_undo(), "selecting never records history");
}

#[test]
fn shift_click_toggles() {
    let mut r = base();
    r.click(18.0, 10.0);
    r.mv(28.0, 10.0);
    r.down_mods(28.0, 10.0, Mods::SHIFT);
    r.up(28.0, 10.0);
    assert_eq!(r.ed.selection().items, set(&[part("R1"), part("D1")]));
    r.down_mods(18.0, 10.0, Mods::SHIFT);
    r.up(18.0, 10.0);
    assert_eq!(r.ed.selection().items, set(&[part("D1")]));
    assert!(!r.can_undo());
}

#[test]
fn marquee_selects_only_what_is_fully_inside() {
    let mut r = base();
    // Covers R1 (14..22 x 9..11) and w1's end at (22,10) but not D1; w1 reaches 26 so is partial.
    r.drag((13.0, 5.0), (23.5, 12.0));
    let items = &r.ed.selection().items;
    assert!(items.contains(&part("R1")), "{items:?}");
    assert!(!items.contains(&part("D1")) && !items.contains(&wire_item("w1")), "{items:?}");
    // A box around the right half: D1 and w1 fully inside.
    r.drag((21.0, 8.0), (31.0, 13.0));
    assert_eq!(r.ed.selection().items, set(&[part("D1"), wire_item("w1")]));
    // Shift extends the selection.
    r.mv(12.0, 5.0);
    r.down_mods(12.0, 5.0, Mods::SHIFT);
    r.mv(23.0, 12.0);
    r.up(23.0, 12.0);
    assert!(r.ed.selection().contains(&part("R1")) && r.ed.selection().contains(&part("D1")));
    assert!(!r.can_undo());
}

#[test]
fn marquee_drag_in_empty_space_draws_a_rectangle_effect() {
    let mut r = base();
    r.mv(10.0, 20.0);
    r.down(10.0, 20.0);
    r.mv(15.0, 24.0);
    assert_eq!(r.ed.marquee(), Some(([10.0, 20.0], [15.0, 24.0])));
    r.up(15.0, 24.0);
    assert_eq!(r.ed.marquee(), None);
}

#[test]
fn select_all_takes_the_active_view_only() {
    let mut r = Rig::from_json(include_str!("../../../../examples/breadboard_demo.circ"), View::Schematic);
    r.ctrl('a');
    let sch = r.ed.selection().items.len();
    // 3 parts placed in the schematic (the board is breadboard only), 3 schematic wires.
    assert_eq!(sch, 6, "{:?}", r.ed.selection());
    assert!(!r.ed.selection().contains(&part("BB1")) && !r.ed.selection().contains(&wire_item("j1")));
    r.view = View::Breadboard;
    r.ev(Event::ViewChanged);
    assert_eq!(r.ed.selection().wires().len(), 0, "wires of the schematic leave the selection");
    assert_eq!(r.ed.selection().parts().len(), 3, "parts also placed in the breadboard stay");
    r.ctrl('a');
    assert_eq!(r.ed.selection().items.len(), 4 + 2);
}

#[test]
fn view_change_keeps_only_what_is_in_the_new_view() {
    let mut r = Rig::from_json(include_str!("../../../../examples/breadboard_demo.circ"), View::Schematic);
    r.click(18.0, 10.0); // R1 body in the schematic
    assert_eq!(r.ed.selection().items, set(&[part("R1")]));
    r.view = View::Breadboard;
    r.ev(Event::ViewChanged);
    assert_eq!(r.ed.selection().items, set(&[part("R1")]), "R1 is placed in both views");
    r.c.parts.iter_mut().find(|p| p.id == "R1").unwrap().breadboard = None;
    r.ev(Event::ViewChanged);
    assert!(r.ed.selection().is_empty());
}

// ---- move -----------------------------------------------------------------------------------

#[test]
fn drag_move_is_one_undo_step_and_rubber_bands_wires() {
    let mut r = base();
    let before = r.fingerprint();
    r.drag((28.0, 10.0), (28.0, 13.0)); // D1 down by 3
    assert_eq!(r.at("D1"), [26, 12]);
    assert_eq!(r.wire_pts("w1"), vec![[22, 10], [26, 13]], "the wire end follows the pin");
    assert!(r.h.dirty());
    let after = r.fingerprint();
    assert!(r.h.undo(&mut r.c), "exactly one step back");
    assert_eq!(r.fingerprint(), before);
    assert!(!r.h.can_undo(), "a drag with many pointer moves is one undo step");
    assert!(!r.h.dirty());
    assert!(r.h.redo(&mut r.c));
    assert_eq!(r.fingerprint(), after);
}

#[test]
fn drag_of_a_selection_moves_parts_wires_and_labels_together() {
    let mut r = base();
    r.drag((12.0, 3.0), (31.0, 13.0)); // marquee: everything of w1, R1, D1 ... but not B1/w2/label
    let sel = r.ed.selection().items.clone();
    assert!(sel.contains(&part("R1")) && sel.contains(&part("D1")) && sel.contains(&wire_item("w1")), "{sel:?}");
    let w2 = r.wire_pts("w2");
    r.drag((18.0, 10.0), (20.0, 11.0)); // grab R1: the whole selection moves by (2,1)
    assert_eq!(r.at("R1"), [16, 10]);
    assert_eq!(r.at("D1"), [28, 10]);
    assert_eq!(r.wire_pts("w1"), vec![[24, 11], [28, 11]], "a selected wire shifts as a whole, ends not doubled");
    // w2 ends on R1.a (14,10): rubber-banded to the new pin, nothing else of it moved.
    let moved = r.wire_pts("w2");
    assert_eq!(moved[..3], w2[..3]);
    assert_eq!(moved[3], [16, 11]);
    assert!(!r.errors().iter().any(|e| e.starts_with("wire-overlap")));
    r.h.undo(&mut r.c);
    assert_eq!(r.at("R1"), [14, 9]);
    assert_eq!(r.wire_pts("w2"), w2);
    // The marquee was no edit: exactly the drag was one step.
    assert!(!r.h.can_undo());
}

#[test]
fn pressing_on_a_selected_item_keeps_the_selection_but_a_click_narrows_it() {
    let mut r = base();
    r.drag((12.0, 3.0), (31.0, 13.0));
    let n = r.ed.selection().items.len();
    assert!(n >= 3);
    r.down(18.0, 10.0);
    assert_eq!(r.ed.selection().items.len(), n, "press keeps the multi selection (it may become a drag)");
    r.up(18.0, 10.0);
    assert_eq!(r.ed.selection().items, set(&[part("R1")]), "released without moving: just that item");
    assert!(!r.can_undo());
}

#[test]
fn dragging_away_and_back_leaves_no_undo_step_and_no_dirt() {
    let mut r = base();
    r.mv(28.0, 10.0);
    r.down(28.0, 10.0);
    r.mv(28.0, 14.0);
    assert_eq!(r.at("D1"), [26, 13], "live preview");
    r.mv(28.0, 10.0);
    r.up(28.0, 10.0);
    assert_eq!(r.at("D1"), [26, 9]);
    assert!(!r.h.dirty());
}

#[test]
fn escape_during_a_drag_puts_everything_back() {
    let mut r = base();
    let before = r.fingerprint();
    r.mv(28.0, 10.0);
    r.down(28.0, 10.0);
    r.mv(30.0, 12.0);
    assert_ne!(r.fingerprint(), before);
    r.key(Key::Escape);
    assert_eq!(r.fingerprint(), before);
    r.up(30.0, 12.0);
    assert_eq!(r.fingerprint(), before);
    assert!(!r.h.dirty());
}

#[test]
fn arrow_keys_nudge_one_undo_step_per_press() {
    let mut r = base();
    r.click(18.0, 10.0);
    r.key(Key::Right);
    r.key(Key::Right);
    r.key(Key::Down);
    assert_eq!(r.at("R1"), [16, 10]);
    assert_eq!(r.wire_pts("w2")[3], [16, 11]);
    r.ctrl('z');
    assert_eq!(r.at("R1"), [16, 9]);
    r.ctrl('z');
    assert_eq!(r.at("R1"), [15, 9]);
    r.ctrl('z');
    assert_eq!(r.at("R1"), [14, 9]);
    assert!(!r.h.can_undo());
    r.key(Key::Up);
    r.key(Key::Left);
    assert_eq!(r.at("R1"), [13, 8]);
}

#[test]
fn moving_a_part_takes_its_labels_along() {
    let mut r = base();
    // B1.pos is at (3,9) and carries the VCC label.
    r.drag((3.0, 11.0), (6.0, 11.0));
    assert_eq!(r.at("B1"), [5, 9]);
    assert_eq!(r.c.labels[0].at, [6, 9], "the label stays on the pin");
    r.h.undo(&mut r.c);
    assert_eq!(r.c.labels[0].at, [3, 9]);
}

#[test]
fn moving_a_part_in_one_view_never_touches_the_other() {
    let mut r = Rig::from_json(include_str!("../../../../examples/breadboard_demo.circ"), View::Schematic);
    let bb_before = r.part("R1").placement(View::Breadboard).cloned();
    r.drag((18.0, 10.0), (20.0, 12.0));
    assert_eq!(r.at("R1"), [16, 11]);
    assert_eq!(r.part("R1").placement(View::Breadboard).cloned(), bb_before);
    assert_eq!(r.wire_pts("j1"), vec![[4, 25], [11, 17]], "breadboard wires untouched");
}

// ---- delete, rotate, mirror -----------------------------------------------------------------

#[test]
fn delete_removes_selected_parts_wires_and_labels_in_one_step() {
    let mut r = base();
    let before = r.fingerprint();
    r.click(18.0, 10.0);
    r.mv(24.0, 10.0);
    r.down_mods(24.0, 10.0, Mods::SHIFT);
    r.up(24.0, 10.0);
    r.mv(4.5, 8.2);
    r.down_mods(4.5, 8.2, Mods::SHIFT);
    r.up(4.5, 8.2);
    assert_eq!(r.ed.selection().items.len(), 3);
    r.key(Key::Delete);
    assert!(r.c.part("R1").is_none() && r.c.wires.iter().all(|w| w.id != "w1") && r.c.labels.is_empty());
    assert!(r.c.part("D1").is_some(), "unrelated parts stay");
    assert!(r.ed.selection().is_empty());
    r.ctrl('z');
    assert_eq!(r.fingerprint(), before, "one undo restores all three");
    assert!(!r.h.can_undo());
    r.ctrl('y');
    assert!(r.c.part("R1").is_none());
}

#[test]
fn delete_with_nothing_selected_does_nothing() {
    let mut r = base();
    r.key(Key::Delete);
    assert!(!r.can_undo());
}

#[test]
fn r_rotates_selected_parts_about_their_origin_and_m_mirrors() {
    let mut r = base();
    r.click(18.0, 10.0);
    r.key(Key::Char('r'));
    let pl = r.part("R1").placement(View::Schematic).unwrap().clone();
    assert_eq!((pl.at, pl.rot, pl.mirror), ([14, 9], 90, false));
    // R1.a (0,1) rotated 90 clockwise -> (-1,0): world (13,9); the wire end followed.
    assert_eq!(r.wire_pts("w2")[3], [13, 9]);
    r.key(Key::Char('m'));
    assert!(r.part("R1").placement(View::Schematic).unwrap().mirror);
    r.ctrl('z');
    r.ctrl('z');
    assert_eq!(r.part("R1").placement(View::Schematic).unwrap().rot, 0);
    // Several parts rotate in one undo step.
    r.ctrl('a');
    r.cmd(Command::Rotate);
    assert_eq!(r.part("D1").placement(View::Schematic).unwrap().rot, 90);
    assert_eq!(r.part("R1").placement(View::Schematic).unwrap().rot, 90);
    r.ctrl('z');
    assert_eq!(r.part("D1").placement(View::Schematic).unwrap().rot, 0);
    assert_eq!(r.part("R1").placement(View::Schematic).unwrap().rot, 0);
    assert!(!r.h.can_undo());
}

#[test]
fn tidy_is_one_step_and_a_no_op_records_nothing() {
    let mut r = Rig::from_json(
        r#"{"format":"short-circ/2","parts":[],"wires":[{"id":"w1","view":"schematic","pts":[[0,0],[3,0],[6,0],[6,4]]}]}"#,
        View::Schematic,
    );
    r.cmd(Command::Tidy);
    assert_eq!(r.wire_pts("w1"), vec![[0, 0], [6, 0], [6, 4]]);
    assert!(r.h.dirty());
    r.cmd(Command::Tidy);
    r.ctrl('z');
    assert_eq!(r.wire_pts("w1").len(), 4, "one undo step back to the untidy wire");
    assert!(!r.can_undo(), "the second tidy changed nothing and recorded nothing");
}

// ---- clipboard ------------------------------------------------------------------------------

#[test]
fn copy_paste_remaps_ids_offsets_and_selects_the_copies() {
    let mut r = base();
    r.drag((12.0, 5.0), (31.0, 13.0));
    assert!(r.ed.selection().contains(&part("R1")) && r.ed.selection().contains(&part("D1")));
    r.ctrl('c');
    assert!(r.last.iter().any(|e| matches!(e, Effect::ClipboardText(_))));
    assert!(r.ed.has_clipboard());
    let n_parts = r.c.parts.len();
    r.ctrl('v');
    assert_eq!(r.c.parts.len(), n_parts + 2);
    let r2 = r.part("R2");
    assert_eq!(r2.placement(View::Schematic).unwrap().at, [16, 11], "+2,+2");
    assert_eq!(r2.props["value"], "330");
    assert_eq!(r.part("D2").placement(View::Schematic).unwrap().at, [28, 11]);
    assert_eq!(r.ed.selection().parts(), vec!["R2".to_string(), "D2".to_string()], "pasted copies are selected");
    // The wire between R1 and D1 was copied, with a fresh id and the offset; w2 (to B1) was not.
    let w = r.c.wires.iter().find(|w| w.id == "w3").expect("pasted wire");
    assert_eq!(w.pts, vec![[24, 12], [28, 12]]);
    assert_eq!(r.c.wires.len(), 3);
    assert!(r.ed.selection().contains(&wire_item("w3")));
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    // One undo removes the whole paste.
    r.ctrl('z');
    assert_eq!(r.c.parts.len(), n_parts);
    assert_eq!(r.c.wires.len(), 2);
    r.ctrl('y');
    assert_eq!(r.c.parts.len(), n_parts + 2);
    // Pasting again moves further.
    r.ctrl('v');
    assert_eq!(r.part("R3").placement(View::Schematic).unwrap().at, [18, 13]);
}

#[test]
fn pasted_parts_keep_only_the_active_view_placement() {
    let mut r = Rig::from_json(include_str!("../../../../examples/breadboard_demo.circ"), View::Schematic);
    r.click(18.0, 10.0); // R1
    r.ctrl('c');
    r.ctrl('v');
    let p = r.part("R2");
    assert!(p.placement(View::Schematic).is_some());
    assert!(p.placement(View::Breadboard).is_none(), "placement of the ACTIVE view only");
    // Pasting into the breadboard view places it there, in that view only.
    r.view = View::Breadboard;
    r.ev(Event::ViewChanged);
    r.ctrl('v');
    let q = r.part("R3");
    assert!(q.placement(View::Schematic).is_none() && q.placement(View::Breadboard).is_some());
    // R1's breadboard position was never copied: the clipboard holds the schematic placement.
    assert_eq!(q.placement(View::Breadboard).unwrap().at, [18, 13], "second paste of the same clip: +4,+4");
}

#[test]
fn cut_removes_and_paste_brings_back_as_one_step_each() {
    let mut r = base();
    r.click(18.0, 10.0);
    r.ctrl('x');
    assert!(r.c.part("R1").is_none());
    assert!(r.ed.has_clipboard());
    r.ctrl('v');
    assert!(r.c.part("R1").is_some(), "the id is free again");
    r.ctrl('z');
    assert!(r.c.part("R1").is_none());
    r.ctrl('z');
    assert!(r.c.part("R1").is_some());
}

#[test]
fn paste_without_a_clipboard_and_copy_without_selection_do_nothing() {
    let mut r = base();
    r.ctrl('v');
    r.ctrl('c');
    assert!(!r.has_refusal());
    assert!(!r.can_undo());
    assert!(!r.ed.has_clipboard());
}

// ---- place ----------------------------------------------------------------------------------

#[test]
fn place_ghost_rotates_and_a_click_places_with_that_transform() {
    let mut r = Rig::new(View::Schematic);
    r.ev(Event::StartPlace { ty: "resistor".into() });
    r.mv(10.2, 5.8);
    let g = r.ed.ghost(&r.c, &r.lib, r.view).unwrap();
    assert_eq!(g.placement.at, [10, 6]);
    assert_eq!(g.pins.len(), 2);
    r.key(Key::Char('r'));
    r.key(Key::Char('m'));
    let g = r.ed.ghost(&r.c, &r.lib, r.view).unwrap();
    assert_eq!((g.placement.rot, g.placement.mirror), (90, true));
    r.down(10.0, 6.0);
    r.up(10.0, 6.0);
    let pl = r.part("R1").placement(View::Schematic).unwrap().clone();
    assert_eq!(pl, Placement { at: [10, 6], rot: 90, mirror: true });
    assert_eq!(r.part("R1").props["value"], "1k");
    // The tool stays active for repeated placing, with the same rotation.
    assert_eq!(r.ed.tool(), Tool::Place);
    r.down(20.0, 6.0);
    r.up(20.0, 6.0);
    assert_eq!(r.part("R2").placement(View::Schematic).unwrap().rot, 90);
    // Right click ends it.
    r.ev(Event::PointerDown { pos: [30.0, 6.0], button: Button::Secondary, mods: Mods::NONE });
    assert_eq!(r.ed.tool(), Tool::Select);
    assert!(r.ed.ghost(&r.c, &r.lib, r.view).is_none(), "ending the tool drops the ghost");
    // Each placement is one undo step.
    r.ctrl('z');
    assert!(r.c.part("R2").is_none() && r.c.part("R1").is_some());
    r.ctrl('z');
    assert!(r.c.parts.is_empty());
}

#[test]
fn r_while_placing_rotates_the_ghost_not_the_selection() {
    let mut r = base();
    r.click(18.0, 10.0);
    r.ev(Event::StartPlace { ty: "led".into() });
    r.key(Key::Char('r'));
    assert_eq!(r.part("R1").placement(View::Schematic).unwrap().rot, 0);
    assert!(!r.can_undo());
}

#[test]
fn escape_ends_placing() {
    let mut r = Rig::new(View::Schematic);
    r.ev(Event::StartPlace { ty: "led".into() });
    assert_eq!(r.ed.tool(), Tool::Place);
    r.key(Key::Escape);
    assert_eq!(r.ed.tool(), Tool::Select);
    r.click(5.0, 5.0);
    assert!(r.c.parts.is_empty());
}

#[test]
fn a_type_without_geometry_in_the_view_cannot_be_placed() {
    let mut r = Rig::new(View::Schematic);
    r.ev(Event::StartPlace { ty: "breadboard_half".into() });
    assert!(r.has_refusal(), "{:?}", r.last);
    assert!(r.last.iter().any(|e| matches!(e, Effect::Refused(m) if m.contains("no schematic view"))));
    assert_eq!(r.ed.tool(), Tool::Select);
    let mut b = Rig::new(View::Breadboard);
    b.ev(Event::StartPlace { ty: "ground".into() });
    assert!(b.last.iter().any(|e| matches!(e, Effect::Refused(m) if m.contains("no breadboard view"))));
    // Switching view while placing: the click is refused with the reason.
    let mut r = Rig::new(View::Schematic);
    r.ev(Event::StartPlace { ty: "ground".into() });
    r.view = View::Breadboard;
    r.down(5.0, 5.0);
    assert!(r.has_refusal() && r.c.parts.is_empty());
    assert!(r.ed.ghost(&r.c, &r.lib, r.view).unwrap().error.is_some());
    // Unknown types are refused too.
    r.ev(Event::StartPlace { ty: "no_such_part".into() });
    assert!(r.has_refusal());
}

#[test]
fn generated_families_can_be_placed() {
    let mut r = Rig::new(View::Schematic);
    assert_eq!(r.place("dip8", 5, 5), "U1");
    assert_eq!(r.part("U1").ty, "dip8");
    let mut b = Rig::new(View::Breadboard);
    assert_eq!(b.place("header4", 5, 5), "J1");
}

fn hole(lib: &Library, id: &str) -> Pt {
    let pos = lib.get("breadboard_half").unwrap().breadboard.as_ref().unwrap().pins[id];
    pin_world(&Placement::at(0, 0), &pos)
}

fn pin_local(lib: &Library, ty: &str, view: View, pin: &str) -> Pt {
    lib.get(ty).unwrap().view(view).unwrap().pins[pin].at
}

#[test]
fn ghost_highlights_the_board_holes_under_its_pins() {
    let mut r = Rig::new(View::Breadboard);
    r.place("breadboard_half", 0, 0);
    r.ev(Event::StartPlace { ty: "resistor".into() });
    let h5 = hole(&r.lib, "c5");
    let a = pin_local(&r.lib, "resistor", View::Breadboard, "a");
    let at = [h5[0] - a[0], h5[1] - a[1]];
    r.mv(at[0] as f64, at[1] as f64);
    let g = r.ed.ghost(&r.c, &r.lib, r.view).unwrap();
    assert!(g.pins.iter().all(|p| p.on_hole), "{g:?}");
    // Off the board there are no holes under the pins.
    r.mv(-30.0, -30.0);
    let g = r.ed.ghost(&r.c, &r.lib, r.view).unwrap();
    assert!(g.pins.iter().all(|p| !p.on_hole));
}

#[test]
fn placing_on_a_hole_puts_the_pin_exactly_on_it() {
    let mut r = Rig::new(View::Breadboard);
    r.place("breadboard_half", 0, 0);
    let h = hole(&r.lib, "e10");
    let a = pin_local(&r.lib, "resistor", View::Breadboard, "a");
    let id = r.place("resistor", h[0] - a[0], h[1] - a[1]);
    let pl = r.part(&id).placement(View::Breadboard).unwrap().clone();
    let pos = r.lib.get("resistor").unwrap().breadboard.as_ref().unwrap().pins["a"];
    assert_eq!(pin_world(&pl, &pos), h);
    let nl = Netlist::build(&r.c, &r.lib, View::Breadboard);
    let net = nl.net_of_pin(&id, "a").unwrap();
    assert!(nl.nets[net].pins.iter().any(|p| p.part == "BB1" && p.pin == "e10"), "{:?}", nl.nets[net]);
}

#[test]
fn a_wire_attached_part_moved_after_placing_drags_its_wire() {
    let mut r = Rig::new(View::Schematic);
    r.place("resistor", 10, 10);
    r.place("resistor", 30, 10);
    r.wire(&[(18, 11), (30, 11)]);
    assert_eq!(r.wire_pts("w1"), vec![[18, 11], [30, 11]]);
    // Move R1 down 2 by dragging its body; the wire end on R1.b follows.
    r.drag((14.0, 11.0), (14.0, 13.0));
    assert_eq!(r.at("R1"), [10, 12]);
    assert_eq!(r.wire_pts("w1"), vec![[18, 13], [30, 11]]);
    // Rotating the part keeps the end on its pin too.
    r.click(14.0, 13.0);
    r.key(Key::Char('r'));
    let pin_b = {
        let pl = r.part("R1").placement(View::Schematic).unwrap().clone();
        pin_world(&pl, &r.lib.get("resistor").unwrap().schematic.as_ref().unwrap().pins["b"])
    };
    assert_eq!(r.wire_pts("w1")[0], pin_b);
}

// ---- wire tool ------------------------------------------------------------------------------

#[test]
fn l_route_rule_horizontal_first_unless_cursor_is_closer_to_vertical_or_shift() {
    use wire::route;
    let s = View::Schematic;
    assert_eq!(route(s, [0, 0], [5, 0], false), vec![[5, 0]]);
    assert_eq!(route(s, [0, 0], [0, 5], false), vec![[0, 5]]);
    assert_eq!(route(s, [0, 0], [0, 0], false), Vec::<Pt>::new());
    assert_eq!(route(s, [0, 0], [6, 4], false), vec![[6, 0], [6, 4]], "dx > dy: horizontal first");
    assert_eq!(route(s, [0, 0], [4, 6], false), vec![[0, 6], [4, 6]], "dy > dx: vertical first");
    assert_eq!(route(s, [0, 0], [5, 5], false), vec![[5, 0], [5, 5]], "tie: horizontal first");
    assert_eq!(route(s, [0, 0], [6, 4], true), vec![[0, 4], [6, 4]], "shift flips");
    assert_eq!(route(s, [0, 0], [4, 6], true), vec![[4, 0], [4, 6]]);
    assert_eq!(route(s, [3, 3], [-1, 0], false), vec![[-1, 3], [-1, 0]]);
    // Breadboard wires go straight.
    assert_eq!(route(View::Breadboard, [0, 0], [6, 4], false), vec![[6, 4]]);
}

#[test]
fn schematic_wire_tool_draws_an_l_and_ends_on_a_pin() {
    let mut r = base();
    r.key(Key::Char('w'));
    assert_eq!(r.ed.tool(), Tool::Wire);
    r.click(30.0, 10.0); // D1.cathode: start
    r.mv(26.0, 16.0);
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert_eq!(p.pts, vec![[30, 10], [30, 16], [26, 16]], "dy > dx: vertical first");
    assert!(p.error.is_none() && p.start_on_pin && !p.end_on_pin);
    r.click(26.0, 16.0);
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_some(), "not finished: no pin, no wire there");
    // End on B1.neg at (3,13).
    r.click(3.0, 13.0);
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_none(), "ended on a pin");
    assert_eq!(r.wire_pts("w3"), vec![[30, 10], [30, 16], [3, 16], [3, 13]], "collinear points merged");
    assert_eq!(r.c.wires.last().unwrap().view, View::Schematic);
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    // One undo step for the whole wire.
    r.ctrl('z');
    assert!(r.c.wires.iter().all(|w| w.id != "w3"));
    assert!(!r.h.can_undo());
    assert!(!r.h.dirty());
}

#[test]
fn wire_tool_shift_flips_the_l() {
    let mut r = base();
    r.key(Key::Char('w'));
    r.click(30.0, 10.0);
    r.ev(Event::PointerMove { pos: [24.0, 14.0], mods: Mods::SHIFT });
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert_eq!(p.pts, vec![[30, 10], [30, 14], [24, 14]], "dx > dy would be horizontal first; shift flips");
    r.ev(Event::PointerDown { pos: [24.0, 14.0], button: Button::Primary, mods: Mods::SHIFT });
    r.key(Key::Enter);
    assert_eq!(r.wire_pts("w3"), vec![[30, 10], [30, 14], [24, 14]]);
}

#[test]
fn wire_ends_on_double_click_enter_and_esc_or_backspace_edit_the_draft() {
    let mut r = Rig::new(View::Schematic);
    r.key(Key::Char('w'));
    // Double click: a click, a click again on the same point (ignored) and the double click event.
    r.click(0.0, 0.0);
    r.click(6.0, 0.0);
    r.click(6.0, 0.0);
    r.ev(Event::DoubleClick { pos: [6.0, 0.0] });
    assert_eq!(r.wire_pts("w1"), vec![[0, 0], [6, 0]]);
    // Enter finishes with the committed vertices.
    r.click(0.0, 4.0);
    r.click(5.0, 4.0);
    r.key(Key::Enter);
    assert_eq!(r.wire_pts("w2"), vec![[0, 4], [5, 4]]);
    // Escape cancels the draft but keeps the tool; a second Escape returns to Select.
    r.click(0.0, 8.0);
    r.click(5.0, 8.0);
    r.key(Key::Escape);
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_none());
    assert_eq!(r.ed.tool(), Tool::Wire);
    assert_eq!(r.c.wires.len(), 2);
    r.key(Key::Escape);
    assert_eq!(r.ed.tool(), Tool::Select);
    // Backspace removes the last vertex; at the start point it cancels.
    r.key(Key::Char('w'));
    r.click(0.0, 12.0);
    r.click(5.0, 12.0);
    r.click(5.0, 16.0);
    r.key(Key::Backspace);
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert_eq!(p.pts[0], [0, 12]);
    assert!(p.pts.contains(&[5, 12]) && !p.pts.contains(&[5, 16]) || p.pts.last() == Some(&[5, 16]));
    r.key(Key::Backspace);
    r.key(Key::Backspace);
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_none());
    // A lone click then Enter makes no wire.
    r.click(0.0, 20.0);
    r.key(Key::Enter);
    assert_eq!(r.c.wires.len(), 2);
}

#[test]
fn double_click_after_a_pin_ended_the_wire_leaves_no_stray_wire() {
    let mut r = base();
    r.key(Key::Char('w'));
    r.click(30.0, 10.0);
    r.click(30.0, 6.0);
    // Second click of a double click on a pin: the first click already ended the wire there.
    r.click(3.0, 6.0);
    let n = r.c.wires.len();
    r.click(3.0, 9.0);
    r.ev(Event::DoubleClick { pos: [3.0, 9.0] });
    assert!(r.c.wires.len() <= n + 1, "no one-point wires");
    assert!(r.c.wires.iter().all(|w| w.pts.len() >= 2));
}

#[test]
fn wire_ending_on_the_interior_of_another_wire_is_a_t_junction() {
    let mut r = base();
    // w1 runs (22,10)-(26,10); come in from below onto (24,10).
    r.key(Key::Char('w'));
    r.click(24.0, 16.0);
    r.click(24.0, 10.0);
    assert_eq!(r.wire_pts("w3"), vec![[24, 16], [24, 10]]);
    let nl = Netlist::build(&r.c, &r.lib, View::Schematic);
    assert_eq!(nl.net_of_wire("w3"), nl.net_of_wire("w1"), "joined by the T");
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    assert_eq!(r.ed.tool(), Tool::Wire);
}

#[test]
fn a_preview_that_would_pass_through_a_pin_is_red_and_the_click_is_refused() {
    let mut r = base();
    // Left of R1.a (14,10): a horizontal wire from (10,10) to (16,10) would cross the pin (14,10).
    r.key(Key::Char('w'));
    r.click(10.0, 10.0);
    r.mv(16.0, 10.0);
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert!(p.error.as_deref().is_some_and(|e| e.contains("R1.a")), "{p:?}");
    let before = r.fingerprint();
    r.click(16.0, 10.0);
    assert!(r.has_refusal(), "{:?}", r.last);
    assert!(r.last.iter().any(|e| matches!(e, Effect::Refused(m) if m.contains("pass through pin"))));
    assert_eq!(r.fingerprint(), before);
    // The draft is still alive: stop before the pin and finish on it.
    r.mv(12.0, 10.0);
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert_eq!((p.pts.clone(), p.error), (vec![[10, 10], [12, 10]], None));
    r.click(14.0, 10.0);
    assert_eq!(r.wire_pts("w3"), vec![[10, 10], [14, 10]]);
}

#[test]
fn a_wire_that_would_overlap_another_or_itself_is_refused() {
    let mut r = base();
    // w1 is (22,10)-(26,10). A wire from (20,10)... would run along it.
    r.key(Key::Char('w'));
    r.click(23.0, 14.0);
    r.click(23.0, 10.0); // ends on w1's interior (T)
    r.click(24.0, 10.0);
    r.mv(25.0, 10.0);
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert!(p.error.as_deref().is_some_and(|e| e.contains("overlap wire w1")), "{p:?}");
    r.click(25.0, 10.0);
    assert!(r.has_refusal());
    // A wire doubling back over itself.
    r.key(Key::Escape);
    r.click(0.0, 30.0);
    r.click(6.0, 30.0);
    r.click(3.0, 30.0);
    assert!(r.has_refusal(), "{:?}", r.last);
    assert!(r.errors().is_empty(), "{:?}", r.errors());
}

#[test]
fn collinear_clicks_are_tidied_into_one_segment() {
    let mut r = Rig::new(View::Schematic);
    r.key(Key::Char('w'));
    r.click(0.0, 0.0);
    r.click(3.0, 0.0);
    r.click(8.0, 0.0);
    r.key(Key::Enter);
    assert_eq!(r.wire_pts("w1"), vec![[0, 0], [8, 0]]);
    assert!(!r.can_undo() || r.h.can_undo());
    r.ctrl('z');
    assert!(r.c.wires.is_empty(), "AddWire and the tidy are one undo step");
}

#[test]
fn breadboard_wire_tool_draws_free_direction_jumpers_with_end_colouring() {
    let mut r = Rig::new(View::Breadboard);
    r.place("breadboard_half", 0, 0);
    let a = hole(&r.lib, "c5");
    let b = hole(&r.lib, "c12");
    r.key(Key::Char('w'));
    r.click(a[0] as f64, a[1] as f64);
    // Diagonal preview straight to the cursor, end on a free grid point: not a hole.
    r.mv(a[0] as f64 + 40.0, a[1] as f64 + 30.0);
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert_eq!(p.pts.len(), 2);
    assert!(p.start_on_pin && !p.end_on_pin, "orange end dot");
    r.mv(b[0] as f64, b[1] as f64);
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert!(p.end_on_pin && p.error.is_none(), "green end dot");
    r.click(b[0] as f64, b[1] as f64);
    assert_eq!(r.wire_pts("j1"), vec![a, b]);
    assert_eq!(r.c.wires[0].view, View::Breadboard);
    let nl = Netlist::build(&r.c, &r.lib, View::Breadboard);
    let net = nl.net_of_wire("j1").unwrap();
    assert!(nl.nets[net].pins.iter().any(|p| p.pin == "c5") && nl.nets[net].pins.iter().any(|p| p.pin == "c12"));
    // A bend is allowed anywhere and does not end the wire (not a hole).
    let c = [a[0] + 3, a[1] - 3];
    r.click(a[0] as f64 + 1.0, a[1] as f64 - 6.0);
    r.click(c[0] as f64 + 100.0, c[1] as f64);
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_some());
    r.key(Key::Enter);
    assert_eq!(r.c.wires.len(), 2);
    assert_eq!(r.c.wires[1].id, "j2");
}

#[test]
fn breadboard_bends_off_the_board_continue_and_a_wire_end_finishes() {
    let mut r = Rig::from_json(include_str!("../../../../examples/breadboard_demo.circ"), View::Breadboard);
    r.key(Key::Char('w'));
    r.click(-20.0, -20.0);
    r.click(-10.0, -15.0); // off the board: a bend, not a terminal
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_some());
    r.click(4.0, 25.0); // j1's end point: finish
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_none());
    assert!(r.c.wires.iter().any(|w| w.id == "j3"));
}

// ---- label tool -----------------------------------------------------------------------------

#[test]
fn label_tool_prompts_on_a_pin_or_wire_and_adds_the_label() {
    let mut r = base();
    r.key(Key::Char('l'));
    assert_eq!(r.ed.tool(), Tool::Label);
    r.click(40.0, 40.0);
    assert!(r.has_refusal(), "not on a pin or wire");
    r.click(24.0, 10.0); // on w1
    assert!(r.last.contains(&Effect::PromptLabel([24, 10])));
    assert_eq!(r.ed.label_prompt(), Some([24, 10]));
    r.ev(Event::CommitLabel { name: "  SIG ".into() });
    assert_eq!(r.c.labels.len(), 2);
    assert_eq!(r.c.labels[1].name, "SIG");
    assert_eq!(r.c.labels[1].at, [24, 10]);
    assert_eq!(r.ed.selection().items, set(&[Item::Label(1)]));
    // Cancel and empty names add nothing.
    r.click(14.0, 10.0); // pin R1.a
    r.ev(Event::CancelLabel);
    r.click(14.0, 10.0);
    r.ev(Event::CommitLabel { name: "   ".into() });
    assert_eq!(r.c.labels.len(), 2);
    // Net names follow.
    let nl = Netlist::build(&r.c, &r.lib, View::Schematic);
    assert_eq!(nl.nets[nl.net_of_wire("w1").unwrap()].name.as_deref(), Some("SIG"));
    // One undo step.
    r.ctrl('z');
    assert_eq!(r.c.labels.len(), 1);
}

#[test]
fn labels_are_movable_and_deletable() {
    let mut r = base();
    r.drag((4.5, 8.2), (6.5, 8.2));
    assert_eq!(r.c.labels[0].at, [5, 9]);
    r.key(Key::Delete);
    assert!(r.c.labels.is_empty());
    r.ctrl('z');
    assert_eq!(r.c.labels[0].at, [5, 9]);
}

#[test]
fn label_tool_is_schematic_only() {
    let mut r = Rig::new(View::Breadboard);
    r.key(Key::Char('l'));
    assert_eq!(r.ed.tool(), Tool::Select);
    assert!(r.has_refusal());
    let mut s = base();
    s.key(Key::Char('l'));
    s.view = View::Breadboard;
    s.ev(Event::ViewChanged);
    assert_eq!(s.ed.tool(), Tool::Select);
}

// ---- properties -----------------------------------------------------------------------------

#[test]
fn rename_id_follows_declared_nets_and_refuses_duplicates_and_empty() {
    let mut r = base();
    r.c.nets = vec![short_circ_core::NetDecl { name: Some("X".into()), pins: vec!["R1.a".into(), "B1.pos".into()] }];
    r.click(18.0, 10.0);
    let mut ctx = r.ctx();
    assert!(props::rename_id(&mut EditorState::new(), &mut ctx, "R1", "").is_err());
    assert!(props::rename_id(&mut EditorState::new(), &mut ctx, "R1", "D1").unwrap_err().contains("already used"));
    assert!(props::rename_id(&mut EditorState::new(), &mut ctx, "R1", "w1").is_err(), "wires count too");
    drop(ctx);
    assert!(!r.can_undo(), "refusals record nothing");
    let mut ed = std::mem::take(&mut r.ed);
    {
        let mut ctx = r.ctx();
        props::rename_id(&mut ed, &mut ctx, "R1", " R9 ").unwrap();
    }
    r.ed = ed;
    assert!(r.c.part("R9").is_some() && r.c.part("R1").is_none());
    assert_eq!(r.c.nets[0].pins, vec!["R9.a".to_string(), "B1.pos".to_string()]);
    assert_eq!(r.ed.selection().items, set(&[part("R9")]), "the selection follows the rename");
    r.ctrl('z');
    assert!(r.c.part("R1").is_some());
    assert_eq!(r.c.nets[0].pins[0], "R1.a");
    // Renaming to the same id is not a step.
    let mut ed = EditorState::new();
    props::rename_id(&mut ed, &mut r.ctx(), "R1", "R1").unwrap();
    r.ctrl('y');
    assert!(r.c.part("R9").is_some(), "the earlier redo is still there: nothing was recorded");
}

#[test]
fn prop_edits_are_single_steps() {
    let mut r = base();
    props::set_prop(&mut r.ctx(), "R1", "value", Some("4k7")).unwrap();
    assert_eq!(r.part("R1").props["value"], "4k7");
    props::set_prop(&mut r.ctx(), "R1", "value", Some("4k7")).unwrap();
    props::add_prop(&mut r.ctx(), "R1", "tol", "5%").unwrap();
    assert!(props::add_prop(&mut r.ctx(), "R1", "tol", "1%").is_err());
    assert!(props::add_prop(&mut r.ctx(), "R1", "  ", "1%").is_err());
    props::rename_prop_key(&mut r.ctx(), "R1", "tol", "tolerance").unwrap();
    assert_eq!(r.part("R1").props.get("tolerance").map(String::as_str), Some("5%"));
    assert!(!r.part("R1").props.contains_key("tol"));
    props::set_prop(&mut r.ctx(), "R1", "tolerance", None).unwrap();
    assert!(!r.part("R1").props.contains_key("tolerance"));
    // value, add, rename (one step, two edits), delete = 4 steps
    for _ in 0..4 {
        assert!(r.h.undo(&mut r.c));
    }
    assert!(!r.h.can_undo());
    assert_eq!(r.part("R1").props["value"], "330");
    assert!(!r.part("R1").props.contains_key("tol"));
}

#[test]
fn colour_fields_validate_and_apply_one_step_each() {
    let mut r = base();
    props::set_part_color(&mut r.ctx(), "R1", ColorField::Color, "#FF0000").unwrap();
    props::set_part_color(&mut r.ctx(), "R1", ColorField::Fill, "blue").unwrap();
    props::set_part_color(&mut r.ctx(), "R1", ColorField::LabelColor, "none").unwrap();
    let p = r.part("R1");
    assert_eq!(
        (p.color.as_deref(), p.fill.as_deref(), p.label_color.as_deref()),
        (Some("#FF0000"), Some("blue"), Some("none"))
    );
    // Invalid input is a warning and is not applied.
    let e = props::set_part_color(&mut r.ctx(), "R1", ColorField::Color, "reddish").unwrap_err();
    assert!(e.contains("reddish") && e.contains("not a colour"), "{e}");
    assert!(props::set_part_color(&mut r.ctx(), "R1", ColorField::Color, "#12345").is_err());
    assert_eq!(r.part("R1").color.as_deref(), Some("#FF0000"));
    // `default` and empty clear the field.
    props::set_part_color(&mut r.ctx(), "R1", ColorField::Fill, "default").unwrap();
    props::set_part_color(&mut r.ctx(), "R1", ColorField::Color, "").unwrap();
    assert!(r.part("R1").fill.is_none() && r.part("R1").color.is_none());
    // wires and labels
    props::set_wire_color(&mut r.ctx(), "w1", "Teal").unwrap();
    assert_eq!(r.c.wires[0].color.as_deref(), Some("Teal"));
    assert!(props::set_wire_color(&mut r.ctx(), "w1", "x").is_err());
    props::set_label_color(&mut r.ctx(), 0, "#0f0").unwrap();
    assert_eq!(r.c.labels[0].color.as_deref(), Some("#0f0"));
    props::set_hidden_label(&mut r.ctx(), "R1", true).unwrap();
    assert!(r.part("R1").hidden_label);
    // 3 + 2 + 1 + 1 + 1 steps
    let mut steps = 0;
    while r.h.undo(&mut r.c) {
        steps += 1;
    }
    assert_eq!(steps, 8);
    assert!(r.part("R1").color.is_none() && !r.part("R1").hidden_label);
}

#[test]
fn normalize_color_forms() {
    assert_eq!(props::normalize_color("  RED ").unwrap().as_deref(), Some("RED"));
    assert_eq!(props::normalize_color("#abc").unwrap().as_deref(), Some("#abc"));
    assert_eq!(props::normalize_color("#aabbccdd").unwrap().as_deref(), Some("#aabbccdd"));
    assert_eq!(props::normalize_color("grey").unwrap().as_deref(), Some("grey"));
    assert_eq!(props::normalize_color("None").unwrap().as_deref(), Some("None"));
    assert_eq!(props::normalize_color("Default").unwrap(), None);
    assert_eq!(props::normalize_color("").unwrap(), None);
    for bad in ["#12", "#ggg", "rgb(1,2,3)", "dark red", "#1234567"] {
        assert!(props::normalize_color(bad).is_err(), "{bad}");
    }
    // Every preset name is accepted.
    for (name, _) in short_circ_core::color::preset_names() {
        assert!(props::normalize_color(name).unwrap().is_some(), "{name}");
    }
    assert_eq!(short_circ_core::color::preset_names().len(), 18);
}

#[test]
fn placement_fields_and_view_membership() {
    let mut r = Rig::from_json(include_str!("../../../../examples/breadboard_demo.circ"), View::Schematic);
    // x, y, rot, mirror through SetPlacement (rubber-bands the wire ends).
    props::set_placement(&mut r.ctx(), "R1", View::Schematic, Placement { at: [14, 12], rot: 0, mirror: false }).unwrap();
    assert_eq!(r.at("R1"), [14, 12]);
    assert_eq!(r.wire_pts("w1"), vec![[11, 10], [14, 13]]);
    assert!(props::set_placement(&mut r.ctx(), "R1", View::Schematic, Placement { at: [0, 0], rot: 45, mirror: false }).is_err());
    props::set_placement(&mut r.ctx(), "R1", View::Schematic, Placement { at: [14, 12], rot: 0, mirror: false }).unwrap();
    // Remove from this view / place in the other view.
    props::remove_from_view(&mut r.ctx(), "R1", View::Schematic).unwrap();
    assert!(r.part("R1").schematic.is_none() && r.part("R1").breadboard.is_some());
    let at = props::place_in_view(&mut r.ctx(), "R1", View::Schematic).unwrap();
    let pl = r.part("R1").schematic.clone().unwrap();
    assert_eq!((pl.at, pl.rot, pl.mirror), (at, 0, false));
    assert!(props::place_in_view(&mut r.ctx(), "R1", View::Schematic).is_err(), "already placed");
    // A free spot: nothing overlaps there.
    let spot_ok = {
        let ib = r.c.parts.iter().position(|p| p.id == "R1").unwrap();
        let (lo, hi) = hit::part_world_box(&r.c, &r.lib, View::Schematic, ib).unwrap();
        !r.c.parts.iter().enumerate().any(|(i, p)| {
            p.id != "R1"
                && hit::part_world_box(&r.c, &r.lib, View::Schematic, i)
                    .is_some_and(|(a, b)| lo[0] < b[0] && a[0] < hi[0] && lo[1] < b[1] && a[1] < hi[1])
        })
    };
    assert!(spot_ok);
    // Types without the other view cannot be placed there.
    let mut s = Rig::new(View::Schematic);
    s.place("ground", 3, 3);
    assert!(props::place_in_view(&mut s.ctx(), "GND1", View::Breadboard).unwrap_err().contains("no breadboard view"));
}

#[test]
fn free_spot_avoids_parts_but_a_part_may_sit_on_a_board() {
    let mut r = Rig::new(View::Schematic);
    r.place("resistor", 0, 0);
    r.place("led", 30, 30);
    r.ctx().apply(Edit::SetPlacement { id: "D1".into(), view: View::Schematic, placement: None }).unwrap();
    let spot = props::free_spot(&r.c, &r.lib, View::Schematic, "D1");
    assert_ne!(spot, [0, 0], "R1 occupies the origin");
    assert!(spot[0] >= 8 || spot[1] >= 2, "{spot:?}");
    // In the breadboard view the origin is free even with a board there.
    let mut b = Rig::new(View::Breadboard);
    b.place("breadboard_half", 0, 0);
    b.c.parts.push(short_circ_core::Part { id: "R1".into(), ty: "resistor".into(), ..Default::default() });
    assert_eq!(props::free_spot(&b.c, &b.lib, View::Breadboard, "R1"), [0, 0]);
}

#[test]
fn label_rename_keeps_order_and_index() {
    let mut r = base();
    r.c.labels.push(short_circ_core::Label { at: [22, 10], name: "B".into(), color: Some("red".into()) });
    r.c.labels.push(short_circ_core::Label { at: [26, 10], name: "C".into(), color: None });
    props::rename_label(&mut r.ctx(), 0, "GND").unwrap();
    let names: Vec<&str> = r.c.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["GND", "B", "C"]);
    assert_eq!(r.c.labels[1].color.as_deref(), Some("red"));
    assert!(props::rename_label(&mut r.ctx(), 0, " ").is_err());
    r.ctrl('z');
    assert_eq!(r.c.labels[0].name, "VCC", "one undo step");
}

#[test]
fn title_and_declared_nets_editor() {
    let mut r = base();
    props::set_title(&mut r.ctx(), " My circuit ").unwrap();
    assert_eq!(r.c.title.as_deref(), Some("My circuit"));
    props::set_title(&mut r.ctx(), "My circuit").unwrap();
    props::set_title(&mut r.ctx(), "").unwrap();
    assert!(r.c.title.is_none());
    // Declare nets from the schematic: w1 joins R1.b and D1.anode; w2 + label VCC join B1.pos and R1.a.
    let n = props::declare_nets_from_view(&mut r.ctx()).unwrap();
    assert_eq!(n, 2, "{:?}", r.c.nets);
    let named = r.c.nets.iter().find(|n| n.name.as_deref() == Some("VCC")).expect("label name used");
    let mut got = named.pins.clone();
    got.sort();
    assert_eq!(got, vec!["B1.pos".to_string(), "R1.a".to_string()]);
    let anon = r.c.nets.iter().find(|n| n.name.is_none()).unwrap();
    assert_eq!(anon.pins, vec!["R1.b".to_string(), "D1.anode".to_string()]);
    // Deleting one, clearing.
    props::delete_declared_net(&mut r.ctx(), 0).unwrap();
    assert_eq!(r.c.nets.len(), 1);
    assert!(props::delete_declared_net(&mut r.ctx(), 5).is_err());
    props::clear_declared_nets(&mut r.ctx()).unwrap();
    assert!(r.c.nets.is_empty());
    props::clear_declared_nets(&mut r.ctx()).unwrap();
    // title(2) + declare + delete + clear = the steps that changed something
    let mut steps = 0;
    while r.h.undo(&mut r.c) {
        steps += 1;
    }
    assert_eq!(steps, 5);
}

#[test]
fn declared_nets_from_the_breadboard_skip_board_holes_and_single_pin_nets() {
    let r = Rig::from_json(include_str!("../../../../examples/breadboard_demo.circ"), View::Breadboard);
    let nets = props::nets_from_view(&r.c, &r.lib, View::Breadboard);
    for n in &nets {
        assert!(n.pins.len() >= 2);
        assert!(n.pins.iter().all(|p| !p.starts_with("BB1.")), "{n:?}");
        assert!(n.name.is_none());
    }
    assert!(!nets.is_empty());
}

// ---- undo / redo, dirty, save ---------------------------------------------------------------

#[test]
fn undo_and_redo_keys_and_dirty_tracking() {
    let mut r = base();
    assert!(!r.h.dirty());
    r.click(18.0, 10.0);
    r.key(Key::Right);
    assert!(r.h.dirty());
    r.ev(Event::Key { key: Key::Char('z'), mods: Mods::CTRL });
    assert!(!r.h.dirty(), "back at the saved state");
    r.ev(Event::Key { key: Key::Char('z'), mods: Mods { ctrl: true, shift: true, alt: false } });
    assert_eq!(r.at("R1"), [15, 9], "Ctrl+Shift+Z redoes");
    assert!(r.h.dirty());
    r.ctrl('z');
    r.ctrl('y');
    assert_eq!(r.at("R1"), [15, 9], "Ctrl+Y redoes");
    r.h.mark_clean();
    assert!(!r.h.dirty());
    r.ctrl('z');
    assert!(r.h.dirty());
    let fx = r.ctrl('z');
    assert!(fx.contains(&Effect::Status("Nothing to undo".into())));
    r.cmd(Command::Redo);
    assert!(!r.h.dirty());
}

#[test]
fn selection_is_pruned_after_undo() {
    let mut r = Rig::new(View::Schematic);
    r.place("resistor", 5, 5);
    assert_eq!(r.ed.selection().items, set(&[part("R1")]));
    r.ctrl('z');
    assert!(r.ed.selection().is_empty());
}

#[test]
fn undo_while_drawing_a_wire_takes_back_the_last_vertex() {
    let mut r = Rig::new(View::Schematic);
    r.key(Key::Char('w'));
    r.click(0.0, 0.0);
    r.click(5.0, 0.0);
    r.click(5.0, 5.0);
    r.ctrl('z');
    let p = r.ed.wire_preview(&r.c, &r.lib, r.view).unwrap();
    assert!(!p.pts.contains(&[5, 5]) || p.pts.last() == Some(&[5, 5]));
    assert!(!r.can_undo());
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("short-circ-editor-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn save_and_reload_round_trip_is_byte_identical() {
    let mut r = Rig::new(View::Schematic);
    r.place("battery", 10, 10);
    r.place("resistor", 14, 9);
    r.wire(&[(11, 10), (14, 10)]);
    r.key(Key::Char('l'));
    r.click(12.0, 10.0);
    r.ev(Event::CommitLabel { name: "VCC".into() });
    props::set_title(&mut r.ctx(), "Round trip").unwrap();
    assert!(r.h.dirty());
    let dir = tmp_dir("roundtrip");
    let path = dir.join("t.circ");
    doc::save_circuit(&path, &r.c).unwrap();
    r.h.mark_clean();
    assert!(!r.h.dirty());
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(first.ends_with('\n') && first.contains("\"format\": \"short-circ/2\""), "{first}");
    let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n != "t.circ").collect();
    assert!(leftovers.is_empty(), "temp file renamed away: {leftovers:?}");
    // Load through parse + migrate, run history/load rules, save again: identical bytes.
    let mut loaded = crate::core_api::load_circuit(&path).unwrap();
    let mut h = History::new();
    h.clear();
    h.mark_clean();
    assert!(!h.dirty());
    let again = dir.join("t2.circ");
    doc::save_circuit(&again, &loaded).unwrap();
    assert_eq!(std::fs::read_to_string(&again).unwrap(), first);
    // And an edit after loading makes it dirty again, an undo cleans it.
    let lib = Library::load(&loaded, None);
    h.apply(&mut loaded, &lib, Edit::SetTitle(Some("x".into()))).unwrap();
    assert!(h.dirty());
    h.undo(&mut loaded);
    assert!(!h.dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_new_circuit_is_current_format_and_saves_cleanly() {
    let c = doc::new_circuit();
    assert_eq!(c.format, short_circ_core::FORMAT);
    assert!(c.parts.is_empty() && c.wires.is_empty());
    let text = short_circ_core::write::circuit_to_string(&c);
    let back = crate::core_api::parse_circuit_text(&text).unwrap();
    assert_eq!(short_circ_core::write::circuit_to_string(&back), text);
    assert_eq!(doc::window_title(None, true), "short-circ - untitled*");
    assert_eq!(doc::window_title(Some(std::path::Path::new("a/b/led.circ")), false), "short-circ - led.circ");
}

#[test]
fn saving_over_an_existing_file_replaces_it() {
    let dir = tmp_dir("replace");
    let path = dir.join("a.circ");
    std::fs::write(&path, "old").unwrap();
    doc::save_circuit(&path, &doc::new_circuit()).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("short-circ/2"));
    assert!(doc::save_circuit(&dir.join("missing-dir").join("a.circ"), &doc::new_circuit()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- drawing a circuit from scratch ---------------------------------------------------------

fn expected_led_nets() -> BTreeSet<Vec<String>> {
    [vec!["B1.pos", "R1.a"], vec!["D1.anode", "R1.b"], vec!["B1.neg", "D1.cathode"]]
        .into_iter()
        .map(|v| {
            let mut v: Vec<String> = v.into_iter().map(String::from).collect();
            v.sort();
            v
        })
        .collect()
}

#[test]
fn drawing_an_led_circuit_from_scratch_in_the_schematic() {
    let mut r = Rig::new(View::Schematic);
    let mut steps = 0;
    let mut check = |r: &mut Rig, what: &str| {
        let errs = r.errors();
        assert!(errs.is_empty(), "after {what}: {errs:?}");
        steps += 1;
        let _ = steps;
    };
    assert_eq!(r.place("battery", 10, 10), "B1");
    check(&mut r, "battery");
    assert_eq!(r.place("resistor", 14, 9), "R1");
    check(&mut r, "resistor");
    assert_eq!(r.place("led", 26, 9), "D1");
    check(&mut r, "led");
    // B1 pos (11,10) -> R1.a (14,10); R1.b (22,10) -> D1.anode (26,10); D1.cathode (30,10) -> down -> B1.neg (11,14)
    r.wire(&[(11, 10), (14, 10)]);
    check(&mut r, "wire 1");
    r.wire(&[(22, 10), (26, 10)]);
    check(&mut r, "wire 2");
    r.wire(&[(30, 10), (30, 14), (11, 14)]);
    check(&mut r, "wire 3");
    assert_eq!(r.nets(View::Schematic), expected_led_nets());
    let report = short_circ_core::validate(&r.c, &r.lib);
    assert!(report.ok, "{:?}", report.issues);
    assert!(report.issues.iter().all(|i| i.rule != "dangling-wire" && i.rule != "unconnected-pin"), "{:?}", report.issues);
    // 3 placements + 3 wires = 6 undo steps, then redo to the same circuit.
    let done = r.fingerprint();
    let mut n = 0;
    while r.h.undo(&mut r.c) {
        n += 1;
    }
    assert_eq!(n, 6);
    assert!(r.c.parts.is_empty() && r.c.wires.is_empty());
    while r.h.redo(&mut r.c) {}
    assert_eq!(r.fingerprint(), done);
}

#[test]
fn drawing_the_same_circuit_on_a_breadboard_gives_the_same_nets() {
    let mut r = Rig::new(View::Breadboard);
    let board = r.place("breadboard_half", 0, 0);
    assert_eq!(board, "BB1");
    let cell = |r: &Rig, id: &str| hole(&r.lib, id);
    // Legs in row c: R1 a/b in columns 5/9, D1 anode/cathode in 12/13, B1 pos/neg in 18/20.
    let place_at = |r: &mut Rig, ty: &str, pin: &str, hole_id: &str| -> String {
        let h = cell(r, hole_id);
        let l = pin_local(&r.lib, ty, View::Breadboard, pin);
        r.place(ty, h[0] - l[0], h[1] - l[1])
    };
    assert_eq!(place_at(&mut r, "battery", "pos", "c18"), "B1");
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    assert_eq!(place_at(&mut r, "resistor", "a", "c5"), "R1");
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    assert_eq!(place_at(&mut r, "led", "anode", "c12"), "D1");
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    // The legs really are in holes: neg is 2 away from pos, cathode 1 from anode, b 4 from a.
    for (a, b) in [("c18", "c20"), ("c5", "c9"), ("c12", "c13")] {
        let d = [cell(&r, b)[0] - cell(&r, a)[0], cell(&r, b)[1] - cell(&r, a)[1]];
        assert_eq!(d[1], 0, "same row");
        assert!(d[0] > 0);
    }
    // Jumpers between strips (row d is in the same strip as row c of its column).
    let jump = |r: &mut Rig, a: &str, b: &str| {
        let (pa, pb) = (hole(&r.lib, a), hole(&r.lib, b));
        r.wire(&[(pa[0], pa[1]), (pb[0], pb[1])]);
    };
    jump(&mut r, "d5", "d18"); // R1.a - B1.pos
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    jump(&mut r, "d9", "d12"); // R1.b - D1.anode
    jump(&mut r, "d13", "d20"); // D1.cathode - B1.neg
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    assert_eq!(r.nets(View::Breadboard), expected_led_nets());
    assert_eq!(r.c.wires.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["j1", "j2", "j3"]);
    // The same circuit drawn in the schematic has the same nets.
    let mut s = Rig::new(View::Schematic);
    s.place("battery", 10, 10);
    s.place("resistor", 14, 9);
    s.place("led", 26, 9);
    s.wire(&[(11, 10), (14, 10)]);
    s.wire(&[(22, 10), (26, 10)]);
    s.wire(&[(30, 10), (30, 14), (11, 14)]);
    assert_eq!(s.nets(View::Schematic), r.nets(View::Breadboard));
    // Undo takes the whole thing apart again: 4 placements + 3 jumpers.
    let mut n = 0;
    while r.h.undo(&mut r.c) {
        n += 1;
    }
    assert_eq!(n, 7);
    assert!(r.c.parts.is_empty());
}

#[test]
fn every_edit_in_a_mixed_session_keeps_the_circuit_free_of_new_errors() {
    // Start from a valid circuit and run a mixed session; after every step the error list may
    // not contain anything the start did not.
    let mut r = Rig::from_json(include_str!("../../../../examples/led.circ"), View::Schematic);
    let base_errors = r.errors();
    assert!(base_errors.is_empty(), "{base_errors:?}");
    let ids: Vec<String> = r.c.parts.iter().map(|p| p.id.clone()).collect();
    assert!(ids.len() >= 3);
    r.ctrl('a');
    r.key(Key::Right);
    r.key(Key::Down);
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    r.cmd(Command::Tidy);
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    r.key(Key::Delete);
    for _ in 0..5 {
        r.ctrl('z');
        assert!(r.errors().is_empty(), "{:?}", r.errors());
    }
}

// ---- misc -----------------------------------------------------------------------------------

#[test]
fn escape_clears_the_selection_in_the_select_tool() {
    let mut r = base();
    r.click(18.0, 10.0);
    r.key(Key::Escape);
    assert!(r.ed.selection().is_empty());
}

#[test]
fn tool_keys_and_set_tool() {
    let mut r = base();
    r.key(Key::Char('w'));
    assert_eq!(r.ed.tool(), Tool::Wire);
    r.key(Key::Char('l'));
    assert_eq!(r.ed.tool(), Tool::Label);
    r.key(Key::Char('s'));
    assert_eq!(r.ed.tool(), Tool::Select);
    r.ev(Event::SetTool(Tool::Wire));
    assert_eq!(r.ed.tool(), Tool::Wire);
    r.ev(Event::SetTool(Tool::Select));
    assert_eq!(r.ed.tool(), Tool::Select);
    assert!(r.ed.hint(View::Schematic).contains("click"));
    for t in [Tool::Select, Tool::Wire, Tool::Label, Tool::Place] {
        r.ed.tool = t;
        assert!(!r.ed.hint(View::Breadboard).is_empty());
    }
}

#[test]
fn right_click_in_the_wire_tool_cancels_then_leaves_the_tool() {
    let mut r = Rig::new(View::Schematic);
    r.key(Key::Char('w'));
    r.click(0.0, 0.0);
    r.ev(Event::PointerDown { pos: [5.0, 0.0], button: Button::Secondary, mods: Mods::NONE });
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_none());
    assert_eq!(r.ed.tool(), Tool::Wire);
    r.ev(Event::PointerDown { pos: [5.0, 0.0], button: Button::Secondary, mods: Mods::NONE });
    assert_eq!(r.ed.tool(), Tool::Select);
}

#[test]
fn board_interior_is_empty_space_for_marquee_but_click_selects_and_rim_drags() {
    let mut r = Rig::new(View::Breadboard);
    r.place("breadboard_half", 0, 0);
    let h = hole(&r.lib, "e10");
    // A click inside the board selects it (on release, as a click).
    r.click(h[0] as f64, h[1] as f64);
    assert_eq!(r.ed.selection().items, set(&[part("BB1")]));
    r.click(-40.0, -40.0);
    assert!(r.ed.selection().is_empty());
    // A drag from inside the board is a marquee, nothing moves.
    let before = r.fingerprint();
    r.drag((h[0] as f64, h[1] as f64), (h[0] as f64 + 3.0, h[1] as f64 + 3.0));
    assert_eq!(r.fingerprint(), before);
    // The frame of the board grabs it.
    let vd = r.lib.get("breadboard_half").unwrap().breadboard.clone().unwrap();
    let rim = [vd.size[0] / 2.0, 0.2];
    r.drag((rim[0], rim[1]), (rim[0], rim[1] + 3.0));
    assert_eq!(r.at("BB1"), [0, 3]);
}

#[test]
fn hit_testing_prefers_labels_then_pins_then_wires_then_parts() {
    let r = base();
    let tol = HitTol::default();
    let v = View::Schematic;
    assert_eq!(hit::hit_item(&r.c, &r.lib, v, [4.5, 8.2], tol), Some(Item::Label(0)));
    assert_eq!(hit::hit_item(&r.c, &r.lib, v, [14.0, 10.0], tol), Some(part("R1")), "pin beats the wire end");
    assert_eq!(hit::hit_item(&r.c, &r.lib, v, [24.0, 10.0], tol), Some(wire_item("w1")));
    assert_eq!(hit::hit_item(&r.c, &r.lib, v, [18.0, 10.5], tol), Some(part("R1")));
    assert_eq!(hit::hit_item(&r.c, &r.lib, v, [60.0, 60.0], tol), None);
    // Labels do not exist in the breadboard view.
    assert_eq!(hit::label_hit(&r.c, [60.0, 60.0]), None);
}

#[test]
fn reset_and_cancel_all_forget_everything_in_progress() {
    let mut r = base();
    r.key(Key::Char('w'));
    r.click(0.0, 0.0);
    {
        let mut ctx = EditCtx::new(&mut r.c, &mut r.h, &r.lib, View::Schematic);
        r.ed.cancel_all(&mut ctx);
    }
    assert_eq!(r.ed.tool(), Tool::Select);
    assert!(r.ed.wire_preview(&r.c, &r.lib, r.view).is_none());
    r.click(18.0, 10.0);
    assert!(!r.ed.selection().is_empty());
    r.ed.reset();
    assert!(r.ed.selection().is_empty() && !r.ed.has_clipboard());
}
