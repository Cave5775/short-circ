//! Built-in components embedded in the executable. The parts work stream extends this list
//! (one `include_str!` per file in /parts) and implements `generate`.

use crate::model::PartDef;

pub const BUILTIN: &[(&str, &str)] = &[
    ("resistor", include_str!("../../../parts/resistor.part")),
    ("battery", include_str!("../../../parts/battery.part")),
    ("led", include_str!("../../../parts/led.part")),
];

/// Parametric families by type name, e.g. "dip8", "header4". Returns None if `ty` is not one.
/// TODO(parts stream): implement dipN (even N, 4..=40) and headerN (1..=40).
pub fn generate(_ty: &str) -> Option<PartDef> {
    None
}
