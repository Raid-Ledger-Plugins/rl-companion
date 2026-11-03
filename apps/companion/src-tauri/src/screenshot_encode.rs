//! Screenshot re-encode (ROK-1966 Q4): decode Steam's JPEG/PNG, fit it inside
//! 4096 px, and encode a lossy WebP of at most 1 MiB — the blob store's limit.
//! Quality steps down first, then the size, until it fits; past that the
//! screenshot is refused for good.

use image::imageops::FilterType;
use image::{ImageReader, Limits, RgbImage};
use std::io::Cursor;

/// `COMPANION_WEBP_MAX_BYTES`: the blob store's per-WebP cap.
pub const WEBP_MAX_BYTES: usize = 1_048_576;
/// `COMPANION_IMAGE_EDGE_MAX`: the longest WebP side.
pub const IMAGE_EDGE_MAX: u32 = 4096;
/// `BG3_SCREENSHOT_SOURCE_EDGE_MAX`: the longest original side the contract takes.
pub const SOURCE_EDGE_MAX: u32 = 16_384;
/// The decoder's allocation budget. A source inside [`SOURCE_EDGE_MAX`] whose
/// decoded pixels need more than this is refused before any pixel buffer is
/// allocated: RGBA up to 8192 x 8192 (RGB about 9450 x 9450) fits, so every
/// real 4K/8K capture does, while a 16384 x 16384 header does not. The RGBA
/// to RGB copy afterwards adds at most 3/4 of it, so peak memory stays under
/// about 450 MiB.
pub const DECODE_ALLOC_MAX: u64 = 256 * 1024 * 1024;
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
    let decoded = decode_rgb(bytes)?;
    let webp = fit_under(fit(decoded, IMAGE_EDGE_MAX), WEBP_MAX_BYTES)?;
    Ok(Encoded {
        width: webp.1,
        height: webp.2,
        webp: webp.0,
        source_width: sw,
        source_height: sh,
    })
}

/// Decode `bytes` (JPEG or PNG) to RGB inside [`SOURCE_EDGE_MAX`] and
/// [`DECODE_ALLOC_MAX`].
pub fn decode_rgb(bytes: &[u8]) -> Result<RgbImage, String> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(SOURCE_EDGE_MAX);
    limits.max_image_height = Some(SOURCE_EDGE_MAX);
    limits.max_alloc = Some(DECODE_ALLOC_MAX);
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("not an image ({e})"))?;
    reader.limits(limits);
    Ok(reader
        .decode()
        .map_err(|e| format!("not a readable image ({e})"))?
        .into_rgb8())
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
        img = image::imageops::resize(&img, w, h, FilterType::Triangle);
    }
    Err(format!("could not fit a WebP under {max} bytes"))
}

/// `img` scaled down so its longest side is at most `max` (never up).
pub fn fit(img: RgbImage, max: u32) -> RgbImage {
    let edge = img.width().max(img.height());
    if edge <= max {
        return img;
    }
    let (w, h) = scaled(img.dimensions(), f64::from(max) / f64::from(edge));
    image::imageops::resize(&img, w, h, FilterType::Triangle)
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
