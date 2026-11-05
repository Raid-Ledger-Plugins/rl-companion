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
//! Comments, `<![CDATA[…]]>`, `<?…?>` and `<!…>` are skipped; other
//! elements (`save`, `region`, `children`, …) only nest. Quoted attribute
//! values may contain `>`; the five XML entities and `&#…;` / `&#x…;` are
//! decoded in the values the scanner keeps, any other `&` sequence there is
//! an error. Only a matching node's direct attributes are kept, never a
//! nested node's, and of those only `id` / `type` / `value` / `handle`.
//!
//! **Memory (spec §4, ≤ 512 MiB peak).** Per tag nothing is copied until a
//! value is kept ([`LSX_MAX_TAG_ATTRS`] bounds the scan of one tag). What a
//! file retains is bounded by [`LSX_MAX_TOTAL_ATTRS`] attributes and
//! [`LSX_MAX_RETAINED_BYTES`] of value text, charged *before* decoding:
//! ≈ 262k × ~200 B of `LsxAttr` + `Vec` slack ≈ 50 MiB, plus 32 MiB of
//! text — under 100 MiB beside the (≤ 256 MiB) entry buffer.

use crate::error::CatalogueError;

use super::loca::is_loca_handle;
use super::lsx_tag::{Raw, RawAttrs, Tag, parse_tag};

/// Deepest `<node>` nesting accepted.
pub const LSX_MAX_DEPTH: usize = 64;
/// Most matching nodes one file may yield.
pub const LSX_MAX_NODES: usize = 65_536;
/// Most attributes kept on one matching node.
pub const LSX_MAX_ATTRS: usize = 256;
/// Most attributes on any one tag, matching or not. A real LSX
/// `<attribute>` carries `id`, `type`, `value`/`handle` and `version`; a
/// `<node>` carries `id` (and sometimes `key`).
pub const LSX_MAX_TAG_ATTRS: usize = 16;
/// Most attributes one file may retain across all matching nodes.
pub const LSX_MAX_TOTAL_ATTRS: usize = 262_144;
/// Most value bytes (`id` + `type` + `value` + `handle`, as written) one
/// file may retain across all matching nodes.
pub const LSX_MAX_RETAINED_BYTES: usize = 32 * 1024 * 1024;

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
    let mut retained = Retained::default();
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
                    push_attr(&mut out[idx], &tag.attrs, &mut retained, start)?;
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

/// `Some(Some(len))` for a comment / CDATA / `<?…?>` / `<!…>` of `len`
/// bytes, `Some(None)` if it never ends, `None` for an ordinary tag.
fn skip_special(rest: &str) -> Option<Option<usize>> {
    let (open, close) = if rest.starts_with("<!--") {
        ("<!--", "-->")
    } else if rest.starts_with("<![CDATA[") {
        ("<![CDATA[", "]]>")
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
    let matched = match tag.attrs.id {
        Some(id) => id.eq_decoded(node_id)?,
        None => false,
    };
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

/// What one file has retained so far, against the file-wide caps.
#[derive(Default)]
struct Retained {
    attrs: usize,
    bytes: usize,
}

impl Retained {
    /// Charge one attribute of `raw_len` bytes as written (an upper bound
    /// of its decoded size), before anything is decoded or copied.
    fn charge(&mut self, raw_len: usize) -> Result<(), CatalogueError> {
        let attrs = self.attrs + 1;
        if attrs > LSX_MAX_TOTAL_ATTRS {
            return Err(too_large(
                "lsx retained attributes",
                attrs,
                LSX_MAX_TOTAL_ATTRS,
            ));
        }
        let bytes = self.bytes.saturating_add(raw_len);
        if bytes > LSX_MAX_RETAINED_BYTES {
            return Err(too_large(
                "lsx retained attribute bytes",
                bytes,
                LSX_MAX_RETAINED_BYTES,
            ));
        }
        self.attrs = attrs;
        self.bytes = bytes;
        Ok(())
    }
}

fn push_attr(
    node: &mut LsxNode,
    raw: &RawAttrs<'_>,
    retained: &mut Retained,
    at: usize,
) -> Result<(), CatalogueError> {
    if node.attrs.len() >= LSX_MAX_ATTRS {
        return Err(too_large(
            "lsx attributes on one node",
            node.attrs.len() + 1,
            LSX_MAX_ATTRS,
        ));
    }
    if raw.id.is_none_or(|r| r.text.is_empty()) {
        return Err(parse_err(at, "attribute without an id"));
    }
    retained.charge(raw.raw_len())?;
    let decode = |r: Option<Raw<'_>>| r.map(Raw::decode).transpose();
    let attr = LsxAttr {
        id: decode(raw.id)?.unwrap_or_default(),
        kind: decode(raw.kind)?,
        value: decode(raw.value)?,
        handle: decode(raw.handle)?,
    };
    node.attrs.push(attr);
    Ok(())
}

pub(super) fn too_large(what: &'static str, size: usize, limit: usize) -> CatalogueError {
    CatalogueError::TooLarge {
        what,
        size: size as u64,
        limit: limit as u64,
    }
}

pub(super) fn parse_err(at: usize, reason: &'static str) -> CatalogueError {
    CatalogueError::Parse {
        file: FILE,
        at,
        reason,
    }
}
