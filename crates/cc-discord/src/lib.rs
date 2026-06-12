//! `cc-discord` — the Discord **local RPC** boundary.
//!
//! `protocol` is the pure, syscall-free wire layer (framing, command building,
//! response/event parsing, and the voice-channel filter — ARCHITECTURE §2).
//! `client` is the live async actor implementing `RpcClient` over the
//! `discord-ipc-0` `UnixStream`; it is exercised by an in-process mock IPC
//! server in tests, with the real-Discord auth round-trip deferred to Phase 2
//! live-validation.

pub mod client;
pub mod protocol;

pub use client::DiscordRpc;
pub use protocol::{
    frame, frame_command, get_channels, get_guilds, handshake, parse_guilds, parse_speaking,
    parse_voice_channels, read_frame, select_voice, subscribe_voice_events, OP_CLOSE, OP_FRAME,
    OP_HANDSHAKE,
};
