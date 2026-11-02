//! short-circ core: format model, netlist, validation, SVG export. No GUI dependencies.
//! Format `short-circ/2`; see docs/FORMAT.md.
//!
//! v2 transition: modules marked `cfg(any())` still speak the v1 model and are switched back
//! on, one by one, by the core refactor stream (delete the attribute when a module compiles).

pub mod color;
pub mod edit;
pub mod migrate;
pub mod model;
pub mod route;
pub mod transform;
pub mod write;

#[cfg(any())]
pub mod builtin;
#[cfg(any())]
pub mod fzp;
#[cfg(any())]
pub mod netlist;
#[cfg(any())]
pub mod resolve;
#[cfg(any())]
pub mod svg;
#[cfg(any())]
pub mod svgimport;
#[cfg(any())]
pub mod validate;

pub use migrate::{parse_circuit, parse_part, ParseError};
pub use model::*;
#[cfg(any())]
pub use netlist::{Net, Netlist, PinRef};
#[cfg(any())]
pub use resolve::Library;
#[cfg(any())]
pub use validate::validate;
