use std::path::{Path, PathBuf};

const STEAMID64_BASE: u64 = 76561197960265728;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamAccount(pub u32);

pub fn loginusers_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("COUCHCORD_LOGINUSERS").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".steam/steam/config/loginusers.vdf"))
}

pub fn active_account(loginusers: &Path) -> Option<SteamAccount> {
    let text = std::fs::read_to_string(loginusers).ok()?;
    parse_most_recent(&text)
}

fn parse_most_recent(text: &str) -> Option<SteamAccount> {
    let mut current = None;
    for line in text.lines() {
        let fields: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        match fields.as_slice() {
            [id] => {
                if let Some(account) = id
                    .parse::<u64>()
                    .ok()
                    .and_then(|n| n.checked_sub(STEAMID64_BASE))
                    .and_then(|n| u32::try_from(n).ok())
                {
                    current = Some(SteamAccount(account));
                }
            }
            ["MostRecent", "1"] if current.is_some() => return current,
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
	"76561197960266729"
	{
		"AccountName"		"alpha"
		"MostRecent"		"0"
	}
	"76561197960266730"
	{
		"AccountName"		"bravo"
		"MostRecent"		"1"
	}
}
"#;

    #[test]
    fn picks_the_most_recent_account() {
        assert_eq!(parse_most_recent(SAMPLE), Some(SteamAccount(1002)));
    }

    #[test]
    fn none_without_a_most_recent_login() {
        assert_eq!(parse_most_recent("\"users\"\n{\n}\n"), None);
    }

    #[test]
    fn reads_the_file_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loginusers.vdf");
        std::fs::write(&path, SAMPLE).unwrap();
        assert_eq!(active_account(&path), Some(SteamAccount(1002)));
        assert_eq!(active_account(&dir.path().join("missing.vdf")), None);
    }
}
