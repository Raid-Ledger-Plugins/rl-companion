//! The BG3 per-campaign upload picker (ROK-2068 fix 3, ROK-2067 D15): saves
//! grouped by `campaignKey`, which groups the user ticked, and the filter
//! that keeps an unticked campaign's saves out of the upload batch.
//! Unticking never deletes anything already uploaded.

use crate::collector::PendingUpload;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// A never-seen campaign starts ticked only if its newest save is this recent.
pub const DEFAULT_ON_SECS: i64 = 30 * 24 * 60 * 60;

/// One save's campaign, as the scan read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveCampaign {
    pub key: String,
    /// The save's display name for its campaign ([`campaign_name`]).
    pub name: String,
    /// The save's mtime (unix seconds, clamped to now).
    pub saved_unix: i64,
}

/// Saves sharing one `campaignKey`. `name` comes from the newest save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignGroup {
    pub key: String,
    pub name: String,
    pub newest_unix: i64,
    pub count: usize,
}

/// One picker row for the window.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Bg3CampaignDto {
    pub key: String,
    pub name: String,
    /// The newest save's time (RFC 3339, UTC).
    pub newest_saved_at: String,
    pub save_count: usize,
    pub ticked: bool,
    /// What `ticked` would be without the user's choice (the 30-day rule).
    pub default_on: bool,
}

/// The party leader's name when `meta.lsf` has one, else the save's name.
pub fn campaign_name(leader: Option<&str>, save_name: &str) -> String {
    leader
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.len() <= 64 && !l.chars().any(char::is_control))
        .unwrap_or(save_name)
        .to_owned()
}

/// Group saves by key, newest campaign first. `saves` must be newest first,
/// so each group's first save names it.
pub fn group(saves: &[SaveCampaign]) -> Vec<CampaignGroup> {
    let mut out: Vec<CampaignGroup> = Vec::new();
    for s in saves {
        match out.iter_mut().find(|g| g.key == s.key) {
            Some(g) => {
                g.count += 1;
                if s.saved_unix > g.newest_unix {
                    g.newest_unix = s.saved_unix;
                }
            }
            None => out.push(CampaignGroup {
                key: s.key.clone(),
                name: s.name.clone(),
                newest_unix: s.saved_unix,
                count: 1,
            }),
        }
    }
    out.sort_by(|a, b| b.newest_unix.cmp(&a.newest_unix).then(a.key.cmp(&b.key)));
    out
}

pub fn default_on(g: &CampaignGroup, now_unix: i64) -> bool {
    now_unix - g.newest_unix <= DEFAULT_ON_SECS
}

/// The user's tick for `g`, else the 30-day default.
pub fn ticked(g: &CampaignGroup, ticks: &BTreeMap<String, bool>, now_unix: i64) -> bool {
    ticks
        .get(&g.key)
        .copied()
        .unwrap_or_else(|| default_on(g, now_unix))
}

pub fn dtos(
    groups: &[CampaignGroup],
    ticks: &BTreeMap<String, bool>,
    now_unix: i64,
) -> Vec<Bg3CampaignDto> {
    groups
        .iter()
        .map(|g| Bg3CampaignDto {
            key: g.key.clone(),
            name: g.name.clone(),
            newest_saved_at: rl_collectors::rfc3339_utc_from_unix_seconds(g.newest_unix),
            save_count: g.count,
            ticked: ticked(g, ticks, now_unix),
            default_on: default_on(g, now_unix),
        })
        .collect()
}

/// Group what the scan found and keep only the ticked campaigns' uploads, in
/// the scan's (newest-first) order.
pub fn select(
    found: Vec<(SaveCampaign, Option<PendingUpload>)>,
    ticks: &BTreeMap<String, bool>,
    now_unix: i64,
) -> (Vec<CampaignGroup>, Vec<PendingUpload>) {
    let saves: Vec<SaveCampaign> = found.iter().map(|(c, _)| c.clone()).collect();
    let groups = group(&saves);
    let on: BTreeSet<&str> = groups
        .iter()
        .filter(|g| ticked(g, ticks, now_unix))
        .map(|g| g.key.as_str())
        .collect();
    let pending = found
        .into_iter()
        .filter(|(c, _)| on.contains(c.key.as_str()))
        .filter_map(|(_, p)| p)
        .collect();
    (groups, pending)
}

/// The last scan's groups, for the window's status (set by the BG3 scan).
static LAST: Mutex<Vec<CampaignGroup>> = Mutex::new(Vec::new());

pub fn remember(groups: Vec<CampaignGroup>) {
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = groups;
}

pub fn last() -> Vec<CampaignGroup> {
    LAST.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Unix seconds for `t` (before the epoch → 0).
pub fn unix(t: SystemTime) -> i64 {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    i64::try_from(secs).unwrap_or(0)
}

pub fn now_unix() -> i64 {
    unix(SystemTime::now())
}

/// A window-supplied key is a lowercase hex SHA-256 (contract §3.3).
pub fn valid_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
