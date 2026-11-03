//! Hit testing for the editor, on world coordinates in grid units. Pins, wires and part boxes use
//! the viewer's helpers in `canvas`; labels and boards are handled here.

use super::{HitTol, Item};
use crate::canvas;
use short_circ_core::transform::pin_world;
use short_circ_core::{Circuit, Label, PartDef, PartLookup, Pt, View};

/// Slack around a part box that still counts as a hit.
const PART_TOL: f64 = 0.3;
/// Width of the grabbable frame of a board (the inside of a board is "empty space").
pub const BOARD_RIM: f64 = 0.6;

pub fn def_is_board(lib: &dyn PartLookup, ty: &str) -> bool {
    lib.get(ty).is_some_and(PartDef::is_board)
}

/// World box of the part at index `i` in `view`.
pub fn part_world_box(c: &Circuit, lib: &dyn PartLookup, view: View, i: usize) -> Option<([f64; 2], [f64; 2])> {
    let p = c.parts.get(i)?;
    let pl = p.placement(view)?;
    Some(canvas::part_box(pl, lib.get(&p.ty), view))
}

/// Approximate box of a label's text (the viewer draws it up and to the right of its point).
pub fn label_box(l: &Label) -> ([f64; 2], [f64; 2]) {
    let n = l.name.chars().count().max(1) as f64;
    let (x, y) = (l.at[0] as f64, l.at[1] as f64);
    ([x + 0.1, y - 1.2], [x + 0.35 + 0.5 * n, y - 0.25])
}

pub fn label_hit(c: &Circuit, pos: [f64; 2]) -> Option<usize> {
    c.labels.iter().enumerate().rev().find_map(|(i, l)| {
        let (lo, hi) = label_box(l);
        (pos[0] >= lo[0] && pos[0] <= hi[0] && pos[1] >= lo[1] && pos[1] <= hi[1]).then_some(i)
    })
}

fn in_box(pos: [f64; 2], lo: [f64; 2], hi: [f64; 2], tol: f64) -> bool {
    pos[0] >= lo[0] - tol && pos[0] <= hi[0] + tol && pos[1] >= lo[1] - tol && pos[1] <= hi[1] + tol
}

/// Index of the part body under `pos`: the smallest box wins; a board only counts near its
/// frame, so its inside stays free for marquee selection.
pub fn part_hit(c: &Circuit, lib: &dyn PartLookup, view: View, pos: [f64; 2]) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, p) in c.parts.iter().enumerate() {
        let Some((lo, hi)) = part_world_box(c, lib, view, i) else { continue };
        if !in_box(pos, lo, hi, PART_TOL) {
            continue;
        }
        if def_is_board(lib, &p.ty) {
            let inner_lo = [lo[0] + BOARD_RIM, lo[1] + BOARD_RIM];
            let inner_hi = [hi[0] - BOARD_RIM, hi[1] - BOARD_RIM];
            let inside = inner_lo[0] < inner_hi[0] && inner_lo[1] < inner_hi[1] && in_box(pos, inner_lo, inner_hi, 0.0);
            if inside {
                continue;
            }
        }
        let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
        if best.is_none_or(|(a, _)| area < a) {
            best = Some((area, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Index of the board whose box contains `pos` (the smallest one).
pub fn board_under(c: &Circuit, lib: &dyn PartLookup, view: View, pos: [f64; 2]) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, p) in c.parts.iter().enumerate() {
        if !def_is_board(lib, &p.ty) {
            continue;
        }
        let Some((lo, hi)) = part_world_box(c, lib, view, i) else { continue };
        if in_box(pos, lo, hi, 0.0) {
            let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
            if best.is_none_or(|(a, _)| area < a) {
                best = Some((area, i));
            }
        }
    }
    best.map(|(_, i)| i)
}

/// What a click at `pos` grabs: a label's text, a pin of a real part (selects the part), a wire,
/// then a part body.
pub fn hit_item(c: &Circuit, lib: &dyn PartLookup, view: View, pos: [f64; 2], tol: HitTol) -> Option<Item> {
    if view == View::Schematic {
        if let Some(i) = label_hit(c, pos) {
            return Some(Item::Label(i));
        }
    }
    if let Some(h) = canvas::pin_at(c, lib, view, pos, tol.pin) {
        if !h.board {
            return Some(Item::Part(c.parts[h.part].id.clone()));
        }
    }
    if let Some(wi) = canvas::wire_at(c, view, pos, tol.wire) {
        return Some(Item::Wire(c.wires[wi].id.clone()));
    }
    part_hit(c, lib, view, pos).map(|i| Item::Part(c.parts[i].id.clone()))
}

/// Parts, wires and labels of `view` lying completely inside the box.
pub fn items_inside(c: &Circuit, lib: &dyn PartLookup, view: View, lo: [f64; 2], hi: [f64; 2]) -> Vec<Item> {
    let inside = |p: [f64; 2]| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1];
    let mut out = Vec::new();
    for (i, p) in c.parts.iter().enumerate() {
        if let Some((a, b)) = part_world_box(c, lib, view, i) {
            if inside(a) && inside(b) {
                out.push(Item::Part(p.id.clone()));
            }
        }
    }
    for w in c.wires_in(view) {
        if w.pts.iter().all(|p| inside([p[0] as f64, p[1] as f64])) {
            out.push(Item::Wire(w.id.clone()));
        }
    }
    if view == View::Schematic {
        for (k, l) in c.labels.iter().enumerate() {
            if inside([l.at[0] as f64, l.at[1] as f64]) {
                out.push(Item::Label(k));
            }
        }
    }
    out
}

/// Everything of `view`.
pub fn all_items(c: &Circuit, view: View) -> Vec<Item> {
    let mut out: Vec<Item> =
        c.parts.iter().filter(|p| p.placement(view).is_some()).map(|p| Item::Part(p.id.clone())).collect();
    out.extend(c.wires_in(view).map(|w| Item::Wire(w.id.clone())));
    if view == View::Schematic {
        out.extend((0..c.labels.len()).map(Item::Label));
    }
    out
}

/// A pin of a placed part in a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinPoint {
    pub part: usize,
    pub pin: String,
    pub at: Pt,
    pub board: bool,
}

/// World positions of every pin of every part placed in `view`.
pub fn pin_points(c: &Circuit, lib: &dyn PartLookup, view: View) -> Vec<PinPoint> {
    let mut out = Vec::new();
    for (i, p) in c.parts.iter().enumerate() {
        let (Some(pl), Some(def)) = (p.placement(view), lib.get(&p.ty)) else { continue };
        let board = def.is_board();
        for (pin, pos) in def.placed_pins(view) {
            out.push(PinPoint { part: i, pin: pin.id.clone(), at: pin_world(pl, pos), board });
        }
    }
    out
}

/// `p` lies on the closed segment `a-b` (exact integer arithmetic).
pub fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    let (ax, ay, bx, by, px, py) =
        (a[0] as i64, a[1] as i64, b[0] as i64, b[1] as i64, p[0] as i64, p[1] as i64);
    (bx - ax) * (py - ay) - (by - ay) * (px - ax) == 0
        && px >= ax.min(bx)
        && px <= ax.max(bx)
        && py >= ay.min(by)
        && py <= ay.max(by)
}
