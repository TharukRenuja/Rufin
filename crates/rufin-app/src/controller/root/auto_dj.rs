use super::queue_state::defer_queue_snapshot;
use super::*;

impl AppController {
    pub(in crate::controller) fn auto_dj_topup(&self) -> bool {
        match self.auto_dj_top_up() {
            Ok(topped_up) => topped_up,
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
                false
            }
        }
    }
    pub(in crate::controller) fn auto_dj_top_up(&self) -> Result<bool, String> {
        auto_dj_handles(&self.auto_dj_enabled, &self.queue, &self.store)
    }
    pub(in crate::controller) fn auto_dj_top_up_deferred(&self) {
        if !self
            .auto_dj_enabled
            .lock()
            .map(|enabled| *enabled)
            .unwrap_or_default()
        {
            return;
        }

        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let queue = Arc::clone(&self.queue);
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let playback = Arc::clone(&self.playback);
        let queue_persist_generation = Arc::clone(&self.queue_persist_generation);
        let events = self.events.clone();
        thread::spawn(
            move || match auto_dj_handles(&auto_dj_enabled, &queue, &store) {
                Ok(true) => {
                    let queue_snapshot = queue
                        .lock()
                        .ok()
                        .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot));
                    if let Some(snapshot) = queue_snapshot {
                        defer_queue_snapshot(
                            store.clone(),
                            events.clone(),
                            queue_persist_generation,
                            snapshot.clone(),
                        );
                        let _sent = events.send(ControllerEvent::Queue(Box::new(Some(snapshot))));
                    }
                    prepare_next_stream_from_handles(
                        store, runtime, secrets, playback, queue, events,
                    );
                }
                Ok(false) => {}
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            },
        );
    }
}

#[derive(Clone, Debug)]
struct AutoDjQueueState {
    server_id: ServerId,
    current: QueueEntry,
    queued_track_ids: HashSet<TrackId>,
    remaining: usize,
}

fn auto_dj_handles(
    auto_dj_enabled: &Arc<Mutex<bool>>,
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    store: &StoreHandle,
) -> Result<bool, String> {
    if !auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default()
    {
        return Ok(false);
    }
    let Some(state) = auto_dj_state(queue) else {
        return Ok(false);
    };
    let settings = load_settings_for_active_server(store);
    let refill_threshold = usize::from(settings.auto_dj_refill_threshold);
    if state.remaining >= refill_threshold {
        return Ok(false);
    }
    let mut tracks = store
        .with_store(|store| store.load_tracks(&state.server_id, 0, AUTO_DJ_LIBRARY_LIMIT))
        .map(|page| page.items)?;
    cover_art_policy::bind_tracks(&mut tracks, &settings);
    let mut candidates = auto_dj_candidates(
        &tracks,
        &state.current,
        &state.queued_track_ids,
        shuffle_seed(),
    );
    auto_dj_refs(store, &state.server_id, &mut candidates)?;
    if candidates.is_empty() {
        return Ok(false);
    }
    if !auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default()
    {
        return Ok(false);
    }
    append_auto_dj(queue, &state, refill_threshold, &candidates)
}
fn auto_dj_refs(
    store: &StoreHandle,
    server_id: &ServerId,
    candidates: &mut [Track],
) -> Result<(), String> {
    let saved = store.with_store(|store| store.saved_server(server_id))?;
    let saved =
        saved.or_else(|| (server_id.as_str() == LOCAL_SOURCE_SERVER_ID).then(local_source_saved));
    let Some(saved) = saved else {
        return Ok(());
    };
    track_album_refs(store, &saved, candidates, &[])
}

fn auto_dj_state(queue: &Arc<Mutex<Option<QueueEngine>>>) -> Option<AutoDjQueueState> {
    queue.lock().ok().and_then(|queue| {
        let queue = queue.as_ref()?;
        let current = queue.current()?.clone();
        let queued = queue
            .entries()
            .iter()
            .map(|entry| entry.track_id.clone())
            .collect::<HashSet<_>>();
        Some(AutoDjQueueState {
            server_id: queue.server_id().clone(),
            current,
            queued_track_ids: queued,
            remaining: queue.remaining_after_current(),
        })
    })
}

fn append_auto_dj(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    state: &AutoDjQueueState,
    refill_threshold: usize,
    candidates: &[Track],
) -> Result<bool, String> {
    let mut queue = queue
        .lock()
        .map_err(|_| "queue lock was poisoned".to_string())?;
    let Some(queue) = queue.as_mut() else {
        return Ok(false);
    };
    let Some(current) = queue.current() else {
        return Ok(false);
    };
    if queue.server_id() != &state.server_id
        || current.id != state.current.id
        || current.track_id != state.current.track_id
    {
        return Ok(false);
    }
    if queue.remaining_after_current() >= refill_threshold {
        return Ok(false);
    }
    let queued_track_ids = queue
        .entries()
        .iter()
        .map(|entry| entry.track_id.clone())
        .collect::<HashSet<_>>();
    let candidates = candidates
        .iter()
        .filter(|track| !queued_track_ids.contains(&track.id))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(false);
    }
    for track in candidates {
        queue.append(track);
    }
    Ok(true)
}
