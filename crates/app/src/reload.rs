//! Live reload: cheap change detection for the open circuit file and its `parts/` folder.
//! The decision logic is pure so it can be unit tested; the app polls [`stamp`] about
//! every [`POLL_INTERVAL`] seconds using egui's repaint timer (no threads).

use std::path::Path;
use std::time::SystemTime;

/// Seconds between polls of the file system.
pub const POLL_INTERVAL: f64 = 0.4;

/// What the file system looked like at one poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stamp {
    /// Modified time and length of the `.circ` file, `None` if it cannot be read.
    pub file: Option<(SystemTime, u64)>,
    /// Newest modified time among the `parts/` folder and its `.part` files.
    pub parts: Option<SystemTime>,
}

/// Take a stamp of `circuit` and of the `parts/` folder next to it.
pub fn stamp(circuit: &Path) -> Stamp {
    let file = std::fs::metadata(circuit).ok().and_then(|m| m.modified().ok().map(|t| (t, m.len())));
    let parts = circuit.parent().and_then(|d| newest_mtime(&d.join("parts")));
    Stamp { file, parts }
}

/// Newest modification time of `dir` itself (covers added and removed files) and of the
/// `.part` files in it. `None` if the folder does not exist.
pub fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(dir).ok()?.modified().ok();
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("part") {
            continue;
        }
        if let Some(t) = e.metadata().ok().and_then(|m| m.modified().ok()) {
            newest = Some(newest.map_or(t, |n| n.max(t)));
        }
    }
    newest
}

/// True when it is time for the next poll (`now` and `last` are egui clock seconds).
pub fn poll_due(now: f64, last: f64) -> bool {
    now - last >= POLL_INTERVAL || now < last
}

/// Whether the app should reload. A file that is currently unreadable (editors that save by
/// replacing the file leave a brief gap) never triggers a reload; the next poll will see it.
pub fn should_reload(prev: &Stamp, cur: &Stamp) -> bool {
    cur.file.is_some() && prev != cur
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn st(file: Option<(u64, u64)>, parts: Option<u64>) -> Stamp {
        Stamp { file: file.map(|(m, l)| (t(m), l)), parts: parts.map(t) }
    }

    #[test]
    fn unchanged_does_not_reload() {
        let a = st(Some((10, 100)), Some(5));
        assert!(!should_reload(&a, &a));
    }

    #[test]
    fn file_mtime_or_length_change_reloads() {
        let a = st(Some((10, 100)), None);
        assert!(should_reload(&a, &st(Some((11, 100)), None)));
        assert!(should_reload(&a, &st(Some((10, 101)), None)));
    }

    #[test]
    fn parts_folder_change_reloads() {
        let a = st(Some((10, 100)), Some(5));
        assert!(should_reload(&a, &st(Some((10, 100)), Some(6))));
        assert!(should_reload(&a, &st(Some((10, 100)), None)), "parts folder removed");
        assert!(should_reload(&st(Some((10, 100)), None), &st(Some((10, 100)), Some(1))), "parts folder added");
    }

    #[test]
    fn missing_file_waits() {
        let a = st(Some((10, 100)), None);
        assert!(!should_reload(&a, &st(None, None)));
        // ...and when it comes back with a new time, it reloads.
        assert!(should_reload(&st(None, None), &st(Some((12, 100)), None)));
    }

    #[test]
    fn poll_timer() {
        assert!(!poll_due(1.1, 1.0));
        assert!(poll_due(1.45, 1.0));
        assert!(poll_due(5.0, 1.0));
        assert!(poll_due(0.5, 1.0), "clock went backwards: poll now");
    }

    #[test]
    fn stamp_follows_real_files() {
        let dir = std::env::temp_dir().join(format!("short-circ-reload-test-{}", std::process::id()));
        let parts = dir.join("parts");
        std::fs::create_dir_all(&parts).unwrap();
        let circ = dir.join("a.circ");
        std::fs::write(&circ, "{}").unwrap();
        let s1 = stamp(&circ);
        assert!(s1.file.is_some() && s1.parts.is_some());
        assert!(!should_reload(&s1, &stamp(&circ)));
        // A longer file is a change even if the clock tick is the same.
        std::fs::write(&circ, "{ }").unwrap();
        assert!(should_reload(&s1, &stamp(&circ)));
        // A new .part file moves the parts stamp (or at least the directory mtime).
        let s2 = stamp(&circ);
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(parts.join("x.part"), "{}").unwrap();
        let s3 = stamp(&circ);
        assert!(s3.parts >= s2.parts);
        assert!(stamp(&dir.join("missing.circ")).file.is_none());
        assert!(newest_mtime(&dir.join("nope")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
