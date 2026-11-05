//! Minimal LSX (Larian XML) scanner, hand-written (ruling Q12): yields the
//! `<attribute>` children of every `<node id="…">` with a given id.
//!
//! LSX shape (LSLib `LSXReader`; the node ids the catalogue asks for are
//! UNVERIFIED, U7/U8):
//!
//! ```text
//! <node id="Origin">
//!   <attribute id="Name" type="FixedString" value="Astarion" />
//!   <attribute id="DisplayName" type="TranslatedString" handle="h…" version="1" />
//!   <children> <node id="…"> … </node> </children>
//! </node>
//! ```
//!
//! Comments, `<?…?>` and `<!…>` are skipped; other elements (`save`,
//! `region`, `children`, …) only nest. Quoted attribute values may contain
//! `>`; the five XML entities and `&#…;` / `&#x…;` are decoded, any other
//! `&` sequence is an error. Only a matching node's direct attributes are
//! kept, never a nested node's.

use crate::error::CatalogueError;

use super::loca::is_loca_handle;

/// Deepest `<node>` nesting accepted.
pub const LSX_MAX_DEPTH: usize = 64;
/// Most matching nodes one file may yield.
pub const LSX_MAX_NODES: usize = 65_536;
/// Most attributes kept on one matching node.
pub const LSX_MAX_ATTRS: usize = 256;

const FILE: &str = "lsx";

/// One `<attribute>` element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LsxAttr {
    pub id: String,
    pub kind: Option<String>,
    pub value: Option<String>,
    pub handle: Option<String>,
}

/// One matching `<node>` and its direct attributes, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LsxNode {
    pub attrs: Vec<LsxAttr>,
}

impl LsxNode {
    /// The first attribute with this id.
    pub fn attr(&self, id: &str) -> Option<&LsxAttr> {
        self.attrs.iter().find(|a| a.id == id)
    }
}

/// Where one id → name table lives in an LSX file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LsxSource {
    pub node_id: &'static str,
    pub key_attr: &'static str,
    pub handle_attr: &'static str,
}

/// `(key, loca handle)` per node that has both: the key is the key
/// attribute's `value`; the handle is the handle attribute's `handle` (or
/// its `value` when that is a handle). Nodes missing either are skipped.
pub fn lsx_id_handles(nodes: &[LsxNode], src: &LsxSource) -> Vec<(String, String)> {
    nodes
        .iter()
        .filter_map(|n| {
            let key = n.attr(src.key_attr)?.value.clone()?;
            let h = n.attr(src.handle_attr)?;
            let handle = h.handle.clone().or_else(|| h.value.clone())?;
            (!key.is_empty() && is_loca_handle(&handle)).then_some((key, handle))
        })
        .collect()
}

/// Every `<node id="node_id">` in `text`, with its direct attributes.
pub fn read_lsx_nodes(text: &str, node_id: &str) -> Result<Vec<LsxNode>, CatalogueError> {
    let mut out: Vec<LsxNode> = Vec::new();
    let mut stack: Vec<Option<usize>> = Vec::new();
    let mut pos = 0;
    while let Some(off) = text[pos..].find('<') {
        let start = pos + off;
        let rest = &text[start..];
        if let Some(skip) = skip_special(rest) {
            pos = start + skip.ok_or_else(|| parse_err(start, "unterminated markup"))?;
            continue;
        }
        if let Some(close) = rest.strip_prefix("</") {
            let end = close
                .find('>')
                .ok_or_else(|| parse_err(start, "unterminated tag"))?;
            if close[..end].trim() == "node" && stack.pop().is_none() {
                return Err(parse_err(start, "</node> without an open node"));
            }
            pos = start + 2 + end + 1;
            continue;
        }
        let tag = parse_tag(text, start)?;
        pos = tag.end;
        match tag.name {
            "node" => open_node(&mut out, &mut stack, &tag, node_id, start)?,
            "attribute" => {
                if let Some(&Some(idx)) = stack.last() {
                    push_attr(&mut out[idx], tag.attrs, start)?;
                }
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(parse_err(text.len(), "a node is never closed"));
    }
    Ok(out)
}

/// `Some(Some(len))` for a comment / `<?…?>` / `<!…>` of `len` bytes,
/// `Some(None)` if it never ends, `None` for an ordinary tag.
fn skip_special(rest: &str) -> Option<Option<usize>> {
    let (open, close) = if rest.starts_with("<!--") {
        ("<!--", "-->")
    } else if rest.starts_with("<?") {
        ("<?", "?>")
    } else if rest.starts_with("<!") {
        ("<!", ">")
    } else {
        return None;
    };
    Some(
        rest[open.len()..]
            .find(close)
            .map(|e| open.len() + e + close.len()),
    )
}

fn open_node(
    out: &mut Vec<LsxNode>,
    stack: &mut Vec<Option<usize>>,
    tag: &Tag<'_>,
    node_id: &str,
    at: usize,
) -> Result<(), CatalogueError> {
    let matched = tag.attrs.iter().any(|(k, v)| *k == "id" && v == node_id);
    let idx = if matched {
        if out.len() >= LSX_MAX_NODES {
            return Err(too_large(
                "lsx matching node count",
                out.len() + 1,
                LSX_MAX_NODES,
            ));
        }
        out.push(LsxNode::default());
        Some(out.len() - 1)
    } else {
        None
    };
    if !tag.self_closing {
        if stack.len() >= LSX_MAX_DEPTH {
            return Err(parse_err(at, "nodes nest too deep"));
        }
        stack.push(idx);
    }
    Ok(())
}

fn push_attr(
    node: &mut LsxNode,
    attrs: Vec<(&str, String)>,
    at: usize,
) -> Result<(), CatalogueError> {
    if node.attrs.len() >= LSX_MAX_ATTRS {
        return Err(too_large(
            "lsx attributes on one node",
            node.attrs.len() + 1,
            LSX_MAX_ATTRS,
        ));
    }
    let mut attr = LsxAttr::default();
    for (k, v) in attrs {
        match k {
            "id" => attr.id = v,
            "type" => attr.kind = Some(v),
            "value" => attr.value = Some(v),
            "handle" => attr.handle = Some(v),
            _ => {}
        }
    }
    if attr.id.is_empty() {
        return Err(parse_err(at, "attribute without an id"));
    }
    node.attrs.push(attr);
    Ok(())
}

struct Tag<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, String)>,
    self_closing: bool,
    end: usize,
}

/// An opening or self-closing tag starting at `text[start] == '<'`.
fn parse_tag(text: &str, start: usize) -> Result<Tag<'_>, CatalogueError> {
    let b = text.as_bytes();
    let name_end = scan_name(b, start + 1);
    let name = &text[start + 1..name_end];
    if name.is_empty() {
        return Err(parse_err(start, "tag without a name"));
    }
    let mut attrs = Vec::new();
    let mut i = name_end;
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match b.get(i) {
            None => return Err(parse_err(start, "tag runs past the end")),
            Some(b'>') => {
                return Ok(Tag {
                    name,
                    attrs,
                    self_closing: false,
                    end: i + 1,
                });
            }
            Some(b'/') if b.get(i + 1) == Some(&b'>') => {
                return Ok(Tag {
                    name,
                    attrs,
                    self_closing: true,
                    end: i + 2,
                });
            }
            Some(_) => {}
        }
        let key_end = scan_name(b, i);
        let key = &text[i..key_end];
        let quote = match (key.is_empty(), b.get(key_end), b.get(key_end + 1)) {
            (false, Some(b'='), Some(&q)) if q == b'"' || q == b'\'' => q,
            _ => return Err(parse_err(i, "malformed tag attribute")),
        };
        let v_start = key_end + 2;
        let v_len = text[v_start..]
            .find(char::from(quote))
            .ok_or_else(|| parse_err(i, "unterminated attribute value"))?;
        attrs.push((key, unescape(&text[v_start..v_start + v_len], v_start)?));
        i = v_start + v_len + 1;
    }
}

fn scan_name(b: &[u8], mut i: usize) -> usize {
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

fn numeric_ref(e: &str) -> Option<char> {
    let n = e.strip_prefix('#')?;
    let code = match n.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => n.parse::<u32>().ok()?,
    };
    char::from_u32(code).filter(|&c| c != '\0')
}

fn too_large(what: &'static str, size: usize, limit: usize) -> CatalogueError {
    CatalogueError::TooLarge {
        what,
        size: size as u64,
        limit: limit as u64,
    }
}

fn parse_err(at: usize, reason: &'static str) -> CatalogueError {
    CatalogueError::Parse {
        file: FILE,
        at,
        reason,
    }
}
