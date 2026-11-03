//! The companion's own log for `/dev/v1/logs` (AC-7): a ring buffer whose
//! lines are redacted BEFORE they are stored, with the contract's patterns
//! (`(?i)bearer\s+\S+`, `(?i)rlcd(ev)?_…`) plus user-home paths and
//! `secret=`-style pairs.

use super::paging::clip;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

pub const CAPACITY: usize = 1000;
const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

struct Line {
    seq: usize,
    at: String,
    level: &'static str,
    target: String,
    msg: String,
}

#[derive(Default)]
pub struct LogBuffer {
    inner: Mutex<(usize, VecDeque<Line>)>,
}

fn patterns() -> &'static [(Regex, &'static str); 4] {
    static P: OnceLock<[(Regex, &'static str); 4]> = OnceLock::new();
    P.get_or_init(|| {
        let re = |s: &str| Regex::new(s).unwrap_or_else(|_| unreachable!("static pattern"));
        [
            (re(r"(?i)rlcd(ev)?_[A-Za-z0-9_-]*"), "<redacted>"),
            (re(r"(?i)(bearer)\s+\S+"), "$1 <redacted>"),
            (
                re(r"(?i)\b(token|secret|password|passwd|api[_-]?key)(\s*[=:]\s*)[^\s,;&]+"),
                "$1$2<redacted>",
            ),
            // `/Users/<name>`, `C:\Users\<name>`, `/home/<name>`.
            (re(r"(?i)([/\\](?:users|home)[/\\])[^/\\\s]+"), "$1<user>"),
        ]
    })
}

/// The redacted form of `raw` (what every stored line goes through).
pub fn redact(raw: &str) -> String {
    let mut out = raw.to_owned();
    for (re, with) in patterns() {
        out = re.replace_all(&out, *with).into_owned();
    }
    out
}

fn level_rank(level: &str) -> Option<usize> {
    LEVELS.iter().position(|l| *l == level)
}

impl LogBuffer {
    /// Store one line (redacted, clipped to the schema's caps).
    pub fn push(&self, level: &'static str, target: &str, msg: &str, at: String) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let seq = g.0;
        g.0 += 1;
        if g.1.len() == CAPACITY {
            g.1.pop_front();
        }
        g.1.push_back(Line {
            seq,
            at,
            level,
            target: clip(&redact(target), 128),
            msg: clip(&redact(msg), 2048),
        });
    }

    /// Lines with `seq ≥ from`, at least as severe as `level`, at or after
    /// `since` (RFC 3339 compares as text), oldest first, as `(seq, line)`.
    pub fn select(
        &self,
        from: usize,
        level: Option<&str>,
        since: Option<&str>,
    ) -> Vec<(usize, Value)> {
        let max = level.and_then(level_rank).unwrap_or(LEVELS.len() - 1);
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.1.iter()
            .filter(|l| l.seq >= from)
            .filter(|l| level_rank(l.level).is_some_and(|r| r <= max))
            .filter(|l| since.is_none_or(|s| l.at.as_str() >= s))
            .map(|l| {
                let v = json!({ "at": l.at, "level": l.level, "target": l.target, "msg": l.msg });
                (l.seq, v)
            })
            .collect()
    }
}

pub fn is_level(s: &str) -> bool {
    level_rank(s).is_some()
}

/// The process-wide buffer.
pub fn global() -> &'static LogBuffer {
    static B: OnceLock<LogBuffer> = OnceLock::new();
    B.get_or_init(LogBuffer::default)
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    rl_collectors::rfc3339_utc_from_unix_seconds(secs)
}

/// Log a line: stored redacted, and echoed (redacted) to stderr.
pub fn log(level: &'static str, target: &str, msg: &str) {
    eprintln!("RL Companion {target}: {}", redact(msg));
    global().push(level, target, msg, now());
}
