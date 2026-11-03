//! The HTTPS listener and `/dev/v1/*` router. Admission (auth.rs) runs before
//! routing, so an unauthenticated caller learns nothing about which paths exist.

use super::auth::{admit, AuthPolicy, Lockout};
use super::bind;
use super::health::{health, HealthInput};
use super::logbuf::{self, LogBuffer};
use super::paging::parse_query;
use super::routes::{self, CollectorsFn};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::{header, Method, Request, Response};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// hyper drops a connection that hasn't sent full headers in this long
/// (needs the `TokioTimer`); also bounds how long an idle keep-alive waits.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Open connections (including pending TLS handshakes) served at once.
pub const MAX_CONNECTIONS: usize = 32;
/// How long in-flight requests get to finish when the listener closes.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the listener re-checks that its bound IP is still this PC's
/// (spec §7 fallback for the network-profile watch).
pub const BIND_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// Refused requests are logged at most once per host per window, without
/// the path, so a LAN host can neither flush nor write into the audit ring.
pub const DENIAL_LOG_WINDOW: Duration = Duration::from_secs(60);
const DENIAL_LOG_MAX_HOSTS: usize = 256;

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
    collectors: RwLock<Option<CollectorsFn>>,
    logs: Option<&'static LogBuffer>,
    denials: Mutex<DenialLog>,
    closed: Mutex<Option<String>>,
}

/// One routed request: what to send, and whether admission let it through.
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    pub status: u16,
    pub body: Value,
    pub admitted: bool,
}

/// Per-host rate limit for audit lines about refused requests.
#[derive(Debug, Default)]
pub struct DenialLog(HashMap<IpAddr, (Instant, u32)>);

impl DenialLog {
    /// Count a refusal from `ip`. Returns the line to log — the first refusal
    /// per host per window (with the count held back since), else `None`.
    /// Never contains request text. Past the host cap, hosts share one bucket.
    pub fn note(&mut self, ip: IpAddr, status: u16, now: Instant) -> Option<String> {
        let live = |t: &Instant| now.duration_since(*t) < DENIAL_LOG_WINDOW;
        if self.0.len() >= DENIAL_LOG_MAX_HOSTS && !self.0.contains_key(&ip) {
            self.0.retain(|_, (t, _)| live(t));
        }
        let key = if self.0.len() >= DENIAL_LOG_MAX_HOSTS && !self.0.contains_key(&ip) {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            ip
        };
        if let Some((since, held)) = self.0.get_mut(&key) {
            if live(since) {
                *held += 1;
                return None;
            }
        }
        let held = self.0.get(&key).map_or(0, |e| e.1);
        self.0.insert(key, (now, 0));
        let who = if key.is_unspecified() {
            "other hosts".to_owned()
        } else {
            key.to_string()
        };
        Some(format!(
            "refused {status} from {who} (path not logged; {held} more refused since the last line)"
        ))
    }
}

impl Default for ServerCtx {
    fn default() -> Self {
        Self {
            snapshot: RwLock::new(Snapshot::default()),
            lockout: Mutex::new(Lockout::default()),
            started: Instant::now(),
            collectors: RwLock::new(None),
            logs: None,
            denials: Mutex::new(DenialLog::default()),
            closed: Mutex::new(None),
        }
    }
}

impl ServerCtx {
    pub fn set_snapshot(&self, s: Snapshot) {
        *self.snapshot.write().unwrap_or_else(|p| p.into_inner()) = s;
    }

    pub fn set_collectors(&self, f: CollectorsFn) {
        *self.collectors.write().unwrap_or_else(|p| p.into_inner()) = Some(f);
    }

    /// Serve `/dev/v1/logs` from `logs` instead of the process-wide buffer.
    pub fn with_logs(logs: &'static LogBuffer) -> Self {
        Self {
            logs: Some(logs),
            ..Self::default()
        }
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
        query: Option<&str>,
        auth: Option<&str>,
    ) -> (u16, Value) {
        let r = self.route_audited(peer, method, path, query, auth);
        (r.status, r.body)
    }

    /// [`Self::route`], also saying whether admission passed (for the audit).
    pub fn route_audited(
        &self,
        peer: IpAddr,
        method: &Method,
        path: &str,
        query: Option<&str>,
        auth: Option<&str>,
    ) -> Routed {
        let snap = self.snapshot();
        let denied = |status: u16, code: &str| Routed {
            status,
            body: json!({ "error": code }),
            admitted: false,
        };
        let IpAddr::V4(ip) = peer else {
            return denied(403, "forbidden");
        };
        let admitted = {
            let mut lockout = self.lockout.lock().unwrap_or_else(|p| p.into_inner());
            admit(&snap.policy, &mut lockout, ip, auth, Instant::now())
        };
        if let Err(d) = admitted {
            return denied(d.status(), d.code());
        }
        let (status, body) = self.dispatch(&snap, method, path, query);
        Routed {
            status,
            body,
            admitted: true,
        }
    }

    fn dispatch(
        &self,
        snap: &Snapshot,
        method: &Method,
        path: &str,
        query: Option<&str>,
    ) -> (u16, Value) {
        match (method, path) {
            (&Method::GET, "/dev/v1/health") => (200, self.health(snap)),
            (&Method::GET, _) => {
                let collectors = self
                    .collectors
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let logs = self.logs.unwrap_or_else(logbuf::global);
                routes::get(snap, path, &parse_query(query), collectors.as_ref(), logs)
                    .unwrap_or_else(|| (404, json!({ "error": "not_found" })))
            }
            _ => (404, json!({ "error": "not_found" })),
        }
    }

    /// Write the audit line for one response (spec §7: never headers, query
    /// or bodies). Refused callers are rate-limited and never logged with
    /// their path; an admitted 404 logs `<unknown>`, not the raw path.
    pub fn audit(&self, peer: IpAddr, method: &Method, path: &str, r: &Routed, bytes: usize) {
        let line = if r.admitted {
            let shown = if r.status == 404 { "<unknown>" } else { path };
            Some(format!("{method} {shown} {} {bytes}B from {peer}", r.status))
        } else {
            let mut d = self.denials.lock().unwrap_or_else(|p| p.into_inner());
            d.note(peer, r.status, Instant::now())
        };
        if let Some(line) = line {
            self.log(if r.status >= 400 { "warn" } else { "info" }, &line);
        }
    }

    fn log(&self, level: &'static str, msg: &str) {
        match self.logs {
            Some(buf) => buf.push(level, "dev_bridge", msg, String::new()),
            None => logbuf::log(level, "dev_bridge", msg),
        }
    }

    /// Why the listener closed itself (bound IP gone), if it did.
    pub fn closed_reason(&self) -> Option<String> {
        self.closed.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub(crate) fn set_closed_reason(&self, why: Option<String>) {
        *self.closed.lock().unwrap_or_else(|p| p.into_inner()) = why;
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

async fn respond(
    ctx: Arc<ServerCtx>,
    peer: SocketAddr,
    req: Request<Incoming>,
) -> Response<Full<Bytes>> {
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().map(str::to_owned);
    // Routes read files (SavedVariables up to 16 MiB): keep them off the
    // shared async workers.
    let job = (ctx.clone(), method.clone(), path.clone());
    let routed = tokio::task::spawn_blocking(move || {
        let (c, m, p) = job;
        c.route_audited(peer.ip(), &m, &p, query.as_deref(), auth.as_deref())
    })
    .await
    .unwrap_or_else(|_| Routed {
        status: 500,
        body: json!({ "error": "internal" }),
        admitted: true,
    });
    let bytes = serde_json::to_vec(&routed.body).unwrap_or_default();
    ctx.audit(peer.ip(), &method, &path, &routed, bytes.len());
    let mut res = Response::new(Full::new(Bytes::from(bytes)));
    *res.status_mut() = hyper::StatusCode::from_u16(routed.status)
        .unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    res
}

/// Serve one connection until it ends or `shutdown` flips: a pending TLS
/// handshake is dropped, an open connection finishes its in-flight request
/// (bounded by [`DRAIN_TIMEOUT`]) and is closed — keep-alive does not outlive "off".
async fn serve_conn(
    ctx: Arc<ServerCtx>,
    acceptor: TlsAcceptor,
    tcp: tokio::net::TcpStream,
    peer: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) {
    let tls = tokio::select! {
        r = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)) => match r {
            Ok(Ok(tls)) => tls,
            _ => return,
        },
        _ = shutdown.wait_for(|stop| *stop) => return,
    };
    let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
        let ctx = ctx.clone();
        async move { Ok::<_, Infallible>(respond(ctx, peer, req).await) }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    let conn = builder.serve_connection(hyper_util::rt::TokioIo::new(tls), svc);
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => {}
        _ = shutdown.wait_for(|stop| *stop) => {
            conn.as_mut().graceful_shutdown();
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, conn).await;
        }
    }
}

/// True while the listener may keep running on `bound`.
fn bound_ip_ok(ctx: &ServerCtx, bound: Option<IpAddr>) -> bool {
    let Some(IpAddr::V4(ip)) = bound else {
        return true;
    };
    match bind::bound_ip_check(ip, bind::ip_is_local) {
        Ok(()) => true,
        Err(why) => {
            ctx.log("warn", &why);
            ctx.set_closed_reason(Some(why));
            false
        }
    }
}

/// Accept until `stop` fires or the bound IP leaves this PC; then drop the
/// listener (port released) and close every connection it accepted.
pub async fn serve(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ServerCtx>,
    mut stop: oneshot::Receiver<()>,
) {
    let (shutdown, shutdown_rx) = watch::channel(false);
    let mut conns = JoinSet::new();
    let bound = listener.local_addr().ok().map(|a| a.ip());
    let mut check = tokio::time::interval(BIND_CHECK_INTERVAL);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        while conns.try_join_next().is_some() {}
        tokio::select! {
            _ = &mut stop => break,
            _ = check.tick() => if !bound_ip_ok(&ctx, bound) { break },
            accepted = listener.accept() => {
                let Ok((tcp, peer)) = accepted else { continue };
                if conns.len() >= MAX_CONNECTIONS {
                    continue; // dropping `tcp` refuses it
                }
                conns.spawn(serve_conn(ctx.clone(), acceptor.clone(), tcp, peer, shutdown_rx.clone()));
            }
        }
    }
    drop(listener);
    let _ = shutdown.send(true);
    let drain = async { while conns.join_next().await.is_some() {} };
    if tokio::time::timeout(DRAIN_TIMEOUT * 2, drain).await.is_err() {
        conns.abort_all();
        while conns.join_next().await.is_some() {}
    }
}
