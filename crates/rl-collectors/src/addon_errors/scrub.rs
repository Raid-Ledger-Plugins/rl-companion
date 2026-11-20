//! Contract §4 scrubbing: user home folders → `<home>\`, WoW install roots →
//! `<wow>\`, the account's own character / realm names → `<char>` /
//! `<realm>`. [`has_unscrubbed_path`] mirrors the schema's refusal
//! (`WOW_ADDON_ERRORS_UNSCRUBBED_PATH_RE`).

use crate::wow_sv::is_flavor_folder;

const HOME: &str = "<home>\\";
const WOW: &str = "<wow>\\";
const DRIVE: &str = "<drive>";
/// Longest absolute path looked back over for a path start (Windows MAX_PATH).
const MAX_PATH_LOOKBACK: usize = 260;

fn is_sep(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

/// `[A-Za-z]:[\\/]` at `p`, not preceded by an ASCII letter or digit.
fn drive_at(s: &[u8], p: usize) -> bool {
    p + 2 < s.len()
        && s[p].is_ascii_alphabetic()
        && s[p + 1] == b':'
        && is_sep(s[p + 2])
        && (p == 0 || !s[p - 1].is_ascii_alphanumeric())
}

/// `[\\/]users[\\/]` (any case) starting at `p`.
fn users_at(s: &[u8], p: usize) -> bool {
    p + 6 < s.len()
        && is_sep(s[p])
        && s[p + 1..p + 6].eq_ignore_ascii_case(b"users")
        && is_sep(s[p + 6])
}

/// The schema's `not.pattern`: true when the text would be refused.
pub fn has_unscrubbed_path(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|p| drive_at(b, p) || users_at(b, p))
}

/// Where the absolute path that contains byte `i` starts: the last drive
/// letter on the line within [`MAX_PATH_LOOKBACK`], else the last `/` that
/// follows whitespace / a quote / a bracket, else `i` (a relative path).
fn path_start(b: &[u8], floor: usize, i: usize) -> usize {
    let line = b[floor..i]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(floor, |p| floor + p + 1);
    let low = line.max(i.saturating_sub(MAX_PATH_LOOKBACK));
    if let Some(p) = (low..i).rev().find(|&p| drive_at(b, p)) {
        return p;
    }
    let opener = |p: usize| {
        p == line || matches!(b[p - 1], b' ' | b'\t' | b'"' | b'\'' | b'(' | b'[' | b'=')
    };
    (low..i).find(|&p| b[p] == b'/' && opener(p)).unwrap_or(i)
}

/// `World of Warcraft<sep>_<flavor>_<sep>` at `i`: the end of the match.
fn wow_root_end(b: &[u8], i: usize) -> Option<usize> {
    const NAME: &[u8] = b"world of warcraft";
    let after = i + NAME.len();
    if after >= b.len() || !b[i..after].eq_ignore_ascii_case(NAME) || !is_sep(b[after]) {
        return None;
    }
    let rest = &b[after + 1..];
    let len = rest.iter().position(|&c| is_sep(c))?;
    let flavor = std::str::from_utf8(&rest[..len]).ok()?;
    is_flavor_folder(flavor).then_some(after + 1 + len + 1)
}

/// A WoW root later on the drive path starting at `p` (same line, within
/// [`MAX_PATH_LOOKBACK`], before any other drive letter): its end.
fn wow_root_after_drive(b: &[u8], p: usize) -> Option<usize> {
    let limit = b.len().min(p + MAX_PATH_LOOKBACK);
    for j in p + 3..limit {
        if b[j] == b'\n' || drive_at(b, j) {
            return None;
        }
        if let Some(end) = wow_root_end(b, j) {
            return Some(end);
        }
    }
    None
}

/// `[drive:]<sep>Users<sep><name>[<sep>]` at the `<sep>` `i`: (start, end).
fn home_span(b: &[u8], i: usize) -> (usize, usize) {
    let start = if i >= 2 && drive_at(b, i - 2) {
        i - 2
    } else {
        i
    };
    let name = i + 7;
    let stop = |c: u8| is_sep(c) || matches!(c, b'\n' | b'\r' | b'"' | b'\'' | b':' | b' ');
    let len = b[name..]
        .iter()
        .position(|&c| stop(c))
        .unwrap_or(b.len() - name);
    let end = name + len;
    let end = if end < b.len() && is_sep(b[end]) {
        end + 1
    } else {
        end
    };
    (start, end)
}

/// Replace WoW install roots, then home folders, then any leftover drive
/// prefix, so the result never matches [`has_unscrubbed_path`].
pub fn scrub_paths(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut done, mut i) = (0, 0);
    while i < b.len() {
        if let Some(end) = wow_root_end(b, i) {
            let start = path_start(b, done, i);
            out.push_str(&s[done..start]);
            out.push_str(WOW);
            (done, i) = (end, end);
        } else if users_at(b, i) {
            let (start, end) = home_span(b, i);
            out.push_str(&s[done..start]);
            out.push_str(HOME);
            (done, i) = (end, end);
        } else if drive_at(b, i) {
            out.push_str(&s[done..i]);
            let (repl, end) = if users_at(b, i + 2) {
                (HOME, home_span(b, i + 2).1)
            } else if let Some(end) = wow_root_after_drive(b, i) {
                (WOW, end)
            } else {
                (DRIVE, i + 2)
            };
            out.push_str(repl);
            (done, i) = (end, end);
        } else {
            i += 1;
        }
    }
    out.push_str(&s[done..]);
    out
}

fn is_word_char(c: Option<char>) -> bool {
    c.is_some_and(char::is_alphanumeric)
}

/// Replace whole-word, ASCII-case-insensitive occurrences of `token`.
fn replace_word(s: &str, token: &str, repl: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let end = i + token.len();
        let hit = end <= s.len()
            && s.is_char_boundary(end)
            && s[i..end].eq_ignore_ascii_case(token)
            && !is_word_char(s[..i].chars().next_back())
            && !is_word_char(s[end..].chars().next());
        if hit {
            out.push_str(repl);
            i = end;
        } else {
            let c = s[i..].chars().next().unwrap_or_default();
            out.push(c);
            i += c.len_utf8().max(1);
        }
    }
    out
}

/// Replace the account's own names. `names` pairs a token with its
/// placeholder; longer tokens go first; tokens under 2 chars are ignored.
pub fn scrub_names(s: &str, names: &[(String, &'static str)]) -> String {
    let mut sorted: Vec<&(String, &'static str)> = names
        .iter()
        .filter(|(t, _)| t.chars().count() >= 2)
        .collect();
    sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    sorted.into_iter().fold(s.to_owned(), |acc, (token, repl)| {
        replace_word(&acc, token, repl)
    })
}
