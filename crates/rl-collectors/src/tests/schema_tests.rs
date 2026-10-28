//! Native only: validate normaliser output against the pinned
//! `bg3-save/v1` JSON schema (`jsonschema` does not target WASM here).

use serde_json::Value;

use super::fixtures::{
    CASES, PLACEHOLDER_WEBP, SCHEMA, act2, json, message_from_golden, meta_from_golden, pipeline,
    synthetic_save,
};
use crate::{Bg3SaveMessage, normalise_save_info};

const INVALID_PAYLOAD_UNKNOWN_KEY: &str =
    include_str!("../../../../contracts/bg3-save/v1/fixtures/invalid/payload-unknown-key.json");
const INVALID_RAW_FOLDER_KEY: &str =
    include_str!("../../../../contracts/bg3-save/v1/fixtures/invalid/campaign-key-raw-folder.json");

fn errors(instance: &Value) -> Vec<String> {
    let validator = jsonschema::validator_for(&json(SCHEMA)).expect("schema compiles");
    validator.iter_errors(instance).map(|e| e.to_string()).collect()
}

rl_test! {
    fn normaliser_output_validates_for_every_golden_pair() {
        for case in &CASES {
            let golden = json(case.golden);
            let payload = normalise_save_info(&json(case.saveinfo), &meta_from_golden(&golden)).expect("normalises");
            let message = serde_json::to_value(message_from_golden(payload, &golden)).expect("serialises");
            assert_eq!(errors(&message), Vec::<String>::new(), "case {}", case.name);
        }
    }
}

rl_test! {
    fn a_message_built_from_a_package_validates() {
        let golden = json(act2().golden);
        let bytes = synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP));
        let message = Bg3SaveMessage::new(pipeline(&bytes, &golden), "2026-10-24T12:00:05Z", "0.1.0");
        let value = serde_json::to_value(message).expect("serialises");
        assert_eq!(errors(&value), Vec::<String>::new());
    }
}

rl_test! {
    fn the_validator_is_not_hollow() {
        assert!(!errors(&json(INVALID_PAYLOAD_UNKNOWN_KEY)).is_empty(), "unknown payload key accepted");
        assert!(!errors(&json(INVALID_RAW_FOLDER_KEY)).is_empty(), "raw folder name accepted");
    }
}
