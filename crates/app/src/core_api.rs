//! Thin adapter between the app and `short-circ-core`.
//!
//! Everything the app needs from the netlist, validation and SVG modules goes through this
//! file, so wiring in the real implementations is a change to this file only.
//! Parsing, libraries, transforms and the data model are used from core directly elsewhere.

use short_circ_core::{Circuit, Library, Pt, Report};
use std::path::Path;

/// One pin on a net.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinRef {
    pub part: String,
    pub pin: String,
    pub at: Pt,
}

/// One electrical net (mirrors `core::netlist::Net`).
#[derive(Debug, Clone, Default)]
pub struct Net {
    #[allow(dead_code)] // mirrors core::netlist::Net; used once the adapter is wired
    pub id: usize,
    pub name: String,
    pub pins: Vec<PinRef>,
    pub wires: Vec<String>,
}

/// Net lookup used for hover highlighting and the info panel.
#[derive(Debug, Clone, Default)]
pub struct Nets {
    nets: Vec<Net>,
}

impl Nets {
    pub fn of_pin(&self, part: &str, pin: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.pins.iter().any(|p| p.part == part && p.pin == pin))
    }

    pub fn of_wire(&self, wire: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.wires.iter().any(|w| w == wire))
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.nets.len()
    }
}

/// Build the netlist for a circuit.
pub fn build_nets(circuit: &Circuit, lib: &Library) -> Nets {
    let nl = short_circ_core::Netlist::build(circuit, lib);
    let nets = nl
        .nets
        .iter()
        .map(|n| Net {
            id: n.id,
            name: n.name.clone().unwrap_or_default(),
            pins: n
                .pins
                .iter()
                .map(|p| PinRef { part: p.part.clone(), pin: p.pin.clone(), at: p.at })
                .collect(),
            wires: n.wires.clone(),
        })
        .collect();
    Nets { nets }
}

/// Validate a circuit.
pub fn validate(circuit: &Circuit, lib: &Library) -> Report {
    short_circ_core::validate(circuit, lib)
}

/// True when SVG export is available (enables File > Export SVG).
pub fn svg_available() -> bool {
    true
}

/// Render the circuit to an SVG document.
pub fn render_svg(circuit: &Circuit, lib: &Library, dark: bool) -> Result<String, String> {
    Ok(short_circ_core::svg::render_svg(circuit, lib, dark))
}

/// Read and parse a circuit file, checking the format tag.
pub fn load_circuit(path: &Path) -> Result<Circuit, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_circuit_text(&src).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parse circuit text and check the `format` field.
pub fn parse_circuit_text(src: &str) -> Result<Circuit, String> {
    let c = short_circ_core::parse_circuit(src).map_err(|e| format!("invalid circuit file: {e}"))?;
    if c.format != short_circ_core::FORMAT {
        return Err(format!("unsupported format \"{}\" (expected \"{}\")", c.format, short_circ_core::FORMAT));
    }
    Ok(c)
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
    fn accepts_minimal() {
        let c = parse_circuit_text(r#"{"format":"short-circ/1"}"#).unwrap();
        assert!(c.parts.is_empty());
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
        assert!(build_nets(&c, &lib).of_pin("R1", "a").is_some());
        assert!(render_svg(&c, &lib, false).unwrap().contains("<svg"));
    }
}
