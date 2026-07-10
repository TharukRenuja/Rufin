use super::*;

impl AppController {
    pub(in crate::controller) fn persist_and_emit_queue(&self) {
        self.preload_queue_emit(true);
    }
    pub(in crate::controller) fn persist_and_emit_playback(&self) {
        self.persist_current_queue_snapshot_deferred();
        self.sync_playback_snapshot_from_queue();
        self.emit_playback_snapshot();
    }
    pub(in crate::controller) fn start_queue_emit(&self) {
        self.preload_queue_emit(false);
    }
    fn preload_queue_emit(&self, prepare_next: bool) {
        let queue_snapshot = self.queue_snapshot();
        if let Some(snapshot) = &queue_snapshot {
            self.persist_queue_snapshot_deferred(snapshot.clone());
        }
        if prepare_next {
            self.sync_playback_snapshot_from_queue();
        } else {
            self.sync_playback_queue_metadata_from_queue();
        }
        let _sent = self
            .events
            .send(ControllerEvent::Queue(Box::new(queue_snapshot)));
        if prepare_next {
            self.emit_playback_snapshot();
            self.prepare_next_stream();
            self.warm_waveforms_for_queue();
        }
    }
    pub(in crate::controller) fn persist_current_queue_snapshot(&self) {
        if let Some(snapshot) = self.queue_snapshot() {
            self.persist_queue_snapshot(&snapshot);
        }
    }
    pub(in crate::controller) fn persist_current_queue_snapshot_deferred(&self) {
        if !self.store.uses_disk_storage() {
            self.persist_current_queue_snapshot();
            return;
        }

        defer_current_queue_snapshot(
            self.store.clone(),
            self.events.clone(),
            Arc::clone(&self.queue_persist_generation),
            Arc::clone(&self.queue),
        );
    }
    pub(in crate::controller) fn persist_queue_snapshot_deferred(&self, snapshot: QueueSnapshot) {
        if !self.store.uses_disk_storage() {
            self.persist_queue_snapshot(&snapshot);
            return;
        }

        defer_queue_snapshot(
            self.store.clone(),
            self.events.clone(),
            Arc::clone(&self.queue_persist_generation),
            snapshot,
        );
    }
    pub(in crate::controller) fn persist_queue_snapshot(&self, snapshot: &QueueSnapshot) {
        if let Err(error) = self
            .store
            .with_store(|store| store.save_queue_snapshot(snapshot))
        {
            handle_queue_persist_error(&self.events, error);
        }
    }
    pub(in crate::controller) fn queue_snapshot(&self) -> Option<QueueSnapshot> {
        self.queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot))
    }
    pub fn cached_track_local_path(&self, track_id: &TrackId) -> Option<String> {
        let source_id = self
            .store
            .with_store(|store| store.active_source())
            .ok()
            .flatten()
            .map(|saved| saved.source.id)?;
        self.store
            .with_store(|store| {
                if let Some(path) = store.track_local_path(&source_id, track_id)? {
                    return Ok(Some(path));
                }
                store.track_local_match_path(&source_id, track_id)
            })
            .ok()
            .flatten()
    }
    pub fn cached_track_source_format(&self, track_id: &TrackId) -> Option<String> {
        let source_id = self
            .store
            .with_store(|store| store.active_source())
            .ok()
            .flatten()
            .map(|saved| saved.source.id)?;
        self.store
            .with_store(|store| store.track_source_format(&source_id, track_id))
            .ok()
            .flatten()
    }
    pub(in crate::controller) fn update_playback_snapshot(
        &self,
        operation: impl FnOnce(&mut PlaybackSnapshot),
    ) {
        if let Ok(mut snapshot) = self.playback_snapshot.lock() {
            operation(&mut snapshot);
        }
    }
    pub(in crate::controller) fn sync_playback_snapshot_from_queue(&self) {
        sync_queue_snapshot(&self.queue, &self.playback_snapshot, &self.auto_dj_enabled);
    }
    pub(in crate::controller) fn sync_playback_queue_metadata_from_queue(&self) {
        sync_queue_metadata(&self.queue, &self.playback_snapshot, &self.auto_dj_enabled);
    }
    pub(in crate::controller) fn emit_playback_snapshot(&self) {
        let snapshot = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default();
        let _sent = self
            .events
            .send(ControllerEvent::Playback(Box::new(snapshot)));
    }
}

pub(in crate::controller) fn sync_queue_metadata(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    auto_dj_enabled: &Arc<Mutex<bool>>,
) {
    let queue = queue.lock().ok();
    let queue = queue.as_ref().and_then(|queue| queue.as_ref());
    let Ok(mut snapshot) = playback_snapshot.lock() else {
        return;
    };
    snapshot.repeat_mode = queue
        .map(QueueEngine::repeat_mode)
        .unwrap_or(RepeatMode::Off);
    snapshot.shuffle_enabled = queue
        .map(|queue| queue.shuffle().enabled)
        .unwrap_or_default();
    snapshot.auto_dj_enabled = auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default();
    let Some(queue) = queue else {
        snapshot.current = None;
        snapshot.position_seconds = 0;
        snapshot.position_millis = 0;
        snapshot.duration_seconds = 0;
        snapshot.state = PlaybackState::Stopped;
        snapshot.last_error = None;
        snapshot.buffering_percent = None;
        set_waveform_cache_key(&mut snapshot, None);
        return;
    };
    let current_matches = snapshot.current.as_ref().zip(queue.current()).is_some_and(
        |(snapshot_current, queue_current)| {
            snapshot_current.id == queue_current.id
                && snapshot_current.track_id == queue_current.track_id
        },
    );
    if current_matches {
        snapshot.current_source_id = Some(queue.source_id().clone());
        snapshot.current = queue.current().cloned();
        snapshot.position_seconds = queue.progress_seconds();
        snapshot.position_millis = u64::from(snapshot.position_seconds) * 1_000;
        snapshot.duration_seconds = snapshot
            .current
            .as_ref()
            .map(|entry| entry.duration_seconds)
            .unwrap_or(0);
        set_waveform_cache_key(&mut snapshot, waveform_cache_key_for_queue(Some(queue)));
    }
}

pub(in crate::controller) fn sync_queue_snapshot(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    auto_dj_enabled: &Arc<Mutex<bool>>,
) {
    let queue = queue.lock().ok();
    let queue = queue.as_ref().and_then(|queue| queue.as_ref());
    let Ok(mut snapshot) = playback_snapshot.lock() else {
        return;
    };
    snapshot.current_source_id = queue.map(|queue| queue.source_id().clone());
    snapshot.current = queue.and_then(|queue| queue.current().cloned());
    snapshot.position_seconds = queue.map(QueueEngine::progress_seconds).unwrap_or(0);
    snapshot.position_millis = u64::from(snapshot.position_seconds) * 1_000;
    snapshot.duration_seconds = snapshot
        .current
        .as_ref()
        .map(|entry| entry.duration_seconds)
        .unwrap_or(0);
    snapshot.repeat_mode = queue
        .map(QueueEngine::repeat_mode)
        .unwrap_or(RepeatMode::Off);
    snapshot.shuffle_enabled = queue
        .map(|queue| queue.shuffle().enabled)
        .unwrap_or_default();
    snapshot.auto_dj_enabled = auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default();
    set_waveform_cache_key(&mut snapshot, waveform_cache_key_for_queue(queue));
    if snapshot.current.is_none() {
        snapshot.current_source_id = None;
        snapshot.state = PlaybackState::Stopped;
        snapshot.last_error = None;
        snapshot.buffering_percent = None;
    }
}

pub(in crate::controller) fn defer_queue_snapshot(
    store: StoreHandle,
    events: Sender<ControllerEvent>,
    generation: Arc<AtomicU64>,
    snapshot: QueueSnapshot,
) {
    let request_generation = generation.fetch_add(1, Ordering::AcqRel) + 1;
    thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if generation.load(Ordering::Acquire) != request_generation {
            return;
        }
        if let Err(error) = store.with_store(|store| store.save_queue_snapshot(&snapshot)) {
            handle_queue_persist_error(&events, error);
        }
    });
}

pub(in crate::controller) fn defer_current_queue_snapshot(
    store: StoreHandle,
    events: Sender<ControllerEvent>,
    generation: Arc<AtomicU64>,
    queue: Arc<Mutex<Option<QueueEngine>>>,
) {
    let request_generation = generation.fetch_add(1, Ordering::AcqRel) + 1;
    thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if generation.load(Ordering::Acquire) != request_generation {
            return;
        }
        let snapshot = queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot));
        if let Some(snapshot) = snapshot
            && let Err(error) = store.with_store(|store| store.save_queue_snapshot(&snapshot))
        {
            handle_queue_persist_error(&events, error);
        }
    });
}

fn handle_queue_persist_error(events: &Sender<ControllerEvent>, error: String) {
    if queue_persist_error_is_transient(&error) {
        debug!(%error, "skipped queue persistence while store is busy");
        return;
    }
    let _sent = events.send(ControllerEvent::Error(error));
}

fn queue_persist_error_is_transient(error: &str) -> bool {
    error.contains("database is locked") || error.contains("database table is locked")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_persist_transient_error() {
        assert!(queue_persist_error_is_transient(
            "sqlite failed: database is locked"
        ));
        assert!(queue_persist_error_is_transient(
            "sqlite failed: database table is locked"
        ));
        assert!(!queue_persist_error_is_transient("sqlite failed: disk I/O"));
    }
}
