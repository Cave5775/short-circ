//! `--netlist FILE [--view V] [--json]`: print the nets of one view of a circuit so connectivity
//! can be checked without looking at a picture.

use crate::core_api;
use short_circ_core::{Library, Net, Netlist, View};
use std::cmp::Ordering;
use std::path::Path;

/// Compare strings so that digit runs compare as numbers: `R2 < R10`, pin `9 < 10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(|c| c.is_ascii_digit()) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (da, db) = (run(&mut a), run(&mut b));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| da.len().cmp(&db.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Pins of a net as `R1.a`, sorted by part then pin (natural order).
pub fn sorted_pins(net: &Net) -> Vec<String> {
    let mut pins: Vec<(&str, &str)> = net.pins.iter().map(|p| (p.part.as_str(), p.pin.as_str())).collect();
    pins.sort_by(|a, b| natural_cmp(a.0, b.0).then_with(|| natural_cmp(a.1, b.1)));
    pins.into_iter().map(|(p, n)| format!("{p}.{n}")).collect()
}

/// One line per net: `id  name-or-dash  pins: ...  wires: ...` (`-` for an empty list).
pub fn netlist_text(nets: &[Net]) -> String {
    let join = |v: Vec<String>| if v.is_empty() { "-".to_string() } else { v.join(", ") };
    let mut s = String::new();
    for n in nets {
        s.push_str(&format!(
            "net {}  {}  pins: {}  wires: {}\n",
            n.id,
            n.name.as_deref().filter(|x| !x.is_empty()).unwrap_or("-"),
            join(sorted_pins(n)),
            join(n.wires.clone())
        ));
    }
    if nets.is_empty() {
        s.push_str("no nets\n");
    }
    s
}

/// `{"nets":[{"id":0,"name":"GND","pins":["R1.b"],"wires":["w1"]}]}`; `name` is null when the
/// net has no label.
pub fn netlist_json(nets: &[Net]) -> String {
    let nets: Vec<serde_json::Value> = nets
        .iter()
        .map(|n| {
            serde_json::json!({
                "id": n.id,
                "name": n.name.as_deref().filter(|x| !x.is_empty()),
                "pins": sorted_pins(n),
                "wires": n.wires,
            })
        })
        .collect();
    serde_json::json!({ "nets": nets }).to_string()
}

/// Exit codes: 0 printed, 2 unreadable input.
pub fn run(file: &Path, json: bool, view: View) -> i32 {
    let circuit = match core_api::load_circuit(file) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let lib = Library::load(&circuit, file.parent());
    let nl = Netlist::build(&circuit, &lib, view);
    if json {
        println!("{}", netlist_json(&nl.nets));
    } else {
        print!("{}", netlist_text(&nl.nets));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn led() -> Netlist {
        let c = core_api::parse_circuit_text(include_str!("../../../examples/led.circ")).unwrap();
        let lib = Library::load(&c, None);
        Netlist::build(&c, &lib, View::Schematic)
    }

    #[test]
    fn breadboard_view_lists_jumpers_and_strips() {
        let c = core_api::parse_circuit_text(include_str!("../../../examples/breadboard_demo.circ")).unwrap();
        let lib = Library::load(&c, None);
        let nl = Netlist::build(&c, &lib, View::Breadboard);
        let text = netlist_text(&nl.nets);
        assert!(text.lines().any(|l| l.contains("wires: j1")), "{text}");
        let v: serde_json::Value = serde_json::from_str(&netlist_json(&nl.nets)).unwrap();
        let nets = v["nets"].as_array().unwrap();
        // The battery plus lead and one jumper form a net with the battery pin first (natural order).
        let pos = nets.iter().find(|n| n["wires"].as_array().unwrap().iter().any(|w| w == "j1")).unwrap();
        assert!(pos["pins"].as_array().unwrap().iter().any(|p| p == "B1.pos"));
        assert!(pos["pins"].as_array().unwrap().iter().any(|p| p == "R1.a"));
        assert!(pos["name"].is_null(), "labels are not used in the breadboard view");
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("R2", "R10"), Ordering::Less);
        assert_eq!(natural_cmp("R10", "R2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("9", "10"), Ordering::Less);
        assert_eq!(natural_cmp("U1", "R1"), Ordering::Greater);
        assert_eq!(natural_cmp("R", "R1"), Ordering::Less);
        assert_eq!(natural_cmp("R01", "R1"), Ordering::Greater, "ties broken by length, still total");
    }

    #[test]
    fn json_shape_and_content() {
        let v: serde_json::Value = serde_json::from_str(&netlist_json(&led().nets)).unwrap();
        let nets = v["nets"].as_array().unwrap();
        assert!(!nets.is_empty());
        let gnd = nets.iter().find(|n| n["name"] == "GND").expect("GND net");
        assert!(gnd["id"].is_u64());
        assert!(gnd["wires"].as_array().unwrap().iter().any(|w| w == "w3"));
        let pins = gnd["pins"].as_array().unwrap();
        assert!(!pins.is_empty());
        assert!(pins.iter().all(|p| p.as_str().unwrap().contains('.')));
        // Pins are sorted.
        let strs: Vec<&str> = pins.iter().map(|p| p.as_str().unwrap()).collect();
        let mut sorted = strs.clone();
        sorted.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(strs, sorted);
        // Unlabelled nets have a null name.
        assert!(nets.iter().any(|n| n["name"].is_null()));
    }

    #[test]
    fn text_lists_every_net() {
        let nl = led();
        let t = netlist_text(&nl.nets);
        assert_eq!(t.lines().count(), nl.nets.len());
        assert!(t.contains("GND"));
        assert!(t.contains("pins: "));
        assert!(t.lines().any(|l| l.contains("  -  pins:")), "unnamed net shows a dash: {t}");
        assert_eq!(netlist_text(&[]), "no nets\n");
    }

    #[test]
    fn empty_lists_print_dash() {
        let nl = vec![Net { id: 0, name: None, pins: vec![], wires: vec![] }];
        assert_eq!(netlist_text(&nl), "net 0  -  pins: -  wires: -\n");
        assert_eq!(netlist_json(&nl), r#"{"nets":[{"id":0,"name":null,"pins":[],"wires":[]}]}"#);
    }

    #[test]
    fn unreadable_file_exits_2() {
        assert_eq!(run(Path::new("Z:/definitely/not/here.circ"), false, View::Schematic), 2);
        assert_eq!(run(Path::new("Z:/definitely/not/here.circ"), true, View::Breadboard), 2);
    }
}
