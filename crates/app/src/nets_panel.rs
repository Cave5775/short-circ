//! The "Nets" side panel and the pure helpers behind net locking and "fit to net".

use crate::canvas::Highlight;
use crate::core_api::{Net, Nets};
use eframe::egui::{self, RichText};
use short_circ_core::Circuit;

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
    /// A key for `net`: its first pin, else its first wire.
    pub fn of_net(net: &Net) -> Option<NetKey> {
        if let Some(p) = net.pins.first() {
            Some(NetKey::Pin(p.part.clone(), p.pin.clone()))
        } else {
            net.wires.first().map(|w| NetKey::Wire(w.clone()))
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

/// World box to fit when focusing on `net`: its pins and wire points, padded and widened to at
/// least [`MIN_SPAN`] around the centre. `None` when the net has no geometry in `circuit`.
pub fn net_fit_bounds(net: &Net, circuit: &Circuit) -> Option<([f64; 2], [f64; 2])> {
    let mut pts: Vec<[f64; 2]> = net.pins.iter().map(|p| [p.at[0] as f64, p.at[1] as f64]).collect();
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

/// Pins as `R1.a` text, in net order.
pub fn pin_list(net: &Net) -> String {
    net.pins.iter().map(|p| format!("{}.{}", p.part, p.pin)).collect::<Vec<_>>().join("  ")
}

/// Draw the panel. Returns the net the user clicked, if any.
pub fn show(ui: &mut egui::Ui, nets: &Nets, locked: Option<&Net>) -> Option<NetKey> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.heading("Nets");
        ui.label(format!("({})", nets.len()));
    });
    ui.separator();
    if nets.len() == 0 {
        ui.label("No nets.");
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for net in nets.iter() {
            let is_locked = locked.is_some_and(|l| l.id == net.id);
            let title = format!("{}  -  {} pin{}", net_title(net), net.pins.len(), if net.pins.len() == 1 { "" } else { "s" });
            let resp = ui.selectable_label(is_locked, RichText::new(title).strong());
            if resp.clicked() {
                clicked = NetKey::of_net(net);
            }
            if !net.pins.is_empty() {
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
            pins: pins.iter().map(|(p, n, at)| PinRef { part: (*p).into(), pin: (*n).into(), at: *at }).collect(),
            wires: wires.iter().map(|w| w.to_string()).collect(),
        }
    }

    fn led() -> (Circuit, Nets) {
        let c = core_api::parse_circuit_text(include_str!("../../../examples/led.circ")).unwrap();
        let lib = short_circ_core::Library::load(&c, None);
        let nets = core_api::build_nets(&c, &lib);
        (c, nets)
    }

    #[test]
    fn fit_bounds_cover_pins_and_wires() {
        let c = Circuit {
            wires: vec![short_circ_core::Wire { id: "w1".into(), pts: vec![[0, 0], [30, 0], [30, 20]], color: None }],
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
}
