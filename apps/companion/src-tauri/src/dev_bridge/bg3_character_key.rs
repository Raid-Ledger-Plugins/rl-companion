//! ROK-2079 OQ-4: `party[].characterKeyPrefix` on the BG3 save-structure read
//! (`dev-bridge/v1`). `characterKey = SHA-256(campaignKey ‖ ':' ‖ entityGuid)`
//! over the UTF-8 of `<campaignKey>:<guid>`; the bridge carries its first 8
//! hex chars only. The GUID itself never leaves this module as text except
//! as hash input.
//!
//! **GUID text (UNVERIFIED — the operator's sweep confirms it).** The NewAge
//! `core.v0.Level` entity table stores each GUID as 16 raw bytes. They are
//! rendered the way LSLib renders every other BG3 UUID
//! ([`rl_collectors::uuid_display`]): .NET `Guid` order (the first three
//! groups little-endian) after `ByteSwapGuid` swaps each byte pair of the
//! last eight bytes, lowercase, hyphenated 8-4-4-4-12. Bytes `00..0f` →
//! `03020100-0504-0706-0908-0b0a0d0c0f0e`.

use rl_collectors::{sha256_hex, uuid_display};
use std::fmt;

/// One NewAge entity's GUID bytes as stored. `Debug` never prints them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EntityGuid([u8; 16]);

impl fmt::Debug for EntityGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EntityGuid(<redacted>)")
    }
}

impl EntityGuid {
    /// Wrap the 16 bytes as stored in the entity table.
    pub fn new(raw: [u8; 16]) -> Self {
        Self(raw)
    }

    /// Lowercase hyphenated text in LSLib order (see the module doc).
    pub fn text(&self) -> String {
        uuid_display(&self.0)
    }
}

/// The first 8 hex chars of the member's `characterKey`; `None` unless
/// `campaign_key` is the 64-lowercase-hex key the read computed.
pub fn character_key_prefix(campaign_key: Option<&str>, guid: &EntityGuid) -> Option<String> {
    let key = campaign_key
        .filter(|k| k.len() == 64 && k.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))?;
    let digest = sha256_hex(format!("{key}:{}", guid.text()).as_bytes());
    digest.get(..8).map(str::to_owned)
}
