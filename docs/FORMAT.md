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
- `parts[]`: `id` (unique string across parts and wires, used as reference designator, e.g. `R1`), `type` (component type name, see resolution), `at` `[x,y]` integer part origin, `rot` (0, 90, 180, 270 clockwise, default 0), `mirror` (bool, flips x before rotation, default false), `props` (free string map; `value`, `label`, anything). Optional `hidden_label` bool. Optional colour specs (see Colours): `color` (outline of every shape without its own `stroke`), `fill` (fill of closed shapes without their own `fill`), `label_color` (the part's text).
- `wires[]`: `id` (unique), `pts` list of >= 2 integer points forming a polyline. Segments may be any direction but orthogonal is strongly preferred. Optional `color` (colour spec, see Colours).
- `labels[]`: names a net. `at` must coincide with a pin position or lie on a wire. All points sharing a label name are the same net (like KiCad net labels). Optional `color` (colour spec).
- `theme` (optional object): circuit-wide colour defaults per kind of object: `background`, `grid`, `wire`, `part`, `label`, `pin`, `junction`, each an optional colour spec (see Colours).
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
- `shapes[]`: drawn in order. Common optional fields: `stroke` (colour spec; default: the part's `color`, else the theme part colour), `fill` (colour spec or `"none"`; default: the part's `fill`, else none; only closed shapes `rect`, `circle`, `poly` are filled), `width` (stroke width in grid units, default 0.12). For `text` shapes the colour is `fill`, else `stroke`, else the part's `label_color`, else the theme part colour (a `"none"` in a text shape is ignored).
  - `{"k":"line","pts":[[x,y],...]}` polyline
  - `{"k":"rect","at":[x,y],"size":[w,h],"r":0}` optional corner radius
  - `{"k":"circle","at":[x,y],"r":1}`
  - `{"k":"arc","at":[x,y],"r":1,"from":0,"to":180}` degrees, clockwise from +x, y-down
  - `{"k":"poly","pts":[...]}` closed polygon, fill allowed
  - `{"k":"text","at":[x,y],"text":"...","size":1,"anchor":"start|middle|end"}`; `{ref}` expands to the part id, `{value}` to `props.value`, `{prop:name}` to any prop.
- Optional `params`: for generated families, `{"gen":"dip","pins":8}` is reserved for built-in generators (`dip`, `header`). Third-party parts should just list pins and shapes.
- Optional `svg`: raw SVG path data string (`d` attribute) imported from elsewhere; converted to `poly`/`line` shapes at load. Ignored if `shapes` present.
- Optional `meta`: string-to-string map of free-form metadata, never interpreted by the viewer. Imported Fritzing parts carry `source` (`"fritzing"`), `title`, `module_id` (the fzp `moduleId`), `label`, `description`, `tags` (comma separated) and every fzp `<property>` by lowercase name (`family`, `package`, `pins`, `resistance`, ...). Omitted when empty.
- Shape colours (`stroke`, `fill`) imported from SVG are lowercase `#rrggbb` strings. A black `stroke` is dropped (theme foreground); a black `fill` stays `#000000`.

### Importing Fritzing parts
`short-circ.exe --import-fritzing` converts a Fritzing part (`.fzpz` bundle, or `.fzp` plus its SVG files) into a `.part`:
- The chosen view's SVG (`--view schematic`, the default, or `breadboard`) becomes the shapes. Its real size is taken from the root `width`/`height`/`viewBox` (units `in`, `mm`, `cm`, `pt`, `pc`, `px`; unit-less lengths are 72 dpi for Illustrator exports, 96 dpi for Inkscape 0.92+, otherwise 90 dpi) and scaled to the grid, 1 unit = 0.1 in. CSS `<style>` classes, `<use>`, group styles and nested transforms are honoured; SVG defaults apply (fill black, stroke none unless given).
- One pin per `<connector>` that has a `p` in the chosen view. Pin `id` is the trimmed connector `name`, or the connector `id` when the name is empty or already taken (the original name is kept in the pin's `name`). The position is the centre of the `terminalId` element; without a terminal, the outer end of the `svgId` element if it is a long thin line, else its centre. Positions are rounded to integers (the drawing is shifted by the mean rounding error first when pins are off-grid by 0.08 unit or more), pins that coincide are nudged with a warning, and the whole part is moved so its bounding box starts at `(0,0)`. `dir` points to the nearest edge of `size`.
- A `{ref}` text is added above the body. Type name: `--type`, else a lowercase letters/digits/underscore slug of the title (then of the moduleId).
- `.fzpz` and `.fzp` files found in a `parts/` folder are imported on the fly with the defaults above (a `.part` or an earlier source with the same type wins; files that fail to import are ignored).
- Not handled: PCB view, breadboard legs and multi-layer artwork, buses, gradients and patterns (flat fallback), filters, clip paths, masks, raster images, text fonts.

## Component resolution order for a part `type`
1. `lib[]` inside the circuit file
2. `*.part` files (and Fritzing `*.fzpz` / `*.fzp`, see below) in a `parts/` folder next to the `.circ` file
3. the same in a `parts/` folder next to the executable
4. Built-ins embedded in the executable

## Connectivity rules (normative; the netlist builder implements exactly this)
A **point** is connected to everything else at the same integer coordinate. Two things are electrically joined when:
1. A wire **endpoint** (first or last pt) equals a pin position, a wire pt (any vertex or endpoint) of another wire, or lies on the interior of another wire segment (T-junction).
2. A wire **interior vertex** equals another wire's endpoint or a pin position (the polyline touches there).
3. Two wires crossing at a point that is not an endpoint or vertex of either do **not** connect (they merely cross).
4. Pins of different parts at the same coordinate connect directly.
5. Labels with the same name merge their nets.
A net with zero pins is allowed but reported as a warning.

## Colours
A **colour spec** is a string, matched case-insensitively with surrounding whitespace ignored:
- `#rgb`, `#rrggbb` or `#rrggbbaa` hex (`#rgb` doubles each digit; `aa` is opacity, `ff` = opaque), or
- a preset name from the table below (`grey` is accepted as an alias of `gray`), or
- `"none"`: draw nothing (no outline, no fill, no wire, ...), or
- `"default"`: use the default for that kind of object (the next precedence level below).
The empty string and anything else is not a colour (`bad-color` warning) and is treated like `"default"`.

| name | hex | name | hex | name | hex |
|---|---|---|---|---|---|
| `black` | `#000000` | `white` | `#ffffff` | `gray` | `#808080` |
| `silver` | `#c0c0c0` | `red` | `#d93025` | `orange` | `#f57c00` |
| `yellow` | `#f2c200` | `green` | `#1e8e3e` | `lime` | `#84cc16` |
| `cyan` | `#00acc1` | `blue` | `#1a73e8` | `navy` | `#1a237e` |
| `purple` | `#8e24aa` | `magenta` | `#d81b9a` | `pink` | `#f48fb1` |
| `brown` | `#8d5a2b` | `teal` | `#00897b` | `gold` | `#d4a017` |

`short-circ.exe --list-colors` prints this table.

**Precedence**, highest first. The first level that gives a colour (or `"none"`) wins:
1. The object's own colour: `wire.color`, `label.color`.
2. For shapes inside a part: the shape's own `stroke` / `fill` (text: `fill`, then `stroke`), then the part's `color` / `fill` / `label_color`. So a shape's own colour beats the part-level colour.
3. `theme.<kind>` of the circuit, where kind is `wire`, `label`, `pin`, `junction`, `part` (outline and text of part shapes), `background`, `grid`.
4. The built-in default of the active light or dark theme.
Closed shapes have no fill unless level 1 or 2 gives one (there is no theme fill).

Explicit colours (levels 1 to 3) are used exactly as written in both the viewer and SVG export; they are never inverted or adjusted for dark mode, so `black` on a dark background stays black. Only the built-in defaults follow the light/dark theme. Junction dots use `theme.junction` if set, otherwise the resolved colour of the first wire (file order) that touches the dot. A `background` of `none` leaves the SVG transparent (the viewer keeps its own background); a `grid` of `none` hides the viewer grid. Translucent colours (`#rrggbbaa`) are written to SVG as `#rrggbb` plus `fill-opacity` / `stroke-opacity`. SVG export and the viewer share one resolver, so they agree.

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
| `bad-color` (warning) | a colour spec (part `color`/`fill`/`label_color`, wire/label `color`, `theme.*`, a `lib` shape `stroke`/`fill`) is not a colour, `"none"` or `"default"`; `ids=[object id]` (`theme` for the theme, the type name for a `lib` shape), the message names the field and the value |

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
- `short-circ.exe --netlist file.circ [--json]` prints every net as `net ID  NAME-or--  pins: R1.a, B1.pos  wires: w1, w2` (pins sorted naturally, `-` for an empty list). With `--json`: `{"nets":[{"id":0,"name":"GND","pins":["R1.b"],"wires":["w1"]}]}`; `name` is `null` for an unlabelled net. Exit 0 ok, 2 unreadable. Use it to check connectivity as the netlist builder sees it.
- `short-circ.exe --render-part TYPE out.svg [--dark] [--circuit-dir DIR]` draws one part with every pin id (`id` or `id:name`) labelled at its pin position, plus its declared bounding box and a title line `type - name (category), WxH, N pin(s)`. `TYPE` resolves like a circuit would (`DIR/parts`, the exe's `parts/`, built-ins, generated `dipN`/`headerN`). Exit 0 written, 1 unknown type or write error. Use it to check part geometry and pin placement.
- `short-circ.exe --import-svg in.svg out.part --type NAME [--pins "a:0,1;b:8,1"] [--scale PX_PER_UNIT]`
- `short-circ.exe --import-fritzing IN OUT [--view schematic|breadboard] [--type NAME]` imports Fritzing parts. `IN` is a `.fzpz`, a `.fzp`, or a folder (every `.fzpz`/`.fzp` directly inside, not recursive; a `.fzp` next to a `.fzpz` of the same name is skipped). `OUT` is a `.part` file, or a folder (created if missing) that receives `<type>.part`; a folder `IN` needs a folder `OUT` and cannot take `--type`. One summary line per part on stdout (`type: size WxH, N pin(s) -> path`), warnings on stderr (`warning: file: ...`). Exit 0 all imported, 1 an import or write error (other files of a folder are still written), 2 bad arguments.
- `short-circ.exe --list-parts` prints resolvable component types
- `short-circ.exe --list-colors` prints the preset colour names with their hex codes
- In the GUI, File > Import Fritzing part... does the same for one file and writes into `parts/` next to the open circuit (or the executable).
The GUI never requires any of these tools, and nothing in this repo outside `.claude/skills/` refers to Claude.

## Clarifications from the reference implementation
- Any wire vertex or endpoint lying on another wire's segment connects (T-junction), including interior vertices.
- A pin lying in a wire's interior without a vertex there is NOT connected; it raises `wire-through-pin`.
- A label that touches nothing is ignored. `net-short` currently covers only two different label names on one net; its `ids` are the label names.
- `wire-overlap`/`wire-crossing` report one issue per wire pair; `dangling-wire` one per dangling endpoint; `unconnected-pin` has `ids=[part]` with the pin id in the message.
- A wire endpoint also counts as touching if a label name appears at least twice at that point set (closed loops are not dangling).
- Text positions are SVG baselines and text stays upright when parts rotate. `{ref}` is empty when `hidden_label` is set.
- `stroke:"none"` in a shape means no outline. A missing `fill` means no fill.
