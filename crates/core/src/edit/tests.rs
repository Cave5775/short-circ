use super::*;
use crate::model::{Dir, PartDef, PinDef, PinPos, ViewDef};
use std::collections::BTreeMap;

struct Lib(BTreeMap<String, PartDef>);

impl PartLookup for Lib {
    fn get(&self, ty: &str) -> Option<&PartDef> {
        self.0.get(ty)
    }
}

fn view(size: [f64; 2], pins: &[(&str, Pt)]) -> ViewDef {
    ViewDef {
        size,
        pins: pins.iter().map(|(id, at)| (id.to_string(), PinPos { at: *at, dir: Dir::Left })).collect(),
        ..ViewDef::default()
    }
}

fn lib() -> Lib {
    let mut m = BTreeMap::new();
    m.insert(
        "res".to_string(),
        PartDef {
            format: crate::model::FORMAT.into(),
            ty: "res".into(),
            pins: vec![PinDef { id: "a".into(), name: None }, PinDef { id: "b".into(), name: None }],
            schematic: Some(view([8.0, 2.0], &[("a", [0, 1]), ("b", [8, 1])])),
            breadboard: Some(view([4.0, 1.0], &[("a", [0, 0]), ("b", [4, 0])])),
            ..PartDef::default()
        },
    );
    Lib(m)
}

fn part(id: &str, sch: Option<Pt>, bb: Option<Pt>) -> Part {
    Part {
        id: id.into(),
        ty: "res".into(),
        schematic: sch.map(|a| Placement::at(a[0], a[1])),
        breadboard: bb.map(|a| Placement::at(a[0], a[1])),
        ..Part::default()
    }
}

fn wire(id: &str, view: View, pts: &[Pt]) -> Wire {
    Wire { id: id.into(), view, pts: pts.to_vec(), color: None }
}

fn circuit() -> Circuit {
    Circuit {
        format: crate::model::FORMAT.into(),
        parts: vec![part("R1", Some([10, 10]), Some([10, 10])), part("R2", Some([30, 10]), Some([30, 10]))],
        wires: vec![
            // R1.b (18,11) -> R2.a (30,11) with a bend, schematic
            wire("w1", View::Schematic, &[[18, 11], [24, 11], [24, 11 + 0], [30, 11]]),
            // a wire ending at R1.a (10,11) with an interior vertex on R1.b's position (18,11)
            wire("w2", View::Schematic, &[[10, 11], [18, 11], [18, 20]]),
            // breadboard jumper from R1.b (14,10) to somewhere
            wire("j1", View::Breadboard, &[[14, 10], [14, 3]]),
            // breadboard wire sitting where R1.a is in schematic (10,11): must not move
            wire("j2", View::Breadboard, &[[10, 11], [12, 11]]),
        ],
        ..Circuit::default()
    }
}

fn pts(c: &Circuit, id: &str) -> Vec<Pt> {
    c.wires.iter().find(|w| w.id == id).unwrap().pts.clone()
}

fn ctext(c: &Circuit) -> String {
    crate::write::circuit_to_string(c)
}

/// Apply `edit` through History and check undo/redo restore exact states.
fn roundtrip(c0: &Circuit, edit: Edit) -> Circuit {
    let l = lib();
    let mut c = c0.clone();
    let mut h = History::new();
    h.apply(&mut c, &l, edit).expect("applies");
    let after = ctext(&c);
    assert_ne!(after, ctext(c0), "edit changed nothing");
    assert!(h.can_undo() && !h.can_redo() && h.dirty());
    assert!(h.undo(&mut c));
    assert_eq!(ctext(&c), ctext(c0), "undo restores");
    assert!(!h.dirty() && h.can_redo());
    assert!(h.redo(&mut c));
    assert_eq!(ctext(&c), after, "redo restores");
    c
}

#[test]
fn fresh_id_picks_smallest_unused() {
    let c = circuit();
    assert_eq!(fresh_id(&c, "R"), "R3");
    assert_eq!(fresh_id(&c, "w"), "w3");
    assert_eq!(fresh_id(&c, "j"), "j3");
    assert_eq!(fresh_id(&c, "C"), "C1");
    let mut c = c;
    c.parts.retain(|p| p.id != "R1");
    assert_eq!(fresh_id(&c, "R"), "R1");
}

#[test]
fn add_and_remove_part() {
    let c0 = circuit();
    let c = roundtrip(&c0, Edit::AddPart(part("R3", Some([1, 1]), None)));
    assert_eq!(c.parts.len(), 3);
    let l = lib();
    let mut c = c0.clone();
    assert_eq!(apply(&mut c, &l, Edit::AddPart(part("R1", None, None))), Err(EditError::DuplicateId("R1".into())));
    // wire ids clash too
    assert!(apply(&mut c, &l, Edit::AddPart(part("w1", None, None))).is_err());
    assert!(apply(&mut c, &l, Edit::AddPart(part("", None, None))).is_err());
    assert_eq!(ctext(&c), ctext(&c0));
    let c = roundtrip(&c0, Edit::RemovePart { id: "R2".into() });
    assert_eq!(c.parts.len(), 1);
    assert_eq!(c.wires.len(), 4, "wires stay");
    assert!(matches!(apply(&mut c.clone(), &l, Edit::RemovePart { id: "zz".into() }), Err(EditError::UnknownPart(_))));
}

fn net(name: &str, pins: &[&str]) -> crate::model::NetDecl {
    crate::model::NetDecl { name: Some(name.into()), pins: pins.iter().map(|s| s.to_string()).collect() }
}

#[test]
fn remove_part_strips_declared_nets_and_undo_restores_them() {
    let l = lib();
    let mut c0 = circuit();
    c0.nets = vec![net("VCC", &["R1.a", "R2.a"]), net("ONLY", &["R2.b"]), net("GND", &["R1.b", "R2.b"]), net("EMPTY", &[])];
    let bad_nets = |c: &Circuit| crate::validate(c, &l).issues.iter().filter(|i| i.rule == "bad-net").count();
    assert_eq!(bad_nets(&c0), 0);
    // Regression: R2 stayed in `nets` and every reference to it became a `bad-net` error.
    let c = roundtrip(&c0, Edit::RemovePart { id: "R2".into() });
    assert_eq!(bad_nets(&c), 0);
    let shape: Vec<(String, Vec<String>)> = c.nets.iter().map(|n| (n.name.clone().unwrap(), n.pins.clone())).collect();
    assert_eq!(
        shape,
        vec![
            ("VCC".to_string(), vec!["R1.a".to_string()]),
            ("GND".to_string(), vec!["R1.b".to_string()]),
            ("EMPTY".to_string(), vec![]),
        ],
        "ONLY lost its last pin and is dropped; a net that was already empty stays"
    );
    // Through a grouped History as the editor does it: one undo brings everything back.
    let mut c = c0.clone();
    let mut h = History::new();
    h.begin_group();
    h.apply(&mut c, &l, Edit::RemovePart { id: "R1".into() }).unwrap();
    h.apply(&mut c, &l, Edit::RemovePart { id: "R2".into() }).unwrap();
    h.end_group();
    assert!(c.nets.iter().all(|n| n.name.as_deref() == Some("EMPTY")));
    assert!(h.undo(&mut c));
    assert_eq!(ctext(&c), ctext(&c0));
}

#[test]
fn remove_part_keeps_references_owned_by_a_dotted_part_id() {
    let l = lib();
    let mut c = circuit();
    c.parts.push(part("R1.x", Some([50, 10]), None));
    c.nets = vec![net("N", &["R1.a", "R1.x.a"])];
    apply(&mut c, &l, Edit::RemovePart { id: "R1".into() }).unwrap();
    assert_eq!(c.nets[0].pins, vec!["R1.x.a".to_string()]);
    assert_eq!(crate::validate(&c, &l).issues.iter().filter(|i| i.rule == "bad-net").count(), 0);
}

#[test]
fn set_placement_some_and_none() {
    let c0 = circuit();
    let c = roundtrip(
        &c0,
        Edit::SetPlacement { id: "R1".into(), view: View::Breadboard, placement: None },
    );
    assert!(c.part("R1").unwrap().breadboard.is_none());
    assert!(c.part("R1").unwrap().schematic.is_some());
    let c = roundtrip(
        &c0,
        Edit::SetPlacement { id: "R2".into(), view: View::Breadboard, placement: Some(Placement { at: [5, 5], rot: 270, mirror: true }) },
    );
    assert_eq!(c.part("R2").unwrap().breadboard, Some(Placement { at: [5, 5], rot: 270, mirror: true }));
    let l = lib();
    let mut c = c0.clone();
    let bad = Edit::SetPlacement { id: "R2".into(), view: View::Schematic, placement: Some(Placement { at: [0, 0], rot: 45, mirror: false }) };
    assert_eq!(apply(&mut c, &l, bad), Err(EditError::BadRotation(45)));
    // placing a part that had no placement moves no wire
    let mut c = c0.clone();
    c.parts.push(part("R3", None, None));
    apply(&mut c, &l, Edit::SetPlacement { id: "R3".into(), view: View::Schematic, placement: Some(Placement::at(10, 10)) }).unwrap();
    assert_eq!(pts(&c, "w2"), pts(&c0, "w2"));
}

#[test]
fn move_part_rubber_bands_endpoints_on_pins_only() {
    let c0 = circuit();
    let c = roundtrip(&c0, Edit::MovePart { id: "R1".into(), view: View::Schematic, to: [10, 14] });
    // R1 moved by (0,+4): w1 first end (R1.b) follows, its far end (R2.a) stays
    assert_eq!(pts(&c, "w1"), vec![[18, 15], [24, 11], [24, 11], [30, 11]]);
    // w2 starts on R1.a (follows); its interior vertex at R1.b's old spot does NOT move
    assert_eq!(pts(&c, "w2"), vec![[10, 15], [18, 11], [18, 20]]);
    // other view untouched, even a wire that sits on a schematic pin coordinate
    assert_eq!(pts(&c, "j1"), vec![[14, 10], [14, 3]]);
    assert_eq!(pts(&c, "j2"), vec![[10, 11], [12, 11]]);
    assert_eq!(c.part("R1").unwrap().breadboard, c0.part("R1").unwrap().breadboard);
}

#[test]
fn move_in_breadboard_follows_only_breadboard_wires() {
    let c0 = circuit();
    let c = roundtrip(&c0, Edit::MovePart { id: "R1".into(), view: View::Breadboard, to: [20, 20] });
    assert_eq!(pts(&c, "j1"), vec![[24, 20], [14, 3]]);
    assert_eq!(pts(&c, "j2"), vec![[10, 11], [12, 11]]);
    assert_eq!(pts(&c, "w1"), pts(&c0, "w1"));
    assert_eq!(pts(&c, "w2"), pts(&c0, "w2"));
}

#[test]
fn move_by_moves_both_ends_of_wires_between_parts() {
    let c0 = circuit();
    let c = roundtrip(&c0, Edit::MoveBy { ids: vec!["R1".into(), "R2".into(), "R1".into()], view: View::Schematic, delta: [2, -3] });
    assert_eq!(c.part("R1").unwrap().schematic.as_ref().unwrap().at, [12, 7]);
    assert_eq!(c.part("R2").unwrap().schematic.as_ref().unwrap().at, [32, 7]);
    // both ends of w1 moved, interior bends did not
    assert_eq!(pts(&c, "w1"), vec![[20, 8], [24, 11], [24, 11], [32, 8]]);
    // an unplaced id fails atomically
    let l = lib();
    let mut c = c0.clone();
    c.parts[1].schematic = None;
    let before = ctext(&c);
    let r = apply(&mut c, &l, Edit::MoveBy { ids: vec!["R1".into(), "R2".into()], view: View::Schematic, delta: [1, 1] });
    assert!(matches!(r, Err(EditError::NotPlaced { .. })));
    assert_eq!(ctext(&c), before);
}

#[test]
fn rotate_and_mirror_rubber_band() {
    let c0 = circuit();
    // rot 90 about the origin (10,10): R1.a (0,1) -> (-1,0) => (9,10); R1.b (8,1) -> (-1,8) => (9,18)
    let c = roundtrip(&c0, Edit::Rotate { id: "R1".into(), view: View::Schematic, degrees: 90 });
    assert_eq!(c.part("R1").unwrap().schematic.as_ref().unwrap().rot, 90);
    assert_eq!(pts(&c, "w1")[0], [9, 18]);
    assert_eq!(pts(&c, "w2")[0], [9, 10]);
    let c = roundtrip(&c0, Edit::Rotate { id: "R1".into(), view: View::Schematic, degrees: -90 });
    assert_eq!(c.part("R1").unwrap().schematic.as_ref().unwrap().rot, 270);
    let l = lib();
    let mut c2 = c.clone();
    apply(&mut c2, &l, Edit::Rotate { id: "R1".into(), view: View::Schematic, degrees: 180 }).unwrap();
    assert_eq!(c2.part("R1").unwrap().schematic.as_ref().unwrap().rot, 90);
    assert!(matches!(apply(&mut c2, &l, Edit::Rotate { id: "R1".into(), view: View::Schematic, degrees: 45 }), Err(EditError::BadRotation(45))));
    // mirror: x negated, R1.b (8,1) -> (-8,1) => (2,11)
    let c = roundtrip(&c0, Edit::Mirror { id: "R1".into(), view: View::Schematic });
    assert!(c.part("R1").unwrap().schematic.as_ref().unwrap().mirror);
    assert_eq!(pts(&c, "w1")[0], [2, 11]);
    assert_eq!(pts(&c, "w2")[0], [10, 11], "R1.a is on the axis");
    // mirror twice is identity
    let mut c3 = c.clone();
    apply(&mut c3, &l, Edit::Mirror { id: "R1".into(), view: View::Schematic }).unwrap();
    assert_eq!(pts(&c3, "w1")[0], [18, 11]);
    assert!(!c3.part("R1").unwrap().schematic.as_ref().unwrap().mirror);
}

#[test]
fn unknown_part_type_moves_without_rubber_banding() {
    let mut c = circuit();
    c.parts[0].ty = "mystery".into();
    let l = lib();
    apply(&mut c, &l, Edit::MovePart { id: "R1".into(), view: View::Schematic, to: [0, 0] }).unwrap();
    assert_eq!(pts(&c, "w2")[0], [10, 11]);
}

#[test]
fn props_colors_hidden_title() {
    let c0 = circuit();
    let c = roundtrip(&c0, Edit::SetProp { id: "R1".into(), key: "value".into(), value: Some("1k".into()) });
    assert_eq!(c.part("R1").unwrap().props["value"], "1k");
    let l = lib();
    let mut c = c;
    apply(&mut c, &l, Edit::SetProp { id: "R1".into(), key: "value".into(), value: None }).unwrap();
    assert!(c.part("R1").unwrap().props.is_empty());
    let c = roundtrip(
        &c0,
        Edit::SetColors { id: "R2".into(), color: Some("red".into()), fill: None, label_color: Some("#fff".into()) },
    );
    let p = c.part("R2").unwrap();
    assert_eq!((p.color.as_deref(), p.fill.as_deref(), p.label_color.as_deref()), (Some("red"), None, Some("#fff")));
    let c = roundtrip(&c0, Edit::SetHiddenLabel { id: "R2".into(), hidden: true });
    assert!(c.part("R2").unwrap().hidden_label);
    let c = roundtrip(&c0, Edit::SetTitle(Some("hello".into())));
    assert_eq!(c.title.as_deref(), Some("hello"));
    let c = roundtrip(&c, Edit::SetTitle(None));
    assert_eq!(c.title, None);
}

#[test]
fn wire_commands() {
    let c0 = circuit();
    let c = roundtrip(&c0, Edit::AddWire(wire("w9", View::Schematic, &[[0, 0], [5, 0]])));
    assert_eq!(c.wires.len(), 5);
    let l = lib();
    let mut cc = c0.clone();
    assert!(matches!(apply(&mut cc, &l, Edit::AddWire(wire("w1", View::Schematic, &[[0, 0], [1, 0]]))), Err(EditError::DuplicateId(_))));
    assert!(matches!(apply(&mut cc, &l, Edit::AddWire(wire("R1", View::Schematic, &[[0, 0], [1, 0]]))), Err(EditError::DuplicateId(_))));
    assert!(matches!(apply(&mut cc, &l, Edit::AddWire(wire("q", View::Schematic, &[[0, 0]]))), Err(EditError::TooFewPoints(_))));
    let c = roundtrip(&c0, Edit::RemoveWire { id: "w1".into() });
    assert_eq!(c.wires.len(), 3);
    let c = roundtrip(&c0, Edit::SetWirePts { id: "w2".into(), pts: vec![[0, 0], [0, 9]] });
    assert_eq!(pts(&c, "w2"), vec![[0, 0], [0, 9]]);
    assert!(matches!(apply(&mut cc, &l, Edit::SetWirePts { id: "w2".into(), pts: vec![[0, 0]] }), Err(EditError::TooFewPoints(_))));
    let c = roundtrip(&c0, Edit::MoveWirePoint { id: "w2".into(), index: 1, to: [18, 15] });
    assert_eq!(pts(&c, "w2")[1], [18, 15]);
    assert!(matches!(apply(&mut cc, &l, Edit::MoveWirePoint { id: "w2".into(), index: 3, to: [0, 0] }), Err(EditError::BadIndex { .. })));
    let c = roundtrip(&c0, Edit::SetWireColor { id: "w2".into(), color: Some("blue".into()) });
    assert_eq!(c.wires[1].color.as_deref(), Some("blue"));
    assert!(matches!(apply(&mut cc, &l, Edit::RemoveWire { id: "nope".into() }), Err(EditError::UnknownWire(_))));
}

#[test]
fn split_wire_on_segment_and_at_vertex() {
    let mut c0 = circuit();
    c0.wires[0].color = Some("red".into());
    // on a segment (the 18..24 run at y=11)
    let c = roundtrip(&c0, Edit::SplitWire { id: "w1".into(), at: [21, 11] });
    assert_eq!(c.wires.len(), 5);
    assert_eq!(c.wires[0].id, "w1");
    assert_eq!(c.wires[1].id, "w3", "fresh id, placed right after");
    assert_eq!(c.wires[0].pts, vec![[18, 11], [21, 11]]);
    assert_eq!(c.wires[1].pts, vec![[21, 11], [24, 11], [24, 11], [30, 11]]);
    assert_eq!(c.wires[1].color.as_deref(), Some("red"));
    assert_eq!(c.wires[1].view, View::Schematic);
    // at an interior vertex
    let c = roundtrip(&c0, Edit::SplitWire { id: "w2".into(), at: [18, 11] });
    assert_eq!(c.wires[0].id, "w1");
    assert_eq!(c.wires[1].pts, vec![[10, 11], [18, 11]]);
    assert_eq!(c.wires[2].pts, vec![[18, 11], [18, 20]]);
    assert_ne!(c.wires[1].id, c.wires[2].id);
    // diagonal segment, integer point on it
    let mut d = Circuit { wires: vec![wire("d", View::Breadboard, &[[0, 0], [4, 2]])], ..Circuit::default() };
    let l = lib();
    apply(&mut d, &l, Edit::SplitWire { id: "d".into(), at: [2, 1] }).unwrap();
    assert_eq!(d.wires.len(), 2);
    assert_eq!(d.wires[1].view, View::Breadboard);
    // errors leave the circuit alone
    let mut cc = c0.clone();
    let before = ctext(&cc);
    assert!(matches!(apply(&mut cc, &l, Edit::SplitWire { id: "w1".into(), at: [18, 11] }), Err(EditError::AtEndpoint { .. })));
    assert!(matches!(apply(&mut cc, &l, Edit::SplitWire { id: "w1".into(), at: [30, 11] }), Err(EditError::AtEndpoint { .. })));
    assert!(matches!(apply(&mut cc, &l, Edit::SplitWire { id: "w1".into(), at: [21, 12] }), Err(EditError::NotOnWire { .. })));
    assert!(matches!(apply(&mut cc, &l, Edit::SplitWire { id: "w1".into(), at: [40, 11] }), Err(EditError::NotOnWire { .. })));
    assert_eq!(ctext(&cc), before);
}

#[test]
fn label_commands() {
    let mut c0 = circuit();
    c0.labels.push(Label { at: [1, 1], name: "A".into(), color: None });
    let c = roundtrip(&c0, Edit::AddLabel(Label { at: [2, 2], name: "B".into(), color: None }));
    assert_eq!(c.labels.len(), 2);
    let c = roundtrip(&c0, Edit::MoveLabel { index: 0, to: [7, 7] });
    assert_eq!(c.labels[0].at, [7, 7]);
    let c = roundtrip(&c0, Edit::SetLabelColor { index: 0, color: Some("red".into()) });
    assert_eq!(c.labels[0].color.as_deref(), Some("red"));
    let c = roundtrip(&c0, Edit::RemoveLabel { index: 0 });
    assert!(c.labels.is_empty());
    let l = lib();
    let mut cc = c0.clone();
    assert!(matches!(apply(&mut cc, &l, Edit::MoveLabel { index: 1, to: [0, 0] }), Err(EditError::BadIndex { .. })));
    assert!(matches!(apply(&mut cc, &l, Edit::RemoveLabel { index: 5 }), Err(EditError::BadIndex { .. })));
}

#[test]
fn declared_nets_and_rename() {
    let mut c0 = circuit();
    let nets = vec![NetDecl { name: Some("X".into()), pins: vec!["R1.a".into(), "R2.b".into()] }];
    let c = roundtrip(&c0, Edit::SetDeclaredNets(nets.clone()));
    assert_eq!(c.nets, nets);
    c0.nets = nets;
    let c = roundtrip(&c0, Edit::SetDeclaredNets(vec![]));
    assert!(c.nets.is_empty());
    // rename a part: the nets follow, and `R10.a` is not touched when renaming `R1`
    c0.nets.push(NetDecl { name: None, pins: vec!["R10.a".into()] });
    let c = roundtrip(&c0, Edit::RenameId { old: "R1".into(), new: "Rx".into() });
    assert!(c.part("Rx").is_some() && c.part("R1").is_none());
    assert_eq!(c.nets[0].pins, vec!["Rx.a".to_string(), "R2.b".to_string()]);
    assert_eq!(c.nets[1].pins, vec!["R10.a".to_string()]);
    // rename a wire
    let c = roundtrip(&c0, Edit::RenameId { old: "w1".into(), new: "wire1".into() });
    assert!(c.wires.iter().any(|w| w.id == "wire1"));
    let l = lib();
    let mut cc = c0.clone();
    assert!(matches!(apply(&mut cc, &l, Edit::RenameId { old: "R1".into(), new: "R2".into() }), Err(EditError::DuplicateId(_))));
    assert!(matches!(apply(&mut cc, &l, Edit::RenameId { old: "R1".into(), new: "w1".into() }), Err(EditError::DuplicateId(_))));
    assert!(matches!(apply(&mut cc, &l, Edit::RenameId { old: "zz".into(), new: "q".into() }), Err(EditError::UnknownPart(_))));
    assert!(matches!(apply(&mut cc, &l, Edit::RenameId { old: "R1".into(), new: "".into() }), Err(EditError::EmptyId)));
    assert_eq!(ctext(&cc), ctext(&c0));
}

#[test]
fn errors_display() {
    for e in [
        EditError::DuplicateId("a".into()),
        EditError::UnknownPart("a".into()),
        EditError::UnknownWire("a".into()),
        EditError::NotPlaced { id: "a".into(), view: View::Breadboard },
        EditError::BadRotation(3),
        EditError::TooFewPoints("a".into()),
        EditError::BadIndex { what: "label", index: 2 },
        EditError::NotOnWire { id: "a".into(), at: [1, 2] },
        EditError::AtEndpoint { id: "a".into(), at: [1, 2] },
        EditError::EmptyId,
    ] {
        assert!(!e.to_string().is_empty());
        let _: &dyn std::error::Error = &e;
    }
}

// ---- tidy -----------------------------------------------------------------------------------

#[test]
fn tidy_removes_duplicates_and_merges_collinear() {
    let mut c = Circuit {
        wires: vec![
            wire("a", View::Schematic, &[[0, 0], [0, 0], [5, 0], [10, 0], [10, 0], [10, 5], [10, 9]]),
            wire("b", View::Schematic, &[[0, 0], [0, 0]]),
            wire("c", View::Schematic, &[[1, 1], [1, 1], [1, 1]]),
            wire("d", View::Breadboard, &[[0, 0], [0, 0], [2, 2], [4, 4]]),
            wire("keep", View::Schematic, &[[0, 5], [3, 5], [3, 8]]),
        ],
        ..Circuit::default()
    };
    tidy(&mut c, View::Schematic);
    assert_eq!(pts(&c, "a"), vec![[0, 0], [10, 0], [10, 9]]);
    assert!(c.wires.iter().all(|w| w.id != "b" && w.id != "c"), "collapsed wires deleted");
    assert_eq!(pts(&c, "keep"), vec![[0, 5], [3, 5], [3, 8]]);
    // other view untouched until asked
    assert_eq!(pts(&c, "d"), vec![[0, 0], [0, 0], [2, 2], [4, 4]]);
    tidy(&mut c, View::Breadboard);
    assert_eq!(pts(&c, "d"), vec![[0, 0], [4, 4]]);
}

#[test]
fn tidy_keeps_reversals_and_touched_vertices() {
    let mut c = Circuit {
        wires: vec![
            // goes out and back along the same line: not a plain collinear continuation
            wire("fold", View::Schematic, &[[0, 0], [5, 0], [2, 0]]),
            // vertex (5,5) is touched by another wire's endpoint: must stay
            wire("long", View::Schematic, &[[0, 5], [5, 5], [9, 5]]),
            wire("tee", View::Schematic, &[[5, 5], [5, 9]]),
            // vertex under a label stays
            wire("lab", View::Schematic, &[[0, 20], [4, 20], [8, 20]]),
        ],
        labels: vec![Label { at: [4, 20], name: "N".into(), color: None }],
        ..Circuit::default()
    };
    tidy(&mut c, View::Schematic);
    assert_eq!(pts(&c, "fold"), vec![[0, 0], [5, 0], [2, 0]]);
    assert_eq!(pts(&c, "long"), vec![[0, 5], [5, 5], [9, 5]]);
    assert_eq!(pts(&c, "lab"), vec![[0, 20], [4, 20], [8, 20]]);
}

#[test]
fn tidy_with_keeps_pin_vertices() {
    let l = lib();
    let mut c = Circuit {
        parts: vec![part("R1", Some([10, 10]), None)],
        // (18,11) is R1.b; a straight run passing through it with a vertex there
        wires: vec![wire("w", View::Schematic, &[[14, 11], [18, 11], [22, 11]])],
        ..Circuit::default()
    };
    let mut plain = c.clone();
    tidy(&mut plain, View::Schematic);
    assert_eq!(pts(&plain, "w"), vec![[14, 11], [22, 11]]);
    tidy_with(&mut c, &l, View::Schematic);
    assert_eq!(pts(&c, "w"), vec![[14, 11], [18, 11], [22, 11]]);
}

#[test]
fn merge_joins_wires_that_meet_end_to_end() {
    let l = lib();
    let mut c = Circuit {
        wires: vec![
            // straight on through (5,0): one wire, the redundant vertex goes
            wire("a", View::Schematic, &[[0, 0], [5, 0]]),
            wire("b", View::Schematic, &[[5, 0], [9, 0]]),
            // a corner at (3,8): the vertex stays; the second wire is given backwards
            wire("c", View::Schematic, &[[0, 8], [3, 8]]),
            wire("d", View::Schematic, &[[3, 12], [3, 8]]),
        ],
        ..Circuit::default()
    };
    assert_eq!(merge_wires(&mut c, &l, View::Schematic), 2);
    assert_eq!(c.wires.len(), 2);
    assert_eq!(pts(&c, "a"), vec![[0, 0], [9, 0]]);
    assert_eq!(pts(&c, "c"), vec![[0, 8], [3, 8], [3, 12]]);
    assert_eq!(merge_wires(&mut c, &l, View::Schematic), 0, "nothing left to join");
}

#[test]
fn merge_chains_keep_the_first_id_and_never_touch_other_views() {
    let l = lib();
    let mut c = Circuit {
        wires: vec![
            wire("a", View::Schematic, &[[0, 0], [2, 0]]),
            wire("b", View::Schematic, &[[2, 0], [2, 4]]),
            wire("c", View::Schematic, &[[2, 4], [6, 4]]),
            wire("j", View::Breadboard, &[[0, 0], [2, 0]]),
            wire("k", View::Breadboard, &[[2, 0], [2, 4]]),
        ],
        ..Circuit::default()
    };
    assert_eq!(merge_wires(&mut c, &l, View::Schematic), 2);
    assert_eq!(pts(&c, "a"), vec![[0, 0], [2, 0], [2, 4], [6, 4]]);
    assert_eq!(c.wires_in(View::Breadboard).count(), 2, "the breadboard is another view");
    assert_eq!(merge_wires(&mut c, &l, View::Breadboard), 1, "no pin or hole at the meeting point");
    assert_eq!(pts(&c, "j"), vec![[0, 0], [2, 0], [2, 4]]);
}

#[test]
fn merge_keeps_the_meeting_point_as_a_vertex_where_something_is_attached() {
    let l = lib();
    let mut c = Circuit {
        parts: vec![part("R1", Some([10, 10]), Some([10, 10]))],
        wires: vec![
            // both end on R1.b (18,11) in the schematic: one wire through the pin keeps a vertex
            wire("a", View::Schematic, &[[14, 11], [18, 11]]),
            wire("b", View::Schematic, &[[18, 11], [22, 11]]),
            // a label sits at (0,5)
            wire("p", View::Schematic, &[[-4, 5], [0, 5]]),
            wire("q", View::Schematic, &[[0, 5], [4, 5]]),
            // another wire ends in the middle of a join at (30,0): T-junction stays connected
            wire("r", View::Schematic, &[[26, 0], [30, 0]]),
            wire("s", View::Schematic, &[[30, 0], [34, 0]]),
            wire("t", View::Schematic, &[[30, 0], [30, 6]]),
        ],
        labels: vec![Label { at: [0, 5], name: "N".into(), color: None }],
        ..Circuit::default()
    };
    // The T at (30,0) has three ends: left alone.
    assert_eq!(merge_wires(&mut c, &l, View::Schematic), 2);
    assert_eq!(pts(&c, "a"), vec![[14, 11], [18, 11], [22, 11]]);
    assert_eq!(pts(&c, "p"), vec![[-4, 5], [0, 5], [4, 5]]);
    assert!(c.wires.iter().any(|w| w.id == "r") && c.wires.iter().any(|w| w.id == "s") && c.wires.iter().any(|w| w.id == "t"));
}

#[test]
fn merge_never_drops_a_breadboard_hole_and_respects_colours() {
    let l = lib();
    let mut c = Circuit {
        parts: vec![part("R1", Some([10, 10]), Some([10, 10]))],
        wires: vec![
            // both end on R1.a (10,10) of the breadboard: a vertex would not connect the hole
            wire("a", View::Breadboard, &[[6, 10], [10, 10]]),
            wire("b", View::Breadboard, &[[10, 10], [10, 14]]),
            // different colours stay two wires
            wire("c", View::Breadboard, &[[0, 0], [3, 0]]),
            wire("d", View::Breadboard, &[[3, 0], [3, 3]]),
        ],
        ..Circuit::default()
    };
    c.wires[2].color = Some("red".into());
    c.wires[3].color = Some("blue".into());
    assert_eq!(merge_wires(&mut c, &l, View::Breadboard), 0);
    assert_eq!(c.wires.len(), 4);
    c.wires[3].color = Some("red".into());
    assert_eq!(merge_wires(&mut c, &l, View::Breadboard), 1);
    assert_eq!(pts(&c, "c"), vec![[0, 0], [3, 0], [3, 3]]);
}

#[test]
fn tidy_can_be_undone_as_a_group() {
    let l = lib();
    let mut c = Circuit { wires: vec![wire("a", View::Schematic, &[[0, 0], [5, 0], [10, 0]])], ..Circuit::default() };
    let before = ctext(&c);
    let mut h = History::new();
    h.begin_group();
    h.apply(&mut c, &l, Edit::SetTitle(Some("x".into()))).unwrap();
    tidy(&mut c, View::Schematic);
    h.end_group();
    assert_eq!(pts(&c, "a").len(), 2);
    assert!(h.undo(&mut c));
    assert_eq!(ctext(&c), before);
}

// ---- history --------------------------------------------------------------------------------

#[test]
fn history_groups_are_one_undo_step() {
    let l = lib();
    let mut c = circuit();
    let c0 = ctext(&c);
    let mut h = History::new();
    h.begin_group();
    for i in 0..5 {
        h.apply(&mut c, &l, Edit::MovePart { id: "R1".into(), view: View::Schematic, to: [10 + i, 10] }).unwrap();
    }
    h.end_group();
    assert_eq!(h.undo.len(), 1);
    h.apply(&mut c, &l, Edit::SetTitle(Some("t".into()))).unwrap();
    assert_eq!(h.undo.len(), 2);
    assert!(h.undo(&mut c));
    assert_eq!(c.part("R1").unwrap().schematic.as_ref().unwrap().at, [14, 10]);
    assert!(h.undo(&mut c));
    assert_eq!(ctext(&c), c0);
    assert!(!h.undo(&mut c) && !h.can_undo());
    assert!(h.redo(&mut c));
    assert_eq!(c.part("R1").unwrap().schematic.as_ref().unwrap().at, [14, 10]);
    assert!(h.redo(&mut c));
    assert_eq!(c.title.as_deref(), Some("t"));
    assert!(!h.redo(&mut c));
}

#[test]
fn history_nested_groups_and_empty_groups() {
    let l = lib();
    let mut c = circuit();
    let mut h = History::new();
    h.begin_group();
    h.begin_group();
    h.apply(&mut c, &l, Edit::SetTitle(Some("1".into()))).unwrap();
    h.end_group();
    h.apply(&mut c, &l, Edit::SetTitle(Some("2".into()))).unwrap();
    h.end_group();
    h.begin_group();
    h.end_group();
    assert_eq!(h.undo.len(), 1);
    h.apply(&mut c, &l, Edit::SetTitle(Some("3".into()))).unwrap();
    assert_eq!(h.undo.len(), 2, "a new group after the first starts a new step");
    h.undo(&mut c);
    assert_eq!(c.title.as_deref(), Some("2"));
    h.undo(&mut c);
    assert_eq!(c.title, None);
}

#[test]
fn history_new_edit_clears_redo_and_failed_edit_records_nothing() {
    let l = lib();
    let mut c = circuit();
    let mut h = History::new();
    h.apply(&mut c, &l, Edit::SetTitle(Some("a".into()))).unwrap();
    h.undo(&mut c);
    assert!(h.can_redo());
    let before = ctext(&c);
    assert!(h.apply(&mut c, &l, Edit::RemovePart { id: "nope".into() }).is_err());
    assert!(h.can_redo(), "a failed edit keeps redo");
    assert!(!h.can_undo());
    assert_eq!(ctext(&c), before);
    h.apply(&mut c, &l, Edit::SetTitle(Some("b".into()))).unwrap();
    assert!(!h.can_redo());
    h.clear();
    assert!(!h.can_undo() && !h.can_redo());
}

#[test]
fn history_stack_is_capped() {
    let l = lib();
    let mut c = circuit();
    let mut h = History::new();
    for i in 0..250 {
        h.apply(&mut c, &l, Edit::SetTitle(Some(i.to_string()))).unwrap();
    }
    assert_eq!(h.undo.len(), 200);
    let mut n = 0;
    while h.undo(&mut c) {
        n += 1;
    }
    assert_eq!(n, 200);
    assert_eq!(c.title.as_deref(), Some("49"), "the oldest 50 steps were dropped");
}

#[test]
fn dirty_tracking() {
    let l = lib();
    let mut c = circuit();
    let mut h = History::new();
    assert!(!h.dirty());
    h.apply(&mut c, &l, Edit::SetTitle(Some("a".into()))).unwrap();
    assert!(h.dirty());
    h.mark_clean();
    assert!(!h.dirty());
    h.apply(&mut c, &l, Edit::SetTitle(Some("b".into()))).unwrap();
    assert!(h.dirty());
    h.undo(&mut c);
    assert!(!h.dirty(), "back at the saved state");
    h.redo(&mut c);
    assert!(h.dirty());
    h.undo(&mut c);
    h.undo(&mut c);
    assert!(h.dirty(), "before the saved state");
    h.redo(&mut c);
    assert!(!h.dirty());
    // editing after undoing past the saved state can never be clean again
    h.undo(&mut c);
    h.apply(&mut c, &l, Edit::SetTitle(Some("z".into()))).unwrap();
    assert!(h.dirty());
    h.undo(&mut c);
    assert!(h.dirty());
    // a group is dirty as a whole and clean after undo
    let mut h = History::new();
    h.begin_group();
    h.apply(&mut c, &l, Edit::SetTitle(Some("1".into()))).unwrap();
    h.apply(&mut c, &l, Edit::SetTitle(Some("2".into()))).unwrap();
    h.end_group();
    assert!(h.dirty());
    h.undo(&mut c);
    assert!(!h.dirty());
    // clear keeps the flag
    h.apply(&mut c, &l, Edit::SetTitle(Some("3".into()))).unwrap();
    h.clear();
    assert!(h.dirty());
    h.mark_clean();
    assert!(!h.dirty());
}

#[test]
fn history_drag_with_rubber_band_undoes_wires_too() {
    let l = lib();
    let mut c = circuit();
    let c0 = ctext(&c);
    let mut h = History::new();
    h.begin_group();
    for dy in 1..=4 {
        h.apply(&mut c, &l, Edit::MovePart { id: "R1".into(), view: View::Schematic, to: [10, 10 + dy] }).unwrap();
    }
    h.end_group();
    assert_eq!(pts(&c, "w1")[0], [18, 15]);
    assert!(h.undo(&mut c));
    assert_eq!(ctext(&c), c0);
}
