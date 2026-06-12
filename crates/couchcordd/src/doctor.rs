//! The Phase 0 de-risk gate.
//!
//! Each check reports one of three states:
//!   - `Ok`   — assumption verified here and now.
//!   - `Warn` — not verifiable in this context (e.g. no game-mode session is
//!              running); informational, not a failure. Most checks Warn when
//!              run headless — they're meant to be re-run inside the session.
//!   - `Fail` — a concrete, actionable problem (e.g. can't read input devices).
//!
//! Exit code = number of `Fail`s, so `0` means nothing is actionably broken.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;

enum Status {
    Ok,
    Warn,
    Fail,
}

struct Check {
    status: Status,
    title: &'static str,
    detail: String,
    hint: Option<String>,
}

impl Check {
    fn print(&self) {
        let (mark, label) = match self.status {
            Status::Ok => ("\x1b[32m✓\x1b[0m", "OK  "),
            Status::Warn => ("\x1b[33m‼\x1b[0m", "WARN"),
            Status::Fail => ("\x1b[31m✗\x1b[0m", "FAIL"),
        };
        println!("{mark} [{label}] {}", self.title);
        for line in self.detail.lines() {
            println!("        {line}");
        }
        if let Some(hint) = &self.hint {
            println!("        \x1b[2m→ {hint}\x1b[0m");
        }
    }
}

pub fn run() -> ExitCode {
    println!("couchcordd doctor — Phase 0 de-risk gate\n");
    let checks = [check_discord_ipc(), check_steam_virtual_keyboard(), check_gamescope_overlay()];
    for c in &checks {
        c.print();
        println!();
    }
    let fails = checks.iter().filter(|c| matches!(c.status, Status::Fail)).count();
    let warns = checks.iter().filter(|c| matches!(c.status, Status::Warn)).count();
    println!(
        "summary: {} ok, {warns} warn, {fails} fail",
        checks.iter().filter(|c| matches!(c.status, Status::Ok)).count()
    );
    if warns > 0 && fails == 0 {
        println!("\x1b[2mre-run inside a game-mode session (game using Steam Input) to verify the WARN checks.\x1b[0m");
    }
    ExitCode::from(fails.min(255) as u8)
}

// ---------------------------------------------------------------------------
// 1. Discord local RPC socket
// ---------------------------------------------------------------------------

fn check_discord_ipc() -> Check {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());
    // Discord uses discord-ipc-0 .. discord-ipc-9 (first free slot).
    let mut found = None;
    for n in 0..10 {
        let path = PathBuf::from(&runtime).join(format!("discord-ipc-{n}"));
        if path.exists() {
            // Confirm it's a live, connectable socket, not a stale node.
            let connectable = UnixStream::connect(&path).is_ok();
            found = Some((path, connectable));
            break;
        }
    }
    match found {
        Some((path, true)) => Check {
            status: Status::Ok,
            title: "Discord local RPC socket",
            detail: format!("connectable at {}", path.display()),
            hint: None,
        },
        Some((path, false)) => Check {
            status: Status::Fail,
            title: "Discord local RPC socket",
            detail: format!("{} exists but refused a connection (stale?)", path.display()),
            hint: Some("restart Discord so it recreates the IPC socket".into()),
        },
        None => Check {
            status: Status::Warn,
            title: "Discord local RPC socket",
            detail: format!("no discord-ipc-N socket under {runtime}"),
            hint: Some("start the Discord client (it must be running for voice + RPC)".into()),
        },
    }
}

// ---------------------------------------------------------------------------
// 2. Steam Input virtual keyboard + input-device readability (input group)
// ---------------------------------------------------------------------------

fn check_steam_virtual_keyboard() -> Check {
    let mut keyboards: Vec<String> = Vec::new();
    let mut virtual_candidates: Vec<String> = Vec::new();
    let mut denied = 0usize;
    let mut total = 0usize;

    let mut entries: Vec<PathBuf> = match fs::read_dir("/dev/input") {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")))
            .collect(),
        Err(e) => {
            return Check {
                status: Status::Fail,
                title: "Steam Input virtual keyboard",
                detail: format!("cannot list /dev/input: {e}"),
                hint: None,
            }
        }
    };
    entries.sort();

    for path in &entries {
        total += 1;
        match evdev::Device::open(path) {
            Ok(dev) => {
                let name = dev.name().unwrap_or("<unnamed>").to_string();
                let is_keyboard = dev
                    .supported_keys()
                    .is_some_and(|k| k.contains(evdev::Key::KEY_ENTER) && k.contains(evdev::Key::KEY_A));
                if is_keyboard {
                    keyboards.push(name.clone());
                    let lname = name.to_lowercase();
                    if lname.contains("steam") || lname.contains("virtual") {
                        virtual_candidates.push(name);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => denied += 1,
            Err(_) => {}
        }
    }

    if denied > 0 && keyboards.is_empty() {
        return Check {
            status: Status::Fail,
            title: "Steam Input virtual keyboard",
            detail: format!("permission denied on {denied}/{total} input devices; none readable"),
            hint: Some("join the input group: sudo usermod -aG input $USER, then re-login".into()),
        };
    }

    let mut detail = format!("{} readable input devices, keyboards: {}", total, keyboards.len());
    if !keyboards.is_empty() {
        detail.push_str(&format!("\n  [{}]", keyboards.join(", ")));
    }
    if denied > 0 {
        detail.push_str(&format!("\n  ({denied} device(s) not readable — partial input-group access)"));
    }

    if !virtual_candidates.is_empty() {
        Check {
            status: Status::Ok,
            title: "Steam Input virtual keyboard",
            detail: format!("{detail}\n  steam/virtual candidate: {}", virtual_candidates.join(", ")),
            hint: None,
        }
    } else {
        Check {
            status: Status::Warn,
            title: "Steam Input virtual keyboard",
            detail: format!("{detail}\n  no steam/virtual keyboard present"),
            hint: Some(
                "expected only while a game runs with Steam Input active — re-run inside a game session"
                    .into(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// 3. gamescope nested-X display + GAMESCOPE_EXTERNAL_OVERLAY atom
// ---------------------------------------------------------------------------

fn check_gamescope_overlay() -> Check {
    // Discover candidate X displays from the abstract/filesystem sockets.
    let mut displays: Vec<u32> = match fs::read_dir("/tmp/.X11-unix") {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.strip_prefix('X').map(str::to_string)))
            .filter_map(|s| s.parse::<u32>().ok())
            .collect(),
        Err(_) => Vec::new(),
    };
    displays.sort_unstable();
    displays.dedup();

    if displays.is_empty() {
        return Check {
            status: Status::Warn,
            title: "gamescope external-overlay",
            detail: "no X displays found under /tmp/.X11-unix".into(),
            hint: Some("the gamescope nested-X server only exists inside a game-mode session".into()),
        };
    }

    let mut seen = Vec::new();
    for &n in &displays {
        let dpy = format!(":{n}");
        match probe_external_overlay(&dpy) {
            Ok(true) => {
                return Check {
                    status: Status::Ok,
                    title: "gamescope external-overlay",
                    detail: format!("{dpy} advertises GAMESCOPE_EXTERNAL_OVERLAY (renderable)"),
                    hint: None,
                }
            }
            Ok(false) => seen.push(format!("{dpy} (no overlay atom)")),
            Err(e) => seen.push(format!("{dpy} (unreachable: {e})")),
        }
    }

    Check {
        status: Status::Warn,
        title: "gamescope external-overlay",
        detail: format!("checked {}: none advertise the atom", seen.join(", ")),
        hint: Some("run inside the gamescope game-mode session to verify overlay rendering".into()),
    }
}

/// Connect to `dpy` and check whether the `GAMESCOPE_EXTERNAL_OVERLAY` atom is
/// registered (only_if_exists). Its presence means a gamescope compositor that
/// supports the external-overlay path owns this display.
fn probe_external_overlay(dpy: &str) -> anyhow::Result<bool> {
    use x11rb::protocol::xproto::ConnectionExt;
    let (conn, _screen) = x11rb::connect(Some(dpy))?;
    let atom = conn.intern_atom(true, b"GAMESCOPE_EXTERNAL_OVERLAY")?.reply()?.atom;
    Ok(atom != 0)
}
