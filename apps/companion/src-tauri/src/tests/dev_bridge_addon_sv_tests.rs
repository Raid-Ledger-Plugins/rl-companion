//! `GET /dev/v1/addons/<folder>/sv` (`sv-raw`): only the fixed allowlist
//! (`Mailworthy`, `!BugGrabber`, literal or percent-encoded) is served, with
//! the same paging/cursor/file choice as `/dev/v1/ledgerlink/sv`; anything
//! else is 404 and the ledgerlink route still reads only `LedgerLink.lua`.

use super::dev_bridge_routes_tests::{ctx, get, pages, write, CALLER, FLAVOR};
use super::dev_bridge_unit_tests::schema_errors;
use hyper::Method;
use serde_json::{json, Value};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

fn acct_rel(file: &str) -> String {
    format!("{FLAVOR}/WTF/Account/TESTACCOUNT/SavedVariables/{file}")
}

fn char_rel(ch: &str, file: &str) -> String {
    format!("{FLAVOR}/WTF/Account/TESTACCOUNT/Testrealm/{ch}/SavedVariables/{file}")
}

/// A WoW folder with account files for Mailworthy, !BugGrabber, LedgerLink
/// and an addon outside the allowlist.
fn seeded(name: &str) -> PathBuf {
    let wow = super::temp_dir(name);
    let mut big = String::from("MailworthyDB = {\n");
    for i in 0..2500 {
        big.push_str(&format!("\t[\"mail{i}\"] = \"é€ subject {i}\",\n"));
    }
    big.push_str("}\n");
    write(&wow.join(acct_rel("Mailworthy.lua")), big.as_bytes());
    write(
        &wow.join(acct_rel("!BugGrabber.lua")),
        b"BugGrabberDB = {\n\t[\"errors\"] = {},\n}\n",
    );
    write(
        &wow.join(acct_rel("LedgerLink.lua")),
        b"LedgerLinkDB = {}\n",
    );
    write(&wow.join(acct_rel("Secret.lua")), b"SecretDB = {}\n");
    wow
}

fn text_of(wow: &Path, rel: &str) -> String {
    std::fs::read_to_string(wow.join(rel)).expect("read fixture")
}

fn joined(all: &[Value]) -> String {
    all.iter()
        .map(|p| p["text"].as_str().unwrap_or(""))
        .collect()
}

#[test]
fn mailworthy_account_file_pages_like_ledgerlink_sv() {
    let wow = seeded("addon-sv-mailworthy");
    let c = ctx(&wow);
    let base = format!("flavor={FLAVOR}&scope=account");
    let all = pages(&c, "/dev/v1/addons/Mailworthy/sv", "sv-raw", &base);
    assert!(all.len() > 1, "a large Mailworthy.lua needs several pages");
    let rel = acct_rel("Mailworthy.lua");
    assert_eq!(
        joined(&all),
        text_of(&wow, &rel),
        "pages concatenate to the file"
    );
    assert_eq!(
        all[0]["path"],
        json!(rel),
        "path is relative to the WoW folder"
    );
    assert_eq!(all[0]["scope"], json!("account"));
    assert_eq!(all[0]["flavor"], json!(FLAVOR));
}

#[test]
fn bug_grabber_is_served_literal_and_percent_encoded() {
    let wow = seeded("addon-sv-buggrabber");
    let c = ctx(&wow);
    let base = format!("flavor={FLAVOR}&scope=account");
    let want = acct_rel("!BugGrabber.lua");
    for path in [
        "/dev/v1/addons/!BugGrabber/sv",
        "/dev/v1/addons/%21BugGrabber/sv",
        "/dev/v1/addons/%21%42ugGrabber/sv",
    ] {
        let all = pages(&c, path, "sv-raw", &base);
        assert_eq!(all[0]["path"], json!(want), "{path} reads !BugGrabber.lua");
        assert_eq!(joined(&all), text_of(&wow, &want), "{path} text");
    }
}

#[test]
fn folders_outside_the_allowlist_are_not_found() {
    let wow = seeded("addon-sv-unknown");
    let c = ctx(&wow);
    let q = format!("flavor={FLAVOR}&scope=account");
    for path in [
        "/dev/v1/addons/Secret/sv",
        "/dev/v1/addons/LedgerLink/sv",
        "/dev/v1/addons/BugGrabber/sv",
        "/dev/v1/addons/mailworthy/sv",
        "/dev/v1/addons/Mailworthy/../Secret/sv",
        "/dev/v1/addons/..%2FSecret/sv",
        "/dev/v1/addons/Mailworthy%2/sv",
        "/dev/v1/addons//sv",
        "/dev/v1/addons/Mailworthy/sv/parsed",
    ] {
        let (status, body) = get(&c, path, Some(&q));
        assert_eq!(
            (status, &body),
            (404, &json!({ "error": "not_found" })),
            "{path}"
        );
    }
}

#[test]
fn allowlisted_addon_without_a_file_is_not_found() {
    let wow = super::temp_dir("addon-sv-missing");
    write(
        &wow.join(acct_rel("LedgerLink.lua")),
        b"LedgerLinkDB = {}\n",
    );
    write(
        &wow.join(acct_rel("Mailworthy.lua")),
        b"MailworthyDB = {}\n",
    );
    let c = ctx(&wow);
    for (path, scope) in [
        ("/dev/v1/addons/!BugGrabber/sv", "account"),
        ("/dev/v1/addons/Mailworthy/sv", "character"),
    ] {
        let q = format!("flavor={FLAVOR}&scope={scope}");
        let (status, body) = get(&c, path, Some(&q));
        assert_eq!(
            (status, &body),
            (404, &json!({ "error": "not_found" })),
            "{path} {scope}"
        );
    }
}

#[test]
fn character_scope_picks_the_newest_character_file() {
    let wow = seeded("addon-sv-character");
    let old = char_rel("Alpha", "Mailworthy.lua");
    let new = char_rel("Beta", "Mailworthy.lua");
    write(&wow.join(&old), b"MailworthyCharDB = { old = true }\n");
    write(&wow.join(&new), b"MailworthyCharDB = { new = true }\n");
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(wow.join(&old))
        .and_then(|f| f.set_modified(past))
        .expect("age the older file");
    let c = ctx(&wow);
    let q = format!("flavor={FLAVOR}&scope=character");
    let all = pages(&c, "/dev/v1/addons/Mailworthy/sv", "sv-raw", &q);
    assert_eq!(all[0]["path"], json!(new), "newest character file wins");
    assert_eq!(all[0]["scope"], json!("character"));
}

#[test]
fn bad_scope_flavor_and_cursor_are_invalid_requests() {
    let wow = seeded("addon-sv-invalid");
    let c = ctx(&wow);
    for (q, field) in [
        (format!("flavor={FLAVOR}&scope=realm"), "scope"),
        ("flavor=_nope_&scope=account".to_owned(), "flavor"),
        (format!("flavor={FLAVOR}&cursor=!!!"), "cursor"),
    ] {
        let (status, body) = get(&c, "/dev/v1/addons/Mailworthy/sv", Some(&q));
        assert_eq!(status, 400, "{q} → {body}");
        assert_eq!(body["error"], json!("invalid_request"), "{q}");
        assert_eq!(body["issues"][0]["path"], json!([field]), "{q} → {body}");
        assert_eq!(schema_errors("error", &body), Vec::<String>::new(), "{q}");
    }
}

#[test]
fn bearer_auth_is_required() {
    let wow = seeded("addon-sv-auth");
    let c = ctx(&wow);
    let q = format!("flavor={FLAVOR}&scope=account");
    let (status, body) = c.route(
        IpAddr::V4(CALLER),
        &Method::GET,
        "/dev/v1/addons/Mailworthy/sv",
        Some(&q),
        None,
    );
    assert_eq!(status, 401, "no bearer → 401, got {body}");
    assert!(
        body.get("text").is_none(),
        "no file text without auth: {body}"
    );
}

#[test]
fn ledgerlink_sv_still_reads_only_ledgerlink_lua() {
    let wow = seeded("addon-sv-ledgerlink");
    let c = ctx(&wow);
    let q = format!("flavor={FLAVOR}&scope=account");
    let all = pages(&c, "/dev/v1/ledgerlink/sv", "sv-raw", &q);
    assert_eq!(all[0]["path"], json!(acct_rel("LedgerLink.lua")));
    assert_eq!(joined(&all), "LedgerLinkDB = {}\n");
}
