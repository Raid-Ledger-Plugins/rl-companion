//! Raid Ledger Companion collectors.
//!
//! Pure functions over bytes, so the same crate runs natively (inside the
//! Tauri companion) and as WASM (the web drop-zone):
//!
//! - [`read_lspk`] reads a Larian LSPK v18 package (a BG3 `.lsv` save):
//!   header, LZ4-compressed file table, and lazily decompressed entries
//!   (stored, LZ4 or zstd).
//! - [`extract_save_info`] and [`extract_thumbnail_webp`] pull the two
//!   entries the `bg3-save/v1` contract needs.
//! - [`read_lsf_attribute`] reads one attribute out of an LSF v7 resource;
//!   [`campaign_key_from_package`] uses it to key a save by its `meta.lsf`
//!   GameID, falling back to the save-folder hash.
//! - [`normalise_save_info`] turns `SaveInfo.json` into the `bg3-save/v1`
//!   payload; [`Bg3SaveMessage`] wraps it in the companion envelope.
//! - [`SettleWatcher`] / [`is_settled`] decide when a save file has stopped
//!   changing, from successive `(size, mtime)` samples.
//! - [`parse_ledgerlink_outbox`] reads the LedgerLink addon's
//!   `LedgerLinkDB.outbox` from a WoW SavedVariables file;
//!   [`changed_entries`] and [`ledgerlink_sv_candidates`] support the
//!   watcher. The crate never touches the filesystem.
//!
//! Every parser returns a typed error on malformed, truncated or
//! unsupported input; none of them panics.

#![forbid(unsafe_code)]

mod error;
mod extract;
mod lsf;
mod lspk;
mod newage;
mod normalise;
mod settle;
mod time;
mod webp;
mod wow_sv;

#[cfg(test)]
mod tests;

pub use error::{ExtractError, LsfError, LspkError, NormaliseError, WebpError, WowSvError};
pub use extract::{
    META_LSF_MAX_BYTES, META_LSF_NAME, SAVE_INFO_MAX_BYTES, SAVE_INFO_NAME, WEBP_MAX_BYTES,
    campaign_key_from_package, campaign_key_from_save_folder, extract_save_info,
    extract_thumbnail_webp, game_id_from_package, sha256_hex, thumbnail_ref,
};
pub use lsf::{
    LSF_MAGIC, LSF_MAX_BLOB_SECTION_BYTES, LSF_MAX_SECTION_BYTES, LSF_MAX_STRINGS, LSF_VERSION,
    LZ4_FRAME_MAX_BLOCKS, lsf_attribute_len, lsf_version, read_lsf_attribute,
    read_lsf_attribute_bytes,
};
pub use newage::{LSMF_MAGIC, LSMF_MAX_BYTES, Lsmf, LsmfComponent, LsmfError, count_webp_images};
pub use lspk::{
    CompressionMethod, DEFAULT_MAX_ENTRY_BYTES, Entry, LSPK_VERSION, MAX_FILES, Package, read_lspk,
};
pub use normalise::{
    Bg3Class, Bg3SaveMessage, Bg3SavePayload, COLLECTOR_VERSION, CONTRACT, CONTRACT_VERSION,
    CampaignKeySource, ENVELOPE_VERSION, PartyMember, SaveMeta, ThumbnailRef, normalise_save_info,
};
pub use settle::{DEFAULT_STABLE_SAMPLES, FileSample, SettleWatcher, is_settled};
pub use time::rfc3339_utc_from_unix_seconds;
pub use webp::webp_dimensions;
pub use wow_sv::{
    DEV_SNAPSHOT_MAX_BYTES, DevSnapshotEntry, DevSnapshots, LEDGERLINK_DB, LEDGERLINK_SV_FILE,
    MAX_EXPORT_TEXT_BYTES, MAX_SV_BYTES, Outbox, OutboxEntry, SkipReason, SkippedEntry,
    SvCandidate, changed_entries, is_flavor_folder, is_player_guid, is_valid_page_token,
    ledgerlink_sv_candidates, ledgerlink_sv_path, parse_ledgerlink_dev_snapshots,
    parse_ledgerlink_outbox, validate_export_text,
};
