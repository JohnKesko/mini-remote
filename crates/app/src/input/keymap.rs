//! Translation tables: physical keys to QEMU/XT scancodes, and keys or
//! characters to X11 keysyms (what VNC servers expect).

use egui_winit::winit::keyboard::{Key, KeyCode, KeyLocation, NamedKey, PhysicalKey};

/// X11 keysyms used by name elsewhere in the app.
pub mod xk {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const ESCAPE: u32 = 0xff1b;
    pub const DELETE: u32 = 0xffff;
    pub const PRINT: u32 = 0xff61;
    pub const F1: u32 = 0xffbe;
    pub const F4: u32 = 0xffc1;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const CAPS_LOCK: u32 = 0xffe5;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const SUPER_L: u32 = 0xffeb;
    pub const SUPER_R: u32 = 0xffec;
    /// AltGr on European keyboards.
    pub const ISO_LEVEL3_SHIFT: u32 = 0xfe03;
}

/// Scancodes used by name elsewhere in the app.
pub mod sc {
    pub const ESCAPE: u32 = 0x01;
    pub const TAB: u32 = 0x0f;
    pub const CONTROL_L: u32 = 0x1d;
    pub const CONTROL_R: u32 = 0xe01d;
    pub const SHIFT_L: u32 = 0x2a;
    pub const SHIFT_R: u32 = 0x36;
    pub const ALT_L: u32 = 0x38;
    pub const ALT_R: u32 = 0xe038;
    pub const CAPS_LOCK: u32 = 0x3a;
    pub const F1: u32 = 0x3b;
    pub const F4: u32 = 0x3e;
    pub const DELETE: u32 = 0xe053;
    pub const PRINT: u32 = 0xe037;
    pub const SUPER_L: u32 = 0xe05b;
    pub const SUPER_R: u32 = 0xe05c;
}

/// QEMU/XT scancode for a physical key (extended keys as 0xE0xx), or 0 when unknown.
pub fn scancode(key: PhysicalKey) -> u32 {
    match key {
        PhysicalKey::Code(code) => scancode_for_code(code),
        PhysicalKey::Unidentified(_) => 0,
    }
}

pub fn scancode_for_code(code: KeyCode) -> u32 {
    use KeyCode::*;
    match code {
        Escape => 0x01,
        Digit1 => 0x02,
        Digit2 => 0x03,
        Digit3 => 0x04,
        Digit4 => 0x05,
        Digit5 => 0x06,
        Digit6 => 0x07,
        Digit7 => 0x08,
        Digit8 => 0x09,
        Digit9 => 0x0a,
        Digit0 => 0x0b,
        Minus => 0x0c,
        Equal => 0x0d,
        Backspace => 0x0e,
        Tab => 0x0f,
        KeyQ => 0x10,
        KeyW => 0x11,
        KeyE => 0x12,
        KeyR => 0x13,
        KeyT => 0x14,
        KeyY => 0x15,
        KeyU => 0x16,
        KeyI => 0x17,
        KeyO => 0x18,
        KeyP => 0x19,
        BracketLeft => 0x1a,
        BracketRight => 0x1b,
        Enter => 0x1c,
        ControlLeft => 0x1d,
        KeyA => 0x1e,
        KeyS => 0x1f,
        KeyD => 0x20,
        KeyF => 0x21,
        KeyG => 0x22,
        KeyH => 0x23,
        KeyJ => 0x24,
        KeyK => 0x25,
        KeyL => 0x26,
        Semicolon => 0x27,
        Quote => 0x28,
        Backquote => 0x29,
        ShiftLeft => 0x2a,
        Backslash => 0x2b,
        KeyZ => 0x2c,
        KeyX => 0x2d,
        KeyC => 0x2e,
        KeyV => 0x2f,
        KeyB => 0x30,
        KeyN => 0x31,
        KeyM => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        ShiftRight => 0x36,
        NumpadMultiply => 0x37,
        AltLeft => 0x38,
        Space => 0x39,
        CapsLock => 0x3a,
        F1 => 0x3b,
        F2 => 0x3c,
        F3 => 0x3d,
        F4 => 0x3e,
        F5 => 0x3f,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        NumLock | NumpadClear => 0x45,
        ScrollLock => 0x46,
        Numpad7 => 0x47,
        Numpad8 => 0x48,
        Numpad9 => 0x49,
        NumpadSubtract => 0x4a,
        Numpad4 => 0x4b,
        Numpad5 => 0x4c,
        Numpad6 => 0x4d,
        NumpadAdd => 0x4e,
        Numpad1 => 0x4f,
        Numpad2 => 0x50,
        Numpad3 => 0x51,
        Numpad0 => 0x52,
        NumpadDecimal => 0x53,
        // The extra key next to left Shift on ISO (European) keyboards.
        IntlBackslash => 0x56,
        F11 => 0x57,
        F12 => 0x58,
        NumpadEqual => 0x59,
        F13 => 0x5d,
        F14 => 0x5e,
        F15 => 0x5f,
        F16 => 0x55,
        F17 => 0xe003,
        F18 => 0xe077,
        F19 => 0xe004,
        F20 => 0x5a,
        F21 => 0x74,
        F22 => 0xe079,
        F23 => 0x6d,
        F24 => 0x6f,
        KanaMode => 0x70,
        Lang2 => 0x71,
        Lang1 => 0x72,
        IntlRo => 0x73,
        Convert => 0x79,
        NonConvert => 0x7b,
        IntlYen => 0x7d,
        NumpadComma => 0x7e,
        MediaTrackPrevious => 0xe010,
        MediaTrackNext => 0xe019,
        NumpadEnter => 0xe01c,
        ControlRight => 0xe01d,
        AudioVolumeMute => 0xe020,
        LaunchApp2 => 0xe021,
        MediaPlayPause => 0xe022,
        MediaStop => 0xe024,
        AudioVolumeDown => 0xe02e,
        AudioVolumeUp => 0xe030,
        BrowserHome => 0xe032,
        NumpadDivide => 0xe035,
        PrintScreen => 0xe037,
        AltRight => 0xe038,
        Pause => 0xe046,
        Home => 0xe047,
        ArrowUp => 0xe048,
        PageUp => 0xe049,
        ArrowLeft => 0xe04b,
        ArrowRight => 0xe04d,
        End => 0xe04f,
        ArrowDown => 0xe050,
        PageDown => 0xe051,
        Insert => 0xe052,
        Delete => 0xe053,
        SuperLeft => 0xe05b,
        SuperRight => 0xe05c,
        ContextMenu => 0xe05d,
        Power => 0xe05e,
        Sleep => 0xe05f,
        WakeUp => 0xe063,
        BrowserSearch => 0xe065,
        BrowserFavorites => 0xe066,
        BrowserRefresh => 0xe067,
        BrowserStop => 0xe068,
        BrowserForward => 0xe069,
        BrowserBack => 0xe06a,
        LaunchApp1 => 0xe06b,
        LaunchMail => 0xe06c,
        MediaSelect => 0xe06d,
        _ => 0,
    }
}

/// Keysym for a key that is not a plain character (Enter, arrows, F1, ...).
/// `location` tells numpad keys apart from the main ones.
pub fn named_keysym(key: NamedKey, location: KeyLocation) -> Option<u32> {
    use NamedKey::*;
    let numpad = location == KeyLocation::Numpad;
    let sym = match key {
        Enter if numpad => 0xff8d,
        Enter => xk::RETURN,
        Tab => xk::TAB,
        Space => 0x0020,
        Backspace => xk::BACKSPACE,
        Escape => xk::ESCAPE,
        Delete if numpad => 0xff9f,
        Delete => xk::DELETE,
        Insert if numpad => 0xff9e,
        Insert => 0xff63,
        Home if numpad => 0xff95,
        Home => 0xff50,
        End if numpad => 0xff9c,
        End => 0xff57,
        PageUp if numpad => 0xff9a,
        PageUp => 0xff55,
        PageDown if numpad => 0xff9b,
        PageDown => 0xff56,
        ArrowLeft if numpad => 0xff96,
        ArrowLeft => 0xff51,
        ArrowUp if numpad => 0xff97,
        ArrowUp => 0xff52,
        ArrowRight if numpad => 0xff98,
        ArrowRight => 0xff53,
        ArrowDown if numpad => 0xff99,
        ArrowDown => 0xff54,
        Clear => 0xff0b,
        F1 => 0xffbe,
        F2 => 0xffbf,
        F3 => 0xffc0,
        F4 => 0xffc1,
        F5 => 0xffc2,
        F6 => 0xffc3,
        F7 => 0xffc4,
        F8 => 0xffc5,
        F9 => 0xffc6,
        F10 => 0xffc7,
        F11 => 0xffc8,
        F12 => 0xffc9,
        F13 => 0xffca,
        F14 => 0xffcb,
        F15 => 0xffcc,
        F16 => 0xffcd,
        F17 => 0xffce,
        F18 => 0xffcf,
        F19 => 0xffd0,
        F20 => 0xffd1,
        F21 => 0xffd2,
        F22 => 0xffd3,
        F23 => 0xffd4,
        F24 => 0xffd5,
        F25 => 0xffd6,
        F26 => 0xffd7,
        F27 => 0xffd8,
        F28 => 0xffd9,
        F29 => 0xffda,
        F30 => 0xffdb,
        F31 => 0xffdc,
        F32 => 0xffdd,
        F33 => 0xffde,
        F34 => 0xffdf,
        F35 => 0xffe0,
        Shift if location == KeyLocation::Right => xk::SHIFT_R,
        Shift => xk::SHIFT_L,
        Control if location == KeyLocation::Right => xk::CONTROL_R,
        Control => xk::CONTROL_L,
        Alt if location == KeyLocation::Right => xk::ALT_R,
        Alt => xk::ALT_L,
        AltGraph => xk::ISO_LEVEL3_SHIFT,
        Super | Meta if location == KeyLocation::Right => xk::SUPER_R,
        Super | Meta => xk::SUPER_L,
        Hyper => 0xffed,
        CapsLock => xk::CAPS_LOCK,
        NumLock => 0xff7f,
        ScrollLock => 0xff14,
        Pause => 0xff13,
        PrintScreen => xk::PRINT,
        ContextMenu => 0xff67,
        Help => 0xff6a,
        Cancel => 0xff69,
        Execute => 0xff62,
        Find => 0xff68,
        Select => 0xff60,
        Undo => 0xff65,
        Redo => 0xff66,
        Convert => 0xff23,
        NonConvert => 0xff22,
        KanaMode => 0xff2d,
        HangulMode => 0xff31,
        HanjaMode => 0xff34,
        AudioVolumeMute => 0x1008ff12,
        AudioVolumeDown => 0x1008ff11,
        AudioVolumeUp => 0x1008ff13,
        MediaPlayPause | MediaPlay => 0x1008ff14,
        MediaStop => 0x1008ff15,
        MediaTrackPrevious => 0x1008ff16,
        MediaTrackNext => 0x1008ff17,
        BrowserHome => 0x1008ff18,
        LaunchMail => 0x1008ff19,
        BrowserSearch => 0x1008ff1b,
        LaunchApplication2 => 0x1008ff1d,
        BrowserBack => 0x1008ff26,
        BrowserForward => 0x1008ff27,
        BrowserStop => 0x1008ff28,
        BrowserRefresh => 0x1008ff29,
        Power => 0x1008ff2a,
        WakeUp => 0x1008ff2b,
        Eject => 0x1008ff2c,
        Standby => 0x1008ff2f,
        BrowserFavorites => 0x1008ff30,
        BrightnessUp => 0x1008ff02,
        BrightnessDown => 0x1008ff03,
        _ => return None,
    };
    Some(sym)
}

/// Keysym for a typed character: Latin-1 maps to itself, everything else
/// to 0x0100_0000 + the Unicode code point.
pub fn char_keysym(c: char) -> Option<u32> {
    let cp = c as u32;
    match cp {
        0x08 => Some(xk::BACKSPACE),
        0x09 => Some(xk::TAB),
        0x0a | 0x0d => Some(xk::RETURN),
        0x1b => Some(xk::ESCAPE),
        0x7f => Some(xk::DELETE),
        0x00..=0x1f | 0x80..=0x9f => None,
        0x20..=0x7e | 0xa0..=0xff => Some(cp),
        _ => Some(0x0100_0000 + cp),
    }
}

/// Keysym for a numpad digit or operator, so it arrives as a numpad key.
pub fn numpad_char_keysym(c: char) -> Option<u32> {
    let sym = match c {
        '0'..='9' => 0xffb0 + (c as u32 - '0' as u32),
        '*' => 0xffaa,
        '+' => 0xffab,
        '-' => 0xffad,
        '.' | ',' => 0xffae,
        '/' => 0xffaf,
        '=' => 0xffbd,
        _ => return None,
    };
    Some(sym)
}

/// Keysym for a "dead" accent key, which combines with the next letter.
pub fn dead_keysym(accent: Option<char>) -> Option<u32> {
    let sym = match accent? {
        '`' | '\u{300}' => 0xfe50,
        '´' | '\'' | '\u{301}' => 0xfe51,
        '^' | 'ˆ' | '\u{302}' => 0xfe52,
        '~' | '˜' | '\u{303}' => 0xfe53,
        '¯' | '\u{304}' => 0xfe54,
        '˘' | '\u{306}' => 0xfe55,
        '˙' | '\u{307}' => 0xfe56,
        '¨' | '"' | '\u{308}' => 0xfe57,
        '˚' | '°' | '\u{30a}' => 0xfe58,
        '˝' | '\u{30b}' => 0xfe59,
        'ˇ' | '\u{30c}' => 0xfe5a,
        '¸' | '\u{327}' => 0xfe5b,
        '˛' | '\u{328}' => 0xfe5c,
        _ => return None,
    };
    Some(sym)
}

/// The single character a key produced, if it produced exactly one.
pub fn single_char(key: &Key) -> Option<char> {
    match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some(c)
        }
        Key::Named(NamedKey::Space) => Some(' '),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits() {
        assert_eq!(scancode_for_code(KeyCode::KeyA), 0x1e);
        assert_eq!(scancode_for_code(KeyCode::KeyZ), 0x2c);
        assert_eq!(scancode_for_code(KeyCode::KeyQ), 0x10);
        assert_eq!(scancode_for_code(KeyCode::KeyM), 0x32);
        assert_eq!(scancode_for_code(KeyCode::Digit1), 0x02);
        assert_eq!(scancode_for_code(KeyCode::Digit0), 0x0b);
    }

    #[test]
    fn special_keys() {
        assert_eq!(scancode_for_code(KeyCode::IntlBackslash), 0x56);
        assert_eq!(scancode_for_code(KeyCode::AltRight), 0xe038);
        assert_eq!(scancode_for_code(KeyCode::SuperLeft), 0xe05b);
        assert_eq!(scancode_for_code(KeyCode::SuperRight), 0xe05c);
        assert_eq!(scancode_for_code(KeyCode::ContextMenu), 0xe05d);
        assert_eq!(scancode_for_code(KeyCode::ControlRight), 0xe01d);
        assert_eq!(scancode_for_code(KeyCode::CapsLock), 0x3a);
        assert_eq!(scancode_for_code(KeyCode::NumpadEnter), 0xe01c);
        assert_eq!(scancode_for_code(KeyCode::Numpad5), 0x4c);
        assert_eq!(scancode_for_code(KeyCode::NumpadDivide), 0xe035);
        assert_eq!(scancode_for_code(KeyCode::Delete), 0xe053);
        assert_eq!(scancode_for_code(KeyCode::Fn), 0);
    }

    #[test]
    fn every_function_key_has_a_scancode() {
        let keys = [
            KeyCode::F1,
            KeyCode::F2,
            KeyCode::F3,
            KeyCode::F4,
            KeyCode::F5,
            KeyCode::F6,
            KeyCode::F7,
            KeyCode::F8,
            KeyCode::F9,
            KeyCode::F10,
            KeyCode::F11,
            KeyCode::F12,
            KeyCode::F13,
            KeyCode::F14,
            KeyCode::F15,
            KeyCode::F16,
            KeyCode::F17,
            KeyCode::F18,
            KeyCode::F19,
            KeyCode::F20,
            KeyCode::F21,
            KeyCode::F22,
            KeyCode::F23,
            KeyCode::F24,
        ];
        let mut seen = std::collections::HashSet::new();
        for key in keys {
            let code = scancode_for_code(key);
            assert_ne!(code, 0, "{key:?}");
            assert!(seen.insert(code), "duplicate scancode for {key:?}");
        }
    }

    #[test]
    fn scancodes_are_unique() {
        use KeyCode::*;
        let all = [
            Backquote, Backslash, BracketLeft, BracketRight, Comma, Digit0, Digit1, Digit2,
            Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Equal, IntlBackslash, IntlRo,
            IntlYen, KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
            KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ, Minus,
            Period, Quote, Semicolon, Slash, AltLeft, AltRight, Backspace, CapsLock, ContextMenu,
            ControlLeft, ControlRight, Enter, SuperLeft, SuperRight, ShiftLeft, ShiftRight, Space,
            Tab, Convert, KanaMode, Lang1, Lang2, NonConvert, Delete, End, Home, Insert, PageDown,
            PageUp, ArrowDown, ArrowLeft, ArrowRight, ArrowUp, NumLock, Numpad0, Numpad1, Numpad2,
            Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, NumpadAdd,
            NumpadComma, NumpadDecimal, NumpadDivide, NumpadEnter, NumpadEqual, NumpadMultiply,
            NumpadSubtract, Escape, PrintScreen, ScrollLock, Pause,
        ];
        let mut seen = std::collections::HashMap::new();
        for key in all {
            let code = scancode_for_code(key);
            assert_ne!(code, 0, "{key:?} has no scancode");
            if let Some(other) = seen.insert(code, key) {
                panic!("{key:?} and {other:?} share scancode {code:#x}");
            }
        }
    }

    #[test]
    fn named_keys() {
        let main = KeyLocation::Standard;
        assert_eq!(named_keysym(NamedKey::Enter, main), Some(0xff0d));
        assert_eq!(named_keysym(NamedKey::Enter, KeyLocation::Numpad), Some(0xff8d));
        assert_eq!(named_keysym(NamedKey::Tab, main), Some(0xff09));
        assert_eq!(named_keysym(NamedKey::Escape, main), Some(0xff1b));
        assert_eq!(named_keysym(NamedKey::ArrowLeft, main), Some(0xff51));
        assert_eq!(named_keysym(NamedKey::ArrowDown, main), Some(0xff54));
        assert_eq!(named_keysym(NamedKey::F1, main), Some(0xffbe));
        assert_eq!(named_keysym(NamedKey::F12, main), Some(0xffc9));
        assert_eq!(named_keysym(NamedKey::F24, main), Some(0xffd5));
        assert_eq!(named_keysym(NamedKey::Delete, main), Some(0xffff));
        assert_eq!(named_keysym(NamedKey::Shift, KeyLocation::Right), Some(0xffe2));
        assert_eq!(named_keysym(NamedKey::Super, KeyLocation::Left), Some(0xffeb));
        assert_eq!(named_keysym(NamedKey::ContextMenu, main), Some(0xff67));
        assert_eq!(named_keysym(NamedKey::Space, main), Some(0x20));
    }

    #[test]
    fn characters() {
        assert_eq!(char_keysym('a'), Some(0x61));
        assert_eq!(char_keysym('A'), Some(0x41));
        assert_eq!(char_keysym('@'), Some(0x40));
        assert_eq!(char_keysym('å'), Some(0xe5));
        assert_eq!(char_keysym('ÿ'), Some(0xff));
        assert_eq!(char_keysym('€'), Some(0x0100_20ac));
        assert_eq!(char_keysym('ł'), Some(0x0100_0142));
        assert_eq!(char_keysym('\n'), Some(0xff0d));
        assert_eq!(char_keysym('\u{1}'), None);
        assert_eq!(numpad_char_keysym('7'), Some(0xffb7));
        assert_eq!(dead_keysym(Some('´')), Some(0xfe51));
        assert_eq!(dead_keysym(Some('¨')), Some(0xfe57));
        assert_eq!(dead_keysym(None), None);
    }
}
