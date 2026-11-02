//! Uploader: `POST {server}/api/companion/v1/messages/:contract` with the
//! device bearer token. Retries 5xx/408/network with exponential backoff.
//! A 429 stops the batch until `Retry-After` has passed. Only the server's own
//! content rejections ([`TERMINAL_CODES`]) are final; every other 4xx (a route
//! the server does not serve yet, a proxy page, a redirect) is tried again on
//! a later scan (envelope/v1 §3-4).

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

/// The companion error codes that reject the message itself (envelope/v1 §4):
/// resending the same content can never succeed, so it is recorded as sent.
pub const TERMINAL_CODES: [&str; 4] = [
    "invalid_envelope",
    "invalid_payload",
    "payload_too_large",
    "captured_at_in_future",
];

/// How one message ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    /// 2xx — applied, unchanged or stale all ack 202 (CONTRACT §2.1).
    Accepted,
    /// A [`TERMINAL_CODES`] 4xx: never resend this content. `issue` is the first
    /// `issues[].code` (e.g. `BAD_HEADER`, `RULESET_REQUIRED`).
    Rejected {
        status: u16,
        code: String,
        issue: Option<String>,
    },
    /// 401: the token is gone or revoked — re-link the device.
    Unauthorized,
    /// 429: send nothing (this batch included) until `wait` has passed.
    RateLimited { wait: Duration },
    /// A 4xx that is not about this message: the server does not serve the
    /// contract yet (404), the version is retired (409), or a proxy answered.
    /// Nothing is recorded; the batch stops and the next scan tries again.
    Deferred { reason: String },
    /// Out of attempts (5xx / 408 / network). Try again on the next scan.
    GaveUp { reason: String },
}

/// One HTTP attempt's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    Done(UploadOutcome),
    /// Retryable now, after the backoff.
    Retry {
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
        429 => Attempt::Done(UploadOutcome::RateLimited {
            wait: parse_retry_after(retry_after),
        }),
        408 | 500..=599 => Attempt::Retry {
            reason: format!("server error {status}"),
        },
        _ => Attempt::Done(other_status(status, body)),
    }
}

/// A non-2xx that is neither 401, 408, 429 nor 5xx (including a 3xx, as
/// redirects are not followed): terminal only for a [`TERMINAL_CODES`] body.
fn other_status(status: u16, body: &[u8]) -> UploadOutcome {
    let Ok(parsed) = serde_json::from_slice::<ErrorBody>(body) else {
        return UploadOutcome::Deferred {
            reason: format!("unexpected answer {status} from the server"),
        };
    };
    if !TERMINAL_CODES.contains(&parsed.code.as_str()) {
        return UploadOutcome::Deferred {
            reason: deferred_reason(status, &parsed.code),
        };
    }
    let issue = parsed
        .issues
        .into_iter()
        .map(|i| i.code)
        .find(|c| !c.is_empty());
    UploadOutcome::Rejected {
        status,
        code: parsed.code,
        issue,
    }
}

fn deferred_reason(status: u16, code: &str) -> String {
    match code {
        "unknown_contract" | "unknown_contract_version" => {
            "the server does not take this upload yet".to_owned()
        }
        "retired_contract_version" => "this app is out of date; update the Companion".to_owned(),
        "" => format!("unexpected answer {status} from the server"),
        other => format!("server answered {status} ({other})"),
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
            Attempt::Retry { reason } => {
                last = reason;
                if n + 1 < MAX_ATTEMPTS {
                    sleep(backoff(n)).await;
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
        // No redirects: a 3xx would turn the POST into a GET. It is reported
        // as a deferred answer instead (review MINOR 7).
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
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

impl Uploader {
    /// The blob URL (`companion/v1/blobs/:sha256`); the id must be 64 hex.
    pub fn blob_url(&self, sha256: &str) -> Result<url::Url, String> {
        let hex = sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err(format!("bad blob id {sha256:?}"));
        }
        self.base
            .join(&format!("../blobs/{sha256}"))
            .map_err(|e| e.to_string())
    }

    /// One blob PUT, classified like a message POST.
    pub async fn attempt_blob(
        &self,
        token: &DeviceToken,
        blob: &crate::collector::Blob,
    ) -> Attempt {
        let url = match self.blob_url(&blob.sha256) {
            Ok(u) => u,
            Err(reason) => return Attempt::Done(UploadOutcome::GaveUp { reason }),
        };
        let sent = self
            .http
            .put(url)
            .bearer_auth(token.expose())
            .header(reqwest::header::CONTENT_TYPE, blob.content_type.as_str())
            .body(blob.bytes.clone())
            .send()
            .await;
        match sent {
            Err(e) => Attempt::Retry {
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

    /// PUT a blob with retries.
    pub async fn send_blob<S, SF>(
        &self,
        token: &DeviceToken,
        blob: &crate::collector::Blob,
        sleep: S,
    ) -> UploadOutcome
    where
        S: FnMut(Duration) -> SF,
        SF: Future<Output = ()>,
    {
        send_with_retry(|| self.attempt_blob(token, blob), sleep).await
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
