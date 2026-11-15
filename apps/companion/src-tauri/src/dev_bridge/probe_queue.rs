//! DevQueue.lua rendering (contract "Queue file (data, never code)"): the
//! single assignment `RaidLedgerProbeQueue = [=*[<JSON>]=*]`, the JSON being
//! `$defs/probe-queue`. LedgerLink parses the string with a JSON reader.

use serde_json::{json, Value};

pub const QUEUE_GLOBAL: &str = "RaidLedgerProbeQueue";
pub const QUEUE_VERSION: u64 = 1;
/// Most pending probes one flavor's queue holds (`$defs/probe-queue`).
pub const QUEUE_MAX: usize = 50;

/// The queue JSON, compact: `{"queueVersion":1,"probes":[…]}`.
pub fn queue_json(entries: &[Value]) -> String {
    json!({ "queueVersion": QUEUE_VERSION, "probes": entries }).to_string()
}

/// The smallest long-bracket level whose closing `]=…=]` the text never
/// holds, so the JSON cannot end the Lua string early.
pub fn bracket_level(text: &str) -> usize {
    (0..)
        .find(|n| !text.contains(&format!("]{}]", "=".repeat(*n))))
        .unwrap_or(0)
}

/// The whole DevQueue.lua file. The JSON never starts with a newline, so
/// Lua's skip-first-newline rule never eats a byte of it.
pub fn render(entries: &[Value]) -> String {
    let body = queue_json(entries);
    let eq = "=".repeat(bracket_level(&body));
    format!("{QUEUE_GLOBAL} = [{eq}[{body}]{eq}]\n")
}

/// The JSON text between the long brackets of a rendered file (tests and
/// diagnostics); `None` when `text` is not one `RaidLedgerProbeQueue` string.
pub fn extract(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(QUEUE_GLOBAL)?.strip_prefix(" = [")?;
    let n = rest.bytes().take_while(|b| *b == b'=').count();
    let eq = "=".repeat(n);
    let inner = rest[n..].strip_prefix('[')?;
    let inner = inner.trim_end_matches('\n');
    inner.strip_suffix(&format!("]{eq}]"))
}
