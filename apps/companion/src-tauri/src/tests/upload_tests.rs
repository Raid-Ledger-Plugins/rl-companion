use super::*;
use crate::runtime::effect;
use crate::token::DeviceToken;
use crate::upload::{backoff, classify, parse_retry_after, Attempt, UploadOutcome, Uploader};

pub const INGEST: &str = "/api/companion/v1/messages/wow-ledgerlink";

pub fn token() -> DeviceToken {
    DeviceToken::parse(TOKEN).expect("fixture token is valid")
}

fn body() -> Value {
    json!({ "contract": "wow-ledgerlink" })
}

async fn send_to(server: &MockServer, clock: &Clock) -> UploadOutcome {
    Uploader::new(&server.uri())
        .expect("mock URL")
        .send(&token(), "wow-ledgerlink", &body(), clock.sleeper())
        .await
}

async fn mount_sequence(server: &MockServer, responses: Vec<ResponseTemplate>) {
    for r in responses {
        Mock::given(method("POST"))
            .and(path(INGEST))
            .respond_with(r)
            .up_to_n_times(1)
            .mount(server)
            .await;
    }
}

async fn hits(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

#[test]
fn backoff_schedule_doubles_then_caps_at_60s() {
    let waits: Vec<u64> = (0..7).map(|n| backoff(n).as_secs()).collect();
    assert_eq!(waits, vec![2, 4, 8, 16, 32, 60, 60]);
}

#[test]
fn retry_after_is_parsed_and_clamped() {
    assert_eq!(parse_retry_after(Some(" 7 ")).as_secs(), 7);
    assert_eq!(parse_retry_after(Some("0")).as_secs(), 1);
    assert_eq!(parse_retry_after(Some("99999")).as_secs(), 600);
    assert_eq!(
        parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")).as_secs(),
        60
    );
    assert_eq!(parse_retry_after(None).as_secs(), 60);
}

#[test]
fn terminal_vs_retryable_codes() {
    let retryable = |s: u16| matches!(classify(s, None, b""), Attempt::Retry { .. });
    for s in [408, 429, 500, 502, 503, 504] {
        assert!(retryable(s), "{s} must be retried");
    }
    for s in [400, 403, 404, 409, 413, 415] {
        assert!(!retryable(s), "{s} must be terminal");
    }
    assert_eq!(
        classify(202, None, b""),
        Attempt::Done(UploadOutcome::Accepted)
    );
    assert_eq!(
        classify(401, None, b""),
        Attempt::Done(UploadOutcome::Unauthorized)
    );
    let rejected = classify(
        400,
        None,
        br#"{"code":"invalid_payload","message":"m","issues":[{"path":"payload.exportString","code":"BAD_HEADER"}]}"#,
    );
    assert_eq!(
        rejected,
        Attempt::Done(UploadOutcome::Rejected {
            status: 400,
            code: "invalid_payload".into(),
            issue: Some("BAD_HEADER".into()),
        })
    );
    assert_eq!(
        classify(413, None, b"not json"),
        Attempt::Done(UploadOutcome::Rejected {
            status: 413,
            code: "http_413".into(),
            issue: None
        })
    );
}

#[test]
fn effects_record_only_settled_outcomes() {
    let rejected = UploadOutcome::Rejected {
        status: 400,
        code: "invalid_payload".into(),
        issue: Some("ROSTER_HELD".into()),
    };
    let fx = effect(&rejected);
    assert!(
        fx.record && !fx.stop && !fx.ok,
        "terminal reject is recorded, not retried: {fx:?}"
    );
    assert_eq!(fx.text, "Rejected 400: invalid_payload (ROSTER_HELD)");
    let fx = effect(&UploadOutcome::Accepted);
    assert!(fx.record && fx.ok);
    let fx = effect(&UploadOutcome::Unauthorized);
    assert!(
        !fx.record && fx.stop,
        "401 stops the batch and records nothing: {fx:?}"
    );
    let fx = effect(&UploadOutcome::GaveUp {
        reason: "server error 503".into(),
    });
    assert!(
        !fx.record && !fx.stop,
        "a retryable failure is tried again next scan: {fx:?}"
    );
}

#[tokio::test]
async fn server_errors_back_off_then_succeed() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            ResponseTemplate::new(503),
            ResponseTemplate::new(500),
            ResponseTemplate::new(202),
        ],
    )
    .await;
    let clock = Clock::default();
    assert_eq!(send_to(&server, &clock).await, UploadOutcome::Accepted);
    assert_eq!(clock.waits(), vec![2, 4]);
}

#[tokio::test]
async fn rate_limit_honours_retry_after() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "7")
                .set_body_json(json!({ "code": "rate_limited", "message": "slow down" })),
            ResponseTemplate::new(202).set_body_json(json!({ "status": "accepted" })),
        ],
    )
    .await;
    let clock = Clock::default();
    assert_eq!(send_to(&server, &clock).await, UploadOutcome::Accepted);
    assert_eq!(clock.waits(), vec![7]);
}

#[tokio::test]
async fn terminal_400_is_not_retried_and_keeps_the_issue_code() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![ResponseTemplate::new(400).set_body_json(json!({
            "code": "invalid_payload", "message": "Rejected",
            "issues": [{ "path": "payload.exportString", "code": "RULESET_REQUIRED" }]
        }))],
    )
    .await;
    let clock = Clock::default();
    let outcome = send_to(&server, &clock).await;
    assert_eq!(
        outcome,
        UploadOutcome::Rejected {
            status: 400,
            code: "invalid_payload".into(),
            issue: Some("RULESET_REQUIRED".into())
        }
    );
    assert_eq!(hits(&server).await, 1, "a 400 is sent once");
    assert!(clock.waits().is_empty());
}

#[tokio::test]
async fn unauthorized_is_reported_once() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![ResponseTemplate::new(401)
            .set_body_json(json!({ "code": "unauthorized", "message": "x" }))],
    )
    .await;
    let clock = Clock::default();
    assert_eq!(send_to(&server, &clock).await, UploadOutcome::Unauthorized);
    assert_eq!(hits(&server).await, 1);
}

#[tokio::test]
async fn persistent_failure_gives_up_after_five_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(INGEST))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let clock = Clock::default();
    let outcome = send_to(&server, &clock).await;
    assert_eq!(
        outcome,
        UploadOutcome::GaveUp {
            reason: "server error 502".into()
        }
    );
    assert_eq!(hits(&server).await, 5);
    assert_eq!(clock.waits(), vec![2, 4, 8, 16]);
}
