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
    /// The user's pick; `None` = not picked (nothing uploads).
    pub selected: Option<i64>,
    /// New screenshots held back for want of a pick.
    pub waiting: usize,
}

/// A `CompanionHubGuildIdSchema` value: int 1–2147483647.
pub fn valid_hub(id: i64) -> bool {
    (1..=i64::from(i32::MAX)).contains(&id)
}

/// The picker's view of `cfg`.
pub fn hubs_view(cfg: &AppConfig, waiting: usize) -> Bg3ScreenshotHubsDto {
    let targets = cfg
        .last_server_config
        .as_ref()
        .and_then(|sc| sc.contract(BG3_SCREENSHOT.contract_id))
        .and_then(|c| c.hub_targets.clone());
    Bg3ScreenshotHubsDto {
        server_lists_hubs: targets.is_some(),
        targets: targets.unwrap_or_default(),
        selected: cfg.bg3_screenshot_hub,
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
