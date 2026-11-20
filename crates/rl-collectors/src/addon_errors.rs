//! WoW addon errors collector core (ROK-2099 S5): the `wow-addon-errors/v1`
//! contract (`contracts/wow-addon-errors/v1/`).
//!
//! BugGrabber keeps the errors it caught in `BugGrabberDB.errors`, written to
//! `WTF/Account/<ACCOUNT>/SavedVariables/!BugGrabber.lua`. [`scan_buggrabber`]
//! parses that file's bytes, never reads `locals`, scrubs paths and the
//! account's own character/realm names (contract §4), and merges entries that
//! scrub to the same `(addon, message, stack)`. [`AddonErrorsState`] keeps the
//! per-file dedupe state so an unchanged file yields nothing, and
//! [`build_messages`] splits the new records into messages by count and by
//! the 262 144-byte body cap. No filesystem access, no network.

mod batch;
mod parse;
mod scrub;
mod state;
mod text;

use serde::{Deserialize, Serialize};

use crate::extract::sha256_hex;
use crate::normalise::{COLLECTOR_VERSION, ENVELOPE_VERSION};

pub use batch::{AddonErrorsError, PayloadHeader, build_messages, toc_version};
pub use parse::{BugGrabberScan, ScrubContext, TimeContext, scan_buggrabber};
pub use scrub::{
    has_unscrubbed_home, has_unscrubbed_path, scrub_battletags, scrub_names, scrub_paths,
};
pub use state::AddonErrorsState;

/// `contract` of the envelope.
pub const ADDON_ERRORS_CONTRACT: &str = "wow-addon-errors";
/// `contractVersion` of the envelope.
pub const ADDON_ERRORS_CONTRACT_VERSION: u32 = 1;
/// The only `source` in v1.
pub const ADDON_ERRORS_SOURCE: &str = "buggrabber";
/// BugGrabber's account-wide SavedVariables file name.
pub const BUGGRABBER_SV_FILE: &str = "!BugGrabber.lua";
/// BugGrabber's SavedVariables table.
pub const BUGGRABBER_DB: &str = "BugGrabberDB";
/// `records` cap per message.
pub const ADDON_ERRORS_MAX_RECORDS: usize = 50;
/// The server's body cap (`COMPANION_WOW_ADDON_ERRORS_MAX_BYTES`).
pub const ADDON_ERRORS_MAX_BYTES: usize = 262_144;
/// `message` cap, UTF-16 units.
pub const ADDON_ERROR_MESSAGE_MAX: usize = 1024;
/// `stack` cap, UTF-16 units.
pub const ADDON_ERROR_STACK_MAX: usize = 4096;
/// `addon` cap, UTF-16 units.
pub const ADDON_ERROR_ADDON_MAX: usize = 64;
/// `count` cap.
pub const ADDON_ERROR_COUNT_MAX: u32 = 1_000_000;
/// Appended to a truncated `message` / `stack`.
pub const TRUNCATED_MARKER: &str = "…[truncated]";

/// One scrubbed error record (`WowAddonErrorRecordSchema`). No `locals`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddonErrorRecord {
    pub addon: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
}

impl AddonErrorRecord {
    /// Dedupe key: SHA-256 of `addon \0 message \0 stack` (the contract's
    /// uniqueness tuple; NUL never survives scrubbing).
    pub fn key(&self) -> String {
        let stack = self.stack.as_deref().unwrap_or("");
        sha256_hex(format!("{}\0{}\0{}", self.addon, self.message, stack).as_bytes())
    }
}

/// The uploader's own character (`characterHint`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterHint {
    pub name: String,
    pub realm: String,
}

/// `WowAddonErrorsPayloadSchema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddonErrorsPayload {
    pub source: String,
    pub source_version: String,
    pub account_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character_hint: Option<CharacterHint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wow_flavor: Option<String>,
    pub records: Vec<AddonErrorRecord>,
}

/// One `wow-addon-errors` envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddonErrorsMessage {
    pub envelope_version: u32,
    pub contract: String,
    pub contract_version: u32,
    pub captured_at: String,
    pub app_version: String,
    pub collector_version: String,
    pub payload: AddonErrorsPayload,
}

impl AddonErrorsMessage {
    /// Wrap a payload; `collectorVersion` is this crate's version.
    pub fn new(
        payload: AddonErrorsPayload,
        captured_at: impl Into<String>,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            envelope_version: ENVELOPE_VERSION,
            contract: ADDON_ERRORS_CONTRACT.to_owned(),
            contract_version: ADDON_ERRORS_CONTRACT_VERSION,
            captured_at: captured_at.into(),
            app_version: app_version.into(),
            collector_version: COLLECTOR_VERSION.to_owned(),
            payload,
        }
    }
}
