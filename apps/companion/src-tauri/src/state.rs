//! App-wide state managed by Tauri (`app.state::<Shared>()`).

use crate::config::AppConfig;
use crate::token::{DeviceToken, TokenError, TokenStore};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use tauri::async_runtime::JoinHandle;

pub const CONFIG_FILE: &str = "config.json";

pub struct Shared {
    config_path: PathBuf,
    config: Mutex<AppConfig>,
    tokens: Box<dyn TokenStore>,
    link_task: Mutex<Option<JoinHandle<()>>>,
    /// Serialises token writes so a compare-and-clear can't interleave with a
    /// re-link's save.
    token_write: Mutex<()>,
    /// Tray Pause/Resume. C-3's uploader must check this before sending.
    paused: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    pub fn new(config_path: PathBuf, tokens: Box<dyn TokenStore>) -> Self {
        let config = AppConfig::load(&config_path);
        Self {
            config_path,
            config: Mutex::new(config),
            tokens,
            link_task: Mutex::new(None),
            token_write: Mutex::new(()),
            paused: AtomicBool::new(false),
        }
    }

    /// A snapshot; never hold it across an `.await`.
    pub fn config(&self) -> AppConfig {
        lock(&self.config).clone()
    }

    /// Apply `f` to the config and persist it.
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> std::io::Result<AppConfig> {
        let mut cfg = lock(&self.config);
        f(&mut cfg);
        cfg.save(&self.config_path)?;
        Ok(cfg.clone())
    }

    /// Token for the CURRENT server origin.
    pub fn token(&self) -> Result<Option<DeviceToken>, TokenError> {
        self.tokens.load(&self.config().server_url)
    }

    pub fn save_token(&self, origin: &str, token: &DeviceToken) -> Result<(), TokenError> {
        let _w = lock(&self.token_write);
        self.tokens.save(origin, token)
    }

    /// Forget `sent` after it drew a 401 — but only while `origin` is still
    /// the current server and `sent` is still its stored token. A re-link (or
    /// a server switch) between the request and its 401 keeps the new
    /// credential. Returns whether the token was cleared.
    pub fn clear_token_if(&self, origin: &str, sent: &DeviceToken) -> Result<bool, TokenError> {
        Ok(self.clear_token_if_with(origin, sent, || ())?.is_some())
    }

    /// `clear_token_if`, running `on_clear` under the same lock once the
    /// token is cleared: whatever it reads (the link generation) predates
    /// any re-link's `save_token`. `None` when nothing was cleared.
    pub fn clear_token_if_with<R>(
        &self,
        origin: &str,
        sent: &DeviceToken,
        on_clear: impl FnOnce() -> R,
    ) -> Result<Option<R>, TokenError> {
        let _w = lock(&self.token_write);
        let cfg = lock(&self.config);
        if cfg.server_url != origin || self.tokens.load(origin)?.as_ref() != Some(sent) {
            return Ok(None);
        }
        self.tokens.clear(origin)?;
        Ok(Some(on_clear()))
    }

    /// Forget the token for the current origin (401 or user "Unlink").
    pub fn clear_token(&self) -> Result<(), TokenError> {
        self.tokens.clear(&self.config().server_url)
    }

    /// Replace (and abort) any running link poll.
    pub fn set_link_task(&self, task: Option<JoinHandle<()>>) {
        if let Some(old) = std::mem::replace(&mut *lock(&self.link_task), task) {
            old.abort();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Flip pause; returns the new value.
    pub fn toggle_paused(&self) -> bool {
        !self.paused.fetch_xor(true, Ordering::Relaxed)
    }
}
