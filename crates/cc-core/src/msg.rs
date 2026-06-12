//! Domain-typed messages that cross boundaries. Split per-domain so a Discord
//! change never touches the input or render compile surface (ARCHITECTURE §3).

use crate::ids::{ChannelId, GuildId, UserId};
use crate::model::{Guild, VoiceChannel, VoiceMember};

/// INPUT domain: `cc-input` → `cc-menu`. Semantic, never keycodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputIntent {
    /// The chord that opens/toggles the menu.
    Chord,
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    /// Explicit dismiss (close menu, release input).
    Dismiss,
    /// Cycle the overlay through the 8 anchor positions.
    AnchorCycle,
}

/// INPUT control: `cc-menu` → `cc-input`. Logical capture of the virtual-kbd
/// nav keys while the menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputControl {
    Grab,
    Release,
}

/// DISCORD command: `cc-menu` → `cc-discord`. Domain verbs, never RPC opcodes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiscordCommand {
    Connect,
    ListGuilds,
    ListVoiceChannels { guild: GuildId },
    JoinVoice { channel: ChannelId },
    /// → `SELECT_VOICE_CHANNEL { channel_id: null }`.
    LeaveVoice,
    /// Per-CHANNEL subscription to voice-state + speaking events.
    SubscribeVoice { channel: ChannelId },
    UnsubscribeVoice { channel: ChannelId },
}

/// DISCORD event: `cc-discord` → `cc-menu`. Already-filtered, already-domain.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiscordEvent {
    Connected { user: UserId },
    Disconnected { reason: DisconnectReason },
    Guilds(Vec<Guild>),
    /// Channels here are already filtered to voice kinds by `cc-discord`.
    VoiceChannels { guild: GuildId, channels: Vec<VoiceChannel> },
    JoinedVoice { channel: ChannelId },
    LeftVoice,
    VoiceMembers { channel: ChannelId, members: Vec<VoiceMember> },
    SpeakingChanged { channel: ChannelId, user: UserId, speaking: bool },
}

/// Why the Discord connection went away — drives the recovery UX (showing
/// "start Discord" vs "reconnecting").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisconnectReason {
    /// No client / IPC socket — the user must start Discord.
    ClientNotRunning,
    SocketClosed,
    AuthFailed,
    Timeout,
}

/// The per-channel event stream item produced by `RpcClient::subscribe_voice`.
/// A thin alias over the subset of `DiscordEvent` that a voice subscription
/// emits, so the stream type names exactly what it can carry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VoiceEvent {
    Members { channel: ChannelId, members: Vec<VoiceMember> },
    SpeakingChanged { channel: ChannelId, user: UserId, speaking: bool },
}
