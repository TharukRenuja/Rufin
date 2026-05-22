impl AppController {
    pub fn load_settings(&self) -> AppSettings {
        load_settings_from_store(&self.store)
    }
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.store.save_settings(settings)
    }
    pub fn reload_snapshot(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || match load_snapshot(&store) {
            Ok(snapshot) => {
                let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
            }
            Err(error) => {
                let _sent = events.send(ControllerEvent::Error(error));
            }
        });
    }
    pub fn cached_album_detail(
        &self,
        album_id: &AlbumId,
    ) -> Result<Option<(Album, Vec<Track>)>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_album_detail(&saved.server.id, album_id))
            .map(|detail| {
                detail.map(|(mut album, mut tracks)| {
                    external_metadata::normalize_album_detail(&mut album, &mut tracks, &settings);
                    (album, tracks)
                })
            })
    }
    pub fn cached_album_tracks(
        &self,
        album_ids: &[AlbumId],
    ) -> Result<std::collections::HashMap<AlbumId, Vec<Track>>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(std::collections::HashMap::new());
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_tracks_for_albums(&saved.server.id, album_ids))
            .map(|mut tracks_by_album| {
                for tracks in tracks_by_album.values_mut() {
                    external_metadata::normalize_tracks(tracks, &settings);
                }
                tracks_by_album
            })
    }
    pub fn cached_artist_detail(
        &self,
        artist_id: &ArtistId,
    ) -> Result<Option<CachedArtistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_artist_detail(&saved.server.id, artist_id))
            .map(|detail| {
                detail.map(|mut detail| {
                    normalize_artist_detail_image_refs(&mut detail, &settings);
                    detail
                })
            })
    }
    pub fn cached_playlist_cover_refs(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Vec<ImageRef>, String> {
        self.cached_playlist_detail(playlist_id).map(|detail| {
            detail
                .map(|detail| track_cover_refs_for_items(&detail.tracks))
                .unwrap_or_default()
        })
    }
    pub fn cached_playlist_detail(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Option<rufin_provider::PlaylistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_playlist_detail(&saved.server.id, playlist_id))
            .map(|detail| {
                detail.map(|mut detail| {
                    external_metadata::normalize_tracks(&mut detail.tracks, &settings);
                    for entry in &mut detail.entries {
                        external_metadata::normalize_track(&mut entry.track, &settings);
                    }
                    detail
                })
            })
    }
    pub fn cached_genre_detail(
        &self,
        genre_id: &GenreId,
    ) -> Result<Option<CachedGenreDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_genre_detail(&saved.server.id, genre_id))
            .map(|detail| {
                detail.map(|mut detail| {
                    external_metadata::normalize_albums(&mut detail.albums, &settings);
                    external_metadata::normalize_tracks(&mut detail.tracks, &settings);
                    detail
                })
            })
    }
    pub fn cached_albums_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_albums(&saved.server.id, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_albums(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_albums_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_albums_matching(&saved.server.id, query, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_albums(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_tracks_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_tracks(&saved.server.id, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_tracks(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_track(&self, track_id: &TrackId) -> Result<Option<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_track(&saved.server.id, track_id))
            .map(|track| {
                track.map(|mut track| {
                    external_metadata::normalize_track(&mut track, &settings);
                    track
                })
            })
    }
    pub fn cached_tracks_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_tracks_matching(&saved.server.id, query, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_tracks(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_artists_page(
        &self,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_artists(&saved.server.id, album_artist, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_artists(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_artists_page_matching(
        &self,
        album_artist: bool,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| {
                store.load_artists_matching(&saved.server.id, album_artist, query, offset, limit)
            })
            .map(|mut page| {
                external_metadata::normalize_artists(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_genres_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_genres(&saved.server.id, offset, limit))
    }
    pub fn cached_genres_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_genres_matching(&saved.server.id, query, offset, limit))
    }
    pub fn cached_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_playlists(&saved.server.id, offset, limit))
    }
    pub fn cached_playlists_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store.with_store(|store| {
            store.load_playlists_matching(&saved.server.id, query, offset, limit)
        })
    }
    pub fn bootstrap(
        fake_scale: Option<FakeScale>,
    ) -> (
        Self,
        Receiver<ControllerEvent>,
        LibrarySnapshot,
        Option<QueueSnapshot>,
        PlaybackSnapshot,
    ) {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
        if let Some(scale) = fake_scale {
            let store = StoreHandle::open_memory()
                .unwrap_or_else(|error| panic!("failed to open fake memory store: {error}"));
            seed_fake_cache(&store, scale)
                .unwrap_or_else(|error| panic!("failed to seed fake cache: {error}"));
            let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
                warn!(%error, "failed to load fake snapshot");
                LibrarySnapshot::first_run()
            });
            let settings = load_settings_from_store(&store);
            let queue = restore_queue(&store, snapshot.server.as_ref());
            let queue_snapshot = queue.as_ref().map(QueueEngine::snapshot);
            let playback_snapshot = playback_snapshot_from_queue(
                queue.as_ref(),
                settings.auto_dj_enabled,
                &settings.playback,
            );
            let controller = Self {
                store,
                runtime,
                secrets: Arc::new(MemorySecretStore::new()),
                queue: Arc::new(Mutex::new(queue)),
                playback: Arc::new(Mutex::new(Box::new(FakePlaybackBackend::new()))),
                playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
                auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
                last_progress_snapshot: Arc::new(Mutex::new(None)),
                last_report_snapshot: Arc::new(Mutex::new(None)),
                external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
                events,
                sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
                home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
                playlist_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
                explore_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
                cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
                external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
                cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
            };
            return (
                controller,
                receiver,
                snapshot,
                queue_snapshot,
                playback_snapshot,
            );
        }
        let store = StoreHandle::open_for_app().unwrap_or_else(|error| {
            warn!(%error, "failed to open app store, falling back to memory");
            StoreHandle::open_memory().unwrap_or_else(|memory_error| {
                panic!("failed to open memory store: {memory_error}")
            })
        });
        let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
            warn!(%error, "failed to load app snapshot");
            LibrarySnapshot::first_run()
        });
        let settings = load_settings_from_store(&store);
        let queue = restore_queue(&store, snapshot.server.as_ref());
        let queue_snapshot = queue.as_ref().map(QueueEngine::snapshot);
        let playback_snapshot = playback_snapshot_from_queue(
            queue.as_ref(),
            settings.auto_dj_enabled,
            &settings.playback,
        );
        let controller = Self {
            store,
            runtime,
            secrets: platform_secret_store(),
            queue: Arc::new(Mutex::new(queue)),
            playback: Arc::new(Mutex::new(playback_backend(false))),
            playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            playlist_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            explore_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        };
        (
            controller,
            receiver,
            snapshot,
            queue_snapshot,
            playback_snapshot,
        )
    }
    #[cfg(test)]
    fn bootstrap_memory_for_test() -> (
        Self,
        Receiver<ControllerEvent>,
        LibrarySnapshot,
        Option<QueueSnapshot>,
        PlaybackSnapshot,
    ) {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
        let store = StoreHandle::open_memory()
            .unwrap_or_else(|error| panic!("failed to open memory store: {error}"));
        let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
            panic!("failed to load memory snapshot: {error}");
        });
        let settings = load_settings_from_store(&store);
        let controller = Self {
            store,
            runtime,
            secrets: Arc::new(MemorySecretStore::new()),
            queue: Arc::new(Mutex::new(None)),
            playback: Arc::new(Mutex::new(Box::new(FakePlaybackBackend::new()))),
            playback_snapshot: Arc::new(Mutex::new(PlaybackSnapshot {
                auto_dj_enabled: settings.auto_dj_enabled,
                volume: settings.playback.volume,
                muted: settings.playback.muted,
                ..PlaybackSnapshot::default()
            })),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            playlist_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            explore_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        };
        (
            controller,
            receiver,
            snapshot,
            None,
            PlaybackSnapshot {
                auto_dj_enabled: settings.auto_dj_enabled,
                volume: settings.playback.volume,
                muted: settings.playback.muted,
                ..PlaybackSnapshot::default()
            },
        )
    }
    pub fn clear_active_server_cache_for_app() -> Result<(), String> {
        let store = StoreHandle::open_for_app()?;
        let Some(saved) = store.with_store(|store| store.active_server())? else {
            return Err("No active server is saved.".to_string());
        };
        store.with_store(|store| {
            store.clear_library_cache(&saved.server.id)?;
            Ok(())
        })?;
        clear_disk_cover_cache(&saved.server.id)?;
        Ok(())
    }
    pub fn forget_active_server_for_app() -> Result<(), String> {
        let store = StoreHandle::open_for_app()?;
        let Some(saved) = store.with_store(|store| store.active_server())? else {
            return Err("No active server is saved.".to_string());
        };
        platform_secret_store()
            .delete_token(&saved.server.id)
            .map_err(|error| error.to_string())?;
        store.with_store(|store| {
            store.forget_server(&saved.server.id)?;
            Ok(())
        })?;
        clear_disk_cover_cache(&saved.server.id)?;
        Ok(())
    }
    pub fn start_background_sync_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        }
    }
    #[cfg(test)]
    pub fn resync_active_server(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "No active music server is saved.".to_string(),
            ));
        }
    }
    pub fn resync_server(&self, server_id: ServerId) {
        let saved = self
            .store
            .with_store(|store| {
                Ok(store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id))
            })
            .unwrap_or(None);
        if let Some(saved) = saved {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "The selected server is no longer saved.".to_string(),
            ));
        }
    }
    pub fn refresh_home_sections_without_explore_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, HomeRefreshTarget::WithoutExplore);
        }
    }
    pub fn refresh_home_section_for_active(&self, kind: HomeSectionKind) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, HomeRefreshTarget::Section(kind));
        }
    }
    pub fn refresh_playlists_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_playlist_refresh_for_saved(saved);
        }
    }
    pub fn prefetch_explore_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_explore_prefetch_for_saved(saved);
        }
    }
    pub fn promote_prefetched_explore_for_active(&self, section: HomeSection) {
        if section.kind != HomeSectionKind::Explore {
            return;
        }
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        let Some(saved) = active else {
            return;
        };
        start_prefetched_home_section_promotion_thread(
            self.store.clone(),
            self.events.clone(),
            saved.server.id,
            section,
        );
    }
    fn start_explore_prefetch_for_saved(&self, saved: SavedServer) {
        start_explore_prefetch_thread(
            ExplorePrefetchContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                explore_prefetch_in_flight: Arc::clone(&self.explore_prefetch_in_flight),
            },
            saved,
        );
    }
    fn start_home_refresh_for_saved(&self, saved: SavedServer, target: HomeRefreshTarget) {
        start_home_refresh_thread(
            HomeRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                home_refresh_in_flight: Arc::clone(&self.home_refresh_in_flight),
            },
            saved,
            target,
        );
    }
    fn start_playlist_refresh_for_saved(&self, saved: SavedServer) {
        start_playlist_refresh_thread(
            PlaylistRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                playlist_refresh_in_flight: Arc::clone(&self.playlist_refresh_in_flight),
            },
            saved,
        );
    }
    fn sync_context(&self) -> SyncContext {
        SyncContext {
            store: self.store.clone(),
            runtime: Arc::clone(&self.runtime),
            secrets: Arc::clone(&self.secrets),
            events: self.events.clone(),
            sync_in_flight: Arc::clone(&self.sync_in_flight),
            cover_in_flight: Arc::clone(&self.cover_in_flight),
            external_cover_prefetch_in_flight: Arc::clone(&self.external_cover_prefetch_in_flight),
            cover_slots: Arc::clone(&self.cover_slots),
        }
    }
    pub fn startup_sync_delay_ms(&self) -> Option<u64> {
        let saved = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()?;
        let albums = self
            .store
            .with_store(|store| {
                store
                    .load_albums(&saved.server.id, 0, 1)
                    .map(|page| page.total)
            })
            .unwrap_or(0);
        let tracks = self
            .store
            .with_store(|store| {
                store
                    .load_tracks(&saved.server.id, 0, 1)
                    .map(|page| page.total)
            })
            .unwrap_or(0);
        if albums == 0 && tracks == 0 {
            return Some(500);
        }
        let sync_state = self
            .store
            .with_store(|store| store.sync_state(&saved.server.id))
            .ok();
        if sync_state
            .as_ref()
            .is_some_and(|state| state.status == "error")
        {
            return Some(8_000);
        }
        let age = self
            .store
            .with_store(|store| store.sync_completed_age_seconds(&saved.server.id))
            .ok()
            .flatten();
        match age {
            Some(seconds) if seconds < STARTUP_CACHE_STALE_SECONDS => None,
            _ => Some(8_000),
        }
    }
    pub fn play_tracks_now(&self, tracks: Vec<Track>) {
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
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }
    pub fn play_now(&self, track: Track) {
        self.play_tracks_now(vec![track]);
    }
    pub fn play_album_now(&self, album_id: AlbumId) {
        match self.cached_album_detail(&album_id) {
            Ok(Some((_, tracks))) => self.play_tracks_now(tracks),
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
    pub fn play_next(&self, track: Track) {
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
        if removed_current && !has_current_after_remove {
            let _result = self.send_playback_command(PlaybackCommand::Stop);
        }
        self.persist_and_emit_queue();
        if removed_current && has_current_after_remove {
            self.start_current_track();
        }
    }
    pub fn activate_queue_entry(&self, entry_id: QueueEntryId) {
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
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }
    pub fn move_queue_entry_after_current(&self, entry_id: QueueEntryId) {
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
    pub fn clear_queue(&self) {
        let result = self.with_queue_mut(|queue| {
            queue.clear();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        let _result = self.send_playback_command(PlaybackCommand::Stop);
        self.persist_and_emit_queue();
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
        self.persist_and_emit_queue();
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
        self.persist_and_emit_queue();
    }
}
