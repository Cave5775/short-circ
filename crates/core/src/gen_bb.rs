//! Breadboard views for the generated families `dipN` and `headerN`.
//!
//! Plain functions over the model types only. Units are grid units (1 unit = 0.1 in); the SVG
//! viewBox uses 10 viewBox units per grid unit, so a view of size `[w, h]` has the viewBox
//! `0 0 {10w} {10h}`.
//!
//! Pin ids are "1".."n", the same ids `builtin::generate` gives the schematic pins.

use crate::model::{PinPos, ViewDef};
use std::collections::BTreeMap;
use std::fmt::Write;

const LEAD: &str = "#b0b0b0";

fn pin(x: i32, y: i32) -> PinPos {
    PinPos { at: [x, y], ..PinPos::default() }
}

/// DIP-`n` on a breadboard (`n` even, at least 4): the body lies along x, notch at the left, in
/// the usual orientation that straddles a board's centre channel.
///
/// With `h = n / 2`: pin k (1..=h) is on the bottom row at `(k, 3)`, going right; pin h+k is on
/// the top row at `(h+1-k, 0)`, going left. That is counter-clockwise numbering seen from above,
/// pin 1 at the bottom left next to the notch. The rows are 3 units apart (0.3 in), pitch 1,
/// and there is one unit of margin on each side, so the size is `[h + 2, 3]`.
pub fn dip_breadboard(n: usize) -> ViewDef {
    assert!(n >= 4 && n % 2 == 0, "dip_breadboard needs an even pin count >= 4");
    let h = (n / 2) as i32;
    let w = h + 2;
    let mut pins = BTreeMap::new();
    for k in 1..=h {
        pins.insert(k.to_string(), pin(k, 3));
        pins.insert((h + k).to_string(), pin(h + 1 - k, 0));
    }
    let mut s = String::new();
    for k in 1..=h {
        let x = k * 10 - 2;
        let _ = write!(s, "<rect x='{x}' y='25' width='4' height='5' fill='{LEAD}'/>");
        let _ = write!(s, "<rect x='{x}' y='0' width='4' height='5' fill='{LEAD}'/>");
    }
    let x1 = h * 10 + 6;
    let _ = write!(
        s,
        "<path d='M4 5 H{x1} V25 H4 V18 A3 3 0 0 0 4 12 Z' fill='#212121' stroke='#000' stroke-width='1'/>"
    );
    s.push_str("<circle cx='12' cy='21.5' r='1.3' fill='#6e6e6e'/>");
    ViewDef { size: [w as f64, 3.0], pins, shapes: Vec::new(), svg: Some(wrap(w, 3, &s)) }
}

/// A single row of `n` header pins (`n` at least 1) at pitch 1: pin k (1..=n) at `(k, 1)`. The
/// black plastic strip spans half a unit before the first and after the last pin, so the size
/// is `[n + 1, 2]`.
pub fn header_breadboard(n: usize) -> ViewDef {
    assert!(n >= 1, "header_breadboard needs at least one pin");
    let w = n as i32 + 1;
    let mut pins = BTreeMap::new();
    let mut s = format!("<rect x='5' y='3' width='{}' height='14' fill='#212121' stroke='#000' stroke-width='1'/>", n * 10);
    for k in 1..=n as i32 {
        pins.insert(k.to_string(), pin(k, 1));
        let x = k * 10;
        let _ = write!(s, "<rect x='{}' y='7.5' width='5' height='5' fill='#d4a017'/>", x as f64 - 2.5);
    }
    ViewDef { size: [w as f64, 2.0], pins, shapes: Vec::new(), svg: Some(wrap(w, 2, &s)) }
}

fn wrap(w: i32, h: i32, body: &str) -> String {
    format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {} {}'>{body}</svg>", w * 10, h * 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(v: &ViewDef, id: &str) -> [i32; 2] {
        v.pins.get(id).unwrap_or_else(|| panic!("pin {id} missing")).at
    }

    fn check_svg(v: &ViewDef) {
        let svg = v.svg.as_deref().unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        let vb = format!("viewBox='0 0 {} {}'", (v.size[0] * 10.0) as i32, (v.size[1] * 10.0) as i32);
        assert!(svg.contains(&vb), "{vb} not in {svg}");
    }

    #[test]
    fn dip8_positions() {
        let v = dip_breadboard(8);
        assert_eq!(v.size, [6.0, 3.0]);
        assert_eq!(v.pins.len(), 8);
        // bottom row left to right 1..4, top row right to left 5..8
        assert_eq!(at(&v, "1"), [1, 3]);
        assert_eq!(at(&v, "4"), [4, 3]);
        assert_eq!(at(&v, "5"), [4, 0]);
        assert_eq!(at(&v, "8"), [1, 0]);
        check_svg(&v);
    }

    #[test]
    fn dip_rows_are_three_apart_with_pitch_one() {
        for n in [4usize, 6, 8, 14, 16, 28, 40] {
            let v = dip_breadboard(n);
            let h = n / 2;
            assert_eq!(v.pins.len(), n);
            for k in 1..=h {
                let a = at(&v, &k.to_string());
                let b = at(&v, &(n + 1 - k).to_string());
                assert_eq!(a[1] - b[1], 3, "n={n} k={k}");
                assert_eq!(a[0], b[0], "partner pins face each other");
                if k > 1 {
                    assert_eq!(at(&v, &k.to_string())[0] - at(&v, &(k - 1).to_string())[0], 1);
                }
            }
            for p in v.pins.values() {
                assert!(p.at[0] >= 0 && p.at[0] as f64 <= v.size[0]);
                assert!(p.at[1] >= 0 && p.at[1] as f64 <= v.size[1]);
            }
            check_svg(&v);
        }
    }

    #[test]
    fn dip_ids_are_one_to_n() {
        let v = dip_breadboard(16);
        for k in 1..=16 {
            assert!(v.pins.contains_key(&k.to_string()), "pin {k}");
        }
    }

    #[test]
    fn header_positions() {
        let v = header_breadboard(5);
        assert_eq!(v.size, [6.0, 2.0]);
        assert_eq!(v.pins.len(), 5);
        for k in 1..=5 {
            assert_eq!(at(&v, &k.to_string()), [k, 1]);
        }
        check_svg(&v);
        let one = header_breadboard(1);
        assert_eq!(one.pins.len(), 1);
        assert_eq!(at(&one, "1"), [1, 1]);
        assert_eq!(one.size, [2.0, 2.0]);
    }

    #[test]
    fn header_40() {
        let v = header_breadboard(40);
        assert_eq!(v.pins.len(), 40);
        assert_eq!(at(&v, "40"), [40, 1]);
        check_svg(&v);
    }
}
