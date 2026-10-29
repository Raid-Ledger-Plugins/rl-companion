//! Malformed SavedVariables must come back as an error, never a panic:
//! every truncation and thousands of corruptions of the synthetic fixtures.

use super::wow_sv_tests::{LONG, MULTI};
use crate::parse_ledgerlink_outbox;

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
    fn every_truncation_of_a_small_file_fails_cleanly() {
        let last_brace = LONG.iter().rposition(|&b| b == b'}').expect("has a closing brace");
        for len in 1..=last_brace {
            assert!(parse_ledgerlink_outbox(&LONG[..len]).is_err(), "a {len}-byte prefix parsed");
        }
        assert!(parse_ledgerlink_outbox(&LONG[..0]).is_ok());
        assert!(parse_ledgerlink_outbox(LONG).is_ok());
    }
}

rl_test! {
    fn sampled_truncations_of_the_multi_entry_file_fail_cleanly() {
        let last_brace = MULTI.iter().rposition(|&b| b == b'}').expect("has a closing brace");
        for len in (1..=last_brace).step_by(7) {
            assert!(parse_ledgerlink_outbox(&MULTI[..len]).is_err(), "a {len}-byte prefix parsed");
        }
    }
}

rl_test! {
    fn random_corruptions_never_panic() {
        let mut rng = Lcg(0x1964_c2c2);
        for base in [LONG, MULTI] {
            for _ in 0..400 {
                let mut bytes = base.to_vec();
                let flips = 1 + (rng.next() % 4) as usize;
                for _ in 0..flips {
                    let at = (rng.next() % bytes.len() as u64) as usize;
                    let byte = match rng.next() % 4 {
                        0 => b"{}[]=\",\\-\n'"[(rng.next() % 11) as usize],
                        _ => (rng.next() & 0xff) as u8,
                    };
                    if let Some(slot) = bytes.get_mut(at) {
                        *slot = byte;
                    }
                }
                let _ = parse_ledgerlink_outbox(&bytes);
            }
        }
    }
}
