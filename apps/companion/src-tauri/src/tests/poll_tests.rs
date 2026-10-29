use super::*;
use crate::link::LinkError;
use crate::poll::{run_poll, LinkOutcome, PollPlan};

fn plan(interval: u64, expires_in: u64) -> PollPlan {
    PollPlan {
        device_code: DEVICE_CODE.to_owned(),
        interval_secs: interval,
        expires_in_secs: expires_in,
    }
}

async fn run(
    responses: Vec<ResponseTemplate>,
    plan: PollPlan,
) -> (LinkOutcome, Vec<u64>, MockServer) {
    let server = MockServer::start().await;
    mount_poll_sequence(&server, responses).await;
    let clock = Clock::default();
    let outcome = run_poll(&client(&server), &plan, clock.sleeper()).await;
    (outcome, clock.waits(), server)
}

#[tokio::test]
async fn pending_then_success_links_and_waits_interval_before_each_poll() {
    let responses = vec![
        poll_error("authorization_pending"),
        poll_error("authorization_pending"),
        poll_success(),
    ];
    let (outcome, waits, server) = run(responses, plan(5, 600)).await;
    match outcome {
        LinkOutcome::Linked { token, device } => {
            assert_eq!(token.expose(), TOKEN);
            assert_eq!(device.id, 7);
            assert_eq!(device.name, "Gaming PC");
        }
        other => panic!("expected Linked, got {other:?}"),
    }
    assert_eq!(waits, vec![5, 5, 5], "one interval wait before every poll");
    let bodies = poll_bodies(&server).await;
    assert_eq!(bodies.len(), 3);
    assert!(
        bodies
            .iter()
            .all(|b| *b == json!({ "deviceCode": DEVICE_CODE })),
        "poll body is exactly {{deviceCode}}: {bodies:?}"
    );
}

#[tokio::test]
async fn slow_down_adopts_the_server_interval_for_all_later_polls() {
    let slow =
        ResponseTemplate::new(400).set_body_json(json!({ "error": "slow_down", "interval": 10 }));
    let responses = vec![slow, poll_error("authorization_pending"), poll_success()];
    let (outcome, waits, _s) = run(responses, plan(5, 600)).await;
    assert!(
        matches!(outcome, LinkOutcome::Linked { .. }),
        "got {outcome:?}"
    );
    assert_eq!(waits, vec![5, 10, 10]);
}

#[tokio::test]
async fn slow_down_without_interval_adds_five_seconds() {
    let (outcome, waits, _s) =
        run(vec![poll_error("slow_down"), poll_success()], plan(5, 600)).await;
    assert!(
        matches!(outcome, LinkOutcome::Linked { .. }),
        "got {outcome:?}"
    );
    assert_eq!(waits, vec![5, 10]);
}

#[tokio::test]
async fn expired_token_is_terminal() {
    let (outcome, _w, server) = run(
        vec![poll_error("expired_token"), poll_success()],
        plan(5, 600),
    )
    .await;
    assert_eq!(outcome, LinkOutcome::Expired);
    assert_eq!(
        poll_bodies(&server).await.len(),
        1,
        "no poll after expired_token"
    );
}

#[tokio::test]
async fn access_denied_is_terminal() {
    let (outcome, _w, server) = run(
        vec![poll_error("access_denied"), poll_success()],
        plan(5, 600),
    )
    .await;
    assert_eq!(outcome, LinkOutcome::Denied);
    assert_eq!(
        poll_bodies(&server).await.len(),
        1,
        "no poll after access_denied"
    );
}

#[tokio::test]
async fn invalid_grant_is_terminal() {
    let (outcome, _w, _s) = run(vec![poll_error("invalid_grant")], plan(5, 600)).await;
    assert_eq!(outcome, LinkOutcome::InvalidGrant);
}

#[tokio::test]
async fn device_limit_409_is_terminal() {
    let limit =
        ResponseTemplate::new(409).set_body_json(json!({ "code": "device_limit", "message": "x" }));
    let (outcome, _w, _s) = run(vec![limit], plan(5, 600)).await;
    assert_eq!(outcome, LinkOutcome::DeviceLimit);
}

#[tokio::test]
async fn stops_locally_once_expires_in_has_elapsed() {
    let pending = (0..10)
        .map(|_| poll_error("authorization_pending"))
        .collect();
    let (outcome, waits, server) = run(pending, plan(5, 12)).await;
    assert_eq!(outcome, LinkOutcome::Expired);
    assert_eq!(waits, vec![5, 5, 5]);
    assert_eq!(poll_bodies(&server).await.len(), 3);
}

#[tokio::test]
async fn transient_5xx_is_retried_then_links() {
    let responses = vec![ResponseTemplate::new(503), poll_success()];
    let (outcome, waits, _s) = run(responses, plan(5, 600)).await;
    assert!(
        matches!(outcome, LinkOutcome::Linked { .. }),
        "got {outcome:?}"
    );
    assert_eq!(waits, vec![5, 5]);
}

#[tokio::test]
async fn persistent_5xx_gives_up_after_the_transient_budget() {
    let responses = (0..10).map(|_| ResponseTemplate::new(502)).collect();
    let (outcome, waits, _s) = run(responses, plan(5, 600)).await;
    assert_eq!(
        outcome,
        LinkOutcome::Failed(LinkError::Http {
            status: 502,
            code: None
        })
    );
    assert_eq!(waits.len(), crate::poll::MAX_TRANSIENT_FAILURES as usize);
}

#[tokio::test]
async fn rate_limited_honours_retry_after_once() {
    let limited = ResponseTemplate::new(429)
        .insert_header("Retry-After", "30")
        .set_body_json(json!({ "code": "rate_limited", "message": "x" }));
    let responses = vec![limited, poll_error("authorization_pending"), poll_success()];
    let (outcome, waits, _s) = run(responses, plan(5, 600)).await;
    assert!(
        matches!(outcome, LinkOutcome::Linked { .. }),
        "got {outcome:?}"
    );
    assert_eq!(waits, vec![5, 30, 5]);
}
