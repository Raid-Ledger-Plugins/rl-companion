//! In-app update rules, kept free of Tauri so they unit-test (updater.rs
//! does the I/O).
//!
//! Operator rulings 2026-11-05:
//! - **Channel by build.** A build whose version carries a pre-release tag
//!   (`0.1.0-test.6`) follows the `test` channel, which every release tag
//!   feeds. A plain `X.Y.Z` build follows `stable`, which only plain tags feed.
//! - **Auto-install when idle.** A found update downloads straight away and
//!   installs on its own only while nothing is in flight: no scan or upload
//!   running, the window hidden, and the dev bridge quiet for
//!   [`BRIDGE_QUIET`]. "Update now" installs at once.

use serde::Serialize;
use std::time::Duration;

/// Where the release workflow publishes each channel's `latest.json`: a
/// rolling GitHub release per channel whose asset is replaced on every tag.
pub const RELEASES: &str = "https://github.com/Raid-Ledger-Plugins/rl-companion/releases/download";

/// The first check waits for the first collector scan to settle.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
/// The app lives in the tray for days, so the check repeats.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// After a failed check (offline, GitHub down), retry sooner.
pub const RETRY_AFTER_ERROR: Duration = Duration::from_secs(60 * 60);
/// How often a downloaded update re-tests the idle rule.
pub const IDLE_POLL: Duration = Duration::from_secs(30);
/// The dev bridge counts as idle once it has answered nothing for this long.
pub const BRIDGE_QUIET: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Test,
}

impl Channel {
    /// The channel a build follows, from its own version.
    pub fn for_version(v: &semver::Version) -> Self {
        if v.pre.is_empty() {
            Channel::Stable
        } else {
            Channel::Test
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Test => "test",
        }
    }

    /// The release tag that holds this channel's `latest.json`.
    pub fn release_tag(self) -> String {
        format!("updates-{}", self.name())
    }

    /// The `latest.json` URL the updater polls.
    pub fn endpoint(self) -> String {
        format!("{RELEASES}/{}/latest.json", self.release_tag())
    }
}

/// What the idle rule looks at.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdleInputs {
    /// A collector scan (and its uploads) is running.
    pub scanning: bool,
    /// The main window is on screen: the user may be in the middle of something.
    pub window_visible: bool,
    /// Time since the dev bridge last answered a request; `None` = never.
    pub since_bridge_request: Option<Duration>,
}

/// Whether a downloaded update may install (and restart) on its own now.
pub fn is_idle(i: &IdleInputs) -> bool {
    let bridge_quiet = i.since_bridge_request.is_none_or(|d| d >= BRIDGE_QUIET);
    !i.scanning && !i.window_visible && bridge_quiet
}

/// When to check again after a check that `succeeded` (or not).
pub fn next_check_in(succeeded: bool) -> Duration {
    if succeeded {
        CHECK_EVERY
    } else {
        RETRY_AFTER_ERROR
    }
}

/// Download progress as a whole percent, when the size is known.
pub fn percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|t| *t > 0)?;
    Some((downloaded.min(total) * 100 / total) as u8)
}

/// Where the updater is. `version` is the offered version where there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum Phase {
    /// No check has finished yet.
    Idle,
    Checking,
    UpToDate,
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    /// Downloaded and verified; installs when idle or on "Update now".
    Ready {
        version: String,
    },
    Installing {
        version: String,
    },
    Error {
        message: String,
    },
}

/// The window's view of the updater (`get_update_status`, `update-status` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    pub channel: Channel,
    #[serde(flatten)]
    pub phase: Phase,
    /// RFC 3339 time of the last finished check.
    pub last_checked: Option<String>,
}

/// The tray item's text, and whether clicking it does anything.
pub fn tray_label(phase: &Phase) -> (String, bool) {
    match phase {
        Phase::Checking => ("Checking for updates…".into(), false),
        Phase::Downloading { version, .. } => (format!("Downloading {version}…"), false),
        Phase::Ready { version } => (format!("Update to {version} and restart"), true),
        Phase::Installing { version } => (format!("Installing {version}…"), false),
        Phase::Idle | Phase::UpToDate | Phase::Error { .. } => ("Check for updates".into(), true),
    }
}

/// What a click on the tray item (or the window's button) should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Check,
    InstallNow,
    Nothing,
}

pub fn click_action(phase: &Phase) -> Action {
    match phase {
        Phase::Ready { .. } => Action::InstallNow,
        Phase::Idle | Phase::UpToDate | Phase::Error { .. } => Action::Check,
        Phase::Checking | Phase::Downloading { .. } | Phase::Installing { .. } => Action::Nothing,
    }
}

/// Marker left in the app data dir before an automatic install, so the
/// relaunched app stays in the tray instead of popping its window up.
pub const SILENT_RESTART_MARKER: &str = "update-silent-restart";
