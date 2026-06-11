use super::*;

impl AppController {
    pub fn request_lyrics_for_current(&self) {
        self.lyrics_cache(true);
    }
    pub fn request_server_lyrics_for_current(&self) {
        self.lyrics_search(true, JellyfinLyricsSearch::ServerOnly);
    }
    pub fn refresh_lyrics_for_current(&self) {
        self.lyrics_cache(false);
    }
    pub fn clear_remote_lyrics_for_current(&self) {
        let Some((server_id, entry, _position)) = self.current_queue_entry() else {
            return;
        };
        match self
            .store
            .with_store(|store| store.delete_remote_lyrics(&server_id, &entry.track_id))
        {
            Ok(true) => {
                let _sent = self.events.send(ControllerEvent::Lyrics(Box::new(None)));
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
    pub(in crate::controller) fn lyrics_search(
        &self,
        use_cache: bool,
        search: JellyfinLyricsSearch,
    ) {
        let settings = load_settings_from_store(&self.store);
        let external_providers = settings.external_lyrics_providers.clone();
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
        let cue_track = track_has_cue_source(&self.store, &server_id, &entry.track_id);
        let cached = use_cache.then(|| {
            self.store
                .with_store(|store| store.load_lyrics(&server_id, &entry.track_id))
                .unwrap_or(None)
        });
        if let Some(cached) = cached.flatten().filter(|lyrics| {
            cached_lyrics_allowed_for_track(lyrics, search, &external_providers, cue_track)
        }) {
            debug!(track_id = %entry.track_id, "loaded lyrics from cache");
            let _sent = self
                .events
                .send(ControllerEvent::Lyrics(Box::new(Some(cached))));
            return;
        }
        let provider_is_local = self
            .store
            .with_store(|store| store.saved_server(&server_id))
            .unwrap_or(None)
            .is_some_and(|saved| saved.server.provider == LOCAL_PROVIDER_ID);
        let allow_remote = matches!(
            search,
            JellyfinLyricsSearch::ServerThenRemote | JellyfinLyricsSearch::RemoteThenServer
        );
        if provider_is_local {
            debug!(track_id = %entry.track_id, allow_remote, "local provider has no server lyrics");
            let events = self.events.clone();
            let store = self.store.clone();
            let local_external_providers = external_providers.clone();
            thread::spawn(move || {
                match allow_remote.then(|| external_best_lyrics(&entry, &local_external_providers))
                {
                    Some(Ok(Some(lyrics))) => {
                        debug!(track_id = %entry.track_id, provider = ?lyrics.external_provider, "loaded lyrics from external provider");
                        let _saved =
                            store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                        let _sent = events.send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
                    }
                    Some(Err(error)) => {
                        debug!(track_id = %entry.track_id, %error, "external lyric fallback failed");
                        let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                    }
                    Some(Ok(None)) | None => {
                        let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                    }
                }
            });
            return;
        }
        debug!(track_id = %entry.track_id, allow_remote, ?search, "requesting lyrics from provider");
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.saved_server(&server_id))
                .unwrap_or(None)
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
                    match allow_remote.then(|| external_best_lyrics(&entry, &external_providers)) {
                        Some(Ok(Some(lyrics))) => {
                            debug!(track_id = %entry.track_id, provider = ?lyrics.external_provider, "loaded lyrics from external provider");
                            let _saved =
                                store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                            let _sent =
                                events.send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
                        }
                        Some(Err(error)) => {
                            debug!(track_id = %entry.track_id, %error, "external lyric fallback failed");
                            let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                        }
                        Some(Ok(None)) | None => {
                            let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                        }
                    }
                }
                Err(error) => match allow_remote
                    .then(|| external_best_lyrics(&entry, &external_providers))
                {
                    Some(Ok(Some(lyrics))) => {
                        debug!(track_id = %entry.track_id, provider = ?lyrics.external_provider, "loaded lyrics from external provider after provider error");
                        let _saved =
                            store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                        let _sent = events.send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
                    }
                    Some(Err(fallback_error)) => {
                        debug!(track_id = %entry.track_id, %fallback_error, "external lyric fallback failed");
                        let _sent = events.send(ControllerEvent::Error(error));
                        let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                    }
                    Some(Ok(None)) | None => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                    }
                },
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
        let settings = load_settings_from_store(&self.store);
        if settings.private_mode || !settings.external_lyrics_enabled {
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
        thread::spawn(
            move || match save_lrclib_result(&server_id, &entry, &result, output_path) {
                Ok((path, lyrics)) => {
                    let _saved = store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                    let _sent = events.send(ControllerEvent::LyricsSaved { path, lyrics });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            },
        );
    }
    pub fn preview_lyrics_search_result(&self, track_id: TrackId, result: LyricsSearchResult) {
        let Some((server_id, entry, _position)) = self.current_queue_entry() else {
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
        thread::spawn(
            move || match lyrics_from_search_result(entry.track_id, &result) {
                Ok(Some(lyrics)) => {
                    let _saved = store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                    let _sent = events.send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
                }
                Ok(None) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "Selected lyric result has no lyrics to load.".to_string(),
                    ));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            },
        );
    }
}
