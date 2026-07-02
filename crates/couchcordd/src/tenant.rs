//! Multi-tenant support: one Deck/HTPC, several Steam accounts.
//!
//! Steam profiles on a shared device all run as the same Linux user, so any
//! state couchcord persists per Discord login (token cache, per-user prefs)
//! must be namespaced by the *Steam account* that launched the session, not
//! by `$HOME`. Steam flips `"MostRecent" "1"` in `loginusers.vdf` on every
//! profile login, which makes it the reliable at-launch signal (this client
//! era does not write `ActiveUser` to `registry.vdf`).

use std::path::PathBuf;

const STEAMID64_BASE: u64 = 76561197960265728;

/// The active Steam tenant on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    /// 32-bit account id (the `userdata/<id>` directory name).
    pub account_id: u32,
}

impl Tenant {
    /// Per-tenant state directory (token cache, future per-user prefs):
    /// `$XDG_STATE_HOME/couchcord/tenants/<account_id>` (default
    /// `~/.local/state/...`).
    pub fn state_dir(&self) -> PathBuf {
        state_root().join("tenants").join(self.account_id.to_string())
    }

    /// Create the state dir if missing; best-effort, returns it either way.
    pub fn ensure_state_dir(&self) -> PathBuf {
        let dir = self.state_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}

fn state_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("couchcord");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/state/couchcord")
}

/// Detect the active Steam account, or `None` off-Steam / before first login.
pub fn detect() -> Option<Tenant> {
    let path = if let Ok(p) = std::env::var("COUCHCORD_LOGINUSERS") {
        PathBuf::from(p)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".steam/steam/config/loginusers.vdf")
    };
    let text = std::fs::read_to_string(path).ok()?;
    parse_most_recent(&text).map(|steamid64| Tenant {
        account_id: (steamid64 - STEAMID64_BASE) as u32,
    })
}

/// Scan a `loginusers.vdf` for the steamid64 whose block has `"MostRecent" "1"`.
///
/// The format is sequential (`"7656…" { …fields… }`), so tracking the last
/// seen steamid is sufficient — no full VDF parser needed.
fn parse_most_recent(text: &str) -> Option<u64> {
    let mut current: Option<u64> = None;
    for line in text.lines() {
        let fields: Vec<&str> = line
            .split('"')
            .enumerate()
            .filter(|(i, _)| i % 2 == 1) // quoted spans only
            .map(|(_, s)| s)
            .collect();
        match fields.as_slice() {
            [id] => {
                if let Ok(n) = id.parse::<u64>() {
                    if n > STEAMID64_BASE {
                        current = Some(n);
                    }
                }
            }
            [key, value] if *key == "MostRecent" && *value == "1" => {
                if current.is_some() {
                    return current;
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
"users"
{
	"76561198177326773"
	{
		"AccountName"		"alpha"
		"MostRecent"		"0"
		"Timestamp"		"1782947639"
	}
	"76561198040675702"
	{
		"AccountName"		"bravo"
		"MostRecent"		"1"
		"Timestamp"		"1782966107"
	}
}
"#;

    #[test]
    fn picks_the_most_recent_block() {
        assert_eq!(parse_most_recent(SAMPLE), Some(76561198040675702));
    }

    #[test]
    fn account_id_conversion() {
        let t = Tenant {
            account_id: (76561198040675702u64 - STEAMID64_BASE) as u32,
        };
        assert_eq!(t.account_id, 80409974);
        assert!(t.state_dir().ends_with("tenants/80409974"));
    }

    #[test]
    fn none_when_no_most_recent() {
        assert_eq!(parse_most_recent("\"users\"\n{\n}\n"), None);
    }
}
