# short-circ

short-circ is a small, portable Windows viewer for circuit diagrams. Circuits are plain JSON files (`.circ`) that are easy to read, diff and generate. The viewer draws them in a Fritzing-like style, checks them for common wiring mistakes, and exports them to SVG.

It is a single static `.exe` built with Rust and [egui](https://github.com/emilk/egui). There is no installer, no runtime and no web view. The same executable also runs as a headless command-line tool.

Phase 1 is the viewer and checker. A builder/editor is planned for later (see [Roadmap](#roadmap)).

## Features

- **Viewer**: pan, zoom, fit to window, hover pins and wires to see their names and nets, click a part for its details, live reload when the file changes.
- **Net highlighting**: click a pin or wire to lock its net highlight, or pick a net in the nets panel.
- **Validator**: finds overlapping wires, crossings without a junction, dangling wires, unconnected pins, overlapping parts, bad colours and more.
- **SVG export**: from the File menu, or with `--render` on the command line.
- **Command line**: `--validate` with exit codes for scripts and CI, plus `--render`, `--netlist`, `--render-part`, `--list-parts`, `--list-colors`, `--import-svg` and `--import-fritzing`.
- **Built-in parts**: 23 component types, plus generated DIP packages (`dipN`) and pin headers (`headerN`), and a part browser to inspect them.
- **Fritzing import**: convert Fritzing `.fzpz` and `.fzp` parts into short-circ parts. See [Fritzing component import](#fritzing-component-import).
- **Colours**: hex codes, preset names and circuit-wide theme defaults. See [Colours](#colours).

## Quick start

1. Build the executable as described in [Building from source](#building-from-source). The result is one file, `target\release\short-circ.exe`, which you can copy anywhere.
2. Open the example circuit from the repository root:

   ```bat
   target\release\short-circ.exe examples\led.circ
   ```

   You can also start the viewer without a file and open one from the File menu.

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
- The View menu toggles the grid, the dark theme (on by default), the issues panel, the nets panel and the part browser.
- View > Labels shows or hides part references, part values, wire ids and pin names. Text that would be smaller than 7 pixels is hidden automatically.
- Shortcuts: Ctrl+O open, F fit, R reload now, G toggle the grid, Esc clear the selection. Help > Shortcuts lists them.

**Hovering and clicking**

- Hover a pin to see its name and net. The wires and pins of that net are highlighted.
- Hover a wire to see its id and net, with the same highlighting.
- Click a part to show its details on the right: type, position, rotation, properties, colours and pins with their nets.
- Click a pin or wire to lock the highlight on its net. Click empty space or press Esc to clear the selection and the lock.
- View > Nets lists every net with its name and pins. Click one to highlight it and zoom to fit it.
- The viewer reloads the circuit automatically when the file, or the `parts/` folder beside it, changes (View > Auto-reload, on by default). If the new file is broken, the old circuit stays on screen and an error banner appears.
- View > Part browser lists every part type that can be resolved, with a preview that labels each pin. It is useful for checking an imported component.
- The issues panel at the bottom lists the problems the validator found. Click an issue to move the view to it.
- File > Export SVG... saves the current circuit as an SVG image.

## File format

A circuit is a `.circ` file. This is [`examples/led.circ`](examples/led.circ):

```json
{
  "format": "short-circ/1",
  "title": "Battery, resistor and LED loop",
  "parts": [
    {"id": "B1", "type": "battery", "at": [10, 10], "props": {"value": "9V"}},
    {"id": "R1", "type": "resistor", "at": [14, 6], "props": {"value": "330"}},
    {"id": "D1", "type": "led", "at": [26, 6], "rot": 90, "props": {"value": "red"}}
  ],
  "wires": [
    {"id": "w1", "pts": [[11, 10], [11, 7], [14, 7]]},
    {"id": "w2", "pts": [[22, 7], [25, 7], [25, 6]]},
    {"id": "w3", "pts": [[25, 10], [25, 16], [11, 16], [11, 14]]}
  ],
  "labels": [{"at": [11, 16], "name": "GND"}]
}
```

- Positions are integers in grid units. One unit is 0.1 inch. +x is right and +y is down.
- Wires are polylines of points. Wires connect where an end or corner lands on a pin or another wire, including T-junctions. Two wires that only cross do not connect.
- Labels with the same name join the same net, so you can use them instead of long wires.

Components are `.part` files. This is [`parts/resistor.part`](parts/resistor.part):

```json
{
  "format": "short-circ/1",
  "type": "resistor",
  "name": "Resistor",
  "category": "passive",
  "size": [8, 2],
  "pins": [
    {"id": "a", "at": [0, 1], "dir": "left"},
    {"id": "b", "at": [8, 1], "dir": "right"}
  ],
  "shapes": [
    {"k": "line", "pts": [[0, 1], [2, 1]]},
    {"k": "rect", "at": [2, 0], "size": [4, 2]},
    {"k": "line", "pts": [[6, 1], [8, 1]]},
    {"k": "text", "at": [4, -0.6], "text": "{ref}", "size": 0.9, "anchor": "middle"},
    {"k": "text", "at": [4, 3.1], "text": "{value}", "size": 0.9, "anchor": "middle"}
  ]
}
```

The full specification, including connectivity rules, validation rules, colour precedence and the CLI contract, is in [`docs/FORMAT.md`](docs/FORMAT.md).

## Command line

The same `short-circ.exe` runs headless when you pass one of these options.

| Command | What it does |
|---|---|
| `short-circ.exe [FILE.circ]` | Open the viewer, optionally with a file. |
| `short-circ.exe --validate FILE.circ [--json]` | Check a circuit and print the report. `--json` prints the report as JSON. |
| `short-circ.exe --render FILE.circ OUT.svg [--dark]` | Export an SVG image. `--dark` renders with the dark theme. |
| `short-circ.exe --import-svg IN.svg OUT.part --type NAME [--pins SPEC] [--scale PX]` | Turn SVG artwork into a `.part` file. `SPEC` is `id:x,y` entries separated by `;` (grid units). `PX` is pixels per grid unit (default 10). |
| `short-circ.exe --import-fritzing IN OUT [--view schematic\|breadboard] [--type NAME]` | Convert a Fritzing part. `IN` is a `.fzpz`, a `.fzp`, or a folder of them. `OUT` is a `.part` file, or a folder when `IN` is a folder. |
| `short-circ.exe --netlist FILE.circ [--json]` | Print every net with its name, pins (such as `R1.a`) and wire ids. |
| `short-circ.exe --render-part TYPE OUT.svg [--dark] [--circuit-dir DIR]` | Draw one part with its pin ids labelled and its bounding box. `TYPE` resolves like it would in a circuit, including `dipN` and `headerN`. |
| `short-circ.exe --list-parts` | List the component types that can be resolved. |
| `short-circ.exe --list-colors` | List the preset colour names with their hex codes. |
| `short-circ.exe --help` (or `-h`) | Show usage. |
| `short-circ.exe --version` (or `-V`) | Show the version. |

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Success. For `--validate`, no errors were found (warnings are allowed). |
| 1 | `--validate` found errors, an SVG or part file could not be written or imported, or `--render-part` was given an unknown type. |
| 2 | An input file could not be read or parsed, or the arguments were invalid. |

The `.exe` is a Windows GUI program, so PowerShell may not show its output directly. Pipe it to get the text, for example:

```powershell
target\release\short-circ.exe --validate examples\led.circ | Out-String
```

## Custom parts

To add a component, write a `.part` file and put it in a `parts/` folder next to your `.circ` file, or next to `short-circ.exe`. Types are looked up in this order:

1. The `lib` array inside the circuit file.
2. `*.part` files in a `parts/` folder next to the `.circ` file.
3. `*.part` files in a `parts/` folder next to `short-circ.exe`.
4. The built-in parts.

You can also create a part from SVG artwork with `--import-svg`, described in the command line table above.

## Fritzing component import

Fritzing parts can be used directly. The importer reads the part's graphics and its connector positions, and snaps the pins to the 0.1 inch grid.

- `short-circ.exe --import-fritzing part.fzpz parts\part.part` converts one part. Pass a folder as `IN` and a folder as `OUT` to convert every `.fzpz` and `.fzp` directly inside it.
- The schematic view is used by default. `--view breadboard` uses the breadboard artwork instead.
- File > Import Fritzing part... does the same from the viewer, and writes the result into the `parts/` folder next to the open circuit.
- A `.fzpz` file, or a `.fzp` with its SVG files, placed in a `parts/` folder is picked up automatically without converting it first.
- Warnings, for example a connector with no schematic view or a duplicate pin name, are printed to stderr during conversion. Check the result with `--render-part TYPE out.svg` or the part browser.

Not handled: the PCB view, breadboard legs, raster images, gradients, filters and text fonts. Fritzing's core parts are licensed CC-BY-SA, so credit them if you redistribute converted parts.

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
crates/core/                  file format, netlist, validation and SVG export (no GUI code)
crates/app/                   the viewer and the command-line front end (egui)
parts/                        built-in component definitions (.part), embedded in the executable
examples/                     sample circuits (.circ)
docs/FORMAT.md                file format specification
.claude/skills/circuit/       optional Claude Code skill
```

## Optional: Claude Code skill

`.claude/skills/circuit/` contains a skill that lets [Claude Code](https://claude.com/claude-code) write, validate and render `.circ` and `.part` files using the command-line tool. It is optional. The app never depends on it.

## Roadmap

These are ideas, not promises:

- An editing UI for building circuits.
- Breadboard and PCB views.
- Autorouting for wires.
- Simulation.

## License

short-circ is released under the MIT License, as declared in the `license` field of the workspace `Cargo.toml`.
