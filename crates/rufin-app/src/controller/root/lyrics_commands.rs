impl AppController {
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
        thread::spawn(move || {
            match save_lrclib_result(&server_id, &entry, &result, output_path) {
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
}
