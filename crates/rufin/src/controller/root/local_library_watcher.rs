use super::*;
use crate::sources::{FreshnessOperations, FreshnessWatcher};
use notify::{EventKind, RecursiveMode, Watcher};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_secs(2);
const RETRY_POLL: Duration = Duration::from_millis(500);

pub(in crate::controller) struct LocalLibraryWatcher {
    active: Arc<ActiveSource>,
    tx: Sender<WatchMessage>,
    retry_scheduled: Arc<AtomicBool>,
}

enum WatchMessage {
    Changed,
    Stop,
}

pub(crate) fn local_freshness_operations(
    roots: crate::sources::LocalRootsLoader,
) -> FreshnessOperations {
    let available_roots = Arc::clone(&roots);
    let watcher_roots = roots;
    FreshnessOperations {
        available: Arc::new(move || !(available_roots)().is_empty()),
        reconcile_cached: full_ingest_cached_reconciliation(),
        start_watcher: Arc::new(move |context, saved, active| {
            let mut roots = watcher_roots();
            roots.sort();
            roots.dedup();
            (!roots.is_empty()).then(|| {
                Box::new(LocalLibraryWatcher::start(roots, context, saved, active))
                    as Box<dyn FreshnessWatcher>
            })
        }),
    }
}

impl LocalLibraryWatcher {
    fn start(
        roots: Vec<PathBuf>,
        context: SyncContext,
        saved: SavedSource,
        active: Arc<ActiveSource>,
    ) -> Self {
        let (tx, rx) = channel();
        let thread_tx = tx.clone();
        let watched_roots = roots.clone();
        let retry_scheduled = Arc::new(AtomicBool::new(false));
        let thread_retry_scheduled = Arc::clone(&retry_scheduled);
        let thread_active = Arc::clone(&active);
        thread::spawn(move || {
            watch_local_roots(
                rx,
                thread_tx,
                watched_roots,
                context,
                saved,
                thread_active,
                thread_retry_scheduled,
            );
        });
        Self {
            active,
            tx,
            retry_scheduled,
        }
    }
}

impl FreshnessWatcher for LocalLibraryWatcher {
    fn active(&self) -> &Arc<ActiveSource> {
        &self.active
    }
}

impl Drop for LocalLibraryWatcher {
    fn drop(&mut self) {
        self.retry_scheduled.store(false, Ordering::Release);
        let _sent = self.tx.send(WatchMessage::Stop);
    }
}

fn watch_local_roots(
    rx: Receiver<WatchMessage>,
    tx: Sender<WatchMessage>,
    roots: Vec<PathBuf>,
    context: SyncContext,
    saved: SavedSource,
    expected: Arc<ActiveSource>,
    retry_scheduled: Arc<AtomicBool>,
) {
    let event_tx = tx.clone();
    let mut watcher =
        match notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                let _sent = event_tx.send(WatchMessage::Changed);
            }
            Ok(_) => {}
            Err(error) => warn!(%error, "local library watcher error"),
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                warn!(%error, "failed to start local library watcher");
                return;
            }
        };

    let mut watched = 0usize;
    for root in &roots {
        if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
            warn!(%error, root = %root.display(), "failed to watch local library root");
        } else {
            watched = watched.saturating_add(1);
        }
    }
    if watched == 0 {
        return;
    }
    info!(roots = watched, "started local library watcher");

    while let Ok(message) = rx.recv() {
        match message {
            WatchMessage::Changed => {
                if !drain_watch_events(&rx) {
                    return;
                }
                trigger_local_reconciliation(&context, &saved, &expected, &retry_scheduled);
            }
            WatchMessage::Stop => return,
        }
    }
}

fn drain_watch_events(rx: &Receiver<WatchMessage>) -> bool {
    loop {
        match rx.recv_timeout(DEBOUNCE) {
            Ok(WatchMessage::Changed) => {}
            Ok(WatchMessage::Stop) | Err(RecvTimeoutError::Disconnected) => return false,
            Err(RecvTimeoutError::Timeout) => return true,
        }
    }
}

fn trigger_local_reconciliation(
    context: &SyncContext,
    saved: &SavedSource,
    expected: &Arc<ActiveSource>,
    retry_scheduled: &Arc<AtomicBool>,
) {
    if !sync_target_is_current(&context.store, &saved.source.id)
        || !local_watch_target_is_active(context, saved, expected)
    {
        return;
    }
    if context.sync_in_flight.contains_or_blocked(&saved.source.id) {
        start_local_retry_after_in_flight(
            context.clone(),
            saved.clone(),
            Arc::clone(expected),
            Arc::clone(retry_scheduled),
        );
        return;
    }
    start_background_sync_thread(context.clone(), saved.clone());
}

fn start_local_retry_after_in_flight(
    context: SyncContext,
    saved: SavedSource,
    expected: Arc<ActiveSource>,
    retry_scheduled: Arc<AtomicBool>,
) {
    if retry_scheduled.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        while retry_scheduled.load(Ordering::Acquire)
            && context.sync_in_flight.contains_or_blocked(&saved.source.id)
            && local_watch_target_is_active(&context, &saved, &expected)
        {
            thread::sleep(RETRY_POLL);
        }
        let should_run = retry_scheduled.swap(false, Ordering::AcqRel)
            && local_watch_target_is_active(&context, &saved, &expected);
        if should_run {
            start_background_sync_thread(context, saved);
        }
    });
}

fn local_watch_target_is_active(
    context: &SyncContext,
    saved: &SavedSource,
    expected: &Arc<ActiveSource>,
) -> bool {
    sync_target_is_current(&context.store, &saved.source.id)
        && selected_active_source(&context.active_source, &saved.source.id)
            .is_ok_and(|active| Arc::ptr_eq(&active, expected))
}
