//! Dev bridge listener end to end over HTTPS on loopback (ROK-2055 C2-L1):
//! off-by-default (AC-2), 401/403/lockout (AC-4), hash-only token + Rotate
//! (AC-5), TLS with SAN = bound IP, `/dev/v1/health` vs the pinned schema.

use super::dev_bridge_unit_tests::schema_errors;
use crate::config::AppConfig;
use crate::dev_bridge::commands::{apply_allowlist, apply_bind, apply_enabled, apply_rotate};
use crate::dev_bridge::settings::DevBridgeConfig;
use crate::dev_bridge::tls::TlsMaterial;
use crate::dev_bridge::{desired, DevBridge, ListenerStatus, TLS_FILE};
use crate::state::Shared;
use crate::token::MemoryStore;
use serde_json::{json, Value};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;

struct Live {
    bridge: DevBridge,
    addr: SocketAddrV4,
    client: reqwest::Client,
}

/// A bridge listening on 127.0.0.1:<random> (bind rules bypassed — loopback
/// is refused for real use), with a client that trusts only its certificate.
async fn live(dir: &Path, cfg: &AppConfig) -> Live {
    let bridge = DevBridge::new(dir.join(TLS_FILE));
    bridge.refresh(cfg);
    bridge
        .listen(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("listen");
    let addr = bridge.listening_addr().await.expect("listening");
    let cert = TlsMaterial::load_or_create(&dir.join(TLS_FILE), Ipv4Addr::LOCALHOST)
        .expect("stored cert")
        .cert()
        .expect("der");
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_der(&cert).expect("cert"))
        .tls_built_in_root_certs(false)
        .https_only(true)
        .no_proxy()
        .build()
        .expect("client");
    Live {
        bridge,
        addr,
        client,
    }
}

impl Live {
    async fn get(&self, path: &str, auth: Option<&str>) -> (u16, Value) {
        let mut req = self.client.get(format!("https://{}{path}", self.addr));
        if let Some(a) = auth {
            req = req.header("Authorization", a);
        }
        let res = req
            .send()
            .await
            .expect("TLS handshake against the bound IP succeeds");
        let status = res.status().as_u16();
        (status, res.json().await.expect("JSON body"))
    }
}

fn cfg_with(token_sha256: Option<String>, allowlist: &[&str]) -> AppConfig {
    AppConfig {
        wow_dir: String::new(),
        dev_bridge: DevBridgeConfig {
            enabled: true,
            allowlist: allowlist.iter().map(|s| (*s).to_owned()).collect(),
            token_sha256,
            ..DevBridgeConfig::default()
        },
        ..AppConfig::default()
    }
}

#[tokio::test]
async fn bridge_off_by_default_opens_no_listener() {
    let dir = super::temp_dir("bridge-off");
    let bridge = DevBridge::new(dir.join(TLS_FILE));
    let fresh = AppConfig::default();
    assert!(!fresh.dev_bridge.enabled, "toggle defaults to off");
    let status = bridge.reconcile(&fresh).await;
    assert_eq!(status.listening, None, "default config listens nowhere");
    assert_eq!(bridge.listening_addr().await, None);
    assert!(
        !dir.join(TLS_FILE).exists(),
        "not even a certificate is made while off"
    );

    // Everything else valid, toggle off: the toggle alone keeps it closed.
    let mut ready = cfg_with(Some("ab".repeat(32)), &["192.168.0.50"]);
    ready.dev_bridge.bind_ip = Some("192.168.0.20".into());
    let addr = SocketAddrV4::new(Ipv4Addr::new(192, 168, 0, 20), 47655);
    assert_eq!(
        desired(&ready.dev_bridge),
        Ok(Some(addr)),
        "on → would listen"
    );
    ready.dev_bridge.enabled = false;
    assert_eq!(desired(&ready.dev_bridge), Ok(None), "off → no listener");
    let st = bridge.reconcile(&ready).await;
    assert_eq!(
        st,
        ListenerStatus::default(),
        "off: no listener, no bind attempt, no error"
    );
    assert!(
        !dir.join(TLS_FILE).exists(),
        "no certificate made while off"
    );

    // Toggle on but no token / no address / a refused address: still nothing.
    let mut cfg = cfg_with(None, &["127.0.0.1"]);
    cfg.dev_bridge.bind_ip = Some("192.168.0.20".into());
    assert!(bridge.reconcile(&cfg).await.error.is_some());
    cfg.dev_bridge.token_sha256 = Some("ab".repeat(32));
    cfg.dev_bridge.bind_ip = None;
    assert!(bridge.reconcile(&cfg).await.error.is_some());
    for refused in ["0.0.0.0", "8.8.8.8", "169.254.1.1", "127.0.0.1"] {
        cfg.dev_bridge.bind_ip = Some(refused.into());
        let st = bridge.reconcile(&cfg).await;
        assert_eq!(st.listening, None, "{refused} never binds");
        assert!(st.error.is_some(), "{refused} reports why");
    }
    assert_eq!(bridge.listening_addr().await, None);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn health_answers_the_right_bearer_with_a_schema_valid_body() {
    let dir = super::temp_dir("bridge-health-live");
    let token = crate::dev_bridge::token::DevToken::generate().expect("rng");
    let live = live(&dir, &cfg_with(Some(token.sha256_hex()), &["127.0.0.1"])).await;
    let (status, body) = live
        .get(
            "/dev/v1/health",
            Some(&format!("Bearer {}", token.expose())),
        )
        .await;
    assert_eq!(status, 200, "body: {body}");
    assert_eq!(
        schema_errors("health", &body),
        Vec::<String>::new(),
        "body: {body}"
    );
    let (status, body) = live
        .get("/dev/v1/nope", Some(&format!("Bearer {}", token.expose())))
        .await;
    assert_eq!((status, body), (404, json!({ "error": "not_found" })));
    live.bridge.stop().await;
    assert!(
        std::net::TcpStream::connect(live.addr).is_err(),
        "stop releases the port"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn missing_wrong_or_unlisted_callers_get_401_403_then_429() {
    let dir = super::temp_dir("bridge-deny");
    let token = crate::dev_bridge::token::DevToken::generate().expect("rng");
    let good = format!("Bearer {}", token.expose());
    let live = live(&dir, &cfg_with(Some(token.sha256_hex()), &["127.0.0.1"])).await;
    let unauthorized = json!({ "error": "unauthorized" });
    assert_eq!(
        live.get("/dev/v1/health", None).await,
        (401, unauthorized.clone())
    );
    assert_eq!(
        live.get("/dev/v1/nope", None).await,
        (401, unauthorized.clone()),
        "auth before routing"
    );
    // Failures 3..=9 (the two above count too).
    for _ in 0..7 {
        assert_eq!(
            live.get("/dev/v1/health", Some("Bearer rlcdev_wrong"))
                .await,
            (401, unauthorized.clone())
        );
    }
    // 10th failure locks the address; the 11th request is refused even with the right token.
    assert_eq!(
        live.get("/dev/v1/health", Some("Bearer rlcd_device")).await,
        (401, unauthorized)
    );
    assert_eq!(
        live.get("/dev/v1/health", Some(&good)).await,
        (429, json!({ "error": "rate_limited" }))
    );

    live.bridge
        .refresh(&cfg_with(Some(token.sha256_hex()), &["192.168.0.50"]));
    assert_eq!(
        live.get("/dev/v1/health", Some(&good)).await.0,
        429,
        "lockout is checked first"
    );
    live.bridge.stop().await;

    let dir2 = super::temp_dir("bridge-deny-403");
    let other = live_unlisted(&dir2, &token).await;
    assert_eq!(
        other.get("/dev/v1/health", Some(&good)).await,
        (403, json!({ "error": "forbidden" }))
    );
    other.bridge.stop().await;
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

async fn live_unlisted(dir: &Path, token: &crate::dev_bridge::token::DevToken) -> Live {
    live(dir, &cfg_with(Some(token.sha256_hex()), &["192.168.0.50"])).await
}

#[tokio::test]
async fn token_is_stored_as_a_hash_only_and_rotate_invalidates_the_old_one() {
    let dir = super::temp_dir("bridge-rotate");
    let file = dir.join("config.json");
    let shared = Shared::new(file.clone(), Box::new(MemoryStore::default()));
    apply_enabled(&shared, true).expect("enable");
    apply_bind(&shared, "192.168.0.20", 47655).expect("bind");
    apply_allowlist(&shared, &["127.0.0.1".into(), "127.0.0.1".into()]).expect("allowlist");
    assert_eq!(
        shared.config().dev_bridge.allowlist,
        vec!["127.0.0.1"],
        "deduplicated"
    );
    assert!(apply_bind(&shared, "0.0.0.0", 47655).is_err());
    assert!(apply_allowlist(&shared, &["not-an-ip".into()]).is_err());

    let (first, cfg) = apply_rotate(&shared).expect("create");
    let raw = std::fs::read_to_string(&file).expect("config written");
    assert!(
        !raw.contains("rlcdev_"),
        "config never holds an rlcdev_ value: {raw}"
    );
    assert!(!raw.contains(first.expose()));
    assert!(raw.contains(&first.sha256_hex()), "only the hash is stored");
    assert!(
        shared.token().expect("store").is_none(),
        "nothing written to the keychain store"
    );

    let live = live(&dir, &cfg).await;
    let first_auth = format!("Bearer {}", first.expose());
    assert_eq!(live.get("/dev/v1/health", Some(&first_auth)).await.0, 200);
    let (second, cfg) = apply_rotate(&shared).expect("rotate");
    live.bridge.refresh(&cfg);
    assert_eq!(
        live.get("/dev/v1/health", Some(&first_auth)).await.0,
        401,
        "old token is dead"
    );
    let second_auth = format!("Bearer {}", second.expose());
    assert_eq!(live.get("/dev/v1/health", Some(&second_auth)).await.0, 200);
    let raw = std::fs::read_to_string(&file).expect("config written");
    assert!(
        !raw.contains("rlcdev_"),
        "still no rlcdev_ value after Rotate"
    );
    live.bridge.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}
