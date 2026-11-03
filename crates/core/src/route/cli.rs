//! `short-circ --autoroute IN OUT [options]`: command line front end of `route::port`.
//!
//! The binary hands over the arguments after `--autoroute`. Exit codes: 0 every net realised,
//! 1 some nets failed (OUT is still written), 2 unreadable input, bad arguments or unwritable
//! output.

use super::{port, JumperColor, JumperStyle, Prefer, RouteOptions, Scope, Source};
use crate::color::is_valid_color_spec;
use crate::model::View;
use crate::resolve::Library;
use crate::{parse_circuit, write};
use std::io::Write;
use std::path::Path;

fn need<'a>(args: &'a [String], i: usize, flag: &str) -> Result<&'a str, String> {
    args.get(i + 1).map(|s| s.as_str()).ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_int(s: &str, flag: &str, min: i32) -> Result<i32, String> {
    let v: i32 = s.trim().parse().map_err(|_| format!("{flag}: \"{s}\" is not a whole number"))?;
    if v < min {
        return Err(format!("{flag}: {v} is too small (minimum {min})"));
    }
    Ok(v)
}

/// Parse the positional arguments and the options of `--autoroute`.
pub fn parse_args(args: &[String]) -> Result<(Vec<String>, RouteOptions), String> {
    let mut o = RouteOptions::default();
    let mut positional: Vec<String> = Vec::new();
    let mut scope: Option<Scope> = None;
    let mut only: Option<Vec<String>> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut takes_value = true;
        match a {
            "--to" => {
                let v = need(args, i, a)?;
                o.to = match View::parse(v) {
                    Some(view) => view,
                    None => return Err(format!("--to: \"{v}\" is not a view (schematic or breadboard)")),
                };
            }
            "--from-nets" => {
                o.source = Source::Declared;
                takes_value = false;
            }
            "--scope" => {
                let v = need(args, i, a)?;
                scope = Some(match v.to_ascii_lowercase().as_str() {
                    "all" => Scope::All,
                    "unrouted" => Scope::Unrouted,
                    _ => return Err(format!("--scope: \"{v}\" is not all or unrouted")),
                });
            }
            "--only" => {
                let v = need(args, i, a)?;
                let refs: Vec<String> = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if refs.is_empty() {
                    return Err("--only: expected a comma separated list like R1.a,B1.pos".into());
                }
                if let Some(bad) = refs.iter().find(|r| !r.contains('.') || r.starts_with('.') || r.ends_with('.')) {
                    return Err(format!("--only: \"{bad}\" is not a PART.PIN reference"));
                }
                only = Some(refs);
            }
            "--no-place" => {
                o.place_missing = false;
                takes_value = false;
            }
            "--replace" => {
                o.keep_wires = false;
                takes_value = false;
            }
            "--crossings" => {
                o.allow_crossings = true;
                takes_value = false;
            }
            "--prefer" => {
                let v = need(args, i, a)?;
                o.prefer = match v.to_ascii_lowercase().as_str() {
                    "bends" => Prefer::Bends,
                    "length" => Prefer::Length,
                    _ => return Err(format!("--prefer: \"{v}\" is not bends or length")),
                };
            }
            "--clearance" => o.clearance = parse_int(need(args, i, a)?, a, 0)?,
            "--no-labels" => {
                o.label_fallback = false;
                takes_value = false;
            }
            "--jumper" => {
                let v = need(args, i, a)?;
                o.jumper_style = match v.to_ascii_lowercase().as_str() {
                    "straight" => JumperStyle::Straight,
                    "orthogonal" => JumperStyle::Orthogonal,
                    "diagonal" | "diagonal45" => JumperStyle::Diagonal45,
                    _ => return Err(format!("--jumper: \"{v}\" is not straight, orthogonal or diagonal")),
                };
            }
            "--jumper-color" => {
                let v = need(args, i, a)?;
                o.jumper_color = if v.eq_ignore_ascii_case("by-net") {
                    JumperColor::ByNet
                } else if is_valid_color_spec(v) {
                    JumperColor::Single(v.to_string())
                } else {
                    return Err(format!("--jumper-color: \"{v}\" is not by-net or a colour (see --list-colors)"));
                };
            }
            "--max-jumper" => o.max_jumper_len = Some(parse_int(need(args, i, a)?, a, 1)?),
            _ if a.starts_with("--") => return Err(format!("unknown option {a}")),
            _ => {
                positional.push(a.to_string());
                takes_value = false;
            }
        }
        i += if takes_value { 2 } else { 1 };
    }
    // `--only` names the nets, so it wins over `--scope`.
    if let Some(refs) = only {
        o.scope = Scope::Only(refs);
    } else if let Some(s) = scope {
        o.scope = s;
    }
    Ok((positional, o))
}

/// Parse only options (no `IN OUT`).
pub fn parse_options(args: &[String]) -> Result<RouteOptions, String> {
    let (pos, o) = parse_args(args)?;
    match pos.first() {
        Some(p) => Err(format!("unexpected argument {p}")),
        None => Ok(o),
    }
}

/// The `--autoroute` option flags that reproduce `o` (only those that differ from
/// `RouteOptions::default()`), the inverse of [`parse_options`]: `parse_options(&options_to_args(&o))`
/// gives `o` back. Exceptions are values the parser rejects (an empty `Scope::Only`, a
/// `max_jumper_len` below 1, a negative clearance), which have no command line form.
pub fn options_to_args(o: &RouteOptions) -> Vec<String> {
    let d = RouteOptions::default();
    let mut a: Vec<String> = Vec::new();
    let mut flag = |name: &str, value: Option<String>| {
        a.push(name.to_string());
        a.extend(value);
    };
    if o.to != d.to {
        flag("--to", Some(o.to.key().to_string()));
    }
    if o.source == Source::Declared {
        flag("--from-nets", None);
    }
    match &o.scope {
        Scope::All => {}
        Scope::Unrouted => flag("--scope", Some("unrouted".into())),
        Scope::Only(refs) if !refs.is_empty() => flag("--only", Some(refs.join(","))),
        Scope::Only(_) => {}
    }
    if !o.place_missing {
        flag("--no-place", None);
    }
    if !o.keep_wires {
        flag("--replace", None);
    }
    if o.allow_crossings {
        flag("--crossings", None);
    }
    if o.prefer != d.prefer {
        flag("--prefer", Some("length".into()));
    }
    if o.clearance != d.clearance {
        flag("--clearance", Some(o.clearance.to_string()));
    }
    if !o.label_fallback {
        flag("--no-labels", None);
    }
    match o.jumper_style {
        JumperStyle::Orthogonal => {}
        JumperStyle::Straight => flag("--jumper", Some("straight".into())),
        JumperStyle::Diagonal45 => flag("--jumper", Some("diagonal".into())),
    }
    if let JumperColor::Single(spec) = &o.jumper_color {
        flag("--jumper-color", Some(spec.clone()));
    }
    if let Some(n) = o.max_jumper_len {
        flag("--max-jumper", Some(n.to_string()));
    }
    a
}

/// Run `--autoroute`; `args` are the arguments after the flag. Prints to stdout / stderr.
pub fn run(args: &[String]) -> i32 {
    run_with(args, &mut std::io::stdout(), &mut std::io::stderr())
}

/// `run` with injectable output streams (used by the tests).
pub fn run_with(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (pos, opts) = match parse_args(args) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            return 2;
        }
    };
    if pos.len() != 2 {
        let _ = writeln!(err, "error: usage: --autoroute IN OUT [options]");
        return 2;
    }
    let (inp, outp) = (Path::new(&pos[0]), Path::new(&pos[1]));
    let src = match std::fs::read_to_string(inp) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "error: cannot read {}: {e}", inp.display());
            return 2;
        }
    };
    let circuit = match parse_circuit(&src) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(err, "error: {}: {e}", inp.display());
            return 2;
        }
    };
    let dir = inp.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let lib = Library::load(&circuit, Some(dir));
    let outcome = port(&circuit, &lib, &opts);
    if let Err(e) = std::fs::write(outp, write::circuit_to_string(&outcome.circuit)) {
        let _ = writeln!(err, "error: cannot write {}: {e}", outp.display());
        return 2;
    }
    for f in &outcome.failures {
        let _ = writeln!(out, "failed {}: {}", f.net, f.reason);
    }
    let _ = writeln!(
        out,
        "placed {} part(s), added {} wire(s), routed {} net(s), {} failed",
        outcome.placed.len(),
        outcome.wires_added.len(),
        outcome.routed,
        outcome.failures.len()
    );
    if outcome.failures.is_empty() {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn defaults() {
        assert_eq!(parse_options(&[]).unwrap(), RouteOptions::default());
    }

    #[test]
    fn every_flag_is_parsed() {
        let o = parse_options(&args(
            "--to schematic --from-nets --scope unrouted --no-place --replace --crossings --prefer length --clearance 3 \
             --no-labels --jumper diagonal --jumper-color teal --max-jumper 12",
        ))
        .unwrap();
        assert_eq!(o.to, View::Schematic);
        assert_eq!(o.source, Source::Declared);
        assert_eq!(o.scope, Scope::Unrouted);
        assert!(!o.place_missing && !o.keep_wires && o.allow_crossings && !o.label_fallback);
        assert_eq!(o.prefer, Prefer::Length);
        assert_eq!(o.clearance, 3);
        assert_eq!(o.jumper_style, JumperStyle::Diagonal45);
        assert_eq!(o.jumper_color, JumperColor::Single("teal".into()));
        assert_eq!(o.max_jumper_len, Some(12));
    }

    #[test]
    fn values_are_parsed() {
        assert_eq!(parse_options(&args("--to breadboard")).unwrap().to, View::Breadboard);
        assert_eq!(parse_options(&args("--scope all")).unwrap().scope, Scope::All);
        assert_eq!(parse_options(&args("--prefer bends")).unwrap().prefer, Prefer::Bends);
        assert_eq!(parse_options(&args("--jumper straight")).unwrap().jumper_style, JumperStyle::Straight);
        assert_eq!(parse_options(&args("--jumper orthogonal")).unwrap().jumper_style, JumperStyle::Orthogonal);
        assert_eq!(parse_options(&args("--jumper-color by-net")).unwrap().jumper_color, JumperColor::ByNet);
        assert_eq!(parse_options(&args("--jumper-color #ff0000")).unwrap().jumper_color, JumperColor::Single("#ff0000".into()));
        assert_eq!(
            parse_options(&args("--only R1.a,B1.pos")).unwrap().scope,
            Scope::Only(vec!["R1.a".into(), "B1.pos".into()])
        );
        // --only wins over --scope, in either order
        assert_eq!(parse_options(&args("--only R1.a --scope unrouted")).unwrap().scope, Scope::Only(vec!["R1.a".into()]));
        assert_eq!(parse_options(&args("--scope unrouted --only R1.a")).unwrap().scope, Scope::Only(vec!["R1.a".into()]));
    }

    #[test]
    fn options_to_args_round_trips_through_parse_options() {
        let d = RouteOptions::default;
        let sets = [
            d(),
            RouteOptions { to: View::Schematic, ..d() },
            RouteOptions { source: Source::Declared, ..d() },
            RouteOptions { scope: Scope::Unrouted, ..d() },
            RouteOptions { scope: Scope::Only(vec!["R1.a".into()]), ..d() },
            RouteOptions { scope: Scope::Only(vec!["R1.a".into(), "B1.pos".into(), "U1.12".into()]), ..d() },
            RouteOptions { place_missing: false, ..d() },
            RouteOptions { keep_wires: false, ..d() },
            RouteOptions { allow_crossings: true, ..d() },
            RouteOptions { prefer: Prefer::Length, ..d() },
            RouteOptions { clearance: 0, ..d() },
            RouteOptions { clearance: 6, ..d() },
            RouteOptions { label_fallback: false, ..d() },
            RouteOptions { jumper_style: JumperStyle::Straight, ..d() },
            RouteOptions { jumper_style: JumperStyle::Diagonal45, ..d() },
            RouteOptions { jumper_color: JumperColor::Single("teal".into()), ..d() },
            RouteOptions { jumper_color: JumperColor::Single("#12ab34".into()), ..d() },
            RouteOptions { max_jumper_len: Some(1), ..d() },
            RouteOptions { max_jumper_len: Some(24), ..d() },
            RouteOptions {
                to: View::Schematic,
                source: Source::Declared,
                scope: Scope::Only(vec!["R1.a".into()]),
                place_missing: false,
                keep_wires: false,
                allow_crossings: true,
                prefer: Prefer::Length,
                clearance: 3,
                label_fallback: false,
                jumper_style: JumperStyle::Diagonal45,
                jumper_color: JumperColor::Single("red".into()),
                max_jumper_len: Some(9),
            },
        ];
        for o in sets {
            let a = options_to_args(&o);
            assert_eq!(parse_options(&a), Ok(o.clone()), "{a:?}");
        }
    }

    #[test]
    fn options_to_args_only_emits_what_differs_from_the_defaults() {
        assert!(options_to_args(&RouteOptions::default()).is_empty());
        let o = RouteOptions { to: View::Schematic, clearance: 2, scope: Scope::Only(vec!["R1.a".into(), "B1.pos".into()]), ..RouteOptions::default() };
        assert_eq!(options_to_args(&o), args("--to schematic --only R1.a,B1.pos --clearance 2"));
        let o = RouteOptions { jumper_color: JumperColor::Single("#ff0000".into()), keep_wires: false, ..RouteOptions::default() };
        assert_eq!(options_to_args(&o), args("--replace --jumper-color #ff0000"));
    }

    #[test]
    fn bad_values_have_clear_errors() {
        for (bad, needle) in [
            ("--to pcb", "--to"),
            ("--scope some", "--scope"),
            ("--only", "needs a value"),
            ("--only R1", "PART.PIN"),
            ("--prefer fast", "--prefer"),
            ("--clearance x", "--clearance"),
            ("--clearance -1", "too small"),
            ("--jumper wavy", "--jumper"),
            ("--jumper-color nocolour", "--jumper-color"),
            ("--max-jumper 0", "too small"),
            ("--max-jumper", "needs a value"),
            ("--frobnicate", "unknown option"),
            ("stray", "unexpected argument"),
        ] {
            let e = parse_options(&args(bad)).expect_err(bad);
            assert!(e.contains(needle), "{bad}: {e}");
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("short-circ-route-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run_capture(a: &[String]) -> (i32, String, String) {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let code = run_with(a, &mut o, &mut e);
        (code, String::from_utf8(o).unwrap(), String::from_utf8(e).unwrap())
    }

    #[test]
    fn run_ports_a_v1_file_to_the_breadboard() {
        let dir = temp_dir("ok");
        let inp = dir.join("led.circ");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v1/led.circ"), &inp).unwrap();
        let out = dir.join("out.circ");
        let a = vec![inp.display().to_string(), out.display().to_string(), "--to".into(), "breadboard".into()];
        let (code, stdout, stderr) = run_capture(&a);
        assert_eq!(code, 0, "{stdout} {stderr}");
        assert!(stderr.is_empty());
        assert_eq!(stdout.lines().count(), 1, "{stdout}");
        assert!(stdout.starts_with("placed 4 part(s), added "), "{stdout}");
        assert!(stdout.trim_end().ends_with("routed 3 net(s), 0 failed"), "{stdout}");
        let text = std::fs::read_to_string(&out).unwrap();
        let c = parse_circuit(&text).unwrap();
        assert_eq!(c.format, "short-circ/2");
        assert!(c.parts.iter().any(|p| p.ty == "breadboard_half"));
        assert!(c.parts.iter().all(|p| p.breadboard.is_some()));
        let lib = Library::load(&c, None);
        assert_eq!(validate(&c, &lib).errors, 0);
        // canonical: writing the parsed output gives the same bytes
        assert_eq!(write::circuit_to_string(&c), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_reports_failures_with_exit_1_and_still_writes() {
        let dir = temp_dir("fail");
        let inp = dir.join("ic.circ");
        std::fs::write(
            &inp,
            r#"{"format":"short-circ/2","parts":[{"id":"U1","type":"dip8"}],"nets":[{"name":"A","pins":["U1.1","U1.4"]}]}"#,
        )
        .unwrap();
        let out = dir.join("out.circ");
        let a = vec![
            inp.display().to_string(),
            out.display().to_string(),
            "--from-nets".into(),
            "--max-jumper".into(),
            "1".into(),
        ];
        let (code, stdout, _) = run_capture(&a);
        assert_eq!(code, 1, "{stdout}");
        assert!(stdout.starts_with("failed A: jumper too long"), "{stdout}");
        assert!(stdout.contains("too long"), "{stdout}");
        assert!(stdout.lines().last().unwrap().contains(" failed"), "{stdout}");
        assert!(out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_exit_2_for_bad_input_or_arguments() {
        let dir = temp_dir("bad");
        let out = dir.join("out.circ");
        let o = out.display().to_string();
        let missing = dir.join("missing.circ").display().to_string();
        let (code, _, err) = run_capture(&[missing, o.clone()]);
        assert_eq!(code, 2);
        assert!(err.contains("cannot read"), "{err}");
        let garbage = dir.join("garbage.circ");
        std::fs::write(&garbage, "not json").unwrap();
        let (code, _, err) = run_capture(&[garbage.display().to_string(), o.clone()]);
        assert_eq!(code, 2);
        assert!(err.starts_with("error:"), "{err}");
        let (code, _, err) = run_capture(&[garbage.display().to_string()]);
        assert_eq!(code, 2);
        assert!(err.contains("usage"), "{err}");
        let (code, _, err) = run_capture(&[garbage.display().to_string(), o.clone(), "--to".into(), "moon".into()]);
        assert_eq!(code, 2);
        assert!(err.contains("--to"), "{err}");
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
