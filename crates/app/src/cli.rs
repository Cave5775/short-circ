//! Command line parsing and the non-GUI modes (`--validate`, `--render`, `--list-parts`).

use crate::core_api;
use short_circ_core::{svgimport, Circuit, Library, PartDef, Report, Severity};
use std::path::{Path, PathBuf};

/// `(id, x, y)` pins as taken by `svgimport::import_svg`.
pub type PinSpec = Vec<(String, i32, i32)>;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Gui(Option<PathBuf>),
    Validate { file: PathBuf, json: bool },
    Render { file: PathBuf, out: PathBuf, dark: bool },
    ImportSvg { input: PathBuf, out: PathBuf, ty: String, pins: PinSpec, scale: Option<f64> },
    Netlist { file: PathBuf, json: bool },
    RenderPart { ty: String, out: PathBuf, dark: bool, circuit_dir: Option<PathBuf> },
    ListParts,
    Help,
    Version,
}

pub const HELP: &str = "\
short-circ - circuit diagram viewer

USAGE:
  short-circ [FILE.circ]                      open the viewer
  short-circ --validate FILE.circ [--json]    check a circuit (exit 0 ok, 1 errors, 2 unreadable)
  short-circ --render FILE.circ OUT.svg [--dark]
                                              export an SVG image
  short-circ --import-svg IN.svg OUT.part --type NAME [--pins SPEC] [--scale PX]
                                              import SVG artwork as a component file
                                              SPEC is id:x,y entries separated by ; (grid units),
                                              PX is pixels per grid unit (default 10)
  short-circ --netlist FILE.circ [--json]     print each net: id, name, pins (R1.a), wire ids
                                              (exit 0 ok, 2 unreadable)
  short-circ --render-part TYPE OUT.svg [--dark] [--circuit-dir DIR]
                                              draw one part with its pin ids labelled
                                              (exit 1 if TYPE is unknown; dipN/headerN work)
  short-circ --list-parts                     list available component types
  short-circ --help | --version

Viewer: drag to pan, wheel to zoom, F to fit, drop a .circ file onto the window to open it.
";

pub fn version_line() -> String {
    format!("short-circ {}", env!("CARGO_PKG_VERSION"))
}

/// Parse arguments (without the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
        let mut mode: Option<&str> = None;
    let mut positional: Vec<PathBuf> = Vec::new();
    let (mut json, mut dark) = (false, false);
    let (mut ty, mut pins, mut scale) = (None::<String>, None::<String>, None::<String>);
    let mut circuit_dir = None::<String>;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-V" => return Ok(Command::Version),
            "--list-parts" => set_mode(&mut mode, "list-parts")?,
            "--validate" => set_mode(&mut mode, "validate")?,
            "--render" => set_mode(&mut mode, "render")?,
            "--netlist" => set_mode(&mut mode, "netlist")?,
            "--render-part" => set_mode(&mut mode, "render-part")?,
            "--import-svg" => set_mode(&mut mode, "import-svg")?,
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
    match mode {
        None => {
            if json || dark {
                return Err("--json needs --validate and --dark needs --render".into());
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
            Ok(Command::Validate { file: positional.remove(0), json })
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
            Ok(Command::Netlist { file: positional.remove(0), json })
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
            let ty = positional.remove(0).to_string_lossy().into_owned();
            let out = positional.remove(0);
            Ok(Command::RenderPart { ty, out, dark, circuit_dir: circuit_dir.map(PathBuf::from) })
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
            let out = positional.remove(1);
            Ok(Command::Render { file: positional.remove(0), out, dark })
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
            for t in lib.types() {
                println!("{t}");
            }
            0
        }
        Command::Validate { file, json } => validate(&file, json),
        Command::Render { file, out, dark } => render(&file, &out, dark),
        Command::Netlist { file, json } => crate::netlist_out::run(&file, json),
        Command::RenderPart { ty, out, dark, circuit_dir } => {
            crate::partview::run(&ty, &out, dark, circuit_dir.as_deref())
        }
        Command::ImportSvg { input, out, ty, pins, scale } => {
            import_svg_file(&input, &out, &ty, &pins, scale)
        }
    }
}

fn validate(file: &Path, json: bool) -> i32 {
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
                };
                println!("{}", report_json(&Report::from_issues(vec![issue])));
            }
            return 2;
        }
    };
    let lib = Library::load(&circuit, file.parent());
    let report = core_api::validate(&circuit, &lib);
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

fn render(file: &Path, out: &Path, dark: bool) -> i32 {
    let circuit = match core_api::load_circuit(file) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let lib = Library::load(&circuit, file.parent());
    match core_api::render_svg(&circuit, &lib, dark) {
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

/// Import SVG text as a part. Uses `import_svg_scaled` when a scale was given.
fn import_part(svg: &str, ty: &str, pins: &[(String, i32, i32)], scale: Option<f64>) -> Result<PartDef, String> {
    match scale {
        Some(px) => svgimport::import_svg_scaled(svg, ty, pins, px),
        None => svgimport::import_svg(svg, ty, pins),
    }
}

/// Pretty printed JSON for a `.part` file, with a trailing newline.
fn part_json(part: &PartDef) -> Result<String, String> {
    serde_json::to_string_pretty(part)
        .map(|s| s + "\n")
        .map_err(|e| format!("cannot serialise part: {e}"))
}

/// One-line summary: type, size in grid units, pin count, output path.
fn import_summary(part: &PartDef, out: &Path) -> String {
    format!(
        "{}: size {}x{}, {} pin(s) -> {}",
        part.ty,
        part.size[0],
        part.size[1],
        part.pins.len(),
        out.display()
    )
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
        let json = part_json(&part)?;
        std::fs::write(out, json).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
        Ok(part)
    });
    match result {
        Ok(part) => {
            println!("{}", import_summary(&part, out));
            0
        }
        Err(msg) => {
            eprintln!("error: {msg}");
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

    #[test]
    fn gui_default() {
        assert_eq!(p(&[]).unwrap(), Command::Gui(None));
        assert_eq!(p(&["a.circ"]).unwrap(), Command::Gui(Some("a.circ".into())));
    }

    #[test]
    fn validate_flags_any_order() {
        let want = Command::Validate { file: "a.circ".into(), json: true };
        assert_eq!(p(&["--validate", "a.circ", "--json"]).unwrap(), want);
        assert_eq!(p(&["--json", "--validate", "a.circ"]).unwrap(), want);
        assert_eq!(
            p(&["--validate", "a.circ"]).unwrap(),
            Command::Validate { file: "a.circ".into(), json: false }
        );
    }

    #[test]
    fn render_args() {
        assert_eq!(
            p(&["--render", "a.circ", "o.svg", "--dark"]).unwrap(),
            Command::Render { file: "a.circ".into(), out: "o.svg".into(), dark: true }
        );
        assert!(p(&["--render", "a.circ"]).is_err());
    }

    #[test]
    fn netlist_args() {
        let want = Command::Netlist { file: "a.circ".into(), json: true };
        assert_eq!(p(&["--netlist", "a.circ", "--json"]).unwrap(), want);
        assert_eq!(p(&["--json", "--netlist", "a.circ"]).unwrap(), want);
        assert_eq!(
            p(&["--netlist", "a.circ"]).unwrap(),
            Command::Netlist { file: "a.circ".into(), json: false }
        );
        assert!(p(&["--netlist"]).is_err());
        assert!(p(&["--netlist", "a", "b"]).is_err());
        assert!(p(&["--netlist", "a", "--dark"]).is_err());
        assert!(p(&["--netlist", "a", "--validate", "b"]).is_err());
    }

    #[test]
    fn render_part_args() {
        assert_eq!(
            p(&["--render-part", "dip8", "o.svg", "--dark", "--circuit-dir", "d"]).unwrap(),
            Command::RenderPart { ty: "dip8".into(), out: "o.svg".into(), dark: true, circuit_dir: Some("d".into()) }
        );
        assert_eq!(
            p(&["--render-part", "dip8", "o.svg"]).unwrap(),
            Command::RenderPart { ty: "dip8".into(), out: "o.svg".into(), dark: false, circuit_dir: None }
        );
        assert!(p(&["--render-part", "dip8"]).is_err());
        assert!(p(&["--render-part", "a", "b", "c"]).is_err());
        assert!(p(&["--render-part", "a", "b", "--json"]).is_err());
        assert!(p(&["--render-part", "a", "b", "--circuit-dir"]).is_err(), "value missing");
        assert!(p(&["--render", "a.circ", "o.svg", "--circuit-dir", "d"]).is_err());
        assert!(p(&["--circuit-dir", "d", "a.circ"]).is_err());
    }

    #[test]
    fn misc_commands_and_errors() {
        assert_eq!(p(&["--list-parts"]).unwrap(), Command::ListParts);
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
    fn text_report() {
        let r = Report::from_issues(vec![short_circ_core::Issue {
            rule: "dangling-wire".into(),
            severity: Severity::Warning,
            ids: vec!["w1".into()],
            at: Some([3, 4]),
            message: "m".into(),
        }]);
        let t = report_text(Path::new("x.circ"), &r);
        assert!(t.contains("0 error(s), 1 warning(s)"));
        assert!(t.contains("dangling-wire [w1] at (3,4): m"));
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
        let json = part_json(&part).unwrap();
        assert!(json.contains("\n  \""), "pretty printed");
        let back = short_circ_core::parse_part(&json).unwrap();
        assert_eq!(back.ty, "res");
        assert_eq!(back.size, [8.0, 2.0]);
        assert_eq!(back.pins.len(), 2);
        assert_eq!(import_summary(&back, Path::new("res.part")), "res: size 8x2, 2 pin(s) -> res.part");
        // Explicit scale: 80x20 px at 20 px per unit is 4x1, widened to 8 by the pin at x=8.
        let scaled = import_part(SVG, "res", &pin_vec("a:0,1;b:8,1"), Some(20.0)).unwrap();
        assert_eq!(scaled.size, [8.0, 1.0]);
    }

    #[test]
    fn import_exit_codes() {
        let dir = std::env::temp_dir().join(format!("short-circ-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
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
}
