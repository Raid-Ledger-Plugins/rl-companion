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
//! - [`normalise_save_info`] turns `SaveInfo.json` into the `bg3-save/v1`
//!   payload; [`Bg3SaveMessage`] wraps it in the companion envelope.
//! - [`SettleWatcher`] / [`is_settled`] decide when a save file has stopped
//!   changing, from successive `(size, mtime)` samples. The crate never
//!   touches the filesystem.
//!
//! Every parser returns a typed error on malformed, truncated or
//! unsupported input; none of them panics.

#![forbid(unsafe_code)]

mod error;
mod extract;
mod lspk;
mod normalise;
mod settle;
mod time;
mod webp;

#[cfg(test)]
mod tests;

pub use error::{ExtractError, LspkError, NormaliseError, WebpError};
pub use extract::{
    SAVE_INFO_MAX_BYTES, SAVE_INFO_NAME, WEBP_MAX_BYTES, campaign_key_from_save_folder,
    extract_save_info, extract_thumbnail_webp, sha256_hex, thumbnail_ref,
};
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
