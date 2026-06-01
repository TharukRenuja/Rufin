use super::*;

impl AppController {
    pub(in crate::controller) fn persist_and_emit_queue(&self) {
        self.persist_and_emit_queue_with_next_preload(true);
    }
    pub(in crate::controller) fn persist_and_emit_queue_for_playback_start(&self) {
        self.persist_and_emit_queue_with_next_preload(false);
    }
    fn persist_and_emit_queue_with_next_preload(&self, prepare_next: bool) {
        let queue_snapshot = self.queue_snapshot();
        if let Some(snapshot) = &queue_snapshot {
            self.persist_queue_snapshot_deferred(snapshot.clone());
        }
        self.sync_playback_snapshot_from_queue();
        let _sent = self
            .events
            .send(ControllerEvent::Queue(Box::new(queue_snapshot)));
        self.emit_playback_snapshot();
        if prepare_next {
            self.prepare_next_stream();
        }
    }
    pub(in crate::controller) fn persist_current_queue_snapshot(&self) {
        if let Some(snapshot) = self.queue_snapshot() {
            self.persist_queue_snapshot(&snapshot);
        }
    }
    pub(in crate::controller) fn persist_queue_snapshot_deferred(&self, snapshot: QueueSnapshot) {
        if matches!(&self.store, StoreHandle::Memory { .. }) {
            self.persist_queue_snapshot(&snapshot);
            return;
        }

        persist_queue_snapshot_deferred_from_handles(
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
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
    }
    pub(in crate::controller) fn queue_snapshot(&self) -> Option<QueueSnapshot> {
        self.queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot))
    }
    pub fn cached_track_local_path(&self, track_id: &TrackId) -> Option<String> {
        let server_id = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()
            .map(|saved| saved.server.id)?;
        self.store
            .with_store(|store| {
                if let Some(path) = store.track_local_path(&server_id, track_id)? {
                    return Ok(Some(path));
                }
                store.track_local_match_path(&server_id, track_id)
            })
            .ok()
            .flatten()
    }
    pub fn cached_track_source_format(&self, track_id: &TrackId) -> Option<String> {
        let server_id = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()
            .map(|saved| saved.server.id)?;
        self.store
            .with_store(|store| store.track_source_format(&server_id, track_id))
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
        let queue = self.queue.lock().ok();
        let queue = queue.as_ref().and_then(|queue| queue.as_ref());
        self.update_playback_snapshot(|snapshot| {
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
            snapshot.auto_dj_enabled = self
                .auto_dj_enabled
                .lock()
                .map(|enabled| *enabled)
                .unwrap_or_default();
            set_waveform_cache_key(snapshot, waveform_cache_key_for_queue(queue));
            if snapshot.current.is_none() {
                snapshot.state = PlaybackState::Stopped;
                snapshot.last_error = None;
                snapshot.buffering_percent = None;
            }
        });
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

pub(in crate::controller) fn persist_queue_snapshot_deferred_from_handles(
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
            let _sent = events.send(ControllerEvent::Error(error));
        }
    });
}
