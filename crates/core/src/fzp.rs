//! Import of Fritzing parts (`.fzpz` bundles and loose `.fzp` + SVG files) as [`PartDef`]s.
//!
//! A `.fzp` is XML: `<title>`, `<properties>`, `<views>` (each view names its SVG through
//! `<layers image="schematic/foo.svg">`) and `<connectors>`, where every `<connector id name>`
//! lists, per view, the SVG element (`svgId`, optionally `terminalId`) that carries the pin.
//! The chosen view's SVG is converted by [`crate::svgimport::read_scene`] in *physical* mode
//! (real size from `width`/`height`/`viewBox`, 1 grid unit = 0.1 in); pin positions come from the
//! element bounding boxes.
//!
//! Pin position: centre of the `terminalId` element, else the *outer end* of the `svgId` element
//! when that element is clearly elongated (a pin line), else its centre. Positions are shifted
//! together so the average rounding error is zero, rounded to integers, and the whole drawing is
//! moved by a whole number of units so everything lies at x, y >= 0.
//!
//! Not handled: bus definitions, bendable legs, PCB view, multi-layer breadboard art, raster
//! images, gradients (flat fallback), filters, clip paths, masks, fonts (text keeps size/anchor).

use crate::model::{Anchor, PartDef, PinDef, Shape, Style, FORMAT};
use crate::svgimport::{self, Scene};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Which Fritzing view to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Schematic,
    Breadboard,
}

impl View {
    /// Element name of the view inside `<views>` and `<connector><views>`.
    fn tag(self) -> &'static str {
        match self {
            View::Schematic => "schematicView",
            View::Breadboard => "breadboardView",
        }
    }
    /// Folder name used by the Fritzing SVG layouts.
    pub fn folder(self) -> &'static str {
        match self {
            View::Schematic => "schematic",
            View::Breadboard => "breadboard",
        }
    }
    pub fn parse(s: &str) -> Option<View> {
        match s.trim().to_ascii_lowercase().as_str() {
            "schematic" => Some(View::Schematic),
            "breadboard" => Some(View::Breadboard),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub view: View,
    /// Part type name; derived from the title when `None`.
    pub type_name: Option<String>,
}

/// Parts beyond this size are not read (guards against zip bombs and absurd inputs).
const MAX_ENTRY: u64 = 32 * 1024 * 1024;
const WARN_BIG: f64 = 60.0;

type Loader<'a> = dyn FnMut(&str) -> Option<Vec<u8>> + 'a;

struct Conn {
    id: String,
    name: String,
    svg_id: Option<String>,
    term_id: Option<String>,
    leg_id: Option<String>,
}

fn text_of(n: roxmltree::Node) -> String {
    n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect::<String>().trim().to_string()
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Lowercase slug of letters, digits and underscores; empty when nothing usable is left.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let t = out.trim_matches('_');
    t.chars().take(48).collect::<String>().trim_matches('_').to_string()
}

fn parse_xml(xml: &str) -> Result<roxmltree::Document<'_>, String> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    roxmltree::Document::parse_with_options(xml.trim_start_matches('\u{feff}'), opts)
        .map_err(|e| format!("invalid XML: {e}"))
}

/// Import a `.fzp` document. `load_asset` returns the bytes of a file named as in the fzp's
/// `<layers image="...">`.
pub fn import_fzp(
    fzp_xml: &str,
    load_asset: &mut Loader<'_>,
    opts: &ImportOptions,
) -> Result<(PartDef, Vec<String>), String> {
    let mut warnings: Vec<String> = Vec::new();
    let doc = parse_xml(fzp_xml)?;
    let module = doc.root_element();
    if module.tag_name().name() != "module" {
        return Err("not a Fritzing part file (no <module> element)".into());
    }
    let module_id = module.attribute("moduleId").unwrap_or("").trim().to_string();
    let title = child(module, "title").map(text_of).unwrap_or_default();

    // Metadata.
    let mut meta: BTreeMap<String, String> = BTreeMap::new();
    meta.insert("source".into(), "fritzing".into());
    if !title.is_empty() {
        meta.insert("title".into(), title.clone());
    }
    if !module_id.is_empty() {
        meta.insert("module_id".into(), module_id.clone());
    }
    for key in ["label", "description"] {
        if let Some(t) = child(module, key).map(text_of).filter(|t| !t.is_empty()) {
            meta.insert(key.into(), t);
        }
    }
    if let Some(tags) = child(module, "tags") {
        let v: Vec<String> = tags.children().filter(|c| c.is_element()).map(text_of).filter(|t| !t.is_empty()).collect();
        if !v.is_empty() {
            meta.insert("tags".into(), v.join(", "));
        }
    }
    if let Some(props) = child(module, "properties") {
        for p in props.children().filter(|c| c.is_element() && c.tag_name().name() == "property") {
            let (Some(name), v) = (p.attribute("name"), text_of(p)) else { continue };
            let name = name.trim().to_ascii_lowercase();
            if !name.is_empty() && !v.is_empty() {
                meta.entry(name).or_insert(v);
            }
        }
    }

    // The chosen view and its SVG.
    let view = opts.view;
    let views = child(module, "views").ok_or("part has no <views>")?;
    let view_el = child(views, view.tag()).ok_or_else(|| format!("part has no {} (try --view)", view.tag()))?;
    let image = child(view_el, "layers")
        .and_then(|l| l.attribute("image"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{} names no SVG image", view.tag()))?;
    let svg_bytes = load_asset(&image).ok_or_else(|| format!("SVG file not found: {image}"))?;
    let svg = String::from_utf8_lossy(&svg_bytes).into_owned();
    let scene = svgimport::read_scene(&svg, true).map_err(|e| format!("{image}: {e}"))?;
    warnings.extend(scene.warnings.iter().cloned());
    if scene.shapes.is_empty() {
        return Err(format!("{image}: no drawable shapes found"));
    }

    // Connectors present in this view.
    let mut conns: Vec<Conn> = Vec::new();
    if let Some(cs) = child(module, "connectors") {
        for c in cs.children().filter(|c| c.is_element() && c.tag_name().name() == "connector") {
            let id = c.attribute("id").unwrap_or("").trim().to_string();
            let name = c.attribute("name").unwrap_or("").trim().to_string();
            let p = child(c, "views")
                .and_then(|v| child(v, view.tag()))
                .and_then(|v| v.children().find(|x| x.is_element() && x.tag_name().name() == "p"));
            let Some(p) = p else {
                warnings.push(format!("connector {id} has no {}; skipped", view.tag()));
                continue;
            };
            let at = |k: &str| p.attribute(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            conns.push(Conn { id, name, svg_id: at("svgId"), term_id: at("terminalId"), leg_id: at("legId") });
        }
    }
    if conns.is_empty() {
        warnings.push(format!("no connectors in the {}; the part has no pins", view.tag()));
    }

    // Pin positions in grid units (unshifted scene coordinates).
    let scene_bb = svgimport::shapes_bbox(&scene.shapes).ok_or("SVG has no geometry")?;
    let centre = [(scene_bb[0] + scene_bb[2]) / 2.0, (scene_bb[1] + scene_bb[3]) / 2.0];
    let mut used_ids: BTreeSet<String> = BTreeSet::new();
    let mut raw_pins: Vec<(String, Option<String>, [f64; 2])> = Vec::new();
    for c in &conns {
        let mut id = if c.name.is_empty() { c.id.clone() } else { c.name.clone() };
        if id.is_empty() || used_ids.contains(&id) {
            id = if c.id.is_empty() { format!("pin{}", raw_pins.len() + 1) } else { c.id.clone() };
        }
        let mut unique = id.clone();
        let mut n = 2;
        while used_ids.contains(&unique) {
            unique = format!("{id}_{n}");
            n += 1;
        }
        if unique != c.name && !c.name.is_empty() {
            warnings.push(format!("pin name {:?} is not unique or usable; using id {unique:?}", c.name));
        }
        let Some(pos) = locate_pin(&scene, c, centre, &mut warnings) else {
            warnings.push(format!("pin {unique}: element not found in {image}; pin skipped"));
            continue;
        };
        used_ids.insert(unique.clone());
        let display = if unique != c.name && !c.name.is_empty() { Some(c.name.clone()) } else { None };
        raw_pins.push((unique, display, pos));
    }

    // Snap: shift everything so pins land on integers with zero mean error, then round.
    // Axes whose rounding errors are all tiny (< 0.08 unit = 0.008 in) are left alone.
    let snap = |axis: usize| -> f64 {
        let errs: Vec<f64> = raw_pins.iter().map(|p| p.2[axis].round() - p.2[axis]).collect();
        if errs.iter().all(|e| e.abs() < 0.08) {
            0.0
        } else {
            errs.iter().sum::<f64>() / errs.len() as f64
        }
    };
    let (sx, sy) = if raw_pins.is_empty() { (0.0, 0.0) } else { (snap(0), snap(1)) };
    let mut pts: Vec<[i32; 2]> = raw_pins.iter().map(|p| [(p.2[0] + sx).round() as i32, (p.2[1] + sy).round() as i32]).collect();
    let mut taken: BTreeSet<[i32; 2]> = BTreeSet::new();
    for (i, p) in pts.iter_mut().enumerate() {
        if taken.contains(p) {
            let orig = *p;
            'search: for r in 1..6i32 {
                for d in [[r, 0], [-r, 0], [0, r], [0, -r]] {
                    let q = [orig[0] + d[0], orig[1] + d[1]];
                    if !taken.contains(&q) {
                        *p = q;
                        break 'search;
                    }
                }
            }
            warnings.push(format!(
                "pins landed on the same point ({},{}); pin {} moved to ({},{})",
                orig[0], orig[1], raw_pins[i].0, p[0], p[1]
            ));
        }
        taken.insert(*p);
    }
    // Whole-unit translation so all geometry is at x, y >= 0.
    let mut min = [scene_bb[0] + sx, scene_bb[1] + sy];
    for p in &pts {
        min[0] = min[0].min(p[0] as f64);
        min[1] = min[1].min(p[1] as f64);
    }
    // Geometry up to 0.4 unit left of / above the origin (round caps, pin stubs) stays put.
    let (tx, ty) = ((-(min[0] + 0.4).floor()) as i32, (-(min[1] + 0.4).floor()) as i32);
    let (dx, dy) = (sx + tx as f64, sy + ty as f64);
    let mut shapes: Vec<Shape> = scene.shapes.into_iter().map(|s| svgimport::translate_shape(s, dx, dy)).collect();
    for p in &mut pts {
        p[0] += tx;
        p[1] += ty;
    }
    let mut max = [scene_bb[2] + dx, scene_bb[3] + dy];
    for p in &pts {
        max[0] = max[0].max(p[0] as f64);
        max[1] = max[1].max(p[1] as f64);
    }
    // Overshoot of up to 0.1 unit (stroke caps) does not grow the box.
    let w = (max[0] - 0.1).ceil().max(1.0);
    let h = (max[1] - 0.1).ceil().max(1.0);
    if w > WARN_BIG || h > WARN_BIG {
        warnings.push(format!("size {w}x{h} grid units is unusually large (0.1 in each); check the SVG units"));
    }
    if w < 1.5 && h < 1.5 {
        warnings.push(format!("size {w}x{h} grid units is unusually small; check the SVG units"));
    }
    if let Some(d) = scene.declared_in {
        let (dw, dh) = (d[0] * 10.0, d[1] * 10.0);
        if (dw - w).abs() > 4.0 || (dh - h).abs() > 4.0 {
            warnings.push(format!("drawing extends well beyond the declared SVG size ({dw:.1}x{dh:.1} units)"));
        }
    } else {
        warnings.push(format!("SVG declares no physical size; assumed {} dpi", scene.dpi));
    }

    let pins: Vec<PinDef> = raw_pins
        .iter()
        .zip(&pts)
        .map(|(rp, at)| PinDef { id: rp.0.clone(), at: *at, dir: svgimport::infer_dir(*at, w, h), name: rp.1.clone() })
        .collect();

    shapes.push(Shape::Text {
        at: [w / 2.0, -0.8],
        text: "{ref}".into(),
        size: 0.9,
        anchor: Anchor::Middle,
        style: Style::default(),
    });

    let ty = match opts.type_name.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            let s = slug(&title);
            let s = if s.is_empty() { slug(&module_id) } else { s };
            if s.is_empty() {
                "fritzing_part".to_string()
            } else {
                s
            }
        }
    };
    let name = if title.is_empty() { ty.clone() } else { title.clone() };
    let part = PartDef {
        format: FORMAT.to_string(),
        ty,
        name,
        category: Some("imported".to_string()),
        size: [w, h],
        pins,
        shapes,
        params: None,
        svg: None,
        meta: Some(meta),
    };
    Ok((part, warnings))
}

/// Where the pin attaches, in scene coordinates.
fn locate_pin(scene: &Scene, c: &Conn, centre: [f64; 2], warnings: &mut Vec<String>) -> Option<[f64; 2]> {
    if let Some(t) = &c.term_id {
        if let Some(bb) = scene.ids.get(t) {
            return Some([(bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0]);
        }
        warnings.push(format!("terminal element {t:?} not found; using the pin element"));
    }
    let (bb, via) = match (&c.svg_id, &c.leg_id) {
        (Some(s), _) if scene.ids.contains_key(s) => (scene.ids[s], false),
        (_, Some(l)) if scene.ids.contains_key(l) => (scene.ids[l], true),
        _ => return None,
    };
    let (w, h) = (bb[2] - bb[0], bb[3] - bb[1]);
    let mid = [(bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0];
    let far = |lo: f64, hi: f64, c: f64| if (lo - c).abs() > (hi - c).abs() { lo } else { hi };
    let p = if via {
        // A leg: its end is the free tip.
        if w >= h {
            [far(bb[0], bb[2], centre[0]), mid[1]]
        } else {
            [mid[0], far(bb[1], bb[3], centre[1])]
        }
    } else if w > 0.3 && w >= 1.5 * h {
        [far(bb[0], bb[2], centre[0]), mid[1]]
    } else if h > 0.3 && h >= 1.5 * w {
        [mid[0], far(bb[1], bb[3], centre[1])]
    } else {
        mid
    };
    Some(p)
}

// ---------------------------------------------------------------------------------------
// Bundles and files

/// Candidate archive names for an image path such as `schematic/foo.svg`.
fn candidates(image: &str) -> Vec<String> {
    let img = image.replace('\\', "/").trim_start_matches('/').to_ascii_lowercase();
    let dotted = img.replace('/', ".");
    vec![
        img.clone(),
        format!("svg.{dotted}"),
        format!("svg.core.{dotted}"),
        format!("core/{img}"),
        format!("svg/core/{img}"),
        format!("svg/{img}"),
        dotted,
    ]
}

/// Index of the archive entry that holds `image`, tolerating the flat `svg.schematic.foo.svg`
/// naming of `.fzpz` files and a few directory layouts.
fn find_entry(names: &[String], image: &str) -> Option<usize> {
    let lower: Vec<String> = names.iter().map(|n| n.replace('\\', "/").to_ascii_lowercase()).collect();
    for c in candidates(image) {
        if let Some(i) = lower.iter().position(|n| *n == c) {
            return Some(i);
        }
    }
    let img = image.replace('\\', "/").to_ascii_lowercase();
    let base = img.rsplit('/').next().unwrap_or(&img).to_string();
    let first = img.split('/').next().filter(|_| img.contains('/')).unwrap_or("").to_string();
    let hits: Vec<usize> = (0..lower.len())
        .filter(|&i| {
            let n = &lower[i];
            *n == base || n.ends_with(&format!("/{base}")) || n.ends_with(&format!(".{base}"))
        })
        .collect();
    hits.iter().copied().find(|&i| !first.is_empty() && lower[i].contains(&first)).or_else(|| hits.first().copied())
}

fn read_limited<R: Read>(r: R) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    r.take(MAX_ENTRY).read_to_end(&mut v).ok()?;
    Some(v)
}

/// Import a `.fzpz` bundle (a zip holding one `.fzp` and its SVG files).
pub fn import_fzpz(bytes: &[u8], opts: &ImportOptions) -> Result<(PartDef, Vec<String>), String> {
    let mut za = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a valid .fzpz (zip) file: {e}"))?;
    let names: Vec<String> = za.file_names().map(str::to_string).collect();
    let fzp_names: Vec<&String> = names.iter().filter(|n| n.to_ascii_lowercase().ends_with(".fzp")).collect();
    let fzp_name = fzp_names.first().ok_or("the bundle contains no .fzp file")?.to_string();
    let xml_bytes = {
        let f = za.by_name(&fzp_name).map_err(|e| format!("cannot read {fzp_name}: {e}"))?;
        read_limited(f).ok_or_else(|| format!("cannot read {fzp_name}"))?
    };
    let xml = String::from_utf8_lossy(&xml_bytes).into_owned();
    let mut loader = |image: &str| -> Option<Vec<u8>> {
        let i = find_entry(&names, image)?;
        read_limited(za.by_index(i).ok()?)
    };
    let (part, mut warnings) = import_fzp(&xml, &mut loader, opts)?;
    if fzp_names.len() > 1 {
        warnings.push(format!("bundle holds {} .fzp files; imported {fzp_name}", fzp_names.len()));
    }
    Ok((part, warnings))
}

/// Import a loose `.fzp`. The SVG is looked up relative to the fzp, its parent folders and the
/// `svg/<core|contrib|user|obsolete>/` layout of the fritzing-parts repository.
pub fn import_fzp_file(path: &Path, opts: &ImportOptions) -> Result<(PartDef, Vec<String>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let xml = String::from_utf8_lossy(&bytes).into_owned();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut loader = |image: &str| -> Option<Vec<u8>> {
        for p in asset_paths(&dir, image) {
            if let Ok(b) = std::fs::read(&p) {
                return Some(b);
            }
        }
        None
    };
    import_fzp(&xml, &mut loader, opts)
}

fn asset_paths(dir: &Path, image: &str) -> Vec<PathBuf> {
    let img = image.replace('\\', "/");
    let img = img.trim_start_matches('/');
    let base = img.rsplit('/').next().unwrap_or(img);
    let dotted = img.replace('/', ".");
    let mut out = vec![dir.join(img), dir.join(format!("svg.{dotted}")), dir.join(base)];
    let mut cur = Some(dir.to_path_buf());
    for _ in 0..4 {
        let Some(d) = cur.take() else { break };
        for sub in ["", "svg/core", "svg/contrib", "svg/user", "svg/obsolete", "svg"] {
            let p = if sub.is_empty() { d.join(img) } else { d.join(sub).join(img) };
            out.push(p);
        }
        cur = d.parent().map(Path::to_path_buf);
    }
    out
}

/// Import by file extension: `.fzpz` or `.fzp`.
pub fn import_path(path: &Path, opts: &ImportOptions) -> Result<(PartDef, Vec<String>), String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        "fzpz" => {
            let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            import_fzpz(&bytes, opts)
        }
        "fzp" => import_fzp_file(path, opts),
        _ => Err(format!("{}: expected a .fzpz or .fzp file", path.display())),
    }
}

/// The `.fzpz` and `.fzp` files directly inside `dir`, sorted. A `.fzp` is skipped when a
/// `.fzpz` with the same stem exists.
pub fn list_fritzing_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut all: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
    all.sort();
    let ext = |p: &Path| p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let zips: BTreeSet<_> = all.iter().filter(|p| ext(p) == "fzpz").map(|p| p.with_extension("")).collect();
    all.into_iter()
        .filter(|p| match ext(p).as_str() {
            "fzpz" => true,
            "fzp" => !zips.contains(&p.with_extension("")),
            _ => false,
        })
        .collect()
}

/// Pretty printed `.part` JSON with a trailing newline.
pub fn part_json(part: &PartDef) -> Result<String, String> {
    serde_json::to_string_pretty(part).map(|s| s + "\n").map_err(|e| format!("cannot serialise part: {e}"))
}

/// Import `input` and write `<type>.part` into `out_dir` (created if needed). Returns the part,
/// the warnings and the written path.
pub fn import_to_dir(input: &Path, out_dir: &Path, opts: &ImportOptions) -> Result<(PartDef, Vec<String>, PathBuf), String> {
    let (part, warnings) = import_path(input, opts)?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let out = out_dir.join(format!("{}.part", part.ty));
    std::fs::write(&out, part_json(&part)?).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok((part, warnings, out))
}

#[cfg(test)]
mod tests;
