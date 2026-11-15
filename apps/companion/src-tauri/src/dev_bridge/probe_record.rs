//! One stored probe (ROK-2191) and its lifecycle states: `queued` →
//! `answered` | `expired` (past `ttlHours`) | `rejected` | `cancelled`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Longest stored `error` (`probe-detail.error` max 512 chars).
pub const ERROR_MAX_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeStatus {
    Queued,
    Answered,
    Expired,
    Rejected,
    Cancelled,
}

impl ProbeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Answered => "answered",
            Self::Expired => "expired",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }

    /// The `?status=` query value; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Queued,
            Self::Answered,
            Self::Expired,
            Self::Rejected,
            Self::Cancelled,
        ]
        .into_iter()
        .find(|st| st.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRecord {
    pub probe_id: String,
    pub title: String,
    pub status: ProbeStatus,
    /// Unix seconds.
    pub submitted_at: i64,
    /// Unix seconds; set when the probe was answered or rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The resolved folder (never `auto`).
    pub flavor: String,
    pub ttl_hours: u32,
    /// `probe-detail.request`: output form, resolved flavor, no probeId.
    pub request: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

impl ProbeRecord {
    /// Still waiting at `now` (queued and not past its ttl).
    pub fn is_live(&self, now: i64) -> bool {
        self.status == ProbeStatus::Queued && !self.is_due(now)
    }

    /// Queued and past `submittedAt + ttlHours`.
    pub fn is_due(&self, now: i64) -> bool {
        self.status == ProbeStatus::Queued
            && now >= self.submitted_at + i64::from(self.ttl_hours) * 3600
    }

    /// The DevQueue.lua entry: `probeId` first, then the request echo.
    pub fn queue_entry(&self) -> Value {
        let mut entry = Map::new();
        entry.insert("probeId".into(), self.probe_id.clone().into());
        if let Value::Object(m) = &self.request {
            entry.extend(m.clone());
        }
        Value::Object(entry)
    }

    /// One `probe-list` row (`$defs/probe-summary`).
    pub fn summary_json(&self) -> Value {
        let mut v = json!({
            "probeId": self.probe_id,
            "title": self.title,
            "status": self.status.as_str(),
            "submittedAt": rl_collectors::rfc3339_utc_from_unix_seconds(self.submitted_at),
        });
        if let (Some(at), Some(m)) = (self.answered_at, v.as_object_mut()) {
            let ts = rl_collectors::rfc3339_utc_from_unix_seconds(at);
            m.insert("answeredAt".into(), ts.into());
        }
        v
    }
}

/// `msg` cut to [`ERROR_MAX_CHARS`] characters.
pub fn clip_error(msg: &str) -> String {
    msg.chars().take(ERROR_MAX_CHARS).collect()
}
