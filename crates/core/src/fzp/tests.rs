use super::*;
use crate::model::{Circuit, Dir, Part, Placement, Wire};
use std::io::Write;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn only(v: View) -> ImportOptions {
    ImportOptions { views: vec![v], type_name: None }
}

fn sch(p: &PartDef) -> &ViewDef {
    p.schematic.as_ref().expect("no schematic view")
}

fn bb(p: &PartDef) -> &ViewDef {
    p.breadboard.as_ref().expect("no breadboard view")
}

fn ids(p: &PartDef) -> Vec<&str> {
    p.pins.iter().map(|x| x.id.as_str()).collect()
}

/// Position of a pin in a view; panics with the available pins when absent.
fn pin(p: &PartDef, view: View, id: &str) -> PinPos {
    let v = p.view(view).unwrap_or_else(|| panic!("no {view:?} view"));
    *v.pins.get(id).unwrap_or_else(|| panic!("no pin {id} in {:?}", v.pins.keys().collect::<Vec<_>>()))
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("short-circ-fzp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn pins_from_terminals_mm_units_and_group_transform() {
    let (p, warns) = import_fzp_file(&fixtures().join("ic4.fzp"), &only(View::Schematic)).unwrap();
    assert_eq!(p.ty, "ic4_test_part");
    assert_eq!(p.name, "IC4 Test Part");
    assert!(p.breadboard.is_none(), "only the schematic was requested");
    // 12.7 x 7.62 mm is exactly 5 x 3 grid units.
    assert_eq!(sch(&p).size, [5.0, 3.0]);
    assert_eq!(pin(&p, View::Schematic, "VCC").at, [0, 1]);
    assert_eq!(pin(&p, View::Schematic, "VCC").dir, Dir::Left);
    // The surrounding spaces of the name are trimmed.
    assert_eq!(pin(&p, View::Schematic, "GND").at, [0, 2]);
    // These two sit inside <g transform="translate(10.16 0)">.
    assert_eq!(pin(&p, View::Schematic, "OUT").at, [5, 1]);
    assert_eq!(pin(&p, View::Schematic, "OUT").dir, Dir::Right);
    // The second "OUT" falls back to its connector id.
    assert_eq!(pin(&p, View::Schematic, "connector3").at, [5, 2]);
    // Logical pins hold every connector, the view only those it has.
    assert_eq!(ids(&p), ["VCC", "GND", "OUT", "connector3", "NC"]);
    assert_eq!(sch(&p).pins.len(), 4, "connector4 has no schematic view");
    assert!(warns.iter().any(|w| w.contains("connector4")), "{warns:?}");
    // Metadata.
    let meta = p.meta.as_ref().unwrap();
    assert_eq!(meta["title"], "IC4 Test Part");
    assert_eq!(meta["module_id"], "Ic4TestPartModuleID");
    assert_eq!(meta["family"], "Test ICs");
    assert_eq!(meta["package"], "DIP4 [THT]");
    assert_eq!(meta["pins"], "4");
    assert_eq!(meta["source"], "fritzing");
    assert_eq!(p.category.as_deref(), Some("imported"));
    // Graphics: the body keeps its CSS colours, pins keep theirs, and the invisible terminal
    // markers are not drawn.
    let shapes = &sch(&p).shapes;
    let body = shapes
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
    let rects = shapes.iter().filter(|s| matches!(s, Shape::Rect { .. })).count();
    assert_eq!(rects, 1, "terminal rects are invisible");
    let lines: Vec<_> = shapes.iter().filter_map(|s| if let Shape::Line { style, .. } = s { Some(style.clone()) } else { None }).collect();
    assert_eq!(lines.len(), 4);
    assert!(lines.iter().all(|s| s.stroke.as_deref() == Some("#787878")));
    assert!(shapes.iter().any(|s| matches!(s, Shape::Text { text, .. } if text == "IC4")));
    assert!(shapes.iter().any(|s| matches!(s, Shape::Text { text, .. } if text == "{ref}")));
    // The result is a valid .part.
    let json = part_json(&p).unwrap();
    let back = crate::parse_part(&json).unwrap();
    assert_eq!(back.pins.len(), 5);
    assert_eq!(back.meta.unwrap()["family"], "Test ICs");
}

#[test]
fn pins_without_terminal_use_far_end_and_use_elements() {
    let (p, warns) = import_fzp_file(&fixtures().join("bare.fzp"), &only(View::Schematic)).unwrap();
    assert_eq!(p.ty, "bare_part");
    assert_eq!(sch(&p).size, [10.0, 5.0], "72 dpi: 72x36 px is 1 x 0.5 in");
    assert_eq!(pin(&p, View::Schematic, "A").at, [0, 2]);
    assert_eq!(pin(&p, View::Schematic, "A").dir, Dir::Left);
    // B comes from <use xlink:href="#stub" transform=...> with an undeclared xlink prefix.
    assert_eq!(pin(&p, View::Schematic, "B").at, [10, 3]);
    assert_eq!(pin(&p, View::Schematic, "B").dir, Dir::Right);
    assert!(sch(&p).shapes.iter().filter(|s| matches!(s, Shape::Line { .. })).count() >= 2);
    assert!(warns.is_empty(), "{warns:?}");
}

#[test]
fn view_selection() {
    let both = import_fzp_file(&fixtures().join("ic4.fzp"), &ImportOptions::default()).unwrap().0;
    let opts = ImportOptions { views: vec![View::Breadboard], type_name: Some("ic4_bb".into()) };
    let (b, warns) = import_fzp_file(&fixtures().join("ic4.fzp"), &opts).unwrap();
    assert_eq!(b.ty, "ic4_bb");
    assert!(b.schematic.is_none());
    assert_eq!(bb(&b).size, [5.0, 4.0], "unit-less 45x36 at 90 dpi");
    assert_eq!(ids(&b), ["VCC", "GND", "OUT", "connector3", "NC"]);
    let placed: Vec<&str> = bb(&b).pins.keys().map(String::as_str).collect();
    assert_eq!(placed, ["GND", "NC", "OUT", "VCC"], "connector3 has no breadboard view");
    assert_eq!(pin(&b, View::Breadboard, "VCC").at, [1, 1]);
    assert_eq!(pin(&b, View::Breadboard, "NC").at, [4, 3]);
    assert_eq!(pin(&b, View::Breadboard, "NC").dir, Dir::default(), "breadboard pins have no dir");
    assert!(warns.iter().any(|w| w.contains("connector3")), "{warns:?}");
    // Both views at once give one part with both.
    assert_eq!(both.ty, "ic4_test_part");
    assert_eq!(sch(&both).size, [5.0, 3.0]);
    assert_eq!(bb(&both).size, [5.0, 4.0]);
    assert_eq!(both.pins.len(), 5);
    assert_eq!(View::parse("Breadboard"), Some(View::Breadboard));
    assert_eq!(View::parse("pcb"), None);
}

#[test]
fn asking_for_a_missing_view_warns_or_fails() {
    // bare.fzp has a schematic only: the default asks for both, so it warns.
    let (p, warns) = import_fzp_file(&fixtures().join("bare.fzp"), &ImportOptions::default()).unwrap();
    assert!(p.schematic.is_some() && p.breadboard.is_none());
    assert!(warns.iter().any(|w| w.contains("breadboardView")), "{warns:?}");
    // Asking only for the view it lacks is an error.
    let err = import_fzp_file(&fixtures().join("bare.fzp"), &only(View::Breadboard)).unwrap_err();
    assert!(err.contains("breadboardView"), "{err}");
    let none = ImportOptions { views: vec![], type_name: None };
    assert!(import_fzp_file(&fixtures().join("bare.fzp"), &none).is_err());
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
    let ids = ids(&p);
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
    assert_ne!(pin(&p, View::Schematic, "A").at, pin(&p, View::Schematic, "B").at);
    assert!(warns.iter().any(|w| w.contains("same point")), "{warns:?}");
}

#[test]
fn near_grid_pins_snap_with_the_drawing() {
    // Pins at x = 1.375 (like a 72 dpi export): the drawing shifts so the pin lands on 1.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="0.5in" height="0.3in" viewBox="0 0 5 3">
        <line id="a" x1="1.375" y1="0" x2="1.375" y2="3" stroke="#000" stroke-width="0.1"/></svg>"##;
    let (p, _) = import_inline(&mini_fzp(&conn("c0", "A", "a")), svg).unwrap();
    // The line is the leftmost geometry, so the whole drawing moves to x = 0 with the pin on it.
    let Shape::Line { pts, .. } = &sch(&p).shapes[0] else { panic!("{:?}", sch(&p).shapes[0]) };
    assert_eq!(pin(&p, View::Schematic, "A").at[0], 0);
    assert!(pts[0][0].abs() < 1e-6, "{pts:?}");
    assert!(sch(&p).shapes.iter().all(|s| match s {
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

fn zip_of(files: &[(&str, PathBuf, bool)]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        for (name, src, deflate) in files {
            let m = if *deflate { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
            zw.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(m)).unwrap();
            zw.write_all(&std::fs::read(src).unwrap()).unwrap();
        }
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn build_fzpz(flat_names: bool) -> Vec<u8> {
    let fx = fixtures();
    let (s, b) = if flat_names {
        ("svg.schematic.ic4_schematic.svg", "svg.breadboard.ic4_breadboard.svg")
    } else {
        ("svg/core/schematic/ic4_schematic.svg", "svg/core/breadboard/ic4_breadboard.svg")
    };
    zip_of(&[
        ("ic4.fzp", fx.join("ic4.fzp"), true),
        (s, fx.join("schematic/ic4_schematic.svg"), true),
        (b, fx.join("breadboard/ic4_breadboard.svg"), false),
    ])
}

#[test]
fn fzpz_round_trip_matches_loose_files() {
    let loose = import_fzp_file(&fixtures().join("ic4.fzp"), &ImportOptions::default()).unwrap().0;
    for flat in [true, false] {
        let zip_bytes = build_fzpz(flat);
        let (p, _) = import_fzpz(&zip_bytes, &ImportOptions::default()).unwrap();
        assert_eq!(part_json(&p).unwrap(), part_json(&loose).unwrap(), "flat={flat}");
        let (b, _) = import_fzpz(&zip_bytes, &only(View::Breadboard)).unwrap();
        assert_eq!(bb(&b).size, [5.0, 4.0], "flat={flat}");
        assert!(b.schematic.is_none());
    }
    // A bundle without its SVG fails cleanly.
    let nosvg = zip_of(&[("ic4.fzp", fixtures().join("ic4.fzp"), true)]);
    assert!(import_fzpz(&nosvg, &ImportOptions::default()).unwrap_err().contains("not found"));
    // With only the schematic SVG the breadboard view is skipped with a warning.
    let sch_only = zip_of(&[("ic4.fzp", fixtures().join("ic4.fzp"), true), ("svg.schematic.ic4_schematic.svg", fixtures().join("schematic/ic4_schematic.svg"), true)]);
    let (p, warns) = import_fzpz(&sch_only, &ImportOptions::default()).unwrap();
    assert!(p.schematic.is_some() && p.breadboard.is_none());
    assert!(warns.iter().any(|w| w.contains("breadboard") && w.contains("not found")), "{warns:?}");
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
    assert_eq!(sch(&p).pins.len(), 4);
    assert!(p.breadboard.is_none(), "its SVG is not in the tree");
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
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, part_json(&part).unwrap());
    let back = crate::parse_part(&text).unwrap();
    assert_eq!(back.ty, part.ty);
    assert!(back.schematic.is_some() && back.breadboard.is_some());
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
    let ic4 = lib.get("ic4_test_part").unwrap();
    assert!(ic4.schematic.is_some() && ic4.breadboard.is_some(), "default options import both views");
    assert!(lib.get("bare_part").is_some());
    assert!(lib.get("resistor").is_some(), "built-ins still there");
    // An earlier source (the circuit's own lib) is never overridden.
    let c: Circuit = crate::parse_circuit(r#"{"format":"short-circ/1","lib":[{"type":"bare_part","size":[1,1]}]}"#).unwrap();
    assert_eq!(crate::Library::load(&c, Some(&d)).get("bare_part").unwrap().schematic.as_ref().unwrap().size, [1.0, 1.0]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn imported_part_validates_and_renders_in_a_circuit() {
    let (p, _) = import_fzp_file(&fixtures().join("ic4.fzp"), &ImportOptions::default()).unwrap();
    let circuit = Circuit {
        parts: vec![Part {
            id: "U1".into(),
            ty: p.ty.clone(),
            schematic: Some(Placement::at(10, 10)),
            ..Part::default()
        }],
        wires: vec![
            Wire { id: "w1".into(), view: View::Schematic, pts: vec![[10, 11], [6, 11]], color: None },
            Wire { id: "w2".into(), view: View::Schematic, pts: vec![[15, 11], [19, 11]], color: None },
        ],
        lib: vec![p],
        ..Default::default()
    };
    let lib = crate::Library::load(&circuit, None);
    let report = crate::validate(&circuit, &lib);
    assert_eq!(report.errors, 0, "{:?}", report.issues);
    assert!(!report.issues.iter().any(|i| i.rule == "unknown-type"));
    let nl = crate::Netlist::build(&circuit, &lib, View::Schematic);
    assert!(nl.nets.iter().any(|n| n.pins.iter().any(|r| r.part == "U1" && r.pin == "VCC") && n.wires.contains(&"w1".to_string())));
    assert!(nl.nets.iter().any(|n| n.pins.iter().any(|r| r.part == "U1" && r.pin == "OUT") && n.wires.contains(&"w2".to_string())));
    let svg = crate::svg::render_svg(&circuit, &lib, false, View::Schematic);
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

// ---------------------------------------------------------------------------------------
// Two views from one import

fn mini4(opts: &ImportOptions) -> (PartDef, Vec<String>) {
    import_fzp_file(&fixtures().join("mini4.fzp"), opts).unwrap()
}

#[test]
fn one_import_holds_both_views() {
    let (p, warns) = mini4(&ImportOptions::default());
    assert_eq!(p.ty, "mini_four");
    assert_eq!(p.name, "Mini Four");
    assert_eq!(p.category.as_deref(), Some("imported"));
    assert_eq!(ids(&p), ["A", "B", "C", "D"]);
    // Schematic: native shapes, {ref} text, dir toward the nearest edge, pins as in the SVG.
    let s = sch(&p);
    assert_eq!(s.size, [6.0, 4.0]);
    assert!(s.svg.is_none());
    for (id, at, dir) in [("A", [0, 1], Dir::Left), ("B", [0, 3], Dir::Left), ("C", [6, 1], Dir::Right), ("D", [6, 3], Dir::Right)] {
        let pp = pin(&p, View::Schematic, id);
        assert_eq!((pp.at, pp.dir), (at, dir), "schematic pin {id}");
    }
    assert!(s.shapes.iter().any(|x| matches!(x, Shape::Text { text, .. } if text == "{ref}")));
    assert!(s.shapes.iter().any(|x| matches!(x, Shape::Rect { .. })));
    // Breadboard: the art is kept, no shapes, pins at integer grid positions measured in the
    // SVG's own space (through the translated and the rotated groups), no dir.
    let b = bb(&p);
    assert_eq!(b.size, [6.0, 4.0]);
    assert!(b.shapes.is_empty());
    for (id, at) in [("A", [1, 1]), ("B", [1, 3]), ("C", [5, 1]), ("D", [5, 3])] {
        let pp = pin(&p, View::Breadboard, id);
        assert_eq!((pp.at, pp.dir), (at, Dir::default()), "breadboard pin {id}");
    }
    // The pins were 0.1 unit off the grid; the viewBox origin moved instead of the art.
    let svg = b.svg.as_deref().unwrap();
    assert!(svg.contains(r#"viewBox="1 1 60 40""#), "{svg}");
    assert!(svg.contains(r#"transform="translate(5 0)""#) && svg.contains("rotate(90)"), "art is untouched");
    assert!(svg.contains("linearGradient") && svg.contains("url(#grad)") && svg.contains("feGaussianBlur"), "gradients and filters stay");
    assert!(svg.contains(r##"href="#dot""##), "local references stay");
    // Buses: A+B joined; the single-member bus and the unknown member are dropped.
    assert_eq!(p.buses, vec![vec!["A".to_string(), "B".to_string()]]);
    assert!(warns.iter().any(|w| w.contains("connector9")), "{warns:?}");
}

#[test]
fn breadboard_svg_is_sanitised() {
    let (p, warns) = mini4(&ImportOptions::default());
    let svg = bb(&p).svg.as_deref().unwrap();
    assert!(!svg.contains("<script") && !svg.contains("alert"), "{svg}");
    assert!(!svg.contains("onclick"), "{svg}");
    assert!(!svg.contains("http://example.com") && !svg.contains("evil.example"), "{svg}");
    // The data URI image and the xmlns declarations are not external references.
    assert!(svg.contains("data:image/png;base64,iVBORw0KGgo="), "{svg}");
    assert!(svg.contains(r#"xmlns="http://www.w3.org/2000/svg""#), "{svg}");
    // The rest survives and the result still parses.
    assert!(svg.contains(r#"id="body""#) && svg.contains(r##"fill="url(#grad)""##));
    assert!(roxmltree::Document::parse(svg).is_ok());
    assert!(warns.iter().any(|w| w.contains("script")), "{warns:?}");
    assert!(warns.iter().any(|w| w.contains("event handler")), "{warns:?}");
    assert!(warns.iter().any(|w| w.contains("external")), "{warns:?}");
}

#[test]
fn only_the_requested_view_is_produced() {
    let (p, _) = mini4(&only(View::Breadboard));
    assert!(p.schematic.is_none());
    assert_eq!(bb(&p).pins.len(), 4);
    assert_eq!(ids(&p), ["A", "B", "C", "D"], "logical pins do not depend on the views");
    assert_eq!(p.buses.len(), 1);
    let (p, _) = mini4(&only(View::Schematic));
    assert!(p.breadboard.is_none());
    assert_eq!(sch(&p).pins.len(), 4);
    // Duplicates and order in the option do not matter.
    let both = ImportOptions { views: vec![View::Breadboard, View::Schematic, View::Breadboard], type_name: None };
    let (p, _) = mini4(&both);
    assert!(p.schematic.is_some() && p.breadboard.is_some());
}

#[test]
fn breadboard_family_is_a_board() {
    let (p, warns) = import_fzp_file(&fixtures().join("tinyboard.fzp"), &ImportOptions::default()).unwrap();
    assert_eq!(p.ty, "tiny_board");
    assert_eq!(p.category.as_deref(), Some("board"));
    assert!(p.is_board());
    assert_eq!(ids(&p), ["h1", "h2", "h3"]);
    assert_eq!(p.buses, vec![vec!["h1".to_string(), "h2".to_string()]]);
    assert_eq!(bb(&p).size, [4.0, 2.0]);
    for (i, id) in ["h1", "h2", "h3"].iter().enumerate() {
        assert_eq!(pin(&p, View::Breadboard, id).at, [i as i32 + 1, 1]);
        assert_eq!(pin(&p, View::Schematic, id).at, [i as i32 + 1, 1]);
    }
    assert!(bb(&p).svg.as_deref().unwrap().contains("#e8e0c8"));
    assert!(warns.is_empty(), "{warns:?}");
    // The family comparison ignores case; other families are not boards.
    assert_eq!(p.meta.as_ref().unwrap()["family"], "Breadboard");
    let (q, _) = mini4(&ImportOptions::default());
    assert!(!q.is_board());
}

#[test]
fn round_trip_through_the_canonical_writer() {
    for f in ["mini4.fzp", "tinyboard.fzp", "ic4.fzp"] {
        let (p, _) = import_fzp_file(&fixtures().join(f), &ImportOptions::default()).unwrap();
        let text = crate::write::part_to_string(&p);
        assert_eq!(part_json(&p).unwrap(), text);
        let back = crate::migrate::parse_part(&text).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&p).unwrap(), "{f}");
        assert_eq!(crate::write::part_to_string(&back), text, "{f}: writing again gives the same bytes");
    }
}

#[test]
fn breadboard_art_is_not_rewritten_when_pins_are_off_grid() {
    // 0.45 in wide, pin terminal at x = 1.55 units (user unit = 0.1 grid), y on the grid.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="0.5in" height="0.3in" viewBox="2 4 50 30">
 <rect x="2" y="4" width="50" height="30" fill="#123456"/>
 <rect id="t" x="17.5" y="18" width="2" height="2"/>
</svg>"##;
    let fzp = r#"<module moduleId="Off"><title>Off</title><views><breadboardView><layers image="bb.svg"/></breadboardView></views>
 <connectors><connector id="c0" name="P"><views><breadboardView><p svgId="t" terminalId="t"/></breadboardView></views></connector></connectors></module>"#;
    let (p, _) = import_fzp(fzp, &mut |_| Some(svg.as_bytes().to_vec()), &only(View::Breadboard)).unwrap();
    // The terminal centre is (18.5, 19); relative to the viewBox origin (2,4) that is (16.5, 15)
    // user units, so (1.65, 1.5) grid units. One pin: the mean error is the pin's, x snaps to 2.
    let pp = pin(&p, View::Breadboard, "P");
    assert_eq!(pp.at, [2, 2]);
    let out = bb(&p).svg.as_deref().unwrap();
    // Only the viewBox attribute differs from the input.
    let (a, b) = (out.find("viewBox=\"").unwrap(), svg.find("viewBox=\"").unwrap());
    assert_eq!(out[..a], svg[..b]);
    let tail_out = &out[a + 9..];
    let tail_in = &svg[b + 9..];
    assert_eq!(tail_out[tail_out.find('"').unwrap()..], tail_in[tail_in.find('"').unwrap()..]);
    // New origin: (2 - sx/s, 4 - sy/s) with s = 0.1 and the pin moving by (+0.35, +0.5) grid
    // units, i.e. (-3.5, -5) user units.
    let vb: Vec<f64> = tail_out[..tail_out.find('"').unwrap()].split(' ').map(|n| n.parse().unwrap()).collect();
    assert!((vb[0] - (2.0 - 3.5)).abs() < 1e-6 && (vb[1] - (4.0 - 5.0)).abs() < 1e-6, "{vb:?}");
    assert_eq!(&vb[2..], [50.0, 30.0]);
}

#[test]
fn a_missing_viewbox_is_added_so_the_art_fills_the_size() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="0.4in" height="0.2in">
 <circle id="p" cx="10" cy="10" r="2" fill="#cc0000"/></svg>"##;
    let fzp = r#"<module moduleId="Nv"><title>Nv</title><views><breadboardView><layers image="bb.svg"/></breadboardView></views>
 <connectors><connector id="c0" name="P"><views><breadboardView><p svgId="p"/></breadboardView></views></connector></connectors></module>"#;
    let (p, _) = import_fzp(fzp, &mut |_| Some(svg.as_bytes().to_vec()), &only(View::Breadboard)).unwrap();
    assert_eq!(bb(&p).size, [4.0, 2.0]);
    let out = bb(&p).svg.as_deref().unwrap();
    // Unit-less user units use 90 dpi: 0.4 in is 36 user units wide, 18 high.
    // The circle centre is 0.11 unit off the grid, so the origin moves by one user unit.
    assert!(out.contains(r#"viewBox="1 1 36 18""#), "{out}");
    assert_eq!(pin(&p, View::Breadboard, "P").at, [1, 1]);
}
