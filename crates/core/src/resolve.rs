//! Component resolution, per docs/FORMAT.md. Order: circuit `lib` > `<circuit dir>/parts`
//! > `<exe dir>/parts` > built-ins (embedded, then generated families).

use crate::builtin;
use crate::model::{Circuit, PartDef};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct Library {
    defs: BTreeMap<String, PartDef>,
}

impl Library {
    /// Built-ins only.
    pub fn builtin() -> Self {
        let mut defs = BTreeMap::new();
        for (name, src) in builtin::BUILTIN {
            match crate::parse_part(src) {
                Ok(d) => {
                    defs.insert(d.ty.clone(), d);
                }
                Err(e) => panic!("built-in part {name} is invalid: {e}"),
            }
        }
        Library { defs }
    }

    /// Full resolution for a circuit. `circuit_dir` is the directory of the .circ file if known.
    /// Unreadable or invalid external .part files are skipped (reported via `load_errors`
    /// is not needed in v1); later sources never override earlier ones.
    pub fn load(circuit: &Circuit, circuit_dir: Option<&Path>) -> Self {
        let mut lib = Library::default();
        for d in &circuit.lib {
            lib.defs.entry(d.ty.clone()).or_insert_with(|| d.clone());
        }
        if let Some(dir) = circuit_dir {
            lib.add_dir(&dir.join("parts"));
        }
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.join("parts"))) {
            lib.add_dir(&dir);
        }
        for (k, v) in Library::builtin().defs {
            lib.defs.entry(k).or_insert(v);
        }
        // Generated families (dipN, headerN) for any referenced type not yet defined.
        for p in &circuit.parts {
            if !lib.defs.contains_key(&p.ty) {
                if let Some(d) = builtin::generate(&p.ty) {
                    lib.defs.insert(p.ty.clone(), d);
                }
            }
        }
        lib
    }

    fn add_dir(&mut self, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
            let def = match ext.as_str() {
                "part" => std::fs::read_to_string(&p).ok().and_then(|src| crate::parse_part(&src).ok()),
                // Fritzing parts are imported on the fly with schematic defaults.
                "fzpz" | "fzp" => crate::fzp::import_path(&p, &crate::fzp::ImportOptions::default()).ok().map(|r| r.0),
                _ => None,
            };
            if let Some(d) = def {
                self.defs.entry(d.ty.clone()).or_insert(d);
            }
        }
    }

    pub fn get(&self, ty: &str) -> Option<&PartDef> {
        self.defs.get(ty)
    }

    pub fn types(&self) -> impl Iterator<Item = &str> {
        self.defs.keys().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_resistor() {
        let l = Library::builtin();
        assert_eq!(l.get("resistor").unwrap().pins.len(), 2);
    }

    #[test]
    fn inline_lib_wins() {
        let c = crate::parse_circuit(
            r#"{"format":"short-circ/1","lib":[{"type":"resistor","size":[1,1]}]}"#,
        )
        .unwrap();
        let l = Library::load(&c, None);
        assert_eq!(l.get("resistor").unwrap().size, [1.0, 1.0]);
    }
}
