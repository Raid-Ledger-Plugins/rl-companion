use super::*;
use crate::runtime::{effect, Runtime};
use crate::token::DeviceToken;
use crate::upload::{
    backoff, classify, parse_retry_after, Attempt, UploadOutcome, Uploader, TERMINAL_CODES,
};

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
    for s in [408, 500, 502, 503, 504] {
        assert!(retryable(s), "{s} must be retried");
    }
    for s in [400, 403, 404, 409, 413, 415, 429] {
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
        Attempt::Done(UploadOutcome::Deferred {
            reason: "unexpected answer 413 from the server".into(),
            status: Some(413),
            code: None,
        }),
        "a proxy's non-JSON 413 is not the server rejecting the message"
    );
    assert_eq!(
        classify(429, Some("7"), b""),
        Attempt::Done(UploadOutcome::RateLimited {
            wait: Duration::from_secs(7)
        })
    );
}

fn error_body(code: &str) -> Vec<u8> {
    json!({ "code": code, "message": "m" })
        .to_string()
        .into_bytes()
}

/// Review MAJOR 2: only the server's own content rejections are final.
#[test]
fn only_content_rejections_are_recorded() {
    for (status, code) in [
        (400, "invalid_envelope"),
        (400, "invalid_payload"),
        (413, "payload_too_large"),
        (400, "captured_at_in_future"),
    ] {
        assert!(TERMINAL_CODES.contains(&code));
        let Attempt::Done(outcome) = classify(status, None, &error_body(code)) else {
            panic!("{status} {code} must not be retried in place");
        };
        assert!(
            matches!(&outcome, UploadOutcome::Rejected { code: c, .. } if c == code),
            "{status} {code} is a terminal reject, got {outcome:?}"
        );
        assert!(effect(&outcome).record, "{code} is recorded as sent");
    }
    let not_about_the_message: [(u16, &[u8]); 7] = [
        (404, br#"{"code":"unknown_contract","message":"m"}"#),
        (404, br#"{"code":"unknown_contract_version","message":"m"}"#),
        (409, br#"{"code":"retired_contract_version","message":"m"}"#),
        (403, b"<html>Cloudflare</html>"),
        (404, b"Not Found"),
        (413, b"<html>nginx</html>"),
        (400, br#"{"code":"something_new","message":"m"}"#),
    ];
    for (status, body) in not_about_the_message {
        let Attempt::Done(outcome) = classify(status, None, body) else {
            panic!("{status} is deferred to the next scan, not retried in place");
        };
        assert!(
            matches!(outcome, UploadOutcome::Deferred { .. }),
            "{status} {} must be deferred, got {outcome:?}",
            String::from_utf8_lossy(body)
        );
        let fx = effect(&outcome);
        assert!(
            !fx.record && fx.stop && !fx.relink,
            "deferred: not recorded, batch stops, token kept: {fx:?}"
        );
    }
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
        !fx.record && fx.stop && fx.relink,
        "401 stops the batch, records nothing and asks for a re-link: {fx:?}"
    );
    let fx = effect(&UploadOutcome::RateLimited {
        wait: Duration::from_secs(7),
    });
    assert!(
        !fx.record && fx.stop && !fx.relink && fx.hold == Some(Duration::from_secs(7)),
        "a 429 stops the batch and holds every upload for Retry-After: {fx:?}"
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

/// Review MINOR 3: a 429 is not retried in place (that blocked the loop for
/// up to 4 x 600 s); it ends the batch with the server's `Retry-After`.
#[tokio::test]
async fn rate_limit_stops_with_retry_after() {
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
    assert_eq!(
        send_to(&server, &clock).await,
        UploadOutcome::RateLimited {
            wait: Duration::from_secs(7)
        }
    );
    assert_eq!(hits(&server).await, 1, "a 429 is not retried in place");
    assert!(clock.waits().is_empty());
}

#[test]
fn a_hold_blocks_uploads_until_it_passes() {
    let rt = Runtime::new(std::env::temp_dir().join("rl-companion-hold-test.json"));
    assert_eq!(rt.held_secs(), None);
    rt.hold_for(Duration::from_secs(7));
    let left = rt.held_secs();
    assert!(
        left.is_some_and(|s| (1..=7).contains(&s)),
        "a 7 s hold is in force: {left:?}"
    );
    rt.hold_for(Duration::ZERO);
    assert_eq!(rt.held_secs(), None, "an expired hold lets uploads run");
}

/// Review MINOR 7: a redirect is not followed (it would turn the bearer POST
/// into a GET) and is not recorded.
#[tokio::test]
async fn redirects_are_not_followed() {
    let server = MockServer::start().await;
    let redirect = ResponseTemplate::new(302).insert_header("Location", "/");
    mount_sequence(&server, vec![redirect]).await;
    let clock = Clock::default();
    assert_eq!(
        send_to(&server, &clock).await,
        UploadOutcome::Deferred {
            reason: "unexpected answer 302 from the server".into(),
            status: Some(302),
            code: None,
        }
    );
    assert_eq!(hits(&server).await, 1, "the Location is never requested");
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
