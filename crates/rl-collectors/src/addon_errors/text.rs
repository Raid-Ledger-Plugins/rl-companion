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

/// Hex runs of 8+ characters holding a digit (Lua `table: 000001F3…`,
/// `0x…`, GUID parts) → `<addr>`, so an error that prints a fresh address
/// on every occurrence keeps one key and one record.
pub(super) fn mask_addresses(s: &str) -> String {
    let b = s.as_bytes();
    let boundary = |p: usize| p == 0 || !b[p - 1].is_ascii_alphanumeric();
    let mut out = String::with_capacity(s.len());
    let (mut done, mut i) = (0, 0);
    while i < b.len() {
        let hex = if i + 1 < b.len() && b[i] == b'0' && (b[i + 1] | 0x20) == b'x' && boundary(i) {
            Some(i + 2)
        } else if b[i].is_ascii_hexdigit() && boundary(i) {
            Some(i)
        } else {
            None
        };
        if let Some(h) = hex {
            let n = b[h..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
            let end = h + n;
            let whole = end == b.len() || !b[end].is_ascii_alphanumeric();
            if n >= 8 && whole && b[h..end].iter().any(u8::is_ascii_digit) {
                out.push_str(&s[done..i]);
                out.push_str("<addr>");
                (done, i) = (end, end);
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[done..]);
    out
}
