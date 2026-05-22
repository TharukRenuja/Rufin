impl AppController {
    pub fn save_server_local_access(
        &self,
        server_id: ServerId,
        root_path: PathBuf,
        path_replace_from: Option<String>,
        path_replace_to: Option<String>,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(root_path) = root_path.to_str().map(ToString::to_string) else {
                let _sent = events.send(ControllerEvent::Error(
                    "Could not use the selected local folder path.".to_string(),
                ));
                return;
            };
            let path_replace_to =
                trimmed_optional(path_replace_to.as_deref()).unwrap_or_else(|| root_path.clone());
            let matched_server_id = server_id.clone();
            let result = store.with_store(|store| {
                store.save_server_local_access(&ServerLocalAccess {
                    server_id,
                    root_path: root_path.clone(),
                    path_replace_from: trimmed_optional(path_replace_from.as_deref()),
                    path_replace_to: Some(path_replace_to),
                })
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) =
                runtime.block_on(refresh_local_track_matches(&store, &matched_server_id))
            {
                warn!(%error, "failed to refresh local track matches");
            }
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                events.clone(),
            );
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn update_server_settings(
        &self,
        server_id: ServerId,
        name: String,
        base_url: String,
        trust_invalid_cert: bool,
    ) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let result = store.with_store(|store| {
                let Some(mut saved) = store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id)
                else {
                    return Ok(false);
                };
                if saved.server.provider != LOCAL_PROVIDER_ID && base_url.trim().is_empty() {
                    return Ok(false);
                }
                let next_name = name.trim().to_string();
                let next_base_url = if saved.server.provider == LOCAL_PROVIDER_ID {
                    saved.server.base_url.clone()
                } else {
                    base_url.trim().to_string()
                };
                let changed = saved.server.name != next_name
                    || saved.server.base_url != next_base_url
                    || saved.trust_invalid_cert != trust_invalid_cert;
                if !changed {
                    return Ok(false);
                }
                saved.server.name = next_name;
                saved.server.base_url = next_base_url;
                saved.trust_invalid_cert = trust_invalid_cert;
                store.save_server(&saved)?;
                Ok(true)
            });
            match result {
                Ok(true) => {
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        "Server settings saved.".to_string(),
                    ));
                    emit_snapshot(&store, &events);
                }
                Ok(false) => {}
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn clear_server_local_access(&self, server_id: ServerId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.delete_server_local_access(&server_id)?;
                store.delete_track_local_matches(&server_id)
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                events.clone(),
            );
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn set_selected_music_folder(&self, server_id: ServerId, folder_id: Option<MusicFolderId>) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.set_selected_music_folder_id(&server_id, folder_id.as_ref())
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn load_folder_for_active(&self, request_id: u64, path: Vec<FolderPathItem>) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let result = load_folder_detail(&store, &runtime, &secrets, &path);
            match result {
                Ok(detail) => {
                    let _sent = events.send(ControllerEvent::FolderLoaded {
                        request_id,
                        path,
                        detail,
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::FolderLoadFailed {
                        request_id,
                        path,
                        error,
                    });
                }
            }
        });
    }
    pub fn search(&self, query: String) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let settings = load_settings_for_active_server(&store);
            let mut snapshot = match load_snapshot(&store) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if let Some(server) = &snapshot.server {
                match store.with_store(|store| store.search_library(&server.id, &query, 50)) {
                    Ok(mut results) => {
                        external_metadata::normalize_search_results(&mut results, &settings);
                        snapshot.search = results;
                    }
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            }
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        });
    }
    pub fn set_album_favorite(&self, album_id: AlbumId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Album(album_id), favorite);
    }
    pub fn set_track_favorite(&self, track_id: TrackId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Track(track_id), favorite);
    }
    pub fn set_artist_favorite(&self, artist_id: ArtistId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Artist(artist_id), favorite);
    }
    pub fn toggle_current_favorite(&self) {
        let Some(entry) = self
            .playback_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.current.clone())
        else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        self.set_favorite(FavoriteItemId::Track(entry.track_id), !entry.favorite);
    }
    fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music server is saved.".to_string(),
                ));
                return;
            };

            if saved.server.provider != "fake" && saved.server.provider != "local" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(
                                provider
                                    .as_music_provider()
                                    .set_favorite(item_id.clone(), favorite),
                            )
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }

            let result = store.with_store(|store| {
                match &item_id {
                    FavoriteItemId::Album(album_id) => {
                        store.set_album_favorite(&saved.server.id, album_id, favorite)?;
                    }
                    FavoriteItemId::Track(track_id) => {
                        store.set_track_favorite(&saved.server.id, track_id, favorite)?;
                    }
                    FavoriteItemId::Artist(artist_id) => {
                        store.set_artist_favorite(&saved.server.id, artist_id, favorite)?;
                    }
                }
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }

            if let FavoriteItemId::Track(track_id) = &item_id {
                if let Ok(mut queue) = queue.lock()
                    && let Some(queue) = queue.as_mut()
                {
                    queue.set_track_favorite(track_id, favorite);
                    let snapshot = queue.snapshot();
                    let _saved = store.with_store(|store| store.save_queue_snapshot(&snapshot));
                    let _sent = events.send(ControllerEvent::Queue(Box::new(Some(snapshot))));
                }
                if let Ok(mut snapshot) = playback_snapshot.lock()
                    && let Some(current) = snapshot.current.as_mut()
                    && current.track_id == *track_id
                {
                    current.favorite = favorite;
                    let _sent = events.send(ControllerEvent::Playback(Box::new(snapshot.clone())));
                }
            }

            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::FavoriteChanged {
                        item_id,
                        favorite,
                        snapshot: Box::new(snapshot),
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn create_playlist(&self, name: String, tracks: Vec<Track>) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music server is saved.".to_string(),
                ));
                return;
            };
            let track_ids = tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>();
            let playlist_id = if saved.server.provider == "fake" {
                PlaylistId::new(format!(
                    "fake:playlist:{}",
                    unique_millis().unwrap_or(tracks.len() as u128)
                ))
            } else {
                match provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                    runtime
                        .block_on(
                            provider
                                .as_music_provider()
                                .create_playlist(&name, &track_ids),
                        )
                        .map_err(|error| error.to_string())
                }) {
                    Ok(playlist_id) => playlist_id,
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            };
            let playlist = Playlist {
                id: playlist_id.clone(),
                name: name.trim().to_string(),
                track_count: tracks.len() as u32,
                duration_seconds: tracks.iter().map(|track| track.duration_seconds).sum(),
                image_ref: tracks.iter().find_map(|track| track.image_ref.clone()),
            };
            let entries = playlist_entries_for_tracks(&playlist_id, &tracks);
            let result = store.with_store(|store| {
                store.upsert_playlists(&saved.server.id, &[playlist], 0)?;
                store.upsert_tracks(&saved.server.id, &tracks, 0)?;
                store.upsert_playlist_entries(&saved.server.id, &playlist_id, &entries, 0)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn rename_playlist(&self, playlist_id: PlaylistId, name: String) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music server is saved.".to_string(),
                ));
                return;
            };
            if saved.server.provider != "fake" && saved.server.provider != "local" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(
                                provider
                                    .as_music_provider()
                                    .rename_playlist(&playlist_id, &name),
                            )
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.rename_playlist(&saved.server.id, &playlist_id, name.trim())?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn add_tracks_to_playlist(&self, playlist_id: PlaylistId, tracks: Vec<Track>) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            let mut entries = detail.entries;
            entries.extend(playlist_entries_for_tracks(&detail.playlist.id, &tracks));
            detail.tracks.extend(tracks);
            detail.entries = entries;
            detail
        });
    }
    pub fn remove_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            detail.entries.retain(|entry| entry.entry_id != entry_id);
            detail.tracks = detail
                .entries
                .iter()
                .map(|entry| entry.track.clone())
                .collect();
            detail
        });
    }
    pub fn move_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String, new_index: usize) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            if let Some(old_index) = detail
                .entries
                .iter()
                .position(|entry| entry.entry_id == entry_id)
            {
                let entry = detail.entries.remove(old_index);
                detail
                    .entries
                    .insert(new_index.min(detail.entries.len()), entry);
                detail.tracks = detail
                    .entries
                    .iter()
                    .map(|entry| entry.track.clone())
                    .collect();
            }
            detail
        });
    }
    fn mutate_playlist_entries(
        &self,
        playlist_id: PlaylistId,
        mutate: impl FnOnce(rufin_provider::PlaylistDetail) -> rufin_provider::PlaylistDetail
        + Send
        + 'static,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music server is saved.".to_string(),
                ));
                return;
            };
            let before = match store
                .with_store(|store| store.load_playlist_detail(&saved.server.id, &playlist_id))
            {
                Ok(Some(detail)) => detail,
                Ok(None) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected cached playlist was not found.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let mut after = mutate(before.clone());
            if saved.server.provider != "fake" {
                match sync_playlist_mutation(&store, &runtime, &secrets, &saved, &before, &after) {
                    Ok(Some(fresh)) => after = fresh,
                    Ok(None) => {}
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            }
            let playlist = Playlist {
                track_count: after.entries.len() as u32,
                duration_seconds: after
                    .entries
                    .iter()
                    .map(|entry| entry.track.duration_seconds)
                    .sum(),
                image_ref: after
                    .entries
                    .iter()
                    .find_map(|entry| entry.track.image_ref.clone())
                    .or(after.playlist.image_ref.clone()),
                ..after.playlist.clone()
            };
            let result = store.with_store(|store| {
                store.upsert_playlists(&saved.server.id, &[playlist], 0)?;
                store.upsert_tracks(&saved.server.id, &after.tracks, 0)?;
                store.upsert_playlist_entries(
                    &saved.server.id,
                    &after.playlist.id,
                    &after.entries,
                    0,
                )?;
                Ok(())
            });
            emit_playlist_changed_result(&store, &events, after.playlist.id.clone(), result);
        });
    }
    pub fn request_lyrics_for_current(&self) {
        self.request_lyrics_for_current_with_cache(true);
    }
    pub fn request_server_lyrics_for_current(&self) {
        self.request_lyrics_for_current_with_search(true, JellyfinLyricsSearch::ServerOnly);
    }
    pub fn refresh_lyrics_for_current(&self) {
        self.request_lyrics_for_current_with_cache(false);
    }
    fn request_lyrics_for_current_with_cache(&self, use_cache: bool) {
        let settings = load_settings_from_store(&self.store);
        self.request_lyrics_for_current_with_search(
            use_cache,
            lyrics_search_for_settings(&settings),
        );
    }
    fn request_lyrics_for_current_with_search(
        &self,
        use_cache: bool,
        search: JellyfinLyricsSearch,
    ) {
        let Some((server_id, entry, _position)) = self.current_queue_entry() else {
            debug!("lyrics request skipped because the queue has no current track");
            let _sent = self.events.send(ControllerEvent::Lyrics(Box::new(None)));
            return;
        };
        if let Some(lyrics) = local_sidecar_lyrics(&self.store, &server_id, &entry.track_id) {
            debug!(track_id = %entry.track_id, "loaded lyrics from local sidecar");
            let _saved = self
                .store
                .with_store(|store| store.save_lyrics(&server_id, &lyrics));
            let _sent = self
                .events
                .send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
            return;
        }
        let cached = use_cache.then(|| {
            self.store
                .with_store(|store| store.load_lyrics(&server_id, &entry.track_id))
                .unwrap_or(None)
        });
        if let Some(cached) = cached
            .flatten()
            .filter(|lyrics| cached_lyrics_allowed(lyrics, search))
        {
            debug!(track_id = %entry.track_id, "loaded lyrics from cache");
            let _sent = self
                .events
                .send(ControllerEvent::Lyrics(Box::new(Some(cached))));
            return;
        }
        let allow_remote = matches!(
            search,
            JellyfinLyricsSearch::ServerThenRemote | JellyfinLyricsSearch::RemoteThenServer
        );
        debug!(track_id = %entry.track_id, allow_remote, ?search, "requesting lyrics from provider");
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
                .filter(|saved| saved.server.id == server_id)
            else {
                let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                return;
            };
            if saved.server.provider == "fake" {
                let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                return;
            }
            let result =
                provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                    runtime
                        .block_on(provider.lyrics_with_search(&entry.track_id, search))
                        .map_err(|error| error.to_string())
                });
            match result {
                Ok(Some(lyrics)) => {
                    debug!(track_id = %entry.track_id, source = ?lyrics.source, "loaded lyrics from provider");
                    let _saved = store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                    let _sent = events.send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
                }
                Ok(None) => {
                    debug!(track_id = %entry.track_id, allow_remote, "provider returned no lyrics");
                    let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                }
            }
        });
    }
    pub fn search_lyrics_for_current(&self, artist_name: String, track_name: String) {
        let artist_name = artist_name.trim().to_string();
        let track_name = track_name.trim().to_string();
        if artist_name.is_empty() && track_name.is_empty() {
            return;
        }
        let Some((_server_id, entry, _position)) = self.current_queue_entry() else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        let track_id = entry.track_id.clone();
        let events = self.events.clone();
        thread::spawn(move || match lrclib_search(&artist_name, &track_name) {
            Ok(results) => {
                let _sent = events.send(ControllerEvent::LyricsSearchResults {
                    track_id,
                    artist_name,
                    track_name,
                    results,
                });
            }
            Err(error) => {
                let _sent = events.send(ControllerEvent::Error(error));
                let _sent = events.send(ControllerEvent::LyricsSearchResults {
                    track_id,
                    artist_name,
                    track_name,
                    results: Vec::new(),
                });
            }
        });
    }
    pub fn save_lyrics_search_result(
        &self,
        track_id: TrackId,
        result: LyricsSearchResult,
        output_path: Option<PathBuf>,
    ) {
        let Some((server_id, entry, _position)) = self.current_queue_entry() else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        if entry.track_id != track_id {
            let _sent = self.events.send(ControllerEvent::Error(
                "The playing track changed before lyrics were saved.".to_string(),
            ));
            return;
        }
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            match save_lrclib_result(&store, &server_id, &entry, &result, output_path) {
                Ok((path, lyrics)) => {
                    let _saved = store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                    let _sent = events.send(ControllerEvent::LyricsSaved { path, lyrics });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    fn with_queue_mut<T>(
        &self,
        operation: impl FnOnce(&mut QueueEngine) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "queue lock was poisoned".to_string())?;
        let Some(queue) = queue.as_mut() else {
            return Err("No active queue is available.".to_string());
        };
        operation(queue)
    }
    fn auto_dj_top_up_or_emit_error(&self) -> bool {
        match self.auto_dj_top_up() {
            Ok(topped_up) => topped_up,
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
                false
            }
        }
    }
    fn auto_dj_top_up(&self) -> Result<bool, String> {
        if !self
            .auto_dj_enabled
            .lock()
            .map(|enabled| *enabled)
            .unwrap_or_default()
        {
            return Ok(false);
        }
        let Some((server_id, current, queued_track_ids, remaining)) = self.auto_dj_queue_state()
        else {
            return Ok(false);
        };
        if remaining >= AUTO_DJ_THRESHOLD {
            return Ok(false);
        }
        let settings = load_settings_for_active_server(&self.store);
        let mut tracks = self
            .store
            .with_store(|store| store.load_tracks(&server_id, 0, AUTO_DJ_LIBRARY_LIMIT))
            .map(|page| page.items)?;
        external_metadata::normalize_tracks(&mut tracks, &settings);
        let candidates = auto_dj_candidates(&tracks, &current, &queued_track_ids, shuffle_seed());
        if candidates.is_empty() {
            return Ok(false);
        }
        self.with_queue_mut(|queue| {
            for track in &candidates {
                queue.append(track);
            }
            Ok(())
        })?;
        Ok(true)
    }
    fn auto_dj_queue_state(&self) -> Option<(ServerId, QueueEntry, HashSet<TrackId>, usize)> {
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let snapshot = queue.snapshot();
            let current = queue.current()?.clone();
            let queued = queue
                .entries()
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<HashSet<_>>();
            Some((
                snapshot.server_id,
                current,
                queued,
                queue.remaining_after_current(),
            ))
        })
    }
    fn persist_and_emit_queue(&self) {
        let queue_snapshot = self.queue_snapshot();
        if let Some(snapshot) = &queue_snapshot {
            self.persist_queue_snapshot(snapshot);
        }
        self.sync_playback_snapshot_from_queue();
        let _sent = self
            .events
            .send(ControllerEvent::Queue(Box::new(queue_snapshot)));
        self.emit_playback_snapshot();
        self.prepare_next_stream();
    }
    fn persist_current_queue_snapshot(&self) {
        if let Some(snapshot) = self.queue_snapshot() {
            self.persist_queue_snapshot(&snapshot);
        }
    }
    fn persist_queue_snapshot(&self, snapshot: &QueueSnapshot) {
        if let Err(error) = self
            .store
            .with_store(|store| store.save_queue_snapshot(snapshot))
        {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
    }
    fn queue_snapshot(&self) -> Option<QueueSnapshot> {
        self.queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot))
    }
    fn update_playback_snapshot(&self, operation: impl FnOnce(&mut PlaybackSnapshot)) {
        if let Ok(mut snapshot) = self.playback_snapshot.lock() {
            operation(&mut snapshot);
        }
    }
    fn sync_playback_snapshot_from_queue(&self) {
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
            if snapshot.current.is_none() {
                snapshot.state = PlaybackState::Stopped;
                snapshot.last_error = None;
                snapshot.buffering_percent = None;
            }
        });
    }
    fn emit_playback_snapshot(&self) {
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
