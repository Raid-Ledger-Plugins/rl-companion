//! First-upload preview (ROK-2061 lane 3): before a collector whose manifest
//! sets `confirm_first_upload` sends anything, the runtime holds what it
//! found and the window shows a local summary of the newest item. Upload
//! records consent (later items go without asking); Skip marks the held items
//! as handled so they are never sent, and the next new item asks again.

use crate::collector::PendingUpload;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PartyLine {
    pub name: String,
    pub summary: String,
}

/// What the window shows before the first upload. Built from the parsed
/// message only — nothing here is fetched or sent.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDto {
    pub collector_id: String,
    pub save_name: String,
    /// Short form of the hashed campaign key the message carries.
    pub campaign: String,
    pub location: String,
    pub difficulty: String,
    pub saved_at: String,
    pub party: Vec<PartyLine>,
    /// `data:` URL of the thumbnail that would be uploaded, if any.
    pub thumbnail: Option<String>,
    /// How many items wait behind this preview (this one included).
    pub waiting: usize,
}

/// The held batch: the preview plus the dedupe keys Skip records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub preview: PreviewDto,
    pub keys: Vec<(String, String, String)>,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn party_line(m: &Value) -> PartyLine {
    let classes: Vec<String> = m
        .get("classes")
        .and_then(Value::as_array)
        .map(|cs| {
            cs.iter()
                .map(|c| match c.get("sub").and_then(Value::as_str) {
                    Some(sub) => format!("{} ({sub})", text(c, "main")),
                    None => text(c, "main"),
                })
                .collect()
        })
        .unwrap_or_default();
    let level = m.get("level").and_then(Value::as_u64).unwrap_or(0);
    PartyLine {
        name: text(m, "origin"),
        summary: format!("Level {level} {} {}", text(m, "race"), classes.join(" / "))
            .trim_end()
            .to_owned(),
    }
}

/// Hold `pending` behind a preview of its newest item (by `savedAt`).
pub fn hold(collector_id: &str, pending: &[PendingUpload]) -> Option<Held> {
    let newest = pending
        .iter()
        .max_by_key(|p| text(&p.body["payload"], "savedAt"))?;
    let payload = &newest.body["payload"];
    let party = payload
        .get("party")
        .and_then(Value::as_array)
        .map(|ms| ms.iter().map(party_line).collect())
        .unwrap_or_default();
    let campaign: String = text(payload, "campaignKey").chars().take(8).collect();
    let preview = PreviewDto {
        collector_id: collector_id.to_owned(),
        save_name: text(payload, "saveName"),
        campaign,
        location: text(payload, "currentLevel"),
        difficulty: text(payload, "difficulty"),
        saved_at: text(payload, "savedAt"),
        party,
        thumbnail: newest
            .blobs
            .first()
            .map(|b| format!("data:{};base64,{}", b.content_type, base64(&b.bytes))),
        waiting: pending.len(),
    };
    let keys = pending
        .iter()
        .map(|p| (p.scope.clone(), p.item.clone(), p.sha256.clone()))
        .collect();
    Some(Held { preview, keys })
}

/// Standard base64 with padding (no crate needed for one thumbnail).
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ABC[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}
