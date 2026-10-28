//! Uploader: `POST {server}/api/companion/v1/messages/:contract` with the
//! device bearer token. Retries 5xx/408/network with exponential backoff,
//! honours `Retry-After` on 429, never retries another 4xx (envelope/v1 §3).

use crate::config::API_PREFIX;
use crate::token::DeviceToken;
use serde::Deserialize;
use std::future::Future;
use std::time::Duration;

/// Attempts per message per scan; a message that still fails is retried on
/// the next scan (it is not recorded as sent).
pub const MAX_ATTEMPTS: u32 = 5;
pub const MAX_BACKOFF_SECS: u64 = 60;
pub const DEFAULT_RETRY_AFTER_SECS: u64 = 60;
pub const MAX_RETRY_AFTER_SECS: u64 = 600;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How one message ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    /// 2xx — applied, unchanged or stale all ack 202 (CONTRACT §2.1).
    Accepted,
    /// Terminal 4xx: never resend this content. `issue` is the first
    /// `issues[].code` (e.g. `BAD_HEADER`, `RULESET_REQUIRED`).
    Rejected {
        status: u16,
        code: String,
        issue: Option<String>,
    },
    /// 401: the token is gone or revoked — re-link the device.
    Unauthorized,
    /// Out of attempts (5xx / network / 429). Try again on the next scan.
    GaveUp { reason: String },
}

/// One HTTP attempt's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    Done(UploadOutcome),
    /// Retryable; `wait` overrides the backoff (429 `Retry-After`).
    Retry {
        wait: Option<Duration>,
        reason: String,
    },
}

/// Wait before retry number `failures` (0-based): 2, 4, 8, 16, 32, then 60 s.
pub fn backoff(failures: u32) -> Duration {
    Duration::from_secs(
        2u64.saturating_pow(failures.saturating_add(1))
            .min(MAX_BACKOFF_SECS),
    )
}

/// `Retry-After` in seconds, clamped to 1..=600; missing/HTTP-date → 60 s.
pub fn parse_retry_after(value: Option<&str>) -> Duration {
    let secs = value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS);
    Duration::from_secs(secs.clamp(1, MAX_RETRY_AFTER_SECS))
}

#[derive(Debug, Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    issues: Vec<Issue>,
}

#[derive(Debug, Deserialize)]
struct Issue {
    #[serde(default)]
    code: String,
}

/// Map an HTTP answer to retry-or-done.
pub fn classify(status: u16, retry_after: Option<&str>, body: &[u8]) -> Attempt {
    match status {
        200..=299 => Attempt::Done(UploadOutcome::Accepted),
        401 => Attempt::Done(UploadOutcome::Unauthorized),
        429 => Attempt::Retry {
            wait: Some(parse_retry_after(retry_after)),
            reason: "rate limited by the server".into(),
        },
        408 | 500..=599 => Attempt::Retry {
            wait: None,
            reason: format!("server error {status}"),
        },
        _ => {
            let parsed: ErrorBody = serde_json::from_slice(body).unwrap_or_default();
            let code = if parsed.code.is_empty() {
                format!("http_{status}")
            } else {
                parsed.code
            };
            let issue = parsed
                .issues
                .into_iter()
                .map(|i| i.code)
                .find(|c| !c.is_empty());
            Attempt::Done(UploadOutcome::Rejected {
                status,
                code,
                issue,
            })
        }
    }
}

/// Run `attempt` up to [`MAX_ATTEMPTS`] times, sleeping between retries.
/// `sleep` is injected so tests run instantly.
pub async fn send_with_retry<A, AF, S, SF>(mut attempt: A, mut sleep: S) -> UploadOutcome
where
    A: FnMut() -> AF,
    AF: Future<Output = Attempt>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let mut last = String::from("no attempt made");
    for n in 0..MAX_ATTEMPTS {
        match attempt().await {
            Attempt::Done(outcome) => return outcome,
            Attempt::Retry { wait, reason } => {
                last = reason;
                if n + 1 < MAX_ATTEMPTS {
                    sleep(wait.unwrap_or_else(|| backoff(n))).await;
                }
            }
        }
    }
    UploadOutcome::GaveUp { reason: last }
}

pub struct Uploader {
    base: url::Url,
    http: reqwest::Client,
}

impl Uploader {
    /// `server` is the normalised origin from the config.
    pub fn new(server: &str) -> Result<Self, String> {
        let mut base = url::Url::parse(server).map_err(|e| format!("Bad server URL: {e}"))?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let base = base
            .join(API_PREFIX)
            .and_then(|u| u.join("companion/v1/messages/"))
            .map_err(|e| format!("Bad server URL: {e}"))?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| format!("HTTP client: {e}"))?;
        Ok(Self { base, http })
    }

    /// The ingest URL for a contract.
    pub fn url(&self, contract: &str) -> Result<url::Url, String> {
        self.base.join(contract).map_err(|e| e.to_string())
    }

    /// One POST, classified.
    pub async fn attempt(
        &self,
        token: &DeviceToken,
        contract: &str,
        body: &serde_json::Value,
    ) -> Attempt {
        let url = match self.url(contract) {
            Ok(u) => u,
            Err(reason) => return Attempt::Done(UploadOutcome::GaveUp { reason }),
        };
        let sent = self
            .http
            .post(url)
            .bearer_auth(token.expose())
            .json(body)
            .send()
            .await;
        match sent {
            Err(e) => Attempt::Retry {
                wait: None,
                reason: network_reason(&e).into(),
            },
            Ok(res) => {
                let status = res.status().as_u16();
                let retry_after = res
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let bytes = res.bytes().await.unwrap_or_default();
                classify(status, retry_after.as_deref(), &bytes)
            }
        }
    }

    /// POST with retries.
    pub async fn send<S, SF>(
        &self,
        token: &DeviceToken,
        contract: &str,
        body: &serde_json::Value,
        sleep: S,
    ) -> UploadOutcome
    where
        S: FnMut(Duration) -> SF,
        SF: Future<Output = ()>,
    {
        send_with_retry(|| self.attempt(token, contract, body), sleep).await
    }
}

fn network_reason(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "network timeout"
    } else if e.is_connect() {
        "could not connect to the server"
    } else {
        "network error"
    }
}
