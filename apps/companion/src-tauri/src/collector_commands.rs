//! Window commands for the collector: status, WoW folder, LedgerLink toggle,
//! "Upload now".

use crate::runtime::{status, CollectorStatusDto, Runtime};
use crate::state::Shared;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub fn get_collector_status(app: AppHandle, rt: State<'_, Arc<Runtime>>) -> CollectorStatusDto {
    status(&app, &rt)
}

fn save_dir(
    app: &AppHandle,
    state: &Shared,
    rt: &Runtime,
    dir: &str,
) -> Result<CollectorStatusDto, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("Choose the World of Warcraft folder".into());
    }
    state
        .update_config(|c| c.wow_dir = dir.to_owned())
        .map_err(|e| format!("Could not save settings: {e}"))?;
    rt.kick();
    Ok(status(app, rt))
}

#[tauri::command]
pub fn set_wow_dir(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    dir: String,
) -> Result<CollectorStatusDto, String> {
    save_dir(&app, &state, &rt, &dir)
}

/// Native folder picker; `None` when the user cancels.
#[tauri::command]
pub async fn pick_wow_dir(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
) -> Result<Option<CollectorStatusDto>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    let Some(picked) = rx
        .await
        .map_err(|_| "The folder picker closed unexpectedly".to_string())?
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    save_dir(&app, &state, &rt, &path.to_string_lossy()).map(Some)
}

#[tauri::command]
pub fn set_ledgerlink_enabled(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    enabled: bool,
) -> Result<CollectorStatusDto, String> {
    state
        .update_config(|c| c.ledgerlink_enabled = enabled)
        .map_err(|e| format!("Could not save settings: {e}"))?;
    rt.log(if enabled {
        "LedgerLink uploads turned on"
    } else {
        "LedgerLink uploads turned off"
    });
    rt.kick();
    Ok(status(&app, &rt))
}

#[tauri::command]
pub fn upload_now(rt: State<'_, Arc<Runtime>>) {
    rt.kick();
}
