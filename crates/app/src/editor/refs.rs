//! Reference designators for new parts and a few sensible default values.

use short_circ_core::{Circuit, PartDef};

fn family(ty: &str, prefix: &str) -> bool {
    ty.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The id prefix of a part type: `R1`, `C1`, `D1`, `U1`, ... Power symbols use `GND` / `VCC`
/// (followed by a number from `fresh_id`), breadboards `BB`, anything unknown `P`.
pub fn ref_prefix(ty: &str) -> &'static str {
    let t = ty.to_ascii_lowercase();
    match t.as_str() {
        "resistor" => "R",
        "capacitor" | "capacitor_polarized" => "C",
        "led" | "diode" | "zener" => "D",
        "battery" | "voltage_source_ac" => "B",
        "inductor" => "L",
        "switch" | "pushbutton" => "SW",
        "npn" | "pnp" | "nmos" | "pmos" => "Q",
        "opamp" => "U",
        "crystal" => "Y",
        "fuse" => "F",
        "buzzer" => "BZ",
        "potentiometer" => "RV",
        "ground" => "GND",
        "vcc" => "VCC",
        _ if family(&t, "dip") => "U",
        _ if family(&t, "header") => "J",
        _ if t.starts_with("connector") => "J",
        _ if t.starts_with("breadboard") => "BB",
        _ => "P",
    }
}

/// [`ref_prefix`], but a part whose definition says `category: board` is always `BB`.
pub fn ref_prefix_for(ty: &str, def: Option<&PartDef>) -> &'static str {
    if def.is_some_and(PartDef::is_board) {
        "BB"
    } else {
        ref_prefix(ty)
    }
}

/// A fresh id for a new part of type `ty`.
pub fn fresh_part_id(c: &Circuit, ty: &str, def: Option<&PartDef>) -> String {
    short_circ_core::edit::fresh_id(c, ref_prefix_for(ty, def))
}

/// `value` prop given to a freshly placed part (the schematic symbols print it).
pub fn default_value(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "resistor" => "1k",
        "capacitor" => "100n",
        "capacitor_polarized" => "10u",
        "led" => "red",
        "battery" => "9V",
        "inductor" => "10u",
        "fuse" => "1A",
        "potentiometer" => "10k",
        "crystal" => "16MHz",
        "voltage_source_ac" => "5V",
        _ => return None,
    })
}
