# short-circ

short-circ is a small, portable Windows viewer for circuit diagrams. One circuit file holds two views of the same circuit: a **schematic** and a **breadboard** layout. Circuits are plain JSON (`.circ`), easy to read and to generate. The app checks them for wiring mistakes, checks that the two views agree, and exports SVG.

It is a single static `.exe` (about 7 MB in a release build) built with Rust and [egui](https://github.com/emilk/egui). There is no installer, no runtime and no web view. The same executable also runs as a headless command-line tool.

## Features

- **Two views, one circuit**: parts are logical (`R1` is one resistor) and are placed separately in the schematic and on a breadboard, each view with its own wires. Switch with the Schematic / Breadboard tabs.
- **Viewer**: pan, zoom, fit to window, hover pins and wires to see their names and nets, click a part for its details, live reload when the file changes.
- **Net highlighting**: click a pin or wire to lock its net highlight, or pick a net in the nets panel.
- **Validator**: overlapping wires, crossings without a junction, dangling wires, unconnected pins, overlapping parts, two legs in one breadboard hole, bad colours, and cross-view checks (a net open or merged in one view, a part missing from a view).
- **Autoroute**: write the netlist, or one view, and let the tool place parts and draw the wires of the other view (`--autoroute`).
- **SVG export**: from the File menu, or with `--render` on the command line.
- **Command line**: `--validate` with exit codes for scripts and CI, plus `--render`, `--netlist`, `--autoroute`, `--upgrade`, `--render-part`, `--list-parts`, `--list-colors`, `--import-svg` and `--import-fritzing`.
- **Built-in parts**: 26 types (passives, semiconductors, sources, three breadboards), plus generated DIP packages (`dipN`) and pin headers (`headerN`), and a part browser.
- **Fritzing import**: one `.fzpz` gives a part with both its schematic and breadboard view. See [Fritzing component import](#fritzing-component-import).
- **Colours**: hex codes, preset names and circuit-wide theme defaults. See [Colours](#colours).

## Quick start

1. Build the executable as described in [Building from source](#building-from-source). The result is one file, `target\release\short-circ.exe`, which you can copy anywhere.
2. Open an example circuit from the repository root:

   ```bat
   target\release\short-circ.exe examples\led.circ
   ```

   You can also start the viewer without a file and open one from the File menu.

Examples in [`examples/`](examples):

| File | What it shows |
|---|---|
| `led.circ` | Battery, resistor and LED, with both a schematic and a breadboard view. |
| `breadboard_demo.circ` | A hand-placed breadboard layout with jumpers. |
| `nets_only.circ` | A 555 astable blinker written as parts and declared nets only, no placement and no wires. |
| `555_blinker.circ` | The same circuit after `--autoroute` produced the schematic and the breadboard. |
| `colors.circ` | Colours, theme and defaults. |
| `bad.circ` | Deliberately broken, to see what the validator reports. |

## Building from source

You need:

- Rust (stable) with the MSVC toolchain, installed with [rustup](https://rustup.rs). The default `x86_64-pc-windows-msvc` target is what you want.
- Visual Studio Build Tools with the **Desktop development with C++** workload. This provides the MSVC linker and the Windows SDK.

Run the build from **PowerShell or cmd**, not from Git Bash. Git Bash has its own `link` program, which shadows the MSVC linker and breaks the build.

```bat
cargo build --release -p short-circ
```

The executable is written to `target\release\short-circ.exe`. To run the tests:

```bat
cargo test --workspace
```

`.cargo/config.toml` links the C runtime statically, so the built `.exe` does not need the Visual C++ redistributable.

## Using the viewer

**Opening a file**

- Pass it on the command line: `short-circ.exe examples\led.circ`.
- Use File > Open... or press Ctrl+O.
- Drop a `.circ` file onto the window.
- File > Open Recent lists recently opened files.

**Moving around**

- Drag with the left or middle mouse button to pan.
- Use the mouse wheel to zoom.
- Press F, or use View > Fit to window, to fit the whole circuit in the window.
- Click the **Schematic** or **Breadboard** tab, or press 1 or 2, to switch view. A view the circuit does not use is empty.
- The View menu toggles the grid, the dark theme (on by default), the issues panel, the nets panel and the part browser.
- View > Labels shows or hides part references, part values, wire ids and pin names. Text that would be smaller than 7 pixels is hidden automatically. Text whose colour is too close to the background is corrected to stay readable, and every label has a thin outline of the opposite lightness so it also reads over part fills and breadboard artwork.
- Shortcuts: Ctrl+O open, F fit, 1 / 2 schematic / breadboard, R or F5 reload now, G toggle the grid, Esc clear the selection. Help > Shortcuts lists them.
- **Edit mode** (Ctrl+E or the "Edit mode" button; View mode stays the default and read-only): select, marquee, move, nudge, rotate (R), mirror (M), delete, copy/cut/paste, a Wire tool (W), a Label tool (L, schematic), placing parts from Edit > Add part or the part browser, a properties panel (id, props, colours, placement per view, title, declared nets), undo/redo (Ctrl+Z, Ctrl+Y), File > New / Save (Ctrl+S) / Save As. It works in both the schematic and the breadboard view. **Wire points**: select a wire to see a square at each end and bend and a "+" in the middle of each segment. Drag an end or a bend to move just that point (in the schematic the segments stay horizontal or vertical; the other points stay put, so a wire on a pin stays attached). Drag a "+" or Alt+drag anywhere on a wire to add a bend and place it in one go, which is how to make a turn in a breadboard jumper; double-click a bend to remove it. Dragging the middle of a wire still moves all of it. Files are written in the canonical format; the title shows `*` while there are unsaved changes, live reload is suspended meanwhile (a "changed on disk" banner offers Reload or Keep), and closing or opening another file asks to Save, Discard or Cancel. In Edit mode R rotates, F5 reloads.

**Hovering and clicking**

- Hover a pin to see its name and net. The wires and pins of that net are highlighted.
- Hover a wire to see its id and net, with the same highlighting.
- Click a part to show its details on the right: type, position, rotation, properties, colours and pins with their nets.
- Click a pin or wire to lock the highlight on its net. Click empty space or press Esc to clear the selection and the lock.
- View > Nets lists every net of the current view with its name and pins. Click one to highlight it and zoom to fit it.
- The viewer reloads the circuit automatically when the file, or the `parts/` folder beside it, changes (View > Auto-reload, on by default). If the new file is broken, the old circuit stays on screen and an error banner appears.
- View > Part browser lists every part type that can be resolved, with a preview that labels each pin. It is useful for checking an imported component.
- The issues panel at the bottom lists the problems the validator found. Click an issue to move the view to it.
- File > Export SVG... saves the current view as an SVG image.

## Editing

The app can also edit circuits in both views (move, rotate and place parts, draw wires and jumpers, undo and redo, save). Edit mode: Ctrl+E; the details are under "Using the viewer" above.

Tools > Autoroute... opens the same autorouter as the `--autoroute` command, so you can realise one view from the other without leaving the app. The dialog asks for the toggles first: the target view, whether the nets come from the other view or from the declared `nets`, which nets (all, only unrouted, or only those of the selected part), whether to place missing parts and keep existing wires, and the options of the chosen view (crossings, labels, clearance and preference for the schematic; crossings, jumper style, colour and maximum length for the breadboard). With "Allow wire crossings" off on the breadboard, jumpers never touch each other: each takes another shape or detours around the wires already placed, and a net that cannot be joined that way is listed as failed. It also shows the equivalent command line.

Preview runs the autorouter in the background and shows the result on the canvas, with the new parts and wires highlighted, a list of the nets that could not be routed, and no change to your circuit. Apply commits the result as one undo step (Ctrl+Z puts everything back) and switches to Edit mode so you can save it. Changing a toggle or closing the dialog discards the preview.

## File formats

A circuit is a `.circ` file. Parts are placed per view, wires belong to one view, and an optional `nets` list declares the intended connections. This is the start of [`examples/led.circ`](examples/led.circ):

```json
{
  "format": "short-circ/2",
  "title": "Battery, resistor and LED loop",
  "parts": [
    {"id":"B1","type":"battery","props":{"value":"9V"},"schematic":{"at":[10,10]},"breadboard":{"at":[14,8]}},
    {"id":"R1","type":"resistor","props":{"value":"330"},"schematic":{"at":[14,6]},"breadboard":{"at":[15,15]}},
    ...
  ],
  "wires": [
    {"id":"w1","view":"schematic","pts":[[11,10],[11,7],[14,7]]},
    {"id":"j1","view":"breadboard","pts":[[19,15],[18,15]],"color":"red"},
    ...
  ]
}
```

- Positions are integers in grid units. One unit is 0.1 inch, the breadboard hole pitch. +x is right and +y is down.
- Schematic wires connect where an end or corner lands on a pin or another wire; labels with the same name join a net. Breadboard jumpers connect only at their two ends, and parts connect through the holes of a breadboard part.
- A part with no placement in a view is not in that view.

A component is a `.part` file: logical pins plus geometry per view (schematic shapes, and optionally a breadboard SVG with pin positions on the 0.1 inch grid). See [`parts/resistor.part`](parts/resistor.part).

Older `short-circ/1` files still open; `--upgrade` rewrites them as v2. The full specification (coordinates, connectivity rules, validation rules, colours, the CLI contract) is in [`docs/FORMAT.md`](docs/FORMAT.md).

## Command line

The same `short-circ.exe` runs headless when you pass one of these options. `--view` is `schematic` or `breadboard`.

| Command | What it does |
|---|---|
| `short-circ.exe [FILE.circ]` | Open the viewer, optionally with a file. |
| `short-circ.exe --validate FILE.circ [--view V\|all] [--json]` | Check a circuit and print the report. The default `all` checks every view in use plus the cross-view rules. `--json` prints JSON. |
| `short-circ.exe --render FILE.circ OUT.svg [--view V] [--dark]` | Export an SVG image of one view (default `schematic`). `--dark` uses the dark theme. |
| `short-circ.exe --netlist FILE.circ [--view V] [--json]` | Print every net with its name, pins (such as `R1.a`) and wire ids. |
| `short-circ.exe --autoroute IN OUT [--to V] [--from-nets] [options]` | Place parts and draw wires in the target view (default `breadboard`) from the other view, or from the declared `nets` with `--from-nets`. Options: `--scope all\|unrouted`, `--only R1.a,B1.pos`, `--no-place`, `--replace`, `--crossings`, `--prefer bends\|length`, `--clearance N`, `--no-labels`, `--jumper straight\|orthogonal\|diagonal`, `--jumper-color by-net\|COLOUR`, `--max-jumper N`. |
| `short-circ.exe --upgrade IN [OUT]` | Rewrite a `.circ` or `.part` (v1 or v2) as canonical v2, in place without `OUT`. |
| `short-circ.exe --render-part TYPE OUT.svg [--view V] [--dark] [--circuit-dir DIR]` | Draw one part view with its pin ids labelled and its bounding box. `TYPE` resolves like it would in a circuit, including `dipN` and `headerN`. |
| `short-circ.exe --import-fritzing IN OUT [--views schematic,breadboard] [--type NAME]` | Convert a Fritzing part (both views by default). `IN` is a `.fzpz`, a `.fzp`, or a folder of them. `OUT` is a `.part` file, or a folder when `IN` is a folder. |
| `short-circ.exe --import-svg IN.svg OUT.part --type NAME [--pins SPEC] [--scale PX]` | Turn SVG artwork into a schematic part. `SPEC` is `id:x,y` entries separated by `;` (grid units). `PX` is pixels per grid unit (default 10). |
| `short-circ.exe --list-parts` | List the component types that can be resolved and their views. |
| `short-circ.exe --list-colors` | List the preset colour names with their hex codes. |
| `short-circ.exe --help` (or `-h`) | Show usage. |
| `short-circ.exe --version` (or `-V`) | Show the version. |

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Success. For `--validate`, no errors were found (warnings are allowed). For `--autoroute`, every net was realised. |
| 1 | `--validate` found errors, `--autoroute` could not realise some nets (the output is still written), a file could not be written or imported, or `--render-part` was given an unknown type or a view the part lacks. |
| 2 | An input file could not be read or parsed, or the arguments were invalid. |

The `.exe` is a Windows GUI program, so PowerShell may not show its output directly. Pipe it to get the text, for example:

```powershell
target\release\short-circ.exe --validate examples\led.circ | Out-String
```

A typical netlist-first session:

```powershell
$e = "target\release\short-circ.exe"
& $e --autoroute examples\nets_only.circ my.circ --from-nets --to schematic | Out-String
& $e --autoroute my.circ my.circ --to breadboard | Out-String
& $e --validate my.circ | Out-String
& $e --render my.circ my_bb.svg --view breadboard
```

## Parts and breadboards

Components are `.part` files. To add one, write the file and put it in a `parts/` folder next to your `.circ` file, or next to `short-circ.exe`. Types are looked up in this order:

1. The `lib` array inside the circuit file.
2. `*.part` (and Fritzing `*.fzpz`) files in a `parts/` folder next to the `.circ` file.
3. The same in a `parts/` folder next to `short-circ.exe`.
4. The built-in parts.

A part can have a schematic view, a breadboard view, or both; it can only be placed in the views it has. Breadboards are parts too (`breadboard_mini`, `breadboard_half`, `breadboard_full`): their pins are the holes and their strips are buses, so legs in holes of one strip are connected. Place a board in the breadboard view, put component legs exactly on holes, and use jumpers for the rest, or let `--autoroute` do it. Use `--render-part` to see a part's pin ids and check pin positions, and `--import-svg` to start a part from artwork.

## Fritzing component import

Fritzing parts can be used directly. The importer reads the part's graphics and its connector positions and snaps the pins to the 0.1 inch grid. One `.fzpz` gives **one `.part` with both views**: the schematic becomes shapes, and the breadboard image is kept as a sanitised SVG (scripts and external references are removed).

- `short-circ.exe --import-fritzing part.fzpz parts\part.part` converts one part. Pass a folder as `IN` and a folder as `OUT` to convert every `.fzpz` and `.fzp` directly inside it. `--views schematic` or `--views breadboard` imports just one view.
- File > Import Fritzing part... does the same from the viewer, and writes the result into the `parts/` folder next to the open circuit.
- A `.fzpz` file, or a `.fzp` with its SVG files, placed in a `parts/` folder is picked up automatically without converting it first.
- Warnings, for example a connector with no view or a duplicate pin name, are printed to stderr during conversion. Check the result with `--render-part TYPE out.svg --view breadboard` (and `schematic`) or the part browser.

Not handled: the PCB view, breadboard legs (bendable leads), raster images, gradients, filters and text fonts. Fritzing's core parts are licensed CC-BY-SA, so credit them if you redistribute converted parts. None are bundled.

## Colours

A colour is a hex code (`#rgb`, `#rrggbb` or `#rrggbbaa`) or one of the 18 preset names:

black, white, gray, silver, red, orange, yellow, green, lime, cyan, blue, navy, purple, magenta, pink, brown, teal, gold

Run `short-circ.exe --list-colors` to see the hex code for each name. Colours can be set on:

- wires and labels (`color`),
- parts (`color` for the outline, `fill`, and `label_color` for text),
- the whole circuit, with the `theme` object, which sets defaults for the background, grid, wires, parts, labels, pins and junctions.

The special value `none` draws nothing. `default` uses the default for that kind of object. A shape's own stroke or fill wins over the part's colour, which wins over the circuit theme, which wins over the built-in light or dark default. Explicit colours are never inverted for dark mode. See [`examples/colors.circ`](examples/colors.circ) for an example.

## Project layout

```
crates/core/                  file format, netlist, validation, autorouter and SVG export (no GUI code)
crates/app/                   the viewer, editor and command-line front end (egui)
parts/                        built-in component definitions (.part), embedded in the executable
examples/                     sample circuits (.circ)
docs/FORMAT.md                file format specification
.claude/skills/circuit/       optional Claude Code skill
```

## Optional: Claude Code skill

`.claude/skills/circuit/` contains a skill that lets [Claude Code](https://claude.com/claude-code) write, autoroute, validate and render `.circ` and `.part` files using the command-line tool. It is optional. The app never depends on it. After editing `docs/FORMAT.md`, run `powershell -File scripts\sync-skill.ps1` to refresh the specification copy inside the skill.

## Roadmap

These are ideas, not promises:

- A PCB view.
- Simulation.

## License

short-circ is released under the MIT License, as declared in the `license` field of the workspace `Cargo.toml`.
