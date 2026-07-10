use super::*;

#[derive(Clone, Debug)]
pub(in crate::controller) struct PlaybackActivityEntry {
    source_id: SourceId,
    track_id: TrackId,
    entry_id: QueueEntryId,
    duration_seconds: u32,
    threshold_seconds: u32,
    position_seconds: u32,
    play_recorded: bool,
    skip_recorded: bool,
    store_play_count: bool,
    session_key: String,
}

#[derive(Clone, Debug, Default)]
pub(in crate::controller) struct PlaybackActivityState {
    current: Option<PlaybackActivityEntry>,
}

impl AppController {
    pub(in crate::controller) fn start_playback_activity(
        &self,
        source_id: &SourceId,
        entry: &QueueEntry,
        position_seconds: u32,
    ) {
        let session_key = format!(
            "{}:{}:{}",
            source_id.as_str(),
            entry.id.as_str(),
            unique_millis().unwrap_or(0)
        );
        let activity = PlaybackActivityEntry {
            source_id: source_id.clone(),
            track_id: entry.track_id.clone(),
            entry_id: entry.id.clone(),
            duration_seconds: entry.duration_seconds,
            threshold_seconds: play_threshold_seconds(entry.duration_seconds),
            position_seconds,
            play_recorded: false,
            skip_recorded: false,
            store_play_count: selected_active_source(&self.active_source, source_id)
                .is_ok_and(|active| active.reporter.is_none()),
            session_key,
        };
        if let Ok(mut state) = self.playback_activity.lock() {
            state.current = Some(activity);
        }
        self.record_playback_activity_progress(position_seconds);
    }

    pub(in crate::controller) fn record_playback_activity_progress(&self, seconds: u32) {
        let play = self.play_activity_at(seconds);
        if let Some(activity) = play {
            self.record_store_play(activity);
        }
    }

    pub(in crate::controller) fn record_playback_activity(&self) {
        let play = {
            let mut state = match self.playback_activity.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(activity) = state.current.as_mut() else {
                return;
            };
            activity.position_seconds = activity.duration_seconds;
            if activity.play_recorded || activity.duration_seconds < activity.threshold_seconds {
                None
            } else {
                activity.play_recorded = true;
                Some(activity.clone())
            }
        };
        if let Some(activity) = play {
            self.record_store_play(activity);
        }
    }

    pub(in crate::controller) fn record_current_skip_if_needed(&self) {
        let skip = {
            let mut state = match self.playback_activity.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(activity) = state.current.as_mut() else {
                return;
            };
            if activity.play_recorded || activity.skip_recorded {
                return;
            }
            let remaining = activity
                .duration_seconds
                .saturating_sub(activity.position_seconds);
            if activity.position_seconds >= activity.threshold_seconds || remaining <= 5 {
                return;
            }
            activity.skip_recorded = true;
            Some(activity.clone())
        };
        if let Some(activity) = skip {
            self.record_local_skip(activity);
        }
    }

    pub(in crate::controller) fn clear_playback_activity(&self) {
        if let Ok(mut state) = self.playback_activity.lock() {
            state.current = None;
        }
    }

    fn play_activity_at(&self, seconds: u32) -> Option<PlaybackActivityEntry> {
        let mut state = self.playback_activity.lock().ok()?;
        let activity = state.current.as_mut()?;
        activity.position_seconds = seconds.min(activity.duration_seconds);
        if activity.play_recorded || activity.position_seconds < activity.threshold_seconds {
            return None;
        }
        activity.play_recorded = true;
        Some(activity.clone())
    }

    fn record_store_play(&self, activity: PlaybackActivityEntry) {
        if !activity.store_play_count {
            return;
        }
        if self.store.uses_disk_storage() {
            let store = self.store.clone();
            let events = self.events.clone();
            thread::spawn(move || record_store_play_now(store, events, activity));
        } else {
            record_store_play_now(self.store.clone(), self.events.clone(), activity);
        }
    }

    fn record_local_skip(&self, activity: PlaybackActivityEntry) {
        if self.store.uses_disk_storage() {
            let store = self.store.clone();
            let events = self.events.clone();
            thread::spawn(move || record_local_skip_now(store, events, activity));
        } else {
            record_local_skip_now(self.store.clone(), self.events.clone(), activity);
        }
    }
}

fn record_store_play_now(
    store: StoreHandle,
    events: Sender<ControllerEvent>,
    activity: PlaybackActivityEntry,
) {
    let result = store.with_store(|store| {
        store.record_local_track_played(
            &activity.source_id,
            &activity.track_id,
            &activity.session_key,
        )
    });
    match result {
        Ok(true) => emit_play_activity_delta(&events, activity.track_id),
        Ok(false) => {}
        Err(error) if playback_activity_error_is_transient(&error) => {
            debug!(%error, "skipped playback activity update while store is busy");
        }
        Err(error) => {
            warn!(
                %error,
                source_id = %activity.source_id,
                track_id = %activity.track_id,
                entry_id = activity.entry_id.as_str(),
                "failed to update Store play count"
            );
        }
    }
}

fn record_local_skip_now(
    store: StoreHandle,
    events: Sender<ControllerEvent>,
    activity: PlaybackActivityEntry,
) {
    match store.with_store(|store| {
        store.increment_track_skip_count(&activity.source_id, &activity.track_id)
    }) {
        Ok(()) => emit_skip_activity_delta(&events, activity.track_id),
        Err(error) if playback_activity_error_is_transient(&error) => {
            debug!(%error, "skipped playback activity update while store is busy");
        }
        Err(error) => {
            warn!(
                %error,
                source_id = %activity.source_id,
                track_id = %activity.track_id,
                "failed to update local skip count"
            );
        }
    }
}

fn emit_play_activity_delta(events: &Sender<ControllerEvent>, track_id: TrackId) {
    let mut delta = LibraryDelta::default();
    delta.tracks.stats.push(track_id);
    let _sent = events.send(ControllerEvent::LibraryDelta(Box::new(delta)));
}

fn emit_skip_activity_delta(events: &Sender<ControllerEvent>, track_id: TrackId) {
    let mut delta = LibraryDelta::default();
    delta.tracks.skip_stats.push(track_id);
    let _sent = events.send(ControllerEvent::LibraryDelta(Box::new(delta)));
}

fn playback_activity_error_is_transient(error: &str) -> bool {
    error.contains("database is locked") || error.contains("database table is locked")
}

fn play_threshold_seconds(duration_seconds: u32) -> u32 {
    if duration_seconds <= 10 {
        return duration_seconds;
    }
    let half = duration_seconds / 2;
    if duration_seconds < 60 {
        half.max(5)
    } else {
        half.clamp(30, 240)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PlaybackEvents {
        events: Vec<PlaybackEvent>,
    }

    impl PlaybackBackend for PlaybackEvents {
        fn send(&mut self, _command: PlaybackCommand) -> Result<(), playback::PlaybackError> {
            Ok(())
        }

        fn drain_events(&mut self) -> Vec<PlaybackEvent> {
            std::mem::take(&mut self.events)
        }
    }

    fn activate_source(controller: &AppController, saved: &SavedSource) {
        let active = crate::sources::activate_configured_source(
            &controller.store,
            &controller.secrets,
            saved,
        )
        .expect("activate source");
        *controller.active_source.write().expect("active source") = Some(active);
    }

    #[test]
    fn playback_track_short() {
        assert_eq!(play_threshold_seconds(8), 8);
        assert_eq!(play_threshold_seconds(40), 20);
        assert_eq!(play_threshold_seconds(120), 60);
        assert_eq!(play_threshold_seconds(1_000), 240);
    }

    #[test]
    fn playback_record_threshold() {
        let (controller, _events, _snapshot, _queue, _player) =
            AppController::bootstrap_memory_for_test();
        let source_id = SourceId::new("local:server:test");
        let saved = SavedSource {
            source: SourceIdentity {
                id: source_id.clone(),
                kind: LOCAL_SOURCE_ID.to_string(),
                name: "Local".to_string(),
                base_url: String::new(),
            },
            user_id: "local".to_string(),
            username: "local".to_string(),
            trust_invalid_cert: false,
            use_jellyfin_instant_mix: false,
        };
        let track = library_track(1, None, AlbumId::fake(1), "Artist", &[]);
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&source_id)?;
                let generation = store.begin_sync(&source_id)?;
                store.upsert_tracks(&source_id, std::slice::from_ref(&track), generation)?;
                Ok(())
            })
            .expect("seed local server");
        let mut queue = QueueEngine::new(source_id.clone());
        queue.play_now(&track);
        let entry = queue.current().expect("current").clone();
        *controller.queue.lock().expect("queue") = Some(queue);
        activate_source(&controller, &saved);

        controller.start_playback_activity(&source_id, &entry, 0);
        controller.record_playback_activity_progress(90);
        controller.record_playback_activity_progress(120);

        let detail = smart_detail_named(&controller, &source_id, "Most Played");
        assert_eq!(detail.tracks.len(), 1);
        assert_eq!(detail.tracks[0].id, track.id);
        assert_eq!(detail.tracks[0].play_count, Some(1));
    }

    #[test]
    fn activity_before_threshold_records_one_skip() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = local_source_saved();
        let source_id = saved.source.id.clone();
        let mut track = library_track(1, None, AlbumId::new("local:album:activity"), "Artist", &[]);
        track.id = TrackId::new("local:track:activity");
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                let generation = store.begin_sync(&source_id)?;
                store.upsert_tracks(&source_id, std::slice::from_ref(&track), generation)
            })
            .expect("seed activity track");
        let mut queue = QueueEngine::new(source_id.clone());
        queue.play_now(&track);
        let entry = queue.current().expect("current entry").clone();
        activate_source(&controller, &saved);

        controller.start_playback_activity(&source_id, &entry, 0);
        controller.record_current_skip_if_needed();
        controller.record_current_skip_if_needed();

        let delta = wait_for_activity_delta(&events);
        assert_eq!(delta.tracks.skip_stats, vec![track.id.clone()]);
        assert!(
            events.try_recv().is_err(),
            "skip emitted more than one delta"
        );

        let detail = smart_detail_named(&controller, &source_id, "Most Skipped");
        assert_eq!(detail.tracks.len(), 1);
        assert_eq!(detail.tracks[0].id, track.id);
        assert_eq!(detail.tracks[0].skip_count, Some(1));
    }

    #[test]
    fn activity_at_threshold_records_play_without_skip() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = local_source_saved();
        let source_id = saved.source.id.clone();
        let mut track = library_track(1, None, AlbumId::new("local:album:activity"), "Artist", &[]);
        track.id = TrackId::new("local:track:activity");
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                let generation = store.begin_sync(&source_id)?;
                store.upsert_tracks(&source_id, std::slice::from_ref(&track), generation)
            })
            .expect("seed activity track");
        let mut queue = QueueEngine::new(source_id.clone());
        queue.play_now(&track);
        let entry = queue.current().expect("current entry").clone();
        let threshold = play_threshold_seconds(entry.duration_seconds);
        activate_source(&controller, &saved);

        controller.start_playback_activity(&source_id, &entry, threshold);
        let play_delta = wait_for_activity_delta(&events);
        assert_eq!(play_delta.tracks.stats, vec![track.id.clone()]);
        assert!(play_delta.tracks.skip_stats.is_empty());
        controller.record_current_skip_if_needed();

        let detail = smart_detail_named(&controller, &source_id, "Most Skipped");
        assert!(detail.tracks.is_empty());
        assert!(events.try_recv().is_err(), "threshold emitted a skip delta");
    }

    #[test]
    fn end_of_stream_then_error_does_not_record_manual_skip() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = local_source_saved();
        let source_id = saved.source.id.clone();
        let mut track = library_track(1, None, AlbumId::new("local:album:activity"), "Artist", &[]);
        track.id = TrackId::new("local:track:activity");
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                let generation = store.begin_sync(&source_id)?;
                store.upsert_tracks(&source_id, std::slice::from_ref(&track), generation)
            })
            .expect("seed activity track");
        let mut queue = QueueEngine::new(source_id.clone());
        queue.play_now(&track);
        let entry = queue.current().expect("current entry").clone();
        *controller.queue.lock().expect("queue") = Some(queue);
        activate_source(&controller, &saved);
        controller.start_playback_activity(&source_id, &entry, 0);

        *controller.playback.lock().expect("playback") = Box::new(PlaybackEvents {
            events: vec![
                PlaybackEvent::EndOfStream,
                PlaybackEvent::Error("stream failed".to_string()),
            ],
        });
        controller.poll_playback_events();

        let play_delta = wait_for_activity_delta(&events);
        assert_eq!(play_delta.tracks.stats, vec![track.id.clone()]);
        assert!(play_delta.tracks.skip_stats.is_empty());
        let detail = smart_detail_named(&controller, &source_id, "Most Skipped");
        assert!(detail.tracks.is_empty());
        let played = smart_detail_named(&controller, &source_id, "Most Played");
        assert_eq!(played.tracks.len(), 1);
        assert_eq!(played.tracks[0].id, track.id);
        assert_eq!(played.tracks[0].play_count, Some(1));
    }

    fn smart_detail_named(
        controller: &AppController,
        source_id: &SourceId,
        name: &str,
    ) -> SmartPlaylistDetail {
        controller
            .store
            .with_store(|store| {
                let page = store.load_smart_playlists(source_id, 0, 20)?;
                let playlist = page
                    .items
                    .into_iter()
                    .find(|playlist| playlist.name == name)
                    .expect("smart playlist");
                store
                    .load_smart_playlist_detail(source_id, &playlist.id)
                    .map(|detail| detail.expect("smart playlist detail"))
            })
            .expect("smart detail")
    }

    fn wait_for_activity_delta(events: &Receiver<ControllerEvent>) -> LibraryDelta {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::LibraryDelta(delta) => return *delta,
                ControllerEvent::Snapshot(_)
                | ControllerEvent::SourceSelectionChanged { .. }
                | ControllerEvent::LibrarySyncStatus(_)
                | ControllerEvent::HomeSectionsUpdated { .. }
                | ControllerEvent::PlaylistChanged { .. }
                | ControllerEvent::SmartPlaylistChanged { .. }
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::Visualizer(_)
                | ControllerEvent::Lyrics { .. }
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSearchFailed { .. }
                | ControllerEvent::SearchLoaded { .. }
                | ControllerEvent::SearchFailed { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
                | ControllerEvent::CoverReady { .. }
                | ControllerEvent::CoverUnavailable { .. }
                | ControllerEvent::CoverDeferred { .. }
                | ControllerEvent::LoginStatus(_) => {}
                ControllerEvent::FavoriteChangeFailed { error, .. } => {
                    panic!("favorite change failed: {error}");
                }
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }
}
