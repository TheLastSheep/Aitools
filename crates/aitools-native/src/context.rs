//! Pre-focus context capture & clipboard text/image/file helpers (`SPEC-001-R1`, `SPEC-007-R2`, `SPEC-016-R3`).

use std::borrow::Cow;
use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use tracing::info_span;

use crate::{NativeError, Result};

/// Removable context chip attached when launcher promotes to Agent mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextChipKind {
    Selection,
    Clipboard,
    ActiveApp,
    ActivePath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextChip {
    pub kind: ContextChipKind,
    pub label: String,
    pub value: String,
}

/// Bounded snapshot captured prior to focusing the launcher window (`SPEC-001-R1`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreFocusContext {
    pub selection: Option<String>,
    pub clipboard: Option<String>,
    pub active_app: Option<String>,
    pub active_path: Option<String>,
}

impl PreFocusContext {
    /// Converts populated context fields into removable UI/agent chips (`SPEC-007-R2`).
    pub fn to_chips(&self) -> Vec<ContextChip> {
        let mut chips = Vec::new();
        if let Some(sel) = &self.selection {
            chips.push(ContextChip {
                kind: ContextChipKind::Selection,
                label: "Selection".to_string(),
                value: sel.clone(),
            });
        }
        if let Some(clip) = &self.clipboard {
            chips.push(ContextChip {
                kind: ContextChipKind::Clipboard,
                label: "Clipboard".to_string(),
                value: clip.clone(),
            });
        }
        if let Some(app) = &self.active_app {
            chips.push(ContextChip {
                kind: ContextChipKind::ActiveApp,
                label: "Active App".to_string(),
                value: app.clone(),
            });
        }
        if let Some(path) = &self.active_path {
            chips.push(ContextChip {
                kind: ContextChipKind::ActivePath,
                label: "Active Path".to_string(),
                value: path.clone(),
            });
        }
        chips
    }
}

/// File or directory metadata read from the system pasteboard (`SPEC-015-R6`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopiedFileEntry {
    /// Absolute filesystem path of the copied item.
    pub path: String,
    /// Basename of the file or directory.
    pub name: String,
    /// `true` when the target exists as a regular file.
    pub is_file: bool,
    /// `true` when the target exists as a directory.
    pub is_directory: bool,
}

/// Snapshot of the frontmost application and focused window before launcher wake (`SPEC-016-R3`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWindowSnapshot {
    /// Frontmost application display name.
    pub app_name: Option<String>,
    /// Frontmost application bundle identifier (e.g., `"com.apple.Terminal"`).
    pub bundle_id: Option<String>,
    /// Focused window title, when readable via OS accessibility.
    pub window_title: Option<String>,
    /// Active document path associated with the focused window, if any.
    pub active_path: Option<String>,
    /// Currently selected text in the focused control, if accessible without clobbering clipboard.
    pub selected_text: Option<String>,
}

/// Full live context payload captured on wake before focusing the launcher window (`SPEC-016-R3`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivePreFocusContext {
    /// Correlation trace identifier.
    pub trace_id: String,
    /// Frontmost application display name.
    pub app_name: Option<String>,
    /// Frontmost application bundle identifier.
    pub bundle_id: Option<String>,
    /// Focused window title.
    pub window_title: Option<String>,
    /// Selected text captured prior to wake.
    pub selected_text: Option<String>,
    /// Plain-text clipboard preview captured prior to wake.
    pub clipboard_preview: Option<String>,
    /// Active filesystem path associated with the focused context.
    pub active_path: Option<String>,
    /// Optional image data URL or handle if the clipboard contains an image.
    pub image_handle: Option<String>,
    /// Unix epoch timestamp in milliseconds when captured.
    pub captured_at_ms: u64,
}

impl LivePreFocusContext {
    /// Converts populated context fields into removable UI/agent chips.
    #[must_use]
    pub fn to_chips(&self) -> Vec<ContextChip> {
        PreFocusContext {
            selection: self.selected_text.clone(),
            clipboard: self.clipboard_preview.clone(),
            active_app: self.app_name.clone(),
            active_path: self.active_path.clone(),
        }
        .to_chips()
    }
}

/// Abstraction over OS clipboard so CI tests do not mutate the host pasteboard.
pub trait ClipboardProvider: Send + Sync {
    /// Reads plain UTF-8 text from the clipboard, returning `None` if empty or non-text.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Clipboard`] if accessing the system pasteboard fails.
    fn get_text(&self) -> Result<Option<String>>;

    /// Writes `text` to the clipboard.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Clipboard`] if writing to the system pasteboard fails.
    fn set_text(&self, text: &str) -> Result<()>;

    /// Reads the current clipboard image encoded as a PNG `data:image/png;base64,...` URI.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Clipboard`] if reading or encoding the image fails.
    fn get_image_data_url(&self) -> Result<Option<String>> {
        Ok(None)
    }

    /// Writes an image from a `data:` URI, local file path, or raw base64 string to the clipboard.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Clipboard`] if decoding the image or writing to the pasteboard fails.
    fn set_image_from_input(&self, input: &str) -> Result<()> {
        let _ = input;
        Err(NativeError::Clipboard(
            "image clipboard writes are not supported by this provider".into(),
        ))
    }

    /// Reads copied file entries from the OS pasteboard.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Clipboard`] if querying the file pasteboard fails.
    fn get_copied_files(&self) -> Result<Vec<CopiedFileEntry>> {
        Ok(Vec::new())
    }

    /// Writes file paths to the OS pasteboard.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Clipboard`] if any path is invalid or writing fails.
    fn set_copied_files(&self, paths: &[String]) -> Result<()> {
        let _ = paths;
        Err(NativeError::Clipboard(
            "file clipboard writes are not supported by this provider".into(),
        ))
    }
}

/// System clipboard backed by `arboard` and native OS pasteboard APIs.
pub struct SystemClipboard;

impl ClipboardProvider for SystemClipboard {
    fn get_text(&self) -> Result<Option<String>> {
        let mut board = arboard::Clipboard::new()
            .map_err(|e| NativeError::Clipboard(format!("clipboard init: {e}")))?;
        match board.get_text() {
            Ok(txt) if !txt.is_empty() => Ok(Some(txt)),
            Ok(_) => Ok(None),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(NativeError::Clipboard(format!("clipboard read: {e}"))),
        }
    }

    fn set_text(&self, text: &str) -> Result<()> {
        let mut board = arboard::Clipboard::new()
            .map_err(|e| NativeError::Clipboard(format!("clipboard init: {e}")))?;
        board
            .set_text(text.to_string())
            .map_err(|e| NativeError::Clipboard(format!("clipboard write: {e}")))
    }

    fn get_image_data_url(&self) -> Result<Option<String>> {
        let mut board = arboard::Clipboard::new()
            .map_err(|e| NativeError::Clipboard(format!("clipboard init: {e}")))?;
        match board.get_image() {
            Ok(img) => encode_rgba_to_png_data_url(img.width, img.height, &img.bytes).map(Some),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(NativeError::Clipboard(format!("clipboard image read: {e}"))),
        }
    }

    fn set_image_from_input(&self, input: &str) -> Result<()> {
        let (width, height, rgba) = decode_image_input_rgba(input)?;
        let mut board = arboard::Clipboard::new()
            .map_err(|e| NativeError::Clipboard(format!("clipboard init: {e}")))?;
        board
            .set_image(arboard::ImageData {
                width,
                height,
                bytes: Cow::Borrowed(&rgba),
            })
            .map_err(|e| NativeError::Clipboard(format!("clipboard image write: {e}")))
    }

    fn get_copied_files(&self) -> Result<Vec<CopiedFileEntry>> {
        macos_get_copied_files()
    }

    fn set_copied_files(&self, paths: &[String]) -> Result<()> {
        macos_set_copied_files(paths)
    }
}

/// Deterministic in-memory clipboard for headless tests.
#[derive(Default)]
pub struct MemoryClipboard {
    text: Mutex<Option<String>>,
    image_data_url: Mutex<Option<String>>,
    files: Mutex<Vec<CopiedFileEntry>>,
}

impl MemoryClipboard {
    /// Creates an in-memory clipboard seeded with `text`.
    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            text: Mutex::new(Some(text.into())),
            image_data_url: Mutex::new(None),
            files: Mutex::new(Vec::new()),
        }
    }
}

impl ClipboardProvider for MemoryClipboard {
    fn get_text(&self) -> Result<Option<String>> {
        Ok(self
            .text
            .lock()
            .expect("memory clipboard mutex poisoned")
            .clone())
    }

    fn set_text(&self, text: &str) -> Result<()> {
        *self.text.lock().expect("memory clipboard mutex poisoned") = Some(text.to_string());
        Ok(())
    }

    fn get_image_data_url(&self) -> Result<Option<String>> {
        Ok(self
            .image_data_url
            .lock()
            .expect("memory clipboard image mutex poisoned")
            .clone())
    }

    fn set_image_from_input(&self, input: &str) -> Result<()> {
        let (width, height, rgba) = decode_image_input_rgba(input)?;
        let png_url = encode_rgba_to_png_data_url(width, height, &rgba)?;
        *self
            .image_data_url
            .lock()
            .expect("memory clipboard image mutex poisoned") = Some(png_url);
        Ok(())
    }

    fn get_copied_files(&self) -> Result<Vec<CopiedFileEntry>> {
        Ok(self
            .files
            .lock()
            .expect("memory clipboard files mutex poisoned")
            .clone())
    }

    fn set_copied_files(&self, paths: &[String]) -> Result<()> {
        let entries = paths
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|raw| build_copied_file_entry(raw))
            .collect();
        *self
            .files
            .lock()
            .expect("memory clipboard files mutex poisoned") = entries;
        Ok(())
    }
}

const MAX_CLIPBOARD_CAPTURE_BYTES: usize = 64 * 1024;

/// Decodes a `data:` URI, image file path, or raw base64 payload into `(width, height, rgba8)` pixels.
///
/// # Errors
///
/// Returns [`NativeError::Clipboard`] if the input is empty or cannot be decoded as a supported image.
pub fn decode_image_input_rgba(input: &str) -> Result<(usize, usize, Vec<u8>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(NativeError::Clipboard(
            "image input must not be empty".into(),
        ));
    }

    let raw_bytes = if let Some(rest) = trimmed.strip_prefix("data:") {
        let (_, b64) = rest
            .split_once(";base64,")
            .ok_or_else(|| NativeError::Clipboard("data URL must be base64-encoded".into()))?;
        BASE64_STANDARD
            .decode(b64.trim())
            .map_err(|e| NativeError::Clipboard(format!("invalid base64 image data: {e}")))?
    } else if Path::new(trimmed).is_file() {
        std::fs::read(trimmed)
            .map_err(|e| NativeError::Clipboard(format!("failed to read image '{trimmed}': {e}")))?
    } else {
        BASE64_STANDARD
            .decode(trimmed)
            .map_err(|e| NativeError::Clipboard(format!("invalid image path or base64: {e}")))?
    };

    let dyn_img: DynamicImage = image::load_from_memory(&raw_bytes)
        .map_err(|e| NativeError::Clipboard(format!("unsupported image format: {e}")))?;
    let rgba = dyn_img.to_rgba8();
    Ok((
        rgba.width() as usize,
        rgba.height() as usize,
        rgba.into_raw(),
    ))
}

/// Encodes raw RGBA8 pixels into a `data:image/png;base64,...` URI.
///
/// # Errors
///
/// Returns [`NativeError::Clipboard`] if dimensions do not match `bytes.len()` or PNG encoding fails.
pub fn encode_rgba_to_png_data_url(width: usize, height: usize, bytes: &[u8]) -> Result<String> {
    let w = u32::try_from(width)
        .map_err(|_| NativeError::Clipboard("image width exceeds u32".into()))?;
    let h = u32::try_from(height)
        .map_err(|_| NativeError::Clipboard("image height exceeds u32".into()))?;
    let img = RgbaImage::from_raw(w, h, bytes.to_vec())
        .ok_or_else(|| NativeError::Clipboard("invalid RGBA image buffer dimensions".into()))?;
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img)
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|e| NativeError::Clipboard(format!("PNG encode failed: {e}")))?;
    Ok(format!(
        "data:image/png;base64,{}",
        BASE64_STANDARD.encode(out.into_inner())
    ))
}

fn build_copied_file_entry(raw: &str) -> CopiedFileEntry {
    let path = Path::new(raw);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(raw)
        .to_string();
    let meta = std::fs::metadata(path).ok();
    let is_directory = meta.as_ref().is_some_and(|m| m.is_dir());
    let is_file = meta.as_ref().is_some_and(|m| m.is_file());
    CopiedFileEntry {
        path: raw.to_string(),
        name,
        is_file,
        is_directory,
    }
}

/// Captures the frontmost application, window title, active path, and selected text (`SPEC-016-R3`).
#[must_use]
pub fn capture_frontmost_snapshot() -> ActiveWindowSnapshot {
    #[cfg(target_os = "macos")]
    {
        macos_capture_frontmost_snapshot()
    }
    #[cfg(not(target_os = "macos"))]
    {
        ActiveWindowSnapshot::default()
    }
}

/// Captures pre-focus context inside a `pre_focus_capture` tracing span (`SPEC-000`).
pub fn capture_pre_focus_context(
    trace_id: &str,
    clipboard: &dyn ClipboardProvider,
    selection: Option<String>,
    active_app: Option<String>,
    active_path: Option<String>,
) -> PreFocusContext {
    let _span = info_span!("pre_focus_capture", trace_id = %trace_id).entered();
    let clip = clipboard
        .get_text()
        .ok()
        .flatten()
        .map(|s| truncate_utf8(&s, MAX_CLIPBOARD_CAPTURE_BYTES));

    PreFocusContext {
        selection: selection.map(|s| truncate_utf8(&s, MAX_CLIPBOARD_CAPTURE_BYTES)),
        clipboard: clip,
        active_app,
        active_path,
    }
}

/// Captures live desktop context (frontmost app, AX selection when trusted, clipboard) before focusing (`SPEC-016-R3`).
pub fn capture_live_pre_focus_context(
    trace_id: &str,
    clipboard: &dyn ClipboardProvider,
) -> LivePreFocusContext {
    let _span = info_span!("pre_focus_capture", trace_id = %trace_id).entered();
    let front = capture_frontmost_snapshot();
    let clipboard_preview = clipboard
        .get_text()
        .ok()
        .flatten()
        .map(|s| truncate_utf8(&s, MAX_CLIPBOARD_CAPTURE_BYTES));
    let captured_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));

    LivePreFocusContext {
        trace_id: trace_id.to_string(),
        app_name: front.app_name,
        bundle_id: front.bundle_id,
        window_title: front.window_title,
        selected_text: front
            .selected_text
            .map(|s| truncate_utf8(&s, MAX_CLIPBOARD_CAPTURE_BYTES)),
        clipboard_preview,
        active_path: front.active_path,
        image_handle: None,
        captured_at_ms,
    }
}

#[cfg(target_os = "macos")]
fn macos_capture_frontmost_snapshot() -> ActiveWindowSnapshot {
    use objc2_app_kit::NSWorkspace;

    let mut snap = ActiveWindowSnapshot::default();
    objc2::rc::autoreleasepool(|_| {
        let ws = NSWorkspace::sharedWorkspace();
        if let Some(app) = ws.frontmostApplication() {
            let bundle_id = app.bundleIdentifier().map(|s| s.to_string());
            let app_name = app.localizedName().map(|s| s.to_string());
            if bundle_id.as_deref() == Some("dev.aitools.desktop") {
                return;
            }
            snap.bundle_id = bundle_id;
            snap.app_name = app_name;
        }
    });

    if snap.app_name.is_some() || snap.bundle_id.is_some() {
        macos_enrich_ax_context(&mut snap);
    }
    snap
}

#[cfg(target_os = "macos")]
fn macos_enrich_ax_context(snap: &mut ActiveWindowSnapshot) {
    use std::ffi::c_void;

    use core_foundation::base::{CFGetTypeID, CFRelease, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateSystemWide() -> *const c_void;
        fn AXUIElementCopyAttributeValue(
            element: *const c_void,
            attribute: CFStringRef,
            value: *mut *const c_void,
        ) -> i32;
    }

    struct CfOwned(*const c_void);
    impl Drop for CfOwned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `CfOwned` exclusively owns a +1 retained CoreFoundation object from Create/Copy.
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn copy_ax_attr(element: *const c_void, attr_name: &str) -> Option<CfOwned> {
        let attr = CFString::new(attr_name);
        let mut out: *const c_void = std::ptr::null();
        // SAFETY: `element` is a valid non-null AXUIElementRef and `attr` is a valid CFStringRef.
        let rc =
            unsafe { AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut out) };
        if rc == 0 && !out.is_null() {
            Some(CfOwned(out))
        } else {
            None
        }
    }

    fn cf_as_string(obj: &CfOwned) -> Option<String> {
        if obj.0.is_null() {
            return None;
        }
        // SAFETY: `obj.0` is a valid non-null CFTypeRef owned by `CfOwned`.
        let tid = unsafe { CFGetTypeID(obj.0) };
        if tid != CFString::type_id() {
            return None;
        }
        // SAFETY: Type ID matches `CFString::type_id()`.
        let s = unsafe { CFString::wrap_under_get_rule(obj.0.cast()) };
        let val = s.to_string();
        (!val.is_empty()).then_some(val)
    }

    // Never prompt or synthesize keystrokes when Accessibility permission is absent.
    // SAFETY: Pure query with no parameters.
    if !unsafe { AXIsProcessTrusted() } {
        return;
    }

    // SAFETY: `AXUIElementCreateSystemWide` returns a +1 retained AXUIElementRef or null.
    let system_raw = unsafe { AXUIElementCreateSystemWide() };
    if system_raw.is_null() {
        return;
    }
    let system = CfOwned(system_raw);

    if let Some(focused) = copy_ax_attr(system.0, "AXFocusedUIElement") {
        if let Some(sel) = copy_ax_attr(focused.0, "AXSelectedText") {
            snap.selected_text = cf_as_string(&sel);
        }
    }

    if let Some(window) = copy_ax_attr(system.0, "AXFocusedWindow") {
        if let Some(title) = copy_ax_attr(window.0, "AXTitle") {
            snap.window_title = cf_as_string(&title);
        }
        if let Some(doc) = copy_ax_attr(window.0, "AXDocument") {
            if let Some(raw_doc) = cf_as_string(&doc) {
                let cleaned = raw_doc
                    .strip_prefix("file://")
                    .unwrap_or(&raw_doc)
                    .to_string();
                snap.active_path = Some(cleaned);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_get_copied_files() -> Result<Vec<CopiedFileEntry>> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL};
    use objc2_foundation::{NSString, NSURL};

    let mut out = Vec::new();
    objc2::rc::autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        let Some(items) = pb.pasteboardItems() else {
            return;
        };
        for item in items {
            // SAFETY: `NSPasteboardTypeFileURL` is a static immutable AppKit pasteboard type constant.
            let file_url_type = unsafe { NSPasteboardTypeFileURL };
            if let Some(url_str) = item.stringForType(file_url_type) {
                if let Some(url) = NSURL::URLWithString(&url_str) {
                    if let Some(path) = url.path() {
                        out.push(build_copied_file_entry(&path.to_string()));
                    }
                }
            } else {
                let fallback = NSString::from_str("public.file-url");
                if let Some(url_str) = item.stringForType(&fallback) {
                    if let Some(url) = NSURL::URLWithString(&url_str) {
                        if let Some(path) = url.path() {
                            out.push(build_copied_file_entry(&path.to_string()));
                        }
                    }
                }
            }
        }
    });
    Ok(out)
}

#[cfg(not(target_os = "macos"))]
fn macos_get_copied_files() -> Result<Vec<CopiedFileEntry>> {
    Ok(Vec::new())
}

#[cfg(target_os = "macos")]
fn macos_set_copied_files(paths: &[String]) -> Result<()> {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardWriting,
    };
    use objc2_foundation::{NSArray, NSString, NSURL};

    if paths.is_empty() {
        return Err(NativeError::Clipboard(
            "at least one file path is required".into(),
        ));
    }

    let mut ok = false;
    objc2::rc::autoreleasepool(|_| {
        let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
            Vec::with_capacity(paths.len());
        // SAFETY: `NSPasteboardTypeFileURL` is a static AppKit constant.
        let file_url_type = unsafe { NSPasteboardTypeFileURL };
        for raw in paths {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let ns_path = NSString::from_str(trimmed);
            let url = NSURL::fileURLWithPath(&ns_path);
            let Some(url_abs) = url.absoluteString() else {
                continue;
            };
            let item = NSPasteboardItem::new();
            item.setString_forType(&url_abs, file_url_type);
            items.push(ProtocolObject::from_retained(item));
        }
        if items.is_empty() {
            return;
        }
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let arr = NSArray::from_retained_slice(&items);
        ok = pb.writeObjects(&arr);
    });

    if ok {
        Ok(())
    } else {
        Err(NativeError::Clipboard(
            "failed to write file URLs to NSPasteboard".into(),
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn macos_set_copied_files(_paths: &[String]) -> Result<()> {
    Err(NativeError::Clipboard(
        "file pasteboard writes are currently supported on macOS only".into(),
    ))
}

fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    input[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_001_r1_captures_pre_focus_context_and_chips() {
        let board = MemoryClipboard::with_text("copied snippet");
        let ctx = capture_pre_focus_context(
            "wake-1",
            &board,
            Some("selected text".into()),
            Some("Finder".into()),
            Some("/Users/demo".into()),
        );
        let chips = ctx.to_chips();
        assert_eq!(chips.len(), 4);
        assert_eq!(chips[0].kind, ContextChipKind::Selection);
        assert_eq!(chips[1].value, "copied snippet");
        assert_eq!(chips[2].value, "Finder");
        assert_eq!(chips[3].value, "/Users/demo");
    }

    #[test]
    fn spec_016_r3_live_context_and_clipboard_media_helpers() {
        let board = MemoryClipboard::with_text("hello clip");
        let live = capture_live_pre_focus_context("wake-42", &board);
        assert_eq!(live.trace_id, "wake-42");
        assert_eq!(live.clipboard_preview.as_deref(), Some("hello clip"));
        assert!(live.captured_at_ms > 0);

        // 1x1 red PNG pixel roundtrip through MemoryClipboard image helper
        let png_data_url = encode_rgba_to_png_data_url(1, 1, &[255, 0, 0, 255]).unwrap();
        assert!(png_data_url.starts_with("data:image/png;base64,"));
        board.set_image_from_input(&png_data_url).unwrap();
        let read_back = board.get_image_data_url().unwrap().unwrap();
        assert!(read_back.starts_with("data:image/png;base64,"));

        // File clipboard roundtrip
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmp.path().to_string_lossy().into_owned();
        board
            .set_copied_files(std::slice::from_ref(&tmp_path))
            .unwrap();
        let files = board.get_copied_files().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, tmp_path);
        assert!(files[0].is_file);
    }
}
