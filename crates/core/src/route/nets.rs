//! The nets to realise: nets of the other view or the declared nets, restricted to pins that can
//! exist in the target view.

use super::{part_index, RouteFailure, RouteOptions, Scope, Source};
use crate::model::{Circuit, Part, PartDef, PartLookup, View};
use crate::netlist::Netlist;
use std::collections::HashSet;

/// One net to realise in the target view.
#[derive(Debug, Clone)]
pub struct SrcNet {
    /// Name of the net (label name or declared name), if any.
    pub name: Option<String>,
    /// Usable pins `(part id, pin id)`: the part's type has the target view, the pin has a
    /// position there, boards excluded. Deduplicated, in source order.
    pub pins: Vec<(String, String)>,
    /// Every pin reference of the source net (usable or not), for `Scope::Only` matching.
    pub all_refs: Vec<String>,
    /// The declared net mentioned an unknown part or pin: it is realised as far as possible but
    /// never counted as routed (a failure was recorded).
    pub tainted: bool,
}

impl SrcNet {
    /// Name if the net has one, else its first pin reference.
    pub fn display(&self) -> String {
        match (&self.name, self.pins.first()) {
            (Some(n), _) => n.clone(),
            (None, Some((p, q))) => format!("{p}.{q}"),
            (None, None) => self.all_refs.first().cloned().unwrap_or_default(),
        }
    }
}

/// The definition of a part's type if the part can exist in `view` (type has the view, is not a
/// board and has at least one pin position there).
pub(crate) fn def_for<'a>(lib: &'a dyn PartLookup, part: &Part, view: View) -> Option<&'a PartDef> {
    let def = lib.get(&part.ty)?;
    if def.is_board() {
        return None;
    }
    let vd = def.view(view)?;
    if def.pins.iter().any(|p| vd.pins.contains_key(&p.id)) {
        Some(def)
    } else {
        None
    }
}

pub(crate) fn can_exist(lib: &dyn PartLookup, part: &Part, view: View) -> bool {
    def_for(lib, part, view).is_some()
}

fn pin_usable(lib: &dyn PartLookup, c: &Circuit, idx: &std::collections::HashMap<String, usize>, part: &str, pin: &str, view: View) -> bool {
    let Some(&i) = idx.get(part) else { return false };
    match def_for(lib, &c.parts[i], view) {
        Some(def) => def.view(view).is_some_and(|vd| vd.pins.contains_key(pin)),
        None => false,
    }
}

/// Collect the source nets (only those with at least two usable pins), apply `Scope::Only`.
/// Problems with declared nets are pushed to `failures`.
pub(crate) fn source_nets(c: &Circuit, lib: &dyn PartLookup, opts: &RouteOptions, failures: &mut Vec<RouteFailure>) -> Vec<SrcNet> {
    let idx = part_index(c);
    let to = opts.to;
    let mut out: Vec<SrcNet> = Vec::new();
    match opts.source {
        Source::OtherView => {
            let nl = Netlist::build(c, lib, to.other());
            for net in &nl.nets {
                let mut seen: HashSet<(&str, &str)> = HashSet::new();
                let mut pins = Vec::new();
                let mut all_refs = Vec::new();
                for p in &net.pins {
                    all_refs.push(format!("{}.{}", p.part, p.pin));
                    if pin_usable(lib, c, &idx, &p.part, &p.pin, to) && seen.insert((&p.part, &p.pin)) {
                        pins.push((p.part.clone(), p.pin.clone()));
                    }
                }
                if pins.len() >= 2 {
                    out.push(SrcNet { name: net.name.clone(), pins, all_refs, tainted: false });
                }
            }
        }
        Source::Declared => {
            for d in &c.nets {
                let mut seen: HashSet<(String, String)> = HashSet::new();
                let mut pins = Vec::new();
                let mut unknown: Vec<&str> = Vec::new();
                for r in &d.pins {
                    match split_ref(c, lib, &idx, r) {
                        Some((part, pin)) => {
                            if pin_usable(lib, c, &idx, &part, &pin, to) && seen.insert((part.clone(), pin.clone())) {
                                pins.push((part, pin));
                            }
                        }
                        None => unknown.push(r),
                    }
                }
                let display = d
                    .name
                    .clone()
                    .or_else(|| d.pins.first().cloned())
                    .unwrap_or_default();
                if !unknown.is_empty() {
                    failures.push(RouteFailure {
                        net: display,
                        reason: format!("unknown part or pin {}", unknown.join(", ")),
                    });
                }
                if pins.len() >= 2 {
                    out.push(SrcNet { name: d.name.clone(), pins, all_refs: d.pins.clone(), tainted: !unknown.is_empty() });
                }
            }
        }
    }
    if let Scope::Only(refs) = &opts.scope {
        for r in refs {
            if !out.iter().any(|n| n.all_refs.iter().any(|x| x == r)) {
                failures.push(RouteFailure { net: r.clone(), reason: "no net with two or more usable pins contains this pin".into() });
            }
        }
        out.retain(|n| refs.iter().any(|r| n.all_refs.iter().any(|x| x == r)));
    }
    out
}

/// `PART.PIN` -> (part id, pin id), trying every `.` as the separator (part ids may contain
/// dots), accepting the first split that names an existing part with that pin.
fn split_ref(c: &Circuit, lib: &dyn PartLookup, idx: &std::collections::HashMap<String, usize>, r: &str) -> Option<(String, String)> {
    for (i, ch) in r.char_indices() {
        if ch != '.' {
            continue;
        }
        let (part, pin) = (&r[..i], &r[i + 1..]);
        if let Some(&pi) = idx.get(part) {
            if let Some(def) = lib.get(&c.parts[pi].ty) {
                if def.pins.iter().any(|p| p.id == pin) {
                    return Some((part.to_string(), pin.to_string()));
                }
            }
        }
    }
    None
}
