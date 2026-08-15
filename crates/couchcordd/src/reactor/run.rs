//! The live run loop: compose the real boundary impls and drive the reactor.
//!
//! This is the one place concrete impls are named. It is not unit-tested (it
//! needs Discord running, a controller via Steam Input, and the gamescope
//! session); the logic it drives — `Dispatcher` — is mock-tested in `tests.rs`.

use super::Dispatcher;
use cc_config::Settings;
use cc_core::{Config, ConfigSource, InputSource, RpcClient};
use cc_discord::DiscordRpc;
use cc_input::EvdevInput;
use cc_menu::MenuEngine;
use cc_render::X11Overlay;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(BACKOFF_MAX)
}

/// Load config once, then reconnect evdev/RPC with backoff when either edge dies.
pub async fn run_live() -> anyhow::Result<()> {
    let path = cc_config::default_path();
    let cfg =
        cc_config::load(&path).map_err(|e| anyhow::anyhow!("config ({}): {e}", path.display()))?;
    let settings: Arc<dyn ConfigSource> = Arc::new(Settings::new(cfg.clone(), Some(path)));

    let mut delay = BACKOFF_START;
    loop {
        match run_session(&cfg, settings.clone()).await {
            Ok(()) => {
                delay = BACKOFF_START;
                tracing::warn!("evdev ended; reconnecting in {delay:?}");
            }
            Err(e) => {
                tracing::warn!("{e}; reconnecting in {delay:?}");
            }
        }
        tokio::time::sleep(delay).await;
        delay = next_backoff(delay);
    }
}

/// One connected session. Returns `Ok` when the evdev stream ends; `Err` on
/// RPC connect/close or evdev open failure so the outer loop can rediscover.
async fn run_session(cfg: &Config, settings: Arc<dyn ConfigSource>) -> anyhow::Result<()> {
    let rpc = DiscordRpc::connect_ipc(cfg.voice_kinds.clone())
        .await
        .map_err(|e| anyhow::anyhow!("discord ipc: {e}"))?;
    let user = rpc
        .connect(cfg.client_id)
        .await
        .map_err(|e| anyhow::anyhow!("discord auth: {e}"))?;
    tracing::info!("connected to Discord as user {user}");
    let rpc_closed = rpc.clone();

    let mut input = EvdevInput::open().map_err(|e| anyhow::anyhow!("input: {e}"))?;
    let intents = input.intents(); // 'static stream; `input` still grabs by fd

    let render = X11Overlay::new(settings.clone());

    let engine = MenuEngine::new(cfg);
    let (voice_tx, mut voice_rx) = mpsc::channel(256);
    let mut disp = Dispatcher::new(engine, rpc, input, render, settings);
    disp.set_voice_sink(voice_tx);

    tracing::info!("couchcord running — press the chord to open the menu");
    tokio::pin!(intents);
    loop {
        tokio::select! {
            maybe = intents.next() => match maybe {
                Some(intent) => disp.on_input(intent).await,
                None => return Ok(()),
            },
            Some(ev) = voice_rx.recv() => disp.on_discord(ev).await,
            () = rpc_closed.closed() => {
                return Err(anyhow::anyhow!("discord rpc closed"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(next_backoff(BACKOFF_START), Duration::from_secs(1));
        assert_eq!(next_backoff(Duration::from_secs(16)), BACKOFF_MAX);
        assert_eq!(next_backoff(BACKOFF_MAX), BACKOFF_MAX);
    }
}
