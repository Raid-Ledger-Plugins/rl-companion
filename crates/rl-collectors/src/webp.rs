//! WebP header reader: dimensions only, never pixels.
//!
//! Handles the three first-chunk forms: lossy `VP8 `, lossless `VP8L` and
//! extended `VP8X` (canvas size).

use crate::error::WebpError;

/// Read `(width, height)` from a WebP's first chunk.
pub fn webp_dimensions(data: &[u8]) -> Result<(u32, u32), WebpError> {
    let riff = data.get(0..4).ok_or(WebpError::NotWebp)?;
    let webp = data.get(8..12).ok_or(WebpError::NotWebp)?;
    if riff != b"RIFF" || webp != b"WEBP" {
        return Err(WebpError::NotWebp);
    }
    let (width, height) = match data.get(12..16).ok_or(WebpError::Truncated)? {
        b"VP8 " => vp8(data)?,
        b"VP8L" => vp8l(data)?,
        b"VP8X" => vp8x(data)?,
        other => {
            return Err(WebpError::UnsupportedChunk(
                String::from_utf8_lossy(other).into_owned(),
            ));
        }
    };
    if width == 0 || height == 0 {
        return Err(WebpError::ZeroDimension { width, height });
    }
    Ok((width, height))
}

/// Lossy: 3-byte frame tag, start code `9d 01 2a`, then 14-bit width/height.
fn vp8(data: &[u8]) -> Result<(u32, u32), WebpError> {
    let b = data.get(20..30).ok_or(WebpError::Truncated)?;
    if b[3..6] != [0x9d_u8, 0x01, 0x2a] {
        return Err(WebpError::BadVp8);
    }
    let width = u16::from_le_bytes([b[6], b[7]]) & 0x3fff;
    let height = u16::from_le_bytes([b[8], b[9]]) & 0x3fff;
    Ok((u32::from(width), u32::from(height)))
}

/// Lossless: signature `0x2f`, then 14 bits width-1 and 14 bits height-1.
fn vp8l(data: &[u8]) -> Result<(u32, u32), WebpError> {
    let b = data.get(20..25).ok_or(WebpError::Truncated)?;
    if b[0] != 0x2f {
        return Err(WebpError::BadVp8l);
    }
    let bits = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
    Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
}

/// Extended: 4 bytes flags/reserved, then 24-bit canvas width-1 and height-1.
fn vp8x(data: &[u8]) -> Result<(u32, u32), WebpError> {
    let b = data.get(20..30).ok_or(WebpError::Truncated)?;
    let width = u32::from_le_bytes([b[4], b[5], b[6], 0]) + 1;
    let height = u32::from_le_bytes([b[7], b[8], b[9], 0]) + 1;
    Ok((width, height))
}
