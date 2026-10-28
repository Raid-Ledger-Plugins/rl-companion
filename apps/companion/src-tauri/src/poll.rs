//! The device-link poll loop (RFC 8628 §3.4–3.5 as profiled by CONTRACT.md §3.2, §4).
//!
//! The wait is injected (`sleep`) so tests run instantly and can assert the
//! exact schedule; production passes `tokio::time::sleep`. Cancellation is the
//! caller's job: run this in a task and abort it.

use crate::link::{LinkClient, LinkError, LinkedDevice, PollStep};
use crate::token::DeviceToken;
use std::future::Future;
use std::time::Duration;

/// Server cap on the poll interval after `slow_down` (CONTRACT.md §3.2).
pub const MAX_INTERVAL_SECS: u64 = 60;
/// RFC 8628 §3.5: without a server value, add 5 s on `slow_down`.
pub const SLOW_DOWN_STEP_SECS: u64 = 5;
/// Consecutive transient failures (network, 5xx) tolerated before giving up.
pub const MAX_TRANSIENT_FAILURES: u32 = 5;

/// What to poll, from the start response.
#[derive(Clone)]
pub struct PollPlan {
    pub device_code: String,
    pub interval_secs: u64,
    pub expires_in_secs: u64,
}

/// Terminal result of a link attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkOutcome {
    Linked { token: DeviceToken, device: LinkedDevice },
    Denied,
    Expired,
    InvalidGrant,
    DeviceLimit,
    Failed(LinkError),
}

/// Poll until the link resolves. Waits `interval` BEFORE every poll, so the
/// first poll never races the user's approval and never violates the minimum.
pub async fn run_poll<S, F>(client: &LinkClient, plan: &PollPlan, mut sleep: S) -> LinkOutcome
where
    S: FnMut(Duration) -> F,
    F: Future<Output = ()>,
{
    let mut interval = plan.interval_secs.clamp(1, MAX_INTERVAL_SECS);
    let mut next_wait = interval;
    let mut elapsed = 0u64;
    let mut transient = 0u32;
    loop {
        if elapsed >= plan.expires_in_secs {
            return LinkOutcome::Expired;
        }
        sleep(Duration::from_secs(next_wait)).await;
        elapsed += next_wait;
        next_wait = interval;
        let step = match client.poll(&plan.device_code).await {
            Ok(step) => {
                transient = 0;
                step
            }
            Err(e) if e.is_transient() && transient + 1 < MAX_TRANSIENT_FAILURES => {
                transient += 1;
                continue;
            }
            Err(e) => return LinkOutcome::Failed(e),
        };
        match step {
            PollStep::Pending => {}
            PollStep::SlowDown { interval: new } => {
                interval = new
                    .unwrap_or(interval + SLOW_DOWN_STEP_SECS)
                    .clamp(1, MAX_INTERVAL_SECS);
                next_wait = interval;
            }
            PollStep::RateLimited { retry_after } => {
                next_wait = retry_after.unwrap_or(interval).max(interval);
            }
            PollStep::Success { token, device } => return LinkOutcome::Linked { token, device },
            PollStep::Denied => return LinkOutcome::Denied,
            PollStep::Expired => return LinkOutcome::Expired,
            PollStep::InvalidGrant => return LinkOutcome::InvalidGrant,
            PollStep::DeviceLimit => return LinkOutcome::DeviceLimit,
        }
    }
}
