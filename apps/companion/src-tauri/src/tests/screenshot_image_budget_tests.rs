//! ROK-1966 Codex fixes: the decode pipeline is budgeted from the header
//! before any pixel buffer exists, Steam thumbnails are dimension-checked
//! before the WebView sees them, and re-reads of an original are bounded.

use crate::scope::WatchScope;
use crate::screenshot_encode::{
    decode_fit, encode_screenshot, pipeline_bytes, read_capped, IMAGE_EDGE_MAX, PIPELINE_BUDGET,
};
use crate::screenshot_preview::{downscale, ready, Wanted, PREVIEW_EDGE_MAX};
use image::{ColorType, ImageEncoder, RgbImage};
use std::path::PathBuf;

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A PNG of a few dozen bytes whose header declares a `w` x `h` 8-bit
/// grayscale canvas (the "decompression bomb" shape: tiny file, huge image).
fn bomb_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &[0x78, 0x01]);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn real_png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 7])
    });
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .expect("png");
    out
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("rl-budget-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("thumbnails")).expect("mkdir");
    dir.canonicalize().expect("canonical")
}

#[test]
fn a_huge_grayscale_header_is_refused_before_any_pixel_buffer() {
    let bomb = bomb_png(16_384, 16_384);
    assert!(bomb.len() < 100, "fixture is tiny: {} bytes", bomb.len());
    let err = encode_screenshot(&bomb).expect_err("over the budget");
    assert!(
        err.contains("memory budget"),
        "expected a memory-budget refusal, got: {err}"
    );
    let err = decode_fit(&bomb, PREVIEW_EDGE_MAX).expect_err("over the budget");
    assert!(err.contains("memory budget"), "preview path: {err}");
}

#[test]
fn the_budget_counts_decode_downscale_and_rgb_copy() {
    let gray = pipeline_bytes((16_384, 16_384), ColorType::L8, IMAGE_EDGE_MAX);
    assert!(gray > PIPELINE_BUDGET, "16384^2 gray needs {gray}");
    let eight_k = pipeline_bytes((7680, 4320), ColorType::Rgb8, IMAGE_EDGE_MAX);
    assert!(eight_k <= PIPELINE_BUDGET, "8K RGB needs {eight_k}");
    // 7680x4320x3 + 4096x2304x(3+3)
    assert_eq!(eight_k, 7680 * 4320 * 3 + 4096 * 2304 * 6);
}

#[test]
fn a_wide_real_image_is_fitted_without_a_full_size_float_copy() {
    let img = decode_fit(&real_png(5000, 20), IMAGE_EDGE_MAX).expect("fits");
    assert_eq!(img.dimensions(), (4096, 16));
}

#[test]
fn a_tiny_thumbnail_declaring_a_huge_canvas_is_not_handed_to_the_webview() {
    let dir = temp_dir("thumb");
    let scope = WatchScope::new(&[&dir]);
    let path = dir.join("bomb.png");
    std::fs::write(&path, real_png(64, 40)).expect("original");
    std::fs::write(
        dir.join("thumbnails").join("bomb.png"),
        bomb_png(16_384, 16_384),
    )
    .expect("thumb");
    assert!(
        ready("budget-sha-bomb", &path, &scope).is_none(),
        "a thumbnail whose header is over {PREVIEW_EDGE_MAX} px must not be attached raw"
    );
    let ok = dir.join("ok.png");
    std::fs::write(&ok, real_png(64, 40)).expect("original");
    std::fs::write(dir.join("thumbnails").join("ok.png"), real_png(30, 20)).expect("thumb");
    assert!(
        ready("budget-sha-ok", &ok, &scope).is_some(),
        "a small thumbnail still is"
    );
    // The queued downscale normalises the original instead: a bounded JPEG.
    let w = Wanted {
        sha: "budget-sha-bomb".into(),
        path,
        scope,
    };
    let blob = downscale(&w).expect("downscaled");
    assert_eq!(blob.content_type, "image/jpeg");
}

#[test]
fn rereads_of_an_original_are_bounded() {
    let dir = temp_dir("cap");
    let path = dir.join("grown.png");
    std::fs::write(&path, [0u8; 11]).expect("write");
    let err = read_capped(&path, 10).expect_err("11 bytes over a 10-byte cap");
    assert!(err.contains("over the 10-byte cap"), "got: {err}");
    assert_eq!(read_capped(&path, 11).map(|b| b.len()), Ok(11));
    std::fs::write(&path, bomb_png(16_384, 16_384)).expect("write");
    let w = Wanted {
        sha: "budget-sha-grown".into(),
        path,
        scope: WatchScope::new(&[&dir]),
    };
    let err = downscale(&w).expect_err("a bomb original is refused");
    assert!(err.contains("memory budget"), "got: {err}");
}
