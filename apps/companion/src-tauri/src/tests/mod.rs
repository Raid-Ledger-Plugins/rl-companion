//! Unit tests. HTTP behaviour runs against a `wiremock` server.

mod collector_tests;
mod config_tests;
mod link_tests;
mod poll_tests;
mod upload_tests;
mod watch_tests;

use crate::link::LinkClient;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const DEVICE_CODE: &str = "dc_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const TOKEN: &str = "rlcd_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AB-_xyz";
pub const POLL: &str = "/api/companion/v1/devices/link/poll";
pub const START: &str = "/api/companion/v1/devices/link";
pub const SELF_CHECK: &str = "/api/companion/v1/device";

pub fn client(server: &MockServer) -> LinkClient {
    LinkClient::new(&server.uri()).expect("mock server URL is valid")
}

pub fn poll_error(error: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({ "error": error }))
}

/// The `poll-success` shape from the contract fixtures.
pub fn poll_success() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "token": TOKEN,
        "tokenType": "Bearer",
        "device": {
            "id": 7, "name": "Gaming PC", "os": "windows", "osVersion": null,
            "appVersion": "0.1.0", "tokenHint": "_xyz",
            "createdAt": "2026-10-28T18:04:31.512Z", "lastUsedAt": null,
            "lastUsedNetwork": null, "revokedAt": null, "state": "active"
        }
    }))
}

/// Mount `responses` for the poll endpoint, served once each, in order.
pub async fn mount_poll_sequence(server: &MockServer, responses: Vec<ResponseTemplate>) {
    for r in responses {
        Mock::given(method("POST"))
            .and(path(POLL))
            .respond_with(r)
            .up_to_n_times(1)
            .mount(server)
            .await;
    }
}

pub async fn poll_bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == POLL)
        .map(|r| serde_json::from_slice(&r.body).expect("poll body is JSON"))
        .collect()
}

/// A fake clock: records every requested wait (in seconds) and returns at once.
#[derive(Clone, Default)]
pub struct Clock(Arc<Mutex<Vec<u64>>>);

impl Clock {
    pub fn sleeper(&self) -> impl FnMut(Duration) -> std::future::Ready<()> {
        let log = self.0.clone();
        move |d| {
            log.lock().expect("clock lock").push(d.as_secs());
            std::future::ready(())
        }
    }

    pub fn waits(&self) -> Vec<u64> {
        self.0.lock().expect("clock lock").clone()
    }
}

/// A fresh, empty temp folder unique to this test process + name.
pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rlc-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
