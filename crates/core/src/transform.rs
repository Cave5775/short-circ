//! Part transform, normative per docs/FORMAT.md ("Part transform").

use crate::model::{Part, PartDef, PinDef, Pt};

/// Local point -> world point for a placed part: at + R(mirror(p)).
pub fn to_world(part: &Part, p: [f64; 2]) -> [f64; 2] {
    let (mut x, mut y) = (p[0], p[1]);
    if part.mirror {
        x = -x;
    }
    let (rx, ry) = match part.rot.rem_euclid(360) {
        90 => (-y, x),
        180 => (-x, -y),
        270 => (y, -x),
        _ => (x, y),
    };
    [part.at[0] as f64 + rx, part.at[1] as f64 + ry]
}

/// World position of a pin. Pin locals are integers so the result is exact.
pub fn pin_world(part: &Part, pin: &PinDef) -> Pt {
    let w = to_world(part, [pin.at[0] as f64, pin.at[1] as f64]);
    [w[0].round() as i32, w[1].round() as i32]
}

/// World bounding box `[min, max]` of the transformed `[0,0]-[w,h]` box.
pub fn world_bbox(part: &Part, def: &PartDef) -> ([f64; 2], [f64; 2]) {
    let (w, h) = (def.size[0], def.size[1]);
    let cs = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|c| to_world(part, c));
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
    use std::collections::BTreeMap;

    fn part(at: Pt, rot: i32, mirror: bool) -> Part {
        Part { id: "P".into(), ty: "t".into(), at, rot, mirror, props: BTreeMap::new(), ..Part::default() }
    }

    #[test]
    fn rotations_are_clockwise_on_screen() {
        // (1,0) points right; 90 deg clockwise (y down) points down.
        assert_eq!(to_world(&part([0, 0], 90, false), [1.0, 0.0]), [0.0, 1.0]);
        assert_eq!(to_world(&part([0, 0], 180, false), [1.0, 0.0]), [-1.0, 0.0]);
        assert_eq!(to_world(&part([0, 0], 270, false), [1.0, 0.0]), [0.0, -1.0]);
        assert_eq!(to_world(&part([5, 5], 0, true), [2.0, 1.0]), [3.0, 6.0]);
    }
}
