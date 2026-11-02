//! Command line parsing and the non-GUI modes (`--validate`, `--render`, `--list-parts`).

use crate::core_api;
use short_circ_core::{Circuit, Library, Report, Severity};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Gui(Option<PathBuf>),
    Validate { file: PathBuf, json: bool },
    Render { file: PathBuf, out: PathBuf, dark: bool },
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
    for a in args {
        match a.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-V" => return Ok(Command::Version),
            "--list-parts" => set_mode(&mut mode, "list-parts")?,
            "--validate" => set_mode(&mut mode, "validate")?,
            "--render" => set_mode(&mut mode, "render")?,
            "--json" => json = true,
            "--dark" => dark = true,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => positional.push(PathBuf::from(a)),
        }
    }
    match mode {
        None => {
            if json || dark {
                return Err("--json needs --validate and --dark needs --render".into());
            }
            match positional.len() {
                0 => Ok(Command::Gui(None)),
                1 => Ok(Command::Gui(positional.pop())),
                _ => Err("expected at most one file".into()),
            }
        }
        Some("list-parts") => {
            if !positional.is_empty() || json || dark {
                return Err("--list-parts takes no other arguments".into());
            }
            Ok(Command::ListParts)
        }
        Some("validate") => {
            if dark {
                return Err("--dark only applies to --render".into());
            }
            if positional.len() != 1 {
                return Err("--validate needs exactly one FILE".into());
            }
            Ok(Command::Validate { file: positional.remove(0), json })
        }
        Some(_) => {
            if json {
                return Err("--json only applies to --validate".into());
            }
            if positional.len() != 2 {
                return Err("--render needs FILE and OUT.svg".into());
            }
            let out = positional.remove(1);
            Ok(Command::Render { file: positional.remove(0), out, dark })
        }
    }
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
}
