//! `check_link`'s 401 decision: only a 401 that cleared the token still
//! stored is a revocation (a re-link mid-check keeps its own outcome).

use crate::commands::unauthorized_state;

#[test]
fn a_check_401_that_cleared_the_token_is_a_revocation() {
    assert_eq!(unauthorized_state(true, false), "revoked");
}

#[test]
fn a_check_401_whose_token_a_re_link_replaced_reports_still_linked() {
    assert_eq!(unauthorized_state(false, true), "linked");
}

#[test]
fn a_check_401_whose_token_is_already_gone_reports_unlinked() {
    assert_eq!(unauthorized_state(false, false), "unlinked");
}
