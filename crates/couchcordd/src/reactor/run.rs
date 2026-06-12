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

    // --- discord rpc: connect + authenticate (live) ---
    let rpc = DiscordRpc::connect_ipc(cfg.voice_kinds.clone())
        .await
        .map_err(|e| anyhow::anyhow!("discord ipc: {e}"))?;
    let user = rpc
        .connect(cfg.client_id)
        .await
        .map_err(|e| anyhow::anyhow!("discord auth: {e}"))?;
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
