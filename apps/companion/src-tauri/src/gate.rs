//! The confirm-first consent gate, kept pure (no `AppHandle`) so it can be
//! unit-tested: a confirm-first collector without consent holds its batch
//! behind the local preview and sends nothing.

use crate::collector::PendingUpload;
use crate::manifest::CollectorManifest;
use crate::preview::{hold, Held};

/// What one collector's scan does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// No files found; any earlier hold is stale and is cleared.
    NoFiles,
    /// Files found, all already handled; any earlier hold is cleared.
    UpToDate,
    /// First upload of a confirm-first collector: show this preview and send
    /// nothing until the user says Upload (consent) or Skip.
    Hold(Option<Box<Held>>),
    /// Upload the backlog.
    Send,
}

pub fn gate(
    m: &CollectorManifest,
    consented: bool,
    files: usize,
    pending: &[PendingUpload],
) -> Gate {
    if files == 0 {
        return Gate::NoFiles;
    }
    if pending.is_empty() {
        return Gate::UpToDate;
    }
    if m.confirm_first_upload && !consented {
        return Gate::Hold(hold(m.id, pending).map(Box::new));
    }
    Gate::Send
}
