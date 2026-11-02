---
name: circuit
description: Create, edit and verify circuit diagrams as short-circ `.circ` files, and create or import custom components as `.part` files. Use when the user wants Claude to design a circuit, check it for problems such as overlapping wires, crossings, dangling wires, unconnected pins or overlapping parts, render it to SVG, or add a custom component.
---

# circuit: author and verify short-circ diagrams

The short-circ app is a standalone Windows viewer. This skill only writes files in its format and uses the
app's headless CLI to check them. The app does not depend on this skill.

Format reference: `docs/FORMAT.md` in the repo (normative). Short version below.

## Find the tool
Use `short-circ.exe` from, in order: the user's stated path, `target/release/short-circ.exe` in the repo,
`short-circ.exe` on PATH. If none exists, build it from the repo root with PowerShell
(not Git Bash, whose `link` shadows the MSVC linker): `cargo build --release -p short-circ`.

CLI (exit codes: 0 ok, 1 errors found, 2 unreadable input):
- `short-circ.exe --validate FILE.circ [--json]`
- `short-circ.exe --render FILE.circ OUT.svg [--dark]`
- `short-circ.exe --netlist FILE.circ [--json]` (every net with its pins `R1.a` and wire ids; use it to verify connectivity)
- `short-circ.exe --render-part TYPE OUT.svg [--dark] [--circuit-dir DIR]` (one part with all pin ids labelled; use it to verify part geometry, add `--circuit-dir` for a circuit's own `parts/`)
- `short-circ.exe --list-parts` (resolvable component types; `dipN` and `headerN` are generated on demand)
- `short-circ.exe --list-colors` (preset colour names with hex codes)
- `short-circ.exe --import-svg IN.svg OUT.part --type NAME [--pins "a:0,1;b:8,1"] [--scale PX_PER_UNIT]`
- `short-circ.exe --import-fritzing IN OUT [--view schematic|breadboard] [--type NAME]` (exit 0 ok, 1 import error, 2 bad arguments)

## Workflow
1. Run `--list-parts`, and read the `.part` files you will use (in `parts/`) to get exact pin ids and positions.
2. Write the `.circ` file (rules below). Prefer a few long orthogonal wires over many tiny ones.
3. Run `--validate FILE.circ --json`. Fix every `error`; fix or consciously accept each `warning`.
   Repeat until clean. Do not claim a circuit is verified without a passing run.
4. Run `--render` and look at the SVG (open it, or convert/view it) to sanity-check the layout.
5. Report the file path, validation result and any accepted warnings.

## .circ essentials
```json
{"format":"short-circ/1","title":"...",
 "parts":[{"id":"R1","type":"resistor","at":[14,6],"rot":0,"props":{"value":"330"}}],
 "wires":[{"id":"w1","pts":[[11,10],[11,7],[14,7]]}],
 "labels":[{"at":[11,16],"name":"GND"}],
 "lib":[]}
```
- Integer grid coordinates, 1 unit = 0.1 in, +y is down. Ids unique across parts and wires.
- World pin position = `at + R(mirror(local pin))`; rot is clockwise on screen: 90 maps (x,y) to (-y,x).
  Compute every pin position before routing wires, and end wires exactly on pins.
- Wires connect when an endpoint or vertex touches a pin, a vertex/endpoint, or the inside of another wire
  segment (T-junction). Wires that merely cross do NOT connect. Same-name labels join nets (use for GND/VCC
  instead of long wires).
- Keep wires orthogonal. Leave at least 2 grid units between parts so bounding boxes do not overlap.

## Constraints the validator checks
`wire-overlap` (collinear shared stretch), `wire-crossing` (warning: crossing without junction),
`wire-through-pin`, `wire-diagonal`, `wire-degenerate`, `dangling-wire`, `unconnected-pin`, `part-overlap`,
`net-short` (two label names on one net), `empty-net`, `bad-color` (warning), `duplicate-id`, `unknown-type`,
`bad-rotation`, `schema`.
If the user states other constraints (e.g. max width, all grounds on one net), check them yourself by reading the
JSON, and say which checks are the validator's and which are yours.

## Colours
Colours are optional; use them to make nets readable. A colour is a hex code (`#rgb`, `#rrggbb`, `#rrggbbaa`) or a
preset name (`--list-colors`: black, white, gray, silver, red, orange, yellow, green, lime, cyan, blue, navy, purple,
magenta, pink, brown, teal, gold); `"none"` draws nothing and `"default"` means the theme default.
- Wires: `"color":"red"` per wire. Convention: power red, ground black (or gray, since black vanishes on a dark
  background and explicit colours are never inverted), signals other distinct colours, one colour per net.
- Parts: `"color"` (outline), `"fill"` (fill of closed shapes), `"label_color"` (text). Labels: `"color"`.
- Circuit-wide: `"theme":{"background","grid","wire","part","label","pin","junction"}` sets defaults. If you set
  `background`, also set `part`, `wire`, `label` and `pin` so the drawing stays legible in light and dark mode.
- Precedence: the object's own colour, then a shape's own stroke/fill over the part's `color`/`fill`, then
  `theme`, then the built-in default. A bad colour is a `bad-color` warning and falls back to the default.
See `examples/colors.circ`.

## Custom components
Create from shapes: write a `.part` file (see FORMAT.md). Primitives: line, rect, circle, arc, poly, text.
Pins need integer local positions on the body edge; `size` must cover the pins and drawing. Place it in a
`parts/` folder next to the `.circ` file (auto-resolved), or embed it in the circuit's `lib` array to keep one file.
Import from SVG: `--import-svg` with explicit `--pins` (pin ids and grid positions are your decision; the SVG
only gives artwork). Then draw a one-part test circuit, `--validate` and `--render` it to check pin alignment.
Name types in lowercase with underscores.

### Importing Fritzing parts
`short-circ.exe --import-fritzing PART.fzpz parts/` (or a `.fzp` with its SVGs, or a folder of them; OUT can also be
a `.part` path) writes `parts/<type>.part` with the graphics and pins from the part's schematic view (`--view
breadboard` for the breadboard picture). It prints `type: size WxH, N pin(s) -> path` and warnings; read the
warnings (duplicate pin names, pins moved, unusual size, missing terminals). A `.fzpz` dropped into a `parts/`
folder is also picked up without conversion, but converting gives a file you can read and edit.
Check the result before using it:
1. Open the `.part`: `pins[].id` is the Fritzing connector name (e.g. `VCC`, `Pin 4`; `connectorN` when names
   clash), `at` the local grid position, `size` the bounding box, `meta` the Fritzing title, family and package.
   Fritzing sizes are real (1 unit = 0.1 in), so a small part may be only a few units wide.
2. Make a one-part test circuit that uses the type (`at` e.g. `[10,10]`), `--validate` it and `--render` it; each pin
   should sit on the end of its drawn pin line. Wire to pins using the positions in the `.part` (world = `at` + local).
3. If a pin is clearly misplaced or a drawing is missing, fix the `.part` by hand (it is plain JSON) rather than
   guessing, and tell the user. Fritzing library parts are CC-BY-SA licensed; mention attribution when sharing them.

## Don'ts
Do not edit the Rust sources to work around validation. Do not hand-roll connectivity by guessing pin
coordinates: derive them from the part definitions.
