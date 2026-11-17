//! Hotkey validation and pure double-modifier tap edge detector (`SPEC-001-R2`, `SPEC-001-R3`).

use std::time::Duration;

use crate::{NativeError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Command,
    Control,
    Alt,
    Shift,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalModifier {
    pub modifier: Modifier,
    pub side: Side,
}

impl PhysicalModifier {
    fn bit(self) -> u8 {
        1 << (self.modifier as u8 * 2 + self.side as u8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEdge {
    ModifierDown(PhysicalModifier),
    ModifierUp(PhysicalModifier),
    OtherKeyDown,
    OtherKeyUp,
    Reset,
}

/// Pure edge detector for double-modifier hotkeys (`SPEC-001-R2`).
///
/// Owners must supply monotonic elapsed durations and call `reset()` when monitoring pauses.
#[derive(Debug, Default)]
pub struct DoubleTapDetector {
    pressed: u8,
    down: u8,
    modifier_down_time: Option<Duration>,
    non_modifier_pressed: bool,
    last_up: Option<(Modifier, Duration)>,
    last_observed: Option<Duration>,
}

impl DoubleTapDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn observe(&mut self, edge: InputEdge, now: Duration) -> Option<Modifier> {
        if edge == InputEdge::Reset {
            self.reset();
            return None;
        }
        if self.last_observed.is_some_and(|last| now < last) {
            self.reset();
            return None;
        }
        self.last_observed = Some(now);
        match edge {
            InputEdge::ModifierDown(key) => {
                if self.pressed & key.bit() == 0 {
                    self.down |= key.bit();
                }
                if self.modifier_down_time.is_none() {
                    self.modifier_down_time = Some(now);
                }
                self.pressed |= key.bit();
                None
            }
            InputEdge::ModifierUp(key) => {
                let was_pressed = self.pressed & key.bit() != 0;
                self.pressed &= !key.bit();
                let observed_down = self.down & key.bit() != 0;
                self.down &= !key.bit();
                let started = self.modifier_down_time.take();
                if !was_pressed || !observed_down {
                    self.down = 0;
                    self.last_up = None;
                    return None;
                }
                if started.is_some_and(|started| now - started > Duration::from_millis(300)) {
                    self.last_up = None;
                    return None;
                }
                if self.non_modifier_pressed {
                    self.non_modifier_pressed = false;
                    self.last_up = None;
                    return None;
                }
                if self.last_up.is_some_and(|(modifier, released)| {
                    modifier == key.modifier && now - released < Duration::from_millis(400)
                }) {
                    self.last_up = None;
                    return Some(key.modifier);
                }
                self.last_up = Some((key.modifier, now));
                None
            }
            InputEdge::OtherKeyDown => {
                self.non_modifier_pressed = true;
                self.last_up = None;
                None
            }
            InputEdge::OtherKeyUp => {
                self.non_modifier_pressed = false;
                self.modifier_down_time = None;
                None
            }
            InputEdge::Reset => unreachable!(),
        }
    }
}

/// Validates and normalizes a global hotkey label (`SPEC-001-R3`).
///
/// # Errors
///
/// Returns [`NativeError::HotkeyRejected`] if the label is empty, overly long, contains control
/// characters, has duplicate modifiers, or lacks a single primary key.
pub fn validate_hotkey_label(label: &str) -> Result<String> {
    if label.is_empty()
        || label.trim() != label
        || label.len() > 256
        || label.chars().any(char::is_control)
    {
        return Err(NativeError::HotkeyRejected(
            "invalid hotkey string format".into(),
        ));
    }

    let mut seen = 0_u8;
    let mut parts = Vec::new();
    let mut has_primary_key = false;

    for raw in label.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            return Err(NativeError::HotkeyRejected("empty hotkey token".into()));
        }
        let bit = modifier_bit(token);
        if bit != 0 {
            if seen & bit != 0 {
                return Err(NativeError::HotkeyRejected("duplicate modifier".into()));
            }
            seen |= bit;
            parts.push(normalize_modifier(token));
        } else {
            if has_primary_key {
                return Err(NativeError::HotkeyRejected(
                    "multiple primary keys in hotkey".into(),
                ));
            }
            has_primary_key = true;
            parts.push(token.to_ascii_uppercase());
        }
    }

    if !has_primary_key {
        return Err(NativeError::HotkeyRejected("missing primary key".into()));
    }

    let upper = label.to_ascii_uppercase();
    let is_fn_key = matches!(
        upper.as_str(),
        "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
    );
    if seen == 0 && !is_fn_key {
        return Err(NativeError::HotkeyRejected(
            "bare non-function key cannot be a global hotkey".into(),
        ));
    }

    Ok(parts.join("+"))
}

fn modifier_bit(token: &str) -> u8 {
    match token.to_ascii_uppercase().as_str() {
        "OPTION" | "ALT" => 1,
        "CONTROL" | "CTRL" => 2,
        "COMMAND" | "CMD" | "SUPER" => 4,
        "SHIFT" => 8,
        #[cfg(target_os = "macos")]
        "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL" | "CMDORCONTROL" => 4,
        #[cfg(not(target_os = "macos"))]
        "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL" | "CMDORCONTROL" => 2,
        _ => 0,
    }
}

fn normalize_modifier(token: &str) -> String {
    match token.to_ascii_uppercase().as_str() {
        "OPTION" | "ALT" => "Alt".into(),
        "CONTROL" | "CTRL" => "Control".into(),
        "COMMAND" | "CMD" | "SUPER" => "Super".into(),
        "SHIFT" => "Shift".into(),
        #[cfg(target_os = "macos")]
        "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL" | "CMDORCONTROL" => "Super".into(),
        #[cfg(not(target_os = "macos"))]
        "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL" | "CMDORCONTROL" => "Control".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modifier: Modifier, side: Side) -> PhysicalModifier {
        PhysicalModifier { modifier, side }
    }

    fn tap(
        d: &mut DoubleTapDetector,
        k: PhysicalModifier,
        down_ms: u64,
        up_ms: u64,
    ) -> Option<Modifier> {
        assert_eq!(
            d.observe(InputEdge::ModifierDown(k), Duration::from_millis(down_ms)),
            None
        );
        d.observe(InputEdge::ModifierUp(k), Duration::from_millis(up_ms))
    }

    #[test]
    fn spec_001_r2_double_tap_timing_and_rollback_safety() {
        let k = key(Modifier::Alt, Side::Left);
        let mut d = DoubleTapDetector::new();
        assert_eq!(tap(&mut d, k, 0, 300), None);
        assert_eq!(tap(&mut d, k, 301, 601), Some(Modifier::Alt));

        // Hold > 300ms breaks the tap.
        assert_eq!(tap(&mut d, k, 700, 1001), None);
        assert_eq!(tap(&mut d, k, 1005, 1020), None);

        // Inter-tap gap >= 400ms breaks the pair.
        let mut d2 = DoubleTapDetector::new();
        assert_eq!(tap(&mut d2, k, 0, 10), None);
        assert_eq!(tap(&mut d2, k, 390, 410), None);

        // Clock rollback resets history cleanly.
        let mut d3 = DoubleTapDetector::new();
        assert_eq!(tap(&mut d3, k, 100, 110), None);
        assert_eq!(
            d3.observe(InputEdge::ModifierDown(k), Duration::from_millis(50)),
            None
        );
        assert_eq!(
            d3.observe(InputEdge::ModifierUp(k), Duration::from_millis(60)),
            None
        );
    }

    #[test]
    fn spec_001_r3_validates_hotkey_labels() {
        assert_eq!(validate_hotkey_label("Option+Space").unwrap(), "Alt+SPACE");
        assert_eq!(validate_hotkey_label("F8").unwrap(), "F8");
        assert!(validate_hotkey_label("Space").is_err());
        assert!(validate_hotkey_label("Alt+Option+Space").is_err());
        assert!(validate_hotkey_label(" Alt+Space ").is_err());
    }
}
