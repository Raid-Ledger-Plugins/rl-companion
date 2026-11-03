//! Dev bridge (ROK-2055 C2-L1): bind rules (AC-3), token (AC-5), lockout
//! (AC-4), health schema (AC-6 groundwork), guarded writes (AC-13).

use crate::dev_bridge::auth::{admit, AuthPolicy, Denial, Lockout, LOCKOUT};
use crate::dev_bridge::bind::{bind_addr, check_bind_ip, parse_ipv4, BindError};
use crate::dev_bridge::health::{health, HealthInput};
use crate::dev_bridge::token::{matches, well_formed, DevToken};
use crate::dev_bridge::write_guard::{assert_allowed, guarded_write, queue_path, WriteGuardError};
use serde_json::Value;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

const SCHEMA: &str = include_str!("../../../../../contracts/dev-bridge/v1/schema.json");
const HEALTH_FIXTURE: &str =
    include_str!("../../../../../contracts/dev-bridge/v1/fixtures/health.json");

/// Validate `body` against the pinned `$defs/<def>`; returns the error messages.
pub fn schema_errors(def: &str, body: &Value) -> Vec<String> {
    let mut schema: Value = serde_json::from_str(SCHEMA).expect("schema.json parses");
    let obj = schema.as_object_mut().expect("schema is an object");
    obj.remove("not");
    obj.insert("$ref".into(), Value::String(format!("#/$defs/{def}")));
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    validator
        .iter_errors(body)
        .map(|e| format!("{} at {}", e, e.instance_path))
        .collect()
}

fn ip(s: &str) -> Ipv4Addr {
    s.parse().expect("test IP")
}

#[test]
fn bind_refuses_unspecified() {
    assert_eq!(check_bind_ip(ip("0.0.0.0")), Err(BindError::Unspecified));
}

#[test]
fn bind_refuses_public_ipv4() {
    for raw in [
        "8.8.8.8",
        "1.1.1.1",
        "172.32.0.1",
        "100.64.0.1",
        "192.169.0.1",
    ] {
        assert_eq!(check_bind_ip(ip(raw)), Err(BindError::NotPrivate), "{raw}");
    }
}

#[test]
fn bind_refuses_link_local() {
    assert_eq!(check_bind_ip(ip("169.254.10.1")), Err(BindError::LinkLocal));
}

#[test]
fn bind_refuses_loopback_and_ipv6() {
    assert_eq!(check_bind_ip(ip("127.0.0.1")), Err(BindError::Loopback));
    assert_eq!(parse_ipv4("::"), Err(BindError::NotIpv4));
    assert_eq!(parse_ipv4("fe80::1"), Err(BindError::NotIpv4));
    assert_eq!(parse_ipv4("my-pc.local"), Err(BindError::NotIpv4));
}

#[test]
fn bind_accepts_rfc1918_and_checks_the_port() {
    for raw in ["10.0.0.5", "172.16.0.1", "172.31.255.254", "192.168.0.20"] {
        assert!(bind_addr(raw, 47655).is_ok(), "{raw} is RFC1918");
    }
    assert_eq!(bind_addr("192.168.0.20", 80), Err(BindError::BadPort));
}

#[test]
fn dev_token_is_well_formed_unique_and_redacted() {
    let a = DevToken::generate().expect("rng");
    let b = DevToken::generate().expect("rng");
    assert!(well_formed(a.expose()), "rlcdev_ + 43 base64url");
    assert_ne!(a.expose(), b.expose());
    assert!(
        !format!("{a:?}").contains("rlcdev_"),
        "Debug never prints the token"
    );
    let hash = a.sha256_hex();
    assert!(matches(a.expose(), &hash));
    assert!(!matches(b.expose(), &hash), "another token never matches");
    let device = "rlcd_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AB-_xyz";
    assert!(
        !matches(device, &crate::dev_bridge::token::sha256_hex(device)),
        "a device token is never a dev token"
    );
}

fn policy(token: &DevToken) -> AuthPolicy {
    AuthPolicy {
        allowlist: vec![ip("192.168.0.50")],
        token_sha256: Some(token.sha256_hex()),
    }
}

#[test]
fn admission_refuses_missing_wrong_bearer_and_unlisted_callers() {
    let t = DevToken::generate().expect("rng");
    let p = policy(&t);
    let mut l = Lockout::default();
    let nas = ip("192.168.0.50");
    let now = Instant::now();
    let good = format!("Bearer {}", t.expose());
    assert_eq!(admit(&p, &mut l, nas, None, now), Err(Denial::Unauthorized));
    assert_eq!(
        admit(&p, &mut l, nas, Some("Bearer rlcdev_nope"), now),
        Err(Denial::Unauthorized)
    );
    assert_eq!(
        admit(&p, &mut l, nas, Some(t.expose()), now),
        Err(Denial::Unauthorized),
        "no scheme"
    );
    assert_eq!(
        admit(&p, &mut l, ip("192.168.0.51"), Some(&good), now),
        Err(Denial::Forbidden)
    );
    assert_eq!(admit(&p, &mut l, nas, Some(&good), now), Ok(()));
    let empty = AuthPolicy {
        allowlist: vec![],
        ..p
    };
    assert_eq!(
        admit(&empty, &mut l, nas, Some(&good), now),
        Err(Denial::Forbidden),
        "empty allowlist refuses everyone"
    );
}

#[test]
fn eleventh_failure_in_a_minute_locks_the_address_for_five_minutes() {
    let t = DevToken::generate().expect("rng");
    let p = policy(&t);
    let mut l = Lockout::default();
    let nas = ip("192.168.0.50");
    let good = format!("Bearer {}", t.expose());
    let t0 = Instant::now();
    for i in 0..10 {
        let at = t0 + Duration::from_secs(i * 5);
        assert_eq!(
            admit(&p, &mut l, nas, Some("Bearer x"), at),
            Err(Denial::Unauthorized),
            "failure {}",
            i + 1
        );
    }
    let after = t0 + Duration::from_secs(50);
    assert_eq!(
        admit(&p, &mut l, nas, Some(&good), after),
        Err(Denial::RateLimited),
        "11th request is locked out"
    );
    let other = ip("192.168.0.60");
    let p2 = AuthPolicy {
        allowlist: vec![nas, other],
        ..p.clone()
    };
    assert_eq!(
        admit(&p2, &mut l, other, Some(&good), after),
        Ok(()),
        "lockout is per address"
    );
    let later = t0 + Duration::from_secs(45) + LOCKOUT + Duration::from_secs(1);
    assert_eq!(
        admit(&p, &mut l, nas, Some(&good), later),
        Ok(()),
        "lock lifts after 5 minutes"
    );
}

#[test]
fn failures_spread_over_more_than_a_minute_do_not_lock() {
    let t = DevToken::generate().expect("rng");
    let p = policy(&t);
    let mut l = Lockout::default();
    let nas = ip("192.168.0.50");
    let t0 = Instant::now();
    for i in 0..20 {
        let at = t0 + Duration::from_secs(i * 7);
        assert_eq!(
            admit(&p, &mut l, nas, Some("Bearer x"), at),
            Err(Denial::Unauthorized),
            "attempt {}",
            i + 1
        );
    }
}

#[test]
fn health_body_validates_against_the_pinned_schema() {
    let fixture: Value = serde_json::from_str(HEALTH_FIXTURE).expect("fixture");
    assert_eq!(
        schema_errors("health", &fixture),
        Vec::<String>::new(),
        "golden fixture passes"
    );
    let mut bad = fixture.clone();
    bad["devMode"] = Value::Bool(false);
    assert!(
        !schema_errors("health", &bad).is_empty(),
        "the validator rejects devMode:false"
    );

    let wow = super::temp_dir("bridge-health");
    for d in ["_classic_era_", "_anniversary_", "Data", "_Retail_"] {
        std::fs::create_dir_all(wow.join(d)).expect("mkdir");
    }
    let body = health(&HealthInput {
        wow_dir: wow.to_str().expect("utf8"),
        probes_enabled: false,
        uptime: Duration::from_secs(42),
        now_unix: 1_762_000_000,
    });
    assert_eq!(
        schema_errors("health", &body),
        Vec::<String>::new(),
        "body: {body}"
    );
    assert_eq!(
        body["flavors"],
        serde_json::json!(["_anniversary_", "_classic_era_"])
    );
    let empty = health(&HealthInput {
        wow_dir: "",
        probes_enabled: true,
        uptime: Duration::ZERO,
        now_unix: 0,
    });
    assert_eq!(
        schema_errors("health", &empty),
        Vec::<String>::new(),
        "body: {empty}"
    );
    assert!(empty["wowDir"].is_null());
}

#[test]
fn guarded_write_only_reaches_the_probe_addon_folder() {
    let wow = super::temp_dir("bridge-guard");
    let queue = queue_path(&wow, "_classic_era_").expect("flavor ok");
    guarded_write(&wow, &queue, b"RaidLedgerProbeQueue = [==[{}]==]").expect("queue write");
    assert_eq!(
        std::fs::read(&queue).expect("read"),
        b"RaidLedgerProbeQueue = [==[{}]==]"
    );
    assert!(matches!(
        queue_path(&wow, "../x"),
        Err(WriteGuardError::BadFlavor(_))
    ));
    let refused = [
        wow.join("_classic_era_/WTF/Account/A/SavedVariables/LedgerLink.lua"),
        wow.join("_classic_era_/Interface/AddOns/LedgerLink/LedgerLink.lua"),
        wow.join("_classic_era_/Interface/AddOns/LedgerLinkProbe/../LedgerLink/x.lua"),
        wow.join("_classic_era_/Interface/AddOns/LedgerLinkProbe"),
        wow.join("Data/Interface/AddOns/LedgerLinkProbe/Queue.lua"),
        std::env::temp_dir().join("elsewhere.lua"),
    ];
    for target in refused {
        assert!(
            matches!(
                assert_allowed(&wow, &target),
                Err(WriteGuardError::Outside(_))
            ),
            "{}",
            target.display()
        );
        assert!(
            guarded_write(&wow, &target, b"x").is_err(),
            "{}",
            target.display()
        );
    }
    let _ = std::fs::remove_dir_all(wow);
}
