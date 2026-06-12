//! `cc-assets` — resolve Discord CDN asset *hashes* to decoded image bytes.
//!
//! Phase 1 implements the **pure** parts: building the official
//! `cdn.discordapp.com` URL for a hash (criterion: official Discord API only,
//! network allowed) and an in-memory cache. The async HTTP fetch + decode (the
//! `AssetStore` impl) is Phase 4; it sits behind the trait, and a missing/failed
//! fetch yields `None` so the renderer draws an initials tile.

use cc_core::{AssetHash, AssetKind};
use std::collections::HashMap;
use std::sync::Mutex;

/// Build the official CDN URL for an asset. Animated hashes (`a_…`) are served
/// as GIF; everything else as PNG. `size` must be a power of two in 16..=4096.
pub fn cdn_url(kind: AssetKind, hash: &AssetHash, size: u32) -> String {
    let animated = hash.as_str().starts_with("a_");
    let ext = if animated { "gif" } else { "png" };
    let size = size.clamp(16, 4096);
    match kind {
        AssetKind::GuildIcon { guild } => {
            format!("https://cdn.discordapp.com/icons/{guild}/{}.{ext}?size={size}", hash.as_str())
        }
        AssetKind::UserAvatar { user } => {
            format!("https://cdn.discordapp.com/avatars/{user}/{}.{ext}?size={size}", hash.as_str())
        }
        _ => format!("https://cdn.discordapp.com/{}.{ext}?size={size}", hash.as_str()),
    }
}

/// Initials drawn when an asset is absent/unresolved (the placeholder fallback).
/// Pure helper the renderer uses so a missing icon is a graceful enhancement.
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// A tiny content cache keyed by CDN URL. The real `AssetStore` wraps this around
/// an HTTP client in Phase 4; here it's the unit-tested storage/eviction core.
pub struct AssetCache {
    map: Mutex<HashMap<String, Vec<u8>>>,
    cap: usize,
}

impl AssetCache {
    pub fn new(cap: usize) -> Self {
        AssetCache { map: Mutex::new(HashMap::new()), cap: cap.max(1) }
    }

    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        self.map.lock().unwrap().get(url).cloned()
    }

    /// Insert, evicting an arbitrary entry if over capacity (a real LRU is Phase 4;
    /// correctness of the cap is what we test here).
    pub fn put(&self, url: String, bytes: Vec<u8>) {
        let mut m = self.map.lock().unwrap();
        if m.len() >= self.cap && !m.contains_key(&url) {
            if let Some(k) = m.keys().next().cloned() {
                m.remove(&k);
            }
        }
        m.insert(url, bytes);
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_core::{GuildId, UserId};

    #[test]
    fn guild_icon_url_is_official_cdn() {
        let url = cdn_url(
            AssetKind::GuildIcon { guild: GuildId(123) },
            &AssetHash("abcdef".into()),
            64,
        );
        assert_eq!(url, "https://cdn.discordapp.com/icons/123/abcdef.png?size=64");
    }

    #[test]
    fn user_avatar_url_and_animated_extension() {
        let still = cdn_url(AssetKind::UserAvatar { user: UserId(9) }, &AssetHash("hash".into()), 32);
        assert_eq!(still, "https://cdn.discordapp.com/avatars/9/hash.png?size=32");
        let anim = cdn_url(AssetKind::UserAvatar { user: UserId(9) }, &AssetHash("a_hash".into()), 32);
        assert!(anim.ends_with("a_hash.gif?size=32"), "animated hash → gif");
    }

    #[test]
    fn size_is_clamped_to_valid_range() {
        let url = cdn_url(AssetKind::GuildIcon { guild: GuildId(1) }, &AssetHash("h".into()), 99999);
        assert!(url.ends_with("size=4096"));
        let url = cdn_url(AssetKind::GuildIcon { guild: GuildId(1) }, &AssetHash("h".into()), 1);
        assert!(url.ends_with("size=16"));
    }

    #[test]
    fn initials_takes_up_to_two_uppercase() {
        assert_eq!(initials("mason rhodes"), "MR");
        assert_eq!(initials("Friends"), "F");
        assert_eq!(initials("the cool kids club"), "TC");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn cache_stores_and_retrieves() {
        let c = AssetCache::new(8);
        assert!(c.get("u").is_none());
        c.put("u".into(), vec![1, 2, 3]);
        assert_eq!(c.get("u"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn cache_respects_capacity() {
        let c = AssetCache::new(2);
        c.put("a".into(), vec![0]);
        c.put("b".into(), vec![0]);
        c.put("c".into(), vec![0]); // evicts one
        assert_eq!(c.len(), 2, "never exceeds capacity");
        // updating an existing key doesn't evict
        c.put("c".into(), vec![1]);
        assert_eq!(c.len(), 2);
        assert_eq!(c.get("c"), Some(vec![1]));
    }
}
