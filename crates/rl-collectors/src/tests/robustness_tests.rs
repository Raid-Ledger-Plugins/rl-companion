//! Malformed input must come back as an error, never a panic. These run
//! every parser over every truncation and thousands of corruptions of a
//! valid synthetic save.

use super::fixtures::{PLACEHOLDER_WEBP, act2, synthetic_save};
use crate::{extract_save_info, extract_thumbnail_webp, read_lspk, webp_dimensions};

fn exercise(bytes: &[u8]) {
    if let Ok(package) = read_lspk(bytes) {
        let _ = extract_save_info(&package);
        let _ = extract_thumbnail_webp(&package);
        for entry in package.entries() {
            let _ = package.read_entry_limited(entry, 4 * 1024 * 1024);
        }
    }
}

/// Deterministic 64-bit LCG (Knuth MMIX constants).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 16
    }
}

rl_test! {
    fn every_truncation_of_a_save_fails_cleanly() {
        let full = synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP));
        for len in 0..full.len() {
            // The file table is stored last, so every strict prefix loses it.
            assert!(read_lspk(&full[..len]).is_err(), "a {len}-byte prefix parsed");
            exercise(&full[..len]);
        }
        assert!(read_lspk(&full).is_ok());
    }
}

rl_test! {
    fn random_corruption_never_panics() {
        let full = synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP));
        let table_start = full.len().saturating_sub(400);
        let mut rng = Lcg(0x9E37_79B9_7F4A_7C15);
        for round in 0..4000 {
            let mut bytes = full.clone();
            let r = rng.next();
            // Half anywhere; half in the header or the file table.
            let at = if round % 2 == 0 {
                (r as usize) % bytes.len()
            } else if round % 4 == 1 {
                (r as usize) % 40
            } else {
                table_start + (r as usize) % (bytes.len() - table_start)
            };
            bytes[at] ^= ((r >> 24) as u8) | 1;
            exercise(&bytes);
        }
    }
}

rl_test! {
    fn webp_header_reader_survives_every_truncation() {
        for len in 0..64 {
            let _ = webp_dimensions(&PLACEHOLDER_WEBP[..len]);
        }
    }
}
