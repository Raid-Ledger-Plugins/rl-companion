//! Window commands for the collector: status, WoW folder, LedgerLink toggle,
//! "Upload now".

use crate::collector_view::{apply_root, known};
use crate::manifest::{BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::{status, CollectorStatusDto, Runtime};
use crate::state::Shared;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
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
    let cfg = state
        .update_config(|c| c.wow_dir = dir.to_owned())
        .map_err(|e| format!("Could not save settings: {e}"))?;
    let bridge = app.try_state::<Arc<crate::dev_bridge::DevBridge>>();
    crate::dev_bridge::on_wow_dir_changed(bridge.as_deref().map(|b| &**b), &cfg);
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
        .update_config(|c| c.set_collector_enabled(WOW_LEDGERLINK.id, enabled))
        .map_err(|e| format!("Could not save settings: {e}"))?;
    rt.log_for(
        WOW_LEDGERLINK.id,
        if enabled {
            "LedgerLink uploads turned on"
        } else {
            "LedgerLink uploads turned off"
        },
    );
    rt.kick();
    Ok(status(&app, &rt))
}

#[tauri::command]
pub fn upload_now(rt: State<'_, Arc<Runtime>>) {
    rt.kick();
}

fn save(state: &Shared, f: impl FnOnce(&mut crate::config::AppConfig)) -> Result<(), String> {
    state
        .update_config(f)
        .map(|_| ())
        .map_err(|e| format!("Could not save settings: {e}"))
}

/// Per-game upload switch (ROK-2061): one per registered collector.
#[tauri::command]
pub fn set_collector_enabled(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    id: String,
    enabled: bool,
) -> Result<CollectorStatusDto, String> {
    let m = known(&id)?;
    save(&state, |c| c.set_collector_enabled(m.id, enabled))?;
    let verb = if enabled { "on" } else { "off" };
    rt.log_for(m.id, format!("{} uploads turned {verb}", m.display_name));
    rt.kick();
    Ok(status(&app, &rt))
}

/// Folder override for a non-WoW collector; blank resets to the detected
/// folder. A refused path is not saved; the error is the window's note.
#[tauri::command]
pub fn set_collector_root(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    id: String,
    dir: String,
) -> Result<CollectorStatusDto, String> {
    let env = |k: &str| std::env::var(k).ok();
    let mut cfg = state.config();
    apply_root(&mut cfg, &id, &dir, &env)?;
    save(&state, |c| c.collector_roots = cfg.collector_roots)?;
    let bridge = app.try_state::<Arc<crate::dev_bridge::DevBridge>>();
    let key_dir = app.path().app_data_dir().ok();
    crate::dev_bridge::on_collector_roots_changed(
        bridge.as_deref().map(|b| &**b),
        &state.config(),
        key_dir,
    );
    rt.kick();
    Ok(status(&app, &rt))
}

/// Native folder picker for a collector's override; `None` when cancelled.
#[tauri::command]
pub async fn pick_collector_root(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    id: String,
) -> Result<Option<CollectorStatusDto>, String> {
    known(&id)?;
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
    set_collector_root(app, state, rt, id, path.to_string_lossy().into_owned()).map(Some)
}

/// Upload on the first-upload preview: remember consent, send the batch.
#[tauri::command]
pub fn confirm_first_upload(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    id: String,
) -> Result<CollectorStatusDto, String> {
    let m = known(&id)?;
    save(&state, |c| c.set_consented(m.id))?;
    rt.set_held(m.id, None);
    rt.log_for(m.id, format!("{}: first upload confirmed", m.display_name));
    rt.kick();
    Ok(status(&app, &rt))
}

/// Skip on the first-upload preview: the held items are never sent; the
/// next new item asks again.
#[tauri::command]
pub fn skip_first_upload(
    app: AppHandle,
    rt: State<'_, Arc<Runtime>>,
    id: String,
) -> Result<CollectorStatusDto, String> {
    let m = known(&id)?;
    let n = rt.skip_held(m.id);
    if n > 0 {
        rt.log_for(
            m.id,
            format!("{}: skipped {n} item(s), nothing uploaded", m.display_name),
        );
    }
    rt.kick();
    Ok(status(&app, &rt))
}

/// Fetch the server's `/config` now (ROK-2067 D5) and return the fresh
/// status, tiles included.
#[tauri::command]
pub async fn refresh_server_config(
    app: AppHandle,
    rt: State<'_, Arc<Runtime>>,
) -> Result<CollectorStatusDto, String> {
    crate::server_config::refresh(&app, &rt).await;
    Ok(status(&app, &rt))
}

/// Tick or untick one BG3 campaign in the picker (ROK-2068): persisted, then
/// a rescan. Unticking only stops future uploads; nothing is deleted.
#[tauri::command]
pub fn set_bg3_campaign(
    app: AppHandle,
    state: State<'_, Shared>,
    rt: State<'_, Arc<Runtime>>,
    key: String,
    ticked: bool,
) -> Result<CollectorStatusDto, String> {
    if !crate::bg3_campaigns::valid_key(&key) {
        return Err("Unknown campaign".into());
    }
    save(&state, |c| {
        c.bg3_campaigns.insert(key.clone(), ticked);
    })?;
    let name = crate::bg3_campaigns::last()
        .into_iter()
        .find(|g| g.key == key)
        .map_or_else(|| "A campaign".to_owned(), |g| g.name);
    let verb = if ticked { "on" } else { "off" };
    rt.log_for(BG3_SAVE.id, format!("{name}: uploads turned {verb}"));
    rt.kick();
    Ok(status(&app, &rt))
}
