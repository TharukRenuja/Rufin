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
                Ok((source_id, mut activation)) => {
                    if shuffle_start {
                        activation = activation.shuffled_start();
                    }
                    controller
                        .finish_store_backed_source_activation(&source_id, activation, generation);
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
    ) -> Result<(SourceId, PlayActivation), String> {
        let saved = self
            .store
            .with_store(|store| store.active_source())?
            .ok_or_else(|| "No active music server is saved.".to_string())?;
        let settings = load_settings_from_store(&self.store);
        let anchor_rank = self.store.with_store(|store| {
            store.track_rank_for_source(
                &saved.source.id,
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
            .with_store(|store| store.count_tracks_for_source(&saved.source.id, &source_key))?;
        if total == 0 {
            return Err("No tracks are available to play.".to_string());
        }

        let (before, after) = store_backed_window_extents(total, anchor_rank);
        let mut window = self.store.with_store(|store| {
            store.tracks_window_for_source(
                &saved.source.id,
                &source_key,
                anchor_rank,
                before,
                after,
            )
        })?;
        normalize_store_backed_window_tracks(&self.store, &saved, &settings, &mut window)?;
        let activation = store_backed_window_play_activation(source_key, window, anchor_rank)?;
        Ok((saved.source.id, activation))
    }

    fn finish_store_backed_source_activation(
        &self,
        source_id: &SourceId,
        activation: PlayActivation,
        generation: u64,
    ) {
        if !self.play_activation_generation_matches(generation)
            || !self.active_source_and_queue_match(source_id)
        {
            return;
        }
        self.finish_resolved_play_activation(activation);
    }

    fn active_source_and_queue_match(&self, source_id: &SourceId) -> bool {
        let active_source_matches = self
            .store
            .with_store(|store| store.active_source())
            .ok()
            .flatten()
            .is_some_and(|saved| saved.source.id == *source_id);
        if !active_source_matches {
            return false;
        }
        self.queue.lock().ok().is_some_and(|queue| {
            queue
                .as_ref()
                .map(|queue| queue.snapshot().source_id == *source_id)
                .unwrap_or(true)
        })
    }

    fn replace_queue_from_activation(&self, replacement: QueueReplacement, shuffle_start: bool) {
        let result = self.with_queue_mut(|queue| {
            let previous_track_id = queue.current().map(|entry| entry.track_id.clone());
            queue
                .replace_all(replacement)
                .map_err(|_| "The selected source could not be queued.".to_string())?;
            if shuffle_start {
                if queue.shuffle().enabled {
                    queue.start_first_shuffled_with_seed_avoiding(
                        shuffle_seed(),
                        previous_track_id.as_ref(),
                    );
                } else {
                    queue.start_first_shuffled();
                }
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
    pub fn play_album_tracks(
        &self,
        album_id: AlbumId,
        tracks: Vec<Track>,
        anchor_index: usize,
        shuffled_start: bool,
    ) {
        let Some(activation) = Self::album_play_activation(
            album_id,
            tracks,
            anchor_index,
            Self::active_music_folder(&self.store),
        ) else {
            let _sent = self.events.send(ControllerEvent::Error(
                "The selected track is no longer available.".to_string(),
            ));
            return;
        };
        self.play_activation(if shuffled_start {
            activation.shuffled_start()
        } else {
            activation
        });
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
                let Some(activation) = Self::album_play_activation(
                    album.id,
                    tracks,
                    0,
                    Self::active_music_folder(&self.store),
                ) else {
                    let _sent = self.events.send(ControllerEvent::Error(
                        "No tracks are available to play.".to_string(),
                    ));
                    return;
                };
                self.play_activation(activation.shuffled_start());
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
        anchor_index: usize,
        selected_music_folder_id: Option<MusicFolderId>,
    ) -> Option<PlayActivation> {
        let anchor_track_id = tracks.get(anchor_index)?.id.clone();
        Some(PlayActivation {
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
                    source_index: anchor_index,
                    source_item_id: None,
                },
            },
        })
    }

    pub fn play_playlist_entry(
        &self,
        playlist_id: PlaylistId,
        entry: PlaylistEntry,
        source_index: usize,
        query: Option<String>,
        sort: (PlaylistEntrySortDescriptor, bool),
        shuffled_start: bool,
    ) {
        let (sort, descending) = sort;
        let activation = PlayActivation {
            target: PlayTarget::StoreBackedSource {
                source_key: PlaySourceKey {
                    descriptor: PlaySourceDescriptor::Playlist { playlist_id },
                    order: SourceOrder::PlaylistDisplayed {
                        query,
                        sort,
                        descending,
                    },
                },
                anchor: PlayAnchor {
                    track_id: entry.track.id,
                    source_index,
                    source_item_id: Some(entry.entry_id),
                },
            },
        };
        self.play_activation(if shuffled_start {
            activation.shuffled_start()
        } else {
            activation
        });
    }
    pub fn play_playlist_detail(&self, detail: PlaylistDetail) {
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: detail.playlist.id.clone(),
            },
            order: SourceOrder::PlaylistDisplayed {
                query: None,
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        };
        if let Some(activation) = playlist_detail_activation(source_key, detail) {
            self.play_activation(activation);
        }
    }
    pub fn play_cached_playlist(&self, playlist_id: PlaylistId) {
        let generation = self.next_play_activation_generation();
        let controller = self.clone();
        thread::spawn(move || {
            if !controller.play_activation_generation_matches(generation) {
                return;
            }
            if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id)
                && controller.play_activation_generation_matches(generation)
            {
                controller.play_playlist_detail(detail);
            }
        });
    }
    pub fn play_cached_playlist_next(&self, playlist_id: PlaylistId) {
        let generation = self.next_play_activation_generation();
        let controller = self.clone();
        thread::spawn(move || {
            if !controller.play_activation_generation_matches(generation) {
                return;
            }
            if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id)
                && controller.play_activation_generation_matches(generation)
            {
                for track in detail.tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        });
    }
    pub fn play_cached_playlist_last(&self, playlist_id: PlaylistId) {
        let generation = self.next_play_activation_generation();
        let controller = self.clone();
        thread::spawn(move || {
            if !controller.play_activation_generation_matches(generation) {
                return;
            }
            if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id)
                && controller.play_activation_generation_matches(generation)
            {
                controller.play_last(detail.tracks);
            }
        });
    }
    pub fn play_smart_playlist_detail(&self, detail: SmartPlaylistDetail) {
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::SmartPlaylist {
                smart_playlist_id: detail.smart_playlist.id,
                definition_fingerprint: smart_playlist_definition_fingerprint(
                    &detail.smart_playlist.definition,
                ),
                selected_music_folder_id: Self::active_music_folder(&self.store),
            },
            order: SourceOrder::SmartPlaylistDefinition {
                sort: SmartPlaylistSortDescriptor::Definition,
                limit: detail.smart_playlist.definition.limit,
                skip_count: 0,
            },
        };
        if let Some(activation) = loaded_tracks_activation(source_key, &detail.tracks, 0) {
            self.play_activation(activation.shuffled_start());
        }
    }
    pub fn play_loaded_source_window(
        &self,
        source_key: PlaySourceKey,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        self.play_loaded_source_window_with_shuffle_start(
            source_key,
            total_items,
            anchor_index,
            track_at,
            false,
        )
    }
    fn play_loaded_source_window_with_shuffle_start(
        &self,
        source_key: PlaySourceKey,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
        shuffle_start: bool,
    ) -> bool {
        let Some(activation) =
            loaded_tracks_window_activation(source_key, total_items, anchor_index, track_at)
        else {
            return false;
        };
        self.play_activation(if shuffle_start {
            activation.shuffled_start()
        } else {
            activation
        });
        true
    }
    pub fn play_library_source_window(
        &self,
        descriptor: PlaySourceDescriptor,
        source: (LibraryListSettings, String, bool),
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        let (settings, query, favorite_first) = source;
        self.play_loaded_source_window(
            PlaySourceKey {
                descriptor,
                order: library_displayed_source_order(&settings, &query, favorite_first),
            },
            total_items,
            anchor_index,
            track_at,
        )
    }
    pub fn play_folder_window(
        &self,
        path: Vec<FolderPathItem>,
        query: String,
        settings: TrackTableSettings,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        self.play_loaded_source_window(
            PlaySourceKey {
                descriptor: PlaySourceDescriptor::FolderLoaded {
                    path: path.into_iter().map(|entry| entry.name).collect(),
                    selected_music_folder_id: Self::active_music_folder(&self.store),
                },
                order: SourceOrder::FolderDisplayed {
                    query: source_query(&query),
                    filter_key: track_table_filter_key(&settings, &query, false),
                    sort: track_sort_descriptor(settings.sort_key),
                },
            },
            total_items,
            anchor_index,
            track_at,
        )
    }
    pub fn play_artist_tracks_window(
        &self,
        artist_id: ArtistId,
        scope: ArtistTrackScope,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        self.play_loaded_source_window_with_shuffle_start(
            PlaySourceKey {
                descriptor: PlaySourceDescriptor::ArtistTracks {
                    artist_id,
                    scope,
                    selected_music_folder_id: Self::active_music_folder(&self.store),
                },
                order: SourceOrder::Canonical,
            },
            total_items,
            anchor_index,
            track_at,
            true,
        )
    }
    pub fn play_genre_tracks_window(
        &self,
        genre_id: GenreId,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        self.play_loaded_source_window_with_shuffle_start(
            PlaySourceKey {
                descriptor: PlaySourceDescriptor::GenreTracks {
                    genre_id,
                    selected_music_folder_id: Self::active_music_folder(&self.store),
                },
                order: SourceOrder::Canonical,
            },
            total_items,
            anchor_index,
            track_at,
            true,
        )
    }
    pub fn play_mood_tracks_window(
        &self,
        mood_id: MoodId,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        self.play_loaded_source_window_with_shuffle_start(
            PlaySourceKey {
                descriptor: PlaySourceDescriptor::MoodTracks {
                    mood_id,
                    selected_music_folder_id: Self::active_music_folder(&self.store),
                },
                order: SourceOrder::Canonical,
            },
            total_items,
            anchor_index,
            track_at,
            true,
        )
    }

    fn active_music_folder(store: &StoreHandle) -> Option<MusicFolderId> {
        store
            .with_store(|store| {
                let Some(saved) = store.active_source()? else {
                    return Ok(None);
                };
                store.selected_music_folder_id(&saved.source.id)
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

fn library_displayed_source_order(
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) -> SourceOrder {
    SourceOrder::LibraryDisplayed {
        filter_key: library_filter_key(settings, query, favorite_first),
        sort: library_sort_descriptor(settings.sort_key),
    }
}

fn library_filter_key(
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) -> Option<String> {
    let query = query.trim();
    Some(format!(
        "query={};sort={};descending={};favorite-first={}",
        query,
        library_field_key(settings.sort_key),
        settings.descending,
        favorite_first
    ))
}

fn track_table_filter_key(
    settings: &TrackTableSettings,
    query: &str,
    favorite_first: bool,
) -> Option<String> {
    let query = query.trim();
    Some(format!(
        "query={};sort={};descending={};favorite-first={}",
        query,
        track_sort_key(settings.sort_key),
        settings.descending,
        favorite_first
    ))
}

fn source_query(query: &str) -> Option<String> {
    let query = query.trim();
    (!query.is_empty()).then(|| query.to_string())
}

fn track_sort_key(sort_key: TrackSortKey) -> &'static str {
    match sort_key {
        TrackSortKey::TrackNumber => "track-number",
        TrackSortKey::Title => "title",
        TrackSortKey::Artist => "artist",
        TrackSortKey::Album => "album",
        TrackSortKey::Year => "year",
        TrackSortKey::Duration => "duration",
        TrackSortKey::Favorite => "favorite",
    }
}

fn track_sort_descriptor(sort_key: TrackSortKey) -> TrackSortDescriptor {
    match sort_key {
        TrackSortKey::TrackNumber => TrackSortDescriptor::TrackNumber,
        TrackSortKey::Title => TrackSortDescriptor::Title,
        TrackSortKey::Artist => TrackSortDescriptor::Artist,
        TrackSortKey::Album => TrackSortDescriptor::Album,
        TrackSortKey::Year | TrackSortKey::Duration | TrackSortKey::Favorite => {
            TrackSortDescriptor::Title
        }
    }
}

fn library_sort_descriptor(field: LibraryField) -> TrackSortDescriptor {
    match field {
        LibraryField::TrackNumber | LibraryField::DiscNumber => TrackSortDescriptor::TrackNumber,
        LibraryField::Title | LibraryField::TitleMerged => TrackSortDescriptor::Title,
        LibraryField::Artist | LibraryField::AlbumArtist => TrackSortDescriptor::Artist,
        LibraryField::Album => TrackSortDescriptor::Album,
        LibraryField::DateAdded => TrackSortDescriptor::DateAdded,
        _ => TrackSortDescriptor::Title,
    }
}

fn library_field_key(field: LibraryField) -> &'static str {
    match field {
        LibraryField::RowIndex => "row-index",
        LibraryField::Image => "image",
        LibraryField::Title => "title",
        LibraryField::TitleMerged => "title-merged",
        LibraryField::Artist => "artist",
        LibraryField::AlbumArtist => "album-artist",
        LibraryField::Album => "album",
        LibraryField::Year => "year",
        LibraryField::ReleaseDate => "release-date",
        LibraryField::DateAdded => "date-added",
        LibraryField::LastPlayed => "last-played",
        LibraryField::PlayCount => "play-count",
        LibraryField::UserRating => "user-rating",
        LibraryField::Genre => "genre",
        LibraryField::TrackNumber => "track-number",
        LibraryField::DiscNumber => "disc-number",
        LibraryField::SongCount => "song-count",
        LibraryField::AlbumCount => "album-count",
        LibraryField::Duration => "duration",
        LibraryField::Favorite => "favorite",
    }
}

fn playlist_detail_activation(
    source_key: PlaySourceKey,
    detail: PlaylistDetail,
) -> Option<PlayActivation> {
    if detail.entries.is_empty() {
        return loaded_tracks_activation(source_key, &detail.tracks, 0)
            .map(PlayActivation::shuffled_start);
    }

    let total_items = detail.entries.len();
    let (_before, after) = store_backed_window_extents(total_items, 0);
    let end = (after + 1).min(total_items);
    let window = StoreBackedSourceWindow {
        start_rank: 0,
        total_source_items: total_items,
        items: detail
            .entries
            .into_iter()
            .enumerate()
            .take(end)
            .map(|(source_index, entry)| StoreBackedSourceItem {
                track: entry.track,
                source_index,
                source_item_id: Some(entry.entry_id),
            })
            .collect(),
    };
    store_backed_window_play_activation(source_key, window, 0)
        .ok()
        .map(PlayActivation::shuffled_start)
}

fn loaded_tracks_activation(
    source_key: PlaySourceKey,
    tracks: &[Track],
    anchor_index: usize,
) -> Option<PlayActivation> {
    loaded_tracks_window_activation(source_key, tracks.len(), anchor_index, |index| {
        tracks.get(index).cloned()
    })
}

fn loaded_tracks_window_activation(
    source_key: PlaySourceKey,
    total_items: usize,
    anchor_index: usize,
    mut track_at: impl FnMut(usize) -> Option<Track>,
) -> Option<PlayActivation> {
    if total_items == 0 || anchor_index >= total_items {
        return None;
    }
    let (before, after) = store_backed_window_extents(total_items, anchor_index);
    let start = anchor_index - before;
    let end = (anchor_index + after + 1).min(total_items);
    let window = StoreBackedSourceWindow {
        start_rank: start,
        total_source_items: total_items,
        items: (start..end)
            .map(|index| {
                track_at(index).map(|track| StoreBackedSourceItem {
                    track,
                    source_index: index,
                    source_item_id: None,
                })
            })
            .collect::<Option<Vec<_>>>()?,
    };
    let activation = store_backed_window_play_activation(source_key, window, anchor_index).ok()?;
    Some(activation)
}

fn normalize_store_backed_window_tracks(
    store: &StoreHandle,
    saved: &SavedSource,
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
    use domain::{AlbumId, Playlist, PlaylistEntrySortDescriptor};
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
    fn playlist_detail_activation_uses_loaded_entries() {
        let tracks = vec![
            library_track(1, None, AlbumId::fake(1), "Artist", &[]),
            library_track(2, None, AlbumId::fake(1), "Artist", &[]),
        ];
        let detail = PlaylistDetail {
            playlist: Playlist {
                id: PlaylistId::fake(1),
                name: "Playlist".to_string(),
                owner: None,
                track_count: tracks.len() as u32,
                duration_seconds: 360,
                top_genres: Vec::new(),
                image_refs: Vec::new(),
                image_ref: None,
            },
            tracks: tracks.clone(),
            entries: tracks
                .into_iter()
                .enumerate()
                .map(|(index, track)| PlaylistEntry {
                    entry_id: format!("entry-{index}"),
                    track,
                })
                .collect(),
        };

        let activation = playlist_detail_activation(
            PlaySourceKey {
                descriptor: PlaySourceDescriptor::Playlist {
                    playlist_id: PlaylistId::fake(1),
                },
                order: SourceOrder::PlaylistDisplayed {
                    query: None,
                    sort: PlaylistEntrySortDescriptor::Position,
                    descending: false,
                },
            },
            detail,
        )
        .expect("playlist activation");

        let (shuffle_start, target) = activation.into_parts();
        let PlayTarget::LoadedSource {
            completeness,
            items,
            anchor,
            ..
        } = target
        else {
            panic!("expected loaded playlist source");
        };
        assert!(shuffle_start);
        assert_eq!(completeness, LoadedCompleteness::Complete);
        assert_eq!(anchor.source_index, 0);
        assert_eq!(anchor.source_item_id.as_deref(), Some("entry-0"));
        assert_eq!(items[0].source_item_id.as_deref(), Some("entry-0"));
        assert_eq!(items[1].source_item_id.as_deref(), Some("entry-1"));
    }

    #[test]
    fn queue_replace_occurrence() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = saved_source();
        let album_id = AlbumId::new("queue:album:occurrences");
        let mut repeated_track = library_track(1, None, album_id.clone(), "Artist", &[]);
        repeated_track.id = TrackId::new("queue:track:repeated");
        repeated_track.title = "Repeated".to_string();
        let mut other_track = library_track(2, None, album_id, "Artist", &[]);
        other_track.id = TrackId::new("queue:track:other");
        let playlist = Playlist {
            id: PlaylistId::new("queue:playlist:occurrences"),
            name: "Playlist".to_string(),
            owner: None,
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
                store.save_source(&saved)?;
                store.set_active_source(&saved.source.id)?;
                let generation = store.begin_sync(&saved.source.id)?;
                commit_cached_library(
                    store,
                    &saved.source.id,
                    generation,
                    CachedLibraryObservation {
                        tracks: vec![repeated_track.clone(), other_track.clone()],
                        playlists: vec![PlaylistDetail {
                            playlist: playlist.clone(),
                            tracks: entries.iter().map(|entry| entry.track.clone()).collect(),
                            entries: entries.clone(),
                        }],
                        ..CachedLibraryObservation::default()
                    },
                )?;
                Ok(())
            })
            .expect("seed store");
        controller
            .secrets
            .save_token(&saved.source.id, "queue-token")
            .expect("save source token");
        install_active_source_for_test(&controller, &saved);
        *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.source.id.clone()));

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
                other_track.id.clone(),
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
                    track_id: other_track.id,
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
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = saved_source();
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source.id)
            })
            .expect("set active source");
        controller
            .secrets
            .save_token(&saved.source.id, "queue-token")
            .expect("save source token");
        install_active_source_for_test(&controller, &saved);
        *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.source.id.clone()));

        let album_id = AlbumId::new("queue:album:reuse");
        let mut first = library_track(1, None, album_id.clone(), "Artist", &[]);
        first.id = TrackId::new("queue:track:reuse:first");
        let mut second = library_track(2, None, album_id.clone(), "Artist", &[]);
        second.id = TrackId::new("queue:track:reuse:second");
        let mut third = library_track(3, None, album_id, "Artist", &[]);
        third.id = TrackId::new("queue:track:reuse:third");
        let tracks = [first, second, third];
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::new("queue:playlist:reuse"),
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
    fn first_row_source_activation_plays_clicked_track_with_shuffle_enabled() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = saved_source();
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source.id)
            })
            .expect("set active source");
        controller
            .secrets
            .save_token(&saved.source.id, "queue-token")
            .expect("save source token");
        install_active_source_for_test(&controller, &saved);
        *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.source.id.clone()));

        let album_id = AlbumId::new("queue:album:clicked");
        let mut clicked = library_track(1, None, album_id.clone(), "Artist", &[]);
        clicked.id = TrackId::new("queue:track:clicked");
        let mut other = library_track(2, None, album_id, "Artist", &[]);
        other.id = TrackId::new("queue:track:other");
        let tracks = [clicked, other];
        controller
            .with_queue_mut(|queue| {
                queue.play_now(&tracks[0]);
                queue.set_shuffle(true, 19);
                Ok(())
            })
            .expect("set shuffled current");
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::new("queue:playlist:clicked"),
            },
            order: SourceOrder::PlaylistDisplayed {
                query: None,
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        };

        let played = controller.play_loaded_source_window(source_key, tracks.len(), 0, |index| {
            tracks.get(index).cloned()
        });

        assert!(played);
        let queue = wait_for_queue(&events).expect("source queue");
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(
            queue
                .current_index
                .and_then(|index| queue.entries.get(index))
                .map(|entry| &entry.track_id),
            Some(&tracks[0].id)
        );
    }

    #[test]
    fn source_shuffle_start_uses_shuffled_current() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = saved_source();
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source.id)
            })
            .expect("set active source");
        controller
            .secrets
            .save_token(&saved.source.id, "queue-token")
            .expect("save source token");
        install_active_source_for_test(&controller, &saved);
        *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.source.id.clone()));

        let album_id = AlbumId::new("queue:album:shuffle");
        let mut previous = library_track(1, None, album_id.clone(), "Artist", &[]);
        previous.id = TrackId::new("queue:track:shuffle:previous");
        let mut next = library_track(2, None, album_id, "Artist", &[]);
        next.id = TrackId::new("queue:track:shuffle:next");
        let tracks = [previous, next];
        controller
            .with_queue_mut(|queue| {
                queue.play_now(&tracks[0]);
                queue.set_shuffle(true, 19);
                Ok(())
            })
            .expect("set shuffle");
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::new("queue:playlist:shuffle"),
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
        assert!(queue.shuffle.enabled);
        assert_eq!(queue.shuffle_order.len(), tracks.len());
        assert_eq!(queue.current_index, queue.shuffle_order.first().copied());
        assert_ne!(queue.shuffle.seed, 19);
    }

    #[test]
    fn queue_reorder_preserves_current_entry() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let album_id = AlbumId::new("queue:album:reorder");
        let mut current = library_track(1, None, album_id.clone(), "Artist", &[]);
        current.id = TrackId::new("queue:track:reorder:current");
        let mut middle = library_track(2, None, album_id.clone(), "Artist", &[]);
        middle.id = TrackId::new("queue:track:reorder:middle");
        let mut last = library_track(3, None, album_id, "Artist", &[]);
        last.id = TrackId::new("queue:track:reorder:last");
        let mut queue = QueueEngine::new(SourceId::new("queue:source:reorder"));
        queue.append(&current);
        queue.append(&middle);
        queue.append(&last);
        let initial = queue.snapshot();
        let last_entry_id = initial.entries[2].id.clone();
        *controller.queue.lock().expect("queue") = Some(queue);

        controller.reorder_queue_entry(last_entry_id, 0, false);

        let reordered = wait_for_queue(&events).expect("reordered queue");
        assert_eq!(reordered.entries[0].track_id, last.id);
        assert_eq!(
            reordered.entries[reordered.current_index.expect("current")].track_id,
            current.id
        );
    }

    #[test]
    fn queue_windowed_source_activation_replaces_entries() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let saved = saved_source();
        controller
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source.id)
            })
            .expect("set active source");
        controller
            .secrets
            .save_token(&saved.source.id, "queue-token")
            .expect("save source token");
        install_active_source_for_test(&controller, &saved);
        *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.source.id.clone()));

        let album_id = AlbumId::new("queue:album:window");
        let mut first = library_track(1, None, album_id.clone(), "Artist", &[]);
        first.id = TrackId::new("queue:track:window:first");
        let mut second = library_track(2, None, album_id.clone(), "Artist", &[]);
        second.id = TrackId::new("queue:track:window:second");
        let mut third = library_track(3, None, album_id.clone(), "Artist", &[]);
        third.id = TrackId::new("queue:track:window:third");
        let mut fourth = library_track(4, None, album_id.clone(), "Artist", &[]);
        fourth.id = TrackId::new("queue:track:window:fourth");
        let mut fifth = library_track(5, None, album_id, "Artist", &[]);
        fifth.id = TrackId::new("queue:track:window:fifth");
        let tracks = [first, second, third, fourth, fifth];
        let source_key = PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::new("queue:playlist:window"),
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
                items: tracks[start..start + 3]
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

        controller.play_activation(activation(0, 2));
        let same_window_queue = wait_for_queue(&events).expect("same window source queue");
        assert_eq!(same_window_queue.current_index, Some(2));
        assert_eq!(
            same_window_queue
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            initial_ids
        );

        controller.play_activation(activation(2, 2));
        let moved_queue = wait_for_queue(&events).expect("windowed source queue");

        assert_eq!(moved_queue.current_index, Some(2));
        assert_eq!(moved_queue.entries[0].track_id, tracks[2].id);
        assert_eq!(moved_queue.entries[2].track_id, tracks[4].id);
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
        let source_id = SourceId::new("queue:source:clear");
        let album_id = AlbumId::new("queue:album:clear");
        let mut current = library_track(1, None, album_id.clone(), "Artist", &[]);
        current.id = TrackId::new("queue:track:clear:current");
        let mut stale = library_track(2, None, album_id, "Artist", &[]);
        stale.id = TrackId::new("queue:track:clear:stale");
        let mut queue = QueueEngine::new(source_id.clone());
        queue.append(&current);
        *controller.queue.lock().expect("queue") = Some(queue);

        let generation = controller.next_play_activation_generation();
        let stale_activation = PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: PlaySourceKey {
                    descriptor: PlaySourceDescriptor::Album {
                        album_id: AlbumId::new("queue:album:stale"),
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

        controller.finish_store_backed_source_activation(&source_id, stale_activation, generation);
        let queue = controller.queue_snapshot().expect("queue snapshot");
        assert_eq!(queue.entries.len(), 1);
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(queue.entries[0].track_id, current.id);
    }

    #[test]
    fn queue_clear_keeps_current_playback() {
        let (mut controller, events, ..) = AppController::bootstrap_memory_for_test();
        let commands = Arc::new(Mutex::new(Vec::new()));
        controller.playback = Arc::new(Mutex::new(Box::new(RecordingPlaybackBackend {
            commands: Arc::clone(&commands),
        })));
        let album_id = AlbumId::new("queue:album:clear-playback");
        let mut current = library_track(1, None, album_id.clone(), "Artist", &[]);
        current.id = TrackId::new("queue:track:clear-playback:current");
        let mut next = library_track(2, None, album_id, "Artist", &[]);
        next.id = TrackId::new("queue:track:clear-playback:next");
        let mut queue = QueueEngine::new(SourceId::new("queue:source:clear-playback"));
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
    fn stale_store_activation_cannot_overwrite_newer_queue_change() {
        let (controller, events, ..) = AppController::bootstrap_memory_for_test();
        let source_id = SourceId::new("queue:source:stale-generation");
        let album_id = AlbumId::new("queue:album:stale-generation");
        let mut current = library_track(1, None, album_id.clone(), "Artist", &[]);
        current.id = TrackId::new("queue:track:stale-generation:current");
        let mut stale = library_track(2, None, album_id.clone(), "Artist", &[]);
        stale.id = TrackId::new("queue:track:stale-generation:stale");
        let mut newer = library_track(3, None, album_id, "Artist", &[]);
        newer.id = TrackId::new("queue:track:stale-generation:newer");
        let mut queue = QueueEngine::new(source_id.clone());
        queue.append(&current);
        *controller.queue.lock().expect("queue") = Some(queue);

        let generation = controller.next_play_activation_generation();
        let stale_activation = PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: PlaySourceKey {
                    descriptor: PlaySourceDescriptor::Album {
                        album_id: AlbumId::new("queue:album:stale"),
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

        controller.play_last(vec![newer.clone()]);
        let newer_queue = wait_for_queue(&events).expect("newer queue change");
        assert_eq!(newer_queue.entries.len(), 2);

        controller.finish_store_backed_source_activation(&source_id, stale_activation, generation);
        let queue = controller.queue_snapshot().expect("queue snapshot");
        assert_eq!(queue.entries.len(), 2);
        assert_eq!(queue.entries[0].track_id, current.id);
        assert_eq!(queue.entries[1].track_id, newer.id);
        assert!(queue.entries.iter().all(|entry| entry.track_id != stale.id));
    }
}
