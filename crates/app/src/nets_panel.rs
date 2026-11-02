//! The "Nets" side panel and the pure helpers behind net locking and "fit to net".

use crate::canvas::Highlight;
use crate::core_api::{Net, Nets};
use eframe::egui::{self, RichText};
use short_circ_core::{Circuit, View};

/// Minimum extent (grid units) of the box a net is fitted into, so a tiny net does not zoom in
/// to absurd magnification.
const MIN_SPAN: [f64; 2] = [16.0, 10.0];
/// Padding (grid units) around the net.
const PAD: f64 = 2.0;

/// Identifies a net in a way that survives a reload (net ids are renumbered on every build).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetKey {
    Pin(String, String),
    Wire(String),
}

impl NetKey {
    /// A key for `net`: its first pin of a real part, else its first wire, else its first
    /// (board) pin.
    pub fn of_net(net: &Net) -> Option<NetKey> {
        if let Some(p) = net.part_pins().next() {
            Some(NetKey::Pin(p.part.clone(), p.pin.clone()))
        } else if let Some(w) = net.wires.first() {
            Some(NetKey::Wire(w.clone()))
        } else {
            net.pins.first().map(|p| NetKey::Pin(p.part.clone(), p.pin.clone()))
        }
    }

    pub fn resolve<'a>(&self, nets: &'a Nets) -> Option<&'a Net> {
        match self {
            NetKey::Pin(part, pin) => nets.of_pin(part, pin),
            NetKey::Wire(w) => nets.of_wire(w),
        }
    }
}

/// Add a net's wires and pins to a highlight (union with what is already there).
pub fn add_highlight(hl: &mut Highlight, net: &Net) {
    hl.wires.extend(net.wires.iter().cloned());
    hl.pins.extend(net.pins.iter().map(|p| (p.part.clone(), p.pin.clone())));
}

/// Display name of a net: its label, or `net N`.
pub fn net_title(net: &Net) -> String {
    if net.name.is_empty() {
        format!("net {}", net.id)
    } else {
        net.name.clone()
    }
}

/// World box to fit when focusing on `net`: its pins (board holes only when it has no other
/// pin) and wire points, padded and widened to at least [`MIN_SPAN`] around the centre. `None`
/// when the net has no geometry in `circuit`.
pub fn net_fit_bounds(net: &Net, circuit: &Circuit) -> Option<([f64; 2], [f64; 2])> {
    let any_part_pin = net.part_pins().next().is_some();
    let mut pts: Vec<[f64; 2]> = net
        .pins
        .iter()
        .filter(|p| !p.board || !any_part_pin)
        .map(|p| [p.at[0] as f64, p.at[1] as f64])
        .collect();
    for id in &net.wires {
        if let Some(w) = circuit.wires.iter().find(|w| &w.id == id) {
            pts.extend(w.pts.iter().map(|p| [p[0] as f64, p[1] as f64]));
        }
    }
    let first = *pts.first()?;
    let (mut lo, mut hi) = (first, first);
    for p in &pts {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    lo = [lo[0] - PAD, lo[1] - PAD];
    hi = [hi[0] + PAD, hi[1] + PAD];
    for k in 0..2 {
        let span = hi[k] - lo[k];
        if span < MIN_SPAN[k] {
            let c = (hi[k] + lo[k]) / 2.0;
            lo[k] = c - MIN_SPAN[k] / 2.0;
            hi[k] = c + MIN_SPAN[k] / 2.0;
        }
    }
    Some((lo, hi))
}

/// Pins as `R1.a` text, in net order. Board holes are summarised (`+14 board holes`).
pub fn pin_list(net: &Net) -> String {
    let mut s = net.part_pins().map(|p| format!("{}.{}", p.part, p.pin)).collect::<Vec<_>>().join("  ");
    let holes = net.pins.iter().filter(|p| p.board).count();
    let plural = if holes == 1 { "" } else { "s" };
    if holes > 0 && s.is_empty() {
        s = format!("{holes} board hole{plural}");
    } else if holes > 0 {
        s.push_str(&format!("  +{holes} board hole{plural}"));
    }
    s
}

/// Draw the panel for one view's nets. Returns the net the user clicked, if any.
pub fn show(ui: &mut egui::Ui, view: View, nets: &Nets, locked: Option<&Net>) -> Option<NetKey> {
    let mut clicked = None;
    let used = nets.used().count();
    ui.horizontal(|ui| {
        ui.heading("Nets");
        ui.label(format!("({used}, {} view)", view.key()));
    });
    ui.separator();
    if used == 0 {
        ui.label("No nets.");
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for net in nets.used() {
            let is_locked = locked.is_some_and(|l| l.id == net.id);
            let count = net.part_pins().count();
            let title = format!("{}  -  {} pin{}", net_title(net), count, if count == 1 { "" } else { "s" });
            let resp = ui.selectable_label(is_locked, RichText::new(title).strong());
            if resp.clicked() {
                clicked = NetKey::of_net(net);
            }
            if !pin_list(net).is_empty() {
                ui.add(egui::Label::new(RichText::new(pin_list(net)).monospace().small()).wrap());
            }
            ui.add_space(4.0);
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_api::{self, PinRef};

    fn net(id: usize, name: &str, pins: &[(&str, &str, [i32; 2])], wires: &[&str]) -> Net {
        Net {
            id,
            name: name.into(),
            pins: pins
                .iter()
                .map(|(p, n, at)| PinRef { part: (*p).into(), pin: (*n).into(), at: *at, board: p.starts_with("BB") })
                .collect(),
            wires: wires.iter().map(|w| w.to_string()).collect(),
        }
    }

    fn led() -> (Circuit, Nets) {
        let c = core_api::parse_circuit_text(include_str!("../../../examples/led.circ")).unwrap();
        let lib = short_circ_core::Library::load(&c, None);
        let nets = core_api::build_nets(&c, &lib, View::Schematic);
        (c, nets)
    }

    #[test]
    fn fit_bounds_cover_pins_and_wires() {
        let c = Circuit {
            wires: vec![short_circ_core::Wire { id: "w1".into(), pts: vec![[0, 0], [30, 0], [30, 20]], ..Default::default() }],
            ..Default::default()
        };
        let n = net(0, "", &[("R1", "a", [-4, 3])], &["w1"]);
        let (lo, hi) = net_fit_bounds(&n, &c).unwrap();
        assert_eq!(lo, [-6.0, -2.0]);
        assert_eq!(hi, [32.0, 22.0]);
    }

    #[test]
    fn small_net_gets_minimum_span() {
        let n = net(0, "", &[("R1", "a", [10, 10]), ("R1", "b", [11, 10])], &[]);
        let (lo, hi) = net_fit_bounds(&n, &Circuit::default()).unwrap();
        assert!(hi[0] - lo[0] >= MIN_SPAN[0] && hi[1] - lo[1] >= MIN_SPAN[1]);
        let cx = (lo[0] + hi[0]) / 2.0;
        assert!((cx - 10.5).abs() < 1e-9, "stays centred on the net");
    }

    #[test]
    fn empty_net_has_no_bounds() {
        assert!(net_fit_bounds(&net(0, "", &[], &[]), &Circuit::default()).is_none());
        // A wire id that is not in the circuit contributes nothing.
        assert!(net_fit_bounds(&net(0, "", &[], &["ghost"]), &Circuit::default()).is_none());
    }

    #[test]
    fn keys_resolve_to_the_same_net() {
        let (_, nets) = led();
        let gnd = nets.of_wire("w3").unwrap();
        let key = NetKey::of_net(gnd).unwrap();
        assert_eq!(key.resolve(&nets).unwrap().id, gnd.id);
        assert_eq!(NetKey::Wire("w3".into()).resolve(&nets).unwrap().id, gnd.id);
        assert!(NetKey::Wire("zzz".into()).resolve(&nets).is_none());
        assert!(NetKey::of_net(&net(0, "", &[], &[])).is_none());
    }

    #[test]
    fn highlight_unions_nets() {
        let mut hl = Highlight::default();
        add_highlight(&mut hl, &net(0, "", &[("R1", "a", [0, 0])], &["w1"]));
        add_highlight(&mut hl, &net(1, "", &[("D1", "k", [0, 0])], &["w2"]));
        assert_eq!(hl.wires, vec!["w1", "w2"]);
        assert_eq!(hl.pins.len(), 2);
    }

    #[test]
    fn titles_and_pin_list() {
        let n = net(3, "", &[("R1", "a", [0, 0]), ("D1", "k", [0, 0])], &[]);
        assert_eq!(net_title(&n), "net 3");
        assert_eq!(net_title(&net(0, "GND", &[], &[])), "GND");
        assert_eq!(pin_list(&n), "R1.a  D1.k");
    }

    #[test]
    fn board_holes_are_summarised_and_not_fitted() {
        let n = net(
            0,
            "",
            &[("BB1", "a5", [6, 6]), ("R1", "a", [6, 7]), ("BB1", "b5", [6, 7]), ("BB1", "c5", [6, 8])],
            &["j1"],
        );
        assert_eq!(pin_list(&n), "R1.a  +3 board holes");
        assert_eq!(pin_list(&net(0, "", &[("BB1", "a5", [6, 6])], &["j1"])), "1 board hole");
        assert_eq!(pin_list(&net(0, "", &[], &["j1"])), "");
        assert_eq!(NetKey::of_net(&n), Some(NetKey::Pin("R1".into(), "a".into())), "real pins first");
        let bare = net(1, "", &[("BB1", "a5", [6, 6])], &["j1"]);
        assert_eq!(NetKey::of_net(&bare), Some(NetKey::Wire("j1".into())), "then a wire");
        assert_eq!(NetKey::of_net(&net(2, "", &[("BB1", "a5", [6, 6])], &[])), Some(NetKey::Pin("BB1".into(), "a5".into())));
        // Fit only looks at the real pin (plus padding), not at the holes of the strip.
        let (lo, hi) = net_fit_bounds(&n, &Circuit::default()).unwrap();
        assert_eq!((lo[1] + hi[1]) / 2.0, 7.0);
        assert!(bare.part_pins().next().is_none() && bare.is_used());
        assert!(!net(3, "", &[("BB1", "a5", [6, 6])], &[]).is_used(), "an idle strip is not listed");
    }
}
