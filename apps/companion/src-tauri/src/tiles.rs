//! Game tiles for the window's home view (ROK-2067 D6/D7/D9). One tile per
//! game in the union of the app's collectors and the server's `/config`
//! contracts; each tile's state comes from the cached `/config`. A game with
//! several collectors (BG3 saves + screenshots) is ONE tile with one line per
//! collector (ROK-2073).

use crate::collector_view::CollectorRowDto;
use crate::config::AppConfig;
use crate::manifest::CollectorManifest;
use crate::registry::BUILTINS;
use crate::runtime::CharResult;
use crate::server_config::{PluginState, ServerContract, ServerPlugin};
use crate::upload::NOT_TAKEN_YET;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TileState {
    Supported,
    PluginOff,
    NotOnServer,
    AppTooOld,
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TileDto {
    /// The contract id (equals the collector id for the app's collectors).
    pub id: String,
    /// The collector whose panel the tile opens; `None` for `app_too_old`.
    pub collector_id: Option<String>,
    pub title: String,
    /// A second line under the title ("LedgerLink addon").
    pub subtitle: Option<String>,
    pub state: TileState,
    /// The label drawn ON the art (warning chip or greyed label).
    pub chip: Option<String>,
    /// The tile's status line.
    pub line: String,
    /// The game view's banner (`plugin_off`, `not_on_server`).
    pub banner: Option<String>,
    /// The owner plugin's name, for the UI to emphasise inside `banner`.
    pub plugin_name: Option<String>,
    pub clickable: bool,
    /// The cover as a `data:` URL; `None` → draw the fallback.
    pub cover: Option<String>,
    pub fallback: bool,
    /// Up to two letters for the fallback art.
    pub initials: String,
    /// One line per collector of the game (ROK-2073); empty for `app_too_old`.
    /// The tile's own `state`/`chip`/`line`/`banner` are its [`headline`].
    pub lines: Vec<TileLineDto>,
}

/// One collector's state inside its game's tile.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TileLineDto {
    pub collector_id: String,
    /// "Saves", "Screenshots" (the manifest's `line_label`).
    pub label: String,
    pub state: TileState,
    pub chip: Option<String>,
    pub line: String,
    /// This collector's game-view banner (`plugin_off`, `not_on_server`).
    pub banner: Option<String>,
    pub plugin_name: Option<String>,
}

pub struct TileInputs<'a> {
    pub cfg: &'a AppConfig,
    pub linked: bool,
    pub rows: &'a [CollectorRowDto],
    pub covers: &'a BTreeMap<String, Option<String>>,
}

/// One app collector's server verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict<'a> {
    Supported,
    PluginOff(&'a ServerPlugin),
    NotOnServer,
    Unlinked,
    Checking,
}

fn verdict<'a>(cfg: &'a AppConfig, linked: bool, contract_id: &str) -> Verdict<'a> {
    if !linked {
        return Verdict::Unlinked;
    }
    let Some(sc) = cfg.last_server_config.as_ref() else {
        return Verdict::Checking;
    };
    match sc.contract(contract_id) {
        None => Verdict::NotOnServer,
        Some(c) if c.enabled => Verdict::Supported,
        Some(ServerContract {
            plugin: Some(p), ..
        }) if matches!(p.state, PluginState::Inactive | PluginState::NotInstalled) => {
            Verdict::PluginOff(p)
        }
        Some(_) => Verdict::NotOnServer,
    }
}

fn off_line(p: &ServerPlugin) -> &'static str {
    if p.state == PluginState::NotInstalled {
        "Plugin not installed"
    } else {
        "Plugin turned off"
    }
}

fn off_banner(p: &ServerPlugin) -> String {
    let name = &p.name;
    if p.state == PluginState::NotInstalled {
        format!("Uploads are paused. This server doesn't have the {name} plugin installed. Ask your Raid Ledger admin to install it.")
    } else {
        format!("Uploads are paused. This server has the {name} plugin turned off. Ask your Raid Ledger admin to turn it on.")
    }
}

fn server_host(cfg: &AppConfig) -> String {
    url::Url::parse(&cfg.server_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| cfg.server_url.clone())
}

/// Up to two capitals/digits that start words ("Baldur's Gate 3" → "BG").
pub fn initials(title: &str) -> String {
    title
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .filter(|c| c.is_uppercase() || c.is_ascii_digit())
        .take(2)
        .collect()
}

fn base(v: &TileInputs, id: &str, title: String, state: TileState, line: String) -> TileDto {
    let cover = v.covers.get(id).cloned().flatten();
    TileDto {
        id: id.to_owned(),
        collector_id: None,
        initials: initials(&title),
        title,
        subtitle: None,
        state,
        chip: None,
        line,
        banner: None,
        plugin_name: None,
        clickable: true,
        fallback: cover.is_none(),
        cover,
        lines: Vec::new(),
    }
}

/// The game's name: the server's from the first of the game's collectors whose
/// contract names one (like the cover), else the first manifest's.
fn game_title(v: &TileInputs, ms: &[&CollectorManifest], first: &CollectorManifest) -> String {
    let sc = v.cfg.last_server_config.as_ref();
    ms.iter()
        .find_map(|m| sc?.contract(m.contract_id)?.game.as_ref())
        .map_or_else(|| first.game_name.to_owned(), |g| g.name.clone())
}

/// One collector's line inside its game's tile. `what` names its uploads in
/// the not-on-server banner ("Baldur's Gate 3 uploads").
fn app_line(v: &TileInputs, m: &CollectorManifest, what: &str) -> TileLineDto {
    let status = v
        .rows
        .iter()
        .find(|r| r.id == m.id)
        .map(|r| r.status.clone());
    let verdict = verdict(v.cfg, v.linked, m.contract_id);
    let (state, line) = match &verdict {
        Verdict::Supported => (TileState::Supported, status.unwrap_or_default()),
        Verdict::PluginOff(p) => (TileState::PluginOff, off_line(p).to_owned()),
        Verdict::NotOnServer => (
            TileState::NotOnServer,
            "Not supported on this server".into(),
        ),
        Verdict::Unlinked => (
            TileState::Unknown,
            format!(
                "Link this device to see what {} supports",
                server_host(v.cfg)
            ),
        ),
        Verdict::Checking => (TileState::Unknown, "Checking the server…".into()),
    };
    let mut l = TileLineDto {
        collector_id: m.id.to_owned(),
        label: m.line_label.to_owned(),
        state,
        chip: None,
        line,
        banner: None,
        plugin_name: None,
    };
    if let Verdict::PluginOff(p) = verdict {
        l.chip = Some(off_line(p).to_owned());
        l.banner = Some(off_banner(p));
        l.plugin_name = Some(p.name.clone());
    } else if state == TileState::NotOnServer {
        l.chip = Some("Not on this server".into());
        l.banner = Some(format!(
            "This server doesn't take {what}. Your settings are kept."
        ));
    }
    l
}

/// Higher = more urgent, for the tile's chip/overlay (ROK-2073).
fn urgency(s: TileState) -> u8 {
    match s {
        TileState::Supported => 0,
        TileState::Unknown => 1,
        TileState::NotOnServer => 2,
        TileState::PluginOff => 3,
        TileState::AppTooOld => 4,
    }
}

/// The line that speaks for the whole tile: the most urgent, earliest first.
/// `not_on_server` leads only when NO collector of the game is on the server
/// — a partly supported game keeps it on that collector's line (AC2).
pub fn headline(lines: &[TileLineDto]) -> Option<&TileLineDto> {
    let on: Vec<&TileLineDto> = lines
        .iter()
        .filter(|l| l.state != TileState::NotOnServer)
        .collect();
    let pool = if on.is_empty() {
        lines.iter().collect()
    } else {
        on
    };
    pool.into_iter().rev().max_by_key(|l| urgency(l.state))
}

/// One tile for a game's collectors (`ms`, non-empty, `BUILTINS` order).
fn game_tile(v: &TileInputs, ms: &[&CollectorManifest]) -> Option<TileDto> {
    let first = ms.first()?;
    let title = game_title(v, ms, first);
    let lines: Vec<TileLineDto> = ms
        .iter()
        .map(|m| {
            let what = if ms.len() == 1 {
                format!("{title} uploads")
            } else {
                format!("{title} {}", m.line_label.to_lowercase())
            };
            app_line(v, m, &what)
        })
        .collect();
    let head = headline(&lines)?.clone();
    let mut t = base(v, first.contract_id, title, head.state, head.line);
    if let Some(c) = ms
        .iter()
        .find_map(|m| v.covers.get(m.contract_id).cloned().flatten())
    {
        (t.cover, t.fallback) = (Some(c), false);
    }
    t.collector_id = Some(first.id.to_owned());
    t.subtitle = first
        .game_subtitle
        .filter(|_| ms.len() == 1)
        .map(str::to_owned);
    (t.chip, t.banner, t.plugin_name) = (head.chip, head.banner, head.plugin_name);
    if head.state == TileState::NotOnServer {
        // Only when no collector is on the server: name the whole game.
        t.banner = Some(format!(
            "This server doesn't take {} uploads. Your settings are kept.",
            t.title
        ));
    }
    t.lines = lines;
    Some(t)
}

/// Collectors grouped by `game_key`, in input order (ROK-2073). The key, not
/// the display name, so rewording one manifest's copy cannot split a tile.
pub fn group_by_game<'a>(
    ms: impl IntoIterator<Item = &'a CollectorManifest>,
) -> Vec<Vec<&'a CollectorManifest>> {
    let mut out: Vec<Vec<&'a CollectorManifest>> = Vec::new();
    for m in ms {
        match out
            .iter_mut()
            .find(|g| g.first().is_some_and(|f| f.game_key == m.game_key))
        {
            Some(g) => g.push(m),
            None => out.push(vec![m]),
        }
    }
    out
}

/// The app's collectors grouped by game, in `BUILTINS` order.
fn games() -> Vec<Vec<&'static CollectorManifest>> {
    group_by_game(BUILTINS.iter().map(|b| b.manifest))
}

fn too_old_tile(v: &TileInputs, c: &ServerContract) -> TileDto {
    let title = c
        .plugin
        .as_ref()
        .map_or_else(|| c.id.clone(), |p| p.name.clone());
    let line = "Update the Companion to use this".to_owned();
    let mut t = base(v, &c.id, title, TileState::AppTooOld, line);
    t.chip = Some("Update the Companion".into());
    t.plugin_name = c.plugin.as_ref().map(|p| p.name.clone());
    t.clickable = false;
    t
}

/// D6: the app's collectors in `BUILTINS` order, then server-only contracts
/// A–Z (only while linked with a cached `/config`).
pub fn tiles(v: &TileInputs) -> Vec<TileDto> {
    let mut out: Vec<TileDto> = games().iter().filter_map(|g| game_tile(v, g)).collect();
    let known = |id: &str| BUILTINS.iter().any(|b| b.manifest.contract_id == id);
    let mut extra: Vec<&ServerContract> = match (&v.cfg.last_server_config, v.linked) {
        (Some(sc), true) => sc
            .contracts
            .iter()
            .flatten()
            .filter(|c| !known(&c.id))
            .collect(),
        _ => Vec::new(),
    };
    extra.sort_by(|a, b| a.id.cmp(&b.id));
    extra.dedup_by(|a, b| a.id == b.id);
    out.extend(extra.into_iter().map(|c| too_old_tile(v, c)));
    out
}

/// D7 "Last checked": the cache's time, only while the server is unreachable.
pub fn last_checked(cfg: &AppConfig, linked: bool, reachable: Option<bool>) -> Option<String> {
    let shown = linked && reachable == Some(false) && cfg.last_server_config.is_some();
    shown.then(|| cfg.fetched_at.clone()).flatten()
}

/// D9: for a collector whose plugin is off, the generic "the server does not
/// take this upload yet" becomes the plugin warning, in its row and results.
pub fn apply_plugin_off_copy(
    cfg: &AppConfig,
    linked: bool,
    rows: &mut [CollectorRowDto],
    results: &mut [CharResult],
) {
    for row in rows.iter_mut() {
        let Some(m) = BUILTINS.iter().map(|b| b.manifest).find(|m| m.id == row.id) else {
            continue;
        };
        let Verdict::PluginOff(p) = verdict(cfg, linked, m.contract_id) else {
            continue;
        };
        let warning = off_line(p);
        if row.status.contains(NOT_TAKEN_YET) {
            row.status = warning.to_owned();
        }
        results
            .iter_mut()
            .filter(|r| r.collector_id == row.id && r.result.contains(NOT_TAKEN_YET))
            .for_each(|r| r.result = warning.to_owned());
    }
}
