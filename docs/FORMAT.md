# short-circ file format, version `short-circ/2`

Two JSON text formats, `.circ` (circuit) and `.part` (component). UTF-8, no BOM. Unknown fields are ignored on read (forward compatible).
This document is the contract between the core crate, the app and any external tool.

**One circuit, two views.** A circuit has a **schematic** view (symbolic diagram) and a **breadboard** view (realistic diagram on breadboards). Parts are logical (`R1` is one resistor); each view places them separately and has its own wires. A component file likewise holds one logical identity (type, pins) with geometry per view. Either view may be absent.

## Coordinates and units
- All circuit coordinates are **integers in grid units**, the same grid in every view. 1 grid unit = 0.1 inch (2.54 mm), which is the breadboard hole pitch.
- +x is right, +y is **down** (screen convention).
- Integer coordinates make connectivity and overlap checks exact (no epsilon).
- **Range**: every coordinate (part placement `at`, wire point, label `at`, part-file pin `at`) satisfies `|x|, |y| <= 1000000` grid units, and a view `size` is finite with `|w|, |h| <= 1000000`. A file outside the range is refused on read with an error containing `coordinate out of range` (exit 2 in the CLI); this keeps all geometry arithmetic exact and the connectivity index bounded.
- **Part transform** (normative, all views): a local point `(x,y)` becomes world `at + R(mirror(x,y))`. `mirror` negates x first (`(x,y)->(-x,y)`). Rotation `rot` is clockwise as seen on screen, applied about the part origin: 90 maps `(x,y)->(-y,x)`, 180 `(-x,-y)`, 270 `(y,-x)`. The rotated bbox is the transformed `[0,0]-[w,h]` box (min/max of its corners).
- Part-local artwork coordinates may be floats (grid units, origin = part origin). **Pin positions must be integers.**

## Circuit file: `*.circ`
```json
{
  "format": "short-circ/2",
  "title": "LED with resistor",
  "parts": [
    {"id":"B1","type":"battery","props":{"value":"9V"},
     "schematic":{"at":[10,10]}},
    {"id":"R1","type":"resistor","props":{"value":"330"},
     "schematic":{"at":[20,10]}, "breadboard":{"at":[14,8],"rot":90}}
  ],
  "wires": [
    {"id":"w1","view":"schematic","pts":[[12,10],[20,10]]},
    {"id":"j1","view":"breadboard","pts":[[10,4],[14,8]],"color":"red"}
  ],
  "labels": [{"at":[12,10],"name":"VCC"}],
  "nets": [{"name":"VCC","pins":["B1.pos","R1.a"]}],
  "lib": []
}
```
Fields:
- `format` (required): `"short-circ/2"`. `"short-circ/1"` files are upgraded on load (see Compatibility).
- `title` (optional string).
- `parts[]`: `id` (unique string across parts and wires, used as reference designator, e.g. `R1`), `type` (component type name, see resolution), `props` (free string map; `value`, `label`, anything). Optional `hidden_label` bool. Optional colour specs (see Colours): `color` (outline of every shape without its own `stroke`), `fill` (fill of closed shapes without their own `fill`), `label_color` (the part's text).
  - Placement per view: `schematic` and/or `breadboard`, each `{"at":[x,y],"rot":0,"mirror":false}` (`rot` is 0, 90, 180 or 270, default 0; `mirror` default false, both omitted when default). A part with no placement in a view is not in that view.
- `wires[]`: `id` (unique), `view` (`schematic` or `breadboard`, default `schematic`; a wire belongs to exactly one view), `pts` list of >= 2 integer points forming a polyline, optional `color` (colour spec, see Colours). Schematic wires: segments may be any direction but orthogonal is strongly preferred. Breadboard wires are jumpers: free-form, any direction.
- `labels[]` (schematic view only): names a net. `at` must coincide with a pin position or lie on a wire. All points sharing a label name are the same net (like KiCad net labels). Optional `color` (colour spec).
- `nets[]` (optional): see "Declared nets".
- `theme` (optional object): circuit-wide colour defaults per kind of object: `background`, `grid`, `wire`, `part`, `label`, `pin`, `junction`, each an optional colour spec (see Colours).
- `lib[]`: optional inline component definitions (same schema as `.part`) that take precedence over external ones.

### Declared nets
`nets` is optional, view-independent connection intent: `{"name":"VCC","pins":["B1.pos","R1.a"]}` says these pins (`PART.PIN`) must be one net. It is what you write when you know the netlist but have not drawn wires; autorouting turns it into wires in either view. When present it is the reference for cross-view checks; when absent the schematic is the reference.

## Component file: `*.part`
```json
{
  "format": "short-circ/2",
  "type": "resistor",
  "name": "Resistor",
  "category": "passive",
  "pins": [{"id":"a","name":"A"}, {"id":"b","name":"B"}],
  "schematic": {
    "size": [8, 2],
    "pins": {"a":{"at":[0,1],"dir":"left"}, "b":{"at":[8,1],"dir":"right"}},
    "shapes": [
      {"k":"line","pts":[[0,1],[2,1]]},
      {"k":"rect","at":[2,0],"size":[4,2]},
      {"k":"line","pts":[[6,1],[8,1]]},
      {"k":"text","at":[4,-0.8],"text":"{ref}","size":0.9,"anchor":"middle"}
    ]
  },
  "breadboard": {
    "size": [4, 1],
    "pins": {"a":{"at":[0,0]}, "b":{"at":[4,0]}},
    "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 40 10\">...</svg>"
  }
}
```
- `type`: unique lowercase name referenced by parts. `name`: display name. `category`: free text; `board` has a meaning (see Boards).
- `pins[]`: the **logical** pins, in order: `id` (unique within the part), optional `name`. Defined once for all views.
- `buses[]` (optional): groups of pin ids that are internally connected, e.g. `[["a1","a2","a3","a4","a5"]]` for a breadboard strip, or both sides of a link. Applies in every view the part appears in.
- `schematic` / `breadboard` (each optional, same shape, `ViewDef`):
  - `size`: `[w,h]` bounding box in grid units from the part origin (0,0) to (w,h), used for overlap checks. Local coordinates may extend slightly outside; the bbox is authoritative.
  - `pins`: map from pin id to `{"at":[x,y],"dir":"left|right|up|down"}`. `at` is an integer local position. `dir` is the direction the pin leads out of the body (label placement); it is ignored in the breadboard view and may be omitted. A pin missing from a view's `pins` has no position there and cannot be connected in that view. The map's keys must be ids from `pins[]`.
  - `shapes[]`: vector artwork drawn in order (below).
  - `svg`: a complete SVG document stretched over `size` (the viewer rasterises it; export embeds it). Used by realistic artwork. When both `svg` and `shapes` are present `svg` is drawn first and `shapes` on top.
- A view absent from the file means the part cannot be placed in that view (`no-view`). Power symbols typically have only `schematic`; breadboards have only `breadboard`.
- Optional `params`: for generated families, `{"gen":"dip","pins":8}` is reserved for built-in generators (`dip`, `header`). Third-party parts should just list pins and views.
- Optional `meta`: string-to-string map of free-form metadata, never interpreted by the viewer. Imported Fritzing parts carry `source` (`"fritzing"`), `title`, `module_id` (the fzp `moduleId`), `label`, `description`, `tags` (comma separated) and every fzp `<property>` by lowercase name (`family`, `package`, `pins`, `resistance`, ...). Omitted when empty.

### Shapes
Common optional fields: `stroke` (colour spec; default: the part's `color`, else the theme part colour), `fill` (colour spec or `"none"`; default: the part's `fill`, else none; only closed shapes `rect`, `circle`, `poly` are filled), `width` (stroke width in grid units, default 0.12). For `text` shapes the colour is `fill`, else `stroke`, else the part's `label_color`, else the theme part colour (a `"none"` in a text shape is ignored).
- `{"k":"line","pts":[[x,y],...]}` polyline
- `{"k":"rect","at":[x,y],"size":[w,h],"r":0}` optional corner radius
- `{"k":"circle","at":[x,y],"r":1}`
- `{"k":"arc","at":[x,y],"r":1,"from":0,"to":180}` degrees, clockwise from +x, y-down
- `{"k":"poly","pts":[...]}` closed polygon, fill allowed
- `{"k":"text","at":[x,y],"text":"...","size":1,"anchor":"start|middle|end"}`; `{ref}` expands to the part id, `{value}` to `props.value`, `{prop:name}` to any prop.

Shape colours (`stroke`, `fill`) imported from SVG are lowercase `#rrggbb` strings. A black `stroke` is dropped (theme foreground); a black `fill` stays `#000000`.

### Boards
A part with `category: "board"` is a carrier such as a breadboard: its pins are holes, its `buses` are the strips. Boards are exempt from `part-overlap` and from `unconnected-pin`, and any number of component legs may sit in distinct holes of one. Typical generated boards: `breadboard_mini`, `breadboard_half`, `breadboard_full`.

### Importing Fritzing parts
`short-circ.exe --import-fritzing` converts a Fritzing part (`.fzpz` bundle, or `.fzp` plus its SVG files) into **one `.part` with every view the source has**:
- `--views schematic,breadboard` (default both; either one alone is allowed). A listed view the source lacks gives a warning; it is an error only when none of the requested views can be produced. The schematic SVG becomes `schematic.shapes` (existing converter); the breadboard SVG is kept as `breadboard.svg` after sanitising (below). Real sizes come from the root `width`/`height`/`viewBox` (units `in`, `mm`, `cm`, `pt`, `pc`, `px`; unit-less lengths are 72 dpi for Illustrator exports, 96 dpi for Inkscape 0.92+, otherwise 90 dpi) scaled to the grid; the breadboard size comes from the root `width`/`height`, else from the `viewBox` size times that scale. For the schematic CSS `<style>` classes, `<use>`, group styles and nested transforms are honoured; SVG defaults apply (fill black, stroke none unless given).
- **Sanitiser** (breadboard art). Removed: `<script>` and `<foreignObject>` elements, `on*` attributes, `javascript:` URLs, `xlink:href`/`href` values that do not start with `#` or `data:image/`, `<style>` blocks containing `@import`, `javascript:` or an external `url()`, and `xml-stylesheet` processing instructions. The only other change to the artwork is the root `viewBox` (shifted so the pins land on the grid, or added when the source has none). The same rules guard every use of `breadboard.svg`, whatever its origin (inline `lib`, hand-written `.part`): SVG export and the viewer do not draw artwork that the sanitiser would modify (the part's `shapes` are still drawn), and the viewer's rasteriser never loads files for `<image>`.
- Logical `pins[]` come from the `<connector>` list: pin `id` is the trimmed connector `name`, or the connector `id` when the name is empty or already taken (the original name is kept in `name`). A connector's position in a view is the centre of its `terminalId` element; without a terminal, the outer end of the `svgId` element if it is a long thin line, else its centre. Positions are rounded to integers (the drawing is shifted by the mean rounding error first when pins are off-grid by 0.08 unit or more), coinciding pins are nudged with a warning, and the whole view is moved so its bounding box starts at `(0,0)`. Schematic `dir` points to the nearest edge of `size`. A connector without a position in a view is omitted from that view's `pins`.
- Fritzing `<buses>` become `buses[]`.
- A `{ref}` text is added above the schematic body. Type name: `--type`, else a lowercase letters/digits/underscore slug of the title (then of the moduleId). A `category` of `board` is set for parts whose family property is `breadboard`.
- `.fzpz` and `.fzp` files found in a `parts/` folder are imported on the fly with the defaults above (a `.part` or an earlier source with the same type wins; files that fail to import are ignored).
- Not handled: PCB view, breadboard legs (bendable leads), multi-layer artwork, patterns, text fonts in the schematic converter. Fritzing core parts are CC-BY-SA; none are bundled.

## Component resolution order for a part `type`
1. `lib[]` inside the circuit file
2. `*.part` files (and Fritzing `*.fzpz` / `*.fzp`) in a `parts/` folder next to the `.circ` file
3. the same in a `parts/` folder next to the executable
4. Built-ins embedded in the executable (schematic and, for common parts and boards, breadboard views) plus the generated families `dipN` and `headerN`

## Connectivity rules (normative; the netlist builder implements exactly this)
Connectivity is derived **per view** from geometry. Each view has its own nets. Pins are the logical pins that have a position in the view, for parts placed in it.

### Schematic
A **point** is connected to everything else at the same integer coordinate. Two things are electrically joined when:
1. A wire **endpoint** (first or last pt) equals a pin position, a wire pt (any vertex or endpoint) of another wire, or lies on the interior of another wire segment (T-junction).
2. A wire **interior vertex** equals another wire's endpoint or a pin position (the polyline touches there).
3. Two wires crossing at a point that is not an endpoint or vertex of either do **not** connect (they merely cross).
4. Pins of different parts at the same coordinate connect directly.
5. Labels with the same name merge their nets.
6. Pins of one part listed together in a `bus` are one node.
A net with zero pins is allowed but reported as a warning.

### Breadboard
Wires are flexible jumpers and never join sideways:
1. A wire joins things only at its **two end points** (first and last pt). Interior points are bends and connect nothing, and a wire never T-connects onto another wire's side.
2. Everything at the same grid point is one node: pins of any parts (a leg in a board hole, a board hole and a wire end, ...) and wire end points.
3. Pins of one part listed together in a `bus` are one node (this is how a strip joins its holes). A wire is a node joining its two end points.
4. Labels are not used in this view.

## Cross-view consistency
For parts placed in **both** views, the nets of each view are compared with the reference: the declared `nets` when the circuit has any (only pins mentioned there are constrained), otherwise the schematic. Boards and parts placed in only one view are ignored by the comparison. The rules are listed in the validation table (`net-open`, `net-merged`, `unplaced-part`, `no-view`). A view is "in use" when at least one part is placed in it or at least one wire is drawn in it.

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
Closed shapes have no fill unless level 1 or 2 gives one (there is no theme fill). Breadboard `svg` artwork is drawn as authored and is never recoloured (part `color`/`fill` apply only to `shapes`).

Explicit colours (levels 1 to 3) are used exactly as written in both the viewer and SVG export; they are never inverted or adjusted for dark mode, so `black` on a dark background stays black. Only the built-in defaults follow the light/dark theme. Junction dots use `theme.junction` if set, otherwise the resolved colour of the first wire (file order) that touches the dot. A `background` of `none` leaves the SVG transparent (the viewer keeps its own background); a `grid` of `none` hides the viewer grid. Translucent colours (`#rrggbbaa`) are written to SVG as `#rrggbb` plus `fill-opacity` / `stroke-opacity`. SVG export and the viewer share one resolver, so they agree.

## Validation rules (ids used in reports)
Severity `error` unless noted. "Per view" rules run once for each view in use and carry `view` in the issue.
| rule | scope | meaning |
|---|---|---|
| `schema` | circuit | JSON invalid, wrong `format`, missing required field |
| `duplicate-id` | circuit | id used twice among parts and wires |
| `unknown-type` | circuit | part type not resolvable |
| `bad-rotation` | per view | rot not in {0,90,180,270} |
| `off-grid` | circuit | non-integer coordinate (should not occur after parse; reserved) |
| `no-view` | per view | a part is placed in a view its type has no geometry for |
| `wire-overlap` | schematic | two wires share a collinear stretch of positive length |
| `wire-crossing` (warning) | schematic | two wires cross at a point where neither has an endpoint/vertex, implying a missing junction |
| `wire-through-pin` | schematic | a wire passes through a pin position at its interior without a vertex there |
| `wire-diagonal` (warning) | schematic | non-orthogonal segment |
| `wire-degenerate` | per view | zero-length segment or fewer than 2 distinct points |
| `dangling-wire` (warning) | per view | an end touches nothing (schematic: an endpoint; breadboard: an end point) |
| `unconnected-pin` (warning) | per view | pin on no wire and no other pin (board pins and pins whose bus in the part definition has 2 or more members are exempt) |
| `part-overlap` | per view | bounding boxes of two non-board parts intersect with positive area |
| `pin-conflict` | breadboard | two or more pins of non-board parts at the same point (two legs in one hole); one issue per conflicting point, `ids` = the part ids |
| `net-short` | schematic | two pins on the same net belong to a part pair that declares `props.nets` distinct, or two labels with different names join the same net |
| `empty-net` (warning) | schematic | net with no pins |
| `bad-color` (warning) | circuit | a colour spec (part `color`/`fill`/`label_color`, wire/label `color`, `theme.*`, a `lib` shape `stroke`/`fill`) is not a colour, `"none"` or `"default"`; `ids=[object id]` (`theme` for the theme, the type name for a `lib` shape), the message names the field and the value |
| `net-open` (warning) | cross-view | a reference net is split into 2+ groups in a view (not yet routed there); `ids` = one pin of each group (`PART.PIN`) |
| `net-merged` | cross-view | a view joins pins of two different reference nets; `ids` = a pin of each |
| `unplaced-part` (warning) | cross-view | a part whose type has geometry for a view that is in use, but the part has no placement there (boards included) |
| `bad-net` | circuit | a declared `nets` pin reference names a part or pin that does not exist |

## Report format (CLI `--validate --json`)
```json
{"ok":false,"errors":1,"warnings":2,"issues":[
 {"rule":"wire-overlap","severity":"error","view":"schematic","ids":["w1","w2"],"at":[14,10],"message":"w1 and w2 overlap between (14,10) and (16,10)"}
]}
```
`view` is omitted for circuit-wide and cross-view issues; `at` is `null` when there is no location (always for cross-view issues). Exit code: 0 no errors (warnings allowed), 1 errors, 2 unreadable input.

`--validate` with `--view all` (the default) runs every rule over the circuit; with `--view V` it runs the circuit-wide rules plus only the rules of view `V` (library `validate_view`), and skips the cross-view rules. Each per-view rule reports once per view in use.

## Canonical writing
Every tool that writes `.circ` or `.part` (the editor, `--upgrade`, `--import-fritzing`, autorouting) uses one writer so files diff cleanly: 2-space indentation, top-level keys as `"key": value`, entries compact (`{"id":"R1","type":"resistor"}`, no spaces) and one per line for `parts`, `wires`, `labels`, `nets`, part `pins` and `buses`, view `pins` and `shapes`, stable key order (circuit: `format`, `title`, `parts`, `wires`, `labels`, `nets`, `theme`, `lib`; view `pins` in the order of the part's `pins[]`), no trailing spaces, a final newline. Parsing then writing the output again must give identical bytes. Files are written through a temporary file in the same directory that is renamed over the target (`write::write_atomic`), so a failed write never damages an existing file.

What the writer omits or always writes:
- Optional fields that are `None` are omitted. Defaults are omitted: placement `rot` 0 and `mirror` false, `hidden_label` false, rect `r` 0, text `size` 1 and `anchor` `start`.
- Always written: a part's `props` (even `{}`), a wire's `view`, `labels` and `lib` (`[]` when empty), and the `dir` of a schematic pin. A breadboard pin's `dir` is omitted when it is `left`.
- `nets` is omitted when empty and `theme` when absent (it is written on one line).
- Inline `lib` entries are written as full part objects, indented under `lib`.
- Floats print without a trailing `.0` (`2`, not `2.0`).
- The file is UTF-8 without a BOM, and a BOM makes the file unreadable, so write files without one (Windows PowerShell 5.1 `Set-Content -Encoding utf8` adds a BOM and `>` writes UTF-16; use a file-writing tool or `[IO.File]::WriteAllText` with `UTF8Encoding($false)`).

## Editing
Edits apply to a circuit in memory and are grouped: a drag, a paste or an autoroute run is one undo step. Moving a part drags the wire ends attached to its pins in the same view. Wires never move between views and moving a part in one view never changes another view.

Reference-implementation details:
- Applying a group of edits is atomic: if any edit fails, the circuit is left unchanged.
- Rubber-banding (moving a part with its wires) moves only a wire's first and last point, only when that point sits exactly on an old pin position of a moved part, and only for wires of the same view. Labels do not follow.
- Removing a part also removes its `PART.PIN` references from the declared `nets`, and drops a net that this leaves with no pins (the wires of the part stay and dangle).
- Splitting a wire keeps the original id for the first half; the second half gets a fresh id.
- `tidy` (merge collinear points and drop duplicates) keeps every vertex that another wire, a label or a pin touches in the schematic; in the breadboard it merges freely.

## Autoroute and port
`route::port` (CLI `--autoroute`, GUI dialog) realises the nets of one view in the other: it places missing parts, then draws wires. It is pure (circuit in, circuit out) and its output must pass `--validate` with no new errors; nets it cannot realise are reported with a reason, never silently dropped.

Options (`RouteOptions`), with CLI flag and default:
| option | flag | default |
|---|---|---|
| target view (`to`) | `--to schematic\|breadboard` (`sch`, `bb` accepted) | `breadboard` |
| source of nets | `--from-nets` (declared `nets`) | the other view |
| scope | `--scope all\|unrouted`, `--only R1.a,B1.pos` | `all` |
| place parts missing in the target | `--no-place` to disable | on |
| keep existing target wires | `--replace` to remove them first | keep |
| allow crossings (schematic: wires cross; breadboard: a jumper may pass over another) | `--crossings` | off |
| prefer fewer bends or shorter length | `--prefer bends\|length` | `bends` |
| schematic: clearance around other parts (grid units) | `--clearance N` | 1 |
| schematic: label nets that fail to route | `--no-labels` to disable | on |
| breadboard: jumper shape | `--jumper straight\|orthogonal\|diagonal` (`diagonal45` is an alias of `diagonal`) | `orthogonal` |
| breadboard: jumper colour | `--jumper-color by-net\|<colour>` | `by-net` |
| breadboard: longest jumper (Manhattan length, grid units) | `--max-jumper N` | unlimited |
| breadboard: allow jumpers over component bodies | `--over-parts` | off (jumpers keep off components) |
| breadboard: allow placed components to be moved | `--move-parts` | off |
| join wires that meet end to end | `--no-merge` to disable | on |

Rules: schematic routing is orthogonal on the grid and avoids part bodies (plus clearance), foreign pins and foreign wires, so it introduces no `wire-overlap`, `wire-through-pin` or accidental connection; with crossings off it also introduces no `wire-crossing`. Breadboard placement puts ICs astride the board's centre gap, passives bridging strips, and each pin in its own hole (no `pin-conflict`); leftover nets get jumpers; with crossings off a jumper never touches another breadboard wire (it takes another shape, then a detour with orthogonal bends, and a net that cannot is reported `failed NET: every jumper route would cross another wire`). Parts already placed in the target view are never moved, unless `--move-parts` is given: then the components of the routed nets are placed again from scratch (boards stay) and all breadboard wires are removed first, because they would no longer meet the pins; it has no effect with `--no-place`. An `orthogonal` jumper always has horizontal and vertical segments only. With components avoided (the default) a jumper may still start or end under a component that covers its hole. After routing, wires of the target view that meet end to end (exactly two ends, same colour) are joined into one wire, existing wires included; the join keeps a vertex where a pin, label or other wire touches the meeting point, and breadboard wires are never joined on a hole.

Reference-implementation details:
- Scope: a net that is already fully connected in the target view counts as routed under `all` and is skipped under `unrouted`. `--only` wins over `--scope`. `--replace` also drops labels that sat on the removed wires.
- Schematic placement leaves a gap between placed parts of max(4, 2 * clearance + 2 + tracks) grid units, where tracks = min(8, 2 + nets / 6), so routing channels exist.
- Breadboard placement scores candidate positions by the number of nets that would need a jumper (weight 100 each), then by the distance to the parts they connect to.
- A net with a usable pin whose part is not placed in the target view (`--no-place`, or no spot found) is never counted as routed: each such pin is reported as `failed NET: PART.PIN is not placed in the <view> view`, except for a part already reported by its id. The pins that are placed are still wired.
- A board added by placement goes to (0,0) of the breadboard view, or clear of everything already placed there when loose legs (parts placed without a board) would otherwise land on its holes and short their nets; it is not added (and the parts get "no breadboard available") if no position adds no validation error.
- Schematic placement keeps new part bodies one unit clear of existing wires and labels, so a new pin never lands on one.
- `--clearance` is at most 100 and `--max-jumper` at most 1000000 (larger values are argument errors, exit 2; the library clamps them).
- Failures are printed `failed NET: reason`; when a part could not be placed at all, NET is the part id. OUT is still written and the exit code is 1.
- Limits: with no crossings and no rotation of placed parts, nets whose pins interleave around one IC cannot all be wired planar-ly. Such nets fall back to net labels, or fail under `--no-labels`. `--crossings` lets them cross instead (the result then has `wire-crossing` warnings). On the breadboard the strips of a net can run out of free holes that are not walled in by earlier jumpers; with crossings off such a net is reported rather than drawn across, and `--crossings` joins it.

## CLI contract (`short-circ.exe`)
Views: `--view schematic|breadboard` where noted; omitted means the default stated.
- `short-circ.exe [file.circ]` open GUI
- `short-circ.exe --validate file.circ [--view schematic|breadboard|all] [--json]` (default `all`: every view in use plus the cross-view checks)
- `short-circ.exe --render file.circ out.svg [--view schematic|breadboard] [--dark]` (default `schematic`)
- `short-circ.exe --netlist file.circ [--view schematic|breadboard] [--json]` (default `schematic`) prints every net as `net ID  NAME-or--  pins: R1.a, B1.pos  wires: w1, w2` (pins sorted naturally, `-` for an empty list). With `--json`: `{"nets":[{"id":0,"name":"GND","pins":["R1.b"],"wires":["w1"]}]}`; `name` is `null` for an unlabelled net. Exit 0 ok, 2 unreadable.
- `short-circ.exe --render-part TYPE out.svg [--view schematic|breadboard] [--dark] [--circuit-dir DIR]` draws one part view with every pin id (`id` or `id:name`) labelled at its pin position, plus its declared bounding box and a title line `type - name (category), WxH, N pin(s)` (default `schematic`). `TYPE` resolves like a circuit would. Exit 0 written, 1 unknown type, missing view or write error.
- `short-circ.exe --upgrade IN [OUT]` rewrites a v1 or v2 `.circ`/`.part` in canonical v2 form to `OUT`, or in place when `OUT` is omitted. Exit 0 ok, 2 unreadable.
- `short-circ.exe --autoroute IN OUT [options above]` runs `route::port` and writes the resulting circuit to `OUT` (canonical form), printing one line per failure (`failed NET: reason`) and a summary (`placed N part(s), added M wire(s), routed K net(s), F failed`). Exit 0 all nets realised, 1 some failed, 2 unreadable or bad arguments.
- `short-circ.exe --import-svg in.svg out.part --type NAME [--pins "a:0,1;b:8,1"] [--scale PX_PER_UNIT]` (produces a schematic-only part)
- `short-circ.exe --import-fritzing IN OUT [--views schematic,breadboard] [--type NAME]` imports Fritzing parts. `IN` is a `.fzpz`, a `.fzp`, or a folder (every `.fzpz`/`.fzp` directly inside, not recursive; a `.fzp` next to a `.fzpz` of the same name is skipped). `OUT` is a `.part` file, or a folder (created if missing) that receives `<type>.part`; a folder `IN` needs a folder `OUT` and cannot take `--type`. One summary line per part on stdout (`type: views schematic+breadboard, N pin(s) -> path`), warnings on stderr (`warning: file: ...`). Exit 0 all imported, 1 an import or write error (other files of a folder are still written), 2 bad arguments.
- `short-circ.exe --list-parts` prints resolvable component types with the views each has
- `short-circ.exe --list-colors` prints the preset colour names with their hex codes
- In the GUI, File > Import Fritzing part... does the same for one file and writes into `parts/` next to the open circuit (or the executable).
The GUI never requires any of these tools, and nothing in this repo refers to Claude except this README paragraph about the optional skill, which is published with each release rather than stored here.

## Compatibility
`short-circ/1` files load through one upgrade function (`migrate`) and nothing else in the program knows about v1:
- circuit: each part's `at`/`rot`/`mirror` become its `schematic` placement; every wire gets `view: schematic`; labels and `lib` are kept (library entries are upgraded as parts below); `format` becomes `short-circ/2`.
- part: `size`, pin `at`/`dir` and `shapes` move into `schematic`; pin `id`/`name` stay in the logical `pins`. The v1 `svg` path-data field is dropped (it was never rendered).
- A v1 file validates with the same results it had before (no breadboard view is in use, so no cross-view check fires).
`--upgrade` writes the result as canonical v2.

## Clarifications from the reference implementation
- Any wire vertex or endpoint lying on another wire's segment connects (T-junction) in the schematic, including interior vertices.
- A pin lying in a wire's interior without a vertex there is NOT connected in the schematic; it raises `wire-through-pin`.
- A label that touches nothing is ignored. `net-short` currently covers only two different label names on one net; its `ids` are the label names.
- `wire-overlap`/`wire-crossing` report one issue per wire pair; `dangling-wire` one per dangling endpoint; `unconnected-pin` has `ids=[part]` with the pin id in the message.
- A wire endpoint also counts as touching if a label name appears at least twice at that point set (closed loops are not dangling).
- Text positions are SVG baselines and text stays upright when parts rotate. `{ref}` is empty when `hidden_label` is set.
- `stroke:"none"` in a shape means no outline. A missing `fill` means no fill.
