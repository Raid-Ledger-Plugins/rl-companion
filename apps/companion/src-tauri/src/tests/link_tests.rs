use super::*;
use crate::link::{classify_poll, DeviceInfo, LinkError, SelfCheck};
use crate::token::{DeviceToken, MemoryStore, TokenStore};
use wiremock::matchers::header;

fn device() -> DeviceInfo {
    DeviceInfo {
        name: "Gaming PC".into(),
        os: "windows".into(),
        os_version: None,
        app_version: "0.1.0".into(),
    }
}

#[tokio::test]
async fn start_sends_contract_body_and_parses_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(START))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "deviceCode": DEVICE_CODE, "userCode": "BCDF-GHJK",
            "verificationUri": "https://raid.example/link",
            "verificationUriComplete": "https://raid.example/link#code=BCDF-GHJK",
            "expiresIn": 600, "interval": 5
        })))
        .mount(&server)
        .await;
    let start = client(&server)
        .start(&device())
        .await
        .expect("start succeeds");
    assert_eq!(start.user_code, "BCDF-GHJK");
    assert_eq!((start.expires_in, start.interval), (600, 5));
    assert!(
        !format!("{start:?}").contains(DEVICE_CODE),
        "Debug must not print the device code"
    );
    let reqs = server.received_requests().await.unwrap_or_default();
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).expect("json body");
    assert_eq!(
        body,
        json!({ "device": { "name": "Gaming PC", "os": "windows", "appVersion": "0.1.0" } }),
        "osVersion omitted, camelCase keys"
    );
}

#[tokio::test]
async fn start_surfaces_rate_limit_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(START))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(json!({ "code": "rate_limited", "message": "x" })),
        )
        .mount(&server)
        .await;
    let err = client(&server)
        .start(&device())
        .await
        .expect_err("429 is an error");
    assert_eq!(
        err,
        LinkError::Http {
            status: 429,
            code: Some("rate_limited".into())
        }
    );
}

#[tokio::test]
async fn self_check_sends_bearer_and_maps_200_and_401() {
    let server = MockServer::start().await;
    let token = DeviceToken::parse(TOKEN).expect("valid token");
    Mock::given(method("GET"))
        .and(path(SELF_CHECK))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "deviceId": 7, "name": "Gaming PC", "createdAt": "2026-10-28T18:04:31.512Z" }),
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(SELF_CHECK))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({ "code": "unauthorized", "message": "x" })),
        )
        .mount(&server)
        .await;
    let c = client(&server);
    assert_eq!(
        c.self_check(&token).await,
        Ok(SelfCheck::Valid {
            device_id: 7,
            name: "Gaming PC".into()
        })
    );
    assert_eq!(c.self_check(&token).await, Ok(SelfCheck::Unauthorized));
}

#[test]
fn classify_rejects_a_malformed_token_on_success() {
    let body =
        json!({ "token": "rlcd_short", "tokenType": "Bearer", "device": { "id": 1, "name": "x" } })
            .to_string();
    assert!(matches!(
        classify_poll(200, &body, None),
        Err(LinkError::Protocol(_))
    ));
}

#[test]
fn token_parse_and_redaction() {
    let t = DeviceToken::parse(TOKEN).expect("valid");
    assert_eq!(t.hint(), "_xyz");
    assert!(!format!("{t:?}").contains("AbCd"), "Debug is redacted");
    assert!(DeviceToken::parse("rlcd_tooshort").is_none());
    assert!(DeviceToken::parse(&TOKEN.replace("rlcd_", "xxxx_")).is_none());
    assert!(DeviceToken::parse(&format!("{TOKEN}!")).is_none());
}

#[test]
fn memory_store_is_keyed_by_origin() {
    let store = MemoryStore::default();
    let t = DeviceToken::parse(TOKEN).expect("valid");
    store.save("https://raid.gamernight.net", &t).expect("save");
    assert_eq!(
        store.load("https://raid.gamernight.net").expect("load"),
        Some(t)
    );
    assert_eq!(
        store.load("https://slot-1.gamernight.net").expect("load"),
        None
    );
    store.clear("https://raid.gamernight.net").expect("clear");
    assert_eq!(
        store.load("https://raid.gamernight.net").expect("load"),
        None
    );
}
