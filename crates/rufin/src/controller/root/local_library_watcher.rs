use super::*;
use notify::{EventKind, RecursiveMode, Watcher};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_secs(2);
const RETRY_POLL: Duration = Duration::from_millis(500);

pub(in crate::controller) struct LocalLibraryWatcher {
    roots: Vec<PathBuf>,
    tx: Sender<WatchMessage>,
    retry_scheduled: Arc<AtomicBool>,
}

enum WatchMessage {
    Changed,
    Stop,
}

impl AppController {
    pub fn refresh_local_library_watcher(&self) {
        refresh_local_library_watcher(self.sync_context(), Arc::clone(&self.local_library_watcher));
    }
}

pub(in crate::controller) fn refresh_local_library_watcher(
    context: SyncContext,
    slot: Arc<Mutex<Option<LocalLibraryWatcher>>>,
) {
    let target = active_local_watch_target(&context.store);
    let Ok(mut current) = slot.lock() else {
        return;
    };
    let Some((saved, roots)) = target else {
        *current = None;
        return;
    };
    if current
        .as_ref()
        .is_some_and(|watcher| watcher.roots == roots)
    {
        return;
    }
    *current = Some(LocalLibraryWatcher::start(roots, context, saved));
}

fn active_local_watch_target(store: &StoreHandle) -> Option<(SavedSource, Vec<PathBuf>)> {
    let saved = store
        .with_store(|store| store.active_source())
        .ok()
        .flatten()?;
    if saved.source.kind != LOCAL_SOURCE_ID {
        return None;
    }
    let mut roots = load_settings_from_store(store)
        .sources
        .local_folders
        .into_iter()
        .map(|folder| PathBuf::from(folder.path))
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    (!roots.is_empty()).then_some((saved, roots))
}

impl LocalLibraryWatcher {
    fn start(roots: Vec<PathBuf>, context: SyncContext, saved: SavedSource) -> Self {
        let (tx, rx) = channel();
        let thread_tx = tx.clone();
        let watched_roots = roots.clone();
        let retry_scheduled = Arc::new(AtomicBool::new(false));
        let thread_retry_scheduled = Arc::clone(&retry_scheduled);
        thread::spawn(move || {
            watch_local_roots(
                rx,
                thread_tx,
                watched_roots,
                context,
                saved,
                thread_retry_scheduled,
            );
        });
        Self {
            roots,
            tx,
            retry_scheduled,
        }
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
                trigger_local_reconciliation(&context, &saved, &retry_scheduled);
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
    retry_scheduled: &Arc<AtomicBool>,
) {
    if !sync_target_is_current(&context.store, &saved.source.id)
        || active_local_watch_target(&context.store).is_none()
    {
        return;
    }
    if context.sync_in_flight.contains_or_blocked(&saved.source.id) {
        start_local_retry_after_in_flight(
            context.clone(),
            saved.clone(),
            Arc::clone(retry_scheduled),
        );
        return;
    }
    start_background_sync_thread(context.clone(), saved.clone());
}

fn start_local_retry_after_in_flight(
    context: SyncContext,
    saved: SavedSource,
    retry_scheduled: Arc<AtomicBool>,
) {
    if retry_scheduled.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        while retry_scheduled.load(Ordering::Acquire)
            && context.sync_in_flight.contains_or_blocked(&saved.source.id)
            && sync_target_is_current(&context.store, &saved.source.id)
            && active_local_watch_target(&context.store).is_some()
        {
            thread::sleep(RETRY_POLL);
        }
        let should_run = retry_scheduled.swap(false, Ordering::AcqRel)
            && sync_target_is_current(&context.store, &saved.source.id)
            && active_local_watch_target(&context.store).is_some();
        if should_run {
            start_background_sync_thread(context, saved);
        }
    });
}
