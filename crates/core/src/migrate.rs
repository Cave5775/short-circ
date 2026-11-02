//! Reading of `.circ` / `.part` text, including upgrade of `short-circ/1` files in memory
//! (docs/FORMAT.md "Compatibility"). This is the only place that knows about v1.
//!
//! Owned by the core refactor stream. Contract:
//! - v1 circuit: `parts[].at/rot/mirror` become `schematic`, every wire gets `view: schematic`,
//!   labels are kept, `format` becomes `short-circ/2`.
//! - v1 part: `size`, `pins[].at/dir` and `shapes` move into `schematic`, pin `id`/`name` stay
//!   in the logical `pins`. The v1 `svg` path-data field is dropped (it was never rendered).
//! - Inline `lib[]` entries of a circuit are upgraded the same way.
//! This stub only accepts v2.

use crate::model::{Circuit, PartDef, FORMAT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_circuit(src: &str) -> Result<Circuit, ParseError> {
    let c: Circuit = serde_json::from_str(src).map_err(|e| ParseError(e.to_string()))?;
    if c.format != FORMAT {
        return Err(ParseError(format!("unsupported format \"{}\" (expected \"{FORMAT}\")", c.format)));
    }
    Ok(c)
}

pub fn parse_part(src: &str) -> Result<PartDef, ParseError> {
    let p: PartDef = serde_json::from_str(src).map_err(|e| ParseError(e.to_string()))?;
    if p.format != FORMAT {
        return Err(ParseError(format!("unsupported format \"{}\" (expected \"{FORMAT}\")", p.format)));
    }
    Ok(p)
}
