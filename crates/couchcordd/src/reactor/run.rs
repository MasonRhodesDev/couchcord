//! The live run loop: compose the real boundary impls and drive the reactor.
//!
//! This is the one place concrete impls are named. The logic it drives,
//! `Dispatcher`, is mock-tested in `tests.rs`.

use super::Dispatcher;
use crate::watch::DirWatch;
use cc_config::Settings;
use cc_core::{ClientId, Config, ConfigSource, InputSource, RpcClient, UserId, VoiceKind};
use cc_discord::DiscordRpc;
use cc_input::EvdevInput;
use cc_menu::MenuEngine;
use cc_render::X11Overlay;
use futures_util::StreamExt;
use inotify::WatchMask;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

const INPUT_DIR: &str = "/dev/input";

#[derive(Debug, PartialEq, Eq)]
enum SessionEnd {
    InputGone,
    DiscordGone,
}

pub async fn run_live() -> anyhow::Result<()> {
    let path = cc_config::default_path();
    let cfg =
        cc_config::load(&path).map_err(|e| anyhow::anyhow!("config ({}): {e}", path.display()))?;
    let settings: Arc<dyn ConfigSource> = Arc::new(Settings::new(cfg.clone(), Some(path)));
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR is not set"))?;
    match crate::tenant::loginusers_path().and_then(|p| crate::tenant::active_account(&p)) {
        Some(account) => tracing::info!("active Steam account {}", account.0),
        None => tracing::info!("no Steam login found"),
    }

    loop {
        let (rpc, user) = connect_discord(&runtime, cfg.client_id, &cfg.voice_kinds).await?;
        tracing::info!("connected to Discord as user {user}");
        loop {
            let (input, input_watch) = tokio::select! {
                opened = open_input() => opened?,
                () = rpc.closed() => break,
            };
            let end = run_session(&cfg, settings.clone(), rpc.clone(), input).await;
            if end == SessionEnd::DiscordGone {
                break;
            }
            tracing::info!("input device gone");
            let mut input_watch = input_watch;
            tokio::select! {
                changed = input_watch.changed() => changed?,
                () = rpc.closed() => break,
            }
        }
        tracing::warn!("Discord connection lost");
    }
}

async fn connect_discord(
    runtime: &Path,
    client_id: ClientId,
    kinds: &[VoiceKind],
) -> anyhow::Result<(DiscordRpc, UserId)> {
    loop {
        let mut watch = DirWatch::arm(cc_discord::ipc_dirs(runtime), WatchMask::empty())
            .map_err(|e| anyhow::anyhow!("watch {}: {e}", runtime.display()))?;
        match DiscordRpc::connect_ipc(runtime, kinds.to_vec()).await {
            Ok(rpc) => {
                tracing::info!("Discord socket is up, waiting for READY (sign-in completes it)");
                match rpc.connect(client_id).await {
                    Ok(user) => return Ok((rpc, user)),
                    Err(e) => tracing::warn!("Discord handshake failed: {e}"),
                }
            }
            Err(e) => tracing::info!("{e}"),
        }
        tracing::info!("waiting for a new Discord socket in {}", watch.describe());
        watch
            .changed()
            .await
            .map_err(|e| anyhow::anyhow!("watch {}: {e}", runtime.display()))?;
    }
}

async fn open_input() -> anyhow::Result<(EvdevInput, DirWatch)> {
    loop {
        let mut watch = DirWatch::arm(
            [INPUT_DIR],
            WatchMask::ATTRIB | WatchMask::DELETE | WatchMask::MOVED_FROM,
        )
        .map_err(|e| anyhow::anyhow!("watch {INPUT_DIR}: {e}"))?;
        match EvdevInput::open() {
            Ok(input) => return Ok((input, watch)),
            Err(e) => tracing::info!("input: {e}; waiting for a change in {INPUT_DIR}"),
        }
        watch
            .changed()
            .await
            .map_err(|e| anyhow::anyhow!("watch {INPUT_DIR}: {e}"))?;
    }
}

async fn run_session(
    cfg: &Config,
    settings: Arc<dyn ConfigSource>,
    rpc: DiscordRpc,
    mut input: EvdevInput,
) -> SessionEnd {
    let rpc_closed = rpc.clone();
    let intents = input.intents();
    let render = X11Overlay::new(settings.clone());
    let engine = MenuEngine::new(cfg);
    let (voice_tx, mut voice_rx) = mpsc::channel(256);
    let mut disp = Dispatcher::new(engine, rpc, input, render, settings);
    disp.set_voice_sink(voice_tx);

    tracing::info!("couchcord is running. Press the chord to open the menu.");
    tokio::pin!(intents);
    loop {
        tokio::select! {
            maybe = intents.next() => match maybe {
                Some(intent) => disp.on_input(intent).await,
                None => return SessionEnd::InputGone,
            },
            Some(ev) = voice_rx.recv() => disp.on_discord(ev).await,
            () = rpc_closed.closed() => return SessionEnd::DiscordGone,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_discord::{frame, read_frame, OP_CLOSE, OP_FRAME, OP_HANDSHAKE};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};

    #[derive(Clone, Copy)]
    enum Reply {
        Ready,
        Reject,
    }

    fn serve(listener: UnixListener, reply: Reply, accepted: Arc<AtomicUsize>) {
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                accepted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(answer(stream, reply));
            }
        });
    }

    async fn answer(mut stream: UnixStream, reply: Reply) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            if let Ok(Some((op, _, _))) = read_frame(&buf) {
                if op != OP_HANDSHAKE {
                    return;
                }
                break;
            }
            match stream.read(&mut tmp).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
        match reply {
            Reply::Ready => {
                let ready = json!({"cmd":"DISPATCH","evt":"READY","data":{"user":{"id":"7"}}});
                let _ = stream.write_all(&frame(OP_FRAME, &ready)).await;
                let _ = stream.read(&mut tmp).await;
            }
            Reply::Reject => {
                let close = json!({"code":4000,"message":"Invalid Client ID"});
                let _ = stream.write_all(&frame(OP_CLOSE, &close)).await;
            }
        }
    }

    #[tokio::test]
    async fn rejected_handshake_waits_for_a_new_socket() {
        let runtime = tempfile::tempdir().unwrap();
        let socket = runtime.path().join("discord-ipc-0");
        let rejects = Arc::new(AtomicUsize::new(0));
        serve(
            UnixListener::bind(&socket).unwrap(),
            Reply::Reject,
            rejects.clone(),
        );

        let path = runtime.path().to_path_buf();
        let connect = tokio::spawn(async move { connect_discord(&path, ClientId(1), &[]).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!connect.is_finished());
        assert_eq!(rejects.load(Ordering::SeqCst), 1);

        std::fs::remove_file(&socket).unwrap();
        let readies = Arc::new(AtomicUsize::new(0));
        serve(
            UnixListener::bind(&socket).unwrap(),
            Reply::Ready,
            readies.clone(),
        );
        let (_rpc, user) = tokio::time::timeout(Duration::from_secs(5), connect)
            .await
            .expect("a recreated socket must wake the wait")
            .unwrap()
            .unwrap();
        assert_eq!(user, UserId(7));
        assert_eq!(rejects.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_flatpak_socket_appearing_later_wakes_the_wait() {
        let runtime = tempfile::tempdir().unwrap();
        let path = runtime.path().to_path_buf();
        let connect = tokio::spawn(async move { connect_discord(&path, ClientId(1), &[]).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!connect.is_finished());

        let export = runtime.path().join("app/com.discordapp.Discord");
        std::fs::create_dir_all(&export).unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        serve(
            UnixListener::bind(export.join("discord-ipc-0")).unwrap(),
            Reply::Ready,
            accepted.clone(),
        );
        let (_rpc, user) = tokio::time::timeout(Duration::from_secs(5), connect)
            .await
            .expect("socket creation under a new export dir must wake the wait")
            .unwrap()
            .unwrap();
        assert_eq!(user, UserId(7));
    }

    #[tokio::test]
    async fn a_rebound_socket_replaces_a_stale_one() {
        let runtime = tempfile::tempdir().unwrap();
        let socket = runtime.path().join("discord-ipc-0");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        let path = runtime.path().to_path_buf();
        let connect = tokio::spawn(async move { connect_discord(&path, ClientId(1), &[]).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!connect.is_finished());

        std::fs::remove_file(&socket).unwrap();
        serve(
            UnixListener::bind(&socket).unwrap(),
            Reply::Ready,
            Arc::new(AtomicUsize::new(0)),
        );
        let (_rpc, user) = tokio::time::timeout(Duration::from_secs(5), connect)
            .await
            .expect("a rebound socket must wake the wait")
            .unwrap()
            .unwrap();
        assert_eq!(user, UserId(7));
    }
}
