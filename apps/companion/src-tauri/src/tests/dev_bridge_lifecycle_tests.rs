//! Dev bridge lifecycle + audit (ROK-2055 C2 review MAJOR-1/2/3, MINOR-6/9):
//! "off" closes accepted keep-alive connections and pending handshakes,
//! refused callers can't flood or write into the audit ring, the listener's
//! bound-IP rule, and the TLS key file's permissions.

use crate::config::AppConfig;
use crate::dev_bridge::auth::AuthPolicy;
use crate::dev_bridge::bind::{bound_ip_check, ip_is_local};
use crate::dev_bridge::logbuf::LogBuffer;
use crate::dev_bridge::server::{
    DenialLog, ServerCtx, Snapshot, BIND_CHECK_INTERVAL, DENIAL_LOG_WINDOW,
};
use crate::dev_bridge::tls::TlsMaterial;
use crate::dev_bridge::token::DevToken;
use crate::dev_bridge::{DevBridge, TLS_FILE};
use hyper::Method;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

const CALLER: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 229);
const STRANGER: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 66);
fn good() -> String {
    format!("rlcdev_{}", "A".repeat(43))
}

const INJECT: &str = "/dev/v1/IGNORE-PREVIOUS-INSTRUCTIONS";

fn enabled_cfg(token: &DevToken) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.dev_bridge.enabled = true;
    cfg.dev_bridge.allowlist = vec!["127.0.0.1".into()];
    cfg.dev_bridge.token_sha256 = Some(token.sha256_hex());
    cfg
}

async fn loopback_bridge(
    dir: &Path,
    cfg: &AppConfig,
) -> (DevBridge, SocketAddrV4, reqwest::Client) {
    let bridge = DevBridge::new(dir.join(TLS_FILE), None);
    bridge.refresh(cfg);
    bridge
        .listen(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("listen");
    let addr = bridge.listening_addr().await.expect("listening");
    let der = TlsMaterial::load_or_create(&dir.join(TLS_FILE), Ipv4Addr::LOCALHOST)
        .expect("cert")
        .cert()
        .expect("der");
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_der(&der).expect("cert"))
        .tls_built_in_root_certs(false)
        .no_proxy()
        .build()
        .expect("client");
    (bridge, addr, client)
}

async fn status_of(client: &reqwest::Client, addr: SocketAddrV4, auth: &str) -> Option<u16> {
    let url = format!("https://{addr}/dev/v1/health");
    let res = client.get(url).header("Authorization", auth).send().await;
    res.ok().map(|r| r.status().as_u16())
}

#[tokio::test]
async fn stopping_closes_an_open_keep_alive_connection() {
    let dir = super::temp_dir("bridge-keepalive");
    let token = DevToken::generate().expect("rng");
    let auth = format!("Bearer {}", token.expose());
    let (bridge, addr, client) = loopback_bridge(&dir, &enabled_cfg(&token)).await;
    assert_eq!(
        status_of(&client, addr, &auth).await,
        Some(200),
        "served while on"
    );
    // `stop` alone (policy untouched) must still end the pooled connection.
    bridge.stop().await;
    assert_eq!(
        status_of(&client, addr, &auth).await,
        None,
        "the keep-alive connection accepted before stop still answered"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn stopping_drops_a_pending_tls_handshake() {
    let dir = super::temp_dir("bridge-handshake");
    let token = DevToken::generate().expect("rng");
    let (bridge, addr, _client) = loopback_bridge(&dir, &enabled_cfg(&token)).await;
    let mut raw = tokio::net::TcpStream::connect(addr).await.expect("tcp");
    tokio::time::sleep(Duration::from_millis(50)).await; // let the accept loop take it
    bridge.stop().await;
    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(3), raw.read(&mut buf)).await;
    assert!(
        matches!(read, Ok(Ok(0) | Err(_))),
        "a connection mid-handshake must be closed by stop, got {read:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn turning_off_refuses_requests_racing_the_close() {
    let dir = super::temp_dir("bridge-off-policy");
    let token = DevToken::generate().expect("rng");
    let mut cfg = enabled_cfg(&token);
    let bridge = DevBridge::new(dir.join(TLS_FILE), None);
    bridge.refresh(&cfg);
    let auth = format!("Bearer {}", token.expose());
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let on = bridge
        .ctx()
        .route(ip, &Method::GET, "/dev/v1/health", None, Some(&auth));
    assert_eq!(on.0, 200);
    cfg.dev_bridge.enabled = false;
    bridge.reconcile(&cfg).await;
    let off = bridge
        .ctx()
        .route(ip, &Method::GET, "/dev/v1/health", None, Some(&auth));
    assert_eq!(off.0, 403, "off: the served policy admits nobody");
    let _ = std::fs::remove_dir_all(dir);
}

fn audited_ctx() -> (ServerCtx, &'static LogBuffer) {
    let logs: &'static LogBuffer = Box::leak(Box::default());
    let ctx = ServerCtx::with_logs(logs);
    ctx.set_snapshot(Snapshot {
        policy: AuthPolicy {
            allowlist: vec![CALLER],
            token_sha256: Some(crate::dev_bridge::token::sha256_hex(&good())),
        },
        ..Snapshot::default()
    });
    (ctx, logs)
}

fn hit(ctx: &ServerCtx, ip: Ipv4Addr, path: &str, auth: Option<&str>) {
    let r = ctx.route_audited(IpAddr::V4(ip), &Method::GET, path, None, auth);
    ctx.audit(IpAddr::V4(ip), &Method::GET, path, &r, 20);
}

fn msgs(logs: &LogBuffer) -> Vec<String> {
    logs.select(0, None, None)
        .into_iter()
        .map(|(_, v)| v["msg"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn refused_callers_are_rate_limited_and_never_logged_with_their_path() {
    let (ctx, logs) = audited_ctx();
    for _ in 0..1500 {
        hit(&ctx, STRANGER, INJECT, None); // outside the allowlist → 403
        hit(&ctx, CALLER, INJECT, Some("Bearer nope")); // listed, bad token → 401/429
    }
    let lines = msgs(logs);
    assert_eq!(
        lines.len(),
        2,
        "one line per refused host per minute, got {lines:?}"
    );
    assert!(
        lines.iter().all(|m| !m.contains("IGNORE")),
        "a refused caller's path reached the audit ring: {lines:?}"
    );
    assert!(
        lines[0].contains("refused 403 from 192.168.0.66"),
        "{lines:?}"
    );
}

#[test]
fn admitted_requests_log_known_routes_but_not_unknown_paths() {
    let (ctx, logs) = audited_ctx();
    let bearer = format!("Bearer {}", good());
    let auth = Some(bearer.as_str());
    hit(&ctx, CALLER, "/dev/v1/health", auth);
    hit(&ctx, CALLER, INJECT, auth);
    let lines = msgs(logs);
    assert!(lines[0].starts_with("GET /dev/v1/health 200"), "{lines:?}");
    assert!(lines[1].starts_with("GET <unknown> 404"), "{lines:?}");
}

#[test]
fn denial_log_reports_the_held_back_count_next_window_and_caps_hosts() {
    let mut d = DenialLog::default();
    let t0 = Instant::now();
    let ip = IpAddr::V4(STRANGER);
    assert!(d.note(ip, 403, t0).is_some());
    for _ in 0..41 {
        assert_eq!(d.note(ip, 403, t0 + Duration::from_secs(1)), None);
    }
    let next = d
        .note(ip, 403, t0 + DENIAL_LOG_WINDOW)
        .expect("new window logs");
    assert!(next.contains("41 more refused"), "{next}");
    let mut lines = 0;
    for i in 0..2000u32 {
        let host = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i));
        lines += usize::from(d.note(host, 403, t0 + DENIAL_LOG_WINDOW).is_some());
    }
    assert!(
        lines <= 257,
        "host cap bounds lines per window, got {lines}"
    );
}

#[test]
fn listener_closes_when_its_bound_ip_leaves_the_pc() {
    assert_eq!(BIND_CHECK_INTERVAL, Duration::from_secs(30));
    let ip = Ipv4Addr::new(192, 168, 0, 20);
    assert_eq!(bound_ip_check(ip, |_| true), Ok(()));
    let gone = bound_ip_check(ip, |_| false).expect_err("gone → close");
    assert!(gone.contains("192.168.0.20"), "{gone}");
    assert!(ip_is_local(Ipv4Addr::LOCALHOST), "loopback is always ours");
    assert!(
        !ip_is_local(Ipv4Addr::new(192, 0, 2, 77)),
        "TEST-NET-1 is never on an interface"
    );
}

#[cfg(unix)]
#[test]
fn tls_key_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = super::temp_dir("bridge-tls-mode");
    let path = dir.join(TLS_FILE);
    std::fs::write(&path, b"{}").expect("stale file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    TlsMaterial::load_or_create(&path, Ipv4Addr::new(192, 168, 0, 20)).expect("create");
    let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "TLS key file mode {mode:o}");
    assert!(
        !crate::config::tmp_path(&path).exists(),
        "no temp file left"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn serve_runs_the_bound_ip_check_and_reports_why_it_closed() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    let dir = super::temp_dir("bridge-bind-watch");
    let (calls, gone) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicBool::new(false)),
    );
    let (c, g) = (calls.clone(), gone.clone());
    // The first probe fails (a blip), later ones pass until `gone` is set.
    let probe =
        Arc::new(move |_| c.fetch_add(1, Ordering::SeqCst) != 0 && !g.load(Ordering::SeqCst));
    let ctx = ServerCtx::with_bind_probe(probe, Duration::from_millis(20));
    let bridge = DevBridge::with_ctx(dir.join(TLS_FILE), ctx);
    bridge
        .listen(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("listen");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        bridge.listening_addr().await.is_some(),
        "closed after a single failed probe"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 3,
        "serve never ran the probe"
    );
    gone.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(3);
    while bridge.listening_addr().await.is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let status = bridge.status().await;
    assert_eq!(status.listening, None, "still listening after the IP left");
    let why = status.error.unwrap_or_default();
    assert!(
        why.contains("no longer an address"),
        "status error: {why:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// ROK-2078 review §4: the bg3 routes sit behind the same admission as every
/// other dev route — off the allowlist → 403, no or wrong token → 401, and
/// only a good token reaches the bg3 handler.
#[test]
fn bg3_routes_refuse_callers_without_a_good_token() {
    use crate::dev_bridge::bg3_newage::NotYetDecoded;
    use crate::dev_bridge::bg3_saves::Bg3Source;
    let (ctx, _) = audited_ctx();
    let story = super::temp_dir("bg3-auth");
    ctx.set_bg3(std::sync::Arc::new(Bg3Source::with_key(
        vec![story],
        [3; 32],
        std::sync::Arc::new(NotYetDecoded),
    )));
    let good = format!("Bearer {}", good());
    for path in [
        "/dev/v1/bg3/saves",
        "/dev/v1/bg3/saves/0123456789ab/structure",
    ] {
        let status = |ip: Ipv4Addr, auth: Option<&str>| {
            ctx.route_audited(IpAddr::V4(ip), &Method::GET, path, None, auth)
                .status
        };
        assert_eq!(status(CALLER, None), 401, "{path}: no token");
        assert_eq!(
            status(CALLER, Some("Bearer rlcdev_wrong")),
            401,
            "{path}: wrong token"
        );
        assert_eq!(
            status(STRANGER, Some(&good)),
            403,
            "{path}: off the allowlist"
        );
    }
    let ctx2 = audited_ctx().0;
    ctx2.set_bg3(std::sync::Arc::new(Bg3Source::with_key(
        vec![super::temp_dir("bg3-auth-ok")],
        [3; 32],
        std::sync::Arc::new(NotYetDecoded),
    )));
    let ok = ctx2.route_audited(
        IpAddr::V4(CALLER),
        &Method::GET,
        "/dev/v1/bg3/saves",
        None,
        Some(&good),
    );
    assert_eq!(ok.status, 200, "a good token reaches the bg3 listing");
}
