//! `cc-input` — the Steam-Input virtual-keyboard boundary.
//!
//! Phase 1 implements the **pure device-classification** logic (which evdev
//! device is the Steam-emitted virtual keyboard vs a real one) and the **nav-key
//! decode** table — both testable without hardware. The live evdev read loop,
//! the `EVIOCGRAB` of the *virtual* device, and the RAII `NavGuard` wiring (the
//! `InputSource` impl) are Phase 3, validated with a controller mid-game.
//!
//! The signal/nav keymap here MUST match the shipped Steam Input `.vdf` action
//! layer — that template is the actual input gate (ARCHITECTURE §4.1); this is
//! only the daemon-side reading of the keys it emits.

pub mod source;
pub use source::EvdevInput;

use cc_core::InputIntent;

/// Heuristic: does this evdev device look like the Steam Input virtual keyboard?
/// Steam's emitted keyboard advertises keyboard keys and carries "steam" /
/// "virtual" in its name. Real hardware keyboards are excluded by the name.
pub fn is_steam_virtual_keyboard(name: &str, has_keyboard_keys: bool) -> bool {
    if !has_keyboard_keys {
        return false;
    }
    let n = name.to_lowercase();
    n.contains("steam") || n.contains("virtual")
}

/// Is this device usable as *some* keyboard at all (for the doctor's listing).
/// A real keyboard has letter + Enter; many mice expose a stray macro key, so
/// we require a broader signature than a single key.
pub fn looks_like_keyboard(has_a: bool, has_enter: bool, has_space: bool, has_leftshift: bool) -> bool {
    has_a && has_enter && has_space && has_leftshift
}

/// The keymap from the action-layer's emitted keys to semantic intents. Uses
/// gamescope-*unmasked* keys (left-Windows is masked — SPEC §technical). These
/// are evdev key *names* to keep the contract readable and testable; the live
/// loop maps `evdev::Key` to these.
pub fn intent_for_key(key: KeyName) -> Option<InputIntent> {
    Some(match key {
        // signal key from the chord (an unmasked, unlikely-in-games combo)
        KeyName::SignalChord => InputIntent::Chord,
        KeyName::Up => InputIntent::Up,
        KeyName::Down => InputIntent::Down,
        KeyName::Left => InputIntent::Left,
        KeyName::Right => InputIntent::Right,
        KeyName::Enter => InputIntent::Confirm,
        KeyName::Escape => InputIntent::Back,
        KeyName::Backspace => InputIntent::Dismiss,
        KeyName::Tab => InputIntent::AnchorCycle,
    })
}

/// The keys the action layer emits. Kept as a small enum so the mapping is a
/// pure, exhaustively-testable table rather than scattered `evdev::Key` matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyName {
    SignalChord,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Backspace,
    Tab,
}

/// Map a raw evdev key to a `KeyName`, or `None` if the action layer doesn't use
/// it. These are the concrete keys the shipped Steam Input `.vdf` emits — all
/// gamescope-*unmasked* (the left-Windows key is masked, so `F13` is the chord
/// signal). The mapping is pure and tested.
pub fn keyname_for_evdev(key: evdev::Key) -> Option<KeyName> {
    use evdev::Key;
    Some(match key {
        Key::KEY_F13 => KeyName::SignalChord,
        Key::KEY_UP => KeyName::Up,
        Key::KEY_DOWN => KeyName::Down,
        Key::KEY_LEFT => KeyName::Left,
        Key::KEY_RIGHT => KeyName::Right,
        Key::KEY_ENTER | Key::KEY_KPENTER => KeyName::Enter,
        Key::KEY_ESC => KeyName::Escape,
        Key::KEY_BACKSPACE => KeyName::Backspace,
        Key::KEY_TAB => KeyName::Tab,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_steam_virtual_keyboard_only_with_keyboard_caps() {
        assert!(is_steam_virtual_keyboard("Steam Virtual Keyboard", true));
        assert!(is_steam_virtual_keyboard("steam-input virtual", true));
        // name matches but no keyboard caps → not it
        assert!(!is_steam_virtual_keyboard("Steam Controller", false));
        // real hardware keyboard → not the virtual one
        assert!(!is_steam_virtual_keyboard("ZSA Technology Labs Voyager Keyboard", true));
        // a Razer mouse exposing macro keys → name excludes it
        assert!(!is_steam_virtual_keyboard("Razer Basilisk V3 Pro", true));
    }

    #[test]
    fn keyboard_signature_requires_more_than_one_key() {
        assert!(looks_like_keyboard(true, true, true, true));
        // a mouse with a stray KEY_A macro but no real keyboard signature
        assert!(!looks_like_keyboard(true, false, false, false));
        assert!(!looks_like_keyboard(true, true, false, false));
    }

    #[test]
    fn every_layer_key_maps_to_an_intent() {
        use KeyName::*;
        let all = [SignalChord, Up, Down, Left, Right, Enter, Escape, Backspace, Tab];
        for k in all {
            assert!(intent_for_key(k).is_some(), "{k:?} must map to an intent");
        }
        assert_eq!(intent_for_key(KeyName::SignalChord), Some(InputIntent::Chord));
        assert_eq!(intent_for_key(KeyName::Enter), Some(InputIntent::Confirm));
        assert_eq!(intent_for_key(KeyName::Tab), Some(InputIntent::AnchorCycle));
    }
}
