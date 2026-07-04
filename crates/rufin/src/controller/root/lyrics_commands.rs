use super::*;

fn lyrics_event(track_id: &TrackId, lyrics: Option<Lyrics>) -> ControllerEvent {
    ControllerEvent::Lyrics {
        track_id: track_id.clone(),
        lyrics: Box::new(lyrics),
    }
}

fn default_external_fallback_for_search(search: JellyfinLyricsSearch) -> bool {
    matches!(
        search,
        JellyfinLyricsSearch::ServerThenRemote | JellyfinLyricsSearch::RemoteThenServer
    )
}

fn cached_lyrics_search_mode(
    search: JellyfinLyricsSearch,
    allow_external_fallback: bool,
) -> JellyfinLyricsSearch {
    if allow_external_fallback && matches!(search, JellyfinLyricsSearch::ServerOnly) {
        JellyfinLyricsSearch::ServerThenRemote
    } else {
        search
    }
}

impl AppController {
    #[cfg(test)]
    pub fn request_track_lyrics(&self, track_id: TrackId) {
        self.lyrics_cache_for_track(track_id, true);
    }
    pub fn request_track_auto_lyrics(&self, track_id: TrackId) {
        self.lyrics_search_for_track_with_external(
            track_id,
            true,
            JellyfinLyricsSearch::ServerOnly,
            true,
        );
    }
    pub fn request_track_server_lyrics(&self, track_id: TrackId) {
        self.lyrics_search_for_track(track_id, true, JellyfinLyricsSearch::ServerOnly);
    }
    pub fn refresh_lyrics_for_current(&self) {
        self.lyrics_cache(false);
    }
    pub fn clear_remote_lyrics_for_current(&self) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            return;
        };
        match self
            .store
            .with_store(|store| store.delete_remote_lyrics(&source_id, &entry.track_id))
        {
            Ok(true) => {
                let _sent = self.events.send(lyrics_event(&entry.track_id, None));
            }
            Ok(false) => {}
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
            }
        }
    }
    pub(in crate::controller) fn lyrics_cache(&self, use_cache: bool) {
        let settings = load_settings_from_store(&self.store);
        self.lyrics_search(use_cache, lyrics_search_for_settings(&settings));
    }
    #[cfg(test)]
    fn lyrics_cache_for_track(&self, track_id: TrackId, use_cache: bool) {
        let settings = load_settings_from_store(&self.store);
        self.lyrics_search_for_track(track_id, use_cache, lyrics_search_for_settings(&settings));
    }
    pub(in crate::controller) fn lyrics_search(
        &self,
        use_cache: bool,
        search: JellyfinLyricsSearch,
    ) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            debug!("lyrics request skipped because the queue has no current track");
            return;
        };
        self.lyrics_search_entry(
            use_cache,
            search,
            default_external_fallback_for_search(search),
            source_id,
            entry,
        );
    }
    fn lyrics_search_for_track(
        &self,
        track_id: TrackId,
        use_cache: bool,
        search: JellyfinLyricsSearch,
    ) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            debug!("lyrics request skipped because the queue has no current track");
            return;
        };
        if entry.track_id != track_id {
            debug!(track_id = %track_id, "skipped stale lyrics request");
            return;
        }
        self.lyrics_search_entry(
            use_cache,
            search,
            default_external_fallback_for_search(search),
            source_id,
            entry,
        );
    }
    fn lyrics_search_for_track_with_external(
        &self,
        track_id: TrackId,
        use_cache: bool,
        search: JellyfinLyricsSearch,
        allow_external_fallback: bool,
    ) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            debug!("lyrics request skipped because the queue has no current track");
            return;
        };
        if entry.track_id != track_id {
            debug!(track_id = %track_id, "skipped stale lyrics request");
            return;
        }
        self.lyrics_search_entry(use_cache, search, allow_external_fallback, source_id, entry);
    }
    fn lyrics_search_entry(
        &self,
        use_cache: bool,
        search: JellyfinLyricsSearch,
        allow_external_fallback: bool,
        source_id: SourceId,
        entry: QueueEntry,
    ) {
        let settings = load_settings_from_store(&self.store);
        let external_providers = settings.external_lyrics_providers.clone();
        if let Some(lyrics) = local_sidecar_lyrics(&self.store, &source_id, &entry.track_id) {
            debug!(track_id = %entry.track_id, "loaded lyrics from local sidecar");
            let _saved = self
                .store
                .with_store(|store| store.save_lyrics(&source_id, &lyrics));
            let _sent = self
                .events
                .send(lyrics_event(&entry.track_id, Some(lyrics)));
            return;
        }
        let cue_track = track_has_cue_source(&self.store, &source_id, &entry.track_id);
        let cached = use_cache.then(|| {
            self.store
                .with_store(|store| store.load_lyrics(&source_id, &entry.track_id))
                .unwrap_or(None)
        });
        if let Some(cached) = cached.flatten()
            && cached_lyrics_allowed_for_track(
                &cached,
                cached_lyrics_search_mode(search, allow_external_fallback),
                &external_providers,
                cue_track,
            )
        {
            let delete_remote = cached.source == source::LyricsSource::Remote;
            if let Some(cached) = lyrics_with_displayable_content(cached) {
                debug!(track_id = %entry.track_id, "loaded lyrics from cache");
                let _sent = self
                    .events
                    .send(lyrics_event(&entry.track_id, Some(cached)));
                return;
            }
            if delete_remote {
                let _deleted = self
                    .store
                    .with_store(|store| store.delete_remote_lyrics(&source_id, &entry.track_id));
            }
        }
        let provider_is_local = self
            .store
            .with_store(|store| store.saved_source(&source_id))
            .unwrap_or(None)
            .is_some_and(|saved| saved.source.kind == LOCAL_SOURCE_ID);
        if provider_is_local {
            debug!(
                track_id = %entry.track_id,
                allow_external_fallback, "local provider has no server lyrics"
            );
            let events = self.events.clone();
            let store = self.store.clone();
            let local_external_providers = external_providers.clone();
            thread::spawn(move || {
                match allow_external_fallback.then(|| {
                    external_best_lyrics(&store, &source_id, &entry, &local_external_providers)
                }) {
                    Some(Ok(Some(lyrics))) => {
                        debug!(track_id = %entry.track_id, provider = ?lyrics.external_provider, "loaded lyrics from external provider");
                        let _saved =
                            store.with_store(|store| store.save_lyrics(&source_id, &lyrics));
                        let _sent = events.send(lyrics_event(&entry.track_id, Some(lyrics)));
                    }
                    Some(Err(error)) => {
                        debug!(track_id = %entry.track_id, %error, "external lyric fallback failed");
                        let _sent = events.send(lyrics_event(&entry.track_id, None));
                    }
                    Some(Ok(None)) | None => {
                        let _sent = events.send(lyrics_event(&entry.track_id, None));
                    }
                }
            });
            return;
        }
        debug!(
            track_id = %entry.track_id,
            allow_external_fallback,
            ?search,
            "requesting lyrics from provider"
        );
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.saved_source(&source_id))
                .unwrap_or(None)
            else {
                let _sent = events.send(lyrics_event(&entry.track_id, None));
                return;
            };
            if saved.source.kind == "fake" {
                let _sent = events.send(lyrics_event(&entry.track_id, None));
                return;
            }
            let result =
                source_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                    runtime
                        .block_on(provider.lyrics_with_search(&entry.track_id, search))
                        .map_err(|error| error.to_string())
                });
            match result {
                Ok(Some(lyrics)) => {
                    debug!(track_id = %entry.track_id, source = ?lyrics.source, "loaded lyrics from provider");
                    let _saved = store.with_store(|store| store.save_lyrics(&source_id, &lyrics));
                    let _sent = events.send(lyrics_event(&entry.track_id, Some(lyrics)));
                }
                Ok(None) => {
                    debug!(
                        track_id = %entry.track_id,
                        allow_external_fallback, "provider returned no lyrics"
                    );
                    match allow_external_fallback.then(|| {
                        external_best_lyrics(&store, &source_id, &entry, &external_providers)
                    }) {
                        Some(Ok(Some(lyrics))) => {
                            debug!(track_id = %entry.track_id, provider = ?lyrics.external_provider, "loaded lyrics from external provider");
                            let _saved =
                                store.with_store(|store| store.save_lyrics(&source_id, &lyrics));
                            let _sent = events.send(lyrics_event(&entry.track_id, Some(lyrics)));
                        }
                        Some(Err(error)) => {
                            debug!(track_id = %entry.track_id, %error, "external lyric fallback failed");
                            let _sent = events.send(lyrics_event(&entry.track_id, None));
                        }
                        Some(Ok(None)) | None => {
                            let _sent = events.send(lyrics_event(&entry.track_id, None));
                        }
                    }
                }
                Err(error) => {
                    match allow_external_fallback.then(|| {
                        external_best_lyrics(&store, &source_id, &entry, &external_providers)
                    }) {
                        Some(Ok(Some(lyrics))) => {
                            debug!(track_id = %entry.track_id, provider = ?lyrics.external_provider, "loaded lyrics from external provider after provider error");
                            let _saved =
                                store.with_store(|store| store.save_lyrics(&source_id, &lyrics));
                            let _sent = events.send(lyrics_event(&entry.track_id, Some(lyrics)));
                        }
                        Some(Err(fallback_error)) => {
                            debug!(track_id = %entry.track_id, %fallback_error, "external lyric fallback failed");
                            let _sent = events.send(ControllerEvent::Error(error));
                            let _sent = events.send(lyrics_event(&entry.track_id, None));
                        }
                        Some(Ok(None)) | None => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            let _sent = events.send(lyrics_event(&entry.track_id, None));
                        }
                    }
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
        let Some((_source_id, entry, _position)) = self.current_playback_entry() else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        let track_id = entry.track_id.clone();
        let settings = load_settings_from_store(&self.store);
        if !crate::external_activity::external_lyrics_lookup(&settings) {
            let _sent = self.events.send(ControllerEvent::LyricsSearchResults {
                track_id,
                artist_name,
                track_name,
                results: Vec::new(),
            });
            return;
        }
        let external_providers = settings.external_lyrics_providers.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            match external_lyrics_search(&external_providers, &artist_name, &track_name) {
                Ok(results) => {
                    debug!(
                        track_id = %track_id,
                        artist_name = %artist_name,
                        track_name = %track_name,
                        results = results.len(),
                        "completed manual external lyric search"
                    );
                    let _sent = events.send(ControllerEvent::LyricsSearchResults {
                        track_id,
                        artist_name,
                        track_name,
                        results,
                    });
                }
                Err(error) => {
                    debug!(
                        track_id = %track_id,
                        artist_name = %artist_name,
                        track_name = %track_name,
                        %error,
                        "manual external lyric search failed"
                    );
                    let _sent = events.send(ControllerEvent::LyricsSearchFailed {
                        track_id,
                        artist_name,
                        track_name,
                        error,
                    });
                }
            }
        });
    }
    pub fn save_lyrics_search_result(
        &self,
        track_id: TrackId,
        result: LyricsSearchResult,
        output_path: PathBuf,
    ) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
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
        thread::spawn(
            move || match save_lrclib_result(&source_id, &entry, &result, output_path) {
                Ok(Some((path, lyrics))) => {
                    let _saved = store.with_store(|store| store.save_lyrics(&source_id, &lyrics));
                    let _sent = events.send(ControllerEvent::LyricsSaved { path, lyrics });
                }
                Ok(None) => {
                    let _sent = events.send(lyrics_event(&entry.track_id, None));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            },
        );
    }
    pub fn preview_lyrics_search_result(&self, track_id: TrackId, result: LyricsSearchResult) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        if entry.track_id != track_id {
            let _sent = self.events.send(ControllerEvent::Error(
                "The playing track changed before lyrics were loaded.".to_string(),
            ));
            return;
        }
        let store = self.store.clone();
        let events = self.events.clone();
        let requested_track_id = entry.track_id.clone();
        thread::spawn(
            move || match lyrics_from_search_result(entry.track_id, &result) {
                Ok(Some(lyrics)) => {
                    let _saved = store.with_store(|store| store.save_lyrics(&source_id, &lyrics));
                    let _sent = events.send(lyrics_event(&requested_track_id, Some(lyrics)));
                }
                Ok(None) => {
                    let _sent = events.send(lyrics_event(&requested_track_id, None));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            },
        );
    }
}
