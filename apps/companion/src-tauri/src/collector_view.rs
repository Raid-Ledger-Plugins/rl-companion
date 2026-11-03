//! Per-game rows for the window (ROK-2061 lane 3): one row per registered
//! collector, plus the device-level state shown once (not per collector).

use crate::bg3_campaigns::{dtos, last, now_unix, Bg3CampaignDto, CampaignGroup};
use crate::config::AppConfig;
use crate::manifest::{resolve_override, CollectorManifest, Platform, BG3_SAVE, WOW_LEDGERLINK};
use crate::preview::PreviewDto;
use crate::registry::BUILTINS;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CollectorRowDto {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    /// This collector's own line from the last scan ("Up to date", …).
    pub status: String,
    /// This OS's default folder, shown when there is no override.
    pub detected_folder: Option<String>,
    /// The user's override as typed ("" = use the detected folder).
    pub folder_override: String,
    /// Why the stored override is not used, when the backend refused it.
    pub override_note: Option<String>,
    pub confirm_first_upload: bool,
    pub consented: bool,
    pub preview: Option<PreviewDto>,
    /// The BG3 campaign picker's rows (ROK-2068); empty for other games and
    /// until the first scan.
    pub campaigns: Vec<Bg3CampaignDto>,
}

/// The rate-limit note for text that is not re-rendered every second (the
/// tray, the baked status line): a countdown there would freeze (ROK-2068).
pub const RETRYING: &str = "Rate limited — retrying automatically";

/// [`device_state`] for text that is not re-rendered: a rate limit reads
/// [`RETRYING`] instead of a countdown.
pub fn device_note(paused: bool, held: bool, linked: Result<bool, String>) -> Option<String> {
    if held && !paused {
        return Some(RETRYING.to_owned());
    }
    device_state(paused, None, linked)
}

/// Device-level state that blocks every collector. `None` = free to upload.
pub fn device_state(
    paused: bool,
    held_secs: Option<u64>,
    linked: Result<bool, String>,
) -> Option<String> {
    if paused {
        return Some("Paused — uploads resume when you unpause".to_owned());
    }
    if let Some(secs) = held_secs {
        return Some(format!("Rate limited — retrying in {secs}s"));
    }
    match linked {
        Ok(true) => None,
        Ok(false) => Some("Link this device to upload".to_owned()),
        Err(e) => Some(format!("Cannot read the device token ({e})")),
    }
}

fn override_of(m: &CollectorManifest, cfg: &AppConfig) -> String {
    if m.id == WOW_LEDGERLINK.id {
        return cfg.wow_dir.clone();
    }
    cfg.collector_roots
        .get(m.id)
        .map(|r| r.join("; "))
        .unwrap_or_default()
}

/// Every refusal for `m`'s stored override, joined (`None` when accepted).
fn override_note(
    m: &CollectorManifest,
    cfg: &AppConfig,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let notes: Vec<String> = cfg
        .roots_override(m.id)?
        .iter()
        .filter_map(|r| resolve_override(r, env).err())
        .map(|e| format!("Not used: {e}"))
        .collect();
    (!notes.is_empty()).then(|| notes.join("; "))
}

pub fn rows(
    cfg: &AppConfig,
    platform: Option<Platform>,
    env: &dyn Fn(&str) -> Option<String>,
    lines: &BTreeMap<String, String>,
    previews: &BTreeMap<String, PreviewDto>,
) -> Vec<CollectorRowDto> {
    BUILTINS
        .iter()
        .map(|b| {
            let m = b.manifest;
            let enabled = cfg.collector_enabled(m.id, m.default_enabled);
            let fallback = if enabled {
                "Waiting for the first scan…"
            } else {
                "Off"
            };
            CollectorRowDto {
                id: m.id.to_owned(),
                display_name: m.display_name.to_owned(),
                enabled,
                status: lines
                    .get(m.id)
                    .cloned()
                    .unwrap_or_else(|| fallback.to_owned()),
                detected_folder: m
                    .default_roots_for(platform, env)
                    .first()
                    .map(|p| p.display().to_string()),
                folder_override: override_of(m, cfg),
                override_note: override_note(m, cfg, env),
                confirm_first_upload: m.confirm_first_upload,
                consented: cfg.consented(m.id),
                preview: previews.get(m.id).cloned(),
                campaigns: campaigns_for(m.id, cfg, &last(), now_unix()),
            }
        })
        .collect()
}

/// The picker rows: BG3's only, from the given scan groups.
pub fn campaigns_for(
    id: &str,
    cfg: &AppConfig,
    groups: &[CampaignGroup],
    now_unix: i64,
) -> Vec<Bg3CampaignDto> {
    if id == BG3_SAVE.id {
        dtos(groups, &cfg.bg3_campaigns, now_unix)
    } else {
        Vec::new()
    }
}

/// The manifest for a window-supplied id, or a user-facing error.
pub fn known(id: &str) -> Result<&'static CollectorManifest, String> {
    BUILTINS
        .iter()
        .map(|b| b.manifest)
        .find(|m| m.id == id)
        .ok_or_else(|| format!("Unknown collector {id:?}"))
}

/// Store `dir` as `id`'s folder override, or clear it when blank. Refuses
/// (without saving) what the registry would drop: relative paths, a bare
/// drive, an unset variable. WoW keeps its own `wowDir` field.
pub fn apply_root(
    cfg: &mut AppConfig,
    id: &str,
    dir: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let m = known(id)?;
    if m.id == WOW_LEDGERLINK.id {
        return Err("Set the World of Warcraft folder in the LedgerLink section".into());
    }
    let dir = dir.trim();
    if dir.is_empty() {
        cfg.collector_roots.remove(id);
        return Ok(());
    }
    resolve_override(dir, env).map_err(|e| format!("{}: {e}", m.display_name))?;
    cfg.collector_roots
        .insert(id.to_owned(), vec![dir.to_owned()]);
    Ok(())
}
