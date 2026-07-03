//! The live Discord local-RPC client: an async actor owning the `UnixStream`,
//! correlating request/response by `nonce`, and fanning voice dispatch events
//! out to per-channel subscribers.
//!
//! Auth note: the RPC `AUTHORIZE → exchange → AUTHENTICATE` dance needs the
//! app's OAuth credentials. We do the standard flow with the user's own app
//! (`client_id` + `client_secret` from config), caching the access token. That
//! exchange (a REST call) is wired in Phase 2 live-validation; the socket actor,
//! framing, nonce correlation, and event routing below are exercised now by an
//! in-process mock IPC server (see tests).

use crate::protocol::{self, OP_FRAME, OP_HANDSHAKE};
use cc_core::{
    ChannelId, ClientId, Guild, GuildId, RpcClient, RpcError, UserId, VoiceChannel, VoiceEvent,
    VoiceKind, VoiceMember,
};
use futures_core::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

/// A pending command: write these bytes, expect a frame with this `nonce`.
struct Request {
    bytes: Vec<u8>,
    nonce: String,
    reply: oneshot::Sender<Value>,
}

/// Handle to the running actor. Cloneable; cheap.
#[derive(Clone)]
pub struct DiscordRpc {
    cmd_tx: mpsc::Sender<Request>,
    events: broadcast::Sender<VoiceEvent>,
    nonce: Arc<AtomicU64>,
    accept: Arc<Vec<VoiceKind>>,
}

impl DiscordRpc {
    /// Connect to the first available `discord-ipc-N` socket under
    /// `$XDG_RUNTIME_DIR` and spawn the IO actor. Returns the handle.
    pub async fn connect_ipc(accept: Vec<VoiceKind>) -> Result<Self, RpcError> {
        let stream = open_ipc().await?;
        Ok(Self::with_stream(stream, accept))
    }

    /// Build a client over an already-connected stream (used by the mock test).
    pub fn with_stream(stream: UnixStream, accept: Vec<VoiceKind>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Request>(64);
        let (events, _) = broadcast::channel::<VoiceEvent>(256);
        let actor_events = events.clone();
        tokio::spawn(async move {
            if let Err(e) = run_actor(stream, cmd_rx, actor_events).await {
                tracing::warn!("discord rpc actor exited: {e}");
            }
        });
        DiscordRpc {
            cmd_tx,
            events,
            nonce: Arc::new(AtomicU64::new(1)),
            accept: Arc::new(accept),
        }
    }

    fn next_nonce(&self) -> String {
        format!("cc-{}", self.nonce.fetch_add(1, Ordering::Relaxed))
    }

    /// Send a framed command and await the matching-nonce reply's `data`.
    async fn request(&self, build: impl FnOnce(&str) -> Vec<u8>) -> Result<Value, RpcError> {
        let nonce = self.next_nonce();
        let bytes = build(&nonce);
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Request {
                bytes,
                nonce,
                reply: tx,
            })
            .await
            .map_err(|_| RpcError::new("rpc actor gone"))?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .map_err(|_| RpcError::new("rpc request timed out"))?
            .map_err(|_| RpcError::new("rpc reply dropped"))?;
        if let Some(err) = reply.get("evt").and_then(Value::as_str) {
            if err == "ERROR" {
                let msg = reply
                    .get("data")
                    .and_then(|d| d.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(RpcError::new(format!("discord error: {msg}")));
            }
        }
        Ok(reply.get("data").cloned().unwrap_or(Value::Null))
    }
}

#[async_trait::async_trait]
impl RpcClient for DiscordRpc {
    async fn connect(&self, app: ClientId) -> Result<UserId, RpcError> {
        // Handshake is op 0, no nonce — handled specially by the actor at start.
        // Here we (re)assert identity and read READY's user. The actor replies to
        // the synthetic "__handshake__" correlation.
        let data = self.request(|_| protocol::handshake(app)).await?;
        let uid = data
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| RpcError::new("handshake READY missing user id"))?;
        Ok(UserId(uid))
    }

    async fn guilds(&self) -> Result<Vec<Guild>, RpcError> {
        let data = self.request(protocol::get_guilds).await?;
        Ok(protocol::parse_guilds(&data))
    }

    async fn voice_channels(&self, guild: GuildId) -> Result<Vec<VoiceChannel>, RpcError> {
        let data = self.request(|n| protocol::get_channels(guild, n)).await?;
        Ok(protocol::parse_voice_channels(&data, &self.accept))
    }

    async fn select_voice(&self, channel: Option<ChannelId>) -> Result<(), RpcError> {
        self.request(|n| protocol::select_voice(channel, n)).await?;
        Ok(())
    }

    async fn selected_voice(&self) -> Result<Option<ChannelId>, RpcError> {
        let data = self
            .request(|n| {
                protocol::frame_command("GET_SELECTED_VOICE_CHANNEL", serde_json::json!({}), n)
            })
            .await?;
        Ok(data
            .get("id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u64>().ok())
            .map(ChannelId))
    }

    fn subscribe_voice(&self, channel: ChannelId) -> BoxStream<'static, VoiceEvent> {
        use futures_util::StreamExt;
        // Best-effort SUBSCRIBE for the relevant events.
        let cmd_tx = self.cmd_tx.clone();
        let nonce = Arc::clone(&self.nonce);
        tokio::spawn(async move {
            for evt in [
                "VOICE_STATE_CREATE",
                "VOICE_STATE_UPDATE",
                "VOICE_STATE_DELETE",
                "SPEAKING_START",
                "SPEAKING_STOP",
            ] {
                let n = format!("sub-{}", nonce.fetch_add(1, Ordering::Relaxed));
                let bytes = protocol::subscribe_voice_events(channel, evt, &n);
                let (tx, _rx) = oneshot::channel();
                let _ = cmd_tx
                    .send(Request {
                        bytes,
                        nonce: n,
                        reply: tx,
                    })
                    .await;
            }
        });
        let rx = self.events.subscribe();
        let stream = tokio_stream_from(rx).filter(move |e| {
            let keep = matches!(
                e,
                VoiceEvent::Members { channel: c, .. } | VoiceEvent::SpeakingChanged { channel: c, .. } if *c == channel
            );
            async move { keep }
        });
        stream.boxed()
    }
}

/// Adapt a broadcast receiver into a Stream (skipping lagged errors).
fn tokio_stream_from(
    mut rx: broadcast::Receiver<VoiceEvent>,
) -> impl futures_core::Stream<Item = VoiceEvent> {
    async_stream(move |yielder| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => yielder.send(ev).await,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

// A tiny async-stream shim so we don't pull the `async-stream` crate.
fn async_stream<T, F, Fut>(f: F) -> impl futures_core::Stream<Item = T>
where
    F: FnOnce(Yielder<T>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel::<T>(64);
    tokio::spawn(f(Yielder { tx }));
    futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

struct Yielder<T> {
    tx: mpsc::Sender<T>,
}
impl<T> Yielder<T> {
    async fn send(&self, item: T) {
        let _ = self.tx.send(item).await;
    }
}

/// Open the first live `discord-ipc-N` socket.
async fn open_ipc() -> Result<UnixStream, RpcError> {
    let runtime =
        std::env::var("XDG_RUNTIME_DIR").map_err(|_| RpcError::new("XDG_RUNTIME_DIR not set"))?;
    for n in 0..10 {
        let path = PathBuf::from(&runtime).join(format!("discord-ipc-{n}"));
        if path.exists() {
            if let Ok(s) = UnixStream::connect(&path).await {
                return Ok(s);
            }
        }
    }
    Err(RpcError::new(
        "no connectable discord-ipc-N socket (is Discord running?)",
    ))
}

/// The IO actor: owns the stream, writes commands, reads frames, correlates by
/// nonce, and routes dispatch events to the broadcast.
async fn run_actor(
    stream: UnixStream,
    mut cmd_rx: mpsc::Receiver<Request>,
    events: broadcast::Sender<VoiceEvent>,
) -> Result<(), RpcError> {
    let (mut rd, wr) = stream.into_split();
    let wr = Arc::new(Mutex::new(wr));
    // nonce → waiting reply. "__handshake__" is the synthetic correlation for the
    // op-0 READY dispatch (which carries no nonce).
    let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Reader loop task.
    let r_pending = Arc::clone(&pending);
    let r_events = events.clone();
    let reader = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut tmp = [0u8; 4096];
        loop {
            // try to drain complete frames first
            loop {
                match protocol::read_frame(&buf) {
                    Ok(Some((op, val, consumed))) => {
                        buf.drain(0..consumed);
                        route_frame(op, val, &r_pending, &r_events).await;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        buf.clear();
                        break;
                    }
                }
            }
            let n = match rd.read(&mut tmp).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            buf.extend_from_slice(&tmp[..n]);
        }
    });

    // Writer loop: take commands, register the nonce, write the frame.
    while let Some(req) = cmd_rx.recv().await {
        // op-0 handshake frames carry no nonce; correlate them synthetically.
        let key = if is_handshake(&req.bytes) {
            "__handshake__".to_string()
        } else {
            req.nonce.clone()
        };
        pending.lock().await.insert(key, req.reply);
        let mut w = wr.lock().await;
        if w.write_all(&req.bytes).await.is_err() {
            break;
        }
        let _ = w.flush().await;
    }
    reader.abort();
    Ok(())
}

fn is_handshake(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && u32::from_le_bytes(bytes[0..4].try_into().unwrap()) == OP_HANDSHAKE
}

/// Route one decoded frame: complete a pending request, or emit voice events.
async fn route_frame(
    op: u32,
    val: Value,
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    events: &broadcast::Sender<VoiceEvent>,
) {
    // READY dispatch (handshake response) has cmd=DISPATCH, evt=READY, no nonce.
    let evt = val.get("evt").and_then(Value::as_str);
    let nonce = val.get("nonce").and_then(Value::as_str);

    if op == OP_FRAME && evt == Some("READY") && nonce.is_none() {
        if let Some(tx) = pending.lock().await.remove("__handshake__") {
            let _ = tx.send(val);
        }
        return;
    }

    // Voice dispatch events (subscribed) carry an evt + data but no nonce.
    if let (Some(evt), None) = (evt, nonce) {
        if let Some(ve) = parse_voice_event(evt, val.get("data").unwrap_or(&Value::Null)) {
            let _ = events.send(ve);
            return;
        }
    }

    // Otherwise: a command reply correlated by nonce.
    if let Some(nonce) = nonce {
        if let Some(tx) = pending.lock().await.remove(nonce) {
            let _ = tx.send(val);
        }
    }
}

/// Map a dispatch event + data into a `VoiceEvent`, or `None` if unrelated.
fn parse_voice_event(evt: &str, data: &Value) -> Option<VoiceEvent> {
    if let Some((channel, user, speaking)) = protocol::parse_speaking(evt, data) {
        return Some(VoiceEvent::SpeakingChanged {
            channel,
            user,
            speaking,
        });
    }
    // VOICE_STATE_* carry a single member; surface as a one-member roster delta.
    if evt.starts_with("VOICE_STATE_") {
        let channel = data
            .get("channel_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok());
        let user = data
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u64>().ok());
        if let (Some(channel), Some(user)) = (channel, user) {
            let name = data
                .get("nick")
                .and_then(Value::as_str)
                .or_else(|| {
                    data.get("user")
                        .and_then(|u| u.get("username"))
                        .and_then(Value::as_str)
                })
                .unwrap_or("")
                .to_string();
            let member = VoiceMember {
                user: UserId(user),
                name,
                avatar: None,
                speaking: false,
                muted: data
                    .get("voice_state")
                    .and_then(|v| v.get("mute"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                deafened: data
                    .get("voice_state")
                    .and_then(|v| v.get("deaf"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            return Some(VoiceEvent::Members {
                channel: ChannelId(channel),
                members: vec![member],
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A minimal mock Discord IPC server over a socketpair: completes the
    // handshake and answers GET_GUILDS / SELECT_VOICE_CHANNEL, then emits a
    // SPEAKING_START dispatch. Proves framing, nonce correlation, and event
    // routing against a real socket — without the real Discord.
    async fn mock_server(mut s: UnixStream) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];

        let reply = |op: u32, v: Value| protocol::frame(op, &v);

        loop {
            // read at least one frame
            let frame = loop {
                if let Ok(Some((op, v, c))) = protocol::read_frame(&buf) {
                    buf.drain(0..c);
                    break Some((op, v));
                }
                match s.read(&mut tmp).await {
                    Ok(0) => break None,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    Err(_) => break None,
                }
            };
            let Some((op, v)) = frame else { break };

            if op == OP_HANDSHAKE {
                // READY dispatch, no nonce
                let ready = json!({"cmd":"DISPATCH","evt":"READY","data":{"user":{"id":"4242"}}});
                let _ = s.write_all(&reply(OP_FRAME, ready)).await;
                continue;
            }
            let nonce = v
                .get("nonce")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match v.get("cmd").and_then(Value::as_str) {
                Some("GET_GUILDS") => {
                    let data = json!({"guilds":[{"id":"1","name":"Friends"}]});
                    let _ = s
                        .write_all(&reply(
                            OP_FRAME,
                            json!({"cmd":"GET_GUILDS","data":data,"nonce":nonce}),
                        ))
                        .await;
                }
                Some("SELECT_VOICE_CHANNEL") => {
                    let _ = s
                        .write_all(&reply(
                            OP_FRAME,
                            json!({"cmd":"SELECT_VOICE_CHANNEL","data":json!({}),"nonce":nonce}),
                        ))
                        .await;
                    // then emit a speaking event (no nonce) for channel 99
                    let sp = json!({"cmd":"DISPATCH","evt":"SPEAKING_START","data":{"channel_id":"99","user_id":"7"}});
                    let _ = s.write_all(&reply(OP_FRAME, sp)).await;
                }
                Some("SUBSCRIBE") => {
                    let _ = s
                        .write_all(&reply(
                            OP_FRAME,
                            json!({"cmd":"SUBSCRIBE","data":json!({}),"nonce":nonce}),
                        ))
                        .await;
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn handshake_guilds_select_and_speaking_over_socket() {
        let (client_side, server_side) = UnixStream::pair().unwrap();
        tokio::spawn(mock_server(server_side));
        let rpc = DiscordRpc::with_stream(client_side, vec![VoiceKind::Guild, VoiceKind::Stage]);

        // handshake → READY user id
        let uid = rpc.connect(ClientId(1514871580591919246)).await.unwrap();
        assert_eq!(uid, UserId(4242));

        // GET_GUILDS round-trips and parses
        let guilds = rpc.guilds().await.unwrap();
        assert_eq!(guilds.len(), 1);
        assert_eq!(guilds[0].name, "Friends");

        // subscribe to channel 99, then SELECT triggers a SPEAKING_START dispatch
        use futures_util::StreamExt;
        let mut stream = rpc.subscribe_voice(ChannelId(99));
        rpc.select_voice(Some(ChannelId(99))).await.unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("a voice event should arrive")
            .expect("stream not closed");
        assert!(matches!(
            ev,
            VoiceEvent::SpeakingChanged {
                channel: ChannelId(99),
                user: UserId(7),
                speaking: true
            }
        ));
    }
}
