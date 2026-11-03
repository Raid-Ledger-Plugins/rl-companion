//! Screenshot re-encode (ROK-1966 Q4): decode Steam's JPEG/PNG, fit it inside
//! 4096 px, and encode a lossy WebP of at most 1 MiB — the blob store's limit.
//! Quality steps down first, then the size, until it fits; past that the
//! screenshot is refused for good.

use image::{ColorType, DynamicImage, ImageDecoder, ImageReader, Limits, RgbImage};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

/// `COMPANION_WEBP_MAX_BYTES`: the blob store's per-WebP cap.
pub const WEBP_MAX_BYTES: usize = 1_048_576;
/// `COMPANION_IMAGE_EDGE_MAX`: the longest WebP side.
pub const IMAGE_EDGE_MAX: u32 = 4096;
/// `BG3_SCREENSHOT_SOURCE_EDGE_MAX`: the longest original side the contract takes.
pub const SOURCE_EDGE_MAX: u32 = 16_384;
/// The memory budget of the whole decode pipeline, checked from the header
/// before any pixel buffer is allocated: the decoded original in its own
/// colour type, its downscaled copy, and the RGB copy that is encoded. The
/// downscale is an area average straight into the output buffer (no
/// full-size float intermediate), so these three buffers are the peak. Every
/// real capture fits (8K RGB is about 160 MiB); a 16384 x 16384 header, even
/// grayscale, does not.
pub const PIPELINE_BUDGET: u64 = 256 * 1024 * 1024;
/// Total WebP encodes one screenshot may cost before it is refused.
pub const MAX_ENCODES: usize = 8;
/// Qualities tried at each size, best first.
const QUALITIES: [f32; 3] = [80.0, 65.0, 50.0];
/// Downscale rounds before giving up.
const MAX_ROUNDS: usize = 6;
/// Never shrink a side below this.
const MIN_EDGE: u32 = 64;

/// One re-encoded screenshot.
#[derive(Debug, Clone)]
pub struct Encoded {
    pub webp: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The original capture's size.
    pub source_width: u32,
    pub source_height: u32,
}

/// The original's size from its header, without decoding pixels.
pub fn source_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("not an image ({e})"))?
        .into_dimensions()
        .map_err(|e| format!("not a readable image ({e})"))
}

/// Re-encode `bytes` (JPEG or PNG) to a WebP of at most [`WEBP_MAX_BYTES`].
pub fn encode_screenshot(bytes: &[u8]) -> Result<Encoded, String> {
    let (sw, sh) = source_dimensions(bytes)?;
    if sw == 0 || sh == 0 || sw > SOURCE_EDGE_MAX || sh > SOURCE_EDGE_MAX {
        return Err(format!("{sw}x{sh} is outside 1-{SOURCE_EDGE_MAX} px"));
    }
    let webp = fit_under(decode_fit(bytes, IMAGE_EDGE_MAX)?, WEBP_MAX_BYTES)?;
    Ok(Encoded {
        width: webp.1,
        height: webp.2,
        webp: webp.0,
        source_width: sw,
        source_height: sh,
    })
}

/// Decode `bytes` (JPEG or PNG) to RGB with its longest side at most
/// `max_edge`, refusing up front when [`pipeline_bytes`] is over
/// [`PIPELINE_BUDGET`]. The decoder carries the same limits as a backstop.
pub fn decode_fit(bytes: &[u8], max_edge: u32) -> Result<RgbImage, String> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(SOURCE_EDGE_MAX);
    limits.max_image_height = Some(SOURCE_EDGE_MAX);
    limits.max_alloc = Some(PIPELINE_BUDGET);
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("not an image ({e})"))?;
    reader.limits(limits);
    let decoder = reader
        .into_decoder()
        .map_err(|e| format!("not a readable image ({e})"))?;
    let ((w, h), color) = (decoder.dimensions(), decoder.color_type());
    let need = pipeline_bytes((w, h), color, max_edge);
    if need > PIPELINE_BUDGET {
        return Err(format!(
            "{w}x{h} {color:?} needs {need} bytes to decode, over the {PIPELINE_BUDGET}-byte memory budget"
        ));
    }
    let img =
        DynamicImage::from_decoder(decoder).map_err(|e| format!("not a readable image ({e})"))?;
    let edge = w.max(h);
    if edge <= max_edge {
        return Ok(img.into_rgb8());
    }
    let (tw, th) = scaled((w, h), f64::from(max_edge) / f64::from(edge));
    Ok(img.thumbnail_exact(tw, th).into_rgb8())
}

/// Peak bytes [`decode_fit`] holds for a `w` x `h` original of `color`
/// fitted inside `max_edge`: the decoded original, its downscaled copy in
/// the same colour type, and the RGB copy.
pub fn pipeline_bytes((w, h): (u32, u32), color: ColorType, max_edge: u32) -> u64 {
    let bpp = u64::from(color.bytes_per_pixel());
    let area = |(a, b): (u32, u32)| u64::from(a) * u64::from(b);
    let edge = w.max(h);
    if edge <= max_edge {
        return area((w, h)) * (bpp + 3);
    }
    let small = area(scaled((w, h), f64::from(max_edge) / f64::from(edge)));
    area((w, h)) * bpp + small * (bpp + 3)
}

/// Read at most `cap` bytes of `path`; a longer file is refused without
/// reading past `cap + 1` bytes (it may have grown since it was measured).
pub fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    file.take(cap.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > cap {
        return Err(format!("{} is over the {cap}-byte cap", path.display()));
    }
    Ok(out)
}

/// Encode `img`, stepping quality then size down until it is ≤ `max` bytes,
/// in at most [`MAX_ENCODES`] encodes. Returns the WebP and its size.
pub fn fit_under(mut img: RgbImage, max: usize) -> Result<(Vec<u8>, u32, u32), String> {
    let mut encodes = 0;
    for _ in 0..MAX_ROUNDS {
        let mut smallest = usize::MAX;
        for q in QUALITIES {
            if encodes == MAX_ENCODES {
                return Err(format!("could not fit a WebP under {max} bytes"));
            }
            encodes += 1;
            let out = webp_lossy(&img, q)?;
            if out.len() <= max {
                return Ok((out, img.width(), img.height()));
            }
            smallest = smallest.min(out.len());
        }
        // Bytes scale roughly with area: aim a little under the cap.
        let factor = ((max as f64 / smallest as f64).sqrt() * 0.9).clamp(0.3, 0.9);
        let (w, h) = scaled(img.dimensions(), factor);
        if w < MIN_EDGE || h < MIN_EDGE {
            break;
        }
        // Area average into the output buffer: no full-size float copy.
        img = image::imageops::thumbnail(&img, w, h);
    }
    Err(format!("could not fit a WebP under {max} bytes"))
}

/// `(w, h) × factor`, rounded down, at least 1 px.
fn scaled((w, h): (u32, u32), factor: f64) -> (u32, u32) {
    let side = |v: u32| ((f64::from(v) * factor).floor() as u32).max(1);
    (side(w), side(h))
}

fn webp_lossy(img: &RgbImage, quality: f32) -> Result<Vec<u8>, String> {
    webp::Encoder::from_rgb(img.as_raw(), img.width(), img.height())
        .encode_simple(false, quality)
        .map(|m| m.to_vec())
        .map_err(|e| format!("WebP encode failed ({e:?})"))
}

#[cfg(test)]
#[path = "tests/screenshot_image_budget_tests.rs"]
mod budget_tests;
