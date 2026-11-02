//! Placement transform, normative per docs/FORMAT.md ("Part transform"). Identical in every view.

use crate::model::{PinPos, Placement, Pt, ViewDef};

/// Local point -> world point for a placed part: at + R(mirror(p)).
pub fn to_world(pl: &Placement, p: [f64; 2]) -> [f64; 2] {
    let (mut x, mut y) = (p[0], p[1]);
    if pl.mirror {
        x = -x;
    }
    let (rx, ry) = match pl.rot.rem_euclid(360) {
        90 => (-y, x),
        180 => (-x, -y),
        270 => (y, -x),
        _ => (x, y),
    };
    [pl.at[0] as f64 + rx, pl.at[1] as f64 + ry]
}

/// World position of a pin. Pin locals are integers so the result is exact.
pub fn pin_world(pl: &Placement, pos: &PinPos) -> Pt {
    let w = to_world(pl, [pos.at[0] as f64, pos.at[1] as f64]);
    [w[0].round() as i32, w[1].round() as i32]
}

/// World bounding box `[min, max]` of the transformed `[0,0]-[w,h]` box of a view.
pub fn world_bbox(pl: &Placement, view: &ViewDef) -> ([f64; 2], [f64; 2]) {
    let (w, h) = (view.size[0], view.size[1]);
    let cs = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|c| to_world(pl, c));
    let mut min = cs[0];
    let mut max = cs[0];
    for c in cs {
        min = [min[0].min(c[0]), min[1].min(c[1])];
        max = [max[0].max(c[0]), max[1].max(c[1])];
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pl(at: Pt, rot: i32, mirror: bool) -> Placement {
        Placement { at, rot, mirror }
    }

    #[test]
    fn rotations_are_clockwise_on_screen() {
        // (1,0) points right; 90 deg clockwise (y down) points down.
        assert_eq!(to_world(&pl([0, 0], 90, false), [1.0, 0.0]), [0.0, 1.0]);
        assert_eq!(to_world(&pl([0, 0], 180, false), [1.0, 0.0]), [-1.0, 0.0]);
        assert_eq!(to_world(&pl([0, 0], 270, false), [1.0, 0.0]), [0.0, -1.0]);
        assert_eq!(to_world(&pl([5, 5], 0, true), [2.0, 1.0]), [3.0, 6.0]);
    }

    #[test]
    fn bbox_of_rotated_view() {
        let v = ViewDef { size: [8.0, 2.0], ..ViewDef::default() };
        let (min, max) = world_bbox(&pl([10, 10], 90, false), &v);
        assert_eq!((min, max), ([8.0, 10.0], [10.0, 18.0]));
    }
}
