//! `GET /dev/v1/addons` (`$defs/addons`): one row per folder under
//! `<flavor>/Interface/AddOns/`, parsed from its `.toc` header only (§7: reads
//! stay under `Interface/AddOns/*/*.toc`).

use super::paging::clip;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Header bytes read per TOC (the `##` lines sit at the top).
const TOC_HEAD_BYTES: u64 = 16 * 1024;
const MAX_FOLDERS: usize = 2000;
const MAX_SV: usize = 32;

/// Every addon row for `flavor`, sorted by folder.
pub fn addons(wow_dir: &Path, flavor: &str) -> Vec<Value> {
    let root = wow_dir.join(flavor).join("Interface").join("AddOns");
    let Ok(rd) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut folders: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| (1..=128).contains(&n.chars().count()))
        .collect();
    folders.sort();
    folders.truncate(MAX_FOLDERS);
    folders
        .iter()
        .map(|f| {
            let head = toc_path(&root.join(f), f).map(|p| read_head(&p));
            parse_toc(f, head.as_deref().unwrap_or(""))
        })
        .collect()
}

/// `<folder>.toc`, else the first `*.toc` (flavor-suffixed TOCs).
fn toc_path(dir: &Path, folder: &str) -> Option<PathBuf> {
    let exact = dir.join(format!("{folder}.toc"));
    if exact.is_file() {
        return Some(exact);
    }
    let mut tocs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toc")))
        .collect();
    tocs.sort();
    tocs.into_iter().next()
}

fn read_head(path: &Path) -> String {
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(TOC_HEAD_BYTES).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// One addon row from TOC text (`## Key: value` lines; first value wins).
pub fn parse_toc(folder: &str, text: &str) -> Value {
    let mut h: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        let Some((k, v)) = line.strip_prefix("##").and_then(|r| r.split_once(':')) else {
            continue;
        };
        h.entry(k.trim().to_ascii_lowercase())
            .or_insert_with(|| v.trim().to_owned());
    }
    let field = |k: &str, max: usize| h.get(k).map(|v| clip(v, max));
    let saved: Vec<String> = ["savedvariables", "savedvariablespercharacter"]
        .iter()
        .filter_map(|k| h.get(*k))
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| clip(s, 128))
        .take(MAX_SV)
        .collect();
    json!({
        "folder": clip(folder, 128),
        "title": field("title", 256),
        "version": field("version", 64),
        "interface": field("interface", 64),
        "savedVariables": saved,
        "loadOnDemand": h.get("loadondemand").is_some_and(|v| v == "1"),
    })
}
