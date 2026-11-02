//! Recent files list, stored in a small text file next to the executable.
//! Every I/O failure is ignored on purpose: the list is a convenience only.

use std::path::{Path, PathBuf};

const FILE_NAME: &str = "short-circ-recent.txt";
pub const MAX_RECENT: usize = 10;

#[derive(Debug, Default)]
pub struct Recent {
    store: Option<PathBuf>,
    pub items: Vec<PathBuf>,
}

impl Recent {
    /// Load from the file next to the exe (empty if missing or unreadable).
    pub fn load() -> Self {
        let store = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(FILE_NAME)));
        Self::load_from(store)
    }

    pub fn load_from(store: Option<PathBuf>) -> Self {
        let items = store
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| parse(&s))
            .unwrap_or_default();
        Recent { store, items }
    }

    /// Move `path` to the front and persist (best effort).
    pub fn add(&mut self, path: &Path) {
        push_front(&mut self.items, tidy(path), MAX_RECENT);
        self.save();
    }

    pub fn save(&self) {
        if let Some(p) = &self.store {
            let _ = std::fs::write(p, serialize(&self.items));
        }
    }
}

/// Absolute path without the Windows verbatim prefix.
fn tidy(path: &Path) -> PathBuf {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

pub fn parse(text: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let p = PathBuf::from(line);
        if !out.contains(&p) && out.len() < MAX_RECENT {
            out.push(p);
        }
    }
    out
}

pub fn serialize(items: &[PathBuf]) -> String {
    items.iter().map(|p| format!("{}\n", p.display())).collect()
}

/// Put `p` at the front, removing any earlier occurrence, keeping at most `max` entries.
pub fn push_front(items: &mut Vec<PathBuf>, p: PathBuf, max: usize) {
    items.retain(|x| *x != p);
    items.insert(0, p);
    items.truncate(max);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_dedups_and_truncates() {
        let mut v: Vec<PathBuf> = vec!["a".into(), "b".into(), "c".into()];
        push_front(&mut v, "b".into(), 3);
        assert_eq!(v, vec![PathBuf::from("b"), "a".into(), "c".into()]);
        push_front(&mut v, "d".into(), 3);
        assert_eq!(v, vec![PathBuf::from("d"), "b".into(), "a".into()]);
    }

    #[test]
    fn roundtrip_text() {
        let v: Vec<PathBuf> = vec!["x.circ".into(), "y z.circ".into()];
        assert_eq!(parse(&serialize(&v)), v);
        assert_eq!(parse("\n  \na\na\n"), vec![PathBuf::from("a")]);
    }

    #[test]
    fn missing_store_is_silent() {
        let mut r = Recent::load_from(Some(PathBuf::from("Z:/definitely/not/here/recent.txt")));
        assert!(r.items.is_empty());
        r.add(Path::new("foo.circ"));
        assert_eq!(r.items.len(), 1);
    }
}
