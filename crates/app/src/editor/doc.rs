//! Documents: a new circuit and saving through the canonical writer.

use short_circ_core::write::circuit_to_string;
use short_circ_core::Circuit;
use std::path::Path;

/// An empty circuit in the current format.
pub fn new_circuit() -> Circuit {
    Circuit::default()
}

/// Write the circuit in canonical form. The file is written next to its target and renamed over
/// it, so a crash never leaves half a file.
pub fn save_circuit(path: &Path, c: &Circuit) -> Result<(), String> {
    let text = circuit_to_string(c);
    let tmp = {
        let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(".tmp");
        path.with_file_name(name)
    };
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot save {}: {e}", path.display()));
    }
    Ok(())
}

/// Display name of a document: its file name, or `untitled`.
pub fn display_name(path: Option<&Path>) -> String {
    path.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into())
}

/// Window title: `short-circ - name`, with `*` appended while there are unsaved changes.
pub fn window_title(path: Option<&Path>, dirty: bool) -> String {
    format!("short-circ - {}{}", display_name(path), if dirty { "*" } else { "" })
}
