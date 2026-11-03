//! Tauri commands called by the window (`invoke(...)`). Errors are user-facing strings.
//!
//! The UI never sees the token or the `deviceCode`; it gets the `userCode`
//! (meant to be shown) and the verification URLs.

use crate::config::{default_device_name, device_os, normalise_server_url, sanitise_device_name};
use crate::link::{DeviceInfo, LinkClient, SelfCheck};
use crate::poll::{run_poll, LinkOutcome, PollPlan};
use crate::runtime::Runtime;
use crate::state::Shared;
use crate::tray;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

/// Event the window listens to for the end of a link attempt.
pub const LINK_EVENT: &str = "link-status";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    server_url: String,
    device_name: String,
    linked: bool,
    token_hint: Option<String>,
    paused: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckDto {
    /// `unlinked` | `linked` | `revoked` | `unreachable`
    state: &'static str,
    device_name: Option<String>,
    message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkStartDto {
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkEventDto {
    /// `linked` | `denied` | `expired` | `invalid` | `device_limit` | `failed`
    state: &'static str,
    message: Option<String>,
    device_name: Option<String>,
}

fn s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub fn get_status(state: State<'_, Shared>) -> Result<StatusDto, String> {
    let cfg = state.config();
    let token = state.token().map_err(s)?;
    Ok(StatusDto {
        server_url: cfg.server_url,
        device_name: cfg.device_name.unwrap_or_else(default_device_name),
        linked: token.is_some(),
        token_hint: token.map(|t| t.hint().to_owned()),
        paused: state.is_paused(),
    })
}

/// Start-up self-check: confirms the stored token still works. A 401 clears it.
#[tauri::command]
pub async fn check_link(app: AppHandle, state: State<'_, Shared>) -> Result<CheckDto, String> {
    let Some(token) = state.token().map_err(s)? else {
        tray::set_status(&app, "Not linked");
        return Ok(CheckDto {
            state: "unlinked",
            device_name: None,
            message: None,
        });
    };
    let client = LinkClient::new(&state.config().server_url).map_err(s)?;
    let dto = match client.self_check(&token).await {
        Ok(SelfCheck::Valid { name, .. }) => {
            tray::set_status(&app, &format!("Linked as {name}"));
            CheckDto {
                state: "linked",
                device_name: Some(name),
                message: None,
            }
        }
        Ok(SelfCheck::Unauthorized) => {
            state.clear_token().map_err(s)?;
            tray::set_status(&app, "Re-link this device");
            CheckDto {
                state: "revoked",
                device_name: None,
                message: None,
            }
        }
        Err(e) => CheckDto {
            state: "unreachable",
            device_name: None,
            message: Some(e.to_string()),
        },
    };
    Ok(dto)
}

#[tauri::command]
pub fn set_server_url(
    app: AppHandle,
    state: State<'_, Shared>,
    url: String,
) -> Result<String, String> {
    let url = normalise_server_url(&url)?;
    state.set_link_task(None);
    let changed = state.config().server_url != url;
    let cfg = state
        .update_config(|c| {
            if c.server_url != url {
                // Another server's `/config` says nothing about this one.
                c.last_server_config = None;
                c.fetched_at = None;
            }
            c.server_url = url;
        })
        .map_err(s)?;
    if changed {
        app.state::<Arc<Runtime>>().server_changed();
    }
    let linked = state.token().map_err(s)?.is_some();
    tray::set_status(&app, if linked { "Linked" } else { "Not linked" });
    Ok(cfg.server_url)
}

#[tauri::command]
pub fn set_device_name(state: State<'_, Shared>, name: String) -> Result<String, String> {
    let name = sanitise_device_name(&name).ok_or("Enter a device name")?;
    state
        .update_config(|c| c.device_name = Some(name.clone()))
        .map_err(s)?;
    Ok(name)
}

/// Begin a link: returns the code to show, then polls in the background and
/// emits [`LINK_EVENT`] once with the outcome.
#[tauri::command]
pub async fn start_link(app: AppHandle, state: State<'_, Shared>) -> Result<LinkStartDto, String> {
    let cfg = state.config();
    let device = DeviceInfo {
        name: cfg.device_name.clone().unwrap_or_else(default_device_name),
        os: device_os().to_owned(),
        os_version: None,
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let client = LinkClient::new(&cfg.server_url).map_err(s)?;
    let start = client.start(&device).await.map_err(s)?;
    let plan = PollPlan {
        device_code: start.device_code.clone(),
        interval_secs: start.interval,
        expires_in_secs: start.expires_in,
    };
    let origin = cfg.server_url;
    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let outcome = run_poll(&client, &plan, tokio::time::sleep).await;
        finish_link(&task_app, &origin, outcome);
    });
    state.set_link_task(Some(task));
    tray::set_status(&app, "Waiting for approval…");
    Ok(LinkStartDto {
        user_code: start.user_code,
        verification_uri: start.verification_uri,
        verification_uri_complete: start.verification_uri_complete,
        expires_in: start.expires_in,
    })
}

fn finish_link(app: &AppHandle, origin: &str, outcome: LinkOutcome) {
    let event = |state, message: Option<String>| LinkEventDto {
        state,
        message,
        device_name: None,
    };
    let payload = match outcome {
        LinkOutcome::Linked { token, device } => {
            match app.state::<Shared>().save_token(origin, &token) {
                Ok(()) => {
                    app.state::<Arc<Runtime>>().linked();
                    LinkEventDto {
                        state: "linked",
                        message: None,
                        device_name: Some(device.name),
                    }
                }
                Err(e) => event("failed", Some(e.to_string())),
            }
        }
        LinkOutcome::Denied => event("denied", None),
        LinkOutcome::Expired => event("expired", None),
        LinkOutcome::InvalidGrant => event("invalid", None),
        LinkOutcome::DeviceLimit => event("device_limit", None),
        LinkOutcome::Failed(e) => event("failed", Some(e.to_string())),
    };
    tray::set_status(
        app,
        if payload.state == "linked" {
            "Linked"
        } else {
            "Not linked"
        },
    );
    let _ = app.emit(LINK_EVENT, payload);
}

#[tauri::command]
pub fn cancel_link(app: AppHandle, state: State<'_, Shared>) {
    state.set_link_task(None);
    tray::set_status(&app, "Not linked");
}

/// Open the approval page — only a URL on the configured server's origin.
#[tauri::command]
pub fn open_verification(
    app: AppHandle,
    state: State<'_, Shared>,
    url: String,
) -> Result<(), String> {
    let origin = state.config().server_url;
    let parsed = url::Url::parse(&url).map_err(s)?;
    if parsed.origin().ascii_serialization() != origin {
        return Err("Refusing to open a link to another site".into());
    }
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(s)
}

/// Forget the local token (the device stays listed on the web until removed there).
#[tauri::command]
pub fn unlink(app: AppHandle, state: State<'_, Shared>) -> Result<(), String> {
    state.set_link_task(None);
    state.clear_token().map_err(s)?;
    tray::set_status(&app, "Not linked");
    Ok(())
}
