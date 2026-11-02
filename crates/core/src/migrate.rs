//! Reading of `.circ` / `.part` text, including upgrade of `short-circ/1` files in memory
//! (docs/FORMAT.md "Compatibility"). This is the only place that knows about v1.
//!
//! - v1 circuit: `parts[].at/rot/mirror` become `schematic`, every wire gets `view: schematic`,
//!   labels are kept, `format` becomes `short-circ/2`.
//! - v1 part: `size`, `pins[].at/dir` and `shapes` move into `schematic`, pin `id`/`name` stay
//!   in the logical `pins`. The v1 `svg` path-data field is dropped (it was never rendered).
//! - Inline `lib[]` entries of a circuit are upgraded the same way (each by its own `format`,
//!   falling back to the circuit's).
//!
//! A file without a `format` field is taken as v1 when it has the v1 shape (a part with a
//! top-level `at`, a definition with a top-level `size`), else as v2.

use crate::model::{
    Circuit, Dir, Label, Part, PartDef, PinDef, PinPos, Placement, Pt, Shape, Theme, ViewDef, Wire, FORMAT, FORMAT_V1,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err<E: std::fmt::Display>(e: E) -> ParseError {
    ParseError(e.to_string())
}

/// Which generation a file belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gen {
    V1,
    V2,
}

fn unsupported(found: &str) -> ParseError {
    ParseError(format!("unsupported format \"{found}\" (expected \"{FORMAT}\" or \"{FORMAT_V1}\")"))
}

/// Reads the `format` field. `Ok(None)` when absent.
fn format_of(v: &Value) -> Result<Option<Gen>, ParseError> {
    match v.get("format") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s == FORMAT => Ok(Some(Gen::V2)),
        Some(Value::String(s)) if s == FORMAT_V1 => Ok(Some(Gen::V1)),
        Some(Value::String(s)) => Err(unsupported(s)),
        Some(other) => Err(ParseError(format!("\"format\" must be a string, found {other}"))),
    }
}

fn looks_like_v1_circuit(v: &Value) -> bool {
    let has_at = |parts: Option<&Value>| {
        parts.and_then(Value::as_array).is_some_and(|a| a.iter().any(|p| p.get("at").is_some()))
    };
    has_at(v.get("parts"))
}

fn looks_like_v1_part(v: &Value) -> bool {
    v.get("size").is_some()
}

pub fn parse_circuit(src: &str) -> Result<Circuit, ParseError> {
    let mut v: Value = serde_json::from_str(src).map_err(err)?;
    if !v.is_object() {
        return Err(ParseError("a circuit file must be a JSON object".into()));
    }
    let generation = match format_of(&v)? {
        Some(g) => g,
        None if looks_like_v1_circuit(&v) => Gen::V1,
        None => Gen::V2,
    };
    // `lib` is converted entry by entry, so take it out first.
    let lib_values = match v.as_object_mut().and_then(|o| o.remove("lib")) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(ParseError("\"lib\" must be an array".into())),
    };
    let mut circuit = match generation {
        Gen::V2 => serde_json::from_value::<Circuit>(v).map_err(err)?,
        Gen::V1 => serde_json::from_value::<V1Circuit>(v).map_err(err)?.upgrade(),
    };
    circuit.format = FORMAT.to_string();
    circuit.lib = lib_values
        .into_iter()
        .map(|d| part_from_value(d, Some(generation)))
        .collect::<Result<_, _>>()?;
    Ok(circuit)
}

pub fn parse_part(src: &str) -> Result<PartDef, ParseError> {
    let v: Value = serde_json::from_str(src).map_err(err)?;
    part_from_value(v, None)
}

/// `default` is the generation to assume when the entry has no `format` of its own and is not
/// recognisable by shape (inline `lib` entries inherit the circuit's).
fn part_from_value(v: Value, default: Option<Gen>) -> Result<PartDef, ParseError> {
    if !v.is_object() {
        return Err(ParseError("a part definition must be a JSON object".into()));
    }
    let generation = match format_of(&v)? {
        Some(g) => g,
        None if looks_like_v1_part(&v) => Gen::V1,
        None => default.unwrap_or(Gen::V2),
    };
    let mut def = match generation {
        Gen::V2 => serde_json::from_value::<PartDef>(v).map_err(err)?,
        Gen::V1 => serde_json::from_value::<V1PartDef>(v).map_err(err)?.upgrade(),
    };
    def.format = FORMAT.to_string();
    Ok(def)
}

// ---------------------------------------------------------------------------------------
// The v1 data model, private to this module.

#[derive(Deserialize)]
struct V1Circuit {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    parts: Vec<V1Part>,
    #[serde(default)]
    wires: Vec<V1Wire>,
    #[serde(default)]
    labels: Vec<Label>,
    #[serde(default)]
    theme: Option<Theme>,
}

#[derive(Deserialize)]
struct V1Part {
    id: String,
    #[serde(rename = "type")]
    ty: String,
    at: Pt,
    #[serde(default)]
    rot: i32,
    #[serde(default)]
    mirror: bool,
    #[serde(default)]
    props: BTreeMap<String, String>,
    #[serde(default)]
    hidden_label: bool,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    fill: Option<String>,
    #[serde(default)]
    label_color: Option<String>,
}

#[derive(Deserialize)]
struct V1Wire {
    id: String,
    pts: Vec<Pt>,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Deserialize)]
struct V1PartDef {
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: Option<String>,
    size: [f64; 2],
    #[serde(default)]
    pins: Vec<V1Pin>,
    #[serde(default)]
    shapes: Vec<Shape>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    meta: Option<BTreeMap<String, String>>,
    // The v1 `svg` path-data field is not read: it is dropped on upgrade.
}

#[derive(Deserialize)]
struct V1Pin {
    id: String,
    at: Pt,
    #[serde(default)]
    dir: Dir,
    #[serde(default)]
    name: Option<String>,
}

impl V1Circuit {
    fn upgrade(self) -> Circuit {
        Circuit {
            format: FORMAT.to_string(),
            title: self.title,
            parts: self
                .parts
                .into_iter()
                .map(|p| Part {
                    id: p.id,
                    ty: p.ty,
                    props: p.props,
                    hidden_label: p.hidden_label,
                    color: p.color,
                    fill: p.fill,
                    label_color: p.label_color,
                    schematic: Some(Placement { at: p.at, rot: p.rot, mirror: p.mirror }),
                    breadboard: None,
                })
                .collect(),
            wires: self
                .wires
                .into_iter()
                .map(|w| Wire { id: w.id, view: Default::default(), pts: w.pts, color: w.color })
                .collect(),
            labels: self.labels,
            nets: Vec::new(),
            lib: Vec::new(),
            theme: self.theme,
        }
    }
}

impl V1PartDef {
    fn upgrade(self) -> PartDef {
        let mut pins = Vec::with_capacity(self.pins.len());
        let mut positions = BTreeMap::new();
        for p in self.pins {
            // A duplicated id keeps its first definition (ids are unique in v2).
            if positions.contains_key(&p.id) {
                continue;
            }
            positions.insert(p.id.clone(), PinPos { at: p.at, dir: p.dir });
            pins.push(PinDef { id: p.id, name: p.name });
        }
        PartDef {
            format: FORMAT.to_string(),
            ty: self.ty,
            name: self.name,
            category: self.category,
            pins,
            buses: Vec::new(),
            schematic: Some(ViewDef { size: self.size, pins: positions, shapes: self.shapes, svg: None }),
            breadboard: None,
            params: self.params,
            meta: self.meta,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::View;

    #[test]
    fn v1_circuit_is_upgraded() {
        let c = parse_circuit(
            r#"{"format":"short-circ/1","title":"t","parts":[
                {"id":"R1","type":"resistor","at":[3,4],"rot":90,"mirror":true,"props":{"value":"1k"},"color":"red"}],
               "wires":[{"id":"w1","pts":[[0,0],[5,0]],"color":"blue"}],
               "labels":[{"at":[0,0],"name":"N"}],"theme":{"wire":"teal"}}"#,
        )
        .unwrap();
        assert_eq!(c.format, FORMAT);
        let p = &c.parts[0];
        assert_eq!(p.schematic, Some(Placement { at: [3, 4], rot: 90, mirror: true }));
        assert_eq!(p.breadboard, None);
        assert_eq!(p.color.as_deref(), Some("red"));
        assert_eq!(p.props["value"], "1k");
        assert_eq!(c.wires[0].view, View::Schematic);
        assert_eq!(c.wires[0].color.as_deref(), Some("blue"));
        assert_eq!(c.labels[0].name, "N");
        assert_eq!(c.theme.unwrap().wire.as_deref(), Some("teal"));
    }

    #[test]
    fn v1_part_is_upgraded_and_svg_dropped() {
        let d = parse_part(
            r#"{"format":"short-circ/1","type":"x","name":"X","category":"c","size":[4,2],
            "pins":[{"id":"a","at":[0,1],"dir":"left","name":"A"},{"id":"b","at":[4,1],"dir":"right"}],
            "shapes":[{"k":"line","pts":[[0,1],[4,1]]}],"svg":"M0 0 L1 1","params":{"gen":"x"},"meta":{"k":"v"}}"#,
        )
        .unwrap();
        assert_eq!(d.format, FORMAT);
        assert_eq!(d.pins.len(), 2);
        assert_eq!(d.pins[0].name.as_deref(), Some("A"));
        let s = d.schematic.as_ref().unwrap();
        assert_eq!(s.size, [4.0, 2.0]);
        assert_eq!(s.pins["a"], PinPos { at: [0, 1], dir: Dir::Left });
        assert_eq!(s.pins["b"], PinPos { at: [4, 1], dir: Dir::Right });
        assert_eq!(s.shapes.len(), 1);
        assert!(s.svg.is_none() && d.breadboard.is_none());
        assert_eq!(d.meta.unwrap()["k"], "v");
        assert!(d.params.is_some());
    }

    #[test]
    fn v2_files_load_unchanged() {
        let c = parse_circuit(
            r#"{"format":"short-circ/2","parts":[{"id":"R1","type":"r","schematic":{"at":[1,2]},"breadboard":{"at":[3,4],"rot":270}}],
            "wires":[{"id":"j","view":"breadboard","pts":[[0,0],[1,1]]}],"nets":[{"name":"N","pins":["R1.a"]}]}"#,
        )
        .unwrap();
        assert_eq!(c.parts[0].breadboard.as_ref().unwrap().rot, 270);
        assert_eq!(c.wires[0].view, View::Breadboard);
        assert_eq!(c.nets.len(), 1);
        let d = parse_part(
            r#"{"format":"short-circ/2","type":"t","pins":[{"id":"a"}],"buses":[["a"]],
            "breadboard":{"size":[1,1],"pins":{"a":{"at":[0,0]}}}}"#,
        )
        .unwrap();
        assert!(d.schematic.is_none());
        assert_eq!(d.breadboard.unwrap().pins["a"].at, [0, 0]);
    }

    #[test]
    fn lib_entries_follow_their_own_format() {
        let c = parse_circuit(
            r#"{"format":"short-circ/2","lib":[
                {"format":"short-circ/1","type":"old","size":[2,2],"pins":[{"id":"a","at":[0,1]}]},
                {"type":"new","pins":[{"id":"a"}],"schematic":{"size":[1,1],"pins":{"a":{"at":[0,0]}}}}]}"#,
        )
        .unwrap();
        assert_eq!(c.lib[0].schematic.as_ref().unwrap().pins["a"].at, [0, 1]);
        assert_eq!(c.lib[1].schematic.as_ref().unwrap().size, [1.0, 1.0]);
        // A v1 circuit's lib entries without their own format are v1.
        let c = parse_circuit(r#"{"format":"short-circ/1","lib":[{"type":"x","size":[3,1]}]}"#).unwrap();
        assert_eq!(c.lib[0].schematic.as_ref().unwrap().size, [3.0, 1.0]);
    }

    #[test]
    fn missing_format_is_detected_by_shape() {
        let c = parse_circuit(r#"{"parts":[{"id":"R1","type":"resistor","at":[2,2]}]}"#).unwrap();
        assert_eq!(c.parts[0].schematic.as_ref().unwrap().at, [2, 2]);
        let c = parse_circuit(r#"{"parts":[{"id":"R1","type":"resistor","schematic":{"at":[5,5]}}]}"#).unwrap();
        assert_eq!(c.parts[0].schematic.as_ref().unwrap().at, [5, 5]);
        assert!(parse_part(r#"{"type":"t","size":[1,1]}"#).unwrap().schematic.is_some());
    }

    #[test]
    fn unknown_formats_and_bad_json_error_clearly() {
        let e = parse_circuit(r#"{"format":"short-circ/3"}"#).unwrap_err();
        assert!(e.0.contains("short-circ/3") && e.0.contains("short-circ/2") && e.0.contains("short-circ/1"), "{e}");
        let e = parse_part(r#"{"format":"other/9","type":"t"}"#).unwrap_err();
        assert!(e.0.contains("other/9"), "{e}");
        assert!(parse_circuit("not json").is_err());
        assert!(parse_circuit("[]").is_err());
        assert!(parse_circuit(r#"{"format":2}"#).is_err());
        // v1 requires its own fields.
        assert!(parse_circuit(r#"{"format":"short-circ/1","parts":[{"id":"R1","type":"r"}]}"#).is_err());
        // Pin positions must be integers.
        assert!(parse_part(
            r#"{"format":"short-circ/2","type":"t","pins":[{"id":"a"}],"schematic":{"size":[1,1],"pins":{"a":{"at":[0.5,0]}}}}"#
        )
        .is_err());
    }

    #[test]
    fn duplicate_v1_pin_ids_keep_the_first() {
        let d = parse_part(
            r#"{"format":"short-circ/1","type":"t","size":[2,2],"pins":[{"id":"a","at":[0,0]},{"id":"a","at":[2,2]}]}"#,
        )
        .unwrap();
        assert_eq!(d.pins.len(), 1);
        assert_eq!(d.schematic.unwrap().pins["a"].at, [0, 0]);
    }
}
