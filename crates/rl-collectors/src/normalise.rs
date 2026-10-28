//! `SaveInfo.json` → `bg3-save/v1` payload, following the pinned contract
//! (`contracts/bg3-save/v1/CONTRACT.md` §3). Pure: no hashing, clock or IO.
//! The caller supplies the facts the file does not hold in [`SaveMeta`].

use serde::{Deserialize, Serialize};

use crate::error::NormaliseError;

/// `contract` field of the envelope.
pub const CONTRACT: &str = "bg3-save";
/// `contractVersion` field of the envelope.
pub const CONTRACT_VERSION: u32 = 1;
/// `envelopeVersion` field of the envelope.
pub const ENVELOPE_VERSION: u32 = 1;
/// `collectorVersion`: this crate's version.
pub const COLLECTOR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What `campaignKey` was hashed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CampaignKeySource {
    SaveFolder,
    GameId,
}

/// Reference to the uploaded WebP thumbnail blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailRef {
    pub sha256: String,
    pub byte_length: u64,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
}

/// Facts the collector supplies (contract §3.1 step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveMeta {
    /// ISO-8601 UTC `Z` from the `.lsv` mtime.
    pub saved_at: String,
    /// Already-hashed 64-hex key.
    pub campaign_key: String,
    pub campaign_key_source: CampaignKeySource,
    pub thumbnail: Option<ThumbnailRef>,
}

/// One class entry; `sub` is `None` when the raw `Sub` is absent or `""`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bg3Class {
    pub main: String,
    pub sub: Option<String>,
}

/// One party member, in save order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyMember {
    pub origin: String,
    pub race: String,
    pub level: u32,
    pub xp_level: u64,
    pub xp_total: u64,
    pub classes: Vec<Bg3Class>,
    pub is_summon: bool,
}

/// The `bg3-save/v1` payload. Field order matches contract §3.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bg3SavePayload {
    pub save_name: String,
    pub saved_at: String,
    pub campaign_key: String,
    pub campaign_key_source: CampaignKeySource,
    pub game_version: String,
    pub platform: String,
    pub current_level: String,
    pub difficulty: String,
    pub ruleset: Option<String>,
    pub party: Vec<PartyMember>,
    pub thumbnail: Option<ThumbnailRef>,
}

/// The companion envelope carrying a `bg3-save/v1` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bg3SaveMessage {
    pub envelope_version: u32,
    pub contract: String,
    pub contract_version: u32,
    pub captured_at: String,
    pub app_version: String,
    pub collector_version: String,
    pub payload: Bg3SavePayload,
}

impl Bg3SaveMessage {
    /// Wrap a payload; `collectorVersion` is this crate's version.
    pub fn new(
        payload: Bg3SavePayload,
        captured_at: impl Into<String>,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            envelope_version: ENVELOPE_VERSION,
            contract: CONTRACT.to_owned(),
            contract_version: CONTRACT_VERSION,
            captured_at: captured_at.into(),
            app_version: app_version.into(),
            collector_version: COLLECTOR_VERSION.to_owned(),
            payload,
        }
    }
}

// Raw SaveInfo.json shape. Unknown keys are ignored (Larian adds keys on
// patches); only the keys the normaliser reads are declared.
#[derive(Deserialize)]
struct RawSaveInfo {
    #[serde(rename = "Active Party")]
    active_party: RawParty,
    #[serde(rename = "Current Level")]
    current_level: String,
    #[serde(rename = "Difficulty")]
    difficulty: Vec<String>,
    #[serde(rename = "Game Version")]
    game_version: String,
    #[serde(rename = "Platform")]
    platform: String,
    #[serde(rename = "Save Name")]
    save_name: String,
}

#[derive(Deserialize)]
struct RawParty {
    #[serde(rename = "Characters")]
    characters: Vec<RawCharacter>,
}

#[derive(Deserialize)]
struct RawCharacter {
    #[serde(rename = "Classes")]
    classes: Option<Vec<RawClass>>,
    #[serde(rename = "Experience Points (Current level)")]
    xp_level: u64,
    #[serde(rename = "Experience Points (Total)")]
    xp_total: u64,
    #[serde(rename = "Level")]
    level: u32,
    #[serde(rename = "Origin")]
    origin: String,
    #[serde(rename = "Race")]
    race: String,
}

#[derive(Deserialize)]
struct RawClass {
    #[serde(rename = "Main")]
    main: String,
    #[serde(rename = "Sub")]
    sub: Option<String>,
}

/// Normalise a parsed `SaveInfo.json` (BOM already stripped) into the
/// `bg3-save/v1` payload (contract §3.2).
pub fn normalise_save_info(
    raw: &serde_json::Value,
    meta: &SaveMeta,
) -> Result<Bg3SavePayload, NormaliseError> {
    let raw = RawSaveInfo::deserialize(raw)?;
    let count = raw.difficulty.len();
    if !(1..=4).contains(&count) {
        return Err(NormaliseError::Difficulty(count));
    }
    if raw.active_party.characters.is_empty() {
        return Err(NormaliseError::EmptyParty);
    }
    let party = raw
        .active_party
        .characters
        .into_iter()
        .enumerate()
        .map(|(index, c)| member(index, c))
        .collect::<Result<Vec<_>, _>>()?;
    let mut difficulty = raw.difficulty.into_iter();
    Ok(Bg3SavePayload {
        save_name: raw.save_name,
        saved_at: meta.saved_at.clone(),
        campaign_key: meta.campaign_key.clone(),
        campaign_key_source: meta.campaign_key_source,
        game_version: raw.game_version,
        platform: raw.platform,
        current_level: raw.current_level,
        difficulty: difficulty.next().unwrap_or_default(),
        ruleset: difficulty.next(),
        party,
        thumbnail: meta.thumbnail.clone(),
    })
}

/// `Classes` absent ⇒ summon; present but empty ⇒ the save is skipped.
fn member(index: usize, c: RawCharacter) -> Result<PartyMember, NormaliseError> {
    let (classes, is_summon) = match c.classes {
        None => (Vec::new(), true),
        Some(list) if list.is_empty() => return Err(NormaliseError::EmptyClasses(index)),
        Some(list) => (list.into_iter().map(class).collect(), false),
    };
    Ok(PartyMember {
        origin: c.origin,
        race: c.race,
        level: c.level,
        xp_level: c.xp_level,
        xp_total: c.xp_total,
        classes,
        is_summon,
    })
}

fn class(raw: RawClass) -> Bg3Class {
    Bg3Class {
        main: raw.main,
        sub: raw.sub.filter(|s| !s.is_empty()),
    }
}
