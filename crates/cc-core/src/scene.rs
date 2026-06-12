//! The render `Scene` contract — the boundary between `cc-menu` (producer) and
//! `cc-render` (consumer). It lives in `cc-core` because both sides reference it
//! and neither may depend on the other.
//!
//! `menu` and `overlay` are **independent layers**: the voice-activity overlay
//! renders whenever connected, regardless of whether the menu is open
//! (ARCHITECTURE §2 correction 2).

use crate::ids::AssetHash;
use crate::model::{Anchor, VoiceMember};

/// An immutable, idempotent snapshot of what should be on screen. Dropped frames
/// self-heal because the next `Scene` is the complete truth.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Scene {
    /// The menu layer; `None` when dismissed.
    pub menu: Option<MenuView>,
    /// The always-on voice overlay; `Some` whenever connected to a channel.
    pub overlay: Option<Overlay>,
}

impl Scene {
    pub fn empty() -> Self {
        Scene::default()
    }
    /// True if nothing is drawn (no menu, no overlay) — the renderer can hide
    /// the window entirely.
    pub fn is_blank(&self) -> bool {
        self.menu.is_none() && self.overlay.is_none()
    }
}

/// A declarative menu screen: a title, rows, and which row is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuView {
    pub title: String,
    pub rows: Vec<Row>,
    pub selected: usize,
}

/// One selectable row. `icon` is an opaque asset hash the renderer resolves via
/// `cc-assets`; `None` → initials/placeholder tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub icon: Option<AssetHash>,
    pub state: RowState,
}

/// Visual state of a row, so the renderer can style without knowing semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RowState {
    Normal,
    /// A spinner / "loading…" row.
    Loading,
    /// The currently-connected channel, when shown in a list.
    Active,
    /// A back / leave affordance.
    Action,
}

/// The voice-activity overlay layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub anchor: Anchor,
    pub roster: Roster,
}

/// Who is in the connected channel right now.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Roster {
    pub channel_name: String,
    pub members: Vec<VoiceMember>,
}
