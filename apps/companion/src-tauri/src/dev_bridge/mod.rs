//! Dev bridge (ROK-2055 C2): an opt-in HTTPS listener on one LAN address that
//! skeleton-key reads `/dev/v1/*` from. Contract: `contracts/dev-bridge/v1/`.
//!
//! Off by default: with the toggle off no socket is opened (AC-2).
//! [`DevBridge::reconcile`] is the only place the listener starts or stops.

pub mod addons;
pub mod bg3_newage;
pub mod bg3_newage_reader;
pub mod bg3_redact;
pub mod bg3_routes;
pub mod bg3_saves;
pub mod auth;
pub mod bind;
pub mod collectors;
pub mod commands;
pub mod conn_budget;
pub mod health;
pub mod logbuf;
pub mod paging;
pub mod routes;
pub mod server;
pub mod settings;
pub mod snapshot_cache;
pub mod snapshot_routes;
pub mod snapshot_validate;
pub mod sv;
pub mod tls;
pub mod token;
pub mod write_guard;

use crate::config::AppConfig;
use serde::Serialize;
use server::{ServerCtx, Snapshot};
use settings::DevBridgeConfig;
use snapshot_cache::SnapshotCache;
use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

pub use tls::TLS_FILE;

struct Running {
    /// What reconcile asked for (port may be 0 in tests).
    requested: SocketAddrV4,
    bound: SocketAddrV4,
    fingerprint: String,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
    /// The 30 s dev snapshot poll (ROK-2072 D9), aborted with the listener.
    poll: JoinHandle<()>,
}

/// What the panel shows about the listener.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerStatus {
    /// `ip:port` while listening.
    pub listening: Option<String>,
    /// Why the bridge is on but not listening.
    pub error: Option<String>,
    /// sha256 of the served certificate.
    pub cert_fingerprint: Option<String>,
}

/// Where the listener should be, or why it can't run.
pub fn desired(cfg: &DevBridgeConfig) -> Result<Option<SocketAddrV4>, String> {
    if !cfg.enabled {
        return Ok(None);
    }
    if cfg.token_sha256.is_none() {
        return Err("Create a token to start the bridge".into());
    }
    let ip = cfg.bind_ip.as_deref().ok_or("Pick a network address")?;
    bind::bind_addr(ip, cfg.port)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn snapshot(cfg: &AppConfig) -> Snapshot {
    let b = &cfg.dev_bridge;
    Snapshot {
        policy: auth::AuthPolicy {
            allowlist: bind::parse_allowlist(&b.allowlist),
            token_sha256: b.token_sha256.clone(),
        },
        wow_dir: cfg.wow_dir.clone(),
        probes_enabled: b.probes_enabled,
        snapshot_cache: None,
    }
}

pub struct DevBridge {
    tls_path: PathBuf,
    ctx: Arc<ServerCtx>,
    running: Mutex<Option<Running>>,
    /// `<app_data_dir>/dev-bridge/snapshots` (ROK-2072); served only while listening.
    cache: Option<Arc<SnapshotCache>>,
}

impl DevBridge {
    pub fn new(tls_path: PathBuf, app_data_dir: Option<&Path>) -> Self {
        Self {
            tls_path,
            ctx: Arc::new(ServerCtx::default()),
            running: Mutex::new(None),
            cache: app_data_dir.map(|d| Arc::new(SnapshotCache::in_app_data(d))),
        }
    }

    /// A bridge serving from `ctx` (tests inject the bound-IP probe).
    #[cfg(test)]
    pub(crate) fn with_ctx(tls_path: PathBuf, ctx: ServerCtx) -> Self {
        Self {
            tls_path,
            ctx: Arc::new(ctx),
            running: Mutex::new(None),
            cache: None,
        }
    }

    /// The served slice of `cfg`; the snapshot cache only while `on`, so
    /// nothing is ingested while the listener is off (AC-9).
    fn served(&self, cfg: &AppConfig, on: bool) -> Snapshot {
        let mut snap = snapshot(cfg);
        if on {
            snap.snapshot_cache = self.cache.clone();
        } else {
            snap.policy = auth::AuthPolicy::default();
        }
        snap
    }

    /// Bring the listener in line with `cfg`: start, move, keep or stop it.
    /// When it should not run, the served policy is emptied first, so a
    /// request racing the close is refused rather than answered.
    pub async fn reconcile(&self, cfg: &AppConfig) -> ListenerStatus {
        let want = desired(&cfg.dev_bridge);
        self.ctx
            .set_snapshot(self.served(cfg, matches!(want, Ok(Some(_)))));
        match want {
            Ok(Some(addr)) => match self.listen(addr).await {
                Ok(()) => self.status().await,
                Err(e) => ListenerStatus {
                    error: Some(e),
                    ..Default::default()
                },
            },
            Ok(None) => {
                self.stop().await;
                ListenerStatus::default()
            }
            Err(e) => {
                self.stop().await;
                ListenerStatus {
                    error: Some(e),
                    ..Default::default()
                }
            }
        }
    }

    /// Listen on `addr` WITHOUT the bind rules — reconcile validates first;
    /// tests call it directly with a loopback address.
    pub(crate) async fn listen(&self, addr: SocketAddrV4) -> Result<(), String> {
        let mut running = self.running.lock().await;
        if running
            .as_ref()
            .is_some_and(|r| r.requested == addr && !r.task.is_finished())
        {
            return Ok(());
        }
        if let Some(old) = running.take() {
            shut(old).await;
        }
        let material = tls::TlsMaterial::load_or_create(&self.tls_path, *addr.ip())?;
        let acceptor = material.acceptor()?;
        let std_listener = std::net::TcpListener::bind(addr)
            .map_err(|e| format!("could not listen on {addr}: {e}"))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| e.to_string())?;
        let bound = match std_listener.local_addr() {
            Ok(std::net::SocketAddr::V4(a)) => a,
            _ => addr,
        };
        let listener =
            tokio::net::TcpListener::from_std(std_listener).map_err(|e| e.to_string())?;
        let (stop, rx) = oneshot::channel();
        self.ctx.set_closed_reason(None);
        let task = tokio::spawn(server::serve(listener, acceptor, self.ctx.clone(), rx));
        let poll = tokio::spawn(snapshot_routes::poll_loop(self.ctx.clone()));
        *running = Some(Running {
            requested: addr,
            bound,
            fingerprint: material.fingerprint(),
            stop,
            task,
            poll,
        });
        Ok(())
    }

    /// Close the listener (port released when this returns).
    pub async fn stop(&self) {
        if let Some(old) = self.running.lock().await.take() {
            shut(old).await;
        }
    }

    /// The bound address while the accept loop runs (it ends on its own
    /// when the bound IP leaves this PC — spec §7 fallback).
    pub async fn listening_addr(&self) -> Option<SocketAddrV4> {
        self.running
            .lock()
            .await
            .as_ref()
            .filter(|r| !r.task.is_finished())
            .map(|r| r.bound)
    }

    pub async fn status(&self) -> ListenerStatus {
        let running = self.running.lock().await;
        let live = running.as_ref().filter(|r| !r.task.is_finished());
        ListenerStatus {
            listening: live.map(|r| r.bound.to_string()),
            error: live.is_none().then(|| self.ctx.closed_reason()).flatten(),
            cert_fingerprint: live.map(|r| r.fingerprint.clone()),
        }
    }

    /// Refresh the served snapshot only (e.g. the WoW folder changed).
    pub fn refresh(&self, cfg: &AppConfig) {
        let mut snap = snapshot(cfg);
        if desired(&cfg.dev_bridge).is_ok_and(|w| w.is_some()) {
            snap.snapshot_cache = self.cache.clone();
        }
        self.ctx.set_snapshot(snap);
    }

    /// Where `/dev/v1/bg3/*` reads saves from: the BG3 `Savegames/Story`
    /// roots; the `saveRef` secret lives under `key_dir` (ROK-2078).
    pub fn set_bg3_source(&self, roots: Vec<PathBuf>, key_dir: Option<PathBuf>) {
        let decoder = Arc::new(bg3_newage_reader::LsmfDecoder);
        self.ctx.set_bg3(Arc::new(bg3_saves::Bg3Source::new(roots, key_dir, decoder)));
    }

    /// Where `/dev/v1/collectors` reads the runtime's state from.
    pub fn set_collectors_source(&self, f: routes::CollectorsFn) {
        self.ctx.set_collectors(f);
    }

    #[cfg(test)]
    pub(crate) fn ctx(&self) -> &ServerCtx {
        &self.ctx
    }
}

/// The WoW folder setting changed: serve the new folder at once (flavors,
/// addons, SavedVariables) without touching the listener.
pub fn on_wow_dir_changed(bridge: Option<&DevBridge>, cfg: &AppConfig) {
    if let Some(b) = bridge {
        b.refresh(cfg);
    }
}

/// Stop accepting, then wait while `serve` closes every accepted connection
/// and pending handshake (bounded by its drain timeout).
async fn shut(r: Running) {
    r.poll.abort();
    let _ = r.stop.send(());
    let _ = r.task.await;
}
