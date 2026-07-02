//! The live run loop: compose the real boundary impls and drive the reactor.
//!
//! This is the one place concrete impls are named. It is not unit-tested (it
//! needs Discord running, a controller via Steam Input, and the gamescope
//! session); the logic it drives — `Dispatcher` — is mock-tested in `tests.rs`.

use super::Dispatcher;
use cc_config::Settings;
use cc_core::{ConfigSource, InputSource, RpcClient};
use cc_discord::DiscordRpc;
use cc_input::EvdevInput;
use cc_menu::MenuEngine;
use cc_render::X11Overlay;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Build every boundary and run until the input stream ends.
pub async fn run_live() -> anyhow::Result<()> {
    // --- config ---
    let path = cc_config::default_path();
    let cfg = cc_config::load(&path)
        .map_err(|e| anyhow::anyhow!("config ({}): {e}", path.display()))?;
    let settings: Arc<dyn ConfigSource> = Arc::new(Settings::new(cfg.clone(), Some(path)));

    // --- tenant: per-Steam-account state namespace (multi-tenant devices) ---
    match crate::tenant::detect() {
        Some(t) => {
            let dir = t.ensure_state_dir();
            tracing::info!("active Steam account {} — tenant state at {}", t.account_id, dir.display());
        }
        None => tracing::info!("no Steam login detected — using shared state"),
    }

    // --- discord rpc: connect + authenticate (live, event-driven) ---
    // At session start we come up before Discord does (the unit is bound to
    // graphical-session.target). No polling anywhere in this path:
    //   socket appearance  -> inotify on the runtime dirs
    //   sign-in            -> the handshake READY reply, which a signed-out
    //                         Discord holds until the user signs in
    //   socket death       -> stream EOF drops the pending reply (error here)
    let (rpc, user) = loop {
        wait_for_discord_socket().await;
        match DiscordRpc::connect_ipc(cfg.voice_kinds.clone()).await {
            Ok(rpc) => {
                tracing::info!("discord socket up — awaiting READY (sign-in completes it)");
                match rpc.connect(cfg.client_id).await {
                    Ok(user) => break (rpc, user),
                    Err(e) => tracing::info!("discord connection ended before READY ({e})"),
                }
            }
            Err(e) => tracing::debug!("socket vanished before connect: {e}"),
        }
        // debounce so a crash-looping Discord can't spin us
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };
    tracing::info!("connected to Discord as user {user}");

    // --- input: the Steam virtual keyboard (needs a game session) ---
    let mut input = EvdevInput::open().map_err(|e| anyhow::anyhow!("input: {e}"))?;
    let intents = input.intents(); // 'static stream; `input` still grabs by fd

    // --- render: the gamescope external-overlay window ---
    let render = X11Overlay::new(settings.clone());

    // --- engine + dispatcher ---
    let engine = MenuEngine::new(&cfg);
    let (voice_tx, mut voice_rx) = mpsc::channel(256);
    let rpc_closed = rpc.closed();
    let mut disp = Dispatcher::new(engine, rpc, input, render, settings.clone());
    disp.set_voice_sink(voice_tx);

    tracing::info!("couchcord running — press the chord to open the menu");
    tokio::pin!(intents);
    tokio::pin!(rpc_closed);
    loop {
        tokio::select! {
            // Discord went away (quit, crash, or the tenant guard closed it on
            // a profile switch). Exit; systemd restarts us into a fresh
            // event-driven connect — waiting on a dead handle helps no one.
            _ = &mut rpc_closed => {
                return Err(anyhow::anyhow!("discord connection lost — restarting to reconnect"));
            }
            maybe = intents.next() => match maybe {
                Some(intent) => disp.on_input(intent).await,
                None => break, // input device gone
            },
            Some(ev) = voice_rx.recv() => disp.on_discord(ev).await,
        }
    }
    Ok(())
}

/// Block until a `discord-ipc-N` socket exists under `$XDG_RUNTIME_DIR` (native
/// path or the flatpak/snap export subdirs) — event-driven via inotify rather
/// than polling. Returns as soon as a socket is present or a creation event
/// lands in a watched dir; the caller loops, so a spurious wake just re-arms.
/// A long defensive timeout guards against a missed event (e.g. the flatpak
/// subdir appearing between our existence check and watch registration).
async fn wait_for_discord_socket() {
    use futures_util::StreamExt;
    use inotify::{Inotify, WatchMask};

    const ROOTS: [&str; 3] = ["", "app/com.discordapp.Discord", "snap.discord"];
    let runtime =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());

    let socket_present = |runtime: &str| {
        ROOTS.iter().any(|root| {
            (0..10).any(|n| {
                std::path::Path::new(runtime)
                    .join(root)
                    .join(format!("discord-ipc-{n}"))
                    .exists()
            })
        })
    };

    loop {
        if socket_present(&runtime) {
            return;
        }
        let Ok(inotify) = Inotify::init() else {
            // no inotify: degrade to a slow sleep rather than a tight loop
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            continue;
        };
        for root in ROOTS {
            let dir = std::path::Path::new(&runtime).join(root);
            if dir.is_dir() {
                // CREATE covers the socket itself and new export subdirs
                let _ = inotify.watches().add(&dir, WatchMask::CREATE);
            }
        }
        // re-check after arming: the socket may have appeared in between
        if socket_present(&runtime) {
            return;
        }
        let Ok(mut events) = inotify.into_event_stream([0u8; 4096]) else {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            continue;
        };
        // any creation event (or the defensive timeout) sends us around the
        // loop to re-check and re-arm — new subdirs get watched on re-entry
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(600),
            events.next(),
        )
        .await;
    }
}
