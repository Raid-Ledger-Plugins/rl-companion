//! A message's blobs go up before it. A blob the server refuses for good is
//! dropped from the message (`payload.thumbnail = null`, bg3-save/v1 §7), so
//! the save still uploads and the server never sees a ref to a blob it
//! refused. Any other blob failure (401, 429, 5xx, network) is the message's
//! outcome, and nothing is POSTed.

use crate::collector::PendingUpload;
use crate::token::DeviceToken;
use crate::upload::{UploadOutcome, Uploader};
use serde_json::Value;
use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;

/// Null `payload.thumbnail` when it names `sha256`. Returns whether it did.
pub fn drop_blob_ref(body: &mut Value, sha256: &str) -> bool {
    let Some(thumb) = body.get_mut("payload").and_then(|p| p.get_mut("thumbnail")) else {
        return false;
    };
    if thumb.get("sha256").and_then(Value::as_str) != Some(sha256) {
        return false;
    }
    *thumb = Value::Null;
    true
}

/// PUT the message's blobs, then POST it (without any refused blob's ref).
/// `sleep` is injected so tests run instantly.
pub async fn send_with_blobs<S, SF>(
    uploader: &Uploader,
    token: &DeviceToken,
    p: &PendingUpload,
    mut sleep: S,
) -> UploadOutcome
where
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let mut body = Cow::Borrowed(&p.body);
    for blob in &p.blobs {
        match uploader.send_blob(token, blob, &mut sleep).await {
            UploadOutcome::Accepted => {}
            UploadOutcome::Rejected { code, .. } => {
                // A required ref (a screenshot's `image`) cannot be dropped:
                // a full Gallery is then that message's own answer.
                let dropped = drop_blob_ref(body.to_mut(), &blob.sha256);
                if !dropped && code == crate::upload::GALLERY_FULL_CODE {
                    return UploadOutcome::Rejected {
                        status: 413,
                        code: code.clone(),
                        issue: None,
                    };
                }
            }
            other => return other,
        }
    }
    uploader.send(token, p.contract, &body, sleep).await
}
