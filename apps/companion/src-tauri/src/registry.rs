//! Collector registry (ROK-2061): builds every compiled-in collector from its
//! manifest + the user's settings, each bound to its own [`WatchScope`]. The
//! runtime iterates this instead of naming a collector.

use crate::bg3_collector::Bg3SaveCollector;
use crate::bg3_screenshot::Bg3ScreenshotCollector;
use crate::collector::{Collector, Scan, SentState};
use crate::config::AppConfig;
use crate::manifest::{
    resolve_override, validate_all, CollectorManifest, ManifestError, Platform, BG3_SAVE,
    BG3_SCREENSHOT, WOW_LEDGERLINK,
};
use crate::scope::WatchScope;
use crate::wow_collector::LedgerLinkCollector;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Builds a collector from its resolved roots and the scope it must read in.
pub type Factory = fn(&[PathBuf], WatchScope, &AppConfig) -> Box<dyn Collector>;

/// A compiled-in collector: its manifest and how to build it.
#[derive(Clone, Copy)]
pub struct Builtin {
    pub manifest: &'static CollectorManifest,
    pub make: Factory,
}

fn make_wow(roots: &[PathBuf], scope: WatchScope, _: &AppConfig) -> Box<dyn Collector> {
    let root = roots.first().cloned().unwrap_or_default();
    Box::new(LedgerLinkCollector::scoped(root, scope))
}

fn make_bg3(roots: &[PathBuf], scope: WatchScope, cfg: &AppConfig) -> Box<dyn Collector> {
    Box::new(
        Bg3SaveCollector::scoped(roots, scope)
            .with_ticks(cfg.bg3_campaigns.clone())
            .remembering(&crate::bg3_campaigns::STORE),
    )
}

fn make_bg3_screenshot(
    roots: &[PathBuf],
    scope: WatchScope,
    cfg: &AppConfig,
) -> Box<dyn Collector> {
    Box::new(
        Bg3ScreenshotCollector::scoped(roots, scope)
            .with_opt_in(cfg.opted_in_at(BG3_SCREENSHOT.id))
            .with_hub(cfg.bg3_screenshot_hub)
            .reporting(&crate::bg3_screenshot::WAITING),
    )
}

/// Every collector this build ships.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        manifest: &WOW_LEDGERLINK,
        make: make_wow,
    },
    Builtin {
        manifest: &BG3_SAVE,
        make: make_bg3,
    },
    Builtin {
        manifest: &BG3_SCREENSHOT,
        make: make_bg3_screenshot,
    },
];

pub struct Entry {
    pub manifest: &'static CollectorManifest,
    pub enabled: bool,
    pub roots: Vec<PathBuf>,
    pub scope: WatchScope,
    pub collector: Box<dyn Collector>,
}

impl Entry {
    /// A changed path is this collector's business: it is switched on, the
    /// file name matches the manifest's `file_patterns` (the single source of
    /// truth for relevance) and the path is inside its scope. A disabled
    /// collector touches nothing on disk.
    pub fn wants(&self, path: &Path) -> bool {
        let named = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| self.manifest.matches_file(n));
        self.enabled && named && self.scope.contains(path)
    }

    /// Folders to watch: the collector's choice, clipped to its scope. None
    /// for a disabled collector (consent: no listing before opt-in).
    pub fn watch_roots(&self) -> Vec<PathBuf> {
        if !self.enabled {
            return Vec::new();
        }
        let mut roots = self.collector.watch_roots();
        roots.retain(|r| self.scope.contains(r));
        roots
    }

    /// Scan only when switched on: `None` for a disabled collector, which
    /// then reads and parses nothing (the spike's opt-in model).
    pub fn scan(&self, origin: &str, sent: &SentState) -> Option<Scan> {
        self.enabled.then(|| self.collector.scan(origin, sent))
    }
}

/// Read-only view of one collector, for the window and ROK-2055's dev bridge.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CollectorSnapshot {
    pub id: String,
    pub display_name: String,
    pub contract_id: String,
    pub contract_version: u32,
    pub enabled: bool,
    pub roots: Vec<String>,
    pub file_patterns: Vec<String>,
    pub settle_ms: u64,
}

pub struct CollectorRegistry {
    entries: Vec<Entry>,
    /// Settings that name no known collector (logged, never fatal).
    pub notes: Vec<String>,
}

impl CollectorRegistry {
    /// The shipped collectors for `cfg`, on this OS with the process env.
    pub fn from_config(cfg: &AppConfig) -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Self::build(BUILTINS, cfg, Platform::current(), &env)
            .expect("compiled-in manifests are valid (checked by tests)")
    }

    pub fn build(
        builtins: &[Builtin],
        cfg: &AppConfig,
        platform: Option<Platform>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ManifestError> {
        let manifests: Vec<&CollectorManifest> = builtins.iter().map(|b| b.manifest).collect();
        validate_all(&manifests)?;
        let mut notes = Vec::new();
        let entries = builtins
            .iter()
            .map(|b| {
                let m = b.manifest;
                let roots = resolve_roots(m, cfg, platform, env, &mut notes);
                let scope = WatchScope::new(&roots);
                Entry {
                    manifest: m,
                    enabled: cfg.collector_enabled(m.id, m.default_enabled),
                    collector: (b.make)(&roots, scope.clone(), cfg),
                    roots,
                    scope,
                }
            })
            .collect();
        let known = |id: &str| manifests.iter().any(|m| m.id == id);
        notes.extend(
            cfg.collectors_enabled
                .keys()
                .chain(cfg.collector_roots.keys())
                .filter(|id| !known(id))
                .map(|id| format!("Unknown collector {id:?} in settings; ignored")),
        );
        Ok(Self { entries, notes })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.manifest.id == id)
    }

    /// The enabled entries whose collector wants `path` ([`Entry::wants`]).
    pub fn wanting<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries.iter().filter(move |e| e.wants(path))
    }

    /// Every collector's watch roots (scope-clipped), sorted and deduped.
    pub fn watch_roots(&self) -> Vec<PathBuf> {
        let mut all: Vec<PathBuf> = self.entries.iter().flat_map(Entry::watch_roots).collect();
        all.sort();
        all.dedup();
        all
    }

    pub fn snapshot(&self) -> Vec<CollectorSnapshot> {
        self.entries
            .iter()
            .map(|e| CollectorSnapshot {
                id: e.manifest.id.to_owned(),
                display_name: e.manifest.display_name.to_owned(),
                contract_id: e.manifest.contract_id.to_owned(),
                contract_version: e.manifest.contract_version,
                enabled: e.enabled,
                roots: e.roots.iter().map(|r| r.display().to_string()).collect(),
                file_patterns: e
                    .manifest
                    .file_patterns
                    .iter()
                    .map(|p| (*p).to_owned())
                    .collect(),
                settle_ms: e.manifest.settle_ms,
            })
            .collect()
    }
}

/// The user's override if set, else this OS's defaults. Override entries that
/// are not absolute folders are dropped with a note; if none survive the
/// collector gets no roots (never a silent fall back to the defaults).
fn resolve_roots(
    m: &CollectorManifest,
    cfg: &AppConfig,
    platform: Option<Platform>,
    env: &dyn Fn(&str) -> Option<String>,
    notes: &mut Vec<String>,
) -> Vec<PathBuf> {
    let Some(raw) = cfg.roots_override(m.id) else {
        return m.default_roots_for(platform, env);
    };
    raw.iter()
        .filter_map(|r| match resolve_override(r, env) {
            Ok(path) => Some(path),
            Err(e) => {
                notes.push(format!("{}: {e}; ignored", m.display_name));
                None
            }
        })
        .collect()
}
