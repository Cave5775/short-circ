//! Cross-view consistency rules: `net-open`, `net-merged`, `unplaced-part`
//! (docs/FORMAT.md "Cross-view consistency").
//!
//! The reference is the declared `nets` when the circuit has any (only pins mentioned there are
//! constrained), otherwise the schematic netlist. Only pins of non-board parts that are placed in
//! both views and have a position in both are compared; boards and one-view parts are ignored.
//! Cross-view issues have no `view` and no location.

use super::{issue, view_in_use};
use crate::model::{Circuit, Issue, NetDecl, PartLookup, Severity, View};
use crate::netlist::Netlist;
use std::collections::{BTreeMap, HashMap, HashSet};

/// A pin that takes part in the comparison.
struct Cmp {
    part: String,
    pin: String,
}

impl Cmp {
    fn reference(&self) -> String {
        format!("{}.{}", self.part, self.pin)
    }
}

/// The nets the other views are held to: `class[i]` is the class of compared pin `i`
/// (`None` = unconstrained), `names[c]` the display name of class `c`.
struct Reference {
    class: Vec<Option<usize>>,
    names: Vec<String>,
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

pub(super) fn check(circuit: &Circuit, lib: &dyn PartLookup, issues: &mut Vec<Issue>) {
    unplaced_parts(circuit, lib, issues);
    compare_nets(circuit, lib, issues);
}

/// `unplaced-part` (warning): the part's type has geometry for a view in use, but the part has
/// no placement there.
fn unplaced_parts(circuit: &Circuit, lib: &dyn PartLookup, issues: &mut Vec<Issue>) {
    for view in View::ALL {
        if !view_in_use(circuit, view) {
            continue;
        }
        for p in &circuit.parts {
            let Some(def) = lib.get(&p.ty) else { continue };
            if def.view(view).is_some() && p.placement(view).is_none() {
                issues.push(issue(
                    "unplaced-part",
                    Severity::Warning,
                    vec![p.id.clone()],
                    None,
                    format!("part {} has no placement in the {} view, which is in use", p.id, view.key()),
                ));
            }
        }
    }
}

fn compare_nets(circuit: &Circuit, lib: &dyn PartLookup, issues: &mut Vec<Issue>) {
    // Compared pins.
    let mut pins: Vec<Cmp> = Vec::new();
    let mut seen_parts: HashSet<&str> = HashSet::new();
    for p in &circuit.parts {
        if !seen_parts.insert(p.id.as_str()) {
            continue;
        }
        let (Some(_), Some(_)) = (p.placement(View::Schematic), p.placement(View::Breadboard)) else { continue };
        let Some(def) = lib.get(&p.ty) else { continue };
        if def.is_board() {
            continue;
        }
        let (Some(s), Some(b)) = (def.schematic.as_ref(), def.breadboard.as_ref()) else { continue };
        for pin in &def.pins {
            if s.pins.contains_key(&pin.id) && b.pins.contains_key(&pin.id) {
                pins.push(Cmp { part: p.id.clone(), pin: pin.id.clone() });
            }
        }
    }
    if pins.is_empty() {
        return;
    }
    let index: HashMap<String, usize> = pins.iter().enumerate().map(|(i, p)| (p.reference(), i)).collect();
    let sch = Netlist::build(circuit, lib, View::Schematic);
    let bb = Netlist::build(circuit, lib, View::Breadboard);

    let declared = !circuit.nets.is_empty();
    let reference = if declared { declared_reference(circuit, &pins, &index) } else { schematic_reference(&sch, &pins) };
    let views: &[(View, &Netlist)] = if declared { &[(View::Schematic, &sch), (View::Breadboard, &bb)] } else { &[(View::Breadboard, &bb)] };

    for &(view, nl) in views {
        let net_of = |i: usize| nl.net_of_pin(&pins[i].part, &pins[i].pin);

        // net-open: a reference net split into 2+ groups in this view.
        let mut groups: BTreeMap<usize, BTreeMap<Option<usize>, usize>> = BTreeMap::new();
        for i in 0..pins.len() {
            if let Some(c) = reference.class[i] {
                groups.entry(c).or_default().entry(net_of(i)).or_insert(i);
            }
        }
        for (c, by_net) in &groups {
            if by_net.len() < 2 {
                continue;
            }
            let mut firsts: Vec<usize> = by_net.values().copied().collect();
            firsts.sort_unstable();
            let ids: Vec<String> = firsts.iter().map(|&i| pins[i].reference()).collect();
            issues.push(issue(
                "net-open",
                Severity::Warning,
                ids.clone(),
                None,
                format!(
                    "net {} is split into {} groups in the {} view (not routed there yet): {}",
                    reference.names[*c],
                    ids.len(),
                    view.key(),
                    ids.join(" | ")
                ),
            ));
        }

        // net-merged: one net of this view holds pins of two different reference nets.
        let mut by_view_net: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
        for i in 0..pins.len() {
            if let (Some(c), Some(n)) = (reference.class[i], net_of(i)) {
                by_view_net.entry(n).or_default().entry(c).or_insert(i);
            }
        }
        for classes in by_view_net.values() {
            if classes.len() < 2 {
                continue;
            }
            let mut firsts: Vec<(usize, usize)> = classes.iter().map(|(&c, &i)| (i, c)).collect();
            firsts.sort_unstable();
            let ids: Vec<String> = firsts.iter().map(|&(i, _)| pins[i].reference()).collect();
            let names: Vec<&str> = firsts.iter().map(|&(_, c)| reference.names[c].as_str()).collect();
            issues.push(issue(
                "net-merged",
                Severity::Error,
                ids.clone(),
                None,
                format!(
                    "the {} view joins different nets ({}) through {}",
                    view.key(),
                    names.join(" and "),
                    ids.join(" and ")
                ),
            ));
        }
    }
}

/// Classes from the declared nets: pins named together are one class; pins not named are free.
fn declared_reference(circuit: &Circuit, pins: &[Cmp], index: &HashMap<String, usize>) -> Reference {
    let members_of = |d: &NetDecl| -> Vec<usize> { d.pins.iter().filter_map(|r| index.get(r.as_str()).copied()).collect() };
    let mut dsu = Dsu((0..pins.len()).collect());
    let mut named = vec![false; pins.len()];
    for d in &circuit.nets {
        let members = members_of(d);
        for &m in &members {
            named[m] = true;
        }
        for &m in members.iter().skip(1) {
            dsu.union(members[0], m);
        }
    }
    let mut ids: BTreeMap<usize, usize> = BTreeMap::new();
    let mut class = vec![None; pins.len()];
    let mut names: Vec<String> = Vec::new();
    for i in 0..pins.len() {
        if named[i] {
            let root = dsu.find(i);
            let c = *ids.entry(root).or_insert_with(|| {
                names.push(String::new());
                names.len() - 1
            });
            class[i] = Some(c);
        }
    }
    // Name: the first declared name among the members, else the first pin.
    for d in &circuit.nets {
        let (Some(name), Some(&m)) = (d.name.as_deref(), members_of(d).first()) else { continue };
        if let Some(c) = class[m] {
            if names[c].is_empty() {
                names[c] = name.to_string();
            }
        }
    }
    for i in 0..pins.len() {
        if let Some(c) = class[i] {
            if names[c].is_empty() {
                names[c] = pins[i].reference();
            }
        }
    }
    Reference { class, names }
}

/// Classes from the schematic netlist (every compared pin is constrained).
fn schematic_reference(sch: &Netlist, pins: &[Cmp]) -> Reference {
    let mut ids: BTreeMap<usize, usize> = BTreeMap::new();
    let mut class = vec![None; pins.len()];
    let mut names: Vec<String> = Vec::new();
    for (i, p) in pins.iter().enumerate() {
        let Some(net) = sch.net_of_pin(&p.part, &p.pin) else { continue };
        let c = *ids.entry(net).or_insert_with(|| {
            names.push(sch.nets[net].name.clone().unwrap_or_else(|| p.reference()));
            names.len() - 1
        });
        class[i] = Some(c);
    }
    Reference { class, names }
}
