use serde_json::json;

use super::fixtures::{
    CASES, PLACEHOLDER_WEBP, act2, json, message_from_golden, meta_from_golden, pipeline,
    synthetic_save,
};
use crate::{
    Bg3SaveMessage, COLLECTOR_VERSION, NormaliseError, normalise_save_info,
    rfc3339_utc_from_unix_seconds,
};

rl_test! {
    fn every_golden_pair_normalises_to_the_pinned_message() {
        for case in &CASES {
            let golden = json(case.golden);
            let payload = normalise_save_info(&json(case.saveinfo), &meta_from_golden(&golden))
                .unwrap_or_else(|e| panic!("{}: {e}", case.name));
            let message = serde_json::to_value(message_from_golden(payload, &golden)).expect("serialises");
            assert_eq!(message, golden, "golden pair {}", case.name);
        }
    }
}

rl_test! {
    fn the_full_package_path_reproduces_the_golden_payload() {
        let golden = json(act2().golden);
        let bytes = synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP));
        let payload = serde_json::to_value(pipeline(&bytes, &golden)).expect("serialises");
        assert_eq!(payload, golden["payload"]);
    }
}

rl_test! {
    fn ignores_unknown_raw_keys() {
        let golden = json(act2().golden);
        let mut raw = json(act2().saveinfo);
        raw["Patch 9 Key"] = json!({ "anything": true });
        raw["Active Party"]["Characters"][0]["Mystery"] = json!([1, 2, 3]);
        let payload = normalise_save_info(&raw, &meta_from_golden(&golden)).expect("loose parse");
        assert_eq!(serde_json::to_value(payload).expect("serialises"), golden["payload"]);
    }
}

rl_test! {
    fn refuses_a_member_with_an_empty_classes_list() {
        let golden = json(act2().golden);
        let mut raw = json(act2().saveinfo);
        raw["Active Party"]["Characters"][2]["Classes"] = json!([]);
        let err = normalise_save_info(&raw, &meta_from_golden(&golden)).unwrap_err();
        assert!(matches!(err, NormaliseError::EmptyClasses(2)), "{err:?}");
    }
}

rl_test! {
    fn refuses_bad_difficulty_tuples_and_empty_parties() {
        let meta = meta_from_golden(&json(act2().golden));
        let mut raw = json(act2().saveinfo);
        raw["Difficulty"] = json!([]);
        let err = normalise_save_info(&raw, &meta).unwrap_err();
        assert!(matches!(err, NormaliseError::Difficulty(0)), "{err:?}");
        raw["Difficulty"] = json!(["a", "b", "c", "d", "e"]);
        let err = normalise_save_info(&raw, &meta).unwrap_err();
        assert!(matches!(err, NormaliseError::Difficulty(5)), "{err:?}");

        let mut raw = json(act2().saveinfo);
        raw["Active Party"]["Characters"] = json!([]);
        let err = normalise_save_info(&raw, &meta).unwrap_err();
        assert!(matches!(err, NormaliseError::EmptyParty), "{err:?}");
    }
}

rl_test! {
    fn refuses_a_raw_shape_it_cannot_read() {
        let meta = meta_from_golden(&json(act2().golden));
        let mut raw = json(act2().saveinfo);
        raw["Active Party"]["Characters"][0]["Level"] = json!("eight");
        let err = normalise_save_info(&raw, &meta).unwrap_err();
        assert!(matches!(err, NormaliseError::Shape(_)), "{err:?}");

        let err = normalise_save_info(&json!({ "Save Name": "x" }), &meta).unwrap_err();
        assert!(matches!(err, NormaliseError::Shape(_)), "{err:?}");
        let err = normalise_save_info(&json!([1, 2]), &meta).unwrap_err();
        assert!(matches!(err, NormaliseError::Shape(_)), "{err:?}");
    }
}

rl_test! {
    fn new_messages_carry_the_contract_constants() {
        let golden = json(act2().golden);
        let payload = normalise_save_info(&json(act2().saveinfo), &meta_from_golden(&golden)).expect("normalises");
        let message = Bg3SaveMessage::new(payload, "2026-10-24T12:00:05Z", "0.1.0");
        assert_eq!(message.envelope_version, 1);
        assert_eq!(message.contract, "bg3-save");
        assert_eq!(message.contract_version, 1);
        assert_eq!(message.collector_version, COLLECTOR_VERSION);
        assert_eq!(COLLECTOR_VERSION, env!("CARGO_PKG_VERSION"));
    }
}

rl_test! {
    fn formats_saved_at_as_utc_z() {
        assert_eq!(rfc3339_utc_from_unix_seconds(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc_from_unix_seconds(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc_from_unix_seconds(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc_from_unix_seconds(951_868_799), "2000-02-29T23:59:59Z");
        assert_eq!(rfc3339_utc_from_unix_seconds(-1), "1969-12-31T23:59:59Z");
    }
}
