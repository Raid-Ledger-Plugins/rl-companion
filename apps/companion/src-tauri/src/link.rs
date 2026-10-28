//! HTTP client for Raid Ledger's companion device link
//! (`packages/contract/companion/device-link/v1/CONTRACT.md` in Raid Ledger).
//!
//! `deviceCode`, `userCode` and the token are secrets — nothing here logs them,
//! and the types holding them have redacted `Debug`.

use crate::config::API_PREFIX;
use crate::token::DeviceToken;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

pub const START_PATH: &str = "companion/v1/devices/link";
pub const POLL_PATH: &str = "companion/v1/devices/link/poll";
pub const SELF_CHECK_PATH: &str = "companion/v1/device";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// `device` in the start request (`CompanionDeviceInfoSchema`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    pub os: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    pub app_version: String,
}

#[derive(Serialize)]
struct StartRequest<'a> {
    device: &'a DeviceInfo,
}

/// `CompanionLinkStartResponseSchema`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

impl fmt::Debug for StartResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartResponse")
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// The subset of `CompanionDeviceSchema` the app uses (unknown keys ignored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedDevice {
    pub id: i64,
    pub name: String,
}

/// One poll's result, already classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollStep {
    Pending,
    /// `interval` is the server's new interval, if it sent one.
    SlowDown { interval: Option<u64> },
    Success { token: DeviceToken, device: LinkedDevice },
    Denied,
    Expired,
    InvalidGrant,
    /// 409 `device_limit`: the approver already has 10 live devices.
    DeviceLimit,
    RateLimited { retry_after: Option<u64> },
}

/// `GET /companion/v1/device` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfCheck {
    Valid { device_id: i64, name: String },
    /// 401 — revoked, unknown, or owner blocked. Re-link.
    Unauthorized,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("could not reach the server: {0}")]
    Network(String),
    #[error("the server answered HTTP {status}{}", fmt_code(.code))]
    Http { status: u16, code: Option<String> },
    #[error("unexpected response from the server: {0}")]
    Protocol(String),
    #[error("invalid server URL: {0}")]
    BadUrl(String),
}

fn fmt_code(code: &Option<String>) -> String {
    code.as_deref().map(|c| format!(" ({c})")).unwrap_or_default()
}

impl LinkError {
    /// Worth retrying on the next poll (network blip, 5xx, 408).
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::Http { status, .. } => *status >= 500 || *status == 408,
            _ => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollSuccessBody {
    token: String,
    token_type: String,
    device: LinkedDevice,
}

#[derive(Deserialize)]
struct PollErrorBody {
    error: String,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct CodedErrorBody {
    code: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSelfBody {
    device_id: i64,
    name: String,
}

/// Classify a poll response. Pure, so the mapping is unit-testable.
pub fn classify_poll(status: u16, body: &str, retry_after: Option<u64>) -> Result<PollStep, LinkError> {
    match status {
        200 => {
            let ok: PollSuccessBody = serde_json::from_str(body)
                .map_err(|_| LinkError::Protocol("malformed poll success body".into()))?;
            if ok.token_type != "Bearer" {
                return Err(LinkError::Protocol("unexpected tokenType".into()));
            }
            let token = DeviceToken::parse(&ok.token)
                .ok_or_else(|| LinkError::Protocol("malformed device token".into()))?;
            Ok(PollStep::Success { token, device: ok.device })
        }
        400 => {
            let err: PollErrorBody = serde_json::from_str(body)
                .map_err(|_| LinkError::Http { status, code: None })?;
            match err.error.as_str() {
                "authorization_pending" => Ok(PollStep::Pending),
                "slow_down" => Ok(PollStep::SlowDown { interval: err.interval }),
                "access_denied" => Ok(PollStep::Denied),
                "expired_token" => Ok(PollStep::Expired),
                "invalid_grant" => Ok(PollStep::InvalidGrant),
                other => Err(LinkError::Http { status, code: Some(other.to_owned()) }),
            }
        }
        429 => Ok(PollStep::RateLimited { retry_after }),
        _ => {
            let code = error_code(body);
            if status == 409 && code.as_deref() == Some("device_limit") {
                return Ok(PollStep::DeviceLimit);
            }
            Err(LinkError::Http { status, code })
        }
    }
}

fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<CodedErrorBody>(body).ok().and_then(|b| b.code)
}

/// Client bound to one Raid Ledger origin.
#[derive(Clone)]
pub struct LinkClient {
    http: reqwest::Client,
    api_base: url::Url,
}

impl LinkClient {
    /// `server` is a site origin (`https://raid.gamernight.net`) or, in tests,
    /// a mock server's base URL. Requests go to `<server>/api/<path>`.
    pub fn new(server: &str) -> Result<Self, LinkError> {
        let mut base = url::Url::parse(server).map_err(|e| LinkError::BadUrl(e.to_string()))?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let api_base = base.join(API_PREFIX).map_err(|e| LinkError::BadUrl(e.to_string()))?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("rl-companion/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| LinkError::Network(e.to_string()))?;
        Ok(Self { http, api_base })
    }

    fn url(&self, path: &str) -> Result<url::Url, LinkError> {
        self.api_base.join(path).map_err(|e| LinkError::BadUrl(e.to_string()))
    }

    /// `POST /companion/v1/devices/link`.
    pub async fn start(&self, device: &DeviceInfo) -> Result<StartResponse, LinkError> {
        let res = self
            .http
            .post(self.url(START_PATH)?)
            .json(&StartRequest { device })
            .send()
            .await
            .map_err(net)?;
        let status = res.status().as_u16();
        let body = res.text().await.map_err(net)?;
        if status != 200 {
            return Err(LinkError::Http { status, code: error_code(&body) });
        }
        serde_json::from_str(&body).map_err(|_| LinkError::Protocol("malformed start response".into()))
    }

    /// `POST /companion/v1/devices/link/poll`, classified.
    pub async fn poll(&self, device_code: &str) -> Result<PollStep, LinkError> {
        let res = self
            .http
            .post(self.url(POLL_PATH)?)
            .json(&serde_json::json!({ "deviceCode": device_code }))
            .send()
            .await
            .map_err(net)?;
        let status = res.status().as_u16();
        let retry_after = retry_after_secs(&res);
        let body = res.text().await.map_err(net)?;
        classify_poll(status, &body, retry_after)
    }

    /// `GET /companion/v1/device` with the bearer token.
    pub async fn self_check(&self, token: &DeviceToken) -> Result<SelfCheck, LinkError> {
        let res = self
            .http
            .get(self.url(SELF_CHECK_PATH)?)
            .bearer_auth(token.expose())
            .send()
            .await
            .map_err(net)?;
        let status = res.status().as_u16();
        let body = res.text().await.map_err(net)?;
        match status {
            200 => {
                let me: DeviceSelfBody = serde_json::from_str(&body)
                    .map_err(|_| LinkError::Protocol("malformed device self-check".into()))?;
                Ok(SelfCheck::Valid { device_id: me.device_id, name: me.name })
            }
            401 => Ok(SelfCheck::Unauthorized),
            _ => Err(LinkError::Http { status, code: error_code(&body) }),
        }
    }
}

fn net(e: reqwest::Error) -> LinkError {
    // Strip the URL: it carries no secret, but keeps messages short.
    LinkError::Network(e.without_url().to_string())
}

fn retry_after_secs(res: &reqwest::Response) -> Option<u64> {
    res.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}
