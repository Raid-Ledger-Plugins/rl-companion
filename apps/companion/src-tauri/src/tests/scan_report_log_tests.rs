//! The window's Activity log is never mirrored into the `/dev/v1/logs` ring:
//! its free text carries paths and character, save and campaign names.

use crate::config::AppConfig;
use crate::dev_bridge::logbuf;
use crate::manifest::WOW_LEDGERLINK;
use crate::runtime::Runtime;

fn ring_has(marker: &str) -> bool {
    logbuf::global()
        .select(0, None, None)
        .into_iter()
        .any(|(_, v)| v.to_string().contains(marker))
}

#[test]
fn a_collector_activity_line_never_reaches_the_dev_bridge_ring() {
    let rt = Runtime::new(super::temp_dir("scan-log-no-mirror").join("sent.json"));
    let line = r"Thrall: cannot read C:\Users\Jake\Games\WTF\Account\THRALL\SavedVariables\LedgerLink.lua (marker_m51)";
    rt.log_for(WOW_LEDGERLINK.id, line);
    rt.log("Cannot watch /home/jake/Games/WTF marker_m52 for Thrall");

    // The ring is readable: a line written to it directly is found.
    logbuf::log("info", "collector", "control marker_m50");
    assert!(ring_has("marker_m50"), "the ring cannot be read back");

    assert!(!ring_has("marker_m51"), "a collector line reached the ring");
    assert!(!ring_has("marker_m52"), "a device line reached the ring");
    let log = rt.snapshot(&AppConfig::default()).log;
    assert!(
        log.iter().any(|l| l.text == line),
        "the window keeps the full text: {log:?}"
    );
}
