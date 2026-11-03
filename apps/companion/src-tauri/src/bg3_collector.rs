//! The BG3 save collector (ROK-2061 lane 2): reads each `<SaveFolder>/*.lsv`
//! under the BG3 `Savegames/Story` folder through the `rl-collectors` LSPK
//! reader (ROK-1962) and emits one `bg3-save/v1` message per changed save.
//! Off by default; the user opts in per game.

use crate::bg3_campaigns::{campaign_name, now_unix, remember, select, unix, SaveCampaign};
use crate::collector::{
    captured_at, scope_key, Blob, Collector, PendingUpload, Scan, SentState, APP_VERSION,
};
use crate::manifest::BG3_SAVE;
use crate::scope::WatchScope;
use rl_collectors::{
    campaign_key_from_package, extract_save_info, extract_thumbnail_webp, normalise_save_info,
    read_lsf_attribute, read_lspk, sha256_hex, thumbnail_ref, Bg3SaveMessage, Package, SaveMeta,
    META_LSF_MAX_BYTES, META_LSF_NAME,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const CONTRACT: &str = rl_collectors::CONTRACT;
pub const CONTRACT_VERSION: u32 = rl_collectors::CONTRACT_VERSION;
/// Larger `.lsv` files are skipped (real saves run 10–100 MB).
pub const MAX_LSV_BYTES: u64 = 256 * 1024 * 1024;
/// The dedupe item inside one save file's scope: one message per file.
const ITEM: &str = "save";

/// What one readable save contributes: its campaign, and its upload when
/// the content changed since the last settled one.
pub type Found = (SaveCampaign, Option<PendingUpload>);

pub struct Bg3SaveCollector {
    roots: Vec<PathBuf>,
    scope: WatchScope,
    /// The user's per-campaign picks (`AppConfig::bg3_campaigns`).
    ticks: BTreeMap<String, bool>,
}

impl Bg3SaveCollector {
    /// `roots` are `Savegames/Story` folders; reads are confined to `scope`.
    pub fn scoped(roots: &[PathBuf], scope: WatchScope) -> Self {
        Self {
            roots: roots.to_vec(),
            scope,
            ticks: BTreeMap::new(),
        }
    }

    /// Only ticked campaigns upload ([`crate::bg3_campaigns::select`]).
    pub fn with_ticks(mut self, ticks: BTreeMap<String, bool>) -> Self {
        self.ticks = ticks;
        self
    }

    /// Every `<root>/<SaveFolder>/<file>` whose name matches the manifest.
    pub fn saves(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for root in &self.roots {
            for folder in self.scope.read_dir(root).unwrap_or_default() {
                if !self.scope.is_dir(&folder) {
                    continue;
                }
                let files = self.scope.read_dir(&folder).unwrap_or_default();
                out.extend(files.into_iter().filter(|f| {
                    f.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| BG3_SAVE.matches_file(n))
                        && self.scope.is_file(f)
                }));
            }
        }
        out
    }

    /// `saves` newest first by mtime (see [`newest_first`]).
    fn by_mtime(&self, saves: Vec<PathBuf>) -> Vec<PathBuf> {
        let stamped = saves
            .into_iter()
            .map(|p| {
                let at = self.scope.metadata(&p).and_then(|m| m.modified()).ok();
                (p, at)
            })
            .collect();
        newest_first(stamped)
    }

    fn scan_file(
        &self,
        path: &Path,
        origin: &str,
        sent: &SentState,
        scan: &mut Scan,
    ) -> Option<Found> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_default();
        let note = |scan: &mut Scan, text: String| {
            scan.notes.push(format!("{name}: {text}"));
            None
        };
        let meta = match self.scope.metadata(path) {
            Ok(m) if m.len() > MAX_LSV_BYTES => {
                return note(scan, "save is over 256 MiB; skipped".into())
            }
            Ok(m) => m,
            Err(e) => return note(scan, format!("cannot read save ({e})")),
        };
        let bytes = match self.scope.read(path) {
            Ok(b) => b,
            Err(e) => return note(scan, format!("cannot read save ({e})")),
        };
        scan.files += 1;
        let folder = path
            .parent()
            .and_then(Path::file_name)
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mtime = meta.modified().ok();
        let campaign = match campaign_of(&bytes, &folder, mtime) {
            Ok(c) => c,
            Err(e) => return note(scan, format!("not readable yet ({e})")),
        };
        let scope = scope_key(origin, &path.to_string_lossy());
        let sha = sha256_hex(&bytes);
        if sent.scope(&scope).get(ITEM) == Some(&sha) {
            return Some((campaign, None));
        }
        match build(&bytes, &folder, mtime) {
            Ok((body, blobs, label)) => Some((
                campaign,
                Some(PendingUpload {
                    contract: CONTRACT,
                    scope,
                    item: ITEM.to_owned(),
                    sha256: sha,
                    label,
                    body,
                    blobs,
                }),
            )),
            Err(e) => note(scan, format!("not readable yet ({e})")),
        }
    }
}

/// One save's campaign for the picker: its key (as [`build`] sends it), the
/// party leader's name from `meta.lsf` (else the save's name) and its mtime.
pub fn campaign_of(
    bytes: &[u8],
    folder: &str,
    mtime: Option<SystemTime>,
) -> Result<SaveCampaign, String> {
    let package = read_lspk(bytes).map_err(|e| e.to_string())?;
    let info = extract_save_info(&package).map_err(|e| e.to_string())?;
    let save_name = info
        .get("Save Name")
        .and_then(|v| v.as_str())
        .unwrap_or(folder);
    let (key, _) = campaign_key_from_package(&package, folder);
    let now = SystemTime::now();
    let at = mtime.filter(|m| *m <= now).unwrap_or(now);
    Ok(SaveCampaign {
        key,
        name: campaign_name(leader_name(&package).as_deref(), save_name),
        saved_unix: unix(at),
    })
}

/// `meta.lsf`'s `LeaderName`, when present and readable. Local display only.
fn leader_name(package: &Package<'_>) -> Option<String> {
    let entry = package.find(META_LSF_NAME)?;
    let bytes = package.read_entry_limited(entry, META_LSF_MAX_BYTES).ok()?;
    read_lsf_attribute(&bytes, "LeaderName").ok()?
}

/// Saves ordered newest first by mtime (unknown mtimes last, ties by path),
/// so a rate-limited batch spends its budget on the saves played last
/// (ROK-2068). The per-campaign filter ([`select`]) keeps this order.
pub fn newest_first(mut saves: Vec<(PathBuf, Option<SystemTime>)>) -> Vec<PathBuf> {
    saves.sort_by(|(pa, a), (pb, b)| b.cmp(a).then_with(|| pa.cmp(pb)));
    saves.into_iter().map(|(p, _)| p).collect()
}

/// Parse one `.lsv` into its message, thumbnail blob and window label. The
/// campaign key is the hash of `meta.lsf`'s GameID (`game-id`), else of the
/// save-folder name (`save-folder`); an unreadable `meta.lsf` never fails the
/// upload (ROK-2070). Neither raw value is ever sent (contract §3.3).
pub fn build(
    bytes: &[u8],
    folder: &str,
    mtime: Option<std::time::SystemTime>,
) -> Result<(serde_json::Value, Vec<Blob>, String), String> {
    let package = read_lspk(bytes).map_err(|e| e.to_string())?;
    let info = extract_save_info(&package).map_err(|e| e.to_string())?;
    // A missing or broken thumbnail is not an error (contract §7).
    let thumb = extract_thumbnail_webp(&package)
        .ok()
        .and_then(|webp| thumbnail_ref(&webp).ok().map(|r| (webp, r)));
    let (campaign_key, campaign_key_source) = campaign_key_from_package(&package, folder);
    let meta = SaveMeta {
        saved_at: captured_at(mtime),
        campaign_key,
        campaign_key_source,
        thumbnail: thumb.as_ref().map(|(_, r)| r.clone()),
    };
    let payload = normalise_save_info(&info, &meta).map_err(|e| e.to_string())?;
    let label = payload.save_name.clone();
    let message = Bg3SaveMessage::new(payload, captured_at(None), APP_VERSION);
    let body = serde_json::to_value(message).map_err(|e| e.to_string())?;
    let blobs = thumb
        .map(|(bytes, r)| Blob {
            sha256: r.sha256,
            content_type: r.content_type,
            bytes,
        })
        .into_iter()
        .collect();
    Ok((body, blobs, label))
}

impl Collector for Bg3SaveCollector {
    fn id(&self) -> &'static str {
        BG3_SAVE.id
    }

    fn watch_roots(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .filter(|r| self.scope.is_dir(r))
            .cloned()
            .collect()
    }

    fn scan(&self, origin: &str, sent: &SentState) -> Scan {
        let mut scan = Scan::default();
        let found: Vec<Found> = self
            .by_mtime(self.saves())
            .iter()
            .filter_map(|save| self.scan_file(save, origin, sent, &mut scan))
            .collect();
        let (groups, pending) = select(found, &self.ticks, now_unix());
        remember(groups);
        scan.pending = pending;
        scan
    }
}
