//! The first-upload preview image for BG3 screenshots (ROK-1966 r2-m3). It is
//! never the original (up to 64 MiB, base64-encoded into every status read):
//! Steam's own `thumbnails/<same name>` when it sits inside the scanned folder,
//! else a small JPEG downscaled off the async runtime. Either is capped at
//! [`PREVIEW_MAX_BYTES`]; past that no image is shown (the copy still stands).

use crate::bg3_screenshot::CONTRACT;
use crate::collector::{Blob, Scan};
use crate::scope::WatchScope;
use crate::screenshot_encode::{decode_rgb, fit};
use image::codecs::jpeg::JpegEncoder;
use rl_collectors::sha256_hex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The longest side of a downscaled preview (the box is 220 px high).
pub const PREVIEW_EDGE_MAX: u32 = 512;
/// The most bytes a preview image may carry, Steam's thumbnail or ours.
pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;
/// Steam's sub-folder of small copies, next to the originals.
const STEAM_THUMBS: &str = "thumbnails";
const JPEG_QUALITY: u8 = 80;

/// Original sha256 -> its preview image (one slot: the newest screenshot).
static CACHE: Mutex<Option<(String, Blob)>> = Mutex::new(None);
/// Original sha256 -> a newest pending screenshot whose preview still has to
/// be downscaled. Keyed (not one slot) so one scan cannot drop another's.
static WANTED: Mutex<BTreeMap<String, Wanted>> = Mutex::new(BTreeMap::new());
/// Past this many queued downscales [`WANTED`] starts over.
const WANTED_MAX: usize = 8;

#[derive(Debug, Clone)]
pub struct Wanted {
    pub sha: String,
    pub path: PathBuf,
    pub scope: WatchScope,
}

/// The preview for original `sha` at `path` without decoding anything: the
/// cached one, or Steam's `thumbnails/<name>` read through `scope`.
pub fn ready(sha: &str, path: &Path, scope: &WatchScope) -> Option<Blob> {
    if let Some(b) = cached(sha) {
        return Some(b);
    }
    let thumb = path.parent()?.join(STEAM_THUMBS).join(path.file_name()?);
    let meta = scope.metadata(&thumb).ok().filter(|m| m.is_file())?;
    if usize::try_from(meta.len()).map_or(true, |n| n > PREVIEW_MAX_BYTES) {
        return None;
    }
    let bytes = scope.read(&thumb).ok()?;
    let blob = image_blob(bytes)?;
    store(sha, &blob);
    Some(blob)
}

/// Queue the screenshot [`attach`] downscales.
pub fn want(w: Wanted) {
    if let Ok(mut map) = WANTED.lock() {
        if map.len() >= WANTED_MAX {
            map.clear();
        }
        map.insert(w.sha.clone(), w);
    }
}

/// When the scan's first pending upload is the screenshot [`want`]ed, make
/// its preview on the blocking pool and attach it. A failure or a panic
/// (untrusted file) only means no image.
pub async fn attach(scan: &mut Scan) {
    let Some(first) = scan
        .pending
        .first_mut()
        .filter(|p| p.contract == CONTRACT && p.blobs.is_empty())
    else {
        return;
    };
    let Some(w) = WANTED.lock().ok().and_then(|mut m| m.remove(&first.item)) else {
        return;
    };
    let sha = w.sha.clone();
    if let Ok(Ok(blob)) = tauri::async_runtime::spawn_blocking(move || downscale(&w)).await {
        store(&sha, &blob);
        first.blobs.push(blob);
    }
}

/// Read the original through its scope and shrink it to a JPEG whose longest
/// side is at most [`PREVIEW_EDGE_MAX`]. Blocking.
pub fn downscale(w: &Wanted) -> Result<Blob, String> {
    let bytes = w.scope.read(&w.path).map_err(|e| e.to_string())?;
    let img = fit(decode_rgb(&bytes)?, PREVIEW_EDGE_MAX);
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&img)
        .map_err(|e| format!("preview encode failed ({e})"))?;
    if out.len() > PREVIEW_MAX_BYTES {
        return Err(format!("preview is {} bytes; over the cap", out.len()));
    }
    image_blob(out).ok_or_else(|| "preview is not an image".into())
}

/// `bytes` as a blob when they are an image under the cap.
fn image_blob(bytes: Vec<u8>) -> Option<Blob> {
    if bytes.len() > PREVIEW_MAX_BYTES {
        return None;
    }
    let format = image::guess_format(&bytes).ok()?;
    Some(Blob {
        sha256: sha256_hex(&bytes),
        content_type: format.to_mime_type().to_owned(),
        bytes,
    })
}

fn cached(sha: &str) -> Option<Blob> {
    let slot = CACHE.lock().ok()?;
    slot.as_ref()
        .filter(|(s, _)| s == sha)
        .map(|(_, b)| b.clone())
}

fn store(sha: &str, blob: &Blob) {
    if let Ok(mut slot) = CACHE.lock() {
        *slot = Some((sha.to_owned(), blob.clone()));
    }
}
