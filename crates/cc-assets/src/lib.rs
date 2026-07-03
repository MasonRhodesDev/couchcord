//! `cc-assets` — resolve Discord CDN asset *hashes* to decoded image bytes.
//!
//! Phase 1 implements the **pure** parts: building the official
//! `cdn.discordapp.com` URL for a hash (criterion: official Discord API only,
//! network allowed) and an in-memory cache. The async HTTP fetch + decode (the
//! `AssetStore` impl) is Phase 4; it sits behind the trait, and a missing/failed
//! fetch yields `None` so the renderer draws an initials tile.

use cc_core::{AssetHash, AssetKind, AssetStore, ImageHandle};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Build the official CDN URL for an asset. Animated hashes (`a_…`) are served
/// as GIF; everything else as PNG. `size` must be a power of two in 16..=4096.
pub fn cdn_url(kind: AssetKind, hash: &AssetHash, size: u32) -> String {
    let animated = hash.as_str().starts_with("a_");
    let ext = if animated { "gif" } else { "png" };
    let size = size.clamp(16, 4096);
    match kind {
        AssetKind::GuildIcon { guild } => {
            format!(
                "https://cdn.discordapp.com/icons/{guild}/{}.{ext}?size={size}",
                hash.as_str()
            )
        }
        AssetKind::UserAvatar { user } => {
            format!(
                "https://cdn.discordapp.com/avatars/{user}/{}.{ext}?size={size}",
                hash.as_str()
            )
        }
        _ => format!(
            "https://cdn.discordapp.com/{}.{ext}?size={size}",
            hash.as_str()
        ),
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
        AssetCache {
            map: Mutex::new(HashMap::new()),
            cap: cap.max(1),
        }
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

/// Decode image bytes (PNG/GIF/JPEG/WebP) into an opaque RGBA `ImageHandle`.
/// Pure; unit-tested with an in-memory PNG.
pub fn decode_rgba(bytes: &[u8]) -> Option<ImageHandle> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(ImageHandle {
        width,
        height,
        rgba: Arc::new(rgba.into_raw()),
    })
}

/// The live `AssetStore`: fetch a hash from the official Discord CDN over HTTPS,
/// cache the bytes, decode to RGBA. Best-effort — any failure yields `None`, and
/// the renderer falls back to an initials tile.
pub struct CdnAssets {
    client: reqwest::Client,
    cache: AssetCache,
    size: u32,
}

impl CdnAssets {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("couchcord/0.1 (+https://github.com/MasonRhodesDev/couchcord)")
            .build()
            .unwrap_or_default();
        CdnAssets {
            client,
            cache: AssetCache::new(256),
            size: 64,
        }
    }
}

impl Default for CdnAssets {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AssetStore for CdnAssets {
    async fn resolve(&self, hash: &AssetHash, kind: AssetKind) -> Option<ImageHandle> {
        let url = cdn_url(kind, hash, self.size);
        if let Some(bytes) = self.cache.get(&url) {
            return decode_rgba(&bytes);
        }
        let resp = self.client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let bytes = resp.bytes().await.ok()?.to_vec();
        let handle = decode_rgba(&bytes)?; // only cache decodable bytes
        self.cache.put(url, bytes);
        Some(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_core::{GuildId, UserId};

    /// A 2×3 RGBA PNG, encoded in-memory, to exercise the decode path offline.
    fn tiny_png() -> Vec<u8> {
        use image::{ColorType, ImageEncoder};
        let pixels: Vec<u8> = vec![255u8; 2 * 3 * 4]; // 2x3, all opaque white
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(&pixels, 2, 3, ColorType::Rgba8.into())
            .unwrap();
        out
    }

    #[test]
    fn decode_png_to_rgba_handle() {
        let h = decode_rgba(&tiny_png()).expect("valid PNG decodes");
        assert_eq!((h.width, h.height), (2, 3));
        assert_eq!(h.rgba.len(), 2 * 3 * 4);
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_rgba(b"not an image").is_none());
    }

    #[test]
    fn guild_icon_url_is_official_cdn() {
        let url = cdn_url(
            AssetKind::GuildIcon {
                guild: GuildId(123),
            },
            &AssetHash("abcdef".into()),
            64,
        );
        assert_eq!(
            url,
            "https://cdn.discordapp.com/icons/123/abcdef.png?size=64"
        );
    }

    #[test]
    fn user_avatar_url_and_animated_extension() {
        let still = cdn_url(
            AssetKind::UserAvatar { user: UserId(9) },
            &AssetHash("hash".into()),
            32,
        );
        assert_eq!(
            still,
            "https://cdn.discordapp.com/avatars/9/hash.png?size=32"
        );
        let anim = cdn_url(
            AssetKind::UserAvatar { user: UserId(9) },
            &AssetHash("a_hash".into()),
            32,
        );
        assert!(anim.ends_with("a_hash.gif?size=32"), "animated hash → gif");
    }

    #[test]
    fn size_is_clamped_to_valid_range() {
        let url = cdn_url(
            AssetKind::GuildIcon { guild: GuildId(1) },
            &AssetHash("h".into()),
            99999,
        );
        assert!(url.ends_with("size=4096"));
        let url = cdn_url(
            AssetKind::GuildIcon { guild: GuildId(1) },
            &AssetHash("h".into()),
            1,
        );
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
