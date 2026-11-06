//! Spec D9 / ROK-2079 D5: a loca string is only sent as a display name if
//! it survives this filter, otherwise it becomes `None` (never truncated,
//! never an error).
//!
//! Steps: every `<…>` markup tag (`<LSTag …>`, `<br>`, `<b>`) becomes a
//! space, whitespace runs collapse to one space, the ends are trimmed.
//! The result is refused when it is empty, longer than
//! [`CAT_MAX_NAME_UTF16`] UTF-16 units, holds a character
//! `COMPANION_DISPLAY_TEXT_RE` refuses (control, U+2028/9, bidi
//! embedding/override/isolate), a GUID (`8-4-4-4-12` hex, or a run of
//! [`CAT_GUID_HEX_RUN`] hex digits) or a run of [`CAT_MAX_DIGIT_RUN`] + 1
//! ASCII digits. A `<` with no closing `>` is refused rather than guessed.

/// Longest display name kept, in UTF-16 units (`companionDisplayTextSchema(128)`).
pub const CAT_MAX_NAME_UTF16: usize = 128;
/// Longest ASCII digit run allowed; one more is an id leak (ROK-2079 D5
/// "10+ digit run").
pub const CAT_MAX_DIGIT_RUN: usize = 9;
/// A run of this many hex digits is a bare (unhyphenated) GUID.
pub const CAT_GUID_HEX_RUN: usize = 32;

/// The display form of a loca string, or `None` when it fails the filter.
pub fn catalogue_display_text(raw: &str) -> Option<String> {
    let text = collapse(&strip_markup(raw)?);
    let ok = !text.is_empty()
        && text.encode_utf16().count() <= CAT_MAX_NAME_UTF16
        && !text.chars().any(refused_char)
        && !has_guid(&text)
        && longest_run(&text, |c| c.is_ascii_digit()) <= CAT_MAX_DIGIT_RUN;
    ok.then_some(text)
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

fn has_guid(s: &str) -> bool {
    if longest_run(s, |c| c.is_ascii_hexdigit()) >= CAT_GUID_HEX_RUN {
        return true;
    }
    let b = s.as_bytes();
    const SHAPE: [usize; 5] = [8, 4, 4, 4, 12];
    let len: usize = SHAPE.iter().sum::<usize>() + 4;
    (0..b.len().saturating_sub(len - 1)).any(|start| hyphenated_guid_at(&b[start..], &SHAPE))
}

fn hyphenated_guid_at(b: &[u8], shape: &[usize; 5]) -> bool {
    let mut i = 0;
    for (n, &group) in shape.iter().enumerate() {
        if n > 0 {
            if b.get(i) != Some(&b'-') {
                return false;
            }
            i += 1;
        }
        let Some(part) = b.get(i..i + group) else {
            return false;
        };
        if !part.iter().all(u8::is_ascii_hexdigit) {
            return false;
        }
        i += group;
    }
    true
}

fn longest_run(s: &str, pred: impl Fn(char) -> bool) -> usize {
    let (mut best, mut cur) = (0, 0);
    for c in s.chars() {
        cur = if pred(c) { cur + 1 } else { 0 };
        best = best.max(cur);
    }
    best
}
