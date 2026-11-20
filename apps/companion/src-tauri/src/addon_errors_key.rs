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
/// missing or unparseable file is replaced. Any other read error (e.g. the
/// file is briefly locked) returns `None` WITHOUT touching the file, so a good
/// key is never overwritten; the caller sends nothing and retries next cycle.
/// If writing a new key fails the key is still returned (it then changes on
/// the next start, which only regroups). Log lines never carry the key or path.
pub fn load_or_create_with(dir: &Path, fill: impl FnOnce(&mut [u8]) -> bool) -> Option<[u8; 32]> {
    let path = dir.join(KEY_FILE);
    match read_key(&path) {
        Ok(Stored::Key(key)) => return Some(key),
        Ok(Stored::Missing | Stored::Unparseable) => {}
        Err(kind) => {
            let msg =
                format!("account key file unreadable ({kind:?}); addon errors not sent this cycle");
            crate::dev_bridge::logbuf::log("warn", "addon_errors", &msg);
            return None;
        }
    }
    let key = fresh(fill)?;
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::create_dir_all(dir);
    if let Err(e) = crate::dev_bridge::write_guard::write_private(&path, hex.as_bytes()) {
        let kind = e.kind();
        let msg = format!("account key file write failed ({kind:?}); key kept in memory only");
        crate::dev_bridge::logbuf::log("warn", "addon_errors", &msg);
    }
    Some(key)
}

/// What the key file holds.
enum Stored {
    Key([u8; 32]),
    Missing,
    Unparseable,
}

/// A stored key: exactly 64 hex chars (at most 128 bytes are read). `Err`
/// only for read errors other than "not found" and "not UTF-8".
fn read_key(path: &Path) -> Result<Stored, std::io::ErrorKind> {
    use std::io::{ErrorKind, Read};
    let mut text = String::new();
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Stored::Missing),
        Err(e) => return Err(e.kind()),
    };
    match file.take(128).read_to_string(&mut text) {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::InvalidData => return Ok(Stored::Unparseable),
        Err(e) => return Err(e.kind()),
    }
    Ok(parse_hex_key(text.trim()).map_or(Stored::Unparseable, Stored::Key))
}

fn parse_hex_key(t: &str) -> Option<[u8; 32]> {
    if t.len() != 64 || !t.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok())
        .collect();
    bytes?.try_into().ok()
}
