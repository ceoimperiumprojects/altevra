//! Per-path debouncer. Coalesces rapid-fire events on the same path so the
//! emitter doesn't fire 50 times when an editor rewrites a file atomically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Debouncer {
    window: Duration,
    /// Path → time when the path was first touched in the current window.
    pending: HashMap<PathBuf, Instant>,
}

impl Debouncer {
    pub fn new(window_ms: u64) -> Self {
        Self {
            window: Duration::from_millis(window_ms),
            pending: HashMap::new(),
        }
    }

    /// Record a touch on `path`. Subsequent touches before the window
    /// elapses are coalesced.
    pub fn touch(&mut self, path: &Path) {
        self.pending
            .entry(path.to_path_buf())
            .or_insert_with(Instant::now);
    }

    /// Drain paths whose debounce window has elapsed.
    pub fn drain_ready(&mut self) -> Vec<PathBuf> {
        let now = Instant::now();
        let ready: Vec<PathBuf> = self
            .pending
            .iter()
            .filter_map(|(p, t)| {
                if now.duration_since(*t) >= self.window {
                    Some(p.clone())
                } else {
                    None
                }
            })
            .collect();
        for p in &ready {
            self.pending.remove(p);
        }
        ready
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn coalesces_rapid_touches() {
        let mut d = Debouncer::new(100);
        let p = Path::new("/tmp/x");
        d.touch(p);
        d.touch(p);
        d.touch(p);
        assert_eq!(d.pending_count(), 1);
        assert_eq!(d.drain_ready().len(), 0); // not yet ready
    }

    #[test]
    fn drains_after_window_elapses() {
        let mut d = Debouncer::new(20);
        d.touch(Path::new("/tmp/a"));
        sleep(Duration::from_millis(30));
        let ready = d.drain_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(d.pending_count(), 0);
    }

    #[test]
    fn drain_is_idempotent() {
        let mut d = Debouncer::new(10);
        d.touch(Path::new("/tmp/b"));
        sleep(Duration::from_millis(20));
        assert_eq!(d.drain_ready().len(), 1);
        assert_eq!(d.drain_ready().len(), 0);
    }
}
