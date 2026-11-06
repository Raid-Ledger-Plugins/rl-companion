//! The tile's status line as dev-bridge/v1's closed `collectors[].status`
//! enum. Every line the runtime shows is built together with its status
//! ([`Line`]), and the device-level precedence lives in [`device_block`],
//! which the window's device line is rendered from too — so the tile copy
//! and the bridge status cannot drift apart.

/// `collectors[].status` (dev-bridge/v1 CONTRACT.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LineStatus {
    Off,
    Paused,
    RateLimited,
    LinkDevice,
    NotScanned,
    NoFiles,
    NeedsConfirm,
    Waiting,
    UpToDate,
    Uploaded,
    Rejected,
    Retrying,
    Error,
}

impl LineStatus {
    /// Every value, in the contract's order.
    pub const ALL: [LineStatus; 13] = [
        Self::Off,
        Self::Paused,
        Self::RateLimited,
        Self::LinkDevice,
        Self::NotScanned,
        Self::NoFiles,
        Self::NeedsConfirm,
        Self::Waiting,
        Self::UpToDate,
        Self::Uploaded,
        Self::Rejected,
        Self::Retrying,
        Self::Error,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Paused => "paused",
            Self::RateLimited => "rate-limited",
            Self::LinkDevice => "link-device",
            Self::NotScanned => "not-scanned",
            Self::NoFiles => "no-files",
            Self::NeedsConfirm => "needs-confirm",
            Self::Waiting => "waiting",
            Self::UpToDate => "up-to-date",
            Self::Uploaded => "uploaded",
            Self::Rejected => "rejected",
            Self::Retrying => "retrying",
            Self::Error => "error",
        }
    }
}

/// One collector's line: the tile copy and the status it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub status: LineStatus,
}

impl Line {
    pub fn new(status: LineStatus, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status,
        }
    }
}

/// The line a collector shows before (or without) a scan of its own.
pub fn fallback(enabled: bool) -> Line {
    if enabled {
        Line::new(LineStatus::NotScanned, "Waiting for the first scan…")
    } else {
        Line::new(LineStatus::Off, "Off")
    }
}

/// A device-level state that blocks every collector, in precedence order:
/// paused > rate-limited > link-device > error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceBlock<'a> {
    Paused,
    RateLimited(u64),
    Unlinked,
    TokenUnreadable(&'a str),
}

impl DeviceBlock<'_> {
    pub fn status(self) -> LineStatus {
        match self {
            Self::Paused => LineStatus::Paused,
            Self::RateLimited(_) => LineStatus::RateLimited,
            Self::Unlinked => LineStatus::LinkDevice,
            Self::TokenUnreadable(_) => LineStatus::Error,
        }
    }
}

/// `None` = free to upload.
pub fn device_block(
    paused: bool,
    held_secs: Option<u64>,
    linked: &Result<bool, String>,
) -> Option<DeviceBlock<'_>> {
    if paused {
        return Some(DeviceBlock::Paused);
    }
    if let Some(secs) = held_secs {
        return Some(DeviceBlock::RateLimited(secs));
    }
    match linked {
        Ok(true) => None,
        Ok(false) => Some(DeviceBlock::Unlinked),
        Err(e) => Some(DeviceBlock::TokenUnreadable(e)),
    }
}

/// A row's bridge status: a switched-off collector is `off`; otherwise a
/// device-level block replaces the collector's own line, as in the window.
pub fn row_status(
    enabled: bool,
    device: Option<LineStatus>,
    line: Option<LineStatus>,
) -> LineStatus {
    if !enabled {
        return LineStatus::Off;
    }
    device.or(line).unwrap_or_else(|| fallback(enabled).status)
}

/// The status of a pass that ran to the end ("Uploaded N of M"): `uploaded`
/// when one was accepted, else `waiting` while some are left unsettled (a
/// later scan retries them) and `up-to-date` when every one was settled.
pub fn finished_status(sent: usize, unsettled: usize) -> LineStatus {
    if sent >= 1 {
        LineStatus::Uploaded
    } else if unsettled > 0 {
        LineStatus::Waiting
    } else {
        LineStatus::UpToDate
    }
}
