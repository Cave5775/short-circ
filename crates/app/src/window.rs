//! First-frame window placement: make the window fit on, and be centred in, the primary
//! monitor's work area (the first run on one machine opened partly off-screen).
//! The geometry is pure; the work area comes from Win32 on Windows.

use eframe::egui::{self, Pos2, Vec2};

/// Size the window asks for at creation (logical points).
pub const DESIRED_INNER: [f32; 2] = [1100.0, 720.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Inner (client) size in logical points.
    pub inner: [f32; 2],
    /// Top-left of the outer window in logical points.
    pub outer_pos: [f32; 2],
}

/// Plan a placement. `work` is `[x, y, w, h]` of the work area in logical points, `extra` the
/// size of the window decorations (outer minus inner).
pub fn plan(work: [f32; 4], extra: [f32; 2], desired: [f32; 2]) -> Placement {
    let inner = [
        desired[0].min((work[2] - extra[0]).max(320.0)),
        desired[1].min((work[3] - extra[1]).max(240.0)),
    ];
    let outer = [inner[0] + extra[0], inner[1] + extra[1]];
    let pos = [
        (work[0] + (work[2] - outer[0]) / 2.0).max(work[0]),
        (work[1] + (work[3] - outer[1]) / 2.0).max(work[1]),
    ];
    Placement { inner, outer_pos: pos }
}

/// Primary monitor work area `[x, y, w, h]` in physical pixels.
#[cfg(windows)]
fn primary_work_area() -> Option<[f32; 4]> {
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    extern "system" {
        fn SystemParametersInfoW(action: u32, param: u32, pv: *mut Rect, win_ini: u32) -> i32;
    }
    const SPI_GETWORKAREA: u32 = 0x0030;
    let mut r = Rect::default();
    // SAFETY: `r` is a valid RECT the call fills in.
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r, 0) };
    (ok != 0 && r.right > r.left && r.bottom > r.top)
        .then(|| [r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32])
}

#[cfg(not(windows))]
fn primary_work_area() -> Option<[f32; 4]> {
    None
}

/// Place the window once the viewport reports its rectangles. Returns true when done (or when
/// there is nothing to do), false to try again next frame.
pub fn place_once(ctx: &egui::Context) -> bool {
    let (inner, outer, ppp) = ctx.input(|i| {
        let v = i.viewport();
        (v.inner_rect, v.outer_rect, i.pixels_per_point)
    });
    let (Some(inner), Some(outer)) = (inner, outer) else { return false };
    let Some(work) = primary_work_area() else {
        if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
            ctx.send_viewport_cmd(cmd);
        }
        return true;
    };
    let work = [work[0] / ppp, work[1] / ppp, work[2] / ppp, work[3] / ppp];
    let extra = [outer.width() - inner.width(), outer.height() - inner.height()];
    let p = plan(work, extra, DESIRED_INNER);
    if (p.inner[0] - inner.width()).abs() > 0.5 || (p.inner[1] - inner.height()).abs() > 0.5 {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(p.inner[0], p.inner[1])));
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(p.outer_pos[0], p.outer_pos[1])));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centres_in_work_area() {
        let p = plan([0.0, 0.0, 1920.0, 1040.0], [16.0, 39.0], [1100.0, 720.0]);
        assert_eq!(p.inner, [1100.0, 720.0]);
        assert_eq!(p.outer_pos, [(1920.0 - 1116.0) / 2.0, (1040.0 - 759.0) / 2.0]);
    }

    #[test]
    fn respects_work_area_origin() {
        let p = plan([0.0, 40.0, 1920.0, 1040.0], [0.0, 0.0], [1000.0, 600.0]);
        assert_eq!(p.outer_pos[1], 40.0 + 220.0);
    }

    #[test]
    fn shrinks_to_small_screens_and_stays_on_screen() {
        let p = plan([0.0, 0.0, 800.0, 600.0], [16.0, 39.0], [1100.0, 720.0]);
        assert_eq!(p.inner, [784.0, 561.0]);
        assert_eq!(p.outer_pos, [0.0, 0.0]);
        // Absurdly tiny work area: window is not pushed to negative coordinates.
        let q = plan([10.0, 10.0, 200.0, 100.0], [16.0, 39.0], [1100.0, 720.0]);
        assert!(q.outer_pos[0] >= 10.0 && q.outer_pos[1] >= 10.0);
    }
}
