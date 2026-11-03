//! Editing single points of a wire: dragging one vertex (an end or a bend) without moving the rest
//! of the wire, adding a bend, removing a bend. Pure functions on point lists; `select` drives them.

use short_circ_core::{Pt, View};

/// Index of the wire point within `tol` grid units of `pos` (the nearest one).
pub fn vertex_at(pts: &[Pt], pos: [f64; 2], tol: f64) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, p) in pts.iter().enumerate() {
        let d = ((p[0] as f64 - pos[0]).powi(2) + (p[1] as f64 - pos[1]).powi(2)).sqrt();
        if d <= tol && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Where a bend can be added: the middle of every segment long enough to hold a grid point of its
/// own. Returns `(segment index, exact middle, grid point the bend lands on)`.
pub fn bend_handles(pts: &[Pt]) -> Vec<(usize, [f64; 2], Pt)> {
    let mut out = Vec::new();
    for (s, w) in pts.windows(2).enumerate() {
        let mid = [(w[0][0] as f64 + w[1][0] as f64) / 2.0, (w[0][1] as f64 + w[1][1] as f64) / 2.0];
        let at = [mid[0].round() as i32, mid[1].round() as i32];
        if at != w[0] && at != w[1] {
            out.push((s, mid, at));
        }
    }
    out
}

/// The bend handle (segment, grid point) within `tol` of `pos`.
pub fn bend_handle_at(pts: &[Pt], pos: [f64; 2], tol: f64) -> Option<(usize, Pt)> {
    bend_handles(pts)
        .into_iter()
        .map(|(s, mid, at)| (((mid[0] - pos[0]).powi(2) + (mid[1] - pos[1]).powi(2)).sqrt(), s, at))
        .filter(|(d, _, _)| *d <= tol)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s, at)| (s, at))
}

/// Nearest segment to `pos` and the grid point to put a bend on (`pos` snapped), unless that point
/// already is a vertex of the wire.
pub fn bend_near(pts: &[Pt], pos: [f64; 2]) -> Option<(usize, Pt)> {
    let at = [pos[0].round() as i32, pos[1].round() as i32];
    if pts.contains(&at) {
        return None;
    }
    let dist = |a: Pt, b: Pt| {
        let (ax, ay) = (a[0] as f64, a[1] as f64);
        let (dx, dy) = (b[0] as f64 - ax, b[1] as f64 - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 == 0.0 { 0.0 } else { (((pos[0] - ax) * dx + (pos[1] - ay) * dy) / len2).clamp(0.0, 1.0) };
        ((ax + t * dx - pos[0]).powi(2) + (ay + t * dy - pos[1]).powi(2)).sqrt()
    };
    pts.windows(2)
        .enumerate()
        .map(|(s, w)| (dist(w[0], w[1]), s))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| (s, at))
}

/// `pts` with a bend at `at` after segment `seg`.
pub fn with_bend(pts: &[Pt], seg: usize, at: Pt) -> Vec<Pt> {
    let mut out = pts.to_vec();
    out.insert((seg + 1).min(out.len()), at);
    out
}

/// `pts` without the bend at `index`; ends cannot be removed and a wire keeps two points.
pub fn without_bend(pts: &[Pt], index: usize) -> Option<Vec<Pt>> {
    if pts.len() < 3 || index == 0 || index + 1 >= pts.len() {
        return None;
    }
    let mut out = pts.to_vec();
    out.remove(index);
    Some(out)
}

#[derive(PartialEq)]
enum Axis {
    Horizontal,
    Vertical,
    Other,
}

fn axis(a: Pt, b: Pt) -> Axis {
    match (a[0] == b[0], a[1] == b[1]) {
        (false, true) => Axis::Horizontal,
        (true, false) => Axis::Vertical,
        _ => Axis::Other,
    }
}

/// `orig` with the point at `index` moved to `to` and nothing else moved. In the schematic a
/// segment that was horizontal or vertical stays so: an elbow point is added next to the moved
/// point instead of tilting the segment (which would also be a `wire-diagonal` warning). The
/// neighbours never move, so a wire that ends on a pin stays attached to it. Breadboard wires are
/// free-form: the point just moves. Always computed from the points before the drag started.
pub fn moved(orig: &[Pt], index: usize, to: Pt, view: View) -> Vec<Pt> {
    if index >= orig.len() {
        return orig.to_vec();
    }
    let elbow = |neighbour: Pt| -> Option<Pt> {
        if view != View::Schematic {
            return None;
        }
        let e = match axis(orig[index], neighbour) {
            Axis::Horizontal => [to[0], neighbour[1]],
            Axis::Vertical => [neighbour[0], to[1]],
            Axis::Other => return None,
        };
        (e != to && e != neighbour).then_some(e)
    };
    let mut out: Vec<Pt> = Vec::with_capacity(orig.len() + 2);
    for (i, &p) in orig.iter().enumerate() {
        if i != index {
            out.push(p);
            continue;
        }
        if index > 0 {
            out.extend(elbow(orig[index - 1]));
        }
        out.push(to);
        if index + 1 < orig.len() {
            out.extend(elbow(orig[index + 1]));
        }
    }
    out.dedup();
    if out.len() < 2 {
        return orig.to_vec();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_vertex_within_tolerance() {
        let pts = [[0, 0], [4, 0], [4, 4]];
        assert_eq!(vertex_at(&pts, [4.1, 0.2], 0.4), Some(1));
        assert_eq!(vertex_at(&pts, [2.0, 0.0], 0.4), None);
        assert_eq!(vertex_at(&pts, [0.0, 0.0], 0.4), Some(0));
    }

    #[test]
    fn bend_handles_skip_segments_too_short() {
        // Length 1 (no grid point between) and a diagonal one-step: no handle.
        assert!(bend_handles(&[[0, 0], [1, 0], [2, 1]]).is_empty());
        let h = bend_handles(&[[0, 0], [4, 0], [4, 3]]);
        assert_eq!(h.iter().map(|(s, _, at)| (*s, *at)).collect::<Vec<_>>(), vec![(0, [2, 0]), (1, [4, 2])]);
        assert_eq!(bend_handle_at(&[[0, 0], [4, 0]], [2.2, 0.1], 0.4), Some((0, [2, 0])));
        assert_eq!(bend_handle_at(&[[0, 0], [4, 0]], [1.0, 0.0], 0.4), None);
    }

    #[test]
    fn bend_near_projects_on_the_closest_segment() {
        let pts = [[0, 0], [10, 0], [10, 10]];
        assert_eq!(bend_near(&pts, [3.2, 0.3]), Some((0, [3, 0])));
        assert_eq!(bend_near(&pts, [10.2, 6.0]), Some((1, [10, 6])));
        assert_eq!(bend_near(&pts, [10.0, 0.0]), None, "already a vertex");
    }

    #[test]
    fn insert_and_remove_bends() {
        let pts = [[0, 0], [10, 0]];
        let b = with_bend(&pts, 0, [5, 0]);
        assert_eq!(b, vec![[0, 0], [5, 0], [10, 0]]);
        assert_eq!(without_bend(&b, 1), Some(pts.to_vec()));
        assert_eq!(without_bend(&b, 0), None, "ends stay");
        assert_eq!(without_bend(&b, 2), None);
        assert_eq!(without_bend(&pts, 1), None);
    }

    #[test]
    fn breadboard_moves_only_the_point() {
        let pts = [[0, 0], [10, 0]];
        assert_eq!(moved(&pts, 1, [10, 5], View::Breadboard), vec![[0, 0], [10, 5]]);
        assert_eq!(moved(&pts, 0, [3, 3], View::Breadboard), vec![[3, 3], [10, 0]]);
    }

    #[test]
    fn schematic_keeps_segments_straight_with_an_elbow() {
        // The last point of a horizontal wire dragged down: the far end stays, an elbow appears.
        assert_eq!(moved(&[[22, 10], [26, 10]], 1, [26, 12], View::Schematic), vec![[22, 10], [26, 10], [26, 12]]);
        // The first point of a vertical wire dragged sideways.
        assert_eq!(moved(&[[5, 0], [5, 8]], 0, [7, 0], View::Schematic), vec![[7, 0], [5, 0], [5, 8]]);
        // Dragged along its own line: no elbow, just shorter.
        assert_eq!(moved(&[[22, 10], [26, 10]], 1, [24, 10], View::Schematic), vec![[22, 10], [24, 10]]);
        // A bend of an L moves: both sides stay straight.
        assert_eq!(
            moved(&[[0, 0], [10, 0], [10, 10]], 1, [12, 3], View::Schematic),
            vec![[0, 0], [12, 0], [12, 3], [10, 3], [10, 10]]
        );
    }

    #[test]
    fn moving_back_restores_the_wire() {
        let orig = [[22, 10], [26, 10]];
        assert_eq!(moved(&orig, 1, [26, 10], View::Schematic), orig.to_vec());
        // Landing on the neighbour never leaves a zero-length wire.
        assert_eq!(moved(&orig, 1, [22, 10], View::Schematic), orig.to_vec());
    }
}
