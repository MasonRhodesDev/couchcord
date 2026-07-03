//! The shared `Config` snapshot type. `cc-config` loads/validates it; everyone
//! else reads it. Defined here because it crosses the `ConfigSource` boundary.

use crate::ids::ClientId;
use crate::model::{Anchor, VoiceKind};
use serde::{Deserialize, Serialize};

/// An immutable configuration snapshot. Read via `ConfigSource::current()`
/// (an `ArcSwap`), never sent on a message channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// The registered Discord application id.
    pub client_id: ClientId,
    /// Default overlay anchor at startup.
    #[serde(default = "default_anchor")]
    pub anchor: Anchor,
    /// Which channel kinds count as "voice" for the browser. Defaults to both
    /// regular and stage voice (Stage is in v1).
    #[serde(default = "default_voice_kinds")]
    pub voice_kinds: Vec<VoiceKind>,
    /// Theme knobs the renderer reads.
    #[serde(default)]
    pub theme: Theme,
}

impl Config {
    /// Whether a given voice kind is browsable under this config.
    pub fn accepts(&self, kind: VoiceKind) -> bool {
        self.voice_kinds.contains(&kind)
    }
}

/// Steam-client-styled defaults (dark, blue accent).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub bg: String,
    pub fg: String,
    pub accent: String,
    pub muted: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            bg: "#171a21".into(), // Steam dark
            fg: "#c7d5e0".into(),
            accent: "#66c0f4".into(), // Steam blue
            muted: "#8f98a0".into(),
        }
    }
}

fn default_anchor() -> Anchor {
    Anchor::TopRight
}

fn default_voice_kinds() -> Vec<VoiceKind> {
    vec![VoiceKind::Guild, VoiceKind::Stage]
}
