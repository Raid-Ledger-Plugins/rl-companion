//! Pull `SaveInfo.json` and the WebP thumbnail out of a save package, and
//! build the hashed values the contract asks the collector for.

use sha2::{Digest, Sha256};

use crate::error::{ExtractError, WebpError};
use crate::lsf::read_lsf_attribute;
use crate::lspk::{Entry, Package};
use crate::normalise::{CampaignKeySource, ThumbnailRef};
use crate::webp::webp_dimensions;

/// Name of the save summary entry at the package root.
pub const SAVE_INFO_NAME: &str = "SaveInfo.json";
/// Cap on `SaveInfo.json` (the sample is 2.6 KB).
pub const SAVE_INFO_MAX_BYTES: u64 = 1024 * 1024;
/// Cap on the thumbnail: the contract's `COMPANION_WEBP_MAX_BYTES`.
pub const WEBP_MAX_BYTES: u64 = 1_048_576;
/// Name of the save metadata resource at the package root.
pub const META_LSF_NAME: &str = "meta.lsf";
/// Cap on `meta.lsf` (the real one is ~1.7 KB; spec D4).
pub const META_LSF_MAX_BYTES: u64 = 64 * 1024;
/// The `meta.lsf` attribute that names one playthrough.
const GAME_ID_ATTRIBUTE: &str = "GameID";
/// Longest GameID text accepted (spec D4).
const GAME_ID_MAX_BYTES: usize = 128;

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

/// The save's GameID text (contract §3.3, ROK-2070 D3): `None` when
/// `meta.lsf` is missing, over [`META_LSF_MAX_BYTES`], unreadable, has no
/// GameID, or the GameID is not 1 to 128 printable ASCII bytes.
pub fn game_id_from_package(package: &Package<'_>) -> Option<String> {
    let entry = package.find(META_LSF_NAME)?;
    let bytes = package.read_entry_limited(entry, META_LSF_MAX_BYTES).ok()?;
    let id = read_lsf_attribute(&bytes, GAME_ID_ATTRIBUTE).ok()??;
    let printable = id.bytes().all(|b| (0x20..=0x7E).contains(&b));
    (printable && (1..=GAME_ID_MAX_BYTES).contains(&id.len())).then_some(id)
}

/// `campaignKey` and its source for one save (ROK-2070 D2): the SHA-256 of
/// the GameID (`game-id`) when it can be read, else the folder-name hash
/// (`save-folder`). Infallible: an unreadable `meta.lsf` never fails an upload.
pub fn campaign_key_from_package(
    package: &Package<'_>,
    folder_name: &str,
) -> (String, CampaignKeySource) {
    match game_id_from_package(package) {
        Some(id) => (sha256_hex(id.as_bytes()), CampaignKeySource::GameId),
        None => (
            campaign_key_from_save_folder(folder_name),
            CampaignKeySource::SaveFolder,
        ),
    }
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
