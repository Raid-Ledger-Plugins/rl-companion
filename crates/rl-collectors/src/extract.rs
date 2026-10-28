//! Pull `SaveInfo.json` and the WebP thumbnail out of a save package, and
//! build the hashed values the contract asks the collector for.

use sha2::{Digest, Sha256};

use crate::error::{ExtractError, WebpError};
use crate::lspk::{Entry, Package};
use crate::normalise::ThumbnailRef;
use crate::webp::webp_dimensions;

/// Name of the save summary entry at the package root.
pub const SAVE_INFO_NAME: &str = "SaveInfo.json";
/// Cap on `SaveInfo.json` (the sample is 2.6 KB).
pub const SAVE_INFO_MAX_BYTES: u64 = 1024 * 1024;
/// Cap on the thumbnail: the contract's `COMPANION_WEBP_MAX_BYTES`.
pub const WEBP_MAX_BYTES: u64 = 1_048_576;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Read and parse the root `SaveInfo.json`, stripping a leading UTF-8 BOM.
pub fn extract_save_info(package: &Package<'_>) -> Result<serde_json::Value, ExtractError> {
    let entry = package
        .find(SAVE_INFO_NAME)
        .ok_or(ExtractError::MissingSaveInfo)?;
    let bytes = package.read_entry_limited(entry, SAVE_INFO_MAX_BYTES)?;
    let json = bytes
        .strip_prefix(UTF8_BOM.as_slice())
        .unwrap_or(bytes.as_slice());
    Ok(serde_json::from_slice(json)?)
}

/// Read the root `<SaveName>.WebP` thumbnail and check its WebP header.
pub fn extract_thumbnail_webp(package: &Package<'_>) -> Result<Vec<u8>, ExtractError> {
    let entry = package
        .entries()
        .iter()
        .find(|e| is_root_webp(e))
        .ok_or(ExtractError::MissingThumbnail)?;
    let bytes = package.read_entry_limited(entry, WEBP_MAX_BYTES)?;
    webp_dimensions(&bytes)?;
    Ok(bytes)
}

fn is_root_webp(entry: &Entry) -> bool {
    let name = &entry.name;
    !name.contains('/') && !name.contains('\\') && name.to_ascii_lowercase().ends_with(".webp")
}

/// The contract's thumbnail blob ref: sha256, length, `image/webp`, size.
pub fn thumbnail_ref(webp: &[u8]) -> Result<ThumbnailRef, WebpError> {
    let byte_length = webp.len() as u64;
    if byte_length > WEBP_MAX_BYTES {
        return Err(WebpError::TooLarge {
            size: byte_length,
            limit: WEBP_MAX_BYTES,
        });
    }
    let (width, height) = webp_dimensions(webp)?;
    Ok(ThumbnailRef {
        sha256: sha256_hex(webp),
        byte_length,
        content_type: "image/webp".to_owned(),
        width,
        height,
    })
}

/// `campaignKey` for `campaignKeySource: "save-folder"` (contract §3.3): the
/// lowercase hex SHA-256 of the folder's own name (no path), UTF-8.
pub fn campaign_key_from_save_folder(folder_name: &str) -> String {
    sha256_hex(folder_name.as_bytes())
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0F)]));
    }
    out
}
