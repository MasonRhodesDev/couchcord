//! couchcord shared vocabulary.
//!
//! This crate is the *only* thing every domain crate may depend on, and it
//! depends on nothing in the workspace. It holds value types, the domain-typed
//! message enums that cross boundaries, the boundary *traits*, and the render
//! `Scene` contract. **No logic, no IO** lives here — only vocabulary.
//!
//! Domain enums are deliberately split per-domain (`InputIntent`,
//! `DiscordCommand`, ...) and `#[non_exhaustive]` so a change in one domain
//! never appears in another domain's compile surface (the upgrade-isolation
//! property). See `docs/ARCHITECTURE.md` §3.

pub mod ids;
pub mod model;
pub mod msg;
pub mod scene;
pub mod config;
pub mod error;
pub mod traits;

pub use config::Config;
pub use error::{ConfigError, InputError, RenderError, RpcError};
pub use ids::{AssetHash, ChannelId, ClientId, GuildId, UserId};
pub use model::{Anchor, Guild, VoiceChannel, VoiceKind, VoiceMember};
pub use msg::{
    DiscordCommand, DiscordEvent, DisconnectReason, InputControl, InputIntent, VoiceEvent,
};
pub use scene::{MenuView, Overlay, Roster, Row, RowState, Scene};
pub use traits::{
    AssetKind, AssetStore, ConfigSource, ImageHandle, InputSource, NavGuard, OverlayRenderer,
    RpcClient,
};
