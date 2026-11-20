//! The per-install secret behind the addon-errors `accountKey` (ROK-2099 S5
//! review D1). `accountKey` = HMAC-SHA256(this key, account folder name), so
//! the low-entropy folder name (a Battle.net account number or an old login
//! name) cannot be recovered by hashing guesses. The key is 32 bytes from the
//! OS CSPRNG, created on first use next to `sent-state.json` (0600 on unix,
//! atomic write), and never uploaded, logged or served by the dev bridge.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// File under the app data dir that holds the key as 64 hex chars.
pub const KEY_FILE: &str = "addon-errors-account.key";

static KEY_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Fallback when no app data dir is known (tests): one key per process.
static MEMORY_KEY: OnceLock<Option<[u8; 32]>> = OnceLock::new();

/// Set once at startup: the app data dir the key lives in.
pub fn set_dir(dir: PathBuf) {
    let _ = KEY_DIR.set(dir);
}

/// The app data dir set at startup, if any.
pub fn default_dir() -> Option<PathBuf> {
    KEY_DIR.get().cloned()
}

fn os_random(key: &mut [u8]) -> bool {
    getrandom::getrandom(key).is_ok()
}

/// The install key: read from `<dir>/KEY_FILE`, else generated and stored.
/// `None` only when the OS RNG fails — callers then send nothing, because a
/// guessable key would make `accountKey` reversible again.
pub fn load_or_create(dir: Option<&Path>) -> Option<[u8; 32]> {
    match dir {
        Some(d) => load_or_create_with(d, os_random),
        None => *MEMORY_KEY.get_or_init(|| fresh(os_random)),
    }
}

fn fresh(fill: impl FnOnce(&mut [u8]) -> bool) -> Option<[u8; 32]> {
    let mut key = [0u8; 32];
    fill(&mut key).then_some(key)
}

/// [`load_or_create`] with the RNG injected (`fill` → `false` = failed). A
/// missing, short or corrupt file is replaced; if the write fails the key is
/// still returned (it then changes on the next start, which only regroups).
pub fn load_or_create_with(dir: &Path, fill: impl FnOnce(&mut [u8]) -> bool) -> Option<[u8; 32]> {
    let path = dir.join(KEY_FILE);
    if let Some(key) = read_key(&path) {
        return Some(key);
    }
    let key = fresh(fill)?;
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::create_dir_all(dir);
    let _ = crate::dev_bridge::write_guard::write_private(&path, hex.as_bytes());
    Some(key)
}

/// A stored key: exactly 64 hex chars (at most 128 bytes are read).
fn read_key(path: &Path) -> Option<[u8; 32]> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(128)
        .read_to_string(&mut text)
        .ok()?;
    let t = text.trim();
    if t.len() != 64 || !t.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok())
        .collect();
    bytes?.try_into().ok()
}
