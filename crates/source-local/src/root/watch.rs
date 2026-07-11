use async_trait::async_trait;
use notify::{
    Event, EventKind, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};
use source::{LibraryChange, LibraryChangeFeed, SourceError, SourceObjectChanges, SourceResult};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tracing::warn;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEBOUNCE: Duration = Duration::from_secs(2);
const FAILED_ROOT_RETRY: Duration = Duration::from_secs(60);

pub struct LocalChangeFeed {
    roots: Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>,
}

impl LocalChangeFeed {
    pub fn new(roots: Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>) -> Self {
        Self { roots }
    }
}

enum FeedMessage {
    Event(Event),
    Failed(String),
}

#[async_trait(?Send)]
impl LibraryChangeFeed for LocalChangeFeed {
    async fn listen(
        &self,
        on_ready: &mut dyn FnMut() -> bool,
        on_change: &mut dyn FnMut(LibraryChange) -> bool,
        should_stop: &dyn Fn() -> bool,
    ) -> SourceResult<()> {
        let mut roots = loop {
            if should_stop() {
                return Ok(());
            }
            let mut roots = (self.roots)();
            roots.sort();
            roots.dedup();
            if !roots.is_empty() {
                break roots;
            }
            thread::sleep(POLL_INTERVAL);
        };

        let (messages, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            let message = match event {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    FeedMessage::Event(event)
                }
                Ok(_) => return,
                Err(error) => FeedMessage::Failed(error.to_string()),
            };
            let _sent = messages.send(message);
        })
        .map_err(feed_error)?;

        let mut watched = 0;
        let mut failed_roots = Vec::new();
        for root in roots.drain(..) {
            match watcher.watch(&root, RecursiveMode::Recursive) {
                Ok(()) => watched += 1,
                Err(error) => {
                    warn!(%error, root = %root.display(), "failed to watch local library root");
                    failed_roots.push(root);
                }
            }
        }
        if watched == 0 {
            return Err(SourceError::Other(
                "No Local library folder could be watched.".to_string(),
            ));
        }
        if !on_ready() {
            return Ok(());
        }

        let mut retry_failed_roots_at = Instant::now() + FAILED_ROOT_RETRY;
        while !should_stop() {
            match receiver.recv_timeout(POLL_INTERVAL) {
                Ok(FeedMessage::Event(event)) => {
                    let mut change = event_change(event);
                    loop {
                        match receiver.recv_timeout(DEBOUNCE) {
                            Ok(FeedMessage::Event(event)) => {
                                merge_change(&mut change, event_change(event));
                            }
                            Ok(FeedMessage::Failed(error)) => {
                                return Err(SourceError::Other(error));
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                return Err(SourceError::Other(
                                    "Local library watcher disconnected.".to_string(),
                                ));
                            }
                        }
                    }
                    if !on_change(change) {
                        return Ok(());
                    }
                }
                Ok(FeedMessage::Failed(error)) => return Err(SourceError::Other(error)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(SourceError::Other(
                        "Local library watcher disconnected.".to_string(),
                    ));
                }
            }

            if !failed_roots.is_empty() && Instant::now() >= retry_failed_roots_at {
                let mut still_failed = Vec::new();
                let mut recovered = false;
                for root in failed_roots.drain(..) {
                    match watcher.watch(&root, RecursiveMode::Recursive) {
                        Ok(()) => recovered = true,
                        Err(error) => {
                            warn!(%error, root = %root.display(), "failed to retry local library root");
                            still_failed.push(root);
                        }
                    }
                }
                failed_roots = still_failed;
                retry_failed_roots_at = Instant::now() + FAILED_ROOT_RETRY;
                if recovered && !on_change(LibraryChange::Full) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

fn feed_error(error: notify::Error) -> SourceError {
    SourceError::Other(error.to_string())
}

fn event_change(event: Event) -> LibraryChange {
    // File paths are usable only when the event describes them completely
    let complete_required = event.need_rescan()
        || event.paths.is_empty()
        || matches!(event.kind, EventKind::Other)
        || matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Name(mode))
                if mode != RenameMode::Both || event.paths.len() != 2
        );
    if complete_required {
        return LibraryChange::Full;
    }
    let mut paths = BTreeSet::new();
    for path in event.paths {
        let Some(path) = path.to_str() else {
            return LibraryChange::Full;
        };
        paths.insert(path.to_string());
    }
    LibraryChange::Objects(SourceObjectChanges::new(paths))
}

fn merge_change(current: &mut LibraryChange, other: LibraryChange) {
    match (&mut *current, other) {
        (LibraryChange::Full, _) => {}
        (current, LibraryChange::Full) => *current = LibraryChange::Full,
        (LibraryChange::Objects(current), LibraryChange::Objects(other)) => current.merge(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, Flag};

    #[test]
    fn ordinary_paths_are_exact_objects() {
        let change = event_change(
            Event::new(EventKind::Create(CreateKind::File))
                .add_path(PathBuf::from("/music/one.flac")),
        );

        assert_eq!(
            change,
            LibraryChange::Objects(SourceObjectChanges::new(["/music/one.flac".to_string()]))
        );
    }

    #[test]
    fn only_a_complete_rename_stays_exact() {
        let complete = event_change(
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                .add_path(PathBuf::from("/music/old.flac"))
                .add_path(PathBuf::from("/music/new.flac")),
        );
        let partial = event_change(
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
                .add_path(PathBuf::from("/music/old.flac")),
        );

        assert_eq!(
            complete,
            LibraryChange::Objects(SourceObjectChanges::new([
                "/music/new.flac".to_string(),
                "/music/old.flac".to_string(),
            ]))
        );
        assert_eq!(partial, LibraryChange::Full);
    }

    #[test]
    fn rescan_and_unknown_events_require_full_coverage() {
        assert_eq!(
            event_change(Event::new(EventKind::Any).set_flag(Flag::Rescan)),
            LibraryChange::Full
        );
        assert_eq!(
            event_change(Event::new(EventKind::Other).add_path(PathBuf::from("/music"))),
            LibraryChange::Full
        );
        assert_eq!(
            event_change(Event::new(EventKind::Any)),
            LibraryChange::Full
        );
    }

    #[test]
    fn debounced_paths_union_and_full_absorbs_them() {
        let mut change =
            LibraryChange::Objects(SourceObjectChanges::new(["/music/one.flac".to_string()]));
        merge_change(
            &mut change,
            LibraryChange::Objects(SourceObjectChanges::new(["/music/two.flac".to_string()])),
        );
        assert_eq!(
            change,
            LibraryChange::Objects(SourceObjectChanges::new([
                "/music/one.flac".to_string(),
                "/music/two.flac".to_string(),
            ]))
        );

        merge_change(&mut change, LibraryChange::Full);
        assert_eq!(change, LibraryChange::Full);
    }
}
