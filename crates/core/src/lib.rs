//! short-circ core: format model, netlist, validation, SVG export. No GUI dependencies.
//! Format `short-circ/2`; see docs/FORMAT.md.

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

pub mod fzp;
pub mod svgimport;

pub use migrate::{parse_circuit, parse_part, ParseError};
pub use model::*;
pub use netlist::{Net, Netlist, PinRef};
pub use resolve::Library;
pub use validate::{validate, validate_view, view_in_use};
