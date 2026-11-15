//! Request bodies (ROK-2191): only `POST /dev/v1/probes` has one, read after
//! admission and capped at [`BODY_MAX_BYTES`] — a larger body is refused
//! without buffering past the cap. Bodies are never logged.

use super::probe_routes::PROBES_PATH;
use http_body_util::{BodyExt, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::{header, Method, Request};
use std::time::Duration;

/// The largest raw POST body read (a valid probe request is ≤ 4 KiB).
pub const BODY_MAX_BYTES: usize = 8 * 1024;
/// How long a caller gets to send the body once its headers arrived.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// What the router gets as the request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReqBody {
    /// No body was read (every route but `POST /dev/v1/probes`).
    None,
    /// The raw bytes (≤ [`BODY_MAX_BYTES`]); empty when it could not be read.
    Bytes(Vec<u8>),
    /// Over [`BODY_MAX_BYTES`].
    TooBig,
}

/// Whether this request's body is read at all: the probe POST, and only
/// while probes are enabled (otherwise it is a 409 without reading).
pub fn wants_body(method: &Method, path: &str, probes_enabled: bool) -> bool {
    probes_enabled && method == Method::POST && path == PROBES_PATH
}

/// Read `req`'s body, at most [`BODY_MAX_BYTES`].
pub async fn read(req: Request<Incoming>) -> ReqBody {
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > BODY_MAX_BYTES as u64) {
        return ReqBody::TooBig;
    }
    let limited = Limited::new(req.into_body(), BODY_MAX_BYTES);
    match tokio::time::timeout(BODY_READ_TIMEOUT, limited.collect()).await {
        Ok(Ok(c)) => ReqBody::Bytes(c.to_bytes().to_vec()),
        Ok(Err(e)) if e.downcast_ref::<LengthLimitError>().is_some() => ReqBody::TooBig,
        _ => ReqBody::Bytes(Vec::new()),
    }
}
