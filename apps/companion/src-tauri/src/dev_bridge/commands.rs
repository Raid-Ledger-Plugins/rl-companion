//! Tauri commands for the "Developer bridge" settings panel (spec §6.5). Each
//! persists through [`Shared::update_config`], then reconciles the listener.
//! The `apply_*` functions hold the logic so tests run without Tauri.

use super::bind::{bind_addr, check_caller, local_private_ipv4s};
use super::settings::MAX_ALLOWLIST;
use super::token::DevToken;
use super::{DevBridge, ListenerStatus};
use crate::config::AppConfig;
use crate::state::Shared;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevBridgeStatusDto {
    enabled: bool,
    bind_ip: Option<String>,
    port: u16,
    allowlist: Vec<String>,
    probes_enabled: bool,
    has_token: bool,
    /// This PC's private IPv4s for the address dropdown.
    addresses: Vec<String>,
    #[serde(flatten)]
    listener: ListenerStatus,
}

/// Returned by Create/Rotate only: the one time the raw token is shown.
#[derive(Serialize)]
pub struct DevTokenDto {
    token: String,
}

fn dto(cfg: &AppConfig, listener: ListenerStatus) -> DevBridgeStatusDto {
    let b = &cfg.dev_bridge;
    DevBridgeStatusDto {
        enabled: b.enabled,
        bind_ip: b.bind_ip.clone(),
        port: b.port,
        allowlist: b.allowlist.clone(),
        probes_enabled: b.probes_enabled,
        has_token: b.token_sha256.is_some(),
        addresses: local_private_ipv4s(),
        listener,
    }
}

fn io(e: std::io::Error) -> String {
    format!("Could not save settings: {e}")
}

pub fn apply_enabled(shared: &Shared, on: bool) -> Result<AppConfig, String> {
    shared
        .update_config(|c| c.dev_bridge.enabled = on)
        .map_err(io)
}

/// "Allow probes" can only be on while the bridge is.
pub fn apply_probes_enabled(shared: &Shared, on: bool) -> Result<AppConfig, String> {
    if on && !shared.config().dev_bridge.enabled {
        return Err("Turn on the Developer bridge first".into());
    }
    shared
        .update_config(|c| c.dev_bridge.probes_enabled = on)
        .map_err(io)
}

pub fn apply_bind(shared: &Shared, ip: &str, port: u16) -> Result<AppConfig, String> {
    let addr = bind_addr(ip, port).map_err(|e| e.to_string())?;
    shared
        .update_config(|c| {
            c.dev_bridge.bind_ip = Some(addr.ip().to_string());
            c.dev_bridge.port = addr.port();
        })
        .map_err(io)
}

pub fn apply_allowlist(shared: &Shared, ips: &[String]) -> Result<AppConfig, String> {
    if ips.len() > MAX_ALLOWLIST {
        return Err(format!("At most {MAX_ALLOWLIST} allowed callers"));
    }
    let mut clean: Vec<String> = Vec::new();
    for raw in ips {
        let ip = check_caller(raw)
            .map_err(|e| format!("{raw}: {e}"))?
            .to_string();
        if !clean.contains(&ip) {
            clean.push(ip);
        }
    }
    shared
        .update_config(|c| c.dev_bridge.allowlist = clean)
        .map_err(io)
}

/// Mint a token, store only its hash; the previous token stops working at
/// the next reconcile. Returns the raw token for the one-time display.
pub fn apply_rotate(shared: &Shared) -> Result<(DevToken, AppConfig), String> {
    let token = DevToken::generate().map_err(|e| format!("Could not create a token: {e}"))?;
    let hash = token.sha256_hex();
    let cfg = shared
        .update_config(|c| c.dev_bridge.token_sha256 = Some(hash))
        .map_err(io)?;
    Ok((token, cfg))
}

type Bridge<'a> = State<'a, Arc<DevBridge>>;

async fn settle(app: &AppHandle, bridge: &DevBridge, cfg: AppConfig) -> DevBridgeStatusDto {
    let listener = bridge.reconcile(&cfg).await;
    crate::tray::set_dev_bridge(app, listener.listening.is_some());
    dto(&cfg, listener)
}

#[tauri::command]
pub async fn get_dev_bridge_status(
    shared: State<'_, Shared>,
    app: AppHandle,
    bridge: Bridge<'_>,
) -> Result<DevBridgeStatusDto, String> {
    Ok(settle(&app, &bridge, shared.config()).await)
}

#[tauri::command]
pub async fn set_dev_bridge_enabled(
    shared: State<'_, Shared>,
    app: AppHandle,
    bridge: Bridge<'_>,
    enabled: bool,
) -> Result<DevBridgeStatusDto, String> {
    let cfg = apply_enabled(&shared, enabled)?;
    Ok(settle(&app, &bridge, cfg).await)
}

#[tauri::command]
pub async fn set_dev_bridge_probes_enabled(
    shared: State<'_, Shared>,
    app: AppHandle,
    bridge: Bridge<'_>,
    enabled: bool,
) -> Result<DevBridgeStatusDto, String> {
    let cfg = apply_probes_enabled(&shared, enabled)?;
    Ok(settle(&app, &bridge, cfg).await)
}

#[tauri::command]
pub async fn set_dev_bridge_bind(
    shared: State<'_, Shared>,
    app: AppHandle,
    bridge: Bridge<'_>,
    ip: String,
    port: u16,
) -> Result<DevBridgeStatusDto, String> {
    let cfg = apply_bind(&shared, &ip, port)?;
    Ok(settle(&app, &bridge, cfg).await)
}

#[tauri::command]
pub async fn set_dev_bridge_allowlist(
    shared: State<'_, Shared>,
    app: AppHandle,
    bridge: Bridge<'_>,
    ips: Vec<String>,
) -> Result<DevBridgeStatusDto, String> {
    let cfg = apply_allowlist(&shared, &ips)?;
    Ok(settle(&app, &bridge, cfg).await)
}

/// Create or Rotate. The window shows `token` once with a Copy button.
#[tauri::command]
pub async fn rotate_dev_bridge_token(
    shared: State<'_, Shared>,
    app: AppHandle,
    bridge: Bridge<'_>,
) -> Result<DevTokenDto, String> {
    let (token, cfg) = apply_rotate(&shared)?;
    let listener = bridge.reconcile(&cfg).await;
    crate::tray::set_dev_bridge(&app, listener.listening.is_some());
    Ok(DevTokenDto {
        token: token.expose().to_owned(),
    })
}
