//! Request admission (spec §6/§7, AC-4): lockout → caller allowlist → bearer.
//! 10 failed auths inside a minute from one address lock it out for 5 minutes.

use super::token;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

pub const FAIL_WINDOW: Duration = Duration::from_secs(60);
pub const MAX_FAILS: usize = 10;
pub const LOCKOUT: Duration = Duration::from_secs(5 * 60);

/// Why a request was refused. Maps to the contract's `error.error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// Caller not in the allowlist (403).
    Forbidden,
    /// Missing or wrong bearer (401).
    Unauthorized,
    /// Locked out after repeated failures (429).
    RateLimited,
}

impl Denial {
    pub fn status(self) -> u16 {
        match self {
            Self::Forbidden => 403,
            Self::Unauthorized => 401,
            Self::RateLimited => 429,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
        }
    }
}

#[derive(Default)]
struct Strikes {
    fails: Vec<Instant>,
    locked_until: Option<Instant>,
}

/// Per-address failure counter. Time is passed in so tests can drive it.
#[derive(Default)]
pub struct Lockout(HashMap<Ipv4Addr, Strikes>);

impl Lockout {
    pub fn is_locked(&mut self, ip: Ipv4Addr, now: Instant) -> bool {
        let Some(s) = self.0.get_mut(&ip) else {
            return false;
        };
        match s.locked_until {
            Some(until) if now < until => true,
            Some(_) => {
                self.0.remove(&ip);
                false
            }
            None => false,
        }
    }

    /// Record a failure; returns true when it triggered a lockout.
    pub fn record_failure(&mut self, ip: Ipv4Addr, now: Instant) -> bool {
        let s = self.0.entry(ip).or_default();
        s.fails.retain(|t| now.duration_since(*t) < FAIL_WINDOW);
        s.fails.push(now);
        if s.fails.len() >= MAX_FAILS && s.locked_until.is_none() {
            s.locked_until = Some(now + LOCKOUT);
            return true;
        }
        false
    }
}

/// What admission needs to know, snapshotted from the config.
#[derive(Debug, Clone, Default)]
pub struct AuthPolicy {
    pub allowlist: Vec<Ipv4Addr>,
    pub token_sha256: Option<String>,
}

/// The bearer value of an `Authorization` header (scheme case-insensitive).
pub fn bearer(header: Option<&str>) -> Option<&str> {
    let (scheme, value) = header?.trim().split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| value.trim())
}

/// Admit or refuse one request from `ip`.
pub fn admit(
    policy: &AuthPolicy,
    lockout: &mut Lockout,
    ip: Ipv4Addr,
    authorization: Option<&str>,
    now: Instant,
) -> Result<(), Denial> {
    if lockout.is_locked(ip, now) {
        return Err(Denial::RateLimited);
    }
    if !policy.allowlist.contains(&ip) {
        return Err(Denial::Forbidden);
    }
    let ok = match (bearer(authorization), policy.token_sha256.as_deref()) {
        (Some(presented), Some(stored)) => token::matches(presented, stored),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    if lockout.record_failure(ip, now) {
        eprintln!("RL Companion dev bridge: {ip} locked out for 5 minutes after {MAX_FAILS} failed sign-ins");
    }
    Err(Denial::Unauthorized)
}
