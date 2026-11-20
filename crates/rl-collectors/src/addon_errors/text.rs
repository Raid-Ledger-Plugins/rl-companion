//! Field sanitising: the contract's one-line / multi-line text rules and
//! UTF-16 length caps.

use super::TRUNCATED_MARKER;

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Cut `s` to at most `max` UTF-16 units; when it was cut, end it with
/// [`TRUNCATED_MARKER`] (included in the `max`).
pub(super) fn truncate_utf16(s: &str, max: usize, marker: bool) -> String {
    if utf16_len(s) <= max {
        return s.to_owned();
    }
    let tail = if marker { TRUNCATED_MARKER } else { "" };
    let budget = max.saturating_sub(utf16_len(tail));
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        if used + c.len_utf16() > budget {
            break;
        }
        used += c.len_utf16();
        out.push(c);
    }
    out.push_str(tail);
    out
}

/// Multi-line text: tab, LF and CR kept; other C0 controls and DEL dropped.
/// `None` when nothing is left.
pub(super) fn multi_line(s: &str, max: usize) -> Option<String> {
    let kept: String = s
        .chars()
        .filter(|&c| matches!(c, '\t' | '\n' | '\r') || !(c < ' ' || c == '\u{7F}'))
        .collect();
    (!kept.is_empty()).then(|| truncate_utf16(&kept, max, true))
}

fn is_one_line_banned(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// One-line text: control, line-separator and bidi characters dropped,
/// trimmed. `None` when nothing (or only whitespace) is left.
pub(super) fn one_line(s: &str, max: usize) -> Option<String> {
    let kept: String = s.chars().filter(|&c| !is_one_line_banned(c)).collect();
    let trimmed = kept.trim();
    (!trimmed.is_empty()).then(|| truncate_utf16(trimmed, max, false).trim().to_owned())
}

/// Whether `s` already satisfies the one-line rule and the length range.
pub(super) fn is_one_line(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && utf16_len(s) <= max && !s.chars().any(is_one_line_banned)
}
