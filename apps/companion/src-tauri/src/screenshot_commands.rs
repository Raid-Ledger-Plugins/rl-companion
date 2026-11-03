//! Window commands for the BG3 screenshot hub pick (ROK-1966 Q3): which hubs
//! the server offers, which one the user chose, and how many screenshots wait.

use crate::bg3_screenshot::WAITING;
use crate::config::AppConfig;
use crate::manifest::BG3_SCREENSHOT;
use crate::runtime::Runtime;
use crate::server_config::HubTarget;
use crate::state::Shared;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::State;

/// What the hub picker shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bg3ScreenshotHubsDto {
    /// The server's `hubTargets` for `bg3-screenshot` (empty when it sent none).
    pub targets: Vec<HubTarget>,
    /// `false` when the last `/config` had no `hubTargets` (older server).
    pub server_lists_hubs: bool,
    /// The effective pick ([`effective_hub`]): the user's own while the server
    /// lists it, else the server's default target; `None` = nothing to send to
    /// (nothing uploads).
    pub selected: Option<i64>,
    /// New screenshots held back for want of a pick.
    pub waiting: usize,
}

/// A `CompanionHubGuildIdSchema` value: int 1–2147483647.
pub fn valid_hub(id: i64) -> bool {
    (1..=i64::from(i32::MAX)).contains(&id)
}

/// The server's `hubTargets` for `bg3-screenshot`, minus ids the schema would
/// refuse; `None` = it sent none.
fn hub_targets(cfg: &AppConfig) -> Option<Vec<HubTarget>> {
    cfg.last_server_config
        .as_ref()
        .and_then(|sc| sc.contract(BG3_SCREENSHOT.contract_id))
        .and_then(|c| c.hub_targets.as_ref())
        .map(|ts| {
            ts.iter()
                .filter(|t| valid_hub(t.hub_guild_id))
                .cloned()
                .collect()
        })
}

/// The hub screenshots go to (Q3): the user's own pick while the server still
/// lists it, else the target the server marks `isDefault` (the hub of their
/// latest save), else none (screenshots wait). Resolved on every read, so the
/// default follows the latest save until the user picks one. A pick the
/// server no longer lists (hub left, deleted or moved to another game) is
/// ignored rather than sent and refused; when the server sends no list at
/// all (older server) the pick is kept.
pub fn effective_hub(cfg: &AppConfig) -> Option<i64> {
    resolve(cfg, hub_targets(cfg).as_deref())
}

fn resolve(cfg: &AppConfig, targets: Option<&[HubTarget]>) -> Option<i64> {
    let Some(targets) = targets else {
        return cfg.bg3_screenshot_hub;
    };
    let listed = |id: i64| targets.iter().any(|t| t.hub_guild_id == id);
    cfg.bg3_screenshot_hub
        .filter(|&id| listed(id))
        .or_else(|| {
            targets
                .iter()
                .find(|t| t.is_default)
                .map(|t| t.hub_guild_id)
        })
}

/// The picker's view of `cfg`.
pub fn hubs_view(cfg: &AppConfig, waiting: usize) -> Bg3ScreenshotHubsDto {
    let targets = hub_targets(cfg);
    Bg3ScreenshotHubsDto {
        server_lists_hubs: targets.is_some(),
        selected: resolve(cfg, targets.as_deref()),
        targets: targets.unwrap_or_default(),
        waiting,
    }
}

#[tauri::command]
pub fn get_bg3_screenshot_hubs(state: State<'_, Shared>) -> Bg3ScreenshotHubsDto {
    hubs_view(&state.config(), WAITING.load(Ordering::Relaxed))
}

/// Pick (or clear, with `None`) the hub BG3 screenshots go to, then rescan.
#[tauri::command]
pub fn set_bg3_screenshot_hub(
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    hub_guild_id: Option<i64>,
) -> Result<Bg3ScreenshotHubsDto, String> {
    if hub_guild_id.is_some_and(|id| !valid_hub(id)) {
        return Err("Unknown hub".into());
    }
    let cfg = state
        .update_config(|c| c.bg3_screenshot_hub = hub_guild_id)
        .map_err(|e| format!("Could not save settings: {e}"))?;
    let line = match hub_guild_id {
        Some(id) => format!("BG3 screenshots now go to hub {id}"),
        None => "BG3 screenshots: hub cleared, nothing uploads".to_owned(),
    };
    rt.log_for(BG3_SCREENSHOT.id, line);
    rt.kick();
    Ok(hubs_view(&cfg, WAITING.load(Ordering::Relaxed)))
}
