//! Canonical writers for `.circ` and `.part` (docs/FORMAT.md "Canonical writing").
//!
//! Owned by the editing stream. Contract: output is valid JSON, one part / wire / label / pin
//! per line, stable key order, 2-space indent, trailing newline, and
//! `write(parse(write(x))) == write(x)` byte for byte. This stub falls back to serde pretty
//! printing until the real writer lands.

use crate::model::{Circuit, PartDef};

pub fn circuit_to_string(c: &Circuit) -> String {
    let mut s = serde_json::to_string_pretty(c).expect("circuit serialises");
    s.push('\n');
    s
}

pub fn part_to_string(p: &PartDef) -> String {
    let mut s = serde_json::to_string_pretty(p).expect("part serialises");
    s.push('\n');
    s
}
