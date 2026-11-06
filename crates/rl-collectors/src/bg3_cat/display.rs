//! Spec D9 / ROK-2079 D5: a loca string is only sent as a display name if
//! it survives this filter, otherwise it becomes `None` (never truncated,
//! never an error).
//!
//! Steps, in one bounded pass: every `<…>` markup tag (`<LSTag …>`,
//! `<br>`, `<b>`) becomes a space, ECMAScript `\s` runs (the set
//! `COMPANION_DISPLAY_TEXT_RE` reads as blank — U+FEFF included) collapse
//! to one space, the ends are trimmed. The pass stops as soon as the output
//! passes the length cap, so a huge loca value never costs more than the cap.
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
    let n = normalise_display(raw).ok()?;
    let ok = !n.display.is_empty()
        && !n.display.chars().any(refused_char)
        && !holds_id_leak(&n.display)
        && !holds_id_leak(&n.leak);
    ok.then_some(n.display)
}

/// Why [`normalise_display`] gave up on a loca string.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The normalised name passed [`CAT_MAX_NAME_UTF16`].
    TooLong,
    /// A `<` with no closing `>`.
    Unclosed,
}

/// Both views [`normalise_display`] builds; neither outgrows the cap.
#[derive(Debug, Default)]
pub(crate) struct Normalised {
    /// Markup gone, whitespace collapsed, ends trimmed.
    pub(crate) display: String,
    /// `display` with U+FEFF deleted rather than read as a space: the
    /// id-leak view, since a BOM between digits hides the join (D8).
    pub(crate) leak: String,
    gap: bool,
    leak_gap: bool,
    units: usize,
}

impl Normalised {
    /// Appends a visible char (and the pending space), or `TooLong`.
    fn push(&mut self, c: char) -> Result<(), Refusal> {
        let space = self.gap && !self.display.is_empty();
        self.units += usize::from(space) + c.len_utf16();
        if self.units > CAT_MAX_NAME_UTF16 {
            return Err(Refusal::TooLong);
        }
        if space {
            self.display.push(' ');
        }
        if self.leak_gap && !self.leak.is_empty() {
            self.leak.push(' ');
        }
        self.display.push(c);
        self.leak.push(c);
        (self.gap, self.leak_gap) = (false, false);
        Ok(())
    }
}

/// One pass over `raw`: markup and whitespace become a single gap, output
/// is capped at [`CAT_MAX_NAME_UTF16`] units and the pass stops there.
pub(crate) fn normalise_display(raw: &str) -> Result<Normalised, Refusal> {
    let mut out = Normalised::default();
    let mut rest = raw;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let close = rest.find('>').ok_or(Refusal::Unclosed)?;
            rest = &rest[close + 1..];
            (out.gap, out.leak_gap) = (true, true);
            continue;
        }
        rest = &rest[c.len_utf8()..];
        if c == '\u{FEFF}' {
            out.gap = true;
        } else if es_whitespace(c) {
            (out.gap, out.leak_gap) = (true, true);
        } else {
            out.push(c)?;
        }
    }
    Ok(out)
}

/// ECMAScript `\s`: WhiteSpace + LineTerminator. Unlike Rust's
/// `char::is_whitespace` it holds U+FEFF and not U+0085 (a control char,
/// refused).
fn es_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
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
