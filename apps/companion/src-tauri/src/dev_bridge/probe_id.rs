//! Probe ids: a 26-char ULID in Crockford base32 (`$defs/probe-id`,
//! `^[0-9A-HJKMNP-TV-Z]{26}$`) — 48-bit Unix ms + 80 random bits, monotonic
//! within one millisecond (the random part is incremented, never redrawn).

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const RAND_BITS: u32 = 80;
const RAND_MASK: u128 = (1u128 << RAND_BITS) - 1;

/// The last id handed out: `(ms, random part)`.
static LAST: Mutex<(u64, u128)> = Mutex::new((0, 0));

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn random80() -> u128 {
    let mut buf = [0u8; 16];
    if getrandom::getrandom(&mut buf[..10]).is_err() {
        // No OS randomness: the monotonic step below still keeps ids unique.
        return 0;
    }
    u128::from_le_bytes(buf) & RAND_MASK
}

/// Encode `(ms, random)` as 26 Crockford base32 characters.
pub fn encode(ms: u64, random: u128) -> String {
    let value = (u128::from(ms & 0xFFFF_FFFF_FFFF) << RAND_BITS) | (random & RAND_MASK);
    (0..26)
        .rev()
        .map(|i| char::from(ALPHABET[((value >> (i * 5)) & 31) as usize]))
        .collect()
}

/// A fresh probe id, strictly greater than every id this process made.
pub fn new_probe_id() -> String {
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let ms = now_ms();
    let (ms, random) = if ms > last.0 {
        (ms, random80())
    } else if last.1 < RAND_MASK {
        (last.0, last.1 + 1)
    } else {
        (last.0 + 1, random80())
    };
    *last = (ms, random);
    encode(ms, random)
}

/// `^[0-9A-HJKMNP-TV-Z]{26}$`.
pub fn is_probe_id(s: &str) -> bool {
    s.len() == 26 && s.bytes().all(|b| ALPHABET.contains(&b))
}
