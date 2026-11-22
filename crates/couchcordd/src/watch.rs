use futures_util::StreamExt;
use inotify::{EventStream, Inotify, WatchMask};
use std::io;
use std::path::{Path, PathBuf};

pub struct DirWatch {
    events: EventStream<[u8; 1024]>,
    targets: Vec<PathBuf>,
}

impl DirWatch {
    pub fn arm(
        dirs: impl IntoIterator<Item = impl AsRef<Path>>,
        mask: WatchMask,
    ) -> io::Result<Self> {
        let mask = mask
            | WatchMask::CREATE
            | WatchMask::MOVED_TO
            | WatchMask::DELETE_SELF
            | WatchMask::MOVE_SELF;
        let inotify = Inotify::init()?;
        let mut targets = Vec::new();
        for dir in dirs {
            let dir = dir.as_ref();
            let target = dir.ancestors().find(|p| p.is_dir()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no existing ancestor of {}", dir.display()),
                )
            })?;
            inotify.watches().add(target, mask)?;
            if !targets.iter().any(|t| t == target) {
                targets.push(target.to_path_buf());
            }
        }
        Ok(DirWatch {
            events: inotify.into_event_stream([0; 1024])?,
            targets,
        })
    }

    pub fn describe(&self) -> String {
        self.targets
            .iter()
            .map(|t| t.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub async fn changed(&mut self) -> io::Result<()> {
        match self.events.next().await {
            Some(event) => event.map(drop),
            None => Err(io::Error::other("inotify stream ended")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const QUIET: Duration = Duration::from_millis(200);
    const WAKE: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn creating_a_missing_parent_wakes() {
        let root = tempfile::tempdir().unwrap();
        let mut watch =
            DirWatch::arm([root.path().join("app/com.example")], WatchMask::empty()).unwrap();
        assert_eq!(watch.describe(), root.path().display().to_string());
        assert!(tokio::time::timeout(QUIET, watch.changed()).await.is_err());
        std::fs::create_dir(root.path().join("app")).unwrap();
        tokio::time::timeout(WAKE, watch.changed())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn removing_a_watched_dir_wakes() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("export");
        std::fs::create_dir(&dir).unwrap();
        let mut watch = DirWatch::arm([&dir], WatchMask::empty()).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        tokio::time::timeout(WAKE, watch.changed())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn the_same_dir_listed_twice_is_one_target() {
        let root = tempfile::tempdir().unwrap();
        let watch = DirWatch::arm(
            [root.path().to_path_buf(), root.path().join("missing")],
            WatchMask::empty(),
        )
        .unwrap();
        assert_eq!(watch.describe(), root.path().display().to_string());
    }
}
