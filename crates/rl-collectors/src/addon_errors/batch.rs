//! Payload header validation and the count + size batch split (contract §3).

use super::scrub::has_unscrubbed_path;
use super::text::{is_one_line, one_line};
use super::{
    ADDON_ERRORS_MAX_BYTES, ADDON_ERRORS_MAX_RECORDS, ADDON_ERRORS_SOURCE, AddonErrorRecord,
    AddonErrorsMessage, AddonErrorsPayload, CharacterHint,
};
use crate::extract::hmac_sha256_hex;
use crate::wow_sv::is_flavor_folder;

/// A header field the contract would refuse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AddonErrorsError {
    #[error("invalid {field} for wow-addon-errors/v1")]
    InvalidField { field: &'static str },
    #[error("one record serialises to {size} bytes, over the message cap")]
    RecordTooLarge { size: usize },
}

/// Everything in the payload except `records`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadHeader {
    pub source_version: String,
    pub account_key: String,
    pub character_hint: Option<CharacterHint>,
    pub wow_flavor: Option<String>,
}

impl PayloadHeader {
    /// Validate the header; `account_folder` (the raw `WTF/Account/<folder>`
    /// name) is only ever sent as HMAC-SHA256 keyed with `install_key` — 32
    /// random bytes kept only on this machine — so the low-entropy folder
    /// name cannot be recovered by hashing guesses (review D1).
    pub fn new(
        source_version: &str,
        account_folder: &str,
        install_key: &[u8; 32],
        character_hint: Option<CharacterHint>,
        wow_flavor: Option<&str>,
    ) -> Result<Self, AddonErrorsError> {
        let bad = |field| AddonErrorsError::InvalidField { field };
        if !is_one_line(source_version, 32) || has_unscrubbed_path(source_version) {
            return Err(bad("sourceVersion"));
        }
        if account_folder.is_empty() {
            return Err(bad("accountKey"));
        }
        if character_hint
            .as_ref()
            .is_some_and(|h| !is_one_line(&h.name, 24) || !is_one_line(&h.realm, 48))
        {
            return Err(bad("characterHint"));
        }
        if wow_flavor.is_some_and(|f| f.len() > 32 || !is_flavor_folder(f)) {
            return Err(bad("wowFlavor"));
        }
        Ok(Self {
            source_version: source_version.to_owned(),
            account_key: hmac_sha256_hex(install_key, account_folder.as_bytes()),
            character_hint,
            wow_flavor: wow_flavor.map(str::to_owned),
        })
    }
}

/// `## Version:` from BugGrabber's `.toc`, when it is valid `sourceVersion`.
pub fn toc_version(toc: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(toc);
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("## Version:"))?;
    one_line(line, 32).filter(|v| !has_unscrubbed_path(v))
}

/// Split `records` into messages of ≤ 50 records whose compact JSON stays
/// within the 262 144-byte body cap. Empty `records` → no message.
pub fn build_messages(
    header: &PayloadHeader,
    records: Vec<AddonErrorRecord>,
    captured_at: &str,
    app_version: &str,
) -> Result<Vec<AddonErrorsMessage>, AddonErrorsError> {
    let wrap = |records| {
        let payload = AddonErrorsPayload {
            source: ADDON_ERRORS_SOURCE.to_owned(),
            source_version: header.source_version.clone(),
            account_key: header.account_key.clone(),
            character_hint: header.character_hint.clone(),
            wow_flavor: header.wow_flavor.clone(),
            records,
        };
        AddonErrorsMessage::new(payload, captured_at, app_version)
    };
    let base = json_len(&wrap(Vec::new()));
    let mut out = Vec::new();
    let (mut batch, mut size): (Vec<AddonErrorRecord>, usize) = (Vec::new(), base);
    for record in records {
        let add = json_len(&record) + usize::from(!batch.is_empty());
        if base + json_len(&record) > ADDON_ERRORS_MAX_BYTES {
            return Err(AddonErrorsError::RecordTooLarge {
                size: json_len(&record),
            });
        }
        if batch.len() == ADDON_ERRORS_MAX_RECORDS || size + add > ADDON_ERRORS_MAX_BYTES {
            out.push(wrap(std::mem::take(&mut batch)));
            size = base;
        }
        size += json_len(&record) + usize::from(!batch.is_empty());
        batch.push(record);
    }
    if !batch.is_empty() {
        out.push(wrap(batch));
    }
    Ok(out)
}

fn json_len<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX / 2, |v| v.len())
}
