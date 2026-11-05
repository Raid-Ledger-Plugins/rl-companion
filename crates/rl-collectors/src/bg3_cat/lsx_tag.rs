//! Tag-level lexing for the LSX scanner: one opening tag's name and the
//! attributes the scanner reads, borrowed from the text and still escaped.
//!
//! Nothing here allocates per attribute: every attribute is counted against
//! [`LSX_MAX_TAG_ATTRS`], the four the scanner reads (`id`, `type`,
//! `value`, `handle`) are kept as borrowed raw slices, and every other one
//! is stepped over. Decoding happens only when the caller keeps a value.

use crate::error::CatalogueError;

use super::lsx::{LSX_MAX_TAG_ATTRS, parse_err, too_large};

/// An attribute value as written (still escaped), and where it starts.
#[derive(Debug, Clone, Copy)]
pub(super) struct Raw<'a> {
    pub text: &'a str,
    pub at: usize,
}

impl Raw<'_> {
    /// The value with entities decoded.
    pub fn decode(self) -> Result<String, CatalogueError> {
        unescape(self.text, self.at)
    }

    /// Whether the decoded value equals `want`; allocates only when the
    /// value holds an entity.
    pub fn eq_decoded(self, want: &str) -> Result<bool, CatalogueError> {
        if self.text.contains('&') {
            Ok(self.decode()? == want)
        } else {
            Ok(self.text == want)
        }
    }
}

/// The attributes the scanner reads. A repeated key keeps its last value.
#[derive(Debug, Default)]
pub(super) struct RawAttrs<'a> {
    pub id: Option<Raw<'a>>,
    pub kind: Option<Raw<'a>>,
    pub value: Option<Raw<'a>>,
    pub handle: Option<Raw<'a>>,
}

impl RawAttrs<'_> {
    /// Upper bound of the decoded bytes of every kept value (decoding only
    /// shrinks a value).
    pub fn raw_len(&self) -> usize {
        [self.id, self.kind, self.value, self.handle]
            .iter()
            .flatten()
            .map(|r| r.text.len())
            .sum()
    }
}

pub(super) struct Tag<'a> {
    pub name: &'a str,
    pub attrs: RawAttrs<'a>,
    pub self_closing: bool,
    pub end: usize,
}

/// An opening or self-closing tag starting at `text[start] == '<'`.
pub(super) fn parse_tag(text: &str, start: usize) -> Result<Tag<'_>, CatalogueError> {
    let b = text.as_bytes();
    let name_end = scan_name(b, start + 1);
    let name = &text[start + 1..name_end];
    if name.is_empty() {
        return Err(parse_err(start, "tag without a name"));
    }
    let mut tag = Tag {
        name,
        attrs: RawAttrs::default(),
        self_closing: false,
        end: 0,
    };
    let mut count = 0usize;
    let mut i = name_end;
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if let Some((self_closing, end)) = tag_end(b, i, start)? {
            tag.self_closing = self_closing;
            tag.end = end;
            return Ok(tag);
        }
        count += 1;
        if count > LSX_MAX_TAG_ATTRS {
            return Err(too_large(
                "lsx attributes on one tag",
                count,
                LSX_MAX_TAG_ATTRS,
            ));
        }
        i = read_attr(text, i, &mut tag.attrs)?;
    }
}

/// `Some((self_closing, end))` when the tag closes at `i`.
fn tag_end(b: &[u8], i: usize, start: usize) -> Result<Option<(bool, usize)>, CatalogueError> {
    match b.get(i) {
        None => Err(parse_err(start, "tag runs past the end")),
        Some(b'>') => Ok(Some((false, i + 1))),
        Some(b'/') if b.get(i + 1) == Some(&b'>') => Ok(Some((true, i + 2))),
        Some(_) => Ok(None),
    }
}

/// One `key="value"` (or `key = 'value'`; XML whitespace is allowed around
/// `=`) at `i`; returns the index just past it.
fn read_attr<'a>(
    text: &'a str,
    i: usize,
    attrs: &mut RawAttrs<'a>,
) -> Result<usize, CatalogueError> {
    let b = text.as_bytes();
    let key_end = scan_name(b, i);
    let key = &text[i..key_end];
    let eq = skip_xml_ws(b, key_end);
    let open = skip_xml_ws(b, eq + 1);
    let quote = match (key.is_empty(), b.get(eq), b.get(open)) {
        (false, Some(b'='), Some(&q)) if q == b'"' || q == b'\'' => q,
        _ => return Err(parse_err(i, "malformed tag attribute")),
    };
    let v_start = open + 1;
    let v_len = text[v_start..]
        .find(char::from(quote))
        .ok_or_else(|| parse_err(i, "unterminated attribute value"))?;
    let raw = Some(Raw {
        text: &text[v_start..v_start + v_len],
        at: v_start,
    });
    match key {
        "id" => attrs.id = raw,
        "type" => attrs.kind = raw,
        "value" => attrs.value = raw,
        "handle" => attrs.handle = raw,
        _ => {}
    }
    Ok(v_start + v_len + 1)
}

/// The first index at or after `i` that is not XML whitespace
/// (space, tab, CR, LF).
fn skip_xml_ws(b: &[u8], mut i: usize) -> usize {
    while b
        .get(i)
        .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
    {
        i += 1;
    }
    i
}

pub(super) fn scan_name(b: &[u8], mut i: usize) -> usize {
    while b
        .get(i)
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b':' | b'.'))
    {
        i += 1;
    }
    i
}

/// Decode the five XML entities and numeric character references.
fn unescape(raw: &str, at: usize) -> Result<String, CatalogueError> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let semi = rest[amp..]
            .find(';')
            .ok_or_else(|| parse_err(at, "unterminated entity"))?;
        let ch = match &rest[amp + 1..amp + semi] {
            "quot" => Some('"'),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "apos" => Some('\''),
            e => numeric_ref(e),
        };
        out.push(ch.ok_or_else(|| parse_err(at, "unknown or invalid entity"))?);
        rest = &rest[amp + semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `#65` / `#x41`: digits only (no sign), a valid non-NUL scalar.
fn numeric_ref(e: &str) -> Option<char> {
    let n = e.strip_prefix('#')?;
    let (digits, radix) = match n.strip_prefix(['x', 'X']) {
        Some(hex) => (hex, 16),
        None => (n, 10),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let code = u32::from_str_radix(digits, radix).ok()?;
    char::from_u32(code).filter(|&c| c != '\0')
}
