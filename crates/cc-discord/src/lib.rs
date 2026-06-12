//! `cc-discord` — the Discord **local RPC** boundary.
//!
//! Phase 1 implements the *pure* protocol: IPC frame encode/decode, command
//! building, response/event parsing, and — owned here, per ARCHITECTURE §2
//! correction 1 — the **voice-channel filter**. The live `UnixStream` connect +
//! OAuth handshake + reconnect (the `RpcClient` impl) is Phase 2, validated
//! against the real `discord-ipc-0` socket.
//!
//! Everything in `protocol` is syscall-free and exhaustively unit-tested.

pub mod protocol;

pub use protocol::{
    get_channels, get_guilds, handshake, parse_guilds, parse_speaking, parse_voice_channels,
    read_frame, select_voice, subscribe_voice_events, OP_CLOSE, OP_FRAME, OP_HANDSHAKE,
};
