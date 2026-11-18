//! Bounded cross-platform application catalog scanner (`SPEC-001-R4`).

use std::collections::HashSet;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{NativeError, Result};

/// Maximum directory recursion depth when scanning application roots.
pub const MAX_DEPTH: usize = 4;
/// Maximum filesystem entries visited in a single catalog scan.
pub const MAX_ENTRIES: usize = 4096;
/// Maximum number of discovered application entries returned by [`scan_roots`].
pub const MAX_APPS: usize = 1024;
/// Duration for which a cached [`AppCatalog`] scan remains fresh.
pub const CACHE_TTL: Duration = Duration::from_secs(30);

static TEMP_ICON_SEQ: AtomicU64 = AtomicU64::new(0);

/// Discovered desktop application entry (`SPEC-001-R4`, `SPEC-016-R1`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LauncherApp {
    /// Primary display name (`.app` stem or localized bundle name).
    pub name: String,
    /// Full filesystem path to the `.app` bundle or launcher entry.
    pub path: String,
    /// Search aliases (e.g. `CFBundleDisplayName`, `CFBundleName`, `CFBundleExecutable`, bundle ID).
    pub aliases: Vec<String>,
    /// Optional platform bundle identifier (`CFBundleIdentifier` on macOS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// Optional filesystem path to the resolved application icon source (`.icns`, `.png`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<String>,
}

/// Bounded cached scanner for installed desktop applications.
#[derive(Debug)]
pub struct AppCatalog {
    roots: Vec<PathBuf>,
    entries: Vec<LauncherApp>,
    scanned_at: Option<Instant>,
}

impl AppCatalog {
    /// Creates a catalog configured to scan the provided application `roots`.
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
        PathBuf::from("/System/Cryptexes/App/System/Applications"),
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
                    apps.push(build_macos_app_entry(&path));
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
                    bundle_id: None,
                    icon_path: None,
                });
            }
        }
    }
    true
}

fn build_macos_app_entry(path: &Path) -> LauncherApp {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("App")
        .to_string();
    let mut aliases = Vec::new();
    let mut bundle_id = None;
    let info_plist = path.join("Contents").join("Info.plist");
    let plist_dict = if info_plist.is_file() {
        match plist::Value::from_file(&info_plist) {
            Ok(plist::Value::Dictionary(dict)) => Some(dict),
            _ => None,
        }
    } else {
        None
    };

    if let Some(ref dict) = plist_dict {
        if let Some(id) = dict.get("CFBundleIdentifier").and_then(|v| v.as_string()) {
            let trimmed = id.trim();
            if !trimmed.is_empty() {
                bundle_id = Some(trimmed.to_string());
                aliases.push(trimmed.to_string());
            }
        }
        for key in ["CFBundleDisplayName", "CFBundleName", "CFBundleExecutable"] {
            if let Some(val) = dict.get(key).and_then(|v| v.as_string()) {
                let trimmed = val.trim();
                if !trimmed.is_empty()
                    && !trimmed.eq_ignore_ascii_case(&stem)
                    && !aliases.iter().any(|a| a.eq_ignore_ascii_case(trimmed))
                {
                    aliases.push(trimmed.to_string());
                }
            }
        }
    }

    let icon_path = resolve_macos_app_icon(path, plist_dict.as_ref(), &stem);
    LauncherApp {
        name: stem,
        path: path.to_string_lossy().into_owned(),
        aliases,
        bundle_id,
        icon_path,
    }
}

fn resolve_macos_app_icon(
    app_path: &Path,
    dict: Option<&plist::Dictionary>,
    app_stem: &str,
) -> Option<String> {
    let resources_dir = app_path.join("Contents").join("Resources");
    if !resources_dir.is_dir() {
        return None;
    }

    if let Some(d) = dict {
        for key in ["CFBundleIconFile", "CFBundleIconName"] {
            if let Some(raw_name) = d.get(key).and_then(|v| v.as_string()) {
                if let Some(found) = find_icon_by_name(&resources_dir, raw_name) {
                    return Some(found.to_string_lossy().into_owned());
                }
            }
        }
        if let Some(files) = d
            .get("CFBundleIcons")
            .and_then(|v| v.as_dictionary())
            .and_then(|v| v.get("CFBundlePrimaryIcon"))
            .and_then(|v| v.as_dictionary())
            .and_then(|v| v.get("CFBundleIconFiles"))
            .and_then(|v| v.as_array())
        {
            for item in files {
                if let Some(raw_name) = item.as_string() {
                    if let Some(found) = find_icon_by_name(&resources_dir, raw_name) {
                        return Some(found.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }

    find_fallback_resources_icon(&resources_dir, app_stem).map(|p| p.to_string_lossy().into_owned())
}

fn find_icon_by_name(resources_dir: &Path, raw_name: &str) -> Option<PathBuf> {
    let trimmed = raw_name.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let candidates = [
        trimmed.to_string(),
        format!("{trimmed}.icns"),
        format!("{trimmed}.png"),
    ];
    let canon_root = fs::canonicalize(resources_dir).ok()?;
    for rel in &candidates {
        let joined = resources_dir.join(rel);
        if joined.is_file() {
            if let Ok(canon) = fs::canonicalize(&joined) {
                if canon.starts_with(&canon_root) {
                    return Some(canon);
                }
            }
        }
    }

    // Case-insensitive fallback for case-sensitive volumes.
    let entries = fs::read_dir(resources_dir).ok()?;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if candidates.iter().any(|c| name_str.eq_ignore_ascii_case(c)) {
            return Some(entry.path());
        }
    }
    None
}

fn is_auxiliary_icon_stem(stem_lower: &str) -> bool {
    [
        "tray",
        "menubar",
        "status",
        "sidebar",
        "alert",
        "badge",
        "cursor",
        "document",
        "docicon",
        "toolbar",
        "placeholder",
        "background",
        "overlay",
    ]
    .iter()
    .any(|needle| stem_lower.contains(needle))
}

fn find_fallback_resources_icon(resources_dir: &Path, app_stem: &str) -> Option<PathBuf> {
    let app_stem_lower = app_stem.to_ascii_lowercase();
    let entries = fs::read_dir(resources_dir).ok()?;
    let mut ranked: Vec<(u8, String, PathBuf)> = Vec::new();

    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(ext) = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        if ext != "icns" && ext != "png" {
            continue;
        }
        let fname_lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let stem_lower = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let is_template = fname_lower.contains("template");
        let is_aux = is_auxiliary_icon_stem(&stem_lower);
        let is_primary_name = matches!(
            stem_lower.as_str(),
            "appicon" | "appiconupdated" | "applicationicon" | "icon" | "app"
        ) || (!app_stem_lower.is_empty() && stem_lower == app_stem_lower);

        let tier = match (ext.as_str(), is_template, is_aux, is_primary_name) {
            ("icns", false, false, true) => 0,
            ("icns", false, false, false) => 1,
            ("png", false, false, true) => 2,
            ("png", false, false, false) => 3,
            ("icns", false, true, _) => 4,
            ("png", false, true, _) => 5,
            ("icns", true, _, _) => 6,
            _ => 7,
        };
        ranked.push((tier, fname_lower, path));
    }

    ranked
        .into_iter()
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, _, path)| path)
}

fn is_png_file(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic).is_ok() && magic == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
}

/// Renders an application or plugin icon asset into a `128×128` max-dimension PNG at `cache_png_path`.
///
/// Uses `/usr/bin/sips -s format png -Z 128` on macOS (fast-copying existing PNGs already `<= 128px`)
/// and writes atomically via a same-directory temporary file rename. On non-macOS hosts, resizes
/// supported raster formats via `image` and returns a clear [`NativeError::Catalog`] error for `.icns`.
///
/// # Errors
///
/// Returns [`NativeError::Catalog`] if `source_path` is missing, conversion fails, or writing
/// `cache_png_path` fails.
pub fn render_app_icon_png_128(source_path: &Path, cache_png_path: &Path) -> Result<()> {
    if !source_path.is_file() {
        return Err(NativeError::Catalog(format!(
            "icon source file does not exist: {}",
            source_path.display()
        )));
    }

    if let Some(parent) = cache_png_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                NativeError::Catalog(format!(
                    "failed to create icon cache directory '{}': {e}",
                    parent.display()
                ))
            })?;
        }
    }

    let seq = TEMP_ICON_SEQ.fetch_add(1, Ordering::Relaxed);
    let stem = cache_png_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("icon");
    let tmp_name = format!(".{stem}.{}.{seq}.tmp.png", std::process::id());
    let tmp_path = cache_png_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from(&tmp_name), |p| p.join(&tmp_name));

    // Fast path: source is already a valid PNG with both dimensions <= 128px.
    if is_png_file(source_path) {
        if let Ok((w, h)) = image::image_dimensions(source_path) {
            if w > 0 && h > 0 && w <= 128 && h <= 128 {
                if let Err(err) = fs::copy(source_path, &tmp_path) {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(NativeError::Catalog(format!(
                        "failed to copy PNG icon '{}': {err}",
                        source_path.display()
                    )));
                }
                return fs::rename(&tmp_path, cache_png_path).map_err(|e| {
                    let _ = fs::remove_file(&tmp_path);
                    NativeError::Catalog(format!(
                        "failed to finalize icon '{}': {e}",
                        cache_png_path.display()
                    ))
                });
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/usr/bin/sips")
            .args(["-s", "format", "png", "-Z", "128"])
            .arg(source_path)
            .arg("--out")
            .arg(&tmp_path)
            .output()
            .map_err(|e| NativeError::Catalog(format!("failed to invoke /usr/bin/sips: {e}")))?;

        if !output.status.success() || !is_png_file(&tmp_path) {
            let _ = fs::remove_file(&tmp_path);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NativeError::Catalog(format!(
                "sips failed rendering '{}' to 128px PNG (status {}): {}",
                source_path.display(),
                output.status,
                stderr.trim()
            )));
        }

        fs::rename(&tmp_path, cache_png_path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            NativeError::Catalog(format!(
                "failed to finalize rendered icon '{}': {e}",
                cache_png_path.display()
            ))
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        match image::open(source_path) {
            Ok(img) => {
                let resized = if img.width() > 128 || img.height() > 128 {
                    img.resize(128, 128, image::imageops::FilterType::Lanczos3)
                } else {
                    img
                };
                if let Err(err) = resized.save_with_format(&tmp_path, image::ImageFormat::Png) {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(NativeError::Catalog(format!(
                        "failed to encode PNG icon '{}': {err}",
                        source_path.display()
                    )));
                }
                fs::rename(&tmp_path, cache_png_path).map_err(|e| {
                    let _ = fs::remove_file(&tmp_path);
                    NativeError::Catalog(format!(
                        "failed to finalize rendered icon '{}': {e}",
                        cache_png_path.display()
                    ))
                })
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp_path);
                Err(NativeError::Catalog(format!(
                    "cannot render '{}' on non-macOS host (macOS sips required for .icns): {err}",
                    source_path.display()
                )))
            }
        }
    }
}

/// Launches a desktop application by filesystem path or platform application name (`SPEC-016-R2`).
///
/// # Errors
///
/// Returns [`NativeError::Catalog`] if `target` is empty, contains NUL bytes, or the OS
/// launcher command fails to spawn.
pub fn launch_application(target: &str) -> Result<()> {
    let trimmed = target.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(NativeError::Catalog(
            "application target path or name must not be empty".into(),
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        if Path::new(trimmed).exists() {
            cmd.arg(trimmed);
        } else {
            cmd.args(["-a", trimmed]);
        }
        cmd.spawn()
            .map_err(|e| NativeError::Catalog(format!("failed to launch '{trimmed}': {e}")))?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let program = if trimmed.ends_with(".desktop") {
            "gio"
        } else {
            "xdg-open"
        };
        let mut cmd = Command::new(program);
        if program == "gio" {
            cmd.args(["launch", trimmed]);
        } else {
            cmd.arg(trimmed);
        }
        cmd.spawn()
            .map_err(|e| NativeError::Catalog(format!("failed to launch '{trimmed}': {e}")))?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", trimmed])
            .spawn()
            .map_err(|e| NativeError::Catalog(format!("failed to launch '{trimmed}': {e}")))?;
        return Ok(());
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(NativeError::Catalog(
            "unsupported platform for launching applications".into(),
        ))
    }
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

    #[test]
    fn spec_016_r1_reads_macos_info_plist_aliases_and_bundle_id() {
        let dir = TempDir::new().unwrap();
        let contents = dir.path().join("Code.app/Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.microsoft.VSCode</string>
  <key>CFBundleDisplayName</key><string>Visual Studio Code</string>
</dict></plist>"#,
        )
        .unwrap();

        let mut catalog = AppCatalog::with_roots(vec![dir.path().to_path_buf()]);
        let apps = catalog.snapshot(Instant::now()).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Code");
        assert_eq!(apps[0].bundle_id.as_deref(), Some("com.microsoft.VSCode"));
        assert!(apps[0].aliases.iter().any(|a| a == "Visual Studio Code"));
        assert!(apps[0].icon_path.is_none());
    }

    #[test]
    fn spec_016_r1_resolves_app_icon_and_prioritizes_nontemplate() {
        let dir = TempDir::new().unwrap();
        // 1. CFBundleIconFile without extension -> resolves <name>.icns
        let app_a = dir.path().join("SafariLike.app");
        fs::create_dir_all(app_a.join("Contents/Resources")).unwrap();
        fs::write(
            app_a.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.example.safari</string>
  <key>CFBundleIconFile</key><string>AppIconUpdated</string>
</dict></plist>"#,
        )
        .unwrap();
        fs::write(
            app_a.join("Contents/Resources/AppIconUpdated.icns"),
            b"icns-bytes",
        )
        .unwrap();

        // 2. No plist keys -> fallback picks non-template app icon over *Template.png
        let app_b = dir.path().join("Helper.app");
        fs::create_dir_all(app_b.join("Contents/Resources")).unwrap();
        fs::write(
            app_b.join("Contents/Resources/AAA_TrayTemplate.png"),
            b"template-png",
        )
        .unwrap();
        fs::write(app_b.join("Contents/Resources/icon.png"), b"real-app-icon").unwrap();

        let mut catalog = AppCatalog::with_roots(vec![dir.path().to_path_buf()]);
        let apps = catalog.snapshot(Instant::now()).unwrap();
        assert_eq!(apps.len(), 2);
        let helper = apps.iter().find(|a| a.name == "Helper").unwrap();
        assert!(
            helper
                .icon_path
                .as_deref()
                .is_some_and(|p| p.ends_with("icon.png")),
            "expected non-template icon.png, got {:?}",
            helper.icon_path
        );
        let safari_like = apps.iter().find(|a| a.name == "SafariLike").unwrap();
        assert!(
            safari_like
                .icon_path
                .as_deref()
                .is_some_and(|p| p.ends_with("AppIconUpdated.icns")),
            "expected AppIconUpdated.icns, got {:?}",
            safari_like.icon_path
        );
    }

    #[test]
    fn spec_016_r1_render_app_icon_png_128_produces_valid_png() {
        let dir = TempDir::new().unwrap();
        // Fast-copy <= 128px PNG across all platforms.
        let small_src = dir.path().join("small.png");
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([16, 185, 129, 255]));
        image::DynamicImage::ImageRgba8(img)
            .save_with_format(&small_src, image::ImageFormat::Png)
            .unwrap();
        let small_out = dir.path().join("cache/small.png");
        render_app_icon_png_128(&small_src, &small_out).unwrap();
        let bytes = fs::read(&small_out).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(image::image_dimensions(&small_out).unwrap(), (64, 64));

        // Downscale > 128px PNG cleanly.
        let big_src = dir.path().join("big.png");
        let big_img = image::RgbaImage::from_pixel(256, 256, image::Rgba([59, 130, 246, 255]));
        image::DynamicImage::ImageRgba8(big_img)
            .save_with_format(&big_src, image::ImageFormat::Png)
            .unwrap();
        let big_out = dir.path().join("cache/big.png");
        render_app_icon_png_128(&big_src, &big_out).unwrap();
        let (w, h) = image::image_dimensions(&big_out).unwrap();
        assert!(w <= 128 && h <= 128 && w > 0 && h > 0);

        // Real macOS .icns rendering via /usr/bin/sips when ZTools.app is present.
        let ztools_icns = Path::new("/Applications/ZTools.app/Contents/Resources/icon.icns");
        if ztools_icns.is_file() {
            let out_icns = dir.path().join("cache/ztools.png");
            render_app_icon_png_128(ztools_icns, &out_icns).unwrap();
            let icns_bytes = fs::read(&out_icns).unwrap();
            assert!(icns_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
            let (iw, ih) = image::image_dimensions(&out_icns).unwrap();
            assert!(iw > 0 && iw <= 128 && ih > 0 && ih <= 128);
        }

        // Real macOS Cryptexes Safari.app catalog discovery & icon resolution.
        if Path::new("/System/Cryptexes/App/System/Applications/Safari.app").is_dir() {
            let mut default_cat = AppCatalog::default_platform().unwrap();
            let apps = default_cat.snapshot(Instant::now()).unwrap();
            let safari = apps.iter().find(|a| a.name == "Safari").expect(
                "Safari.app under /System/Cryptexes/App/System/Applications must be indexed",
            );
            assert!(safari.icon_path.is_some(), "Safari must resolve icon_path");
        }
    }
}
