//! Spec D9 / ROK-2079 D5: a loca string is only sent as a display name if
//! it survives this filter, otherwise it becomes `None` (never truncated,
//! never an error).
//!
//! Steps: every `<…>` markup tag (`<LSTag …>`, `<br>`, `<b>`) becomes a
//! space, Unicode whitespace runs (tab, newline, U+2028/9 included)
//! collapse to one space, the ends are trimmed.
//! The result is refused when it is empty, longer than
//! [`CAT_MAX_NAME_UTF16`] UTF-16 units, holds a character
//! `COMPANION_DISPLAY_TEXT_RE` refuses (control, bidi
//! embedding/override/isolate), any GUID shape the contract's body-wide
//! `bg3HoldsGuid` refuses, or a run of [`CAT_MAX_DIGIT_RUN`] + 1 ASCII
//! digits — those two checked with invisible format characters removed, so
//! a name that passes here never trips the body guard (D8). A `<` with no
//! closing `>` is refused rather than guessed.

/// Longest display name kept, in UTF-16 units (`companionDisplayTextSchema(128)`).
pub const CAT_MAX_NAME_UTF16: usize = 128;
/// Longest ASCII digit run allowed; one more is an id leak (ROK-2079 D5
/// "10+ digit run").
pub const CAT_MAX_DIGIT_RUN: usize = 9;
/// A run of this many hex digits is a bare (unhyphenated) GUID.
pub const CAT_GUID_HEX_RUN: usize = 32;

use super::is_bg3_id;

/// The display form of a loca string, or `None` when it fails the filter.
pub fn catalogue_display_text(raw: &str) -> Option<String> {
    let text = collapse(&strip_markup(raw)?);
    let ok = !text.is_empty()
        && text.encode_utf16().count() <= CAT_MAX_NAME_UTF16
        && !text.chars().any(refused_char)
        && !holds_id_leak(&text);
    ok.then_some(text)
}

/// True when an LSX / stats key may become a payload object key: it is a
/// `BG3_ID_RE` id and the body-wide guard would not refuse it (D8 — game
/// data never rejects a whole message).
pub(crate) fn catalogue_key_ok(key: &str) -> bool {
    is_bg3_id(key) && !holds_id_leak(key)
}

/// A GUID shape or a too-long digit run, once invisible format characters
/// (which would split either without showing) are removed.
fn holds_id_leak(s: &str) -> bool {
    let visible: String = s.chars().filter(|c| !invisible_format(*c)).collect();
    holds_guid(&visible) || longest_run(&visible, |c| c.is_ascii_digit()) > CAT_MAX_DIGIT_RUN
}

/// Zero-width and direction marks: U+200B–U+200F, U+061C, U+FEFF, U+2060.
fn invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{061C}' | '\u{FEFF}' | '\u{2060}'
    )
}

/// Every `<…>` replaced by a space; `None` for an unclosed `<`.
fn strip_markup(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('>')?;
        out.push(' ');
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What `COMPANION_DISPLAY_TEXT_RE` refuses.
fn refused_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Port of the contract's `bg3HoldsGuid`: an 8-4-4-4-12 GUID with each gap
/// `-`, `_`, `.` or nothing; a Larian handle (`h` + 8 hex + `g` + 4 hex, any
/// case); a 32+ hex run; or 32 hex once every `-_.{}` is stripped.
fn holds_guid(s: &str) -> bool {
    let b = s.as_bytes();
    let stripped: String = s
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.' | '{' | '}'))
        .collect();
    (0..b.len()).any(|i| split_guid_at(&b[i..]) || larian_handle_at(&b[i..]))
        || longest_run(s, |c| c.is_ascii_hexdigit()) >= CAT_GUID_HEX_RUN
        || longest_run(&stripped, |c| c.is_ascii_hexdigit()) >= CAT_GUID_HEX_RUN
}

/// `HEX{8} SEP HEX{4} SEP HEX{4} SEP HEX{4} SEP HEX{12}`, `SEP = [-_.]?`. A
/// separator is never hex, so consuming one when present is exact.
fn split_guid_at(b: &[u8]) -> bool {
    let mut i = 0;
    for (n, group) in [8, 4, 4, 4, 12].into_iter().enumerate() {
        if n > 0 && matches!(b.get(i), Some(b'-' | b'_' | b'.')) {
            i += 1;
        }
        if !hex_at(b, i, group) {
            return false;
        }
        i += group;
    }
    true
}

/// `/h[0-9a-f]{8}g[0-9a-f]{4}/i`.
fn larian_handle_at(b: &[u8]) -> bool {
    matches!(b.first(), Some(b'h' | b'H'))
        && hex_at(b, 1, 8)
        && matches!(b.get(9), Some(b'g' | b'G'))
        && hex_at(b, 10, 4)
}

fn hex_at(b: &[u8], at: usize, len: usize) -> bool {
    b.get(at..at + len)
        .is_some_and(|p| p.iter().all(u8::is_ascii_hexdigit))
}

fn longest_run(s: &str, pred: impl Fn(char) -> bool) -> usize {
    let (mut best, mut cur) = (0, 0);
    for c in s.chars() {
        cur = if pred(c) { cur + 1 } else { 0 };
        best = best.max(cur);
    }
    best
}
