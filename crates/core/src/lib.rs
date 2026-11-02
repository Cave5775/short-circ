//! short-circ core: format model, netlist, validation, SVG export. No GUI dependencies.
//! Modules are filled in by parallel work streams; see docs/FORMAT.md.

pub mod builtin;
pub mod model;
pub mod resolve;
pub mod transform;

pub use model::*;
pub use resolve::Library;

/// Parse a `.circ` JSON document.
pub fn parse_circuit(src: &str) -> Result<Circuit, serde_json::Error> {
    serde_json::from_str(src)
}

/// Parse a `.part` JSON document.
pub fn parse_part(src: &str) -> Result<PartDef, serde_json::Error> {
    serde_json::from_str(src)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_circuit() {
        let c = parse_circuit(include_str!("../../../examples/led.circ")).unwrap();
        assert_eq!(c.format, FORMAT);
        assert!(!c.parts.is_empty());
    }

    #[test]
    fn parses_resistor_part() {
        let p = parse_part(include_str!("../../../parts/resistor.part")).unwrap();
        assert_eq!(p.ty, "resistor");
        assert_eq!(p.pins.len(), 2);
    }
}
