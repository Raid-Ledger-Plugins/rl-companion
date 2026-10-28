//! The pinned `bg3-save/v1` contract fixtures, compiled in, plus builders
//! for synthetic `.lsv` packages.

use serde_json::Value;

use super::writer::{FileSpec, Method, write_lspk};
use crate::{
    Bg3SaveMessage, Bg3SavePayload, SaveMeta, extract_save_info, extract_thumbnail_webp,
    normalise_save_info, read_lspk, thumbnail_ref,
};

pub(crate) const SCHEMA: &str = include_str!("../../../../contracts/bg3-save/v1/schema.json");

/// Synthetic flat-grey 1280×720 WebP from the contract (no real save data).
pub(crate) const PLACEHOLDER_WEBP: &[u8] =
    include_bytes!("../../../../contracts/bg3-save/v1/fixtures/placeholder-thumbnail.webp");

/// One golden pair: raw `SaveInfo.json` → expected message.
pub(crate) struct GoldenCase {
    pub(crate) name: &'static str,
    pub(crate) saveinfo: &'static str,
    pub(crate) golden: &'static str,
}

pub(crate) static CASES: [GoldenCase; 4] = [
    GoldenCase {
        name: "act2-party-4p-summon",
        saveinfo: include_str!(
            "../../../../contracts/bg3-save/v1/fixtures/saveinfo/act2-party-4p-summon.saveinfo.json"
        ),
        golden: include_str!("../../../../contracts/bg3-save/v1/fixtures/act2-party-4p-summon.json"),
    },
    GoldenCase {
        name: "act1-solo-no-thumbnail",
        saveinfo: include_str!(
            "../../../../contracts/bg3-save/v1/fixtures/saveinfo/act1-solo-no-thumbnail.saveinfo.json"
        ),
        golden: include_str!("../../../../contracts/bg3-save/v1/fixtures/act1-solo-no-thumbnail.json"),
    },
    GoldenCase {
        name: "ruleset-null",
        saveinfo: include_str!(
            "../../../../contracts/bg3-save/v1/fixtures/saveinfo/ruleset-null.saveinfo.json"
        ),
        golden: include_str!("../../../../contracts/bg3-save/v1/fixtures/ruleset-null.json"),
    },
    GoldenCase {
        name: "unicode-save-name",
        saveinfo: include_str!(
            "../../../../contracts/bg3-save/v1/fixtures/saveinfo/unicode-save-name.saveinfo.json"
        ),
        golden: include_str!("../../../../contracts/bg3-save/v1/fixtures/unicode-save-name.json"),
    },
];

/// The case whose golden thumbnail is [`PLACEHOLDER_WEBP`].
pub(crate) fn act2() -> &'static GoldenCase {
    &CASES[0]
}

pub(crate) fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is JSON")
}

/// The collector-supplied facts, read back from a golden message.
pub(crate) fn meta_from_golden(golden: &Value) -> SaveMeta {
    let p = &golden["payload"];
    SaveMeta {
        saved_at: p["savedAt"].as_str().expect("savedAt").to_owned(),
        campaign_key: p["campaignKey"].as_str().expect("campaignKey").to_owned(),
        campaign_key_source: serde_json::from_value(p["campaignKeySource"].clone())
            .expect("campaignKeySource"),
        thumbnail: serde_json::from_value(p["thumbnail"].clone()).expect("thumbnail"),
    }
}

/// Wrap a payload with the golden message's envelope values.
pub(crate) fn message_from_golden(payload: Bg3SavePayload, golden: &Value) -> Bg3SaveMessage {
    let mut message = Bg3SaveMessage::new(
        payload,
        golden["capturedAt"].as_str().expect("capturedAt"),
        golden["appVersion"].as_str().expect("appVersion"),
    );
    message.collector_version = golden["collectorVersion"]
        .as_str()
        .expect("collectorVersion")
        .to_owned();
    message
}

/// A synthetic save: BOM-prefixed `SaveInfo.json` (zstd), optional WebP
/// thumbnail (zstd), a placeholder `meta.lsf` (LZ4) and a stored level cache.
pub(crate) fn synthetic_save(saveinfo: &str, webp: Option<&[u8]>) -> Vec<u8> {
    let mut info = vec![0xEF, 0xBB, 0xBF];
    info.extend_from_slice(saveinfo.as_bytes());
    let meta_lsf: &[u8] = b"LSOF synthetic meta placeholder LSOF synthetic meta placeholder";
    let level = vec![0x5A_u8; 300];
    let mut files = vec![
        FileSpec { name: "SaveInfo.json", data: &info, method: Method::ZstdRaw },
        FileSpec { name: "meta.lsf", data: meta_lsf, method: Method::Lz4 },
        FileSpec { name: "LevelCache/WLD_Main_A.lsf", data: &level, method: Method::Stored },
    ];
    if let Some(webp) = webp {
        files.insert(1, FileSpec { name: "QuickSave_1.WebP", data: webp, method: Method::ZstdRaw });
    }
    write_lspk(&files)
}

/// The full collector path: package bytes → payload, with the golden
/// message's savedAt / campaignKey and a thumbnail ref computed here.
pub(crate) fn pipeline(bytes: &[u8], golden: &Value) -> Bg3SavePayload {
    let package = read_lspk(bytes).expect("package parses");
    let raw = extract_save_info(&package).expect("SaveInfo.json extracts");
    let webp = extract_thumbnail_webp(&package).expect("thumbnail extracts");
    let mut meta = meta_from_golden(golden);
    meta.thumbnail = Some(thumbnail_ref(&webp).expect("thumbnail ref"));
    normalise_save_info(&raw, &meta).expect("normalises")
}
