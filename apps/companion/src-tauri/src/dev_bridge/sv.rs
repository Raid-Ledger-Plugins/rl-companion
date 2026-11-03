//! `GET /dev/v1/ledgerlink/sv` (`$defs/sv-raw`) and `/sv/parsed`
//! (`$defs/sv-parsed`). Reads only
//! `<flavor>/WTF/Account/*/(*/*/)SavedVariables/LedgerLink.lua` (§7); every
//! path in a body is relative to the WoW folder.

use super::paging::{clip, json_len, text_slice, wrap_next, PAGE_MAX_BYTES};
use super::token::sha256_hex;
use rl_collectors::{
    parse_ledgerlink_outbox, rfc3339_utc_from_unix_seconds, validate_export_text, MAX_SV_BYTES,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const SV_FILE: &str = "LedgerLink.lua";
/// `sv-raw.text` maxLength (chars).
const RAW_TEXT_MAX_CHARS: usize = 12_288;
const MAX_OUTBOX: usize = 40;
const MAX_TOP_KEYS: usize = 32;
/// The most one `sv-parsed.files[]` row may serialise to: the page minus the
/// `{"flavor":…,"files":[],"next":<128 chars>}` wrapper (≈ 190 B), so the
/// pager's one-item-always-fits fallback can never emit a page over 16 KiB.
pub const ROW_MAX_BYTES: usize = PAGE_MAX_BYTES - 512;

#[derive(Debug, Clone)]
pub struct SvFile {
    pub scope: &'static str,
    pub account: String,
    pub realm: Option<String>,
    pub character: Option<String>,
    /// Relative to the WoW folder, forward slashes.
    pub rel: String,
    pub abs: PathBuf,
}

fn dirs(p: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(p) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| (1..=128).contains(&n.chars().count()))
        .collect();
    out.sort();
    out
}

/// Every LedgerLink SavedVariables file of `flavor`, account files first.
pub fn sv_files(wow_dir: &Path, flavor: &str) -> Vec<SvFile> {
    let accounts = wow_dir.join(flavor).join("WTF").join("Account");
    let mut out = Vec::new();
    for acct in dirs(&accounts) {
        let base = format!("{flavor}/WTF/Account/{acct}");
        let mut push = |realm: Option<&str>, ch: Option<&str>| {
            let mid = match (realm, ch) {
                (Some(r), Some(c)) => format!("/{r}/{c}"),
                _ => String::new(),
            };
            let rel = format!("{base}{mid}/SavedVariables/{SV_FILE}");
            let abs = wow_dir.join(&rel);
            // `symlink_metadata`: a symlinked `LedgerLink.lua` is not followed (§7).
            let plain_file = std::fs::symlink_metadata(&abs).is_ok_and(|m| m.file_type().is_file());
            if plain_file && rel.chars().count() <= 512 {
                out.push(SvFile {
                    scope: if ch.is_some() { "character" } else { "account" },
                    account: acct.clone(),
                    realm: realm.map(str::to_owned),
                    character: ch.map(str::to_owned),
                    rel,
                    abs,
                });
            }
        };
        push(None, None);
        for realm in dirs(&accounts.join(&acct)) {
            if realm == "SavedVariables" {
                continue;
            }
            for ch in dirs(&accounts.join(&acct).join(&realm)) {
                push(Some(&realm), Some(&ch));
            }
        }
    }
    out
}

fn mtime(p: &Path) -> (String, i64) {
    let secs = std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    (rfc3339_utc_from_unix_seconds(secs), secs)
}

/// `(len, mtime ns)` of a file: what an sv-raw cursor is tied to.
type FileVersion = (u64, u128);

fn version(p: &Path) -> FileVersion {
    let m = std::fs::metadata(p).ok();
    let len = m.as_ref().map_or(0, std::fs::Metadata::len);
    let ns = m
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    (len, ns)
}

/// An sv-raw cursor: `o<offset>v<len>m<mtime ns>`, base64url (≤ 128 chars).
fn raw_cursor(offset: usize, (len, ns): FileVersion) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("o{offset}v{len}m{ns}"))
}

/// Why a raw page was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum RawError {
    /// No file of that scope (404).
    Missing,
    /// The cursor is not one this endpoint issued (400).
    BadCursor,
    /// The file was rewritten since the cursor was issued (400: restart paging).
    Changed,
}

/// `?cursor=` → `(offset, version)`; `None` cursor = first page.
fn parse_raw_cursor(raw: Option<&str>) -> Result<Option<(usize, FileVersion)>, RawError> {
    use base64::Engine;
    let Some(raw) = raw else { return Ok(None) };
    let text = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or(RawError::BadCursor)?;
    let rest = text.strip_prefix('o').ok_or(RawError::BadCursor)?;
    let (off, rest) = rest.split_once('v').ok_or(RawError::BadCursor)?;
    let (len, ns) = rest.split_once('m').ok_or(RawError::BadCursor)?;
    match (off.parse(), len.parse(), ns.parse()) {
        (Ok(o), Ok(l), Ok(n)) => Ok(Some((o, (l, n)))),
        _ => Err(RawError::BadCursor),
    }
}

fn read_capped(p: &Path) -> Option<Vec<u8>> {
    let len = std::fs::metadata(p).ok()?.len();
    if len > MAX_SV_BYTES as u64 {
        return None;
    }
    std::fs::read(p).ok()
}

/// The newest file of `scope`, one text page from the cursor's offset. A
/// cursor carries the file's length + mtime, so a file rewritten mid-paging
/// is refused instead of splicing two versions together.
pub fn raw(
    wow_dir: &Path,
    flavor: &str,
    scope: &str,
    cursor: Option<&str>,
) -> Result<Value, RawError> {
    let parsed = parse_raw_cursor(cursor)?;
    let file = sv_files(wow_dir, flavor)
        .into_iter()
        .filter(|f| f.scope == scope)
        .max_by_key(|f| mtime(&f.abs).1)
        .ok_or(RawError::Missing)?;
    let ver = version(&file.abs);
    let start = match parsed {
        Some((_, v)) if v != ver => return Err(RawError::Changed),
        Some((o, _)) => o,
        None => 0,
    };
    let bytes = read_capped(&file.abs).ok_or(RawError::Missing)?;
    let text = String::from_utf8_lossy(&bytes);
    if start > text.len() || !text.is_char_boundary(start) {
        return Err(RawError::BadCursor);
    }
    let body = |t: &str, next: Value| {
        json!({ "flavor": flavor, "scope": scope, "path": file.rel, "bytes": bytes.len(),
                "mtime": mtime(&file.abs).0, "text": t, "next": next })
    };
    let overhead = json_len(&body("", Value::String("A".repeat(128))));
    let (slice, end) = text_slice(&text, start, PAGE_MAX_BYTES - overhead, RAW_TEXT_MAX_CHARS);
    let next = (end < text.len()).then(|| raw_cursor(end, ver));
    Ok(body(slice, wrap_next(next)))
}

/// Keys of the top-level table: lines holding exactly one tab then `["key"]`.
pub fn top_keys(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("\t[\""))
        .filter_map(|r| r.split_once("\"]").map(|(k, _)| k))
        .filter(|k| !k.is_empty())
        .map(|k| clip(k, 64))
        .take(MAX_TOP_KEYS)
        .collect()
}

fn outbox_row(e: &rl_collectors::OutboxEntry) -> Value {
    let ts = |s: Option<i64>| s.map(rfc3339_utc_from_unix_seconds);
    json!({
        "guidHash": sha256_hex(&e.guid),
        "section": clip(e.section.as_deref().unwrap_or("unknown"), 128),
        "source": clip(e.source.as_deref().unwrap_or("unknown"), 128),
        "at": ts(e.stored_at),
        "exportedAt": ts(e.exported_at),
        "pages": e.text.split_whitespace().count(),
        "bytes": e.text.len(),
        "decodes": validate_export_text(&e.text).is_ok(),
    })
}

/// The `sv-parsed.files[]` rows of one file. GUIDs are hashed; no export text
/// is returned. Each row is ≤ [`ROW_MAX_BYTES`]: the schema has no truncation
/// marker (`additionalProperties: false`), so an outbox that would not fit is
/// SPLIT across consecutive rows with the same scope/account/realm/character.
/// Continuation rows carry `topKeys: []` and `blockedCount: 0`, so summing
/// rows per file still gives the file's totals.
pub fn parsed_rows(f: &SvFile) -> Vec<Value> {
    let bytes = read_capped(&f.abs).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let (outbox, blocked) = match parse_ledgerlink_outbox(&bytes) {
        Ok(o) => (o.entries, o.skipped.len()),
        Err(_) => (Vec::new(), 0),
    };
    let mut head = json!({
        "scope": f.scope, "account": f.account, "mtime": mtime(&f.abs).0,
        "topKeys": top_keys(&text), "outbox": [], "blockedCount": blocked, "region": null,
    });
    if let (Some(r), Some(c)) = (&f.realm, &f.character) {
        head["realm"] = json!(r);
        head["character"] = json!(c);
    }
    let mut cont = head.clone();
    cont["topKeys"] = json!([]);
    cont["blockedCount"] = json!(0);
    let mut rows = Vec::new();
    let (mut row, mut chunk) = (head, Vec::<Value>::new());
    let mut used = json_len(&row);
    for item in outbox.iter().take(MAX_OUTBOX).map(outbox_row) {
        let add = json_len(&item) + usize::from(!chunk.is_empty());
        if used + add > ROW_MAX_BYTES && !chunk.is_empty() {
            row["outbox"] = Value::Array(std::mem::take(&mut chunk));
            rows.push(std::mem::replace(&mut row, cont.clone()));
            used = json_len(&row);
        }
        used += json_len(&item) + usize::from(!chunk.is_empty());
        chunk.push(item);
    }
    row["outbox"] = Value::Array(chunk);
    rows.push(row);
    rows
}
