//! Thin adapter between the app and `short-circ-core`.
//!
//! Everything the app needs from the netlist, validation and SVG modules goes through this
//! file. Parsing, libraries, transforms and the data model are used from core directly
//! elsewhere. Every call that depends on the layout takes the [`View`] it is about.

use short_circ_core::color::Rgba;
use short_circ_core::{Circuit, Library, PartDef, PartLookup, Pt, Report, View};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// One pin on a net.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinRef {
    pub part: String,
    pub pin: String,
    pub at: Pt,
    /// True for a hole of a board part (breadboard): listed with the net but not interesting
    /// for humans.
    pub board: bool,
}

/// One electrical net (mirrors `core::netlist::Net`).
#[derive(Debug, Clone, Default)]
pub struct Net {
    pub id: usize,
    pub name: String,
    pub pins: Vec<PinRef>,
    pub wires: Vec<String>,
}

impl Net {
    /// Pins that are not board holes.
    pub fn part_pins(&self) -> impl Iterator<Item = &PinRef> {
        self.pins.iter().filter(|p| !p.board)
    }

    /// Worth listing: it has a wire or a pin of a real part (a strip of an empty breadboard
    /// is not).
    pub fn is_used(&self) -> bool {
        !self.wires.is_empty() || self.pins.iter().any(|p| !p.board)
    }
}

/// Net lookup used for hover highlighting and the info panel; one per view.
#[derive(Debug, Clone, Default)]
pub struct Nets {
    nets: Vec<Net>,
    pin_idx: HashMap<(String, String), usize>,
    wire_idx: HashMap<String, usize>,
}

impl Nets {
    pub fn of_pin(&self, part: &str, pin: &str) -> Option<&Net> {
        self.pin_idx.get(&(part.to_string(), pin.to_string())).map(|&i| &self.nets[i])
    }

    pub fn of_wire(&self, wire: &str) -> Option<&Net> {
        self.wire_idx.get(wire).map(|&i| &self.nets[i])
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.nets.len()
    }

    /// Nets worth listing (see [`Net::is_used`]).
    pub fn used(&self) -> impl Iterator<Item = &Net> {
        self.nets.iter().filter(|n| n.is_used())
    }
}

/// Build the netlist of one view.
pub fn build_nets(circuit: &Circuit, lib: &dyn PartLookup, view: View) -> Nets {
    let nl = short_circ_core::Netlist::build(circuit, lib, view);
    let boards: HashSet<&str> = circuit
        .parts
        .iter()
        .filter(|p| lib.get(&p.ty).is_some_and(PartDef::is_board))
        .map(|p| p.id.as_str())
        .collect();
    let nets: Vec<Net> = nl
        .nets
        .iter()
        .map(|n| Net {
            id: n.id,
            name: n.name.clone().unwrap_or_default(),
            pins: n
                .pins
                .iter()
                .map(|p| PinRef { part: p.part.clone(), pin: p.pin.clone(), at: p.at, board: boards.contains(p.part.as_str()) })
                .collect(),
            wires: n.wires.clone(),
        })
        .collect();
    let mut pin_idx = HashMap::new();
    let mut wire_idx = HashMap::new();
    for (i, n) in nets.iter().enumerate() {
        for p in &n.pins {
            pin_idx.entry((p.part.clone(), p.pin.clone())).or_insert(i);
        }
        for w in &n.wires {
            wire_idx.entry(w.clone()).or_insert(i);
        }
    }
    Nets { nets, pin_idx, wire_idx }
}

/// Validate every view in use plus the cross-view checks.
pub fn validate(circuit: &Circuit, lib: &dyn PartLookup) -> Report {
    short_circ_core::validate(circuit, lib)
}

/// True when SVG export is available (enables File > Export SVG).
pub fn svg_available() -> bool {
    true
}

/// Render one view of the circuit to an SVG document.
pub fn render_svg(circuit: &Circuit, lib: &dyn PartLookup, dark: bool, view: View) -> Result<String, String> {
    Ok(short_circ_core::svg::render_svg(circuit, lib, dark, view))
}

/// Preset colour of each breadboard jumper's net, in the order of the breadboard wires of the
/// circuit (the helper svg export uses, so viewer and export agree).
pub fn jumper_colors(circuit: &Circuit, lib: &Library) -> Vec<Option<Rgba>> {
    short_circ_core::svg::jumper_colors(circuit, lib)
}

/// Read and parse a circuit file (v1 files are upgraded on load).
pub fn load_circuit(path: &Path) -> Result<Circuit, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_circuit_text(&src).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parse circuit text (format v1 or v2) and check the result is the current format.
pub fn parse_circuit_text(src: &str) -> Result<Circuit, String> {
    let c = short_circ_core::parse_circuit(src).map_err(|e| format!("invalid circuit file: {e}"))?;
    if c.format != short_circ_core::FORMAT {
        return Err(format!("unsupported format \"{}\" (expected \"{}\")", c.format, short_circ_core::FORMAT));
    }
    Ok(c)
}

/// The views a component has geometry for, as `schematic+breadboard` (`-` for none).
pub fn view_names(def: &PartDef) -> String {
    let names: Vec<&str> = View::ALL.iter().filter(|v| def.view(**v).is_some()).map(|v| v.key()).collect();
    if names.is_empty() {
        "-".to_string()
    } else {
        names.join("+")
    }
}

/// The view to show first: the schematic, unless only the breadboard is in use.
pub fn initial_view(circuit: &Circuit) -> View {
    use short_circ_core::view_in_use;
    if view_in_use(circuit, View::Breadboard) && !view_in_use(circuit, View::Schematic) {
        View::Breadboard
    } else {
        View::Schematic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_format() {
        let e = parse_circuit_text(r#"{"format":"other/9"}"#).unwrap_err();
        assert!(e.contains("unsupported format"));
    }

    #[test]
    fn accepts_minimal_v1_and_v2() {
        for src in [r#"{"format":"short-circ/1"}"#, r#"{"format":"short-circ/2"}"#, "{}"] {
            let c = parse_circuit_text(src).unwrap();
            assert!(c.parts.is_empty());
            assert_eq!(c.format, short_circ_core::FORMAT, "{src}");
        }
    }

    #[test]
    fn empty_circuit_validates() {
        let c = parse_circuit_text(r#"{"format":"short-circ/1"}"#).unwrap();
        let r = validate(&c, &Library::builtin());
        assert!(r.ok && r.issues.is_empty());
    }

    #[test]
    fn led_example_has_nets_and_svg() {
        let c = parse_circuit_text(include_str!("../../../examples/led.circ")).unwrap();
        let lib = Library::load(&c, None);
        let nets = build_nets(&c, &lib, View::Schematic);
        assert!(nets.of_pin("R1", "a").is_some());
        assert!(nets.of_wire("w3").is_some());
        assert!(render_svg(&c, &lib, false, View::Schematic).unwrap().contains("<svg"));
        assert_eq!(initial_view(&c), View::Schematic);
        // The v1 file has no breadboard layout at all.
        assert_eq!(build_nets(&c, &lib, View::Breadboard).used().count(), 0);
    }

    #[test]
    fn breadboard_demo_has_jumper_nets_and_flags_board_holes() {
        let c = parse_circuit_text(include_str!("../../../examples/breadboard_demo.circ")).unwrap();
        let lib = Library::load(&c, None);
        let nets = build_nets(&c, &lib, View::Breadboard);
        let used: Vec<&Net> = nets.used().collect();
        assert!(used.len() >= 3, "{} used nets", used.len());
        assert!(nets.len() > used.len(), "idle board strips are nets too, but not listed");
        let j1 = nets.of_wire("j1").unwrap();
        assert!(j1.part_pins().count() >= 1);
        assert!(j1.pins.iter().any(|p| p.board));
        assert_eq!(jumper_colors(&c, &lib).len(), c.wires_in(View::Breadboard).count());
    }

    #[test]
    fn view_names_lists_geometry() {
        let lib = Library::builtin();
        assert_eq!(view_names(lib.get("resistor").unwrap()), "schematic+breadboard");
        assert_eq!(view_names(lib.get("breadboard_half").unwrap()), "breadboard");
        assert_eq!(view_names(&PartDef::default()), "-");
    }
}
