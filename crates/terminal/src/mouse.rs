//! Encoding of negotiated xterm mouse events; coordinates are zero-based cells.
use crate::grid::{MouseProtocolEncoding, MouseProtocolMode};

/// Mouse action requested by an interactive terminal application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    /// Button press (left/middle/right = 0/1/2).
    Press(u8),
    /// Matching button release.
    Release(u8),
    /// Motion, optionally while holding a button.
    Motion(Option<u8>),
    /// Wheel step (positive scrolls up).
    Wheel(bool),
}
/// xterm modifier bits, independent of the UI toolkit.
#[derive(Clone, Copy, Debug, Default)]
pub struct MouseModifiers {
    /// Shift key.
    pub shift: bool,
    /// Alt/Option key.
    pub alt: bool,
    /// Control key.
    pub ctrl: bool,
}
/// Encodes a negotiated mouse event, or ignores an unsupported/out-of-range action.
#[must_use]
pub fn encode(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    action: MouseAction,
    row: u16,
    col: u16,
    modifiers: MouseModifiers,
) -> Option<Vec<u8>> {
    if mode == MouseProtocolMode::None {
        return None;
    }
    let release = matches!(action, MouseAction::Release(_));
    let mut button = match action {
        MouseAction::Press(button) if button <= 2 => button,
        MouseAction::Release(button) if button <= 2 && mode != MouseProtocolMode::Press => button,
        MouseAction::Motion(button)
            if mode == MouseProtocolMode::AnyMotion
                || (mode == MouseProtocolMode::ButtonMotion && button.is_some()) =>
        {
            32 + button.unwrap_or(3).min(3)
        }
        MouseAction::Wheel(up) => {
            if up {
                64
            } else {
                65
            }
        }
        _ => return None,
    };
    if mode != MouseProtocolMode::Press {
        button += u8::from(modifiers.shift) * 4
            + u8::from(modifiers.alt) * 8
            + u8::from(modifiers.ctrl) * 16;
    }
    let row = u32::from(row) + 1;
    let col = u32::from(col) + 1;
    if encoding == MouseProtocolEncoding::Sgr {
        return Some(
            format!(
                "\x1b[<{button};{col};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes(),
        );
    }
    if release {
        button = (button & 0x1c) | 0x03;
    }
    let values = [u32::from(button) + 32, col + 32, row + 32];
    let mut bytes = b"\x1b[M".to_vec();
    for value in values {
        if encoding == MouseProtocolEncoding::Utf8 {
            if value > 2047 {
                return None;
            }
            let mut encoded = [0; 4];
            bytes.extend_from_slice(char::from_u32(value)?.encode_utf8(&mut encoded).as_bytes());
        } else {
            // X10 coordinates cannot represent columns/rows above 223.
            bytes.push(u8::try_from(value).ok()?);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn press_release_motion_wheel_modifiers_and_encoding_bounds() {
        let sgr = |action| {
            encode(
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr,
                action,
                5,
                300,
                MouseModifiers::default(),
            )
        };
        assert_eq!(sgr(MouseAction::Press(0)), Some(b"\x1b[<0;301;6M".to_vec()));
        assert_eq!(
            sgr(MouseAction::Release(0)),
            Some(b"\x1b[<0;301;6m".to_vec())
        );
        assert_eq!(
            sgr(MouseAction::Motion(Some(0))),
            Some(b"\x1b[<32;301;6M".to_vec())
        );
        assert_eq!(
            sgr(MouseAction::Wheel(true)),
            Some(b"\x1b[<64;301;6M".to_vec())
        );
        assert!(sgr(MouseAction::Motion(None)).is_none());
        assert!(
            encode(
                MouseProtocolMode::None,
                MouseProtocolEncoding::Sgr,
                MouseAction::Press(0),
                0,
                0,
                MouseModifiers::default()
            )
            .is_none()
        );
        assert!(
            encode(
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Default,
                MouseAction::Release(0),
                0,
                0,
                MouseModifiers::default()
            )
            .is_none()
        );
        assert!(
            encode(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
                MouseAction::Press(0),
                0,
                223,
                MouseModifiers::default()
            )
            .is_none()
        );
        assert_eq!(
            encode(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
                MouseAction::Release(0),
                0,
                0,
                MouseModifiers::default()
            ),
            Some(b"\x1b[M#!!".to_vec())
        );
        assert_eq!(
            encode(
                MouseProtocolMode::AnyMotion,
                MouseProtocolEncoding::Utf8,
                MouseAction::Motion(None),
                0,
                300,
                MouseModifiers {
                    ctrl: true,
                    ..MouseModifiers::default()
                }
            ),
            Some("\x1b[MSō!".as_bytes().to_vec())
        );
    }
}
