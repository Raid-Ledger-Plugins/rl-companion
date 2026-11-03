//! `GET /companion/v1/config` (ROK-2067 D5): which contracts the linked server
//! takes, each one's owner plugin state and its game's cover art
//! (Raid Ledger `envelope/v1/CONTRACT.md` §4.2). Unknown keys are ignored —
//! the contract says "the app MUST ignore unknown `/config` keys" — so no
//! struct here uses `deny_unknown_fields`.

use crate::config::API_PREFIX;
use crate::registry::BUILTINS;
use crate::runtime::{status, Runtime, STATUS_EVENT};
use crate::state::Shared;
use crate::token::DeviceToken;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const CONFIG_PATH: &str = "companion/v1/config";
/// Refetch while linked (D5).
pub const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);
/// At most one `unknown_contract`-triggered refetch per minute (D5).
pub const DEFER_DEBOUNCE: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const COVERS_DIR: &str = "covers";

/// The subset of `CompanionConfigSchema` the app reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Absent for anonymous callers.
    #[serde(default)]
    pub contracts: Option<Vec<ServerContract>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerContract {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    /// Owner plugin; `None` = no handler (`null`) or an older server (absent).
    #[serde(default)]
    pub plugin: Option<ServerPlugin>,
    /// Owner plugin's game, for the tile art; `None` = null or absent.
    #[serde(default)]
    pub game: Option<ServerGame>,
    /// Hubs a `bg3-screenshot` may target (ROK-1966); `None` = an older
    /// server or another contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_targets: Option<Vec<HubTarget>>,
}

/// One `CompanionConfigHubTargetSchema` entry: a hub the user may pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubTarget {
    pub hub_guild_id: i64,
    pub name: String,
    /// Pre-selected in the picker (the hub of the caller's latest save).
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerPlugin {
    pub id: String,
    pub name: String,
    pub state: PluginState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Active,
    Inactive,
    NotInstalled,
    /// A state a newer server added; treated like `active` (no warning).
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerGame {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub cover_url: Option<String>,
}

impl ServerConfig {
    pub fn contract(&self, id: &str) -> Option<&ServerContract> {
        self.contracts.as_ref()?.iter().find(|c| c.id == id)
    }
}

/// The runtime's `/config` state (not persisted).
#[derive(Debug, Clone, Default)]
pub struct ServerState {
    /// Last fetch: `Some(true)` answered, `Some(false)` failed, `None` = not tried.
    pub reachable: Option<bool>,
    /// Cover per contract id as a `data:` URL; `None` = draw the fallback.
    pub covers: BTreeMap<String, Option<String>>,
    last_defer: Option<Instant>,
    /// Bumped by every refresh start and every server switch: a refresh
    /// publishes only if nothing newer began while it was out.
    generation: u64,
}

impl ServerState {
    /// A refresh is starting; returns its generation for [`Self::publish`].
    pub fn begin_refresh(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// The server URL changed: forget the old server's state, and make every
    /// refresh still in flight for it stale.
    pub fn reset(&mut self) {
        *self = Self {
            generation: self.generation + 1,
            ..Self::default()
        };
    }

    /// Store a refresh's result unless a newer refresh or a server switch
    /// began since `generation`. Returns whether it was stored.
    pub fn publish(
        &mut self,
        generation: u64,
        reachable: Option<bool>,
        covers: BTreeMap<String, Option<String>>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.reachable = reachable;
        self.covers = covers;
        true
    }

    /// `true` (and the slot is taken) when no deferral-triggered refetch ran
    /// in the last [`DEFER_DEBOUNCE`].
    pub fn take_defer_slot(&mut self, now: Instant) -> bool {
        let due = self
            .last_defer
            .is_none_or(|t| now.saturating_duration_since(t) >= DEFER_DEBOUNCE);
        if due {
            self.last_defer = Some(now);
        }
        due
    }
}

/// The `/config` URL for a normalised server origin.
pub fn config_url(server: &str) -> Result<url::Url, String> {
    let mut base = url::Url::parse(server).map_err(|e| format!("Bad server URL: {e}"))?;
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    base.join(API_PREFIX)
        .and_then(|u| u.join(CONFIG_PATH))
        .map_err(|e| format!("Bad server URL: {e}"))
}

/// Why a `/config` fetch gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 401: the token is revoked or unknown — re-link, as for an upload.
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("the server answered HTTP 401"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

/// One `GET /companion/v1/config` with the device token.
pub async fn fetch(server: &str, token: &DeviceToken) -> Result<ServerConfig, FetchError> {
    let other = FetchError::Other;
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| other(format!("HTTP client: {e}")))?;
    let res = http
        .get(config_url(server).map_err(other)?)
        .bearer_auth(token.expose())
        .send()
        .await
        .map_err(|_| other("could not reach the server".to_owned()))?;
    match res.status().as_u16() {
        401 => return Err(FetchError::Unauthorized),
        c if !(200..300).contains(&c) => {
            return Err(other(format!("the server answered HTTP {c}")))
        }
        _ => {}
    }
    res.json::<ServerConfig>()
        .await
        .map_err(|e| other(format!("unexpected /config answer: {e}")))
}

/// Every contract id a tile may show: the app's collectors and the cache's.
fn cover_ids(cfg: &crate::config::AppConfig) -> Vec<(String, Option<String>)> {
    let mut ids: BTreeMap<String, Option<String>> = BUILTINS
        .iter()
        .map(|b| (b.manifest.contract_id.to_owned(), None))
        .collect();
    for c in cfg
        .last_server_config
        .iter()
        .flat_map(|s| s.contracts.iter().flatten())
    {
        let url = c.game.as_ref().and_then(|g| g.cover_url.clone());
        ids.insert(c.id.clone(), url);
    }
    ids.into_iter().collect()
}

/// `/config` answered 401 to `sent` at `origin`. Same as an upload's 401:
/// drop the dead token so no later refresh sends it, and show the unlinked
/// state instead of cached tiles — unless the device was re-linked or the
/// server switched while the request was out, in which case the current
/// token is not the one refused. Returns whether the device was unlinked.
pub fn revoke_after_401(shared: &Shared, rt: &Runtime, origin: &str, sent: &DeviceToken) -> bool {
    let cleared = shared.clear_token_if(origin, sent).unwrap_or(false);
    if cleared {
        rt.config_unauthorized();
    }
    cleared
}

/// Fetch `/config`, cache it, refresh the covers and tell the window. An
/// unlinked device or a failed fetch keeps the last-known cache and covers.
pub async fn refresh(app: &AppHandle, rt: &Runtime) {
    let generation = rt.update_server(ServerState::begin_refresh);
    let shared = app.state::<Shared>();
    let before = shared.config();
    let dir = app
        .path()
        .app_data_dir()
        .map(|d| d.join(COVERS_DIR))
        .unwrap_or_default();
    let token = shared.token().ok().flatten();
    let fetched = match &token {
        Some(t) => Some(fetch(&before.server_url, t).await),
        None => None,
    };
    if let (Some(Err(FetchError::Unauthorized)), Some(t)) = (&fetched, &token) {
        revoke_after_401(&shared, rt, &before.server_url, t);
    }
    let reachable = fetched.as_ref().map(Result::is_ok);
    if let Some(Ok(sc)) = fetched {
        let at = crate::wow_collector::captured_at(None);
        let saved = shared.update_config(|c| {
            // The URL may have changed during the fetch: keep the other server's
            // answer out of this one's cache.
            if c.server_url == before.server_url {
                c.last_server_config = Some(sc);
                c.fetched_at = Some(at);
            }
        });
        if let Err(e) = saved {
            rt.log(format!("Could not save the server's settings ({e})"));
        }
    }
    let ids = cover_ids(&shared.config());
    let covers = if reachable == Some(true) {
        crate::covers::refresh_all(&dir, &ids, crate::covers::http_fetch).await
    } else {
        crate::covers::cached_all(&dir, &ids)
    };
    // The artwork await is long: a server switch or a newer refresh since
    // this one began owns the state now, so this result is dropped.
    if rt.update_server(|s| s.publish(generation, reachable, covers)) {
        let _ = app.emit(STATUS_EVENT, status(app, rt));
    }
}

/// Fetch on launch, every [`REFRESH_EVERY`], and whenever `config_kick` fires
/// (after link, on an `unknown_contract` deferral). Call once from `setup`.
pub fn start(app: AppHandle, rt: Arc<Runtime>) {
    tauri::async_runtime::spawn(async move {
        loop {
            refresh(&app, &rt).await;
            tokio::select! {
                _ = tokio::time::sleep(REFRESH_EVERY) => {}
                _ = rt.config_kick.notified() => {}
            }
        }
    });
}
