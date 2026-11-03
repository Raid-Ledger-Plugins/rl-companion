//! The dev token (`rlcdev_…`, spec §7): 32 random bytes, base64url. Shown to
//! the user once; only its sha256 is stored. Unrelated to the `rlcd_` device
//! token and never accepted in its place.

use base64::Engine;
use sha2::{Digest, Sha256};
use std::fmt;
use subtle::ConstantTimeEq;

pub const DEV_TOKEN_PREFIX: &str = "rlcdev_";
const BODY_LEN: usize = 43;

/// A freshly minted dev token. `Debug` is redacted.
pub struct DevToken(String);

impl DevToken {
    /// Mint a new token from the OS RNG.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)?;
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        Ok(Self(format!("{DEV_TOKEN_PREFIX}{body}")))
    }

    /// The raw value — returned to the window exactly once, never stored.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn sha256_hex(&self) -> String {
        sha256_hex(&self.0)
    }
}

impl fmt::Debug for DevToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DevToken(<redacted>)")
    }
}

/// Shape check: `^rlcdev_[A-Za-z0-9_-]{43}$` (CONTRACT.md Transport).
pub fn well_formed(raw: &str) -> bool {
    raw.strip_prefix(DEV_TOKEN_PREFIX).is_some_and(|body| {
        body.len() == BODY_LEN
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

pub fn sha256_hex(raw: &str) -> String {
    Sha256::digest(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Constant-time check of a presented token against the stored hash.
pub fn matches(presented: &str, stored_sha256: &str) -> bool {
    if !well_formed(presented) {
        return false;
    }
    let hash = sha256_hex(presented);
    hash.len() == stored_sha256.len() && bool::from(hash.as_bytes().ct_eq(stored_sha256.as_bytes()))
}
