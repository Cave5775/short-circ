//! short-circ core: format model, netlist, validation, SVG export. No GUI dependencies.
//! Format `short-circ/2`; see docs/FORMAT.md.
//!
//! v2 transition: `fzp` and `svgimport` still speak the v1 model and stay switched off
//! (`cfg(any())`) until the import stream ports them; delete the attribute when a module compiles.

pub mod builtin;
pub mod color;
pub mod edit;
pub mod gen_bb;
pub mod migrate;
pub mod model;
pub mod netlist;
pub mod resolve;
pub mod route;
pub mod svg;
pub mod transform;
pub mod validate;
pub mod write;

#[cfg(any())]
pub mod fzp;
#[cfg(any())]
pub mod svgimport;

pub use migrate::{parse_circuit, parse_part, ParseError};
pub use model::*;
pub use netlist::{Net, Netlist, PinRef};
pub use resolve::Library;
pub use validate::{validate, validate_view, view_in_use};
