//! The live `InputSource`: find the Steam Input virtual keyboard, read its key
//! presses on a blocking thread, map them to `InputIntent`s, and `EVIOCGRAB` the
//! *virtual* device on demand (never the physical pad).
//!
//! Grab/read coexistence: the read thread owns the `evdev::Device` (it needs
//! `&mut` for `fetch_events`), so the grab can't go through the same handle.
//! Instead we capture the device's raw fd up front and issue `EVIOCGRAB` as a
//! direct ioctl on that fd — a quick kernel-flag syscall that is safe to make
//! while the read thread is blocked in `fetch_events`. The `NavGuard`'s `Drop`
//! always ungrabs (the soft-brick failsafe, ARCHITECTURE §4.1).

use crate::{intent_for_key, is_steam_virtual_keyboard, keyname_for_evdev};
use cc_core::{InputError, InputIntent, InputSource, NavGuard};
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use std::os::unix::io::RawFd;
use tokio::sync::mpsc;

/// `EVIOCGRAB` = `_IOW('E', 0x90, int)` → dir(W=1)<<30 | size(4)<<16 | 'E'<<8 | 0x90.
const EVIOCGRAB: libc::c_ulong = (1 << 30) | (4 << 16) | ((b'E' as libc::c_ulong) << 8) | 0x90;

fn set_grab(fd: RawFd, on: bool) -> bool {
    // SAFETY: fd is a valid evdev device fd owned for the lifetime of the read
    // thread; the ioctl only toggles the kernel's exclusive-grab flag.
    let arg: libc::c_int = on as libc::c_int;
    unsafe { libc::ioctl(fd, EVIOCGRAB, arg) == 0 }
}

/// Live evdev input source bound to the Steam virtual keyboard.
pub struct EvdevInput {
    rx: Option<mpsc::Receiver<InputIntent>>,
    grab_fd: RawFd,
}

impl EvdevInput {
    /// Discover the Steam Input virtual keyboard and start reading it. Errors if
    /// no such device is present (i.e. no game with Steam Input is running).
    pub fn open() -> Result<Self, InputError> {
        let (path, device) = find_virtual_keyboard()
            .ok_or_else(|| InputError::new("no Steam Input virtual keyboard found"))?;
        Self::from_device(path, device)
    }

    /// Build from an already-opened device (kept for a future test harness using
    /// a uinput-created fake keyboard).
    pub fn from_device(path: String, device: evdev::Device) -> Result<Self, InputError> {
        use std::os::unix::io::AsRawFd;
        let grab_fd = device.as_raw_fd();
        let (tx, rx) = mpsc::channel::<InputIntent>(64);
        std::thread::Builder::new()
            .name("cc-input-read".into())
            .spawn(move || read_loop(device, tx, path))
            .map_err(|e| InputError::new(format!("spawn read thread: {e}")))?;
        Ok(EvdevInput {
            rx: Some(rx),
            grab_fd,
        })
    }
}

impl InputSource for EvdevInput {
    fn intents(&mut self) -> BoxStream<'static, InputIntent> {
        let rx = self.rx.take().expect("intents() called more than once");
        futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|i| (i, rx)) })
            .boxed()
    }

    fn grab(&mut self) -> Result<NavGuard, InputError> {
        let fd = self.grab_fd;
        if !set_grab(fd, true) {
            return Err(InputError::new("EVIOCGRAB failed"));
        }
        Ok(NavGuard::new(move || {
            set_grab(fd, false); // failsafe: always ungrab on drop
        }))
    }
}

/// Blocking read loop: forward mapped key-press intents until the device dies.
fn read_loop(mut device: evdev::Device, tx: mpsc::Sender<InputIntent>, path: String) {
    loop {
        let events = match device.fetch_events() {
            Ok(ev) => ev,
            Err(e) => {
                tracing::warn!("cc-input: {path} read ended: {e}");
                break;
            }
        };
        for ev in events {
            // key presses only (value 1 = down)
            if let evdev::InputEventKind::Key(key) = ev.kind() {
                if ev.value() == 1 {
                    if let Some(intent) = keyname_for_evdev(key).and_then(intent_for_key) {
                        if tx.blocking_send(intent).is_err() {
                            return; // consumer gone
                        }
                    }
                }
            }
        }
    }
}

/// Scan `/dev/input` for the Steam Input virtual keyboard.
fn find_virtual_keyboard() -> Option<(String, evdev::Device)> {
    let mut entries: Vec<_> = std::fs::read_dir("/dev/input")
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("event"))
        })
        .collect();
    entries.sort();
    for path in entries {
        if let Ok(dev) = evdev::Device::open(&path) {
            let name = dev.name().unwrap_or("").to_string();
            let has_keys = dev
                .supported_keys()
                .is_some_and(|k| k.contains(evdev::Key::KEY_ENTER));
            if is_steam_virtual_keyboard(&name, has_keys) {
                return Some((path.display().to_string(), dev));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviocgrab_constant_matches_x86_64_value() {
        // The well-known EVIOCGRAB request number on Linux.
        assert_eq!(EVIOCGRAB, 0x4004_4590);
    }

    #[test]
    fn evdev_keys_map_through_to_intents() {
        // F13 (the chord signal) → Chord; Enter → Confirm; Tab → AnchorCycle.
        let chord = keyname_for_evdev(evdev::Key::KEY_F13).and_then(intent_for_key);
        assert_eq!(chord, Some(InputIntent::Chord));
        let confirm = keyname_for_evdev(evdev::Key::KEY_ENTER).and_then(intent_for_key);
        assert_eq!(confirm, Some(InputIntent::Confirm));
        let cycle = keyname_for_evdev(evdev::Key::KEY_TAB).and_then(intent_for_key);
        assert_eq!(cycle, Some(InputIntent::AnchorCycle));
        // an unmapped key is ignored
        assert!(keyname_for_evdev(evdev::Key::KEY_Q).is_none());
    }
}
