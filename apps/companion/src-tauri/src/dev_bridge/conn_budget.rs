//! Connection slots (ROK-2055 C2 review MAJOR-D1). Allowlisted callers and
//! everyone else draw from SEPARATE budgets, so a LAN host outside the
//! allowlist can never fill the slots the NAS agent needs. Outsiders are
//! still accepted (the contract owes them a 403 body), but only a few at a
//! time: past their budget a new connection is dropped at accept.
//!
//! A [`Slot`] is held by the connection task and released on drop, so the
//! count is exact the moment a connection ends (no stale `JoinSet` length).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// Open connections (including pending TLS handshakes) from allowlisted callers.
pub const MAX_CONNECTIONS: usize = 32;
/// Open connections from callers outside the allowlist, all hosts together.
pub const OUTSIDER_MAX_TOTAL: usize = 8;
/// Open connections from one caller outside the allowlist.
pub const OUTSIDER_MAX_PER_IP: usize = 2;

#[derive(Debug, Default)]
struct Counts {
    listed: usize,
    outsiders: HashMap<IpAddr, usize>,
    outsiders_total: usize,
}

/// Shared slot counter for one listener.
#[derive(Debug, Clone, Default)]
pub struct ConnBudget(Arc<Mutex<Counts>>);

/// One open connection's slot; released when dropped.
#[derive(Debug)]
pub struct Slot {
    budget: ConnBudget,
    outsider: Option<IpAddr>,
}

impl ConnBudget {
    fn counts(&self) -> std::sync::MutexGuard<'_, Counts> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Take a slot for a connection from `ip`, or `None` when its budget is full.
    pub fn try_take(&self, ip: IpAddr, allowlisted: bool) -> Option<Slot> {
        let mut c = self.counts();
        if allowlisted {
            if c.listed >= MAX_CONNECTIONS {
                return None;
            }
            c.listed += 1;
            return Some(Slot {
                budget: self.clone(),
                outsider: None,
            });
        }
        let mine = c.outsiders.get(&ip).copied().unwrap_or(0);
        if c.outsiders_total >= OUTSIDER_MAX_TOTAL || mine >= OUTSIDER_MAX_PER_IP {
            return None;
        }
        c.outsiders.insert(ip, mine + 1);
        c.outsiders_total += 1;
        Some(Slot {
            budget: self.clone(),
            outsider: Some(ip),
        })
    }

    /// `(allowlisted, outsiders)` slots in use.
    pub fn in_use(&self) -> (usize, usize) {
        let c = self.counts();
        (c.listed, c.outsiders_total)
    }
}

impl Slot {
    /// True when this connection came from outside the allowlist.
    pub fn is_outsider(&self) -> bool {
        self.outsider.is_some()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut c = self.budget.counts();
        let Some(ip) = self.outsider else {
            c.listed = c.listed.saturating_sub(1);
            return;
        };
        c.outsiders_total = c.outsiders_total.saturating_sub(1);
        if let Some(n) = c.outsiders.get_mut(&ip) {
            *n -= 1;
            if *n == 0 {
                c.outsiders.remove(&ip);
            }
        }
    }
}
