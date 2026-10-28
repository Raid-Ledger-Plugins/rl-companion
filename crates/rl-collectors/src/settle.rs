//! Settle-watcher: decide when a save file has stopped changing.
//!
//! BG3 writes a `.lsv` in several steps; reading it mid-write yields a
//! truncated package. The companion polls `(size, mtime)` and reads only once
//! the same non-empty sample has been seen `stable_samples` times in a row.
//! The crate never touches the filesystem; the caller supplies the samples.

/// One poll of a file. `None` in the APIs below means "file missing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSample {
    pub size: u64,
    pub mtime_ms: i64,
}

/// Default number of identical consecutive samples that count as settled.
pub const DEFAULT_STABLE_SAMPLES: usize = 3;

/// True when the last `stable_samples` (at least 2) samples are all present,
/// identical and non-empty.
pub fn is_settled(samples: &[Option<FileSample>], stable_samples: usize) -> bool {
    let need = stable_samples.max(2);
    let Some(start) = samples.len().checked_sub(need) else {
        return false;
    };
    let tail = &samples[start..];
    match tail.first().copied().flatten() {
        Some(first) if first.size > 0 => tail.iter().all(|s| *s == Some(first)),
        _ => false,
    }
}

/// Streaming form of [`is_settled`]: feed one sample per poll.
#[derive(Debug, Clone)]
pub struct SettleWatcher {
    stable_samples: usize,
    last: Option<FileSample>,
    run: usize,
    reported: bool,
}

impl Default for SettleWatcher {
    fn default() -> Self {
        Self::new(DEFAULT_STABLE_SAMPLES)
    }
}

impl SettleWatcher {
    /// `stable_samples` is clamped to at least 2.
    pub fn new(stable_samples: usize) -> Self {
        Self {
            stable_samples: stable_samples.max(2),
            last: None,
            run: 0,
            reported: false,
        }
    }

    /// Record one poll. Returns `true` exactly once per settled version of
    /// the file; any change (size, mtime, missing, empty) re-arms it.
    pub fn observe(&mut self, sample: Option<FileSample>) -> bool {
        match sample {
            Some(s) if s.size > 0 && self.last == Some(s) => self.run += 1,
            Some(s) if s.size > 0 => {
                self.last = Some(s);
                self.run = 1;
                self.reported = false;
            }
            _ => {
                self.last = None;
                self.run = 0;
                self.reported = false;
            }
        }
        if !self.reported && self.run >= self.stable_samples {
            self.reported = true;
            return true;
        }
        false
    }
}
