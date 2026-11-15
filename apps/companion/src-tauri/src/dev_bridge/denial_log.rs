//! Per-host rate limit for the audit lines about refused requests (moved out
//! of server.rs; spec §7: never the path, never request text).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

/// Refused requests are logged at most once per host per window, without
/// the path, so a LAN host can neither flush nor write into the audit ring.
pub const DENIAL_LOG_WINDOW: Duration = Duration::from_secs(60);
const DENIAL_LOG_MAX_HOSTS: usize = 256;

/// Per-host rate limit for audit lines about refused requests.
#[derive(Debug, Default)]
pub struct DenialLog(HashMap<IpAddr, (Instant, u32)>);

impl DenialLog {
    /// Count a refusal from `ip`. Returns the line to log — the first refusal
    /// per host per window (with the count held back since), else `None`.
    /// Never contains request text. Past the host cap, hosts share one bucket.
    pub fn note(&mut self, ip: IpAddr, status: u16, now: Instant) -> Option<String> {
        let live = |t: &Instant| now.duration_since(*t) < DENIAL_LOG_WINDOW;
        if self.0.len() >= DENIAL_LOG_MAX_HOSTS && !self.0.contains_key(&ip) {
            self.0.retain(|_, (t, _)| live(t));
        }
        let key = if self.0.len() >= DENIAL_LOG_MAX_HOSTS && !self.0.contains_key(&ip) {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            ip
        };
        if let Some((since, held)) = self.0.get_mut(&key) {
            if live(since) {
                *held += 1;
                return None;
            }
        }
        let held = self.0.get(&key).map_or(0, |e| e.1);
        self.0.insert(key, (now, 0));
        let who = if key.is_unspecified() {
            "other hosts".to_owned()
        } else {
            key.to_string()
        };
        Some(format!(
            "refused {status} from {who} (path not logged; {held} more refused since the last line)"
        ))
    }
}
