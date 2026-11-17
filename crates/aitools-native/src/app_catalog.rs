//! Bounded cross-platform application catalog scanner (`SPEC-001-R4`).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{NativeError, Result};

pub const MAX_DEPTH: usize = 4;
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_APPS: usize = 1024;
pub const CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LauncherApp {
    pub name: String,
    pub path: String,
    pub aliases: Vec<String>,
}

/// Bounded cached scanner for installed desktop applications.
#[derive(Debug)]
pub struct AppCatalog {
    roots: Vec<PathBuf>,
    entries: Vec<LauncherApp>,
    scanned_at: Option<Instant>,
}

impl AppCatalog {
    pub fn with_roots(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            entries: Vec::new(),
            scanned_at: None,
        }
    }

    /// Creates a catalog configured with the host OS standard application directories.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Catalog`] on unsupported platforms.
    pub fn default_platform() -> Result<Self> {
        Ok(Self::with_roots(default_roots()?))
    }

    /// Clears the cached scan timestamp so the next [`Self::snapshot`] rescans disk.
    pub fn invalidate(&mut self) {
        self.scanned_at = None;
    }

    /// Returns cached application entries, rescanning if [`CACHE_TTL`] has elapsed.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Catalog`] if no configured root directory is readable.
    pub fn snapshot(&mut self, now: Instant) -> Result<&[LauncherApp]> {
        let fresh = self.scanned_at.is_some_and(|scanned_at| {
            now.checked_duration_since(scanned_at)
                .is_some_and(|elapsed| elapsed <= CACHE_TTL)
        });
        if !fresh {
            self.entries = scan_roots(&self.roots, MAX_DEPTH, MAX_ENTRIES, MAX_APPS)?;
            self.scanned_at = Some(now);
        }
        Ok(&self.entries)
    }
}

#[cfg(target_os = "macos")]
fn default_roots() -> Result<Vec<PathBuf>> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }
    Ok(roots)
}

#[cfg(target_os = "linux")]
fn default_roots() -> Result<Vec<PathBuf>> {
    let mut roots = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".local/share/applications"));
    }
    Ok(roots)
}

#[cfg(target_os = "windows")]
fn default_roots() -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    if let Some(pd) = std::env::var_os("ProgramData") {
        roots.push(PathBuf::from(pd).join("Microsoft/Windows/Start Menu/Programs"));
    }
    if let Some(ad) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(ad).join("Microsoft/Windows/Start Menu/Programs"));
    }
    Ok(roots)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn default_roots() -> Result<Vec<PathBuf>> {
    Err(NativeError::Catalog(
        "unsupported platform for app catalog".into(),
    ))
}

/// Scans `roots` for desktop applications within bounded depth, entry, and result caps.
///
/// # Errors
///
/// Returns [`NativeError::Catalog`] if at least one root exists and none of the roots are readable.
pub fn scan_roots(
    roots: &[PathBuf],
    max_depth: usize,
    max_entries: usize,
    max_apps: usize,
) -> Result<Vec<LauncherApp>> {
    let mut remaining_entries = max_entries;
    let mut readable_roots = 0usize;
    let mut seen = HashSet::new();
    let mut apps = Vec::new();

    for root in roots {
        if remaining_entries == 0 || apps.len() >= max_apps {
            break;
        }
        if !fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_dir()) {
            continue;
        }
        if scan_directory(
            root,
            0,
            max_depth,
            &mut remaining_entries,
            max_apps,
            &mut seen,
            &mut apps,
        ) {
            readable_roots += 1;
        }
    }

    if readable_roots == 0 {
        return Err(NativeError::Catalog(
            "no launcher application roots could be read".into(),
        ));
    }

    apps.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(apps)
}

fn scan_directory(
    directory: &Path,
    depth: usize,
    max_depth: usize,
    remaining_entries: &mut usize,
    max_apps: usize,
    seen: &mut HashSet<PathBuf>,
    apps: &mut Vec<LauncherApp>,
) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    for entry in entries {
        if *remaining_entries == 0 || apps.len() >= max_apps {
            break;
        }
        *remaining_entries -= 1;
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if path.extension().and_then(|e| e.to_str()) == Some("app") {
                if seen.insert(path.clone()) {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("App")
                        .to_string();
                    apps.push(LauncherApp {
                        name,
                        path: path.to_string_lossy().into_owned(),
                        aliases: Vec::new(),
                    });
                }
            } else if depth < max_depth {
                scan_directory(
                    &path,
                    depth + 1,
                    max_depth,
                    remaining_entries,
                    max_apps,
                    seen,
                    apps,
                );
            }
        } else if file_type.is_file() {
            let ext = path.extension().and_then(|e| e.to_str());
            if matches!(ext, Some("desktop" | "lnk" | "exe")) && seen.insert(path.clone()) {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("App")
                    .to_string();
                apps.push(LauncherApp {
                    name,
                    path: path.to_string_lossy().into_owned(),
                    aliases: Vec::new(),
                });
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn spec_001_r4_bounded_catalog_discovers_apps_and_respects_caps() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("Alpha.app")).unwrap();
        fs::create_dir(dir.path().join("Beta.app")).unwrap();
        fs::create_dir_all(dir.path().join("Sub/Gamma.app")).unwrap();

        let mut catalog = AppCatalog::with_roots(vec![dir.path().to_path_buf()]);
        let apps = catalog.snapshot(Instant::now()).unwrap();
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0].name, "Alpha");
        assert_eq!(apps[2].name, "Gamma");

        let capped = scan_roots(&[dir.path().to_path_buf()], MAX_DEPTH, MAX_ENTRIES, 2).unwrap();
        assert_eq!(capped.len(), 2);
    }
}
