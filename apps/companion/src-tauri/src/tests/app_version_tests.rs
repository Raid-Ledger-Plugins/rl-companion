//! The reported version is the runtime (release-stamped) one, not
//! `Cargo.toml`'s: a `v0.1.0-test.7` build kept saying `0.1.0` in health.

use crate::app_version::resolve;

#[test]
fn the_runtime_version_wins_over_the_crate_version() {
    assert_eq!(resolve(Some("0.1.0-test.8")), "0.1.0-test.8");
}

#[test]
fn without_a_runtime_version_the_crate_version_is_reported() {
    assert_eq!(resolve(None), env!("CARGO_PKG_VERSION"));
    assert_eq!(resolve(Some("")), env!("CARGO_PKG_VERSION"));
}
