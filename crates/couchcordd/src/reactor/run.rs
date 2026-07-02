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

    // --- discord rpc: connect + authenticate (live) ---
    // At session start we come up before Discord does (the unit is bound to
    // graphical-session.target), so wait for the socket instead of exiting —
    // exiting turns into a systemd restart-loop that hits the start limit
    // before Discord ever appears.
    //
    // Auth failures with a live socket usually mean Discord is booting or the
    // current profile simply isn't signed in (multi-tenant device: some
    // profiles may never use Discord). Retry fast only briefly, then settle
    // into a slow heartbeat with a single log line — waiting must be near-free
    // and must not spam the journal forever.
    let (rpc, user) = {
        let mut auth_failures: u32 = 0;
        loop {
            match DiscordRpc::connect_ipc(cfg.voice_kinds.clone()).await {
                Ok(rpc) => match rpc.connect(cfg.client_id).await {
                    Ok(user) => break (rpc, user),
                    Err(e) => {
                        auth_failures += 1;
                        match auth_failures {
                            1 => tracing::warn!("discord auth failed: {e}; retrying"),
                            6 => tracing::info!(
                                "Discord is running but not authenticating (not signed in?) — \
                                 backing off to a 2-minute heartbeat"
                            ),
                            _ => tracing::debug!("discord auth failed: {e}"),
                        }
                    }
                },
                Err(e) => {
                    // socket gone: Discord exited/restarted — a sign-in often
                    // comes with a fresh launch, so return to fast retries
                    if auth_failures >= 6 {
                        tracing::info!("Discord socket gone — resuming fast retries");
                    }
                    auth_failures = 0;
                    tracing::debug!("waiting for Discord: {e}");
                }
            }
            let delay = match auth_failures {
                0..=5 => 5,    // booting / just launched
                6..=9 => 30,   // probably not signed in
                _ => 120,      // dormant heartbeat
            };
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
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
    let mut disp = Dispatcher::new(engine, rpc, input, render, settings.clone());
    disp.set_voice_sink(voice_tx);

    tracing::info!("couchcord running — press the chord to open the menu");
    tokio::pin!(intents);
    loop {
        tokio::select! {
            maybe = intents.next() => match maybe {
                Some(intent) => disp.on_input(intent).await,
                None => break, // input device gone
            },
            Some(ev) = voice_rx.recv() => disp.on_discord(ev).await,
        }
    }
    Ok(())
}
