use super::*;

impl AppController {
    pub fn play_activation(&self, activation: PlayActivation) {
        let generation = self.next_play_activation_generation();
        let (shuffle_start, target) = activation.into_parts();
        let activation = PlayActivation { target };
        if !shuffle_start && self.activate_materialized_source_entry(&activation) {
            return;
        }
        match activation.target {
            PlayTarget::StoreBackedSource { source_key, anchor } => {
                self.play_store_backed_source_activation(
                    source_key,
                    anchor,
                    generation,
                    shuffle_start,
                );
            }
            target => {
                let activation = PlayActivation { target };
                self.finish_resolved_play_activation(if shuffle_start {
                    activation.shuffled_start()
                } else {
                    activation
                });
            }
        }
    }

    fn finish_resolved_play_activation(&self, activation: PlayActivation) {
        let shuffle_start = activation.shuffle_start();
        if !shuffle_start && self.activate_materialized_source_entry(&activation) {
            return;
        }
        let activation = match normalize_loaded_source_activation(activation) {
            Ok(activation) => activation,
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
                return;
            }
        };
        match activation.target {
            NormalizedPlayTarget::TrackOnly(track) => {
                self.play_now(*track);
            }
            NormalizedPlayTarget::Replacement(replacement) => {
                self.replace_queue_from_activation(replacement, activation.shuffle_start);
            }
        }
    }

    fn activate_materialized_source_entry(&self, activation: &PlayActivation) -> bool {
        let (source_key, anchor) = match &activation.target {
            PlayTarget::LoadedSource {
                source_key, anchor, ..
            }
            | PlayTarget::StoreBackedSource { source_key, anchor } => (source_key, anchor),
            PlayTarget::ShuffleStart(_) => return false,
            PlayTarget::TrackOnly(_) => return false,
        };

        let mut activated = None;
        let mut skipped_current = false;
        let result = self.with_queue_mut(|queue| {
            let previous_current = queue.current().map(|entry| entry.id.clone());
            activated =
                queue.activate_source_occurrence(source_key, anchor.source_index, &anchor.track_id);
            if let Some(entry_id) = activated.as_ref() {
                skipped_current = previous_current
                    .as_ref()
                    .is_some_and(|current| current != entry_id);
            }
            Ok(())
        });
        if result.is_err() || activated.is_none() {
            return false;
        }
        if skipped_current {
            self.record_current_skip_if_needed();
        }
        self.start_queue_emit();
        self.start_current_track();
        self.auto_dj_top_up_deferred();
        true
    }

    pub(in crate::controller) fn next_play_activation_generation(&self) -> u64 {
        self.play_activation_generation
            .fetch_add(1, Ordering::AcqRel)
            + 1
    }

    pub(in crate::controller) fn invalidate_play_activation_requests(&self) {
        self.play_activation_generation
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(in crate::controller) fn play_activation_generation_matches(
        &self,
        generation: u64,
    ) -> bool {
        self.play_activation_generation.load(Ordering::Acquire) == generation
    }

    fn play_store_backed_source_activation(
        &self,
        source_key: PlaySourceKey,
        anchor: PlayAnchor,
        generation: u64,
        shuffle_start: bool,
    ) {
        let controller = self.clone();
        thread::spawn(move || {
            if !controller.play_activation_generation_matches(generation) {
                return;
            }
            let resolved = controller.resolve_store_backed_source_activation(source_key, anchor);
            if !controller.play_activation_generation_matches(generation) {
                return;
            }
            match resolved {
                Ok((server_id, mut activation)) => {
                    if shuffle_start {
                        activation = activation.shuffled_start();
                    }
                    controller
                        .finish_store_backed_source_activation(&server_id, activation, generation);
                }
                Err(error) => {
                    if controller.play_activation_generation_matches(generation) {
                        let _sent = controller.events.send(ControllerEvent::Error(error));
                    }
                }
            }
        });
    }

    fn resolve_store_backed_source_activation(
        &self,
        source_key: PlaySourceKey,
        anchor: PlayAnchor,
    ) -> Result<(ServerId, PlayActivation), String> {
        let saved = self
            .store
            .with_store(|store| store.active_server())?
            .ok_or_else(|| "No active music server is saved.".to_string())?;
        let settings = load_settings_for_saved(&self.store, &saved);
        let anchor_rank = self.store.with_store(|store| {
            store.track_rank_for_source(
                &saved.server.id,
                &source_key,
                &anchor.track_id,
                anchor.source_item_id.as_deref(),
            )
        })?;
        let Some(anchor_rank) = anchor_rank else {
            return Err("The selected track is no longer available.".to_string());
        };
        let total = self
            .store
            .with_store(|store| store.count_tracks_for_source(&saved.server.id, &source_key))?;
        if total == 0 {
            return Err("No tracks are available to play.".to_string());
        }

        let (before, after) = store_backed_window_extents(total, anchor_rank);
        let mut window = self.store.with_store(|store| {
            store.tracks_window_for_source(
                &saved.server.id,
                &source_key,
                anchor_rank,
                before,
                after,
            )
        })?;
        normalize_store_backed_window_tracks(&self.store, &saved, &settings, &mut window)?;
        let activation = store_backed_window_play_activation(source_key, window, anchor_rank)?;
        Ok((saved.server.id, activation))
    }

    fn finish_store_backed_source_activation(
        &self,
        server_id: &ServerId,
        activation: PlayActivation,
        generation: u64,
    ) {
        if !self.play_activation_generation_matches(generation)
            || !self.active_server_and_queue_match(server_id)
        {
            return;
        }
        self.finish_resolved_play_activation(activation);
    }

    fn active_server_and_queue_match(&self, server_id: &ServerId) -> bool {
        let active_server_matches = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()
            .is_some_and(|saved| saved.server.id == *server_id);
        if !active_server_matches {
            return false;
        }
        self.queue.lock().ok().is_some_and(|queue| {
            queue
                .as_ref()
                .map(|queue| queue.snapshot().server_id == *server_id)
                .unwrap_or(true)
        })
    }

    fn replace_queue_from_activation(&self, replacement: QueueReplacement, shuffle_start: bool) {
        let result = self.with_queue_mut(|queue| {
            queue
                .replace_all(replacement)
                .map_err(|_| "The selected source could not be queued.".to_string())?;
            if shuffle_start {
                queue.start_first_shuffled();
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.start_queue_emit();
        self.start_current_track();
        self.auto_dj_top_up_deferred();
    }

    pub fn play_tracks_now(&self, tracks: Vec<Track>) {
        self.invalidate_play_activation_requests();
        if tracks.is_empty() {
            let _sent = self.events.send(ControllerEvent::Error(
                "No tracks are available to play.".to_string(),
            ));
            return;
        }
        let result = self.with_queue_mut(|queue| {
            queue.clear();
            let mut tracks = tracks.into_iter();
            if let Some(first) = tracks.next() {
                queue.play_now(&first);
            }
            for track in tracks {
                queue.append(&track);
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.start_queue_emit();
        self.start_current_track();
        self.auto_dj_top_up_deferred();
    }
    pub fn play_now(&self, track: Track) {
        self.play_tracks_now(vec![track]);
    }
    pub fn play_album_now(&self, album_id: AlbumId) {
        match self.cached_album_detail(&album_id) {
            Ok(Some((album, tracks))) => {
                if tracks.is_empty() {
                    let _sent = self.events.send(ControllerEvent::Error(
                        "No tracks are available to play.".to_string(),
                    ));
                    return;
                }
                self.play_activation(Self::album_play_activation(
                    album.id,
                    tracks,
                    Self::active_music_folder(&self.store),
                ));
            }
            Ok(None) => {
                let _sent = self.events.send(ControllerEvent::Error(
                    "The selected cached album was not found.".to_string(),
                ));
            }
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
            }
        }
    }

    fn album_play_activation(
        album_id: AlbumId,
        tracks: Vec<Track>,
        selected_music_folder_id: Option<MusicFolderId>,
    ) -> PlayActivation {
        let anchor_track_id = tracks
            .first()
            .expect("album activation has a first track")
            .id
            .clone();
        PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: PlaySourceKey {
                    descriptor: PlaySourceDescriptor::Album {
                        album_id,
                        selected_music_folder_id,
                    },
                    order: SourceOrder::Canonical,
                },
                completeness: LoadedCompleteness::Complete,
                items: tracks
                    .into_iter()
                    .enumerate()
                    .map(|(source_index, track)| PlaySourceItem {
                        track,
                        source_index,
                        source_item_id: None,
                    })
                    .collect(),
                anchor: PlayAnchor {
                    track_id: anchor_track_id,
                    source_index: 0,
                    source_item_id: None,
                },
            },
        }
        .shuffled_start()
    }

    fn active_music_folder(store: &StoreHandle) -> Option<MusicFolderId> {
        store
            .with_store(|store| {
                let Some(saved) = store.active_server()? else {
                    return Ok(None);
                };
                store.selected_music_folder_id(&saved.server.id)
            })
            .ok()
            .flatten()
    }
    pub fn play_next(&self, track: Track) {
        self.invalidate_play_activation_requests();
        let result = self.with_queue_mut(|queue| {
            queue.play_next(&track);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn play_last(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            let _sent = self.events.send(ControllerEvent::Error(
                "No tracks are available to add to the queue.".to_string(),
            ));
            return;
        }
        self.invalidate_play_activation_requests();
        let result = self.with_queue_mut(|queue| {
            for track in &tracks {
                queue.append(track);
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn remove_from_queue(&self, entry_id: QueueEntryId) {
        self.invalidate_play_activation_requests();
        let mut removed_current = false;
        let mut has_current_after_remove = false;
        let result = self.with_queue_mut(|queue| {
            let current_id = queue.current().map(|entry| entry.id.clone());
            let removed = queue.remove(&entry_id).is_some();
            removed_current = removed && current_id.as_ref() == Some(&entry_id);
            has_current_after_remove = queue.current().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if removed_current {
            self.record_current_skip_if_needed();
        }
        if removed_current && !has_current_after_remove {
            invalidate_playback_requests(&self.playback_request_generation);
            let _result = self.send_playback_command(PlaybackCommand::Stop);
            self.clear_playback_activity();
        }
        if removed_current && has_current_after_remove {
            self.start_queue_emit();
            self.start_current_track();
        } else {
            self.persist_and_emit_queue();
        }
    }
    pub fn activate_queue_entry(&self, entry_id: QueueEntryId) {
        self.invalidate_play_activation_requests();
        let previous_current = self
            .queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref()?.current().map(|entry| entry.id.clone()));
        let result = self.with_queue_mut(|queue| {
            if queue.activate(&entry_id) {
                Ok(())
            } else {
                Err("The selected queue entry was not found.".to_string())
            }
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if previous_current
            .as_ref()
            .is_some_and(|current| current != &entry_id)
        {
            self.record_current_skip_if_needed();
        }
        self.start_queue_emit();
        self.restart_current_track();
        self.auto_dj_top_up_deferred();
    }
    pub fn move_queue_entry_after_current(&self, entry_id: QueueEntryId) {
        self.invalidate_play_activation_requests();
        let result = self.with_queue_mut(|queue| {
            if queue.move_after_current(&entry_id) {
                Ok(())
            } else {
                Err("The selected queue entry was not found.".to_string())
            }
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn reorder_queue_entry(&self, entry_id: QueueEntryId, target_index: usize, after: bool) {
        let result = self.with_queue_mut(|queue| {
            let snapshot = queue.snapshot();
            let Some(old_index) = snapshot
                .entries
                .iter()
                .position(|entry| entry.id == entry_id)
            else {
                return Err("The selected queue entry was not found.".to_string());
            };
            let mut new_index = target_index.saturating_add(usize::from(after));
            if old_index < new_index {
                new_index = new_index.saturating_sub(1);
            }
            if queue.reorder(&entry_id, new_index) {
                Ok(())
            } else {
                Err("The selected queue entry was not found.".to_string())
            }
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn clear_queue(&self) {
        self.invalidate_play_activation_requests();
        let mut kept_current = false;
        let result = self.with_queue_mut(|queue| {
            kept_current = queue.clear_except_current();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if kept_current {
            self.start_queue_emit();
            let _result = self.send_playback_command(PlaybackCommand::PrepareNext(None));
            return;
        }
        self.persist_and_emit_queue();
        invalidate_playback_requests(&self.playback_request_generation);
        self.clear_playback_activity();
        let _result = self.send_playback_command(PlaybackCommand::Stop);
    }
    pub fn toggle_shuffle(&self) {
        let result = self.with_queue_mut(|queue| {
            let enabled = !queue.shuffle().enabled;
            let seed = if enabled {
                shuffle_seed()
            } else {
                queue.shuffle().seed
            };
            queue.set_shuffle(enabled, seed);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_playback();
        self.prepare_next_stream();
    }
    pub fn cycle_repeat(&self) {
        let result = self.with_queue_mut(|queue| {
            let next = match queue.repeat_mode() {
                RepeatMode::Off => RepeatMode::All,
                RepeatMode::All => RepeatMode::One,
                RepeatMode::One => RepeatMode::Off,
            };
            queue.set_repeat_mode(next);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_playback();
        self.prepare_next_stream();
    }
}

fn store_backed_window_extents(total: usize, anchor_rank: usize) -> (usize, usize) {
    if total <= FULL_LOADED_LIMIT {
        return (
            anchor_rank,
            total.saturating_sub(anchor_rank.saturating_add(1)),
        );
    }
    let before = anchor_rank.min(MATERIALIZED_WINDOW_BEFORE_ANCHOR);
    let after = MATERIALIZED_WINDOW_LIMIT.saturating_sub(before.saturating_add(1));
    (before, after)
}

fn normalize_store_backed_window_tracks(
    store: &StoreHandle,
    saved: &SavedServer,
    settings: &AppSettings,
    window: &mut StoreBackedSourceWindow,
) -> Result<(), String> {
    let mut tracks = window
        .items
        .iter()
        .map(|item| item.track.clone())
        .collect::<Vec<_>>();
    scrub_selected_track_image_refs(saved, settings, &mut tracks);
    cover_art_policy::bind_tracks(&mut tracks, settings);
    track_album_refs(store, saved, &mut tracks, &[])?;
    for (item, track) in window.items.iter_mut().zip(tracks) {
        item.track = track;
    }
    Ok(())
}

fn store_backed_window_play_activation(
    source_key: PlaySourceKey,
    window: StoreBackedSourceWindow,
    anchor_rank: usize,
) -> Result<PlayActivation, String> {
    let anchor = window
        .items
        .iter()
        .find(|item| item.source_index == anchor_rank)
        .map(|item| PlayAnchor {
            track_id: item.track.id.clone(),
            source_index: anchor_rank,
            source_item_id: item.source_item_id.clone(),
        })
        .ok_or_else(|| "The selected track is no longer available.".to_string())?;
    let completeness = if window.start_rank == 0
        && window.items.len() == window.total_source_items
        && window.total_source_items <= FULL_LOADED_LIMIT
    {
        LoadedCompleteness::Complete
    } else {
        LoadedCompleteness::Window {
            start: window.start_rank,
            total: Some(window.total_source_items),
        }
    };
    Ok(PlayActivation {
        target: PlayTarget::LoadedSource {
            source_key,
            completeness,
            items: window
                .items
                .into_iter()
                .map(|item| PlaySourceItem {
                    track: item.track,
                    source_index: item.source_index,
                    source_item_id: item.source_item_id,
                })
                .collect(),
            anchor,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{AlbumId, Playlist, PlaylistEntrySortDescriptor, ServerIdentity};
    use source::PlaylistEntry;

    struct RecordingPlaybackBackend {
        commands: Arc<Mutex<Vec<PlaybackCommand>>>,
    }

    impl PlaybackBackend for RecordingPlaybackBackend {
        fn send(&mut self, command: PlaybackCommand) -> Result<(), playback::PlaybackError> {
            self.commands.lock().expect("commands").push(command);
            Ok(())
        }

        fn drain_events(&mut self) -> Vec<PlaybackEvent> {
            Vec::new()
        }
    }

    #[test]
    fn queue_replace_occurrence() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = SavedServer {
            server: ServerIdentity {
                id: ServerId::new("fake:server:queue"),
                provider: "fake".to_string(),
                name: "Queue Test".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        };
        let mut repeated_track = library_track(1, None, AlbumId::fake(1), "Artist", &[]);
        repeated_track.title = "Repeated".to_string();
        let other_track = library_track(2, None, AlbumId::fake(1), "Artist", &[]);
        let playlist = Playlist {
            id: PlaylistId::fake(1),
            name: "Playlist".to_string(),
            track_count: 3,
            duration_seconds: 540,
            top_genres: Vec::new(),
            image_refs: Vec::new(),
            image_ref: None,
        };
        let entries = vec![
            PlaylistEntry {
                entry_id: "entry-one".to_string(),
                track: repeated_track.clone(),
            },
            PlaylistEntry {
                entry_id: "entry-two".to_string(),
                track: repeated_track.clone(),
            },
            PlaylistEntry {
                entry_id: "entry-three".to_string(),
                track: other_track.clone(),
            },
        ];
        controller
            .store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                let generation = store.begin_sync(&saved.server.id)?;
                store.upsert_tracks(
                    &saved.server.id,
                    &[repeated_track.clone(), other_track],
                    generation,
                )?;
                store.upsert_playlists(
                    &saved.server.id,
                    std::slice::from_ref(&playlist),
                    generation,
                )?;
                store.upsert_playlist_entries(
                    &saved.server.id,
                    &playlist.id,
                    &entries,
                    generation,
                )?;
                store.complete_sync(&saved.server.id, generation)?;
                Ok(())
            })
            .expect("seed store");
        *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.server.id.clone()));

        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: playlist.id,
            },
            order: SourceOrder::PlaylistDisplayed {
                query: None,
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        };
        controller.play_activation(PlayActivation {
            target: PlayTarget::StoreBackedSource {
                source_key: source_key.clone(),
                anchor: PlayAnchor {
                    track_id: repeated_track.id.clone(),
                    source_index: 1,
                    source_item_id: Some("entry-two".to_string()),
                },
            },
        });

        let snapshot = wait_for_queue(&events).expect("queue snapshot");
        assert_eq!(snapshot.current_index, Some(1));
        assert_eq!(
            snapshot
                .entries
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<Vec<_>>(),
            vec![
                repeated_track.id.clone(),
                repeated_track.id.clone(),
                TrackId::fake(2),
            ]
        );
        assert!(snapshot.entries[1].origin.is_some());
        let initial_ids = snapshot
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();

        controller.play_activation(PlayActivation {
            target: PlayTarget::StoreBackedSource {
                source_key,
                anchor: PlayAnchor {
                    track_id: TrackId::fake(2),
                    source_index: 2,
                    source_item_id: Some("entry-three".to_string()),
                },
            },
        });

        let moved = wait_for_queue(&events).expect("materialized store queue");
        assert_eq!(moved.current_index, Some(2));
        assert_eq!(
            moved
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            initial_ids
        );
    }

    #[test]
    fn queue_same_source_activation_reuses_entries() {
        let (controller, events, snapshot, ..) =
            AppController::bootstrap_with_fake(FakeScale::Small);
        let tracks = snapshot.tracks[0..3].to_vec();
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::fake(9),
            },
            order: SourceOrder::PlaylistDisplayed {
                query: None,
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        };
        let activation = |anchor_index: usize| PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: source_key.clone(),
                completeness: LoadedCompleteness::Complete,
                items: tracks
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(source_index, track)| PlaySourceItem {
                        track,
                        source_index,
                        source_item_id: Some(format!("entry-{source_index}")),
                    })
                    .collect(),
                anchor: PlayAnchor {
                    track_id: tracks[anchor_index].id.clone(),
                    source_index: anchor_index,
                    source_item_id: Some(format!("entry-{anchor_index}")),
                },
            },
        };

        controller.play_activation(activation(0));
        let initial_queue = wait_for_queue(&events).expect("initial source queue");
        let initial_ids = initial_queue
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();

        controller.play_activation(activation(0));
        let duplicate_queue = wait_for_queue(&events).expect("duplicate source queue");
        assert_eq!(
            duplicate_queue
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            initial_ids
        );

        controller.play_activation(activation(2));
        let moved_queue = wait_for_queue(&events).expect("moved source queue");
        assert_eq!(moved_queue.current_index, Some(2));
        assert_eq!(
            moved_queue
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            initial_ids
        );
    }

    #[test]
    fn source_shuffle_start_uses_shuffled_current() {
        let (controller, events, snapshot, ..) =
            AppController::bootstrap_with_fake(FakeScale::Small);
        let tracks = snapshot.tracks[0..4].to_vec();
        controller
            .with_queue_mut(|queue| {
                queue.set_shuffle(true, 19);
                Ok(())
            })
            .expect("set shuffle");
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::fake(9),
            },
            order: SourceOrder::PlaylistDisplayed {
                query: None,
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        };

        controller.play_activation(
            PlayActivation {
                target: PlayTarget::LoadedSource {
                    source_key,
                    completeness: LoadedCompleteness::Complete,
                    items: tracks
                        .iter()
                        .cloned()
                        .enumerate()
                        .map(|(source_index, track)| PlaySourceItem {
                            track,
                            source_index,
                            source_item_id: Some(format!("entry-{source_index}")),
                        })
                        .collect(),
                    anchor: PlayAnchor {
                        track_id: tracks[0].id.clone(),
                        source_index: 0,
                        source_item_id: Some("entry-0".to_string()),
                    },
                },
            }
            .shuffled_start(),
        );

        let queue = wait_for_queue(&events).expect("source queue");
        assert_eq!(queue.current_index, queue.shuffle_order.first().copied());
        assert_ne!(queue.current_index, Some(0));
    }

    #[test]
    fn queue_reorder_preserves_current_entry() {
        let (controller, events, snapshot, ..) =
            AppController::bootstrap_with_fake(FakeScale::Small);
        let tracks = snapshot.tracks[0..3].to_vec();
        let current_track_id = tracks[0].id.clone();
        let last_track_id = tracks[2].id.clone();
        controller.play_tracks_now(tracks);
        let initial = wait_for_queue(&events).expect("initial queue");
        let last_entry_id = initial.entries[2].id.clone();

        controller.reorder_queue_entry(last_entry_id, 0, false);

        let reordered = wait_for_queue(&events).expect("reordered queue");
        assert_eq!(reordered.entries[0].track_id, last_track_id);
        assert_eq!(
            reordered.entries[reordered.current_index.expect("current")].track_id,
            current_track_id
        );
    }

    #[test]
    fn queue_windowed_source_activation_replaces_entries() {
        let (controller, events, ..) = AppController::bootstrap_with_fake(FakeScale::Small);
        let tracks = (1..=8)
            .map(|number| library_track(number, None, AlbumId::fake(1), "Artist", &[]))
            .collect::<Vec<_>>();
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::fake(10),
            },
            order: SourceOrder::PlaylistDisplayed {
                query: None,
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        };
        let activation = |start: usize, anchor_offset: usize| PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: source_key.clone(),
                completeness: LoadedCompleteness::Window {
                    start,
                    total: Some(tracks.len()),
                },
                items: tracks[start..start + 5]
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(offset, track)| PlaySourceItem {
                        track,
                        source_index: start + offset,
                        source_item_id: None,
                    })
                    .collect(),
                anchor: PlayAnchor {
                    track_id: tracks[start + anchor_offset].id.clone(),
                    source_index: start + anchor_offset,
                    source_item_id: None,
                },
            },
        };

        controller.play_activation(activation(0, 0));
        let initial_queue = wait_for_queue(&events).expect("initial source queue");
        let initial_ids = initial_queue
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();

        controller.play_activation(activation(0, 4));
        let same_window_queue = wait_for_queue(&events).expect("same window source queue");
        assert_eq!(same_window_queue.current_index, Some(4));
        assert_eq!(
            same_window_queue
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            initial_ids
        );

        controller.play_activation(activation(3, 3));
        let moved_queue = wait_for_queue(&events).expect("windowed source queue");

        assert_eq!(moved_queue.current_index, Some(3));
        assert_eq!(moved_queue.entries[0].track_id, tracks[3].id);
        assert_eq!(moved_queue.entries[4].track_id, tracks[7].id);
        assert_ne!(
            moved_queue
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            initial_ids
        );
    }

    #[test]
    fn queue_clear_activation() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = SavedServer {
            server: ServerIdentity {
                id: ServerId::new("fake:server:stale-activation"),
                provider: "fake".to_string(),
                name: "Queue Test".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        };
        controller
            .store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                Ok(())
            })
            .expect("seed store");
        let current = library_track(1, None, AlbumId::fake(1), "Artist", &[]);
        let stale = library_track(2, None, AlbumId::fake(1), "Artist", &[]);
        let mut queue = QueueEngine::new(saved.server.id.clone());
        queue.append(&current);
        *controller.queue.lock().expect("queue") = Some(queue);

        let generation = controller.next_play_activation_generation();
        let stale_activation = PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: PlaySourceKey {
                    descriptor: PlaySourceDescriptor::Album {
                        album_id: AlbumId::fake(1),
                        selected_music_folder_id: None,
                    },
                    order: SourceOrder::Canonical,
                },
                completeness: LoadedCompleteness::Complete,
                items: vec![PlaySourceItem {
                    track: stale.clone(),
                    source_index: 0,
                    source_item_id: None,
                }],
                anchor: PlayAnchor {
                    track_id: stale.id,
                    source_index: 0,
                    source_item_id: None,
                },
            },
        };

        controller.clear_queue();
        let queue = wait_for_queue(&events).expect("cleared queue");
        assert_eq!(queue.entries.len(), 1);
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(queue.entries[0].track_id, current.id);

        controller.finish_store_backed_source_activation(
            &saved.server.id,
            stale_activation,
            generation,
        );
        let queue = controller.queue_snapshot().expect("queue snapshot");
        assert_eq!(queue.entries.len(), 1);
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(queue.entries[0].track_id, current.id);
    }

    #[test]
    fn queue_clear_keeps_current_playback() {
        let (mut controller, events, snapshot, ..) =
            AppController::bootstrap_with_fake(FakeScale::Small);
        let commands = Arc::new(Mutex::new(Vec::new()));
        controller.playback = Arc::new(Mutex::new(Box::new(RecordingPlaybackBackend {
            commands: Arc::clone(&commands),
        })));
        let saved = snapshot.server.expect("server");
        let current = snapshot.tracks[0].clone();
        let next = snapshot.tracks[1].clone();
        let mut queue = QueueEngine::new(saved.id);
        queue.append(&current);
        queue.append(&next);
        *controller.queue.lock().expect("queue") = Some(queue);

        controller.clear_queue();

        let queue = wait_for_queue(&events).expect("cleared queue");
        assert_eq!(queue.entries.len(), 1);
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(queue.entries[0].track_id, current.id);
        assert!(
            !commands
                .lock()
                .expect("commands")
                .iter()
                .any(|command| matches!(command, PlaybackCommand::Stop))
        );
        assert_eq!(
            commands
                .lock()
                .expect("commands")
                .iter()
                .filter(|command| matches!(command, PlaybackCommand::PrepareNext(None)))
                .count(),
            1
        );
        assert!(
            !commands
                .lock()
                .expect("commands")
                .iter()
                .any(|command| matches!(command, PlaybackCommand::PrepareNext(Some(_))))
        );
    }

    #[test]
    fn queue_store_activation() {
        let (controller, events, snapshot, ..) =
            AppController::bootstrap_with_fake(FakeScale::Small);
        let saved = snapshot.server.expect("server");
        let current = snapshot.tracks[0].clone();
        let stale = snapshot.tracks[1].clone();
        let mut queue = QueueEngine::new(saved.id.clone());
        queue.append(&current);
        *controller.queue.lock().expect("queue") = Some(queue);

        let generation = controller.next_play_activation_generation();
        let stale_activation = PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: PlaySourceKey {
                    descriptor: PlaySourceDescriptor::Album {
                        album_id: AlbumId::fake(1),
                        selected_music_folder_id: None,
                    },
                    order: SourceOrder::Canonical,
                },
                completeness: LoadedCompleteness::Complete,
                items: vec![PlaySourceItem {
                    track: stale.clone(),
                    source_index: 0,
                    source_item_id: None,
                }],
                anchor: PlayAnchor {
                    track_id: stale.id.clone(),
                    source_index: 0,
                    source_item_id: None,
                },
            },
        };

        controller.play_random_tracks(random_request(RandomPlayAction::AddLast, 2));
        let random_queue = wait_for_queue(&events).expect("random queue");
        assert!(random_queue.entries.len() > 1);

        controller.finish_store_backed_source_activation(&saved.id, stale_activation, generation);
        let queue = controller.queue_snapshot().expect("queue snapshot");
        assert_eq!(queue.entries.len(), random_queue.entries.len());
        assert_eq!(queue.entries[0].track_id, current.id);
        assert!(
            queue
                .entries
                .iter()
                .skip(1)
                .any(|entry| entry.track_id != stale.id)
        );
    }
}
