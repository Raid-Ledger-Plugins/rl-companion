//! Native only: the addon-errors collector's output validates against the
//! pinned `wow-addon-errors/v1` JSON schema, and the pin's golden / invalid
//! fixtures behave as the contract says.

use serde_json::Value;

use super::addon_errors_tests::{BUGGRABBER, UTC, ctx, header};
use super::fixtures::json;
use crate::{AddonErrorsMessage, AddonErrorsState, build_messages, scan_buggrabber};

const SCHEMA: &str = include_str!("../../../../contracts/wow-addon-errors/v1/schema.json");
const GOLDEN: [&str; 3] = [
    include_str!("../../../../contracts/wow-addon-errors/v1/fixtures/buggrabber-minimal.json"),
    include_str!("../../../../contracts/wow-addon-errors/v1/fixtures/buggrabber-three-addons.json"),
    include_str!("../../../../contracts/wow-addon-errors/v1/fixtures/buggrabber-50-records.json"),
];
const INVALID: [&str; 6] = [
    include_str!("../../../../contracts/wow-addon-errors/v1/fixtures/invalid/record-locals.json"),
    include_str!(
        "../../../../contracts/wow-addon-errors/v1/fixtures/invalid/payload-unknown-key.json"
    ),
    include_str!(
        "../../../../contracts/wow-addon-errors/v1/fixtures/invalid/message-users-folder.json"
    ),
    include_str!(
        "../../../../contracts/wow-addon-errors/v1/fixtures/invalid/stack-drive-path.json"
    ),
    include_str!(
        "../../../../contracts/wow-addon-errors/v1/fixtures/invalid/source-mailworthy.json"
    ),
    include_str!("../../../../contracts/wow-addon-errors/v1/fixtures/invalid/account-key-raw.json"),
];

fn errors(instance: &Value) -> Vec<String> {
    let validator = jsonschema::validator_for(&json(SCHEMA)).expect("schema compiles");
    validator
        .iter_errors(instance)
        .map(|e| e.to_string())
        .collect()
}

rl_test! {
    fn collector_output_validates_against_the_pinned_schema() {
        let scan = scan_buggrabber(BUGGRABBER, &ctx(), &UTC).expect("parses");
        let records = AddonErrorsState::default().pending(&scan);
        let messages = build_messages(&header(), records, "2026-11-19T12:00:05Z", "0.4.0").expect("builds");
        assert_eq!(messages.len(), 1);
        for m in &messages {
            let value = serde_json::to_value(m).expect("serialises");
            assert_eq!(errors(&value), Vec::<String>::new(), "{value:#}");
        }
    }
}

rl_test! {
    fn golden_fixtures_validate_and_round_trip_the_message_type() {
        for golden in GOLDEN {
            let value = json(golden);
            assert_eq!(errors(&value), Vec::<String>::new());
            let typed: AddonErrorsMessage = serde_json::from_value(value.clone()).expect("strict parse");
            assert_eq!(serde_json::to_value(typed).expect("serialises"), value);
        }
    }
}

rl_test! {
    fn the_validator_is_not_hollow() {
        for invalid in INVALID {
            assert!(!errors(&json(invalid)).is_empty(), "accepted: {invalid}");
        }
        let windows = json(GOLDEN[0]);
        let mut bad = windows.clone();
        bad["payload"]["records"][0]["message"] = Value::from(r"C:\Users\ExampleUser\x.lua:1: boom");
        assert!(!errors(&bad).is_empty(), "backslash home path accepted");
        assert!(serde_json::from_str::<AddonErrorsMessage>(INVALID[0]).is_err(), "locals key parsed");
    }
}
