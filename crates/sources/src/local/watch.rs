use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{
    Event, EventKind, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};
use tracing::warn;

use crate::{LocalFilesystemChange, SourceError, SourceResult};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEBOUNCE: Duration = Duration::from_secs(2);
const FAILED_ROOT_RETRY: Duration = Duration::from_secs(60);
const FEED_RETRY_MIN: Duration = Duration::from_secs(5);
const FEED_RETRY_MAX: Duration = Duration::from_secs(60);

pub struct LocalChangeFeed {
    roots: Vec<PathBuf>,
}

impl LocalChangeFeed {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }

    pub fn listen_forever(
        &self,
        on_ready: &mut dyn FnMut(bool) -> bool,
        on_change: &mut dyn FnMut(LocalFilesystemChange) -> bool,
        should_stop: &dyn Fn() -> bool,
    ) -> SourceResult<()> {
        let mut delay = FEED_RETRY_MIN;
        let mut reconnecting = false;
        while !should_stop() {
            let result = self.listen(reconnecting, on_ready, on_change, should_stop);
            if should_stop() {
                return Ok(());
            }
            if let Err(error) = result {
                warn!(%error, "Local library change feed disconnected");
            }
            reconnecting = true;
            if !wait_before_retry(delay, should_stop) {
                return Ok(());
            }
            delay = delay.saturating_mul(2).min(FEED_RETRY_MAX);
        }
        Ok(())
    }

    /// Run the one blocking filesystem feed.
    ///
    /// Rufin owns the source-session cancellation token and sends each result
    /// through Local's automatic/exact inventory operation.
    pub fn listen(
        &self,
        reconnecting: bool,
        on_ready: &mut dyn FnMut(bool) -> bool,
        on_change: &mut dyn FnMut(LocalFilesystemChange) -> bool,
        should_stop: &dyn Fn() -> bool,
    ) -> SourceResult<()> {
        let (messages, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            let message = match event {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    FeedMessage::Event(event)
                }
                Ok(_) => return,
                Err(error) => FeedMessage::Failed(error.to_string()),
            };
            let _ = messages.send(message);
        })
        .map_err(feed_error)?;

        let mut watched = 0;
        let mut failed_roots = Vec::new();
        for root in ordered_roots(self.roots.clone()) {
            match watcher.watch(&root, RecursiveMode::Recursive) {
                Ok(()) => watched += 1,
                Err(error) => {
                    warn!(%error, root = %root.display(), "failed to watch Local music folder");
                    failed_roots.push(root);
                }
            }
        }
        if watched == 0 {
            return Err(SourceError::Other(
                "No Local music folder could be watched.".to_string(),
            ));
        }
        if !on_ready(reconnecting) {
            return Ok(());
        }

        let mut retry_failed_roots_at = Instant::now() + FAILED_ROOT_RETRY;
        while !should_stop() {
            match receiver.recv_timeout(POLL_INTERVAL) {
                Ok(FeedMessage::Event(event)) => {
                    let mut evidence = event_evidence(event);
                    loop {
                        match receiver.recv_timeout(DEBOUNCE) {
                            Ok(FeedMessage::Event(event)) => {
                                evidence.merge(event_evidence(event));
                            }
                            Ok(FeedMessage::Failed(error)) => {
                                return Err(SourceError::Other(error));
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                return Err(SourceError::Other(
                                    "Local music watcher disconnected.".to_string(),
                                ));
                            }
                        }
                    }
                    if !on_change(evidence) {
                        return Ok(());
                    }
                }
                Ok(FeedMessage::Failed(error)) => return Err(SourceError::Other(error)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(SourceError::Other(
                        "Local music watcher disconnected.".to_string(),
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
                            warn!(%error, root = %root.display(), "failed to retry Local music folder");
                            still_failed.push(root);
                        }
                    }
                }
                failed_roots = still_failed;
                retry_failed_roots_at = Instant::now() + FAILED_ROOT_RETRY;
                if recovered && !on_change(LocalFilesystemChange::Rescan) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

fn wait_before_retry(delay: Duration, should_stop: &dyn Fn() -> bool) -> bool {
    let deadline = Instant::now() + delay;
    while !should_stop() {
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
    }
    false
}

enum FeedMessage {
    Event(Event),
    Failed(String),
}

fn ordered_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|root| seen.insert(root.clone()))
        .collect()
}

fn feed_error(error: notify::Error) -> SourceError {
    SourceError::Other(error.to_string())
}

fn event_evidence(event: Event) -> LocalFilesystemChange {
    let complete_required = event.need_rescan()
        || event.paths.is_empty()
        || matches!(event.kind, EventKind::Other)
        || matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Name(mode))
                if mode != RenameMode::Both || event.paths.len() != 2
        );
    if complete_required {
        return LocalFilesystemChange::Rescan;
    }
    LocalFilesystemChange::Paths(event.paths.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use notify::event::{CreateKind, Flag};

    use super::*;

    #[test]
    fn ordinary_paths_are_exact() {
        let evidence = event_evidence(
            Event::new(EventKind::Create(CreateKind::File))
                .add_path(PathBuf::from("/music/one.flac")),
        );
        assert_eq!(
            evidence,
            LocalFilesystemChange::Paths(BTreeSet::from([PathBuf::from("/music/one.flac")]))
        );
    }

    #[test]
    fn partial_rename_and_rescan_require_complete_inventory() {
        let rename = event_evidence(
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
                .add_path(PathBuf::from("/music/old.flac")),
        );
        let rescan = event_evidence(Event::new(EventKind::Any).set_flag(Flag::Rescan));
        assert_eq!(rename, LocalFilesystemChange::Rescan);
        assert_eq!(rescan, LocalFilesystemChange::Rescan);
    }
}
