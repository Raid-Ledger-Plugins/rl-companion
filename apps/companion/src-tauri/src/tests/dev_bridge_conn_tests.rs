//! Dev bridge connection budget (ROK-2055 C2 delta review MAJOR-D1/MINOR-D2):
//! a LAN host outside the allowlist can't fill the slots the allowlisted
//! caller needs, and a refused caller can't park on a keep-alive connection.

use crate::config::AppConfig;
use crate::dev_bridge::conn_budget::{
    ConnBudget, MAX_CONNECTIONS, OUTSIDER_MAX_PER_IP, OUTSIDER_MAX_TOTAL,
};
use crate::dev_bridge::tls::TlsMaterial;
use crate::dev_bridge::token::DevToken;
use crate::dev_bridge::{DevBridge, TLS_FILE};
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::Duration;

fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 168, 0, last))
}

#[test]
fn outsiders_have_their_own_small_budget_and_never_take_allowlisted_slots() {
    let b = ConnBudget::default();
    let mut held = Vec::new();
    for host in 1..=20u8 {
        for _ in 0..5 {
            held.extend(b.try_take(ip(host), false));
        }
    }
    assert_eq!(held.len(), OUTSIDER_MAX_TOTAL, "outsiders capped in total");
    assert!(
        b.try_take(ip(99), false).is_none(),
        "a fresh outsider past the total cap is refused"
    );
    let listed: Vec<_> = (0..MAX_CONNECTIONS)
        .filter_map(|_| b.try_take(ip(229), true))
        .collect();
    assert_eq!(
        listed.len(),
        MAX_CONNECTIONS,
        "outsiders took allowlisted slots: {:?}",
        b.in_use()
    );
    assert!(b.try_take(ip(229), true).is_none(), "allowlisted cap holds");
    held.clear();
    let one_host: Vec<_> = (0..5).filter_map(|_| b.try_take(ip(7), false)).collect();
    assert_eq!(one_host.len(), OUTSIDER_MAX_PER_IP, "per-host cap");
    drop(one_host);
    drop(listed);
    assert_eq!(b.in_use(), (0, 0), "slots are released when dropped");
}

fn cfg(token: &DevToken, allow: &str) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.dev_bridge.enabled = true;
    cfg.dev_bridge.allowlist = vec![allow.into()];
    cfg.dev_bridge.token_sha256 = Some(token.sha256_hex());
    cfg
}

async fn bridge_on(dir: &Path, cfg: &AppConfig) -> (DevBridge, SocketAddrV4, reqwest::Client) {
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
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client");
    (bridge, addr, client)
}

#[tokio::test]
async fn stalled_outsider_handshakes_do_not_lock_out_the_allowlisted_caller() {
    let dir = super::temp_dir("bridge-starve");
    let token = DevToken::generate().expect("rng");
    // Loopback starts OUTSIDE the allowlist: every raw connection is an outsider.
    let (bridge, addr, client) = bridge_on(&dir, &cfg(&token, "192.168.0.229")).await;
    let mut stalled = Vec::new();
    for _ in 0..(MAX_CONNECTIONS + 8) {
        stalled.extend(tokio::net::TcpStream::connect(addr).await.ok());
    }
    tokio::time::sleep(Duration::from_millis(300)).await; // let the accept loop take them
                                                          // Now loopback is the allowlisted caller; the stalled sockets stay open.
    bridge.refresh(&cfg(&token, "127.0.0.1"));
    let res = client
        .get(format!("https://{addr}/dev/v1/health"))
        .header("Authorization", format!("Bearer {}", token.expose()))
        .send()
        .await;
    assert_eq!(
        res.as_ref().ok().map(|r| r.status().as_u16()),
        Some(200),
        "allowlisted caller starved by {} stalled outsider connections: {res:?}",
        stalled.len()
    );
    drop(stalled);
    bridge.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn refused_responses_close_the_connection() {
    let dir = super::temp_dir("bridge-refused-close");
    let token = DevToken::generate().expect("rng");
    let (bridge, addr, client) = bridge_on(&dir, &cfg(&token, "192.168.0.229")).await;
    let res = client
        .get(format!("https://{addr}/dev/v1/health"))
        .send()
        .await
        .expect("403 response");
    assert_eq!(res.status().as_u16(), 403);
    let conn = res
        .headers()
        .get("connection")
        .and_then(|v| v.to_str().ok());
    assert_eq!(conn, Some("close"), "refused keep-alive stays open");
    bridge.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_handler_failure_closes_without_an_off_contract_500_body() {
    let dir = super::temp_dir("bridge-handler-panic");
    let token = DevToken::generate().expect("rng");
    let (bridge, addr, client) = bridge_on(&dir, &cfg(&token, "127.0.0.1")).await;
    bridge.set_collectors_source(std::sync::Arc::new(|| panic!("collector source blew up")));
    let res = client
        .get(format!("https://{addr}/dev/v1/collectors"))
        .header("Authorization", format!("Bearer {}", token.expose()))
        .send()
        .await;
    assert!(
        res.is_err(),
        "expected the connection to close with no response; got status {:?}",
        res.as_ref().ok().map(|r| r.status().as_u16())
    );
    bridge.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}
