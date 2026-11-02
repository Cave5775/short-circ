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
// TODO(merge): wire to core::netlist once merged, e.g.
//   Netlist::build(circuit, lib).nets.iter().map(|n| Net { id: n.id, name: n.name.clone(),
//   pins: n.pins.iter().map(|p| PinRef { part: p.part.clone(), pin: p.pin.clone(), at: p.at }).collect(),
//   wires: n.wires.clone() }).collect()
pub fn build_nets(_circuit: &Circuit, _lib: &Library) -> Nets {
    Nets::default()
}

/// Validate a circuit.
// TODO(merge): wire to core::validate::validate(circuit, lib) once merged.
pub fn validate(_circuit: &Circuit, _lib: &Library) -> Report {
    Report::from_issues(vec![])
}

/// True once `render_svg` produces real output (enables File > Export SVG).
// TODO(merge): return true once core::svg exists.
pub fn svg_available() -> bool {
    false
}

/// Render the circuit to an SVG document.
// TODO(merge): wire to core::svg::render_svg(circuit, lib, dark) once merged.
pub fn render_svg(_circuit: &Circuit, _lib: &Library, _dark: bool) -> Result<String, String> {
    Err("SVG export is not available in this build".to_string())
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
    fn stub_report_is_ok() {
        let c = Circuit::default();
        let r = validate(&c, &Library::builtin());
        assert!(r.ok && r.issues.is_empty());
    }
}
