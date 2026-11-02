//! Editing commands with grouped undo/redo (docs/FORMAT.md "Editing").
//!
//! Owned by the editing stream; this file is a placeholder. Contract the app builds on:
//! - `Edit`: a command enum (add / remove / place / move / rotate / mirror part, set prop and
//!   colours, add / remove / reshape wire, split wire at a point, add / move / remove label,
//!   set / clear declared nets). Every command that touches geometry carries the `View`.
//! - `History`: owns undo/redo stacks of whole-circuit snapshots with `begin_group()` /
//!   `end_group()` so a drag or an autoroute run is one undo step.
//! - `apply(&mut Circuit, &dyn PartLookup, Edit) -> Result<(), EditError>`; moving a part
//!   rubber-bands wire endpoints attached to its pins (same view only) and `tidy` removes
//!   zero-length segments and merges collinear runs.
