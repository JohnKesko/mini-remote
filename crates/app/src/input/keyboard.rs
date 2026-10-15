//! Turns local key presses into VNC key messages, applying the Mac modifier
//! mapping (Command → Control, Option → Alt unless it typed a character).

use std::collections::HashMap;

use egui_winit::winit::event::{ElementState, KeyEvent};
use egui_winit::winit::keyboard::{Key, KeyCode, KeyLocation, PhysicalKey};

use super::keymap::{self, sc, xk};
use crate::model::{AltMapping, MetaMapping};

/// One key message for the remote computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyMsg {
    pub down: bool,
    pub keysym: u32,
    pub scancode: u32,
}

impl KeyMsg {
    pub fn down(keysym: u32, scancode: u32) -> Self {
        KeyMsg { down: true, keysym, scancode }
    }

    pub fn up(keysym: u32, scancode: u32) -> Self {
        KeyMsg { down: false, keysym, scancode }
    }
}

/// How keys are translated; built from the settings and the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyConfig {
    pub meta: MetaMapping,
    pub alt: AltMapping,
    /// The server takes key positions (physical keyboard mode is on and supported).
    pub physical: bool,
    /// Caps Lock events report on/off instead of press/release (macOS).
    pub caps_lock_reports_state: bool,
}

/// The parts of a key event the translation needs. Separate from winit's
/// `KeyEvent` so the logic can be tested.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub physical: PhysicalKey,
    /// The key with all modifiers applied (e.g. "@" for Option+2 on a Swedish Mac).
    pub logical: Key,
    /// The key without any modifiers (e.g. "2"), used for shortcuts.
    pub base: Key,
    pub location: KeyLocation,
    pub pressed: bool,
    pub repeat: bool,
}

impl KeyInput {
    pub fn from_winit(event: &KeyEvent) -> Self {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        let base = {
            use egui_winit::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
            event.key_without_modifiers()
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let base = event.logical_key.clone();
        KeyInput {
            physical: crate::platform::physical_key(event.physical_key),
            logical: event.logical_key.clone(),
            base,
            location: event.location,
            pressed: event.state == ElementState::Pressed,
            repeat: event.repeat,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModKind {
    Shift,
    Control,
    Meta,
    Alt,
}

fn modifier_of(code: KeyCode) -> Option<(ModKind, Side)> {
    Some(match code {
        KeyCode::ShiftLeft => (ModKind::Shift, Side::Left),
        KeyCode::ShiftRight => (ModKind::Shift, Side::Right),
        KeyCode::ControlLeft => (ModKind::Control, Side::Left),
        KeyCode::ControlRight => (ModKind::Control, Side::Right),
        KeyCode::SuperLeft | KeyCode::Meta => (ModKind::Meta, Side::Left),
        KeyCode::SuperRight => (ModKind::Meta, Side::Right),
        KeyCode::AltLeft => (ModKind::Alt, Side::Left),
        KeyCode::AltRight => (ModKind::Alt, Side::Right),
        _ => return None,
    })
}

/// Keysym and scancode a modifier key sends with the current mapping.
fn modifier_message(kind: ModKind, side: Side, cfg: &KeyConfig) -> (u32, u32) {
    let left = side == Side::Left;
    match kind {
        ModKind::Shift => {
            if left { (xk::SHIFT_L, sc::SHIFT_L) } else { (xk::SHIFT_R, sc::SHIFT_R) }
        }
        ModKind::Control => {
            if left { (xk::CONTROL_L, sc::CONTROL_L) } else { (xk::CONTROL_R, sc::CONTROL_R) }
        }
        ModKind::Meta => match cfg.meta {
            MetaMapping::Control => {
                if left { (xk::CONTROL_L, sc::CONTROL_L) } else { (xk::CONTROL_R, sc::CONTROL_R) }
            }
            MetaMapping::Super => {
                if left { (xk::SUPER_L, sc::SUPER_L) } else { (xk::SUPER_R, sc::SUPER_R) }
            }
        },
        ModKind::Alt => match cfg.alt {
            AltMapping::Alt => {
                if left { (xk::ALT_L, sc::ALT_L) } else { (xk::ALT_R, sc::ALT_R) }
            }
            AltMapping::AltGr => (xk::ISO_LEVEL3_SHIFT, sc::ALT_R),
        },
    }
}

/// An Option (Alt) key that is held but maybe not sent yet.
#[derive(Clone, Copy, Debug, Default)]
struct OptionKey {
    held: bool,
    /// Alt was sent to the remote computer (keysym, scancode).
    sent: Option<(u32, u32)>,
    /// Another key was pressed while it was held.
    used: bool,
}

/// Remembers which keys are held so releases match presses, and so
/// everything can be let go when the window loses focus.
#[derive(Debug, Default)]
pub struct Keyboard {
    /// Keys currently down on the remote computer, by local key.
    held: HashMap<PhysicalKey, (u32, u32)>,
    option: [OptionKey; 2],
    /// Modifiers (by kind) that act as Control or Super on the remote side.
    shortcut_mods: HashMap<PhysicalKey, ModKind>,
    /// An accent key (like ´) waiting for the next key.
    pending_dead: Option<u32>,
}

impl Keyboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Translate one local key event into messages for the remote computer.
    pub fn key(&mut self, input: &KeyInput, cfg: &KeyConfig) -> Vec<KeyMsg> {
        let mut out = Vec::new();
        let code = match input.physical {
            PhysicalKey::Code(code) => Some(code),
            PhysicalKey::Unidentified(_) => None,
        };

        if let Some((kind, side)) = code.and_then(modifier_of) {
            self.modifier(input, kind, side, cfg, &mut out);
            return out;
        }

        if code == Some(KeyCode::CapsLock) {
            if input.repeat {
                return out;
            }
            if cfg.caps_lock_reports_state {
                // Each event flips Caps Lock, so send a full press.
                out.push(KeyMsg::down(xk::CAPS_LOCK, sc::CAPS_LOCK));
                out.push(KeyMsg::up(xk::CAPS_LOCK, sc::CAPS_LOCK));
            } else {
                out.push(KeyMsg { down: input.pressed, keysym: xk::CAPS_LOCK, scancode: sc::CAPS_LOCK });
            }
            return out;
        }

        let scancode = keymap::scancode(input.physical);
        if !input.pressed {
            if let Some((keysym, scancode)) = self.held.remove(&input.physical) {
                out.push(KeyMsg::up(keysym, scancode));
            }
            return out;
        }

        if input.repeat
            && let Some(&(keysym, scancode)) = self.held.get(&input.physical)
        {
            out.push(KeyMsg::down(keysym, scancode));
            return out;
        }

        // If a different key is still marked as held at the same position, let it go first.
        if let Some((keysym, scancode)) = self.held.remove(&input.physical) {
            out.push(KeyMsg::up(keysym, scancode));
        }

        let option_held = self.option.iter().any(|o| o.held);
        for o in &mut self.option {
            if o.held {
                o.used = true;
            }
        }
        let shortcut = !self.shortcut_mods.is_empty();

        if !cfg.physical && !shortcut {
            // Accent keys combine with the next key. Hold them back: the Mac may
            // already deliver the combined letter (é), otherwise send both.
            if let Key::Dead(accent) = &input.logical
                && let Some(sym) = keymap::dead_keysym(*accent)
            {
                self.pending_dead = Some(sym);
                return out;
            }
            if let Some(dead) = self.pending_dead.take()
                && keymap::single_char(&input.logical).is_some()
                && !already_combined(input)
            {
                out.push(KeyMsg::down(dead, 0));
                out.push(KeyMsg::up(dead, 0));
            }
        }
        self.pending_dead = None;

        let keysym = if cfg.physical {
            // The server uses the key position; the keysym is only a fallback.
            self.flush_option(cfg, &mut out);
            plain_keysym(input).or_else(|| base_keysym(input))
        } else if option_held && !shortcut && option_typed_character(input) {
            // Option typed a character (like @ or {): send only that character.
            plain_keysym(input)
        } else if option_held || shortcut {
            // A shortcut such as Ctrl+C or Alt+Tab: send the plain key.
            self.flush_option(cfg, &mut out);
            base_keysym(input).or_else(|| plain_keysym(input))
        } else {
            plain_keysym(input)
        };

        match keysym {
            Some(keysym) => {
                self.held.insert(input.physical, (keysym, scancode));
                out.push(KeyMsg::down(keysym, scancode));
            }
            None if !cfg.physical => {
                // Several characters at once (some layouts do this): type them.
                if let Key::Character(text) = &input.logical {
                    for c in text.chars().filter_map(keymap::char_keysym) {
                        out.push(KeyMsg::down(c, 0));
                        out.push(KeyMsg::up(c, 0));
                    }
                }
            }
            None if scancode != 0 => {
                // Unknown symbol, but the server can still use the position.
                self.held.insert(input.physical, (0, scancode));
                out.push(KeyMsg::down(0, scancode));
            }
            None => {}
        }
        out
    }

    fn modifier(
        &mut self,
        input: &KeyInput,
        kind: ModKind,
        side: Side,
        cfg: &KeyConfig,
        out: &mut Vec<KeyMsg>,
    ) {
        if input.repeat {
            return;
        }
        let (keysym, scancode) = modifier_message(kind, side, cfg);
        let slot = side as usize;

        if kind == ModKind::Alt && !cfg.physical {
            // Wait and see whether Option types a character or acts as Alt.
            let option = &mut self.option[slot];
            if input.pressed {
                *option = OptionKey { held: true, sent: None, used: false };
            } else {
                if let Some((keysym, scancode)) = option.sent {
                    out.push(KeyMsg::up(keysym, scancode));
                } else if option.held && !option.used {
                    // Pressed and released on its own: a plain Alt tap.
                    out.push(KeyMsg::down(keysym, scancode));
                    out.push(KeyMsg::up(keysym, scancode));
                }
                *option = OptionKey::default();
            }
            return;
        }

        let shortcut_kind = matches!(kind, ModKind::Control | ModKind::Meta);
        if input.pressed {
            if shortcut_kind {
                self.shortcut_mods.insert(input.physical, kind);
            }
            if let Some((old_sym, old_code)) = self.held.insert(input.physical, (keysym, scancode))
                && old_sym != keysym
            {
                out.push(KeyMsg::up(old_sym, old_code));
            }
            out.push(KeyMsg::down(keysym, scancode));
        } else {
            self.shortcut_mods.remove(&input.physical);
            if let Some((keysym, scancode)) = self.held.remove(&input.physical) {
                out.push(KeyMsg::up(keysym, scancode));
            }
        }
    }

    /// Send any held Option key as Alt now.
    fn flush_option(&mut self, cfg: &KeyConfig, out: &mut Vec<KeyMsg>) {
        for (slot, option) in self.option.iter_mut().enumerate() {
            if option.held && option.sent.is_none() {
                let side = if slot == 0 { Side::Left } else { Side::Right };
                let (keysym, scancode) = modifier_message(ModKind::Alt, side, cfg);
                option.sent = Some((keysym, scancode));
                out.push(KeyMsg::down(keysym, scancode));
            }
        }
    }

    /// Let go of every key held on the remote computer (e.g. on focus loss).
    pub fn release_all(&mut self) -> Vec<KeyMsg> {
        let mut out: Vec<KeyMsg> = Vec::new();
        // Ordinary keys first, then modifiers, so no accidental shortcut fires.
        let mut held: Vec<_> = self.held.drain().collect();
        held.sort_by_key(|(key, _)| match key {
            PhysicalKey::Code(code) => modifier_of(*code).is_some(),
            PhysicalKey::Unidentified(_) => false,
        });
        for (_, (keysym, scancode)) in held {
            out.push(KeyMsg::up(keysym, scancode));
        }
        for option in &mut self.option {
            if let Some((keysym, scancode)) = option.sent {
                out.push(KeyMsg::up(keysym, scancode));
            }
            *option = OptionKey::default();
        }
        self.shortcut_mods.clear();
        self.pending_dead = None;
        out
    }

    /// Is anything held on the remote side right now?
    #[allow(dead_code)]
    pub fn any_held(&self) -> bool {
        !self.held.is_empty() || self.option.iter().any(|o| o.sent.is_some())
    }
}

/// Keysym for what the key types with the current modifiers.
fn plain_keysym(input: &KeyInput) -> Option<u32> {
    key_keysym(&input.logical, input.location)
}

/// Keysym for the key without modifiers (used for shortcuts like Ctrl+C).
fn base_keysym(input: &KeyInput) -> Option<u32> {
    key_keysym(&input.base, input.location).or_else(|| key_keysym(&input.logical, input.location))
}

fn key_keysym(key: &Key, location: KeyLocation) -> Option<u32> {
    match key {
        Key::Named(named) => keymap::named_keysym(*named, location),
        Key::Character(_) => {
            let c = keymap::single_char(key)?;
            if location == KeyLocation::Numpad
                && let Some(sym) = keymap::numpad_char_keysym(c)
            {
                return Some(sym);
            }
            keymap::char_keysym(c)
        }
        Key::Dead(accent) => keymap::dead_keysym(*accent),
        Key::Unidentified(_) => None,
    }
}

/// Did the system already combine an accent with this key (é instead of e)?
fn already_combined(input: &KeyInput) -> bool {
    match (keymap::single_char(&input.logical), keymap::single_char(&input.base)) {
        (Some(typed), Some(base)) => !typed.is_ascii() && !typed.to_lowercase().eq(base.to_lowercase()),
        _ => false,
    }
}

/// Did Option change what this key types (e.g. "2" became "@")?
fn option_typed_character(input: &KeyInput) -> bool {
    match (&input.logical, &input.base) {
        (Key::Dead(_), _) => true,
        (Key::Character(typed), Key::Character(base)) => typed.to_lowercase() != base.to_lowercase(),
        (Key::Character(_), _) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_winit::winit::keyboard::{NamedKey, SmolStr};

    fn cfg() -> KeyConfig {
        KeyConfig {
            meta: MetaMapping::Control,
            alt: AltMapping::Alt,
            physical: false,
            caps_lock_reports_state: true,
        }
    }

    fn ch(s: &str) -> Key {
        Key::Character(SmolStr::new(s))
    }

    fn key(code: KeyCode, logical: Key, base: Key, pressed: bool) -> KeyInput {
        KeyInput {
            physical: PhysicalKey::Code(code),
            logical,
            base,
            location: KeyLocation::Standard,
            pressed,
            repeat: false,
        }
    }

    fn modifier(code: KeyCode, named: NamedKey, pressed: bool) -> KeyInput {
        let mut k = key(code, Key::Named(named), Key::Named(named), pressed);
        k.location = match code {
            KeyCode::ShiftRight | KeyCode::ControlRight | KeyCode::AltRight | KeyCode::SuperRight => {
                KeyLocation::Right
            }
            _ => KeyLocation::Left,
        };
        k
    }

    #[test]
    fn plain_letter() {
        let mut kb = Keyboard::new();
        let down = kb.key(&key(KeyCode::KeyA, ch("a"), ch("a"), true), &cfg());
        assert_eq!(down, vec![KeyMsg::down('a' as u32, 0x1e)]);
        let up = kb.key(&key(KeyCode::KeyA, ch("a"), ch("a"), false), &cfg());
        assert_eq!(up, vec![KeyMsg::up('a' as u32, 0x1e)]);
    }

    #[test]
    fn command_c_becomes_control_c() {
        let mut kb = Keyboard::new();
        let c = cfg();
        let m = kb.key(&modifier(KeyCode::SuperLeft, NamedKey::Super, true), &c);
        assert_eq!(m, vec![KeyMsg::down(xk::CONTROL_L, sc::CONTROL_L)]);
        // macOS reports "c" for Command+C.
        let m = kb.key(&key(KeyCode::KeyC, ch("c"), ch("c"), true), &c);
        assert_eq!(m, vec![KeyMsg::down('c' as u32, 0x2e)]);
        kb.key(&key(KeyCode::KeyC, ch("c"), ch("c"), false), &c);
        let m = kb.key(&modifier(KeyCode::SuperLeft, NamedKey::Super, false), &c);
        assert_eq!(m, vec![KeyMsg::up(xk::CONTROL_L, sc::CONTROL_L)]);
    }

    #[test]
    fn command_as_super() {
        let mut kb = Keyboard::new();
        let c = KeyConfig { meta: MetaMapping::Super, ..cfg() };
        let m = kb.key(&modifier(KeyCode::SuperRight, NamedKey::Super, true), &c);
        assert_eq!(m, vec![KeyMsg::down(xk::SUPER_R, sc::SUPER_R)]);
    }

    #[test]
    fn control_uses_base_key() {
        let mut kb = Keyboard::new();
        let c = cfg();
        kb.key(&modifier(KeyCode::ControlLeft, NamedKey::Control, true), &c);
        // With Control held the typed text can be a control character.
        let m = kb.key(&key(KeyCode::KeyC, ch("\u{3}"), ch("c"), true), &c);
        assert_eq!(m, vec![KeyMsg::down('c' as u32, 0x2e)]);
    }

    #[test]
    fn option_character_is_sent_without_alt() {
        let mut kb = Keyboard::new();
        let c = cfg();
        let m = kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, true), &c);
        assert!(m.is_empty(), "Option waits: {m:?}");
        // Option+2 types "@" on a Swedish Mac keyboard.
        let m = kb.key(&key(KeyCode::Digit2, ch("@"), ch("2"), true), &c);
        assert_eq!(m, vec![KeyMsg::down('@' as u32, 0x03)]);
        let m = kb.key(&key(KeyCode::Digit2, ch("2"), ch("2"), false), &c);
        assert_eq!(m, vec![KeyMsg::up('@' as u32, 0x03)]);
        let m = kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, false), &c);
        assert!(m.is_empty(), "no Alt was sent: {m:?}");
    }

    #[test]
    fn option_with_named_key_is_alt() {
        let mut kb = Keyboard::new();
        let c = cfg();
        kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, true), &c);
        let m = kb.key(&key(KeyCode::Tab, Key::Named(NamedKey::Tab), Key::Named(NamedKey::Tab), true), &c);
        assert_eq!(m, vec![KeyMsg::down(xk::ALT_L, sc::ALT_L), KeyMsg::down(xk::TAB, sc::TAB)]);
        kb.key(&key(KeyCode::Tab, Key::Named(NamedKey::Tab), Key::Named(NamedKey::Tab), false), &c);
        let m = kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, false), &c);
        assert_eq!(m, vec![KeyMsg::up(xk::ALT_L, sc::ALT_L)]);
    }

    #[test]
    fn option_tap_sends_alt() {
        let mut kb = Keyboard::new();
        let c = cfg();
        kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, true), &c);
        let m = kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, false), &c);
        assert_eq!(m, vec![KeyMsg::down(xk::ALT_L, sc::ALT_L), KeyMsg::up(xk::ALT_L, sc::ALT_L)]);
    }

    #[test]
    fn control_option_letter_is_a_shortcut() {
        let mut kb = Keyboard::new();
        let c = cfg();
        kb.key(&modifier(KeyCode::ControlLeft, NamedKey::Control, true), &c);
        kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, true), &c);
        // Ctrl+Alt+T opens a terminal on the Pi.
        let m = kb.key(&key(KeyCode::KeyT, ch("†"), ch("t"), true), &c);
        assert_eq!(m, vec![KeyMsg::down(xk::ALT_L, sc::ALT_L), KeyMsg::down('t' as u32, 0x14)]);
    }

    #[test]
    fn altgr_mapping() {
        let mut kb = Keyboard::new();
        let c = KeyConfig { alt: AltMapping::AltGr, ..cfg() };
        kb.key(&modifier(KeyCode::AltRight, NamedKey::Alt, true), &c);
        let m = kb.key(&key(KeyCode::F4, Key::Named(NamedKey::F4), Key::Named(NamedKey::F4), true), &c);
        assert_eq!(m[0], KeyMsg::down(xk::ISO_LEVEL3_SHIFT, sc::ALT_R));
    }

    #[test]
    fn physical_mode_sends_alt_at_once() {
        let mut kb = Keyboard::new();
        let c = KeyConfig { physical: true, ..cfg() };
        let m = kb.key(&modifier(KeyCode::AltRight, NamedKey::Alt, true), &c);
        assert_eq!(m, vec![KeyMsg::down(xk::ALT_R, sc::ALT_R)]);
        let m = kb.key(&key(KeyCode::IntlBackslash, ch("<"), ch("<"), true), &c);
        assert_eq!(m, vec![KeyMsg::down('<' as u32, 0x56)]);
    }

    #[test]
    fn release_uses_the_pressed_symbol() {
        let mut kb = Keyboard::new();
        let c = cfg();
        kb.key(&modifier(KeyCode::ShiftLeft, NamedKey::Shift, true), &c);
        let m = kb.key(&key(KeyCode::KeyA, ch("A"), ch("a"), true), &c);
        assert_eq!(m, vec![KeyMsg::down('A' as u32, 0x1e)]);
        kb.key(&modifier(KeyCode::ShiftLeft, NamedKey::Shift, false), &c);
        let m = kb.key(&key(KeyCode::KeyA, ch("a"), ch("a"), false), &c);
        assert_eq!(m, vec![KeyMsg::up('A' as u32, 0x1e)]);
    }

    #[test]
    fn repeat_resends_down() {
        let mut kb = Keyboard::new();
        let c = cfg();
        kb.key(&key(KeyCode::KeyX, ch("x"), ch("x"), true), &c);
        let mut r = key(KeyCode::KeyX, ch("x"), ch("x"), true);
        r.repeat = true;
        assert_eq!(kb.key(&r, &c), vec![KeyMsg::down('x' as u32, 0x2d)]);
    }

    #[test]
    fn caps_lock_toggles() {
        let mut kb = Keyboard::new();
        let m = kb.key(&key(KeyCode::CapsLock, Key::Named(NamedKey::CapsLock), Key::Named(NamedKey::CapsLock), false), &cfg());
        assert_eq!(m, vec![KeyMsg::down(xk::CAPS_LOCK, sc::CAPS_LOCK), KeyMsg::up(xk::CAPS_LOCK, sc::CAPS_LOCK)]);
    }

    #[test]
    fn numpad_keys() {
        let mut kb = Keyboard::new();
        let mut k = key(KeyCode::Numpad7, ch("7"), ch("7"), true);
        k.location = KeyLocation::Numpad;
        assert_eq!(kb.key(&k, &cfg()), vec![KeyMsg::down(0xffb7, 0x47)]);
        let mut k = key(KeyCode::NumpadEnter, Key::Named(NamedKey::Enter), Key::Named(NamedKey::Enter), true);
        k.location = KeyLocation::Numpad;
        assert_eq!(kb.key(&k, &cfg()), vec![KeyMsg::down(0xff8d, 0xe01c)]);
    }

    #[test]
    fn accent_keys() {
        let mut kb = Keyboard::new();
        let c = cfg();
        let dead = key(KeyCode::Equal, Key::Dead(Some('´')), ch("´"), true);
        assert!(kb.key(&dead, &c).is_empty());
        // The Mac combined it already: send just the letter.
        let m = kb.key(&key(KeyCode::KeyE, ch("é"), ch("e"), true), &c);
        assert_eq!(m, vec![KeyMsg::down('é' as u32, 0x12)]);
        kb.key(&key(KeyCode::KeyE, ch("é"), ch("e"), false), &c);
        // Not combined: send the accent first so the Pi combines it.
        kb.key(&dead, &c);
        let m = kb.key(&key(KeyCode::KeyE, ch("e"), ch("e"), true), &c);
        assert_eq!(m, vec![KeyMsg::down(0xfe51, 0), KeyMsg::up(0xfe51, 0), KeyMsg::down('e' as u32, 0x12)]);
    }

    #[test]
    fn release_all_lets_everything_go() {
        let mut kb = Keyboard::new();
        let c = cfg();
        kb.key(&modifier(KeyCode::ControlLeft, NamedKey::Control, true), &c);
        kb.key(&modifier(KeyCode::AltLeft, NamedKey::Alt, true), &c);
        kb.key(&key(KeyCode::KeyT, ch("t"), ch("t"), true), &c);
        let m = kb.release_all();
        assert_eq!(
            m,
            vec![
                KeyMsg::up('t' as u32, 0x14),
                KeyMsg::up(xk::CONTROL_L, sc::CONTROL_L),
                KeyMsg::up(xk::ALT_L, sc::ALT_L),
            ]
        );
        assert!(!kb.any_held());
        assert!(kb.release_all().is_empty());
    }
}
