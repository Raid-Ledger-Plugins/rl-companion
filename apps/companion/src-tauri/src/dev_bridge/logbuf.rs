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
            // `/Users/<name>`, `C:\Users\<name>`, `/home/<name>`, also debug-escaped
            // (`C:\\Users\\<name>`). A name may hold spaces when a separator
            // follows it (`C:\Users\Jake Doe\x`); otherwise it ends at whitespace.
            (
                re(r"(?i)([/\\]{1,2}(?:users|home)[/\\]{1,2})(?:[^/\\\s]+(?: [^/\\\s]+)*([/\\])|[^/\\\s]+)"),
                "$1<user>$2",
            ),
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
        since: Option<(i64, u32)>,
    ) -> Vec<(usize, Value)> {
        let max = level.and_then(level_rank).unwrap_or(LEVELS.len() - 1);
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.1.iter()
            .filter(|l| l.seq >= from)
            .filter(|l| level_rank(l.level).is_some_and(|r| r <= max))
            .filter(|l| since.is_none_or(|s| parse_utc(&l.at).is_some_and(|at| at >= s)))
            .map(|l| {
                let v = json!({ "at": l.at, "level": l.level, "target": l.target, "msg": l.msg });
                (l.seq, v)
            })
            .collect()
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` → `(unix seconds, nanos)`, the order key for
/// `?since=` (a text compare misorders `…:00Z` vs `…:00.5Z`). `None` = not RFC 3339 UTC.
pub fn parse_utc(s: &str) -> Option<(i64, u32)> {
    let b = s.as_bytes();
    let sep = |i: usize, c: u8| b.get(i) == Some(&c);
    if s.len() > 30
        || !(sep(4, b'-') && sep(7, b'-') && sep(10, b'T') && sep(13, b':') && sep(16, b':'))
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = s.get(19..)?.strip_suffix('Z')?;
    let nanos = match rest.strip_prefix('.') {
        None if rest.is_empty() => 0,
        Some(f) if (1..=9).contains(&f.len()) && f.bytes().all(|c| c.is_ascii_digit()) => {
            format!("{f:0<9}").parse().ok()?
        }
        _ => return None,
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let max_d = *dim.get(usize::try_from(mo - 1).ok()?)?;
    if !(1..=max_d).contains(&d) || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let (yy, m) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some((days * 86_400 + h * 3600 + mi * 60 + se, nanos))
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
