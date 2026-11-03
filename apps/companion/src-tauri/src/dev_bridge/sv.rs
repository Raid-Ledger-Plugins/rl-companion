//! `GET /dev/v1/ledgerlink/sv` (`$defs/sv-raw`) and `/sv/parsed`
//! (`$defs/sv-parsed`). Reads only
//! `<flavor>/WTF/Account/*/(*/*/)SavedVariables/LedgerLink.lua` (§7); every
//! path in a body is relative to the WoW folder.

use super::paging::{clip, json_len, text_slice, PAGE_MAX_BYTES};
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
            if abs.is_file() && rel.chars().count() <= 512 {
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

fn read_capped(p: &Path) -> Option<Vec<u8>> {
    let len = std::fs::metadata(p).ok()?.len();
    if len > MAX_SV_BYTES as u64 {
        return None;
    }
    std::fs::read(p).ok()
}

/// The newest file of `scope`, one text page from byte `start`.
/// `Err(true)` = no such file (404), `Err(false)` = bad cursor (400).
pub fn raw(wow_dir: &Path, flavor: &str, scope: &str, start: usize) -> Result<Value, bool> {
    let file = sv_files(wow_dir, flavor)
        .into_iter()
        .filter(|f| f.scope == scope)
        .max_by_key(|f| mtime(&f.abs).1)
        .ok_or(true)?;
    let bytes = read_capped(&file.abs).ok_or(true)?;
    let text = String::from_utf8_lossy(&bytes);
    if start > text.len() || !text.is_char_boundary(start) {
        return Err(false);
    }
    let body = |t: &str, next: Value| {
        json!({ "flavor": flavor, "scope": scope, "path": file.rel, "bytes": bytes.len(),
                "mtime": mtime(&file.abs).0, "text": t, "next": next })
    };
    let overhead = json_len(&body("", Value::String("A".repeat(128))));
    let (slice, end) = text_slice(&text, start, PAGE_MAX_BYTES - overhead, RAW_TEXT_MAX_CHARS);
    let next = (end < text.len()).then(|| super::paging::encode_cursor(end));
    Ok(body(slice, super::paging::wrap_next(next)))
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

/// One `sv-parsed.files[]` row. GUIDs are hashed; no export text is returned.
pub fn parsed_file(f: &SvFile) -> Value {
    let bytes = read_capped(&f.abs).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let (outbox, blocked) = match parse_ledgerlink_outbox(&bytes) {
        Ok(o) => (o.entries, o.skipped.len()),
        Err(_) => (Vec::new(), 0),
    };
    let ts = |s: Option<i64>| s.map(rfc3339_utc_from_unix_seconds);
    let rows: Vec<Value> = outbox
        .iter()
        .take(MAX_OUTBOX)
        .map(|e| {
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
        })
        .collect();
    let mut row = json!({
        "scope": f.scope, "account": f.account, "mtime": mtime(&f.abs).0,
        "topKeys": top_keys(&text), "outbox": rows, "blockedCount": blocked, "region": null,
    });
    if let (Some(r), Some(c)) = (&f.realm, &f.character) {
        row["realm"] = json!(r);
        row["character"] = json!(c);
    }
    row
}
