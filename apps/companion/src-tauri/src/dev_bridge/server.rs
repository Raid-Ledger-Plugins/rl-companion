//! The HTTPS listener and `/dev/v1/*` router. Admission (auth.rs) runs before
//! routing, so an unauthenticated caller learns nothing about which paths exist.

use super::auth::{admit, AuthPolicy, Lockout};
use super::health::{health, HealthInput};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::{header, Method, Request, Response};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The config slice requests are served from; swapped on every reconcile so
/// allowlist/token changes apply without a restart.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub policy: AuthPolicy,
    pub wow_dir: String,
    pub probes_enabled: bool,
}

pub struct ServerCtx {
    snapshot: RwLock<Snapshot>,
    lockout: Mutex<Lockout>,
    started: Instant,
}

impl Default for ServerCtx {
    fn default() -> Self {
        Self {
            snapshot: RwLock::new(Snapshot::default()),
            lockout: Mutex::new(Lockout::default()),
            started: Instant::now(),
        }
    }
}

impl ServerCtx {
    pub fn set_snapshot(&self, s: Snapshot) {
        *self.snapshot.write().unwrap_or_else(|p| p.into_inner()) = s;
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Admission + routing for one request: `(status, JSON body)`.
    pub fn route(
        &self,
        peer: IpAddr,
        method: &Method,
        path: &str,
        auth: Option<&str>,
    ) -> (u16, Value) {
        let snap = self.snapshot();
        let IpAddr::V4(ip) = peer else {
            return (403, json!({ "error": "forbidden" }));
        };
        let admitted = {
            let mut lockout = self.lockout.lock().unwrap_or_else(|p| p.into_inner());
            admit(&snap.policy, &mut lockout, ip, auth, Instant::now())
        };
        if let Err(d) = admitted {
            return (d.status(), json!({ "error": d.code() }));
        }
        match (method, path) {
            (&Method::GET, "/dev/v1/health") => (200, self.health(&snap)),
            _ => (404, json!({ "error": "not_found" })),
        }
    }

    fn health(&self, snap: &Snapshot) -> Value {
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        health(&HealthInput {
            wow_dir: &snap.wow_dir,
            probes_enabled: snap.probes_enabled,
            uptime: self.started.elapsed(),
            now_unix,
        })
    }
}

fn respond(ctx: &ServerCtx, peer: SocketAddr, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let path = req.uri().path();
    let (status, body) = ctx.route(peer.ip(), req.method(), path, auth);
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    // Audit line (spec §7): never headers, query or bodies.
    eprintln!(
        "RL Companion dev bridge: {} {} {path} {status} {}B",
        peer.ip(),
        req.method(),
        bytes.len()
    );
    let mut res = Response::new(Full::new(Bytes::from(bytes)));
    *res.status_mut() = hyper::StatusCode::from_u16(status).unwrap_or_default();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    res
}

async fn serve_conn(
    ctx: Arc<ServerCtx>,
    acceptor: TlsAcceptor,
    tcp: tokio::net::TcpStream,
    peer: SocketAddr,
) {
    let Ok(Ok(tls)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await else {
        return;
    };
    let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
        let res = respond(&ctx, peer, &req);
        async move { Ok::<_, Infallible>(res) }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(tls), svc)
        .await;
}

/// Accept until `stop` fires; returning drops the listener (port released).
pub async fn serve(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ServerCtx>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut stop => return,
            accepted = listener.accept() => {
                let Ok((tcp, peer)) = accepted else { continue };
                tokio::spawn(serve_conn(ctx.clone(), acceptor.clone(), tcp, peer));
            }
        }
    }
}
