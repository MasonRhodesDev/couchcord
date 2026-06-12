//! `cc-config` — load + validate the TOML config, and provide the `ConfigSource`
//! (an `ArcSwap` snapshot + `store_anchor` persistence). Config is read via a
//! cheap atomic snapshot, never sent on a message channel (ARCHITECTURE §6.4).

use arc_swap::ArcSwap;
use cc_core::{Anchor, Config, ConfigError, ConfigSource};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Parse + validate a config from TOML text. Pure — the unit-tested core.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let cfg: Config = toml::from_str(text).map_err(|e| ConfigError::new(format!("parse: {e}")))?;
    validate(&cfg)?;
    Ok(cfg)
}

fn validate(cfg: &Config) -> Result<(), ConfigError> {
    if cfg.client_id.0 == 0 {
        return Err(ConfigError::new("client_id is required (register a Discord app)"));
    }
    if cfg.voice_kinds.is_empty() {
        return Err(ConfigError::new("voice_kinds must list at least one kind"));
    }
    Ok(())
}

/// Default config file path: `$XDG_CONFIG_HOME/couchcord/config.toml`.
pub fn default_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into())).join(".config")
    });
    base.join("couchcord").join("config.toml")
}

/// Load config from a path.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::new(format!("read {}: {e}", path.display())))?;
    parse(&text)
}

/// The runtime `ConfigSource`: an atomically-swappable snapshot plus best-effort
/// persistence of the chosen anchor back to the file.
pub struct Settings {
    current: ArcSwap<Config>,
    path: Option<PathBuf>,
}

impl Settings {
    pub fn new(cfg: Config, path: Option<PathBuf>) -> Self {
        Settings { current: ArcSwap::from_pointee(cfg), path }
    }

    /// In-memory only (tests / no persistence).
    pub fn in_memory(cfg: Config) -> Self {
        Settings::new(cfg, None)
    }
}

impl ConfigSource for Settings {
    fn current(&self) -> Arc<Config> {
        self.current.load_full()
    }

    fn store_anchor(&self, anchor: Anchor) {
        let mut next = (*self.current.load_full()).clone();
        next.anchor = anchor;
        self.current.store(Arc::new(next.clone()));
        // Best-effort persist (never panics the daemon on a write error).
        if let Some(path) = &self.path {
            if let Ok(text) = toml::to_string_pretty(&next) {
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
                let _ = std::fs::write(path, text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_core::VoiceKind;

    const FULL: &str = r#"
        client_id = 1514871580591919246
        anchor = "BottomLeft"
        voice_kinds = ["Guild", "Stage"]
    "#;

    #[test]
    fn parses_a_full_config() {
        let c = parse(FULL).unwrap();
        assert_eq!(c.client_id.0, 1514871580591919246);
        assert_eq!(c.anchor, Anchor::BottomLeft);
        assert!(c.accepts(VoiceKind::Guild) && c.accepts(VoiceKind::Stage));
    }

    #[test]
    fn applies_defaults_for_omitted_fields() {
        let c = parse("client_id = 42").unwrap();
        assert_eq!(c.anchor, Anchor::TopRight); // default
        assert_eq!(c.voice_kinds, vec![VoiceKind::Guild, VoiceKind::Stage]); // default: both
        assert_eq!(c.theme.accent, "#66c0f4"); // Steam blue default
    }

    #[test]
    fn missing_client_id_is_an_error() {
        assert!(parse("anchor = \"TopLeft\"").is_err());
        assert!(parse("client_id = 0").is_err()); // zero is invalid
    }

    #[test]
    fn empty_voice_kinds_rejected() {
        assert!(parse("client_id = 1\nvoice_kinds = []").is_err());
    }

    #[test]
    fn config_source_snapshot_and_store_anchor() {
        let cfg = parse("client_id = 7").unwrap();
        let s = Settings::in_memory(cfg);
        assert_eq!(s.current().anchor, Anchor::TopRight);
        s.store_anchor(Anchor::MidLeft);
        assert_eq!(s.current().anchor, Anchor::MidLeft, "snapshot reflects the stored anchor");
        assert_eq!(s.current().client_id.0, 7); // preserved across swap
    }

    #[test]
    fn round_trips_through_toml() {
        let c = parse(FULL).unwrap();
        let text = toml::to_string_pretty(&c).unwrap();
        let c2 = parse(&text).unwrap();
        assert_eq!(c, c2);
    }
}
