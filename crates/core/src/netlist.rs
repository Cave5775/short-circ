//! Connectivity extraction, per docs/FORMAT.md "Connectivity rules". One netlist per view.
//!
//! Every pin and every wire (a whole polyline is one conductor) is a node in a union-find.
//!
//! Schematic: nodes are joined when they meet at an integer point (pin/vertex/endpoint
//! coincidence), when a wire vertex lies on another wire's segment (T-junction), through labels
//! of the same name and through part buses. Two wires that merely cross (no vertex of either at
//! the crossing) stay apart, and a pin in the interior of a segment without a vertex there is
//! not connected either.
//!
//! Breadboard: a wire is a flexible jumper that joins only at its two end points. Everything at
//! the same grid point is one node (pins of any parts, wire end points); interior vertices
//! connect nothing, wires never T-connect, labels are not used. Part buses (breadboard strips)
//! join their holes.

use crate::model::{Circuit, PartLookup, Pt, View};
use crate::transform::pin_world;
use std::collections::HashMap;

/// Side length (grid units) of the spatial hash cells used for segment lookups.
const CELL: i64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinRef {
    pub part: String,
    pub pin: String,
    pub at: Pt,
}

#[derive(Debug, Clone)]
pub struct Net {
    pub id: usize,
    /// Name from the first label (in file order) attached to this net. Schematic only.
    pub name: Option<String>,
    pub pins: Vec<PinRef>,
    /// Wire ids in file order.
    pub wires: Vec<String>,
}

/// What sits at an integer point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occ {
    /// Index into `Geom::pins`.
    Pin(usize),
    /// Wire index, vertex index within the wire.
    Vertex(usize, usize),
}

/// One non-degenerate wire segment (schematic only).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seg {
    pub wire: usize,
    /// Segment index within the wire (segment `i` runs from pts[i] to pts[i+1]).
    pub idx: usize,
    pub a: Pt,
    pub b: Pt,
}

/// Geometry of one view, shared by the netlist builder and the validator.
#[derive(Debug, Clone, Default)]
pub(crate) struct Geom {
    pub pins: Vec<PinRef>,
    /// Per pin: the part is a board (breadboard-like carrier).
    pub pin_board: Vec<bool>,
    /// Per pin: the pin belongs to a bus of two or more pins of its part.
    pub pin_in_bus: Vec<bool>,
    /// Groups of pin indexes (placed pins of one part) that form one node.
    pub buses: Vec<Vec<usize>>,
    /// Ids of the wires drawn in this view, in file order.
    pub wire_ids: Vec<String>,
    pub wire_pts: Vec<Vec<Pt>>,
    /// Schematic: every vertex. Breadboard: only the two end points of each wire.
    pub pt_map: HashMap<Pt, Vec<Occ>>,
    /// Schematic only; empty in the breadboard view.
    pub segs: Vec<Seg>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

fn cross(u: [i64; 2], v: [i64; 2]) -> i64 {
    u[0] * v[1] - u[1] * v[0]
}

pub(crate) fn sub(a: Pt, b: Pt) -> [i64; 2] {
    [a[0] as i64 - b[0] as i64, a[1] as i64 - b[1] as i64]
}

/// `p` lies on the closed segment `a-b` (exact integer arithmetic).
pub(crate) fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    cross(sub(b, a), sub(p, a)) == 0
        && p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

pub(crate) fn cell_of(p: Pt) -> (i32, i32) {
    ((p[0] as i64).div_euclid(CELL) as i32, (p[1] as i64).div_euclid(CELL) as i32)
}

/// Calls `f` for every grid cell the segment may touch (conservative superset, integer math).
fn seg_cells(a: Pt, b: Pt, mut f: impl FnMut(i32, i32)) {
    let (ax, ay, bx, by) = (a[0] as i64, a[1] as i64, b[0] as i64, b[1] as i64);
    let steep = (by - ay).abs() > (bx - ax).abs();
    // (u,v) = (x,y) normally, (y,x) when steep, so that |du| >= |dv|.
    let (mut p, mut q) = if steep { ([ay, ax], [by, bx]) } else { ([ax, ay], [bx, by]) };
    if p[0] > q[0] {
        std::mem::swap(&mut p, &mut q);
    }
    let du = q[0] - p[0];
    if du == 0 {
        // Degenerate segment (a == b): a single point.
        let c = cell_of(a);
        f(c.0, c.1);
        return;
    }
    let dv = q[1] - p[1];
    let v_at = |u: i64| -> (i64, i64) {
        let num = dv * (u - p[0]);
        (p[1] + num.div_euclid(du), p[1] - (-num).div_euclid(du))
    };
    for c in p[0].div_euclid(CELL)..=q[0].div_euclid(CELL) {
        let ulo = p[0].max(c * CELL);
        let uhi = q[0].min((c + 1) * CELL);
        let (f0, c0) = v_at(ulo);
        let (f1, c1) = v_at(uhi);
        let vmin = f0.min(f1);
        let vmax = c0.max(c1);
        for r in vmin.div_euclid(CELL)..=vmax.div_euclid(CELL) {
            if steep {
                f(r as i32, c as i32);
            } else {
                f(c as i32, r as i32);
            }
        }
    }
}

impl Geom {
    pub fn new(circuit: &Circuit, lib: &dyn PartLookup, view: View) -> Geom {
        let mut g = Geom::default();
        for part in &circuit.parts {
            let Some(pl) = part.placement(view) else { continue };
            let Some(def) = lib.get(&part.ty) else { continue };
            let board = def.is_board();
            let mut index: HashMap<&str, usize> = HashMap::new();
            for (pin, pos) in def.placed_pins(view) {
                let at = pin_world(pl, pos);
                let idx = g.pins.len();
                g.pins.push(PinRef { part: part.id.clone(), pin: pin.id.clone(), at });
                g.pin_board.push(board);
                g.pin_in_bus.push(false);
                g.pt_map.entry(at).or_default().push(Occ::Pin(idx));
                index.entry(pin.id.as_str()).or_insert(idx);
            }
            for bus in &def.buses {
                let mut ids: Vec<&str> = bus.iter().map(String::as_str).collect();
                ids.sort_unstable();
                ids.dedup();
                if ids.len() < 2 {
                    continue;
                }
                let members: Vec<usize> = ids.iter().filter_map(|id| index.get(id).copied()).collect();
                for &m in &members {
                    g.pin_in_bus[m] = true;
                }
                if members.len() >= 2 {
                    g.buses.push(members);
                }
            }
        }
        let schematic = view == View::Schematic;
        for (wi, w) in circuit.wires_in(view).enumerate() {
            g.wire_ids.push(w.id.clone());
            g.wire_pts.push(w.pts.clone());
            if schematic {
                for (vi, p) in w.pts.iter().enumerate() {
                    g.pt_map.entry(*p).or_default().push(Occ::Vertex(wi, vi));
                }
                for (si, pair) in w.pts.windows(2).enumerate() {
                    let (a, b) = (pair[0], pair[1]);
                    if a == b {
                        continue;
                    }
                    let id = g.segs.len() as u32;
                    g.segs.push(Seg { wire: wi, idx: si, a, b });
                    let grid = &mut g.grid;
                    seg_cells(a, b, |cx, cy| grid.entry((cx, cy)).or_default().push(id));
                }
            } else if let (Some(first), Some(last)) = (w.pts.first(), w.pts.last()) {
                g.pt_map.entry(*first).or_default().push(Occ::Vertex(wi, 0));
                if w.pts.len() > 1 {
                    g.pt_map.entry(*last).or_default().push(Occ::Vertex(wi, w.pts.len() - 1));
                }
            }
        }
        g
    }

    /// Candidate segment ids whose closed extent may contain `p`.
    pub fn near(&self, p: Pt) -> &[u32] {
        self.grid.get(&cell_of(p)).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn cells(&self) -> impl Iterator<Item = &Vec<u32>> {
        self.grid.values()
    }

    /// Node index (pins first, then wires) for an occupant.
    fn node(&self, o: Occ) -> usize {
        match o {
            Occ::Pin(i) => i,
            Occ::Vertex(w, _) => self.pins.len() + w,
        }
    }

    /// All nodes electrically present at `p`: pins, wires with a vertex there, wires whose
    /// segment passes through it.
    fn nodes_at(&self, p: Pt) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(occs) = self.pt_map.get(&p) {
            out.extend(occs.iter().map(|o| self.node(*o)));
        }
        for &si in self.near(p) {
            let s = &self.segs[si as usize];
            if on_segment(s.a, s.b, p) {
                out.push(self.pins.len() + s.wire);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
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
            // Keep the smaller index as root for determinism.
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0[hi] = lo;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Netlist {
    /// The view this netlist describes.
    pub view: View,
    /// Nets ordered by first appearance (pins in part order, then wires).
    pub nets: Vec<Net>,
    /// Per net id: every attached label as `(name, position)` in file order.
    /// More than one distinct name means two labels were joined (see `net-short`).
    /// Always empty in the breadboard view.
    pub labels: Vec<Vec<(String, Pt)>>,
    pin_net: HashMap<(String, String), usize>,
    wire_net: HashMap<String, usize>,
    pub(crate) pin_net_idx: Vec<usize>,
    pub(crate) wire_net_idx: Vec<usize>,
    pub(crate) geom: Geom,
}

impl Netlist {
    pub fn build(circuit: &Circuit, lib: &dyn PartLookup, view: View) -> Netlist {
        let geom = Geom::new(circuit, lib, view);
        let np = geom.pins.len();
        let nw = geom.wire_ids.len();
        let mut dsu = Dsu((0..np + nw).collect());
        let schematic = view == View::Schematic;

        // Everything at the same integer point is joined (schematic: pins and every vertex;
        // breadboard: pins and wire end points).
        for occs in geom.pt_map.values() {
            let first = geom.node(occs[0]);
            for o in &occs[1..] {
                dsu.union(first, geom.node(*o));
            }
        }
        // Part buses: pins of one part listed together are one node.
        for bus in &geom.buses {
            for &m in &bus[1..] {
                dsu.union(bus[0], m);
            }
        }
        let mut label_node: Vec<Option<usize>> = Vec::new();
        if schematic {
            // A wire vertex lying on another wire's segment (T-junction).
            // Crossings in segment interiors are deliberately not joined.
            for (wi, pts) in geom.wire_pts.iter().enumerate() {
                for v in pts {
                    for &si in geom.near(*v) {
                        let s = &geom.segs[si as usize];
                        if s.wire != wi && on_segment(s.a, s.b, *v) {
                            dsu.union(np + wi, np + s.wire);
                        }
                    }
                }
            }
            // Labels attach to everything at their point, and same-name labels merge.
            let mut anchors: HashMap<&str, usize> = HashMap::new();
            for l in &circuit.labels {
                let attached = geom.nodes_at(l.at);
                let Some(&first) = attached.first() else {
                    label_node.push(None);
                    continue;
                };
                for &n in &attached[1..] {
                    dsu.union(first, n);
                }
                match anchors.get(l.name.as_str()) {
                    Some(&a) => dsu.union(a, first),
                    None => {
                        anchors.insert(l.name.as_str(), first);
                    }
                }
                label_node.push(Some(first));
            }
        }

        // Number the nets by first appearance.
        let mut root_net: HashMap<usize, usize> = HashMap::new();
        let mut node_net = Vec::with_capacity(np + nw);
        let mut nets: Vec<Net> = Vec::new();
        for n in 0..np + nw {
            let r = dsu.find(n);
            let id = *root_net.entry(r).or_insert_with(|| {
                nets.push(Net { id: nets.len(), name: None, pins: vec![], wires: vec![] });
                nets.len() - 1
            });
            node_net.push(id);
            if n < np {
                nets[id].pins.push(geom.pins[n].clone());
            } else {
                nets[id].wires.push(geom.wire_ids[n - np].clone());
            }
        }
        let mut labels: Vec<Vec<(String, Pt)>> = vec![Vec::new(); nets.len()];
        for (l, n) in circuit.labels.iter().zip(&label_node) {
            if let Some(n) = n {
                let net = node_net[*n];
                labels[net].push((l.name.clone(), l.at));
                if nets[net].name.is_none() {
                    nets[net].name = Some(l.name.clone());
                }
            }
        }

        let mut pin_net = HashMap::new();
        for (i, p) in geom.pins.iter().enumerate() {
            pin_net.entry((p.part.clone(), p.pin.clone())).or_insert(node_net[i]);
        }
        let mut wire_net = HashMap::new();
        for (i, id) in geom.wire_ids.iter().enumerate() {
            wire_net.entry(id.clone()).or_insert(node_net[np + i]);
        }
        Netlist {
            view,
            nets,
            labels,
            pin_net,
            wire_net,
            pin_net_idx: node_net[..np].to_vec(),
            wire_net_idx: node_net[np..].to_vec(),
            geom,
        }
    }

    pub fn net_of_pin(&self, part: &str, pin: &str) -> Option<usize> {
        self.pin_net.get(&(part.to_string(), pin.to_string())).copied()
    }

    pub fn net_of_wire(&self, wire_id: &str) -> Option<usize> {
        self.wire_net.get(wire_id).copied()
    }

    /// Net at a point: a pin position, a wire vertex (breadboard: a wire end point), or a point
    /// on a wire segment (schematic only).
    pub fn net_at(&self, p: Pt) -> Option<usize> {
        if let Some(o) = self.geom.pt_map.get(&p).and_then(|v| v.first()) {
            return Some(match *o {
                Occ::Pin(i) => self.pin_net_idx[i],
                Occ::Vertex(w, _) => self.wire_net_idx[w],
            });
        }
        self.geom
            .near(p)
            .iter()
            .map(|&si| &self.geom.segs[si as usize])
            .find(|s| on_segment(s.a, s.b, p))
            .map(|s| self.wire_net_idx[s.wire])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_circuit, Library};

    fn build(src: &str) -> Netlist {
        build_view(src, View::Schematic)
    }

    fn build_view(src: &str, view: View) -> Netlist {
        let c = parse_circuit(src).unwrap();
        let lib = Library::load(&c, None);
        Netlist::build(&c, &lib, view)
    }

    const HEAD: &str = r#""format":"short-circ/1""#;

    #[test]
    fn rotated_part_pin_positions() {
        // LED rot 90 at (26,6): anode (0,1)->(25,6), cathode (4,1)->(25,10).
        let n = build(&format!(
            r#"{{{HEAD},"parts":[{{"id":"D1","type":"led","at":[26,6],"rot":90}}]}}"#
        ));
        let pins = &n.geom.pins;
        assert_eq!(pins[0], PinRef { part: "D1".into(), pin: "anode".into(), at: [25, 6] });
        assert_eq!(pins[1].at, [25, 10]);
        // 180 and mirror
        let n = build(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[10,10],"rot":180}},
            {{"id":"R2","type":"resistor","at":[10,20],"mirror":true}}]}}"#
        ));
        assert_eq!(n.geom.pins[0].at, [10, 9]); // a: (0,1) -> (0,-1)
        assert_eq!(n.geom.pins[1].at, [2, 9]); // b: (8,1) -> (-8,-1)
        assert_eq!(n.geom.pins[2].at, [10, 21]); // mirror a: (0,1)
        assert_eq!(n.geom.pins[3].at, [2, 21]); // mirror b: (-8,1)
    }

    #[test]
    fn endpoint_to_pin_connects() {
        let n = build(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}},{{"id":"R2","type":"resistor","at":[12,0]}}],
            "wires":[{{"id":"w1","pts":[[8,1],[12,1]]}}]}}"#
        ));
        assert_eq!(n.net_of_pin("R1", "b"), n.net_of_pin("R2", "a"));
        assert_eq!(n.net_of_pin("R1", "b"), n.net_of_wire("w1"));
        assert_ne!(n.net_of_pin("R1", "a"), n.net_of_pin("R1", "b"));
    }

    #[test]
    fn t_junction_connects() {
        let n = build(&format!(
            r#"{{{HEAD},"wires":[{{"id":"h","pts":[[0,0],[10,0]]}},{{"id":"v","pts":[[5,0],[5,5]]}}]}}"#
        ));
        assert!(n.net_of_wire("h").is_some());
        assert_eq!(n.net_of_wire("h"), n.net_of_wire("v"));
        assert_eq!(n.nets.len(), 1);
    }

    #[test]
    fn interior_vertex_touching_endpoint_connects() {
        let n = build(&format!(
            r#"{{{HEAD},"wires":[{{"id":"a","pts":[[0,0],[5,0],[5,5]]}},{{"id":"b","pts":[[5,0],[9,0]]}}]}}"#
        ));
        assert_eq!(n.nets.len(), 1);
    }

    #[test]
    fn crossing_does_not_connect() {
        let n = build(&format!(
            r#"{{{HEAD},"wires":[{{"id":"h","pts":[[0,5],[10,5]]}},{{"id":"v","pts":[[5,0],[5,10]]}}]}}"#
        ));
        assert_eq!(n.nets.len(), 2);
        assert_ne!(n.net_of_wire("h"), n.net_of_wire("v"));
    }

    #[test]
    fn pin_in_segment_interior_does_not_connect() {
        let n = build(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[4,4]}}],
            "wires":[{{"id":"w","pts":[[0,5],[20,5]]}}]}}"#
        ));
        assert_ne!(n.net_of_pin("R1", "a"), n.net_of_wire("w"));
    }

    #[test]
    fn labels_merge_nets_and_name_them() {
        let n = build(&format!(
            r#"{{{HEAD},"wires":[{{"id":"a","pts":[[0,0],[5,0]]}},{{"id":"b","pts":[[0,10],[5,10]]}},{{"id":"c","pts":[[0,20],[5,20]]}}],
            "labels":[{{"at":[0,0],"name":"VCC"}},{{"at":[3,10],"name":"VCC"}},{{"at":[5,20],"name":"X"}}]}}"#
        ));
        assert_eq!(n.net_of_wire("a"), n.net_of_wire("b"));
        assert_ne!(n.net_of_wire("a"), n.net_of_wire("c"));
        let na = n.net_of_wire("a").unwrap();
        assert_eq!(n.nets[na].name.as_deref(), Some("VCC"));
        assert_eq!(n.labels[na].len(), 2);
        let nc = n.net_of_wire("c").unwrap();
        assert_eq!(n.nets[nc].name.as_deref(), Some("X"));
    }

    #[test]
    fn pins_at_same_point_connect() {
        let n = build(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}},{{"id":"R2","type":"resistor","at":[8,0]}}]}}"#
        ));
        assert_eq!(n.net_of_pin("R1", "b"), n.net_of_pin("R2", "a"));
        assert_eq!(n.nets.len(), 3);
    }

    #[test]
    fn net_at_finds_pin_vertex_and_segment_points() {
        let n = build(&format!(
            r#"{{{HEAD},"parts":[{{"id":"R1","type":"resistor","at":[0,0]}}],
            "wires":[{{"id":"w","pts":[[8,1],[8,9],[20,9]]}}]}}"#
        ));
        let w = n.net_of_wire("w");
        assert!(w.is_some());
        assert_eq!(n.net_at([8, 1]), w); // pin
        assert_eq!(n.net_at([8, 9]), w); // vertex
        assert_eq!(n.net_at([8, 5]), w); // mid segment
        assert_eq!(n.net_at([14, 9]), w); // mid segment
        assert_eq!(n.net_at([14, 10]), None);
        assert_eq!(n.net_of_pin("R1", "a").map(|i| n.nets[i].pins.len()), Some(1));
    }

    #[test]
    fn unknown_types_are_skipped() {
        let n = build(&format!(r#"{{{HEAD},"parts":[{{"id":"X1","type":"nope","at":[0,0]}}]}}"#));
        assert!(n.nets.is_empty());
        assert_eq!(n.net_of_pin("X1", "a"), None);
    }

    #[test]
    fn diagonal_t_junction_and_cells() {
        let n = build(&format!(
            r#"{{{HEAD},"wires":[{{"id":"d","pts":[[0,0],[40,20]]}},{{"id":"e","pts":[[20,10],[20,30]]}}]}}"#
        ));
        assert_eq!(n.nets.len(), 1);
    }

    #[test]
    fn wires_at_the_coordinate_limit_build_and_validate_quickly() {
        let l = crate::COORD_LIMIT;
        let src = format!(
            r#"{{{HEAD},"wires":[{{"id":"h","pts":[[-{l},0],[{l},0]]}},{{"id":"d","pts":[[-{l},-{l}],[{l},{l}]]}},
            {{"id":"v","pts":[[0,-{l}],[0,{l}]]}},{{"id":"e","pts":[[{l},{l}],[{l},-{l}],[-{l},-{l}]]}}]}}"#
        );
        let t = std::time::Instant::now();
        let c = parse_circuit(&src).unwrap();
        let lib = Library::load(&c, None);
        for view in [View::Schematic, View::Breadboard] {
            let n = Netlist::build(&c, &lib, view);
            if view == View::Schematic {
                // h and v end on the sides of e (T-junctions) and d shares e's end points.
                let net = n.net_of_wire("e");
                assert!(net.is_some() && ["h", "v", "d"].iter().all(|w| n.net_of_wire(w) == net));
            }
        }
        let report = crate::validate(&c, &lib);
        assert!(report.issues.iter().any(|i| i.rule == "wire-crossing"), "{:?}", report.issues);
        assert!(t.elapsed().as_secs() < 20, "took {:?}", t.elapsed());
    }

    #[test]
    fn led_example_is_one_loop() {
        let c = parse_circuit(include_str!("../../../examples/led.circ")).unwrap();
        let n = Netlist::build(&c, &Library::load(&c, None), View::Schematic);
        // B1.pos-R1.a, R1.b-D1.anode, D1.cathode-B1.neg: three nets, each two pins.
        assert_eq!(n.nets.len(), 3);
        assert!(n.nets.iter().all(|net| net.pins.len() == 2));
        assert_eq!(n.net_of_pin("B1", "neg"), n.net_of_pin("D1", "cathode"));
        let gnd = n.net_of_pin("B1", "neg").unwrap();
        assert_eq!(n.nets[gnd].name.as_deref(), Some("GND"));
    }

    // ---- views -------------------------------------------------------------------------

    const V2: &str = r#""format":"short-circ/2""#;

    #[test]
    fn parts_only_appear_in_views_they_are_placed_in() {
        let src = format!(
            r#"{{{V2},"lib":[{{"type":"sym","pins":[{{"id":"a"}}],
              "schematic":{{"size":[1,1],"pins":{{"a":{{"at":[0,0]}}}},"shapes":[{{"k":"circle","at":[0,0],"r":0.3}}]}}}}],
            "parts":[{{"id":"R1","type":"sym","schematic":{{"at":[0,0]}}}},
            {{"id":"R2","type":"sym","breadboard":{{"at":[0,0]}}}}]}}"#
        );
        // `sym` has no breadboard geometry, so R2 contributes no pins there.
        let s = build_view(&src, View::Schematic);
        assert_eq!(s.net_of_pin("R1", "a").map(|_| ()), Some(()));
        assert_eq!(s.net_of_pin("R2", "a"), None);
        let b = build_view(&src, View::Breadboard);
        assert!(b.nets.is_empty());
    }

    #[test]
    fn wires_belong_to_their_view() {
        let src = format!(
            r#"{{{V2},"wires":[{{"id":"s","pts":[[0,0],[5,0]]}},{{"id":"b","view":"breadboard","pts":[[0,0],[5,5]]}}]}}"#
        );
        let s = build_view(&src, View::Schematic);
        assert_eq!(s.nets.len(), 1);
        assert_eq!(s.net_of_wire("s"), Some(0));
        assert_eq!(s.net_of_wire("b"), None);
        let b = build_view(&src, View::Breadboard);
        assert_eq!(b.net_of_wire("b"), Some(0));
        assert_eq!(b.net_of_wire("s"), None);
    }

    /// Two-pin breadboard-only parts: a "board" with a two-hole strip and two loose legs.
    const BB_LIB: &str = r#""lib":[
        {"type":"strip","category":"board","pins":[{"id":"h1"},{"id":"h2"},{"id":"h3"}],"buses":[["h1","h2"]],
         "breadboard":{"size":[3,1],"pins":{"h1":{"at":[0,0]},"h2":{"at":[1,0]},"h3":{"at":[2,0]}},
         "shapes":[{"k":"rect","at":[0,0],"size":[3,1]}]}},
        {"type":"leg","pins":[{"id":"a"}],"breadboard":{"size":[1,1],"pins":{"a":{"at":[0,0]}},
         "shapes":[{"k":"circle","at":[0,0],"r":0.3}]}}]"#;

    #[test]
    fn breadboard_wires_join_only_at_end_points() {
        let n = build_view(
            &format!(
                r#"{{{V2},{BB_LIB},"parts":[
                {{"id":"B1","type":"strip","breadboard":{{"at":[0,0]}}}},
                {{"id":"L1","type":"leg","breadboard":{{"at":[1,0]}}}},
                {{"id":"L2","type":"leg","breadboard":{{"at":[2,0]}}}},
                {{"id":"L3","type":"leg","breadboard":{{"at":[5,5]}}}}],
                "wires":[
                  {{"id":"j1","view":"breadboard","pts":[[0,0],[0,3],[5,5]]}},
                  {{"id":"j2","view":"breadboard","pts":[[3,-3],[3,3]]}},
                  {{"id":"j3","view":"breadboard","pts":[[-3,0],[3,0]]}}]}}"#
            ),
            View::Breadboard,
        );
        // Strip bus h1-h2 joins L1 (at h2) with j1's first end (h1); j1 reaches L3.
        assert_eq!(n.net_of_pin("L1", "a"), n.net_of_pin("B1", "h1"));
        assert_eq!(n.net_of_pin("L1", "a"), n.net_of_pin("L3", "a"));
        assert_eq!(n.net_of_wire("j1"), n.net_of_pin("L3", "a"));
        // L2 sits on h3, which is not in the strip.
        assert_ne!(n.net_of_pin("L2", "a"), n.net_of_pin("L1", "a"));
        assert_eq!(n.net_of_pin("L2", "a"), n.net_of_pin("B1", "h3"));
        // j3 passes over (0,0) and (1,0) with its interior and ends at (3,0), and j2 crosses it at
        // (3,0) with its interior: neither joins anything (no T-junctions).
        assert_ne!(n.net_of_wire("j3"), n.net_of_pin("B1", "h1"));
        assert_ne!(n.net_of_wire("j2"), n.net_of_wire("j3"));
    }

    #[test]
    fn breadboard_interior_vertices_connect_nothing_and_labels_are_ignored() {
        let n = build_view(
            &format!(
                r#"{{{V2},{BB_LIB},"parts":[
                {{"id":"L1","type":"leg","breadboard":{{"at":[1,0]}}}},
                {{"id":"L2","type":"leg","breadboard":{{"at":[2,0]}}}}],
                "wires":[{{"id":"j1","view":"breadboard","pts":[[0,0],[1,0],[2,0],[2,5]]}}],
                "labels":[{{"at":[1,0],"name":"A"}},{{"at":[2,0],"name":"A"}}]}}"#
            ),
            View::Breadboard,
        );
        assert_ne!(n.net_of_pin("L1", "a"), n.net_of_pin("L2", "a"), "vertices at the legs do not connect");
        assert_ne!(n.net_of_wire("j1"), n.net_of_pin("L1", "a"));
        assert!(n.nets.iter().all(|net| net.name.is_none()), "labels are not used in the breadboard view");
        assert!(n.labels.iter().all(|l| l.is_empty()));
    }

    #[test]
    fn breadboard_pins_at_one_point_are_one_node() {
        let n = build_view(
            &format!(
                r#"{{{V2},{BB_LIB},"parts":[
                {{"id":"L1","type":"leg","breadboard":{{"at":[1,0]}}}},
                {{"id":"L2","type":"leg","breadboard":{{"at":[1,0]}}}}]}}"#
            ),
            View::Breadboard,
        );
        assert_eq!(n.net_of_pin("L1", "a"), n.net_of_pin("L2", "a"));
        assert_eq!(n.nets.len(), 1);
    }

    #[test]
    fn schematic_buses_join_pins_of_one_part() {
        let n = build(&format!(
            r#"{{{V2},"lib":[{{"type":"link","pins":[{{"id":"a"}},{{"id":"b"}}],"buses":[["a","b"]],
              "schematic":{{"size":[4,1],"pins":{{"a":{{"at":[0,0]}},"b":{{"at":[4,0]}}}},"shapes":[{{"k":"line","pts":[[0,0],[4,0]]}}]}}}}],
            "parts":[{{"id":"K1","type":"link","schematic":{{"at":[0,0]}}}}]}}"#
        ));
        assert_eq!(n.net_of_pin("K1", "a"), n.net_of_pin("K1", "b"));
        assert_eq!(n.nets.len(), 1);
    }
}
