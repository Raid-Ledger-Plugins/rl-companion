//! Dev bridge (ROK-2055 C2): an opt-in HTTPS listener on one LAN address that
//! skeleton-key reads `/dev/v1/*` from. Contract: `contracts/dev-bridge/v1/`.
//!
//! Off by default: with the toggle off no socket is opened (AC-2).
//! [`DevBridge::reconcile`] is the only place the listener starts or stops.

pub mod addons;
pub mod auth;
pub mod bind;
pub mod collectors;
pub mod commands;
pub mod health;
pub mod logbuf;
pub mod paging;
pub mod routes;
pub mod server;
pub mod settings;
pub mod sv;
pub mod tls;
pub mod token;
pub mod write_guard;

use crate::config::AppConfig;
use serde::Serialize;
use server::{ServerCtx, Snapshot};
use settings::DevBridgeConfig;
use std::net::SocketAddrV4;
use std::path::PathBuf;
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
    }
}

pub struct DevBridge {
    tls_path: PathBuf,
    ctx: Arc<ServerCtx>,
    running: Mutex<Option<Running>>,
}

impl DevBridge {
    pub fn new(tls_path: PathBuf) -> Self {
        Self {
            tls_path,
            ctx: Arc::new(ServerCtx::default()),
            running: Mutex::new(None),
        }
    }

    /// Bring the listener in line with `cfg`: start, move, keep or stop it.
    /// When it should not run, the served policy is emptied first, so a
    /// request racing the close is refused rather than answered.
    pub async fn reconcile(&self, cfg: &AppConfig) -> ListenerStatus {
        let want = desired(&cfg.dev_bridge);
        let mut snap = snapshot(cfg);
        if !matches!(want, Ok(Some(_))) {
            snap.policy = auth::AuthPolicy::default();
        }
        self.ctx.set_snapshot(snap);
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
        *running = Some(Running {
            requested: addr,
            bound,
            fingerprint: material.fingerprint(),
            stop,
            task,
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
        self.ctx.set_snapshot(snapshot(cfg));
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
    let _ = r.stop.send(());
    let _ = r.task.await;
}
