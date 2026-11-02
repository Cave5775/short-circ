//! Component resolution, per docs/FORMAT.md. Order: circuit `lib` > `<circuit dir>/parts`
//! > `<exe dir>/parts` > built-ins (embedded, then generated families).

use crate::builtin;
use crate::model::{Circuit, PartDef, PartLookup};
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
    /// Unreadable or invalid external .part files are skipped; later sources never override
    /// earlier ones.
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
                // TODO(fzp stream): re-enable the on-the-fly import of Fritzing `.fzpz` / `.fzp`
                // files found in `parts/` once `fzp` speaks format v2 (docs/FORMAT.md "Importing
                // Fritzing parts": default `--views` both; a `.part` or an earlier source with the
                // same type wins; files that fail to import are ignored).
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

impl PartLookup for Library {
    fn get(&self, ty: &str) -> Option<&PartDef> {
        self.defs.get(ty)
    }
}

/// A plain map of definitions is a lookup too (handy in tests and tools).
impl PartLookup for BTreeMap<String, PartDef> {
    fn get(&self, ty: &str) -> Option<&PartDef> {
        BTreeMap::get(self, ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::View;

    #[test]
    fn builtin_has_resistor() {
        let l = Library::builtin();
        assert_eq!(l.get("resistor").unwrap().pins.len(), 2);
        assert!(l.get("resistor").unwrap().view(View::Schematic).is_some());
    }

    #[test]
    fn inline_lib_wins() {
        let c = crate::parse_circuit(
            r#"{"format":"short-circ/1","lib":[{"type":"resistor","size":[1,1]}]}"#,
        )
        .unwrap();
        let l = Library::load(&c, None);
        assert_eq!(l.get("resistor").unwrap().schematic.as_ref().unwrap().size, [1.0, 1.0]);
    }

    #[test]
    fn generated_families_resolve_only_when_referenced() {
        let c = crate::parse_circuit(
            r#"{"format":"short-circ/2","parts":[{"id":"U1","type":"dip8","schematic":{"at":[0,0]}}]}"#,
        )
        .unwrap();
        let l = Library::load(&c, None);
        assert_eq!(l.get("dip8").unwrap().pins.len(), 8);
        assert!(l.get("dip14").is_none());
    }

    #[test]
    fn library_and_map_are_part_lookups() {
        let l = Library::builtin();
        let lookup: &dyn PartLookup = &l;
        assert!(lookup.get("resistor").is_some() && lookup.get("zzz").is_none());
        let mut m = BTreeMap::new();
        m.insert("resistor".to_string(), l.get("resistor").unwrap().clone());
        let lookup: &dyn PartLookup = &m;
        assert!(lookup.get("resistor").is_some() && lookup.get("led").is_none());
    }
}
