//! Device token handling: validation and OS keychain storage.
//!
//! The raw `rlcd_` token is a secret (device-link CONTRACT.md §5): it is never
//! logged, never written to the config file, and only sent in the
//! `Authorization` header to the origin that minted it. Storage is keyed by
//! that origin, so pointing the app at a fleet slot never sends the prod token.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

/// Keychain service name (macOS Keychain item / Windows Credential Manager target).
pub const KEYCHAIN_SERVICE: &str = "net.gamernight.rlcompanion";

const TOKEN_PREFIX: &str = "rlcd_";
const TOKEN_BODY_LEN: usize = 43;

/// A validated device token. `Debug` and `Display` are redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceToken(String);

impl DeviceToken {
    /// Accepts only `^rlcd_[A-Za-z0-9_-]{43}$` (`COMPANION_DEVICE_TOKEN_PATTERN`).
    pub fn parse(raw: &str) -> Option<Self> {
        let body = raw.strip_prefix(TOKEN_PREFIX)?;
        let valid = body.len() == TOKEN_BODY_LEN
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        valid.then(|| Self(raw.to_owned()))
    }

    /// The raw secret, for the `Authorization` header and the keychain only.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Last 4 chars, matching the server's `tokenHint`.
    pub fn hint(&self) -> &str {
        &self.0[self.0.len() - 4..]
    }
}

impl fmt::Debug for DeviceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceToken(…{})", self.hint())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("the stored token is malformed")]
    Malformed,
}

/// Where the token lives. `origin` is the normalised server URL.
pub trait TokenStore: Send + Sync {
    fn load(&self, origin: &str) -> Result<Option<DeviceToken>, TokenError>;
    fn save(&self, origin: &str, token: &DeviceToken) -> Result<(), TokenError>;
    fn clear(&self, origin: &str) -> Result<(), TokenError>;
}

/// OS credential store via the `keyring` crate (macOS Keychain, Windows
/// Credential Manager, Linux kernel keyutils).
pub struct KeyringStore;

impl KeyringStore {
    fn entry(origin: &str) -> Result<keyring::Entry, TokenError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, origin).map_err(|e| TokenError::Keychain(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn load(&self, origin: &str) -> Result<Option<DeviceToken>, TokenError> {
        match Self::entry(origin)?.get_password() {
            Ok(raw) => DeviceToken::parse(&raw).map(Some).ok_or(TokenError::Malformed),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(TokenError::Keychain(e.to_string())),
        }
    }

    fn save(&self, origin: &str, token: &DeviceToken) -> Result<(), TokenError> {
        Self::entry(origin)?
            .set_password(token.expose())
            .map_err(|e| TokenError::Keychain(e.to_string()))
    }

    fn clear(&self, origin: &str) -> Result<(), TokenError> {
        match Self::entry(origin)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(TokenError::Keychain(e.to_string())),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl MemoryStore {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl TokenStore for MemoryStore {
    fn load(&self, origin: &str) -> Result<Option<DeviceToken>, TokenError> {
        match self.map().get(origin) {
            Some(raw) => DeviceToken::parse(raw).map(Some).ok_or(TokenError::Malformed),
            None => Ok(None),
        }
    }

    fn save(&self, origin: &str, token: &DeviceToken) -> Result<(), TokenError> {
        self.map().insert(origin.to_owned(), token.expose().to_owned());
        Ok(())
    }

    fn clear(&self, origin: &str) -> Result<(), TokenError> {
        self.map().remove(origin);
        Ok(())
    }
}
