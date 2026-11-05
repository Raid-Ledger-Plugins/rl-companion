//! ROK-2079 OQ-4: `party[].characterKeyPrefix` on the BG3 save-structure read.
//! The golden vector was derived outside the code under test:
//! `printf '%s' "$K:03020100-0504-0706-0908-0b0a0d0c0f0e" | shasum -a 256`
//! with `K = sha256("rok-2079-campaign")`.

use crate::dev_bridge::bg3_character_key::{character_key_prefix, EntityGuid};
use crate::dev_bridge::bg3_newage::{party_row, PartyDecoded};

const CAMPAIGN_KEY: &str = "69b941f15595dece091534503b509f30019bef3a6a5e7c62aa2a44ac4319afce";
/// Bytes `00..0f` as stored; LSLib's text order makes them the GUID below.
const RAW: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const GUID_TEXT: &str = "03020100-0504-0706-0908-0b0a0d0c0f0e";
const GUID_BARE: &str = "030201000504070609080b0a0d0c0f0e";
/// The same bytes hex-encoded in stored order.
const RAW_HEX: &str = "000102030405060708090a0b0c0d0e0f";

#[test]
fn the_prefix_is_the_first_8_hex_of_sha256_campaign_key_colon_guid() {
    let prefix = character_key_prefix(Some(CAMPAIGN_KEY), &EntityGuid::new(RAW));
    assert_eq!(prefix.as_deref(), Some("8f0025fa"));
}

#[test]
fn the_guid_text_is_lowercase_hyphenated_in_lslib_order() {
    assert_eq!(EntityGuid::new(RAW).text(), GUID_TEXT);
}

#[test]
fn no_or_malformed_campaign_key_means_no_prefix() {
    let g = EntityGuid::new(RAW);
    assert_eq!(character_key_prefix(None, &g), None);
    assert_eq!(character_key_prefix(Some(&CAMPAIGN_KEY[..8]), &g), None);
    assert_eq!(
        character_key_prefix(Some(&CAMPAIGN_KEY.to_uppercase()), &g),
        None
    );
}

#[test]
fn the_guid_never_reaches_debug_output() {
    let dbg = format!("{:?}", EntityGuid::new(RAW));
    assert!(
        !dbg.contains(GUID_BARE) && !dbg.contains("03020100") && !dbg.contains(RAW_HEX),
        "{dbg}"
    );
}

#[test]
fn a_party_row_carries_the_prefix_and_never_the_guid() {
    let d = PartyDecoded {
        entity_guid: Some(EntityGuid::new(RAW)),
        ..PartyDecoded::default()
    };
    let row = party_row(0, Some(&d), Some(CAMPAIGN_KEY));
    assert_eq!(row["characterKeyPrefix"], "8f0025fa");
    let text = row.to_string();
    assert!(
        !text.contains(GUID_TEXT) && !text.contains(GUID_BARE) && !text.contains(RAW_HEX),
        "{text}"
    );
}

#[test]
fn a_party_row_without_a_guid_or_key_has_no_prefix_key() {
    let no_guid = party_row(0, Some(&PartyDecoded::default()), Some(CAMPAIGN_KEY));
    assert!(no_guid.get("characterKeyPrefix").is_none(), "{no_guid}");
    let d = PartyDecoded {
        entity_guid: Some(EntityGuid::new(RAW)),
        ..PartyDecoded::default()
    };
    let no_key = party_row(0, Some(&d), None);
    assert!(no_key.get("characterKeyPrefix").is_none(), "{no_key}");
    let undecoded = party_row(3, None, Some(CAMPAIGN_KEY));
    assert_eq!(undecoded, serde_json::json!({ "slot": 3, "decoded": {} }));
}
