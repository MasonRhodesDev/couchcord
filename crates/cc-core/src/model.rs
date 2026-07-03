//! Domain value types — the nouns the whole app speaks in.

use crate::ids::{AssetHash, ChannelId, GuildId, UserId};
use serde::{Deserialize, Serialize};

/// A Discord guild (server) the user is in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guild {
    pub id: GuildId,
    pub name: String,
    pub icon: Option<AssetHash>,
}

/// A *voice* channel. By construction (filtered in `cc-discord`) this is only
/// ever `GUILD_VOICE` (type 2) or `STAGE_VOICE` (type 13) — the menu never sees
/// a text channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceChannel {
    pub id: ChannelId,
    pub name: String,
    pub kind: VoiceKind,
}

/// The two voice channel taxonomies we support. `#[non_exhaustive]` so adding a
/// future kind is additive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum VoiceKind {
    /// Discord `GUILD_VOICE`, channel `type == 2`.
    Guild,
    /// Discord `STAGE_VOICE`, channel `type == 13`. Speaker/audience model.
    Stage,
}

impl VoiceKind {
    /// Map a raw Discord channel `type` to a `VoiceKind`, or `None` if it is not
    /// a voice channel. This is the single source of truth for "what is a voice
    /// channel" and is exercised by `cc-discord`'s filter.
    pub fn from_discord_type(t: u8) -> Option<VoiceKind> {
        match t {
            2 => Some(VoiceKind::Guild),
            13 => Some(VoiceKind::Stage),
            _ => None,
        }
    }
}

/// A member currently in a voice channel, with live speaking/mute state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceMember {
    pub user: UserId,
    pub name: String,
    pub avatar: Option<AssetHash>,
    pub speaking: bool,
    pub muted: bool,
    pub deafened: bool,
}

/// The 8 overlay anchor positions (criterion 7): 4 corners + 4 edge midpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    MidLeft,
    MidRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    /// All 8 anchors in a stable cycle order (used by `InputIntent::AnchorCycle`
    /// and by geometry tests).
    pub const ALL: [Anchor; 8] = [
        Anchor::TopLeft,
        Anchor::TopCenter,
        Anchor::TopRight,
        Anchor::MidRight,
        Anchor::BottomRight,
        Anchor::BottomCenter,
        Anchor::BottomLeft,
        Anchor::MidLeft,
    ];

    /// The next anchor in `ALL` order, wrapping. Pure; unit-tested.
    pub fn next(self) -> Anchor {
        let i = Anchor::ALL.iter().position(|&a| a == self).unwrap_or(0);
        Anchor::ALL[(i + 1) % Anchor::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_kind_from_type() {
        assert_eq!(VoiceKind::from_discord_type(2), Some(VoiceKind::Guild));
        assert_eq!(VoiceKind::from_discord_type(13), Some(VoiceKind::Stage));
        for t in [0u8, 1, 3, 4, 5, 11, 12, 14, 15] {
            assert_eq!(
                VoiceKind::from_discord_type(t),
                None,
                "type {t} must not be voice"
            );
        }
    }

    #[test]
    fn anchor_cycle_visits_all_eight_and_wraps() {
        let mut a = Anchor::TopLeft;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            assert!(
                seen.insert(a),
                "anchor cycle repeated {a:?} before visiting all 8"
            );
            a = a.next();
        }
        assert_eq!(seen.len(), 8);
        assert_eq!(a, Anchor::TopLeft, "cycle of 8 must return to start");
    }
}
