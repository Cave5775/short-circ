use super::*;
use crate::model::{Circuit, Dir, Part, Wire};
use std::io::Write;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn pin<'a>(p: &'a PartDef, id: &str) -> &'a PinDef {
    p.pins.iter().find(|x| x.id == id).unwrap_or_else(|| panic!("no pin {id} in {:?}", p.pins.iter().map(|x| &x.id).collect::<Vec<_>>()))
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("short-circ-fzp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn pins_from_terminals_mm_units_and_group_transform() {
    let (p, warns) = import_fzp_file(&fixtures().join("ic4.fzp"), &ImportOptions::default()).unwrap();
    assert_eq!(p.ty, "ic4_test_part");
    assert_eq!(p.name, "IC4 Test Part");
    // 12.7 x 7.62 mm is exactly 5 x 3 grid units.
    assert_eq!(p.size, [5.0, 3.0]);
    assert_eq!(pin(&p, "VCC").at, [0, 1]);
    assert_eq!(pin(&p, "VCC").dir, Dir::Left);
    // The surrounding spaces of the name are trimmed.
    assert_eq!(pin(&p, "GND").at, [0, 2]);
    // These two sit inside <g transform="translate(10.16 0)">.
    assert_eq!(pin(&p, "OUT").at, [5, 1]);
    assert_eq!(pin(&p, "OUT").dir, Dir::Right);
    // The second "OUT" falls back to its connector id.
    assert_eq!(pin(&p, "connector3").at, [5, 2]);
    assert_eq!(p.pins.len(), 4, "connector4 has no schematic view");
    assert!(warns.iter().any(|w| w.contains("connector4")), "{warns:?}");
    // Metadata.
    let meta = p.meta.as_ref().unwrap();
    assert_eq!(meta["title"], "IC4 Test Part");
    assert_eq!(meta["module_id"], "Ic4TestPartModuleID");
    assert_eq!(meta["family"], "Test ICs");
    assert_eq!(meta["package"], "DIP4 [THT]");
    assert_eq!(meta["pins"], "4");
    assert_eq!(meta["source"], "fritzing");
    // Graphics: the body keeps its CSS colours, pins keep theirs, and the invisible terminal
    // markers are not drawn.
    let body = p
        .shapes
        .iter()
        .find_map(|s| match s {
            Shape::Rect { at, size, style, .. } => Some((*at, *size, style.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!((body.0, body.1), ([1.0, 0.0], [3.0, 3.0]));
    assert_eq!(body.2.fill.as_deref(), Some("#ffffff"));
    assert_eq!(body.2.stroke.as_deref(), Some("#0000ff"));
    assert_eq!(body.2.width, Some(0.098), "0.25 mm");
    let rects = p.shapes.iter().filter(|s| matches!(s, Shape::Rect { .. })).count();
    assert_eq!(rects, 1, "terminal rects are invisible");
    let lines: Vec<_> = p.shapes.iter().filter_map(|s| if let Shape::Line { style, .. } = s { Some(style.clone()) } else { None }).collect();
    assert_eq!(lines.len(), 4);
    assert!(lines.iter().all(|s| s.stroke.as_deref() == Some("#787878")));
    assert!(p.shapes.iter().any(|s| matches!(s, Shape::Text { text, .. } if text == "IC4")));
    assert!(p.shapes.iter().any(|s| matches!(s, Shape::Text { text, .. } if text == "{ref}")));
    // The result is a valid .part.
    let json = part_json(&p).unwrap();
    let back = crate::parse_part(&json).unwrap();
    assert_eq!(back.pins.len(), 4);
    assert_eq!(back.meta.unwrap()["family"], "Test ICs");
}

#[test]
fn pins_without_terminal_use_far_end_and_use_elements() {
    let (p, warns) = import_fzp_file(&fixtures().join("bare.fzp"), &ImportOptions::default()).unwrap();
    assert_eq!(p.ty, "bare_part");
    assert_eq!(p.size, [10.0, 5.0], "72 dpi: 72x36 px is 1 x 0.5 in");
    assert_eq!(pin(&p, "A").at, [0, 2]);
    assert_eq!(pin(&p, "A").dir, Dir::Left);
    // B comes from <use xlink:href="#stub" transform=...> with an undeclared xlink prefix.
    assert_eq!(pin(&p, "B").at, [10, 3]);
    assert_eq!(pin(&p, "B").dir, Dir::Right);
    assert!(p.shapes.iter().filter(|s| matches!(s, Shape::Line { .. })).count() >= 2);
    assert!(warns.is_empty(), "{warns:?}");
}

#[test]
fn view_selection() {
    let sch = import_fzp_file(&fixtures().join("ic4.fzp"), &ImportOptions::default()).unwrap().0;
    let opts = ImportOptions { view: View::Breadboard, type_name: Some("ic4_bb".into()) };
    let (bb, warns) = import_fzp_file(&fixtures().join("ic4.fzp"), &opts).unwrap();
    assert_eq!(bb.ty, "ic4_bb");
    assert_eq!(bb.size, [5.0, 4.0], "unit-less 45x36 at 90 dpi");
    let ids: Vec<&str> = bb.pins.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["VCC", "GND", "OUT", "NC"]);
    assert_eq!(pin(&bb, "VCC").at, [1, 1]);
    assert_eq!(pin(&bb, "NC").at, [4, 3]);
    assert!(warns.iter().any(|w| w.contains("connector3")), "{warns:?}");
    assert_ne!(sch.size, bb.size);
    assert_eq!(View::parse("Breadboard"), Some(View::Breadboard));
    assert_eq!(View::parse("pcb"), None);
}

fn mini_fzp(connectors: &str) -> String {
    format!(
        r#"<module moduleId="Mini"><title>Mini 1</title><views><schematicView>
        <layers image="schematic/m.svg"/></schematicView></views><connectors>{connectors}</connectors></module>"#
    )
}

fn conn(id: &str, name: &str, svg_id: &str) -> String {
    format!(
        r#"<connector id="{id}" name="{name}"><views><schematicView><p layer="schematic" svgId="{svg_id}"/></schematicView></views></connector>"#
    )
}

fn import_inline(fzp: &str, svg: &str) -> Result<(PartDef, Vec<String>), String> {
    let svg = svg.to_string();
    import_fzp(fzp, &mut |_| Some(svg.clone().into_bytes()), &ImportOptions::default())
}

#[test]
fn pin_ids_are_unique_and_never_empty() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="0.4in" height="0.3in" viewBox="0 0 4 3">
        <rect x="1" y="0" width="2" height="3" fill="none" stroke="#000"/>
        <circle id="a" cx="0" cy="1" r="0.05"/><circle id="b" cx="0" cy="2" r="0.05"/>
        <circle id="c" cx="4" cy="1" r="0.05"/><circle id="d" cx="4" cy="2" r="0.05"/></svg>"##;
    let fzp = mini_fzp(&[conn("c0", "X", "a"), conn("c1", "X", "b"), conn("c2", "", "c"), conn("c3", "X", "d")].concat());
    let (p, warns) = import_inline(&fzp, svg).unwrap();
    let ids: Vec<&str> = p.pins.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, ["X", "c1", "c2", "c3"]);
    assert_eq!(p.pins[1].name.as_deref(), Some("X"), "original name kept for display");
    assert_eq!(p.ty, "mini_1");
    assert!(warns.iter().any(|w| w.contains("not unique")), "{warns:?}");
    let set: BTreeSet<_> = ids.iter().collect();
    assert_eq!(set.len(), ids.len());
}

#[test]
fn coincident_pins_are_nudged_with_a_warning() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="0.3in" height="0.3in" viewBox="0 0 3 3">
        <rect x="0" y="0" width="3" height="3" fill="none" stroke="#000"/>
        <circle id="a" cx="1" cy="1" r="0.05"/><circle id="b" cx="1.1" cy="1" r="0.05"/></svg>"##;
    let (p, warns) = import_inline(&mini_fzp(&[conn("c0", "A", "a"), conn("c1", "B", "b")].concat()), svg).unwrap();
    assert_ne!(pin(&p, "A").at, pin(&p, "B").at);
    assert!(warns.iter().any(|w| w.contains("same point")), "{warns:?}");
}

#[test]
fn near_grid_pins_snap_with_the_drawing() {
    // Pins at x = 1.375 (like a 72 dpi export): the drawing shifts so the pin lands on 1.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="0.5in" height="0.3in" viewBox="0 0 5 3">
        <line id="a" x1="1.375" y1="0" x2="1.375" y2="3" stroke="#000" stroke-width="0.1"/></svg>"##;
    let (p, _) = import_inline(&mini_fzp(&conn("c0", "A", "a")), svg).unwrap();
    // The line is the leftmost geometry, so the whole drawing moves to x = 0 with the pin on it.
    let Shape::Line { pts, .. } = &p.shapes[0] else { panic!("{:?}", p.shapes[0]) };
    assert_eq!(pin(&p, "A").at[0], 0);
    assert!(pts[0][0].abs() < 1e-6, "{pts:?}");
    assert!(p.shapes.iter().all(|s| match s {
        Shape::Line { pts, .. } => pts.iter().all(|q| q[0] >= 0.0 && q[1] >= 0.0),
        _ => true,
    }));
}

#[test]
fn errors_are_reported() {
    let o = ImportOptions::default();
    let none: &mut Loader = &mut |_| None;
    assert!(import_fzp("<nope/>", none, &o).unwrap_err().contains("module"));
    let none: &mut Loader = &mut |_| None;
    assert!(import_fzp("<module", none, &o).unwrap_err().contains("XML"));
    let none: &mut Loader = &mut |_| None;
    assert!(import_fzp(&mini_fzp(""), none, &o).unwrap_err().contains("not found"));
    assert!(import_inline(&mini_fzp(""), "<html/>").is_err());
    assert!(import_inline(&mini_fzp(""), "<svg xmlns='http://www.w3.org/2000/svg'/>").is_err());
    let no_view = r#"<module moduleId="x"><title>t</title><views><pcbView/></views></module>"#;
    let none: &mut Loader = &mut |_| None;
    assert!(import_fzp(no_view, none, &o).unwrap_err().contains("schematicView"));
    assert!(import_fzpz(b"not a zip", &o).is_err());
    assert!(import_path(Path::new("x.txt"), &o).is_err());
}

fn build_fzpz(flat_names: bool) -> Vec<u8> {
    let fx = fixtures();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let (sch, bb) = if flat_names {
            ("svg.schematic.ic4_schematic.svg", "svg.breadboard.ic4_breadboard.svg")
        } else {
            ("svg/core/schematic/ic4_schematic.svg", "svg/core/breadboard/ic4_breadboard.svg")
        };
        zw.start_file("ic4.fzp", deflated).unwrap();
        zw.write_all(&std::fs::read(fx.join("ic4.fzp")).unwrap()).unwrap();
        zw.start_file(sch, deflated).unwrap();
        zw.write_all(&std::fs::read(fx.join("schematic/ic4_schematic.svg")).unwrap()).unwrap();
        zw.start_file(bb, stored).unwrap();
        zw.write_all(&std::fs::read(fx.join("breadboard/ic4_breadboard.svg")).unwrap()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

#[test]
fn fzpz_round_trip_matches_loose_files() {
    let loose = import_fzp_file(&fixtures().join("ic4.fzp"), &ImportOptions::default()).unwrap().0;
    for flat in [true, false] {
        let zip_bytes = build_fzpz(flat);
        let (p, _) = import_fzpz(&zip_bytes, &ImportOptions::default()).unwrap();
        assert_eq!(part_json(&p).unwrap(), part_json(&loose).unwrap(), "flat={flat}");
        let opts = ImportOptions { view: View::Breadboard, type_name: None };
        let (b, _) = import_fzpz(&zip_bytes, &opts).unwrap();
        assert_eq!(b.size, [5.0, 4.0], "flat={flat}");
    }
    // A bundle without its SVG fails cleanly.
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        zw.start_file("ic4.fzp", zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(&std::fs::read(fixtures().join("ic4.fzp")).unwrap()).unwrap();
        zw.finish().unwrap();
    }
    assert!(import_fzpz(&buf.into_inner(), &ImportOptions::default()).unwrap_err().contains("not found"));
}

#[test]
fn asset_lookup_in_repo_layout() {
    // fritzing-parts layout: core/x.fzp next to svg/core/schematic/x.svg.
    let d = temp_dir("repo");
    std::fs::create_dir_all(d.join("core")).unwrap();
    std::fs::create_dir_all(d.join("svg/core/schematic")).unwrap();
    std::fs::copy(fixtures().join("ic4.fzp"), d.join("core/ic4.fzp")).unwrap();
    std::fs::copy(fixtures().join("schematic/ic4_schematic.svg"), d.join("svg/core/schematic/ic4_schematic.svg")).unwrap();
    let (p, _) = import_fzp_file(&d.join("core/ic4.fzp"), &ImportOptions::default()).unwrap();
    assert_eq!(p.pins.len(), 4);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn list_files_and_import_to_dir() {
    let d = temp_dir("list");
    std::fs::write(d.join("a.fzpz"), build_fzpz(true)).unwrap();
    std::fs::write(d.join("a.fzp"), "x").unwrap();
    std::fs::write(d.join("b.fzp"), "x").unwrap();
    std::fs::write(d.join("c.txt"), "x").unwrap();
    let names: Vec<String> = list_fritzing_files(&d).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["a.fzpz", "b.fzp"]);
    let out = d.join("out");
    let (part, _, path) = import_to_dir(&d.join("a.fzpz"), &out, &ImportOptions::default()).unwrap();
    assert_eq!(path, out.join("ic4_test_part.part"));
    let back = crate::parse_part(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(back.ty, part.ty);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn library_loads_fritzing_files_from_parts_folders() {
    let d = temp_dir("lib");
    let parts = d.join("parts");
    std::fs::create_dir_all(parts.join("schematic")).unwrap();
    std::fs::write(parts.join("ic4.fzpz"), build_fzpz(true)).unwrap();
    std::fs::copy(fixtures().join("bare.fzp"), parts.join("bare.fzp")).unwrap();
    std::fs::copy(fixtures().join("schematic/bare.svg"), parts.join("schematic/bare.svg")).unwrap();
    std::fs::write(parts.join("broken.fzpz"), b"junk").unwrap();
    let lib = crate::Library::load(&Circuit::default(), Some(&d));
    assert!(lib.get("ic4_test_part").is_some());
    assert!(lib.get("bare_part").is_some());
    assert!(lib.get("resistor").is_some(), "built-ins still there");
    // An earlier source (the circuit's own lib) is never overridden.
    let c: Circuit = crate::parse_circuit(r#"{"format":"short-circ/1","lib":[{"type":"bare_part","size":[1,1]}]}"#).unwrap();
    assert_eq!(crate::Library::load(&c, Some(&d)).get("bare_part").unwrap().size, [1.0, 1.0]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn imported_part_validates_and_renders_in_a_circuit() {
    let (p, _) = import_fzp_file(&fixtures().join("ic4.fzp"), &ImportOptions::default()).unwrap();
    let circuit = Circuit {
        parts: vec![Part {
            id: "U1".into(),
            ty: p.ty.clone(),
            at: [10, 10],
            rot: 0,
            mirror: false,
            props: Default::default(),
            hidden_label: false,
        }],
        wires: vec![
            Wire { id: "w1".into(), pts: vec![[10, 11], [6, 11]], color: None },
            Wire { id: "w2".into(), pts: vec![[15, 11], [19, 11]], color: None },
        ],
        lib: vec![p],
        format: crate::FORMAT.to_string(),
        ..Default::default()
    };
    let lib = crate::Library::load(&circuit, None);
    let report = crate::validate(&circuit, &lib);
    assert_eq!(report.errors, 0, "{:?}", report.issues);
    assert!(!report.issues.iter().any(|i| i.rule == "unknown-type"));
    let nl = crate::Netlist::build(&circuit, &lib);
    assert!(nl.nets.iter().any(|n| n.pins.iter().any(|r| r.part == "U1" && r.pin == "VCC") && n.wires.contains(&"w1".to_string())));
    assert!(nl.nets.iter().any(|n| n.pins.iter().any(|r| r.part == "U1" && r.pin == "OUT") && n.wires.contains(&"w2".to_string())));
    let svg = crate::svg::render_svg(&circuit, &lib, false);
    assert!(svg.starts_with("<?xml") || svg.contains("<svg"));
    assert!(svg.contains("id=\"U1\""), "{svg}");
    assert!(svg.contains("#0000ff"), "body stroke colour is rendered");
}

#[test]
fn slugs() {
    assert_eq!(slug("555 Timer"), "555_timer");
    assert_eq!(slug("  Red LED - 3mm "), "red_led_3mm");
    assert_eq!(slug("Ünï"), "n");
    assert_eq!(slug("***"), "");
}
