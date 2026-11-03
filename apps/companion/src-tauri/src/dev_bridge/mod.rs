//! Dev bridge (ROK-2055 C2): an opt-in HTTPS listener on one LAN address that
//! skeleton-key reads `/dev/v1/*` from. Contract: `contracts/dev-bridge/v1/`.
//!
//! Off by default: with the toggle off no socket is opened (AC-2).
//! [`DevBridge::reconcile`] is the only place the listener starts or stops.

pub mod auth;
pub mod bind;
pub mod commands;
pub mod health;
pub mod server;
pub mod settings;
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
    pub async fn reconcile(&self, cfg: &AppConfig) -> ListenerStatus {
        self.ctx.set_snapshot(snapshot(cfg));
        match desired(&cfg.dev_bridge) {
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
        if running.as_ref().is_some_and(|r| r.requested == addr) {
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

    pub async fn listening_addr(&self) -> Option<SocketAddrV4> {
        self.running.lock().await.as_ref().map(|r| r.bound)
    }

    pub async fn status(&self) -> ListenerStatus {
        let running = self.running.lock().await;
        ListenerStatus {
            listening: running.as_ref().map(|r| r.bound.to_string()),
            error: None,
            cert_fingerprint: running.as_ref().map(|r| r.fingerprint.clone()),
        }
    }

    /// Refresh the served snapshot only (e.g. the WoW folder changed).
    pub fn refresh(&self, cfg: &AppConfig) {
        self.ctx.set_snapshot(snapshot(cfg));
    }
}

async fn shut(r: Running) {
    let _ = r.stop.send(());
    let _ = r.task.await;
}
