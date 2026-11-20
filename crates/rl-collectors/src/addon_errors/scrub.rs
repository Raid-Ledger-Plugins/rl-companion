//! Contract §4 scrubbing: user home folders (Windows / macOS `Users`, Linux
//! and Wine `home`) → `<home>\`, WoW install roots → `<wow>\`, the folders of
//! any other drive or network-share path → `<path>\`, the account folder and
//! its own character / realm names → `<account>` / `<char>` / `<realm>`. [`has_unscrubbed_path`] mirrors the schema's refusal
//! (`WOW_ADDON_ERRORS_UNSCRUBBED_PATH_RE`).

use crate::wow_sv::is_flavor_folder;

const HOME: &str = "<home>\\";
const WOW: &str = "<wow>\\";
const PATH: &str = "<path>\\";
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

/// `[\\/]<word>[\\/]` (any case) starting at `p`.
fn seg_at(s: &[u8], p: usize, word: &[u8]) -> bool {
    let n = word.len();
    p + n + 1 < s.len()
        && is_sep(s[p])
        && s[p + 1..p + 1 + n].eq_ignore_ascii_case(word)
        && is_sep(s[p + 1 + n])
}

/// `[\\/]users[\\/]` (any case) starting at `p`.
fn users_at(s: &[u8], p: usize) -> bool {
    seg_at(s, p, b"users")
}

/// `p` begins a path: start of text, after whitespace, a quote, a bracket,
/// `=`, `<` or Lua's `@` chunk prefix.
fn opener_at(b: &[u8], p: usize) -> bool {
    p == 0
        || matches!(
            b[p - 1],
            b' ' | b'\t' | b'\n' | b'\r' | b'"' | b'\'' | b'(' | b'[' | b'=' | b'<' | b'@'
        )
}

/// A Linux / Wine home: `/home/` at a path start or `X:\home\` (any case).
fn home_at(b: &[u8], p: usize) -> bool {
    seg_at(b, p, b"home") && (opener_at(b, p) || (p >= 2 && drive_at(b, p - 2)))
}

/// The folder-word length of a home folder at `p` (`users` / `home`).
fn home_word(b: &[u8], p: usize) -> Option<usize> {
    if users_at(b, p) {
        Some(5)
    } else if home_at(b, p) {
        Some(4)
    } else {
        None
    }
}

/// A network share `\\server\…` at a path start.
fn unc_at(b: &[u8], p: usize) -> bool {
    p + 2 < b.len() && b[p] == b'\\' && b[p + 1] == b'\\' && !is_sep(b[p + 2]) && opener_at(b, p)
}

/// End of the folder part of an absolute path whose first separator is at
/// `from`: past the separator before an `Interface` / `AddOns` folder, else
/// past the last separator before a line end, a quote, `:` or `>`.
fn folder_end(b: &[u8], from: usize) -> usize {
    let limit = b.len().min(from + MAX_PATH_LOOKBACK);
    let mut end = from;
    for j in from..limit {
        if matches!(b[j], b'\n' | b'\r' | b'"' | b'\'' | b':' | b'>') {
            break;
        }
        if is_sep(b[j]) {
            if seg_at(b, j, b"interface") || seg_at(b, j, b"addons") {
                return j + 1;
            }
            end = j + 1;
        }
    }
    end
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

/// A WoW root later on the path starting at `p` (a drive or `/Users/`) (same line, within
/// [`MAX_PATH_LOOKBACK`], before any other drive letter): its end.
fn wow_root_ahead(b: &[u8], p: usize) -> Option<usize> {
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

/// `[drive:]<sep>Users|home<sep><name>[<sep>]` at the `<sep>` `i`, `word`
/// being the length of `Users` / `home`: (start, end).
fn home_span(b: &[u8], i: usize, word: usize) -> (usize, usize) {
    let start = if i >= 2 && drive_at(b, i - 2) {
        i - 2
    } else {
        i
    };
    let name = i + word + 2;
    let stop = |c: u8| is_sep(c) || matches!(c, b'\n' | b'\r' | b'"' | b'\'' | b':');
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

/// Replace WoW install roots, home folders and any leftover drive prefix.
/// Repeats while a replacement's trailing `\` forms a new match (e.g.
/// `/Users/a/Users/b/`); callers still drop text that stays refusable.
pub fn scrub_paths(s: &str) -> String {
    let mut out = scrub_once(s);
    for _ in 0..8 {
        if !has_unscrubbed_path(&out) {
            break;
        }
        out = scrub_once(&out);
    }
    out
}

fn scrub_once(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut done, mut i) = (0, 0);
    while i < b.len() {
        let hit = if let Some(end) = wow_root_end(b, i) {
            Some((path_start(b, done, i), WOW, end))
        } else if let Some(word) = home_word(b, i) {
            let (start, home_end) = home_span(b, i, word);
            let (repl, end) = wow_root_ahead(b, i).map_or((HOME, home_end), |e| (WOW, e));
            Some((start, repl, end))
        } else if drive_at(b, i) {
            Some(drive_path(b, i))
        } else if unc_at(b, i) {
            let (repl, end) =
                wow_root_ahead(b, i).map_or((PATH, folder_end(b, i + 2)), |e| (WOW, e));
            Some((i, repl, end))
        } else {
            None
        };
        match hit {
            Some((start, repl, end)) => {
                out.push_str(&s[done..start]);
                out.push_str(repl);
                (done, i) = (end, end);
            }
            None => i += 1,
        }
    }
    out.push_str(&s[done..]);
    out
}

/// `X:\…` at `i`: a WoW root, a home folder, else every folder up to the
/// file name (or up to `Interface` / `AddOns`) → `<path>\`.
fn drive_path(b: &[u8], i: usize) -> (usize, &'static str, usize) {
    if let Some(end) = wow_root_ahead(b, i) {
        return (i, WOW, end);
    }
    if let Some(word) = home_word(b, i + 2) {
        return (i, HOME, home_span(b, i + 2, word).1);
    }
    (i, PATH, folder_end(b, i + 2))
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
