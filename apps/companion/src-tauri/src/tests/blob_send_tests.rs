//! ROK-2061 review MAJOR 1: a blob the server refuses must not block its
//! save's message; a transient blob failure still defers the message.

use super::*;
use crate::blob_send::{drop_blob_ref, send_with_blobs};
use crate::collector::{Blob, PendingUpload};
use crate::runtime::effect;
use crate::tests::upload_tests::token;
use crate::upload::{classify_blob, Attempt, UploadOutcome, Uploader};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MESSAGE: &str = "/api/companion/v1/messages/bg3-save";

fn blob_path() -> String {
    format!("/api/companion/v1/blobs/{SHA}")
}

fn pending() -> PendingUpload {
    PendingUpload {
        contract: "bg3-save",
        scope: "scope".into(),
        item: "item".into(),
        sha256: "msg".into(),
        label: "Save 1".into(),
        body: json!({
            "contract": "bg3-save",
            "payload": { "saveName": "Save 1", "thumbnail": { "sha256": SHA, "byteLength": 3 } }
        }),
        blobs: vec![Blob {
            sha256: SHA.into(),
            content_type: "image/webp".into(),
            bytes: vec![1, 2, 3],
        }],
    }
}

async fn run(server: &MockServer, clock: &Clock) -> UploadOutcome {
    let up = Uploader::new(&server.uri()).expect("mock URL");
    send_with_blobs(&up, &token(), &pending(), clock.sleeper()).await
}

async fn mount(server: &MockServer, blob: ResponseTemplate) {
    Mock::given(method("PUT"))
        .and(path(blob_path()))
        .respond_with(blob)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(MESSAGE))
        .respond_with(ResponseTemplate::new(202))
        .mount(server)
        .await;
}

async fn posted(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == MESSAGE)
        .map(|r| serde_json::from_slice(&r.body).expect("message body is JSON"))
        .collect()
}

#[tokio::test]
async fn a_refused_blob_sends_the_message_without_its_thumbnail() {
    let codes = [
        (400, "invalid_blob"),
        (400, "hash_mismatch"),
        (415, "unsupported_media"),
        (409, "blob_ref_mismatch"),
        (413, "blob_quota_exceeded"),
    ];
    for (status, code) in codes {
        let server = MockServer::start().await;
        let refusal = ResponseTemplate::new(status).set_body_json(json!({ "code": code }));
        mount(&server, refusal).await;
        let outcome = run(&server, &Clock::default()).await;
        assert_eq!(outcome, UploadOutcome::Accepted, "{code}: the save uploads");
        let bodies = posted(&server).await;
        assert_eq!(bodies.len(), 1, "{code}: the message is POSTed once");
        let thumb = bodies.first().map(|b| b["payload"]["thumbnail"].clone());
        assert_eq!(
            thumb,
            Some(Value::Null),
            "{code}: no ref to the refused blob"
        );
    }
}

#[tokio::test]
async fn a_stored_blob_keeps_the_thumbnail_ref() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(201)).await;
    assert_eq!(
        run(&server, &Clock::default()).await,
        UploadOutcome::Accepted
    );
    let bodies = posted(&server).await;
    let sha = bodies
        .first()
        .map(|b| b["payload"]["thumbnail"]["sha256"].clone());
    assert_eq!(sha, Some(json!(SHA)));
}

#[tokio::test]
async fn a_transient_blob_failure_still_defers_the_message() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(503)).await;
    let outcome = run(&server, &Clock::default()).await;
    assert!(
        matches!(outcome, UploadOutcome::GaveUp { .. }),
        "{outcome:?}"
    );
    assert!(
        posted(&server).await.is_empty(),
        "no message without its blob"
    );
    assert!(!effect(&outcome).record, "tried again on a later scan");
}

#[tokio::test]
async fn auth_and_rate_limit_on_a_blob_stop_the_message() {
    for (status, expected) in [(401, "Unauthorized"), (429, "RateLimited")] {
        let server = MockServer::start().await;
        mount(&server, ResponseTemplate::new(status)).await;
        let outcome = run(&server, &Clock::default()).await;
        assert!(format!("{outcome:?}").starts_with(expected), "{outcome:?}");
        assert!(posted(&server).await.is_empty(), "{status}: nothing POSTed");
    }
}

#[test]
fn classify_blob_refuses_only_plain_4xx() {
    let refused = classify_blob(404, None, b"<html>");
    assert!(matches!(
        refused,
        Attempt::Done(UploadOutcome::Rejected { status: 404, ref code, .. }) if code == "blob_refused"
    ));
    assert_eq!(
        classify_blob(401, None, b""),
        Attempt::Done(UploadOutcome::Unauthorized)
    );
    assert!(matches!(
        classify_blob(408, None, b""),
        Attempt::Retry { .. }
    ));
    assert!(matches!(
        classify_blob(503, None, b""),
        Attempt::Retry { .. }
    ));
    assert_eq!(
        classify_blob(201, None, b""),
        Attempt::Done(UploadOutcome::Accepted)
    );
}

#[test]
fn drop_blob_ref_only_touches_the_matching_thumbnail() {
    let mut body = pending().body;
    assert!(!drop_blob_ref(&mut body, "bbbb"), "another blob's sha");
    assert_eq!(body["payload"]["thumbnail"]["sha256"], json!(SHA));
    assert!(drop_blob_ref(&mut body, SHA));
    assert_eq!(body["payload"]["thumbnail"], Value::Null);
    assert!(
        !drop_blob_ref(&mut json!({ "payload": {} }), SHA),
        "no thumbnail"
    );
}
