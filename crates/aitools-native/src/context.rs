//! Pre-focus context capture (`SPEC-001-R1`, `SPEC-007-R2`).

use std::sync::Mutex;

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
}

/// System clipboard backed by `arboard`.
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
}

/// Deterministic in-memory clipboard for headless tests.
#[derive(Default)]
pub struct MemoryClipboard {
    text: Mutex<Option<String>>,
}

impl MemoryClipboard {
    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            text: Mutex::new(Some(text.into())),
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
}

const MAX_CLIPBOARD_CAPTURE_BYTES: usize = 64 * 1024;

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
}
