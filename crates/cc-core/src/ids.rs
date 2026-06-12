//! Strongly-typed identifiers. Newtypes so a `GuildId` can never be passed
//! where a `ChannelId` is expected.

use serde::{Deserialize, Serialize};

macro_rules! snowflake_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub u64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<u64> for $name {
            fn from(v: u64) -> Self {
                Self(v)
            }
        }
    };
}

snowflake_id!(GuildId, "A Discord guild (server) snowflake.");
snowflake_id!(ChannelId, "A Discord channel snowflake.");
snowflake_id!(UserId, "A Discord user snowflake.");

/// The registered Discord *application* id (the OAuth client_id). Public,
/// non-secret. Used by the RPC handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientId(pub u64);

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An opaque Discord CDN asset hash (guild icon / user avatar). Resolved to
/// image bytes by `cc-assets`; the menu and renderer treat it as opaque.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetHash(pub String);

impl AssetHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
