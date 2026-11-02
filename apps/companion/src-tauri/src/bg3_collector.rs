//! The BG3 save collector (ROK-2061 lane 2): reads each `<SaveFolder>/*.lsv`
//! under the BG3 `Savegames/Story` folder through the `rl-collectors` LSPK
//! reader (ROK-1962) and emits one `bg3-save/v1` message per changed save.
//! Off by default; the user opts in per game.

use crate::collector::{
    captured_at, scope_key, Blob, Collector, PendingUpload, Scan, SentState, APP_VERSION,
};
use crate::manifest::BG3_SAVE;
use crate::scope::WatchScope;
use rl_collectors::{
    campaign_key_from_save_folder, extract_save_info, extract_thumbnail_webp, normalise_save_info,
    read_lspk, sha256_hex, thumbnail_ref, Bg3SaveMessage, CampaignKeySource, SaveMeta,
};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const CONTRACT: &str = rl_collectors::CONTRACT;
pub const CONTRACT_VERSION: u32 = rl_collectors::CONTRACT_VERSION;
/// Larger `.lsv` files are skipped (real saves run 10–100 MB).
pub const MAX_LSV_BYTES: u64 = 256 * 1024 * 1024;
/// The dedupe item inside one save file's scope: one message per file.
const ITEM: &str = "save";

pub struct Bg3SaveCollector {
    roots: Vec<PathBuf>,
    scope: WatchScope,
}

impl Bg3SaveCollector {
    /// `roots` are `Savegames/Story` folders; reads are confined to `scope`.
    pub fn scoped(roots: &[PathBuf], scope: WatchScope) -> Self {
        Self {
            roots: roots.to_vec(),
            scope,
        }
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

    fn scan_file(&self, path: &Path, origin: &str, sent: &SentState, scan: &mut Scan) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_default();
        let meta = match self.scope.metadata(path) {
            Ok(m) if m.len() > MAX_LSV_BYTES => {
                return scan
                    .notes
                    .push(format!("{name}: save is over 256 MiB; skipped"))
            }
            Ok(m) => m,
            Err(e) => return scan.notes.push(format!("{name}: cannot read save ({e})")),
        };
        let bytes = match self.scope.read(path) {
            Ok(b) => b,
            Err(e) => return scan.notes.push(format!("{name}: cannot read save ({e})")),
        };
        scan.files += 1;
        let scope = scope_key(origin, &path.to_string_lossy());
        let sha = sha256_hex(&bytes);
        // Dedupe on the whole file before parsing: an unchanged save costs a hash.
        if sent.scope(&scope).get(ITEM) == Some(&sha) {
            return;
        }
        let folder = path
            .parent()
            .and_then(Path::file_name)
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        match build(&bytes, &folder, meta.modified().ok()) {
            Ok((body, blobs, label)) => scan.pending.push(PendingUpload {
                contract: CONTRACT,
                scope,
                item: ITEM.to_owned(),
                sha256: sha,
                label,
                body,
                blobs,
            }),
            Err(e) => scan.notes.push(format!("{name}: not readable yet ({e})")),
        }
    }
}

/// Saves ordered newest first by mtime (unknown mtimes last, ties by path),
/// so a rate-limited batch spends its budget on the saves played last
/// (ROK-2068). A selection filter (per campaign, max age) runs after this.
pub fn newest_first(mut saves: Vec<(PathBuf, Option<SystemTime>)>) -> Vec<PathBuf> {
    saves.sort_by(|(pa, a), (pb, b)| b.cmp(a).then_with(|| pa.cmp(pb)));
    saves.into_iter().map(|(p, _)| p).collect()
}

/// Parse one `.lsv` into its message, thumbnail blob and window label. The
/// raw folder name only feeds the hash (contract §3.3); it is never sent.
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
    let meta = SaveMeta {
        saved_at: captured_at(mtime),
        campaign_key: campaign_key_from_save_folder(folder),
        campaign_key_source: CampaignKeySource::SaveFolder,
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
        for save in self.by_mtime(self.saves()) {
            self.scan_file(&save, origin, sent, &mut scan);
        }
        scan
    }
}
