use super::*;
use std::time::Instant;

const SLOW_SMART_PLAYLIST_DETAIL_MS: u64 = 100;

impl AppController {
    pub fn cached_album_detail(
        &self,
        album_id: &AlbumId,
    ) -> Result<Option<(Album, Vec<Track>)>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(None);
        };
        let settings = load_settings_from_store(&self.store);
        let Some((mut album, mut tracks)) = self
            .store
            .with_store(|store| store.load_album_detail(&saved.source.id, album_id))?
        else {
            return Ok(None);
        };
        scrub_selected_album_image_refs(&saved, &settings, std::slice::from_mut(&mut album));
        scrub_selected_track_image_refs(&saved, &settings, &mut tracks);
        cover_art_policy::bind_album_detail(&mut album, &mut tracks, &settings);
        track_album_refs_with_settings(
            &self.store,
            &saved,
            &settings,
            &mut tracks,
            std::slice::from_ref(&album),
        )?;
        Ok(Some((album, tracks)))
    }
    pub fn cached_album_tracks(
        &self,
        album_ids: &[AlbumId],
    ) -> Result<std::collections::HashMap<AlbumId, Vec<Track>>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(std::collections::HashMap::new());
        };
        let settings = load_settings_from_store(&self.store);
        let mut tracks_by_album = self
            .store
            .with_store(|store| store.load_tracks_for_albums(&saved.source.id, album_ids))?;
        for tracks in tracks_by_album.values_mut() {
            scrub_selected_track_image_refs(&saved, &settings, tracks);
            cover_art_policy::bind_tracks(tracks, &settings);
            track_album_refs(&self.store, &saved, tracks, &[])?;
        }
        Ok(tracks_by_album)
    }
    pub fn cached_artist_detail(
        &self,
        artist_id: &ArtistId,
    ) -> Result<Option<CachedArtistDetail>, String> {
        let Some((saved, mut detail)) = self.store.with_store_fast(|store| {
            let Some(saved) = store.active_source()? else {
                return Ok(None);
            };
            let Some(detail) = store.load_artist_detail(&saved.source.id, artist_id)? else {
                return Ok(None);
            };
            Ok(Some((saved, detail)))
        })?
        else {
            return Ok(None);
        };
        let settings = load_settings_from_store(&self.store);
        scrub_selected_artist_image_refs(
            &saved,
            &settings,
            std::slice::from_mut(&mut detail.artist),
        );
        scrub_selected_album_image_refs(&saved, &settings, &mut detail.albums);
        scrub_selected_album_image_refs(&saved, &settings, &mut detail.appears_on);
        scrub_selected_track_image_refs(&saved, &settings, &mut detail.tracks);
        normalize_artist_detail_image_refs(&mut detail, &settings);
        track_album_refs_with_settings(
            &self.store,
            &saved,
            &settings,
            &mut detail.tracks,
            &detail.albums,
        )?;
        Ok(Some(detail))
    }
    pub fn cached_playlist_detail(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Option<source::PlaylistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(None);
        };
        let settings = load_settings_from_store(&self.store);
        self.cached_playlist_detail_for_saved(playlist_id, &saved, &settings)
    }

    pub fn cached_playlist_detail_for_server(
        &self,
        playlist_id: &PlaylistId,
        server: &SourceIdentity,
        settings: &AppSettings,
    ) -> Result<Option<source::PlaylistDetail>, String> {
        let saved = SavedSource {
            source: server.clone(),
            user_id: String::new(),
            username: String::new(),
            trust_invalid_cert: false,
            use_jellyfin_instant_mix: false,
        };
        self.cached_playlist_detail_for_saved(playlist_id, &saved, settings)
    }

    fn cached_playlist_detail_for_saved(
        &self,
        playlist_id: &PlaylistId,
        saved: &SavedSource,
        settings: &AppSettings,
    ) -> Result<Option<source::PlaylistDetail>, String> {
        let Some(mut detail) = self
            .store
            .with_store(|store| store.load_playlist_detail(&saved.source.id, playlist_id))?
        else {
            return Ok(None);
        };
        scrub_selected_playlist_image_refs(
            saved,
            settings,
            std::slice::from_mut(&mut detail.playlist),
        );
        cover_art_policy::bind_playlist_detail(&mut detail, settings);
        scrub_selected_track_image_refs(saved, settings, &mut detail.tracks);
        cover_art_policy::bind_tracks(&mut detail.tracks, settings);
        for (entry, track) in detail.entries.iter_mut().zip(detail.tracks.iter().cloned()) {
            entry.track = track;
        }
        Ok(Some(detail))
    }
    pub fn cached_genre_detail(
        &self,
        genre_id: &GenreId,
    ) -> Result<Option<CachedGenreDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(None);
        };
        let settings = load_settings_from_store(&self.store);
        let Some(mut detail) = self
            .store
            .with_store(|store| store.load_genre_detail(&saved.source.id, genre_id))?
        else {
            return Ok(None);
        };
        scrub_selected_album_image_refs(&saved, &settings, &mut detail.albums);
        scrub_selected_track_image_refs(&saved, &settings, &mut detail.tracks);
        cover_art_policy::bind_albums(&mut detail.albums, &settings);
        cover_art_policy::bind_tracks(&mut detail.tracks, &settings);
        track_album_refs_with_settings(
            &self.store,
            &saved,
            &settings,
            &mut detail.tracks,
            &detail.albums,
        )?;
        Ok(Some(detail))
    }

    pub fn cached_mood_detail(&self, mood_id: &MoodId) -> Result<Option<CachedMoodDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(None);
        };
        let settings = load_settings_from_store(&self.store);
        let Some(mut detail) = self
            .store
            .with_store(|store| store.load_mood_detail(&saved.source.id, mood_id))?
        else {
            return Ok(None);
        };
        scrub_selected_mood_image_refs(&saved, &settings, std::slice::from_mut(&mut detail.mood));
        scrub_selected_album_image_refs(&saved, &settings, &mut detail.albums);
        scrub_selected_track_image_refs(&saved, &settings, &mut detail.tracks);
        cover_art_policy::bind_albums(&mut detail.albums, &settings);
        cover_art_policy::bind_tracks(&mut detail.tracks, &settings);
        track_album_refs_with_settings(
            &self.store,
            &saved,
            &settings,
            &mut detail.tracks,
            &detail.albums,
        )?;
        Ok(Some(detail))
    }

    pub fn cached_albums_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self
            .store
            .with_store(|store| store.load_albums(&saved.source.id, offset, limit))?;
        scrub_selected_album_image_refs(&saved, &settings, &mut page.items);
        cover_art_policy::bind_albums(&mut page.items, &settings);
        album_track_refs(&self.store, &saved, &mut page.items)?;
        Ok(page)
    }
    pub fn cached_albums_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self.store.with_store(|store| {
            store.load_albums_matching(&saved.source.id, query, offset, limit)
        })?;
        scrub_selected_album_image_refs(&saved, &settings, &mut page.items);
        cover_art_policy::bind_albums(&mut page.items, &settings);
        album_track_refs(&self.store, &saved, &mut page.items)?;
        Ok(page)
    }
    pub fn cached_tracks_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self.store.with_store(|store| {
            let sort = settings.library_list(domain::LibraryListKey::Tracks);
            store.load_tracks_sorted(
                &saved.source.id,
                sort.sort_key,
                sort.descending,
                offset,
                limit,
            )
        })?;
        scrub_selected_track_image_refs(&saved, &settings, &mut page.items);
        cover_art_policy::bind_tracks(&mut page.items, &settings);
        track_album_refs(&self.store, &saved, &mut page.items, &[])?;
        Ok(page)
    }
    pub fn cached_tracks_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self.store.with_store(|store| {
            let sort = settings.library_list(domain::LibraryListKey::Tracks);
            store.load_tracks_matching_sorted(
                &saved.source.id,
                query,
                sort.sort_key,
                sort.descending,
                offset,
                limit,
            )
        })?;
        scrub_selected_track_image_refs(&saved, &settings, &mut page.items);
        cover_art_policy::bind_tracks(&mut page.items, &settings);
        track_album_refs(&self.store, &saved, &mut page.items, &[])?;
        Ok(page)
    }
    pub fn cached_artists_page(
        &self,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self.store.with_store(|store| {
            store.load_artists(&saved.source.id, album_artist, offset, limit)
        })?;
        scrub_selected_artist_image_refs(&saved, &settings, &mut page.items);
        cover_art_policy::bind_artists(&mut page.items, &settings);
        Ok(page)
    }
    pub fn cached_artists_page_matching(
        &self,
        album_artist: bool,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self.store.with_store(|store| {
            store.load_artists_matching(&saved.source.id, album_artist, query, offset, limit)
        })?;
        scrub_selected_artist_image_refs(&saved, &settings, &mut page.items);
        cover_art_policy::bind_artists(&mut page.items, &settings);
        Ok(page)
    }
    pub fn cached_genres_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self
            .store
            .with_store(|store| store.load_genres(&saved.source.id, offset, limit))?;
        scrub_selected_genre_image_refs(&saved, &settings, &mut page.items);
        Ok(page)
    }

    pub fn cached_moods_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Mood>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self
            .store
            .with_store(|store| store.load_moods(&saved.source.id, offset, limit))?;
        scrub_selected_mood_image_refs(&saved, &settings, &mut page.items);
        Ok(page)
    }

    pub fn cached_moods_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Mood>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self.store.with_store(|store| {
            store.load_moods_matching(&saved.source.id, query, offset, limit)
        })?;
        scrub_selected_mood_image_refs(&saved, &settings, &mut page.items);
        Ok(page)
    }

    pub fn cached_genres_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let mut page = self.store.with_store(|store| {
            store.load_genres_matching(&saved.source.id, query, offset, limit)
        })?;
        scrub_selected_genre_image_refs(&saved, &settings, &mut page.items);
        Ok(page)
    }

    pub fn smart_playlist_rule_value_suggestions(
        &self,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok((Vec::new(), Vec::new()));
        };
        self.store.with_store(|store| {
            Ok((
                store.load_track_genre_names(&saved.source.id)?,
                store.load_track_mood_names(&saved.source.id)?,
            ))
        })
    }

    pub fn cached_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        self.store
            .with_store(|store| store.load_playlists(&saved.source.id, offset, limit))
            .map(|mut page| {
                scrub_selected_playlist_image_refs(&saved, &settings, &mut page.items);
                cover_art_policy::bind_playlists(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_playlists_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        self.store
            .with_store(|store| {
                store.load_playlists_matching(&saved.source.id, query, offset, limit)
            })
            .map(|mut page| {
                scrub_selected_playlist_image_refs(&saved, &settings, &mut page.items);
                cover_art_policy::bind_playlists(&mut page.items, &settings);
                page
            })
    }
    pub fn cached_smart_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<source::PagedResponse<SmartPlaylist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(source::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_smart_playlists(&saved.source.id, offset, limit))
            .map(|mut page| {
                scrub_smart_refs(&saved, &mut page.items);
                page
            })
    }
    pub fn cached_smart_playlist_detail(
        &self,
        smart_playlist_id: &SmartPlaylistId,
    ) -> Result<Option<SmartPlaylistDetail>, String> {
        let total_started = Instant::now();
        let store_started = Instant::now();
        let Some((saved, detail, active_ms, load_ms)) = self.store.with_store_fast(|store| {
            let active_started = Instant::now();
            let Some(saved) = store.active_source()? else {
                return Ok(None);
            };
            let active_ms = active_started.elapsed().as_millis() as u64;
            let load_started = Instant::now();
            let detail = store.load_smart_playlist_detail(&saved.source.id, smart_playlist_id)?;
            let load_ms = load_started.elapsed().as_millis() as u64;
            Ok(Some((saved, detail, active_ms, load_ms)))
        })?
        else {
            return Ok(None);
        };
        let store_ms = store_started.elapsed().as_millis() as u64;
        let settings_started = Instant::now();
        let settings = load_settings_from_store(&self.store);
        let settings_ms = settings_started.elapsed().as_millis() as u64;
        let scrub_started = Instant::now();
        let mut detail = detail;
        let (track_count, playlist_name) = if let Some(detail) = detail.as_mut() {
            scrub_smart_refs(&saved, std::slice::from_mut(&mut detail.smart_playlist));
            scrub_selected_track_image_refs(&saved, &settings, &mut detail.tracks);
            (
                detail.tracks.len(),
                Some(detail.smart_playlist.name.as_str().to_string()),
            )
        } else {
            (0, None)
        };
        let scrub_ms = scrub_started.elapsed().as_millis() as u64;
        let total_ms = total_started.elapsed().as_millis() as u64;
        if total_ms >= SLOW_SMART_PLAYLIST_DETAIL_MS {
            warn!(
                smart_playlist_id = %smart_playlist_id.as_str(),
                playlist_name = playlist_name.as_deref().unwrap_or(""),
                track_count,
                store_ms,
                active_ms,
                settings_ms,
                load_ms,
                scrub_ms,
                total_ms,
                "slow cached smart playlist detail"
            );
        }
        Ok(detail)
    }
    pub fn missing_builtin_smart_playlists(&self) -> Result<Vec<SmartPlaylistBuiltin>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(Vec::new());
        };
        self.store
            .with_store(|store| store.missing_builtin_smart_playlists(&saved.source.id))
    }
}
