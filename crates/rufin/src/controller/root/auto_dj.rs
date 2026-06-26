use super::queue_state::defer_queue_snapshot;
use super::*;
use crate::controller::generated_radio::{saved_server_for_generated_queue, spread_radio_tracks};
use crate::controller::provider_tracks::{prepare_cached_tracks, prepare_provider_tracks};
use source::{PlayedFilter, RandomTrackRequest};
use source_local::LOCAL_PROVIDER_ID;

impl AppController {
    #[cfg(test)]
    pub(in crate::controller) fn auto_dj_topup(&self) -> bool {
        match self.auto_dj_top_up() {
            Ok(topped_up) => topped_up,
            Err(error) if auto_dj_error_is_transient(&error) => {
                debug!(%error, "skipped Auto DJ top-up while store is busy");
                false
            }
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
                false
            }
        }
    }

    #[cfg(test)]
    pub(in crate::controller) fn auto_dj_top_up(&self) -> Result<bool, String> {
        auto_dj_handles(self)
    }

    pub(in crate::controller) fn auto_dj_top_up_deferred(&self) -> bool {
        self.auto_dj_top_up_deferred_with(AutoDjCompletion::AppendOnly)
    }

    pub(in crate::controller) fn auto_dj_continue_after_end_deferred(&self) -> bool {
        self.auto_dj_top_up_deferred_with(AutoDjCompletion::ContinueAfterEnd)
    }

    fn auto_dj_top_up_deferred_with(&self, completion: AutoDjCompletion) -> bool {
        if !auto_dj_should_schedule(self) {
            return false;
        }
        let controller = self.clone();
        thread::spawn(move || match auto_dj_handles(&controller) {
            Ok(true) => {
                if completion == AutoDjCompletion::ContinueAfterEnd
                    && auto_dj_continue_after_end(&controller)
                {
                    return;
                }
                emit_auto_dj_top_up(&controller);
            }
            Ok(false) => {
                if completion == AutoDjCompletion::ContinueAfterEnd {
                    controller.stop();
                }
            }
            Err(error) if auto_dj_error_is_transient(&error) => {
                debug!(%error, "skipped Auto DJ top-up while store is busy");
                if completion == AutoDjCompletion::ContinueAfterEnd {
                    controller.stop();
                }
            }
            Err(error) => {
                let _sent = controller.events.send(ControllerEvent::Error(error));
                if completion == AutoDjCompletion::ContinueAfterEnd {
                    controller.stop();
                }
            }
        });
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoDjCompletion {
    AppendOnly,
    ContinueAfterEnd,
}

#[derive(Clone, Debug)]
struct AutoDjQueueState {
    server_id: ServerId,
    current: QueueEntry,
    queued_entries: HashSet<(QueueEntryId, TrackId)>,
    queued_track_ids: HashSet<TrackId>,
    remaining: usize,
}

fn auto_dj_should_schedule(controller: &AppController) -> bool {
    if !controller
        .auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default()
    {
        return false;
    }
    let Some(state) = auto_dj_state(&controller.queue) else {
        return false;
    };
    let Ok(Some(saved)) = saved_server_for_generated_queue(controller, &state.server_id) else {
        return false;
    };
    let settings = load_settings_for_saved(&controller.store, &saved);
    state.remaining < usize::from(settings.auto_dj_refill_threshold)
}

fn auto_dj_handles(controller: &AppController) -> Result<bool, String> {
    if !controller
        .auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default()
    {
        return Ok(false);
    }
    let Some(state) = auto_dj_state(&controller.queue) else {
        return Ok(false);
    };
    let Some(saved) = saved_server_for_generated_queue(controller, &state.server_id)? else {
        return Ok(false);
    };
    let settings = load_settings_for_saved(&controller.store, &saved);
    let refill_threshold = usize::from(settings.auto_dj_refill_threshold);
    if state.remaining >= refill_threshold {
        return Ok(false);
    }
    let candidates = auto_dj_candidate_tracks(controller, &saved, &settings, &state)?;
    if candidates.is_empty() {
        return Ok(false);
    }
    if !controller
        .auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default()
    {
        return Ok(false);
    }
    append_auto_dj(&controller.queue, &state, refill_threshold, &candidates)
}

fn emit_auto_dj_top_up(controller: &AppController) {
    let queue_snapshot = controller
        .queue
        .lock()
        .ok()
        .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot));
    if let Some(snapshot) = queue_snapshot {
        defer_queue_snapshot(
            controller.store.clone(),
            controller.events.clone(),
            Arc::clone(&controller.queue_persist_generation),
            snapshot.clone(),
        );
        let _sent = controller
            .events
            .send(ControllerEvent::Queue(Box::new(Some(snapshot))));
    }
    prepare_next_stream_from_handles(
        controller.store.clone(),
        Arc::clone(&controller.runtime),
        Arc::clone(&controller.secrets),
        Arc::clone(&controller.playback),
        Arc::clone(&controller.queue),
        Arc::clone(&controller.next_preload),
        controller.events.clone(),
    );
}

fn auto_dj_continue_after_end(controller: &AppController) -> bool {
    let mut moved = false;
    let result = controller.with_queue_mut(|queue| {
        moved = queue.advance_after_end_of_stream().is_some();
        Ok(())
    });
    if let Err(error) = result {
        let _sent = controller.events.send(ControllerEvent::Error(error));
        return false;
    }
    if !moved {
        return false;
    }
    controller.start_queue_emit();
    controller.restart_current_track();
    controller.auto_dj_top_up_deferred();
    true
}

fn auto_dj_candidate_tracks(
    controller: &AppController,
    saved: &SavedServer,
    settings: &AppSettings,
    state: &AutoDjQueueState,
) -> Result<Vec<Track>, String> {
    let tracks = controller.generated_tracks_for_saved(
        saved,
        GeneratedTrackSeed::Track(state.current.track_id.clone()),
        AUTO_DJ_PROVIDER_CANDIDATE_LIMIT,
    )?;
    let mut seen_track_ids = state.queued_track_ids.clone();
    let mut candidates =
        collect_auto_dj_candidates(tracks, &mut seen_track_ids, AUTO_DJ_ITEM_COUNT);

    if candidates.len() < AUTO_DJ_ITEM_COUNT {
        match auto_dj_random_fallback_tracks(controller, saved, settings, state) {
            Ok(tracks) => {
                let remaining = AUTO_DJ_ITEM_COUNT - candidates.len();
                candidates.extend(collect_auto_dj_candidates(
                    tracks,
                    &mut seen_track_ids,
                    remaining,
                ));
            }
            Err(error) => {
                debug!(%error, "skipped Auto DJ random fallback");
            }
        }
    }

    Ok(candidates)
}

fn collect_auto_dj_candidates(
    tracks: impl IntoIterator<Item = Track>,
    seen_track_ids: &mut HashSet<TrackId>,
    limit: usize,
) -> Vec<Track> {
    let mut candidates = Vec::new();
    for track in tracks {
        if seen_track_ids.insert(track.id.clone()) {
            candidates.push(track);
            if candidates.len() >= limit {
                break;
            }
        }
    }
    candidates
}

fn auto_dj_random_fallback_tracks(
    controller: &AppController,
    saved: &SavedServer,
    settings: &AppSettings,
    state: &AutoDjQueueState,
) -> Result<Vec<Track>, String> {
    let genre_name = auto_dj_current_genre(controller, state)?;
    let should_spread_cached_tracks =
        saved.server.provider == "fake" || saved.server.provider == LOCAL_PROVIDER_ID;
    let mut tracks = if should_spread_cached_tracks {
        auto_dj_random_fallback_tracks_from_cache(
            controller,
            &saved.server.id,
            genre_name.as_deref(),
        )?
    } else {
        let provider = provider_for_saved(
            &controller.store,
            &controller.runtime,
            &controller.secrets,
            saved,
        )?;
        controller
            .runtime
            .block_on(
                provider
                    .as_music_provider()
                    .random_tracks(RandomTrackRequest {
                        limit: AUTO_DJ_PROVIDER_CANDIDATE_LIMIT,
                        min_year: None,
                        max_year: None,
                        genre_id: None,
                        genre_name,
                        played_filter: PlayedFilter::All,
                    }),
            )
            .map_err(|error| error.to_string())?
    };
    if saved.server.provider == LOCAL_PROVIDER_ID {
        prepare_cached_tracks(controller, saved, settings, &mut tracks)?;
    } else {
        prepare_provider_tracks(controller, saved, settings, &mut tracks)?;
    }
    if should_spread_cached_tracks {
        tracks = spread_radio_tracks(
            &format!("auto-dj:{}", state.current.track_id.as_str()),
            tracks,
        );
    }
    Ok(tracks)
}

fn auto_dj_current_genre(
    controller: &AppController,
    state: &AutoDjQueueState,
) -> Result<Option<String>, String> {
    let track = controller
        .store
        .with_store(|store| store.load_track(&state.server_id, &state.current.track_id))?;
    Ok(track.and_then(|track| {
        track
            .genres
            .into_iter()
            .find(|genre| !genre.trim().is_empty())
    }))
}

fn auto_dj_random_fallback_tracks_from_cache(
    controller: &AppController,
    server_id: &ServerId,
    genre_name: Option<&str>,
) -> Result<Vec<Track>, String> {
    let tracks = if let Some(genre_name) = genre_name {
        controller.store.with_store(|store| {
            store.load_tracks_by_genre_name(server_id, genre_name, AUTO_DJ_PROVIDER_CANDIDATE_LIMIT)
        })?
    } else {
        controller
            .store
            .with_store(|store| store.load_tracks(server_id, 0, AUTO_DJ_PROVIDER_CANDIDATE_LIMIT))?
            .items
    };
    Ok(tracks)
}

fn auto_dj_state(queue: &Arc<Mutex<Option<QueueEngine>>>) -> Option<AutoDjQueueState> {
    queue.lock().ok().and_then(|queue| {
        let queue = queue.as_ref()?;
        if queue.repeat_mode() == RepeatMode::One {
            return None;
        }
        let current = queue.current()?.clone();
        let queued = queue
            .entries()
            .iter()
            .map(|entry| entry.track_id.clone())
            .collect::<HashSet<_>>();
        let queued_entries = queue
            .entries()
            .iter()
            .map(|entry| (entry.id.clone(), entry.track_id.clone()))
            .collect::<HashSet<_>>();
        Some(AutoDjQueueState {
            server_id: queue.server_id().clone(),
            current,
            queued_entries,
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
    if queue.server_id() != &state.server_id || !same_auto_dj_queue(queue, state) {
        return Ok(false);
    }
    let current_matches_trigger =
        current.id == state.current.id && current.track_id == state.current.track_id;
    if current_matches_trigger && queue.remaining_after_current() >= refill_threshold {
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
    queue.trim_auto_dj_history(AUTO_DJ_HISTORY_LIMIT);
    let items = candidates
        .iter()
        .enumerate()
        .map(|(generated_index, track)| QueueItemInput::Generated {
            track: (*track).clone(),
            generated_index,
        })
        .collect::<Vec<_>>();
    queue
        .append_last(QueueInsertion {
            source: QueueInsertionSource::AutoDj {
                generated_from_track_id: state.current.track_id.clone(),
                reason: AutoDjReason::Similarity,
            },
            items,
        })
        .map_err(|error| format!("auto dj queue append failed: {error:?}"))?;
    Ok(true)
}

fn same_auto_dj_queue(queue: &QueueEngine, state: &AutoDjQueueState) -> bool {
    if queue.entries().len() != state.queued_entries.len() {
        return false;
    }
    queue.entries().iter().all(|entry| {
        state
            .queued_entries
            .contains(&(entry.id.clone(), entry.track_id.clone()))
    })
}

fn auto_dj_error_is_transient(error: &str) -> bool {
    error.contains("database is locked") || error.contains("database table is locked")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::root::test_support::library_track;

    #[test]
    fn auto_dj_append_allows_moved_cursor() {
        let server_id = ServerId::fake(1);
        let first = library_track(1, None, AlbumId::fake(1), "Artist", &[]);
        let second = library_track(2, None, AlbumId::fake(1), "Artist", &[]);
        let third = library_track(3, None, AlbumId::fake(1), "Artist", &[]);
        let mut engine = QueueEngine::new(server_id);
        engine.play_now(&first);
        engine.append(&second);
        let queue = Arc::new(Mutex::new(Some(engine)));
        let state = auto_dj_state(&queue).expect("auto dj state");

        queue
            .lock()
            .expect("queue")
            .as_mut()
            .expect("engine")
            .next_track();

        assert!(append_auto_dj(&queue, &state, 2, std::slice::from_ref(&third)).expect("append"));
        let queue = queue.lock().expect("queue");
        let queue = queue.as_ref().expect("queue");
        assert_eq!(queue.entries().len(), 3);
        assert_eq!(queue.entries()[2].track_id, third.id);
    }

    #[test]
    fn auto_dj_append_rejects_replaced_queue() {
        let server_id = ServerId::fake(1);
        let first = library_track(1, None, AlbumId::fake(1), "Artist", &[]);
        let second = library_track(2, None, AlbumId::fake(1), "Artist", &[]);
        let third = library_track(3, None, AlbumId::fake(1), "Artist", &[]);
        let replacement = library_track(4, None, AlbumId::fake(1), "Artist", &[]);
        let mut engine = QueueEngine::new(server_id.clone());
        engine.play_now(&first);
        engine.append(&second);
        let queue = Arc::new(Mutex::new(Some(engine)));
        let state = auto_dj_state(&queue).expect("auto dj state");
        let mut engine = QueueEngine::new(server_id);
        engine.play_now(&replacement);
        engine.append(&second);
        *queue.lock().expect("queue") = Some(engine);

        assert!(!append_auto_dj(&queue, &state, 2, &[third]).expect("append"));
    }
}
