//! Collector registry (ROK-2061): builds every compiled-in collector from its
//! manifest + the user's settings, each bound to its own [`WatchScope`]. The
//! runtime iterates this instead of naming a collector.

use crate::collector::Collector;
use crate::config::AppConfig;
use crate::manifest::{validate_all, CollectorManifest, ManifestError, Platform, WOW_LEDGERLINK};
use crate::scope::WatchScope;
use crate::wow_collector::LedgerLinkCollector;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Builds a collector from its resolved roots and the scope it must read in.
pub type Factory = fn(&[PathBuf], WatchScope) -> Box<dyn Collector>;

/// A compiled-in collector: its manifest and how to build it.
#[derive(Clone, Copy)]
pub struct Builtin {
    pub manifest: &'static CollectorManifest,
    pub make: Factory,
}

fn make_wow(roots: &[PathBuf], scope: WatchScope) -> Box<dyn Collector> {
    let root = roots.first().cloned().unwrap_or_default();
    Box::new(LedgerLinkCollector::scoped(root, scope))
}

/// Every collector this build ships. Lane 2 (ROK-2061) adds `bg3-save`.
pub const BUILTINS: &[Builtin] = &[Builtin {
    manifest: &WOW_LEDGERLINK,
    make: make_wow,
}];

pub struct Entry {
    pub manifest: &'static CollectorManifest,
    pub enabled: bool,
    pub roots: Vec<PathBuf>,
    pub scope: WatchScope,
    pub collector: Box<dyn Collector>,
}

impl Entry {
    /// A changed path is this collector's business: inside its scope and a
    /// file it reads.
    pub fn wants(&self, path: &Path) -> bool {
        self.collector.is_relevant(path) && self.scope.contains(path)
    }

    /// Folders to watch: the collector's choice, clipped to its scope.
    pub fn watch_roots(&self) -> Vec<PathBuf> {
        let mut roots = self.collector.watch_roots();
        roots.retain(|r| self.scope.contains(r));
        roots
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
        let entries = builtins
            .iter()
            .map(|b| {
                let m = b.manifest;
                let roots = resolve_roots(m, cfg, platform, env);
                let scope = WatchScope::new(&roots);
                Entry {
                    manifest: m,
                    enabled: cfg.collector_enabled(m.id, m.default_enabled),
                    collector: (b.make)(&roots, scope.clone()),
                    roots,
                    scope,
                }
            })
            .collect();
        let known = |id: &str| manifests.iter().any(|m| m.id == id);
        let notes = cfg
            .collectors_enabled
            .keys()
            .chain(cfg.collector_roots.keys())
            .filter(|id| !known(id))
            .map(|id| format!("Unknown collector {id:?} in settings; ignored"))
            .collect();
        Ok(Self { entries, notes })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.manifest.id == id)
    }

    /// The entries whose collector wants `path` (see [`Entry::wants`]).
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

/// The user's override if set, else this OS's defaults.
fn resolve_roots(
    m: &CollectorManifest,
    cfg: &AppConfig,
    platform: Option<Platform>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<PathBuf> {
    match cfg.roots_override(m.id) {
        Some(roots) => roots.into_iter().map(PathBuf::from).collect(),
        None => m.default_roots_for(platform, env),
    }
}
