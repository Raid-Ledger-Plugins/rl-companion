//! The BG3 per-campaign upload picker (ROK-2068 fix 3, ROK-2067 D15): saves
//! grouped by `campaignKey`, which groups the user ticked, and the filter
//! that keeps an unticked campaign's saves out of the upload batch.
//! Unticking never deletes anything already uploaded.

use crate::collector::PendingUpload;
use crate::state::Shared;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

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

/// Group saves by key, newest campaign first; the newest save names each.
pub fn group(saves: &[SaveCampaign]) -> Vec<CampaignGroup> {
    let mut out: Vec<CampaignGroup> = Vec::new();
    for s in saves {
        match out.iter_mut().find(|g| g.key == s.key) {
            Some(g) => {
                g.count += 1;
                if s.saved_unix > g.newest_unix {
                    g.newest_unix = s.saved_unix;
                    g.name = s.name.clone();
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

/// `pending` in order, each BG3 save rechecked against the live picks just
/// before it uploads, so unticking a campaign mid-batch stops the rest of
/// that campaign (ROK-2068). `live` runs once per BG3 item as the batch
/// advances, and again per BG3 item still queued whenever a clone of the
/// iterator is walked (a 429 counting what it held); a key with no pick yet
/// keeps the scan's verdict. Other games always pass.
pub fn still_ticked_by<'a>(
    pending: &'a [PendingUpload],
    live: impl Fn() -> BTreeMap<String, bool> + Clone + 'a,
) -> impl Iterator<Item = &'a PendingUpload> + Clone + 'a {
    pending.iter().filter(move |p| {
        p.campaign
            .as_deref()
            .is_none_or(|k| live().get(k).copied().unwrap_or(true))
    })
}

/// [`still_ticked_by`] over the saved config: the upload loop's iterator.
pub fn still_ticked<'a>(
    pending: &'a [PendingUpload],
    app: &'a AppHandle,
) -> impl Iterator<Item = &'a PendingUpload> + Clone + 'a {
    still_ticked_by(pending, move || {
        app.state::<Shared>().config().bg3_campaigns
    })
}

/// What the last BG3 scan saw: its groups (for the window) and the 30-day
/// defaults of campaigns it met for the first time (to persist). The app
/// uses [`STORE`]; a test hands its collector a store of its own, so
/// parallel scans never share one.
#[derive(Debug, Default)]
pub struct CampaignStore(Mutex<(Vec<CampaignGroup>, BTreeMap<String, bool>)>);

impl CampaignStore {
    pub const fn new() -> Self {
        Self(Mutex::new((Vec::new(), BTreeMap::new())))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (Vec<CampaignGroup>, BTreeMap<String, bool>)> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn remember(&self, groups: Vec<CampaignGroup>, first_sight: BTreeMap<String, bool>) {
        *self.lock() = (groups, first_sight);
    }

    pub fn groups(&self) -> Vec<CampaignGroup> {
        self.lock().0.clone()
    }

    /// The first-sight defaults, once: a later call returns them empty.
    pub fn take_first_sight(&self) -> BTreeMap<String, bool> {
        std::mem::take(&mut self.lock().1)
    }
}

/// The app's store, written by the registry's BG3 collector.
pub static STORE: CampaignStore = CampaignStore::new();

/// The last scan's groups, for the window's status.
pub fn last() -> Vec<CampaignGroup> {
    STORE.groups()
}

/// Campaigns with no pick yet, with their 30-day default.
pub fn first_sight(
    groups: &[CampaignGroup],
    ticks: &BTreeMap<String, bool>,
    now_unix: i64,
) -> BTreeMap<String, bool> {
    groups
        .iter()
        .filter(|g| !ticks.contains_key(&g.key))
        .map(|g| (g.key.clone(), default_on(g, now_unix)))
        .collect()
}

/// Add each first-sight default the picks lack; a pick already there (the
/// user's) wins. Returns whether anything was added.
pub fn adopt_defaults(ticks: &mut BTreeMap<String, bool>, fresh: BTreeMap<String, bool>) -> bool {
    let before = ticks.len();
    for (key, on) in fresh {
        ticks.entry(key).or_insert(on);
    }
    ticks.len() > before
}

/// Persist `store`'s first-sight defaults (the app passes [`STORE`]) into
/// `bg3_campaigns`, so a campaign shown OFF stays OFF until the user ticks
/// it, even if they play it again. Writes the config only when a new
/// campaign appeared.
pub fn persist_first_sight(store: &CampaignStore, state: &Shared) {
    let fresh = store.take_first_sight();
    if adopt_defaults(&mut state.config().bg3_campaigns, fresh.clone()) {
        let _ = state.update_config(|c| {
            adopt_defaults(&mut c.bg3_campaigns, fresh);
        });
    }
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
