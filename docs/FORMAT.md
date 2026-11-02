# short-circ file format, version `short-circ/1`

Two JSON text formats. UTF-8, no BOM. Unknown fields are ignored on read (forward compatible).
This document is the contract between the core crate, the app and any external tool.

## Coordinates and units
- All circuit coordinates are **integers in grid units**. 1 grid unit = 0.1 inch (2.54 mm).
- +x is right, +y is **down** (screen convention).
- Integer coordinates make connectivity and overlap checks exact (no epsilon).
- **Part transform** (normative): a local point `(x,y)` becomes world `at + R(mirror(x,y))`. `mirror` negates x first (`(x,y)->(-x,y)`). Rotation `rot` is clockwise as seen on screen, applied about the part origin: 90 maps `(x,y)->(-y,x)`, 180 `(-x,-y)`, 270 `(y,-x)`. The rotated bbox is the transformed `[0,0]-[w,h]` box (min/max of its corners).
- Part-local shape coordinates may be floats (grid units, origin = part origin). **Pin positions must be integers.**

## Circuit file: `*.circ`
```json
{
  "format": "short-circ/1",
  "title": "LED with resistor",
  "parts": [
    {"id":"B1","type":"battery","at":[10,10],"rot":0,"props":{"value":"9V"}},
    {"id":"R1","type":"resistor","at":[20,10],"rot":0,"props":{"value":"330"}}
  ],
  "wires": [
    {"id":"w1","pts":[[12,10],[20,10]]}
  ],
  "labels": [{"at":[12,10],"name":"VCC"}],
  "lib": []
}
```
Fields:
- `format` (required): must be `"short-circ/1"`.
- `title` (optional string).
- `parts[]`: `id` (unique string across parts and wires, used as reference designator, e.g. `R1`), `type` (component type name, see resolution), `at` `[x,y]` integer part origin, `rot` (0, 90, 180, 270 clockwise, default 0), `mirror` (bool, flips x before rotation, default false), `props` (free string map; `value`, `label`, anything). Optional `hidden_label` bool.
- `wires[]`: `id` (unique), `pts` list of >= 2 integer points forming a polyline. Segments may be any direction but orthogonal is strongly preferred. Optional `color` string (`"#rrggbb"` or name).
- `labels[]`: names a net. `at` must coincide with a pin position or lie on a wire. All points sharing a label name are the same net (like KiCad net labels).
- `lib[]`: optional inline component definitions (same schema as `.part`) that take precedence over external ones.

## Component file: `*.part`
```json
{
  "format": "short-circ/1",
  "type": "resistor",
  "name": "Resistor",
  "category": "passive",
  "size": [8, 2],
  "pins": [
    {"id":"a","at":[0,1],"dir":"left"},
    {"id":"b","at":[8,1],"dir":"right"}
  ],
  "shapes": [
    {"k":"line","pts":[[0,1],[2,1]]},
    {"k":"rect","at":[2,0],"size":[4,2]},
    {"k":"line","pts":[[6,1],[8,1]]},
    {"k":"text","at":[4,-0.8],"text":"{ref}","size":0.9,"anchor":"middle"}
  ]
}
```
- `type`: unique lowercase name referenced by parts. `name`: display name.
- `size`: `[w,h]` bounding box in grid units from the part origin (0,0) to (w,h). Used for overlap checks. Local coordinates may extend slightly outside; bbox is authoritative.
- `pins[]`: `id` (unique within part), `at` integer `[x,y]` local, `dir` one of `left|right|up|down` (direction the pin leads out of the body; used for label placement), optional `name`.
- `shapes[]`: drawn in order. Common optional fields: `stroke` (color, default theme foreground), `fill` (color or `"none"`, default none), `width` (stroke width in grid units, default 0.12).
  - `{"k":"line","pts":[[x,y],...]}` polyline
  - `{"k":"rect","at":[x,y],"size":[w,h],"r":0}` optional corner radius
  - `{"k":"circle","at":[x,y],"r":1}`
  - `{"k":"arc","at":[x,y],"r":1,"from":0,"to":180}` degrees, clockwise from +x, y-down
  - `{"k":"poly","pts":[...]}` closed polygon, fill allowed
  - `{"k":"text","at":[x,y],"text":"...","size":1,"anchor":"start|middle|end"}`; `{ref}` expands to the part id, `{value}` to `props.value`, `{prop:name}` to any prop.
- Optional `params`: for generated families, `{"gen":"dip","pins":8}` is reserved for built-in generators (`dip`, `header`). Third-party parts should just list pins and shapes.
- Optional `svg`: raw SVG path data string (`d` attribute) imported from elsewhere; converted to `poly`/`line` shapes at load. Ignored if `shapes` present.

## Component resolution order for a part `type`
1. `lib[]` inside the circuit file
2. `*.part` files in a `parts/` folder next to the `.circ` file
3. `*.part` files in a `parts/` folder next to the executable
4. Built-ins embedded in the executable

## Connectivity rules (normative; the netlist builder implements exactly this)
A **point** is connected to everything else at the same integer coordinate. Two things are electrically joined when:
1. A wire **endpoint** (first or last pt) equals a pin position, a wire pt (any vertex or endpoint) of another wire, or lies on the interior of another wire segment (T-junction).
2. A wire **interior vertex** equals another wire's endpoint or a pin position (the polyline touches there).
3. Two wires crossing at a point that is not an endpoint or vertex of either do **not** connect (they merely cross).
4. Pins of different parts at the same coordinate connect directly.
5. Labels with the same name merge their nets.
A net with zero pins is allowed but reported as a warning.

## Validation rules (ids used in reports)
Severity `error` unless noted.
| rule | meaning |
|---|---|
| `schema` | JSON invalid, wrong `format`, missing required field |
| `duplicate-id` | id used twice among parts and wires |
| `unknown-type` | part type not resolvable |
| `bad-rotation` | rot not in {0,90,180,270} |
| `off-grid` | non-integer coordinate (should not occur after parse; reserved) |
| `wire-overlap` | two wires share a collinear stretch of positive length |
| `wire-crossing` (warning) | two wires cross at a point where neither has an endpoint/vertex, implying a missing junction |
| `wire-through-pin` | a wire passes through a pin position at its interior without a vertex there |
| `wire-diagonal` (warning) | non-orthogonal segment |
| `wire-degenerate` | zero-length segment or fewer than 2 distinct points |
| `dangling-wire` (warning) | an endpoint touches nothing |
| `unconnected-pin` (warning) | pin on no wire and no other pin |
| `part-overlap` | bounding boxes of two parts intersect with positive area |
| `net-short` | two pins on the same net belong to a part pair that declares `props.nets` distinct, or two labels with different names join the same net |
| `empty-net` (warning) | net with no pins |

## Report format (CLI `--validate --json`)
```json
{"ok":false,"errors":1,"warnings":2,"issues":[
 {"rule":"wire-overlap","severity":"error","ids":["w1","w2"],"at":[14,10],"message":"w1 and w2 overlap between (14,10) and (16,10)"}
]}
```
Exit code: 0 no errors (warnings allowed), 1 errors, 2 unreadable input.

## CLI contract (`short-circ.exe`)
- `short-circ.exe [file.circ]` open GUI
- `short-circ.exe --validate file.circ [--json]`
- `short-circ.exe --render file.circ out.svg [--dark]`
- `short-circ.exe --import-svg in.svg out.part --type NAME [--pins "a:0,1;b:8,1"]`
- `short-circ.exe --list-parts` prints resolvable component types
The GUI never requires any of these tools, and nothing in this repo outside `.claude/skills/` refers to Claude.

## Clarifications from the reference implementation
- Any wire vertex or endpoint lying on another wire's segment connects (T-junction), including interior vertices.
- A pin lying in a wire's interior without a vertex there is NOT connected; it raises `wire-through-pin`.
- A label that touches nothing is ignored. `net-short` currently covers only two different label names on one net; its `ids` are the label names.
- `wire-overlap`/`wire-crossing` report one issue per wire pair; `dangling-wire` one per dangling endpoint; `unconnected-pin` has `ids=[part]` with the pin id in the message.
- A wire endpoint also counts as touching if a label name appears at least twice at that point set (closed loops are not dangling).
- Text positions are SVG baselines and text stays upright when parts rotate. `{ref}` is empty when `hidden_label` is set.
- `stroke:"none"` in a shape means no outline. A missing `fill` means no fill.
