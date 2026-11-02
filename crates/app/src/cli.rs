//! Command line parsing and the non-GUI modes (`--validate`, `--render`, `--netlist`,
//! `--render-part`, `--upgrade`, the importers and the `--list-*` commands).

use crate::core_api;
use short_circ_core::{fzp, svgimport, write, Circuit, Library, PartDef, Report, Severity, View};
use std::path::{Path, PathBuf};

/// `(id, x, y)` pins as taken by `svgimport::import_svg`.
pub type PinSpec = Vec<(String, i32, i32)>;

/// Which views `--validate` checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSel {
    /// Every view in use plus the cross-view checks.
    All,
    One(View),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Gui(Option<PathBuf>),
    Validate { file: PathBuf, json: bool, view: ViewSel },
    Render { file: PathBuf, out: PathBuf, dark: bool, view: View },
    ImportSvg { input: PathBuf, out: PathBuf, ty: String, pins: PinSpec, scale: Option<f64> },
    Netlist { file: PathBuf, json: bool, view: View },
    RenderPart { ty: String, out: PathBuf, dark: bool, circuit_dir: Option<PathBuf>, view: View },
    ImportFritzing { input: PathBuf, out: PathBuf, views: Vec<View>, ty: Option<String> },
    Upgrade { input: PathBuf, output: Option<PathBuf> },
    ListParts,
    ListColors,
    Help,
    Version,
}

pub const HELP: &str = "\
short-circ - circuit diagram viewer

USAGE:
  short-circ [FILE.circ]                      open the viewer
  short-circ --validate FILE.circ [--view schematic|breadboard|all] [--json]
                                              check a circuit (exit 0 ok, 1 errors, 2 unreadable);
                                              default view: all (every view in use + cross-view checks)
  short-circ --render FILE.circ OUT.svg [--view schematic|breadboard] [--dark]
                                              export an SVG image (default view: schematic)
  short-circ --import-svg IN.svg OUT.part --type NAME [--pins SPEC] [--scale PX]
                                              import SVG artwork as a component file
                                              SPEC is id:x,y entries separated by ; (grid units),
                                              PX is pixels per grid unit (default 10)
  short-circ --netlist FILE.circ [--view schematic|breadboard] [--json]
                                              print each net: id, name, pins (R1.a), wire ids
                                              (default view: schematic; exit 0 ok, 2 unreadable)
  short-circ --render-part TYPE OUT.svg [--view schematic|breadboard] [--dark] [--circuit-dir DIR]
                                              draw one part view with its pin ids labelled
                                              (exit 1 if TYPE is unknown or lacks the view;
                                              dipN/headerN work; default view: schematic)
  short-circ --upgrade IN [OUT]               rewrite a .circ or .part (format v1 or v2) as canonical
                                              v2, in place when OUT is omitted (exit 0 ok, 2 error)
  short-circ --import-fritzing IN OUT [--views schematic,breadboard] [--type NAME]
                                              import a Fritzing part: IN is a .fzpz, a .fzp or a folder
                                              of them, OUT a .part file or (for folders) a folder;
                                              default views: both
  short-circ --list-parts                     list available component types and their views
  short-circ --list-colors                    list preset colour names and hex codes
  short-circ --help | --version

Viewer: drag to pan, wheel to zoom, F to fit, Schematic / Breadboard tabs switch the view,
drop a .circ file onto the window to open it.
";

pub fn version_line() -> String {
    format!("short-circ {}", env!("CARGO_PKG_VERSION"))
}

/// `--view VALUE`: `schematic`, `breadboard` (short forms `sch` / `bb`) or `all`.
fn parse_view_sel(v: &str) -> Result<ViewSel, String> {
    if v.trim().eq_ignore_ascii_case("all") {
        return Ok(ViewSel::All);
    }
    View::parse(v)
        .map(ViewSel::One)
        .ok_or_else(|| format!("--view must be schematic, breadboard or all, got {v:?}"))
}

/// `--view VALUE` where `all` makes no sense.
fn parse_one_view(v: Option<&str>, flag: &str) -> Result<View, String> {
    match v {
        None => Ok(View::Schematic),
        Some(v) => match parse_view_sel(v)? {
            ViewSel::One(v) => Ok(v),
            ViewSel::All => Err(format!("{flag} all only applies to --validate")),
        },
    }
}

/// `--views a,b`: a comma separated, non-empty list of views, without repeats.
fn parse_views(v: &str) -> Result<Vec<View>, String> {
    let mut out: Vec<View> = Vec::new();
    for part in v.split(',') {
        let view = View::parse(part).ok_or_else(|| format!("--views takes schematic and/or breadboard, got {part:?}"))?;
        if !out.contains(&view) {
            out.push(view);
        }
    }
    Ok(out)
}

/// Parse arguments (without the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut mode: Option<&str> = None;
    let mut positional: Vec<PathBuf> = Vec::new();
    let (mut json, mut dark) = (false, false);
    let (mut ty, mut pins, mut scale) = (None::<String>, None::<String>, None::<String>);
    let mut circuit_dir = None::<String>;
    let mut view = None::<String>;
    let mut views = None::<String>;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-V" => return Ok(Command::Version),
            "--list-parts" => set_mode(&mut mode, "list-parts")?,
            "--list-colors" => set_mode(&mut mode, "list-colors")?,
            "--validate" => set_mode(&mut mode, "validate")?,
            "--render" => set_mode(&mut mode, "render")?,
            "--netlist" => set_mode(&mut mode, "netlist")?,
            "--render-part" => set_mode(&mut mode, "render-part")?,
            "--import-svg" => set_mode(&mut mode, "import-svg")?,
            "--import-fritzing" => set_mode(&mut mode, "import-fritzing")?,
            "--upgrade" => set_mode(&mut mode, "upgrade")?,
            "--view" => view = Some(take_value(&mut it, "--view")?),
            "--views" => views = Some(take_value(&mut it, "--views")?),
            "--json" => json = true,
            "--dark" => dark = true,
            "--type" => ty = Some(take_value(&mut it, "--type")?),
            "--pins" => pins = Some(take_value(&mut it, "--pins")?),
            "--scale" => scale = Some(take_value(&mut it, "--scale")?),
            "--circuit-dir" => circuit_dir = Some(take_value(&mut it, "--circuit-dir")?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => positional.push(PathBuf::from(a)),
        }
    }
    let import_flags = ty.is_some() || pins.is_some() || scale.is_some();
    if circuit_dir.is_some() && mode != Some("render-part") {
        return Err("--circuit-dir only applies to --render-part".into());
    }
    if view.is_some() && !matches!(mode, Some("validate" | "render" | "netlist" | "render-part")) {
        return Err("--view only applies to --validate, --render, --netlist and --render-part".into());
    }
    if views.is_some() && mode != Some("import-fritzing") {
        return Err("--views only applies to --import-fritzing".into());
    }
    match mode {
        None => {
            if json || dark {
                return Err("--json needs --validate or --netlist and --dark needs --render".into());
            }
            if import_flags {
                return Err("--type, --pins and --scale need --import-svg".into());
            }
            match positional.len() {
                0 => Ok(Command::Gui(None)),
                1 => Ok(Command::Gui(positional.pop())),
                _ => Err("expected at most one file".into()),
            }
        }
        Some("list-parts") => {
            if !positional.is_empty() || json || dark || import_flags {
                return Err("--list-parts takes no other arguments".into());
            }
            Ok(Command::ListParts)
        }
        Some("list-colors") => {
            if !positional.is_empty() || json || dark || import_flags {
                return Err("--list-colors takes no other arguments".into());
            }
            Ok(Command::ListColors)
        }
        Some("upgrade") => {
            if json || dark || import_flags {
                return Err("--upgrade takes only IN and an optional OUT".into());
            }
            match positional.len() {
                1 => Ok(Command::Upgrade { input: positional.remove(0), output: None }),
                2 => {
                    let output = positional.pop();
                    Ok(Command::Upgrade { input: positional.remove(0), output })
                }
                _ => Err("--upgrade needs IN and an optional OUT".into()),
            }
        }
        Some("validate") => {
            if dark {
                return Err("--dark only applies to --render".into());
            }
            if import_flags {
                return Err("--type, --pins and --scale only apply to --import-svg".into());
            }
            if positional.len() != 1 {
                return Err("--validate needs exactly one FILE".into());
            }
            let sel = view.as_deref().map(parse_view_sel).transpose()?.unwrap_or(ViewSel::All);
            Ok(Command::Validate { file: positional.remove(0), json, view: sel })
        }
        Some("import-svg") => {
            if json || dark {
                return Err("--json and --dark do not apply to --import-svg".into());
            }
            if positional.len() != 2 {
                return Err("--import-svg needs IN.svg and OUT.part".into());
            }
            let ty = ty
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .ok_or("--import-svg needs --type NAME")?;
            let pins = match pins {
                Some(spec) => parse_pins(&spec)?,
                None => Vec::new(),
            };
            let scale = scale.map(|s| parse_scale(&s)).transpose()?;
            let out = positional.remove(1);
            Ok(Command::ImportSvg { input: positional.remove(0), out, ty, pins, scale })
        }
        Some("netlist") => {
            if dark {
                return Err("--dark does not apply to --netlist".into());
            }
            if import_flags {
                return Err("--type, --pins and --scale only apply to --import-svg".into());
            }
            if positional.len() != 1 {
                return Err("--netlist needs exactly one FILE".into());
            }
            let view = parse_one_view(view.as_deref(), "--view")?;
            Ok(Command::Netlist { file: positional.remove(0), json, view })
        }
        Some("render-part") => {
            if json {
                return Err("--json does not apply to --render-part".into());
            }
            if import_flags {
                return Err("--type, --pins and --scale only apply to --import-svg".into());
            }
            if positional.len() != 2 {
                return Err("--render-part needs TYPE and OUT.svg".into());
            }
            let view = parse_one_view(view.as_deref(), "--view")?;
            let ty = positional.remove(0).to_string_lossy().into_owned();
            let out = positional.remove(0);
            Ok(Command::RenderPart { ty, out, dark, circuit_dir: circuit_dir.map(PathBuf::from), view })
        }
        Some("import-fritzing") => {
            if json || dark || pins.is_some() || scale.is_some() {
                return Err("--import-fritzing only takes --views and --type".into());
            }
            if positional.len() != 2 {
                return Err("--import-fritzing needs IN and OUT".into());
            }
            let views = match views {
                Some(v) => parse_views(&v)?,
                None => View::ALL.to_vec(),
            };
            let ty = ty.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
            let out = positional.remove(1);
            Ok(Command::ImportFritzing { input: positional.remove(0), out, views, ty })
        }
        Some(_) => {
            if json {
                return Err("--json only applies to --validate and --netlist".into());
            }
            if import_flags {
                return Err("--type, --pins and --scale only apply to --import-svg".into());
            }
            if positional.len() != 2 {
                return Err("--render needs FILE and OUT.svg".into());
            }
            let view = parse_one_view(view.as_deref(), "--view")?;
            let out = positional.remove(1);
            Ok(Command::Render { file: positional.remove(0), out, dark, view })
        }
    }
}

/// Value of a flag that takes one argument (the following word).
fn take_value(it: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    it.next().cloned().ok_or_else(|| format!("{flag} needs a value"))
}

/// `--scale`: pixels per grid unit, a positive finite number.
fn parse_scale(s: &str) -> Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => Ok(v),
        _ => Err(format!("--scale needs a positive number of pixels per grid unit, got {s:?}")),
    }
}

/// `--pins`: `id:x,y` entries separated by `;`. Ids are non-empty and unique, coordinates are
/// integers in grid units. An empty spec means no pins.
pub fn parse_pins(spec: &str) -> Result<PinSpec, String> {
    if spec.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut pins: PinSpec = Vec::new();
    for entry in spec.split(';') {
        let (id, xy) = entry
            .split_once(':')
            .ok_or_else(|| format!("pin {entry:?} must look like id:x,y"))?;
        let id = id.trim();
        if id.is_empty() {
            return Err(format!("pin {entry:?} has an empty id"));
        }
        let (x, y) = xy.split_once(',').ok_or_else(|| format!("pin {id} must look like {id}:x,y"))?;
        let coord = |v: &str| {
            v.trim()
                .parse::<i32>()
                .map_err(|_| format!("pin {id} has a non-integer coordinate {:?}", v.trim()))
        };
        let (x, y) = (coord(x)?, coord(y)?);
        if pins.iter().any(|p| p.0 == id) {
            return Err(format!("duplicate pin id {id}"));
        }
        pins.push((id.to_string(), x, y));
    }
    Ok(pins)
}

fn set_mode<'a>(mode: &mut Option<&'a str>, new: &'a str) -> Result<(), String> {
    match mode {
        Some(old) if *old != new => Err(format!("--{old} and --{new} cannot be combined")),
        _ => {
            *mode = Some(new);
            Ok(())
        }
    }
}

/// Run a non-GUI command and return the process exit code.
pub fn run(cmd: Command) -> i32 {
    match cmd {
        Command::Gui(_) => 0,
        Command::Help => {
            print!("{HELP}");
            0
        }
        Command::Version => {
            println!("{}", version_line());
            0
        }
        Command::ListParts => {
            let lib = Library::load(&Circuit::default(), None);
            print!("{}", list_parts_text(&lib));
            0
        }
        Command::ListColors => {
            for (name, c) in short_circ_core::color::preset_names() {
                println!("{name:<8} {}", c.to_hex());
            }
            0
        }
        Command::Validate { file, json, view } => validate(&file, json, view),
        Command::Render { file, out, dark, view } => render(&file, &out, dark, view),
        Command::Netlist { file, json, view } => crate::netlist_out::run(&file, json, view),
        Command::RenderPart { ty, out, dark, circuit_dir, view } => {
            crate::partview::run(&ty, &out, dark, circuit_dir.as_deref(), view)
        }
        Command::Upgrade { input, output } => upgrade(&input, output.as_deref()),
        Command::ImportSvg { input, out, ty, pins, scale } => {
            import_svg_file(&input, &out, &ty, &pins, scale)
        }
        Command::ImportFritzing { input, out, views, ty } => import_fritzing(&input, &out, &views, ty.as_deref()),
    }
}

/// One line per type: `resistor  schematic+breadboard`.
pub fn list_parts_text(lib: &Library) -> String {
    let mut s = String::new();
    for t in lib.types() {
        let views = lib.get(t).map(core_api::view_names).unwrap_or_else(|| "-".into());
        s.push_str(&format!("{t}  {views}\n"));
    }
    s
}

fn validate(file: &Path, json: bool, view: ViewSel) -> i32 {
    let circuit = match core_api::load_circuit(file) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("error: {msg}");
            if json {
                let issue = short_circ_core::Issue {
                    rule: "schema".into(),
                    severity: Severity::Error,
                    ids: vec![],
                    at: None,
                    message: msg,
                    view: None,
                };
                println!("{}", report_json(&Report::from_issues(vec![issue])));
            }
            return 2;
        }
    };
    let lib = Library::load(&circuit, file.parent());
    let report = match view {
        ViewSel::All => core_api::validate(&circuit, &lib),
        ViewSel::One(v) => short_circ_core::validate_view(&circuit, &lib, v),
    };
    if json {
        println!("{}", report_json(&report));
    } else {
        print!("{}", report_text(file, &report));
    }
    if report.errors > 0 {
        1
    } else {
        0
    }
}

fn report_json(r: &Report) -> String {
    serde_json::to_string(r).unwrap_or_else(|_| "{}".to_string())
}

/// Human readable report.
pub fn report_text(file: &Path, r: &Report) -> String {
    let mut s = format!("{}: {} error(s), {} warning(s)\n", file.display(), r.errors, r.warnings);
    for i in &r.issues {
        let sev = if i.severity == Severity::Error { "error" } else { "warning" };
        let at = i.at.map(|p| format!(" at ({},{})", p[0], p[1])).unwrap_or_default();
        s.push_str(&format!("  {sev:<7} {} [{}]{at}: {}\n", i.rule, i.ids.join(", "), i.message));
    }
    s
}

fn render(file: &Path, out: &Path, dark: bool, view: View) -> i32 {
    let circuit = match core_api::load_circuit(file) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let lib = Library::load(&circuit, file.parent());
    match core_api::render_svg(&circuit, &lib, dark, view) {
        Ok(svg) => match std::fs::write(out, svg) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: cannot write {}: {e}", out.display());
                1
            }
        },
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// `--upgrade IN [OUT]`: rewrite a v1 or v2 `.circ` / `.part` as canonical v2. The kind comes
/// from the extension of `IN`. Exit 0 written, 2 unreadable input, unknown kind or write error.
pub fn upgrade(input: &Path, output: Option<&Path>) -> i32 {
    let ext = input.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let src = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", input.display());
            return 2;
        }
    };
    let text = match ext.as_str() {
        "circ" => short_circ_core::parse_circuit(&src).map(|c| write::circuit_to_string(&c)),
        "part" => short_circ_core::parse_part(&src).map(|p| write::part_to_string(&p)),
        _ => {
            eprintln!("error: {}: --upgrade handles .circ and .part files", input.display());
            return 2;
        }
    };
    let text = match text {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {}: {e}", input.display());
            return 2;
        }
    };
    let dest = output.unwrap_or(input);
    match std::fs::write(dest, text) {
        Ok(()) => {
            println!("upgraded {} -> {}", input.display(), dest.display());
            0
        }
        Err(e) => {
            eprintln!("error: cannot write {}: {e}", dest.display());
            2
        }
    }
}

/// Import SVG text as a part. Uses `import_svg_scaled` when a scale was given.
fn import_part(svg: &str, ty: &str, pins: &[(String, i32, i32)], scale: Option<f64>) -> Result<PartDef, String> {
    match scale {
        Some(px) => svgimport::import_svg_scaled(svg, ty, pins, px),
        None => svgimport::import_svg(svg, ty, pins),
    }
}

/// One-line summary of an SVG import: type, schematic size in grid units, pin count, output path.
fn import_svg_summary(part: &PartDef, out: &Path) -> String {
    let size = part.schematic.as_ref().map_or([0.0, 0.0], |v| v.size);
    format!("{}: size {}x{}, {} pin(s) -> {}", part.ty, size[0], size[1], part.pins.len(), out.display())
}

/// One-line summary of a Fritzing import: `type: views schematic+breadboard, N pin(s) -> path`.
fn import_summary(part: &PartDef, out: &Path) -> String {
    format!("{}: views {}, {} pin(s) -> {}", part.ty, core_api::view_names(part), part.pins.len(), out.display())
}

/// Exit codes: 0 written, 1 import or write error, 2 unreadable input.
fn import_svg_file(input: &Path, out: &Path, ty: &str, pins: &[(String, i32, i32)], scale: Option<f64>) -> i32 {
    let svg = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", input.display());
            return 2;
        }
    };
    let result = import_part(&svg, ty, pins, scale).and_then(|part| {
        std::fs::write(out, write::part_to_string(&part)).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
        Ok(part)
    });
    match result {
        Ok(part) => {
            println!("{}", import_svg_summary(&part, out));
            0
        }
        Err(msg) => {
            eprintln!("error: {msg}");
            1
        }
    }
}

/// `--import-fritzing`. Exit codes: 0 all written, 1 an import or write error, 2 bad arguments.
fn import_fritzing(input: &Path, out: &Path, views: &[View], ty: Option<&str>) -> i32 {
    let opts = fzp::ImportOptions { views: views.to_vec(), type_name: ty.map(str::to_string) };
    let show = |name: &str, part: &PartDef, warnings: &[String], to: &Path| {
        println!("{}", import_summary(part, to));
        for w in warnings {
            eprintln!("warning: {name}: {w}");
        }
    };
    if input.is_dir() {
        if ty.is_some() {
            eprintln!("error: --type cannot be used when importing a folder");
            return 2;
        }
        if out.extension().is_some_and(|e| e.eq_ignore_ascii_case("part")) {
            eprintln!("error: OUT must be a folder when IN is a folder");
            return 2;
        }
        let files = fzp::list_fritzing_files(input);
        if files.is_empty() {
            eprintln!("error: no .fzpz or .fzp files in {}", input.display());
            return 1;
        }
        let mut failed = 0;
        for f in &files {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match fzp::import_to_dir(f, out, &opts) {
                Ok((part, warnings, path)) => show(&name, &part, &warnings, &path),
                Err(e) => {
                    eprintln!("error: {name}: {e}");
                    failed += 1;
                }
            }
        }
        return if failed > 0 { 1 } else { 0 };
    }
    let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let result = if out.extension().is_some_and(|e| e.eq_ignore_ascii_case("part")) {
        fzp::import_path(input, &opts).and_then(|(part, warnings)| {
            if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            std::fs::write(out, fzp::part_json(&part)?).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
            Ok((part, warnings, out.to_path_buf()))
        })
    } else {
        fzp::import_to_dir(input, out, &opts)
    };
    match result {
        Ok((part, warnings, path)) => {
            show(&name, &part, &warnings, &path);
            0
        }
        Err(e) => {
            eprintln!("error: {name}: {e}");
            1
        }
    }
}

/// Make console output work from a GUI-subsystem exe: attach to the parent console and, if
/// stdout/stderr were not redirected, point them at it.
#[cfg(windows)]
pub fn attach_console() {
    use std::ffi::c_void;
    type Handle = *mut c_void;
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn GetStdHandle(which: u32) -> Handle;
        fn SetStdHandle(which: u32, h: Handle) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            sec: *const c_void,
            disp: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const GENERIC_READ_WRITE: u32 = 0xC000_0000;
    const SHARE_READ_WRITE: u32 = 3;
    const OPEN_EXISTING: u32 = 3;
    // SAFETY: plain Win32 calls with valid arguments; failures are ignored.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let conout: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(which);
            if h.is_null() || h as isize == -1 {
                let c = CreateFileW(
                    conout.as_ptr(),
                    GENERIC_READ_WRITE,
                    SHARE_READ_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if c as isize != -1 && !c.is_null() {
                    SetStdHandle(which, c);
                }
            }
        }
    }
}

#[cfg(not(windows))]
pub fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    const SCH: View = View::Schematic;
    const BB: View = View::Breadboard;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("core").join("tests").join("fixtures")
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("short-circ-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn gui_default() {
        assert_eq!(p(&[]).unwrap(), Command::Gui(None));
        assert_eq!(p(&["a.circ"]).unwrap(), Command::Gui(Some("a.circ".into())));
    }

    #[test]
    fn validate_flags_any_order() {
        let want = Command::Validate { file: "a.circ".into(), json: true, view: ViewSel::All };
        assert_eq!(p(&["--validate", "a.circ", "--json"]).unwrap(), want);
        assert_eq!(p(&["--json", "--validate", "a.circ"]).unwrap(), want);
        assert_eq!(
            p(&["--validate", "a.circ"]).unwrap(),
            Command::Validate { file: "a.circ".into(), json: false, view: ViewSel::All }
        );
    }

    #[test]
    fn validate_view_argument() {
        let want = |view| Command::Validate { file: "a.circ".into(), json: false, view };
        assert_eq!(p(&["--validate", "a.circ", "--view", "all"]).unwrap(), want(ViewSel::All));
        assert_eq!(p(&["--validate", "a.circ", "--view", "schematic"]).unwrap(), want(ViewSel::One(SCH)));
        assert_eq!(p(&["--view", "breadboard", "--validate", "a.circ"]).unwrap(), want(ViewSel::One(BB)));
        assert_eq!(p(&["--validate", "--view", "BB", "a.circ"]).unwrap(), want(ViewSel::One(BB)), "short, any case");
        assert!(p(&["--validate", "a.circ", "--view", "pcb"]).is_err());
        assert!(p(&["--validate", "a.circ", "--view"]).is_err(), "value missing");
        assert!(p(&["--validate", "a.circ", "--views", "schematic"]).is_err());
    }

    #[test]
    fn render_args() {
        assert_eq!(
            p(&["--render", "a.circ", "o.svg", "--dark"]).unwrap(),
            Command::Render { file: "a.circ".into(), out: "o.svg".into(), dark: true, view: SCH }
        );
        assert_eq!(
            p(&["--render", "a.circ", "o.svg", "--view", "breadboard"]).unwrap(),
            Command::Render { file: "a.circ".into(), out: "o.svg".into(), dark: false, view: BB }
        );
        assert!(p(&["--render", "a.circ"]).is_err());
        assert!(p(&["--render", "a.circ", "o.svg", "--view", "all"]).is_err(), "all is for --validate only");
        assert!(p(&["--view", "breadboard", "a.circ"]).is_err(), "--view needs a mode");
    }

    #[test]
    fn netlist_args() {
        let want = Command::Netlist { file: "a.circ".into(), json: true, view: SCH };
        assert_eq!(p(&["--netlist", "a.circ", "--json"]).unwrap(), want);
        assert_eq!(p(&["--json", "--netlist", "a.circ"]).unwrap(), want);
        assert_eq!(
            p(&["--netlist", "a.circ"]).unwrap(),
            Command::Netlist { file: "a.circ".into(), json: false, view: SCH }
        );
        assert_eq!(
            p(&["--netlist", "a.circ", "--view", "breadboard", "--json"]).unwrap(),
            Command::Netlist { file: "a.circ".into(), json: true, view: BB }
        );
        assert!(p(&["--netlist"]).is_err());
        assert!(p(&["--netlist", "a", "b"]).is_err());
        assert!(p(&["--netlist", "a", "--dark"]).is_err());
        assert!(p(&["--netlist", "a", "--validate", "b"]).is_err());
        assert!(p(&["--netlist", "a", "--view", "all"]).is_err());
    }

    #[test]
    fn render_part_args() {
        assert_eq!(
            p(&["--render-part", "dip8", "o.svg", "--dark", "--circuit-dir", "d"]).unwrap(),
            Command::RenderPart { ty: "dip8".into(), out: "o.svg".into(), dark: true, circuit_dir: Some("d".into()), view: SCH }
        );
        assert_eq!(
            p(&["--render-part", "dip8", "o.svg"]).unwrap(),
            Command::RenderPart { ty: "dip8".into(), out: "o.svg".into(), dark: false, circuit_dir: None, view: SCH }
        );
        assert_eq!(
            p(&["--render-part", "led", "o.svg", "--view", "breadboard"]).unwrap(),
            Command::RenderPart { ty: "led".into(), out: "o.svg".into(), dark: false, circuit_dir: None, view: BB }
        );
        assert!(p(&["--render-part", "dip8"]).is_err());
        assert!(p(&["--render-part", "a", "b", "c"]).is_err());
        assert!(p(&["--render-part", "a", "b", "--json"]).is_err());
        assert!(p(&["--render-part", "a", "b", "--circuit-dir"]).is_err(), "value missing");
        assert!(p(&["--render-part", "a", "b", "--view", "all"]).is_err());
        assert!(p(&["--render", "a.circ", "o.svg", "--circuit-dir", "d"]).is_err());
        assert!(p(&["--circuit-dir", "d", "a.circ"]).is_err());
    }

    #[test]
    fn upgrade_args() {
        assert_eq!(
            p(&["--upgrade", "a.circ"]).unwrap(),
            Command::Upgrade { input: "a.circ".into(), output: None }
        );
        assert_eq!(
            p(&["--upgrade", "a.circ", "b.circ"]).unwrap(),
            Command::Upgrade { input: "a.circ".into(), output: Some("b.circ".into()) }
        );
        assert_eq!(
            p(&["a.part", "--upgrade", "out.part"]).unwrap(),
            Command::Upgrade { input: "a.part".into(), output: Some("out.part".into()) }
        );
        assert!(p(&["--upgrade"]).is_err());
        assert!(p(&["--upgrade", "a", "b", "c"]).is_err());
        assert!(p(&["--upgrade", "a", "--json"]).is_err());
        assert!(p(&["--upgrade", "a", "--view", "schematic"]).is_err());
        assert!(p(&["--upgrade", "a", "--validate"]).is_err());
        assert!(HELP.contains("--upgrade"));
    }

    #[test]
    fn misc_commands_and_errors() {
        assert_eq!(p(&["--list-parts"]).unwrap(), Command::ListParts);
        assert_eq!(p(&["--list-colors"]).unwrap(), Command::ListColors);
        assert!(p(&["--list-colors", "x.circ"]).is_err());
        assert!(p(&["--list-colors", "--list-parts"]).is_err());
        assert_eq!(p(&["--help"]).unwrap(), Command::Help);
        assert_eq!(p(&["--version"]).unwrap(), Command::Version);
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--validate"]).is_err());
        assert!(p(&["--validate", "a", "--dark"]).is_err());
        assert!(p(&["--validate", "a", "--render", "b", "c"]).is_err());
        assert!(p(&["a", "b"]).is_err());
        assert!(p(&["--json", "a.circ"]).is_err());
    }

    #[test]
    fn list_parts_prints_views_per_type() {
        let text = list_parts_text(&Library::builtin());
        assert!(text.lines().any(|l| l == "resistor  schematic+breadboard"), "{text}");
        assert!(text.lines().any(|l| l == "breadboard_half  breadboard"), "{text}");
        assert!(text.lines().any(|l| l == "ground  schematic"), "{text}");
        assert!(text.lines().all(|l| l.split("  ").count() == 2), "two columns separated by two spaces");
    }

    #[test]
    fn text_report() {
        let r = Report::from_issues(vec![short_circ_core::Issue {
            rule: "dangling-wire".into(),
            severity: Severity::Warning,
            ids: vec!["w1".into()],
            at: Some([3, 4]),
            message: "m".into(),
            view: Some(SCH),
        }]);
        let t = report_text(Path::new("x.circ"), &r);
        assert!(t.contains("0 error(s), 1 warning(s)"));
        assert!(t.contains("dangling-wire [w1] at (3,4): m"));
    }

    #[test]
    fn validate_views_select_the_checks() {
        // A schematic wire dangling; a breadboard jumper dangling: each view reports its own.
        let dir = temp("validate-views");
        let file = dir.join("t.circ");
        std::fs::write(
            &file,
            r#"{"format":"short-circ/2","parts":[
 {"id":"R1","type":"resistor","schematic":{"at":[0,0]},"breadboard":{"at":[0,0]}}],
 "wires":[{"id":"w1","view":"schematic","pts":[[20,20],[24,20]]},{"id":"j1","view":"breadboard","pts":[[20,20],[24,24]]}]}"#,
        )
        .unwrap();
        let circuit = core_api::load_circuit(&file).unwrap();
        let lib = Library::load(&circuit, None);
        let all = core_api::validate(&circuit, &lib);
        let sch = short_circ_core::validate_view(&circuit, &lib, SCH);
        let bb = short_circ_core::validate_view(&circuit, &lib, BB);
        let dangling = |r: &Report| r.issues.iter().filter(|i| i.rule == "dangling-wire").count();
        assert!(dangling(&sch) >= 1 && sch.issues.iter().all(|i| i.view != Some(BB)));
        assert!(dangling(&bb) >= 1 && bb.issues.iter().all(|i| i.view != Some(SCH)));
        assert_eq!(dangling(&all), dangling(&sch) + dangling(&bb));
        // Warnings only: every selection exits 0.
        for sel in [ViewSel::All, ViewSel::One(SCH), ViewSel::One(BB)] {
            assert_eq!(validate(&file, false, sel), 0, "{sel:?}");
        }
        assert_eq!(validate(&dir.join("missing.circ"), true, ViewSel::All), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_views_and_exit_codes() {
        let dir = temp("render");
        let out = dir.join("o.svg");
        let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("examples").join("breadboard_demo.circ");
        assert_eq!(render(&demo, &out, false, SCH), 0);
        let sch = std::fs::read_to_string(&out).unwrap();
        assert_eq!(render(&demo, &out, false, BB), 0);
        let bb = std::fs::read_to_string(&out).unwrap();
        assert!(sch.contains("class=\"wire\"") && !sch.contains("class=\"jumper\""));
        assert!(bb.contains("class=\"jumper\"") && bb.contains("class=\"art\"") && !bb.contains("class=\"wire\""));
        assert_eq!(render(&dir.join("missing.circ"), &out, false, BB), 2);
        assert_eq!(render(&demo, &dir.join("no/such/dir/o.svg"), false, BB), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upgrade_circuit_in_place_and_to_a_new_file() {
        let dir = temp("upgrade");
        let v1 = fixtures().join("v1").join("led.circ");
        let file = dir.join("led.circ");
        std::fs::copy(&v1, &file).unwrap();
        let before = {
            let c = core_api::load_circuit(&file).unwrap();
            core_api::validate(&c, &Library::load(&c, file.parent()))
        };
        assert_eq!(validate(&file, false, ViewSel::All), 0);
        assert_eq!(upgrade(&file, None), 0, "in place");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"format\": \"short-circ/2\""), "{text}");
        assert!(text.contains("\"schematic\":") && text.contains("\"view\":\"schematic\""), "{text}");
        assert!(!text.contains("short-circ/1"));
        assert!(text.ends_with("}\n") && !text.contains('\r'));
        // It still loads, validates the same and is a fixed point of the writer.
        let c = core_api::load_circuit(&file).unwrap();
        let after = core_api::validate(&c, &Library::load(&c, file.parent()));
        assert_eq!((after.errors, after.warnings), (before.errors, before.warnings));
        assert_eq!(validate(&file, false, ViewSel::All), 0);
        let again = dir.join("again.circ");
        assert_eq!(upgrade(&file, Some(&again)), 0);
        assert_eq!(std::fs::read_to_string(&again).unwrap(), text, "canonical output is stable");
        // A second input keeps the source untouched when OUT is given.
        let copy = dir.join("copy.circ");
        std::fs::copy(&v1, &copy).unwrap();
        let out = dir.join("out.circ");
        assert_eq!(upgrade(&copy, Some(&out)), 0);
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), std::fs::read_to_string(&v1).unwrap());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upgrade_part_and_error_exit_codes() {
        let dir = temp("upgrade-part");
        let v1 = fixtures().join("v1").join("parts").join("resistor.part");
        let out = dir.join("resistor.part");
        assert_eq!(upgrade(&v1, Some(&out)), 0);
        let part = short_circ_core::parse_part(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(part.ty, "resistor");
        assert!(part.schematic.is_some() && part.pins.len() == 2);
        assert!(std::fs::read_to_string(&out).unwrap().contains("\"format\": \"short-circ/2\""));
        // Errors: missing input, wrong extension, invalid JSON, unwritable output.
        assert_eq!(upgrade(&dir.join("missing.circ"), None), 2);
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "{}").unwrap();
        assert_eq!(upgrade(&txt, None), 2);
        let bad = dir.join("bad.circ");
        std::fs::write(&bad, "{ not json").unwrap();
        assert_eq!(upgrade(&bad, None), 2);
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ not json", "a failed upgrade leaves the file alone");
        assert_eq!(upgrade(&v1, Some(&dir.join("no/such/dir/x.part"))), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><line x1="0" y1="10" x2="80" y2="10"/><rect x="20" y="0" width="40" height="20" fill="none"/></svg>"#;

    fn pin_vec(spec: &str) -> PinSpec {
        parse_pins(spec).unwrap()
    }

    #[test]
    fn import_svg_args() {
        let want = Command::ImportSvg {
            input: "a.svg".into(),
            out: "a.part".into(),
            ty: "res".into(),
            pins: pin_vec("a:0,1;b:8,1"),
            scale: None,
        };
        assert_eq!(p(&["--import-svg", "a.svg", "a.part", "--type", "res", "--pins", "a:0,1;b:8,1"]).unwrap(), want);
        // Flags in any order.
        assert_eq!(p(&["--type", "res", "--pins", "a:0,1;b:8,1", "a.svg", "--import-svg", "a.part"]).unwrap(), want);
        // Pins are optional, scale is optional.
        assert_eq!(
            p(&["--import-svg", "a.svg", "a.part", "--type", "res"]).unwrap(),
            Command::ImportSvg {
                input: "a.svg".into(),
                out: "a.part".into(),
                ty: "res".into(),
                pins: vec![],
                scale: None
            }
        );
        assert_eq!(
            p(&["--import-svg", "a.svg", "a.part", "--type", "res", "--scale", "20.5"]).unwrap(),
            Command::ImportSvg {
                input: "a.svg".into(),
                out: "a.part".into(),
                ty: "res".into(),
                pins: vec![],
                scale: Some(20.5)
            }
        );
    }

    #[test]
    fn import_svg_arg_errors() {
        let base = ["--import-svg", "a.svg", "a.part", "--type", "res"];
        let with = |extra: &[&str]| -> Vec<String> {
            base.iter().chain(extra.iter()).map(|s| s.to_string()).collect::<Vec<_>>()
        };
        assert!(p(&["--import-svg", "a.svg", "a.part"]).is_err(), "--type is required");
        assert!(p(&["--import-svg", "a.svg", "--type", "res"]).is_err(), "OUT is required");
        assert!(p(&["--import-svg", "a.svg", "a.part", "b.part", "--type", "res"]).is_err());
        assert!(p(&["--import-svg", "a.svg", "a.part", "--type"]).is_err(), "value missing");
        assert!(p(&["--import-svg", "a.svg", "a.part", "--type", "  "]).is_err(), "empty type");
        assert!(parse(&with(&["--scale", "0"])).is_err());
        assert!(parse(&with(&["--scale", "-1"])).is_err());
        assert!(parse(&with(&["--scale", "nan"])).is_err());
        assert!(parse(&with(&["--scale", "abc"])).is_err());
        assert!(parse(&with(&["--pins", "a:0"])).is_err());
        assert!(parse(&with(&["--json"])).is_err());
        assert!(parse(&with(&["--dark"])).is_err());
        assert!(parse(&with(&["--view", "schematic"])).is_err());
        // Import flags are rejected in other modes.
        assert!(p(&["--validate", "a.circ", "--type", "res"]).is_err());
        assert!(p(&["--render", "a.circ", "o.svg", "--scale", "2"]).is_err());
        assert!(p(&["--list-parts", "--pins", "a:0,1"]).is_err());
        assert!(p(&["--type", "res", "a.circ"]).is_err(), "no mode given");
    }

    #[test]
    fn pin_spec_parsing() {
        assert_eq!(parse_pins("a:0,1;b:8,1").unwrap(), vec![("a".into(), 0, 1), ("b".into(), 8, 1)]);
        assert_eq!(parse_pins(" a : -2 , 3 ").unwrap(), vec![("a".into(), -2, 3)]);
        assert_eq!(parse_pins("").unwrap(), vec![]);
        for bad in ["a0,1", ":0,1", "a:0", "a:0,1.5", "a:x,1", "a:0,1;", "a:0,1;;b:1,1", "a:0,1;a:2,2"] {
            assert!(parse_pins(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn import_output_parses_as_part() {
        let part = import_part(SVG, "res", &pin_vec("a:0,1;b:8,1"), None).unwrap();
        let text = write::part_to_string(&part);
        assert!(text.contains("\n  \""), "pretty printed");
        let back = short_circ_core::parse_part(&text).unwrap();
        assert_eq!(back.ty, "res");
        assert_eq!(back.schematic.as_ref().unwrap().size, [8.0, 2.0]);
        assert!(back.breadboard.is_none(), "an SVG import is schematic only");
        assert_eq!(back.pins.len(), 2);
        assert_eq!(import_svg_summary(&back, Path::new("res.part")), "res: size 8x2, 2 pin(s) -> res.part");
        // Explicit scale: 80x20 px at 20 px per unit is 4x1, widened to 8 by the pin at x=8.
        let scaled = import_part(SVG, "res", &pin_vec("a:0,1;b:8,1"), Some(20.0)).unwrap();
        assert_eq!(scaled.schematic.as_ref().unwrap().size, [8.0, 1.0]);
    }

    #[test]
    fn import_exit_codes() {
        let dir = temp("import-svg");
        let (svg, bad, out) = (dir.join("in.svg"), dir.join("bad.svg"), dir.join("out.part"));
        std::fs::write(&svg, SVG).unwrap();
        std::fs::write(&bad, "<html/>").unwrap();
        assert_eq!(import_svg_file(&dir.join("missing.svg"), &out, "res", &[], None), 2, "unreadable input");
        assert_eq!(import_svg_file(&bad, &out, "res", &[], None), 1, "import error");
        assert_eq!(import_svg_file(&svg, &out, "res", &pin_vec("a:0,1"), None), 0);
        let part = short_circ_core::parse_part(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(part.ty, "res");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_fritzing_args() {
        let want = |views: &[View], ty: Option<&str>| Command::ImportFritzing {
            input: "a.fzpz".into(),
            out: "o".into(),
            views: views.to_vec(),
            ty: ty.map(str::to_string),
        };
        assert_eq!(p(&["--import-fritzing", "a.fzpz", "o"]).unwrap(), want(&[SCH, BB], None), "both views by default");
        assert_eq!(
            p(&["--views", "breadboard", "--import-fritzing", "a.fzpz", "o", "--type", "x"]).unwrap(),
            want(&[BB], Some("x"))
        );
        assert_eq!(
            p(&["--import-fritzing", "a.fzpz", "o", "--views", "breadboard,schematic,breadboard"]).unwrap(),
            want(&[BB, SCH], None),
            "order kept, repeats dropped"
        );
        assert_eq!(p(&["--import-fritzing", "a.fzpz", "o", "--views", "schematic"]).unwrap(), want(&[SCH], None));
        assert!(p(&["--import-fritzing", "a.fzpz"]).is_err());
        assert!(p(&["--import-fritzing", "a", "b", "c"]).is_err());
        assert!(p(&["--import-fritzing", "a", "b", "--views", "pcb"]).is_err());
        assert!(p(&["--import-fritzing", "a", "b", "--views", "schematic,"]).is_err());
        assert!(p(&["--import-fritzing", "a", "b", "--view", "schematic"]).is_err(), "--view is not --views");
        assert!(p(&["--import-fritzing", "a", "b", "--pins", "a:0,1"]).is_err());
        assert!(p(&["--import-fritzing", "a", "b", "--scale", "2"]).is_err());
        assert!(p(&["--import-fritzing", "a", "b", "--json"]).is_err());
        assert!(p(&["--views", "schematic", "a.circ"]).is_err());
        assert!(p(&["--import-fritzing", "a", "b", "--validate"]).is_err());
        assert!(HELP.contains("--import-fritzing") && HELP.contains("--views"));
    }

    #[test]
    fn import_fritzing_exit_codes_and_outputs() {
        let fx = fixtures();
        let dir = temp("fz");
        let both = [SCH, BB];
        // One fzp to a .part file, with a type override and the breadboard view only.
        let out = dir.join("sub").join("x.part");
        assert_eq!(import_fritzing(&fx.join("mini4.fzp"), &out, &[BB], Some("mini_bb")), 0);
        let part = short_circ_core::parse_part(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(part.ty, "mini_bb");
        assert!(part.breadboard.is_some() && part.schematic.is_none());
        // Both views in one part.
        let out2 = dir.join("both.part");
        assert_eq!(import_fritzing(&fx.join("mini4.fzp"), &out2, &both, None), 0);
        let part2 = short_circ_core::parse_part(&std::fs::read_to_string(&out2).unwrap()).unwrap();
        assert!(part2.breadboard.is_some() && part2.schematic.is_some());
        assert_eq!(import_summary(&part2, Path::new("p")), format!("{}: views schematic+breadboard, {} pin(s) -> p", part2.ty, part2.pins.len()));
        // Output folder: named after the type.
        assert_eq!(import_fritzing(&fx.join("bare.fzp"), &dir.join("parts"), &[SCH], None), 0);
        assert!(dir.join("parts").join("bare_part.part").is_file());
        // A folder: every part is written, a failing one gives exit 1.
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("schematic")).unwrap();
        std::fs::copy(fx.join("bare.fzp"), src.join("bare.fzp")).unwrap();
        std::fs::copy(fx.join("schematic").join("bare.svg"), src.join("schematic").join("bare.svg")).unwrap();
        let folder_out = dir.join("folder_out");
        assert_eq!(import_fritzing(&src, &folder_out, &[SCH], None), 0);
        assert!(folder_out.join("bare_part.part").is_file());
        std::fs::write(src.join("broken.fzpz"), b"junk").unwrap();
        assert_eq!(import_fritzing(&src, &folder_out, &[SCH], None), 1);
        assert_eq!(import_fritzing(&src, &folder_out, &[SCH], Some("t")), 2, "--type with a folder");
        assert_eq!(import_fritzing(&src, &dir.join("o.part"), &[SCH], None), 2, "folder to file");
        // Missing or wrong input.
        assert_eq!(import_fritzing(&dir.join("missing.fzpz"), &folder_out, &[SCH], None), 1);
        assert_eq!(import_fritzing(&src.join("broken.fzpz"), &folder_out, &[SCH], None), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
