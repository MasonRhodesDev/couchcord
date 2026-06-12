//! The four boundary traits. Each is the *complete, sufficient* contract for its
//! domain — note what is absent: no JSON, no `xcb::Window`, no `evdev::Device`,
//! no socket. Implementations live in the sibling impl crates; only `couchcordd`
//! names the concrete impls.

use crate::config::Config;
use crate::error::{InputError, RenderError, RpcError};
use crate::ids::{AssetHash, ChannelId, ClientId, GuildId, UserId};
use crate::model::{Anchor, Guild, VoiceChannel};
use crate::msg::VoiceEvent;
use crate::scene::Scene;
use async_trait::async_trait;
use futures_core::stream::BoxStream;
use std::sync::Arc;

/// `cc-discord` boundary. Domain verbs in, domain events out — never RPC opcodes.
#[async_trait]
pub trait RpcClient: Send + Sync + 'static {
    async fn connect(&self, app: ClientId) -> Result<UserId, RpcError>;
    async fn guilds(&self) -> Result<Vec<Guild>, RpcError>;
    /// PRE-FILTERED to voice kinds here, once, in the domain that owns the
    /// taxonomy. The menu never sees a text channel.
    async fn voice_channels(&self, guild: GuildId) -> Result<Vec<VoiceChannel>, RpcError>;
    /// `None` = leave (`SELECT_VOICE_CHANNEL { channel_id: null }`).
    async fn select_voice(&self, channel: Option<ChannelId>) -> Result<(), RpcError>;
    async fn selected_voice(&self) -> Result<Option<ChannelId>, RpcError>;
    /// Long-lived, per-CHANNEL voice subscription.
    fn subscribe_voice(&self, channel: ChannelId) -> BoxStream<'static, VoiceEvent>;
}

/// `cc-input` boundary. Emits semantic intents; never a Discord type or a pixel.
pub trait InputSource: Send + 'static {
    fn intents(&mut self) -> BoxStream<'static, crate::msg::InputIntent>;
    /// RAII-scoped capture of the *virtual* keyboard's nav keys. Dropping the
    /// guard ALWAYS ungrabs — the soft-brick failsafe lives in the type.
    fn grab(&mut self) -> Result<NavGuard, InputError>;
}

/// A guard whose `Drop` runs the ungrab. `cc-input` constructs it with the
/// concrete ungrab closure; `cc-core` only provides the RAII mechanism (still no
/// IO of its own).
pub struct NavGuard {
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl NavGuard {
    pub fn new(on_drop: impl FnOnce() + Send + 'static) -> Self {
        NavGuard { on_drop: Some(Box::new(on_drop)) }
    }
    /// A guard that does nothing on drop (for tests / no-op input sources).
    pub fn noop() -> Self {
        NavGuard { on_drop: None }
    }
}

impl Drop for NavGuard {
    fn drop(&mut self) {
        if let Some(f) = self.on_drop.take() {
            f();
        }
    }
}

/// `cc-render` boundary — async because realize/draw do blocking X work off-task.
#[async_trait]
pub trait OverlayRenderer: Send + 'static {
    /// Discover the gamescope nested-X display + atom owner, retrying until present.
    async fn realize(&mut self) -> Result<(), RenderError>;
    /// Paint an immutable, idempotent `Scene` snapshot.
    async fn draw(&mut self, scene: &Scene) -> Result<(), RenderError>;
    /// Pure geometry; 8 cases; unit-tested without a live X server.
    fn set_anchor(&mut self, anchor: Anchor);
}

/// `cc-assets` boundary. Best-effort, cached; `None` → renderer draws initials.
#[async_trait]
pub trait AssetStore: Send + Sync + 'static {
    async fn resolve(&self, hash: &AssetHash, kind: AssetKind) -> Option<ImageHandle>;
}

/// Which CDN asset family a hash belongs to (sizes/paths differ).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AssetKind {
    GuildIcon { guild: GuildId },
    UserAvatar { user: UserId },
}

/// An opaque, decoded RGBA image handle. `cc-assets` produces it; `cc-render`
/// consumes it. Cheap to clone (ref-counted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageHandle {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// `cc-config` boundary — deliberately NOT a message channel.
pub trait ConfigSource: Send + Sync + 'static {
    fn current(&self) -> Arc<Config>;
    /// Persist the 8-position choice.
    fn store_anchor(&self, anchor: Anchor);
}
