//! Per-collector watch scope (ROK-2061): a collector may only ever read under
//! its manifest's resolved roots. Every filesystem read a collector makes goes
//! through [`WatchScope`], which canonicalises the path (resolving `..` and
//! every symlink) and requires it to sit under a canonical root — so a symlink
//! that points outside the game folder is refused, not followed.
//!
//! Threat model: the local user. A path swapped between [`WatchScope::check`]
//! and the read that follows it (a TOCTOU window) is not defended against;
//! anyone who can do that can already read the file themselves.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The canonical roots one collector may read under. Roots that do not exist
/// yet are kept only in their raw form and grant nothing until they appear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchScope {
    roots: Vec<PathBuf>,
}

/// A read outside the scope.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScopeError {
    #[error("{0} is outside this collector's folders")]
    Outside(PathBuf),
    #[error("{0} cannot be resolved")]
    Unresolvable(PathBuf),
}

impl From<ScopeError> for io::Error {
    fn from(e: ScopeError) -> Self {
        io::Error::new(io::ErrorKind::PermissionDenied, e.to_string())
    }
}

impl WatchScope {
    pub fn new<P: AsRef<Path>>(roots: &[P]) -> Self {
        Self {
            roots: roots.iter().map(|r| r.as_ref().to_path_buf()).collect(),
        }
    }

    /// The roots as configured (not canonicalised).
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Resolve `path` and return its canonical form if it lies under a root.
    /// Roots are canonicalised on every call, so a root created after start-up
    /// is honoured and a root swapped for a symlink is re-resolved.
    pub fn check(&self, path: &Path) -> Result<PathBuf, ScopeError> {
        let real =
            fs::canonicalize(path).map_err(|_| ScopeError::Unresolvable(path.to_path_buf()))?;
        let inside = self
            .roots
            .iter()
            .filter_map(|r| fs::canonicalize(r).ok())
            .any(|root| real.starts_with(&root));
        if inside {
            Ok(real)
        } else {
            Err(ScopeError::Outside(path.to_path_buf()))
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.check(path).is_ok()
    }

    /// `fs::metadata` of the resolved path, only inside the scope.
    pub fn metadata(&self, path: &Path) -> io::Result<fs::Metadata> {
        fs::metadata(self.check(path)?)
    }

    /// `fs::read` of the resolved path, only inside the scope.
    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(self.check(path)?)
    }

    /// Entries of a directory inside the scope, joined onto `dir` as given (so
    /// callers can strip their own root prefix). Each entry is itself checked
    /// before use by the next scoped call.
    pub fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        // List the checked canonical path, not `dir` again: a component
        // swapped for a symlink after the check cannot redirect the listing.
        let real = self.check(dir)?;
        let mut out: Vec<PathBuf> = fs::read_dir(&real)?
            .flatten()
            .map(|e| dir.join(e.file_name()))
            .collect();
        out.sort();
        Ok(out)
    }

    /// Whether `path` is a directory inside the scope.
    pub fn is_dir(&self, path: &Path) -> bool {
        self.metadata(path).is_ok_and(|m| m.is_dir())
    }

    /// Whether `path` is a file inside the scope.
    pub fn is_file(&self, path: &Path) -> bool {
        self.metadata(path).is_ok_and(|m| m.is_file())
    }
}
