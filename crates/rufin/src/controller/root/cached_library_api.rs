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
        let detail = self
            .store
            .with_store(|store| store.load_album_detail(&saved.source_id, album_id))?;
        Ok(detail)
    }
    pub fn cached_album_tracks(
        &self,
        album_ids: &[AlbumId],
    ) -> Result<std::collections::HashMap<AlbumId, Vec<Track>>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(std::collections::HashMap::new());
        };
        let tracks_by_album = self
            .store
            .with_store(|store| store.load_tracks_for_albums(&saved.source_id, album_ids))?;
        Ok(tracks_by_album)
    }
    pub fn cached_artist_detail(
        &self,
        artist_id: &ArtistId,
    ) -> Result<Option<CachedArtistDetail>, String> {
        let detail = self.store.with_store_fast(|store| {
            let Some(saved) = store.active_source()? else {
                return Ok(None);
            };
            store.load_artist_detail(&saved.source_id, artist_id)
        })?;
        Ok(detail)
    }
    pub fn cached_playlist_detail(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Option<library::PlaylistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(None);
        };
        self.cached_playlist_detail_for_saved(playlist_id, &saved)
    }

    pub fn cached_playlist_detail_for_server(
        &self,
        playlist_id: &PlaylistId,
        server: &SourceIdentity,
        _settings: &StoredSettings,
    ) -> Result<Option<library::PlaylistDetail>, String> {
        let Some(saved) = self
            .store
            .with_store(|store| store.stored_source(&server.id))?
        else {
            return Ok(None);
        };
        self.cached_playlist_detail_for_saved(playlist_id, &saved)
    }

    fn cached_playlist_detail_for_saved(
        &self,
        playlist_id: &PlaylistId,
        saved: &StoredSource,
    ) -> Result<Option<library::PlaylistDetail>, String> {
        let Some(mut detail) = self
            .store
            .with_store(|store| store.load_playlist_detail(&saved.source_id, playlist_id))?
        else {
            return Ok(None);
        };
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
        let detail = self
            .store
            .with_store(|store| store.load_genre_detail(&saved.source_id, genre_id))?;
        Ok(detail)
    }

    pub fn cached_mood_detail(&self, mood_id: &MoodId) -> Result<Option<CachedMoodDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(None);
        };
        let detail = self
            .store
            .with_store(|store| store.load_mood_detail(&saved.source_id, mood_id))?;
        Ok(detail)
    }

    pub fn cached_albums_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        let page = self
            .store
            .with_store(|store| store.load_albums(&saved.source_id, offset, limit))?;
        Ok(page)
    }
    pub fn cached_albums_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        let page = self.store.with_store(|store| {
            store.load_albums_matching(&saved.source_id, query, offset, limit)
        })?;
        Ok(page)
    }
    pub fn cached_tracks_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let page = self.store.with_store(|store| {
            let sort = settings.library_list(domain::LibraryListKey::Tracks);
            store.load_tracks_sorted(
                &saved.source_id,
                library_track_sort(sort.sort_key),
                sort.descending,
                offset,
                limit,
            )
        })?;
        Ok(page)
    }
    pub fn cached_tracks_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_from_store(&self.store);
        let page = self.store.with_store(|store| {
            let sort = settings.library_list(domain::LibraryListKey::Tracks);
            store.load_tracks_matching_sorted(
                &saved.source_id,
                query,
                library_track_sort(sort.sort_key),
                sort.descending,
                offset,
                limit,
            )
        })?;
        Ok(page)
    }
    pub fn cached_artists_page(
        &self,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_artists(&saved.source_id, album_artist, offset, limit))
    }
    pub fn cached_artists_page_matching(
        &self,
        album_artist: bool,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store.with_store(|store| {
            store.load_artists_matching(&saved.source_id, album_artist, query, offset, limit)
        })
    }
    pub fn cached_genres_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_genres(&saved.source_id, offset, limit))
    }

    pub fn cached_moods_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Mood>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_moods(&saved.source_id, offset, limit))
    }

    pub fn cached_moods_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Mood>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_moods_matching(&saved.source_id, query, offset, limit))
    }

    pub fn cached_genres_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_genres_matching(&saved.source_id, query, offset, limit))
    }

    pub fn smart_playlist_rule_value_suggestions(
        &self,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok((Vec::new(), Vec::new()));
        };
        self.store.with_store(|store| {
            Ok((
                store.load_track_genre_names(&saved.source_id)?,
                store.load_track_mood_names(&saved.source_id)?,
            ))
        })
    }

    pub fn cached_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_playlists(&saved.source_id, offset, limit))
    }
    pub fn cached_playlists_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store.with_store(|store| {
            store.load_playlists_matching(&saved.source_id, query, offset, limit)
        })
    }
    pub fn cached_smart_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<SmartPlaylist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Ok(PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_smart_playlists(&saved.source_id, offset, limit))
    }
    pub fn cached_smart_playlist_detail(
        &self,
        smart_playlist_id: &SmartPlaylistId,
    ) -> Result<Option<SmartPlaylistDetail>, String> {
        let total_started = Instant::now();
        let store_started = Instant::now();
        let Some((detail, active_ms, load_ms)) = self.store.with_store_fast(|store| {
            let active_started = Instant::now();
            let Some(saved) = store.active_source()? else {
                return Ok(None);
            };
            let active_ms = active_started.elapsed().as_millis() as u64;
            let load_started = Instant::now();
            let detail = store.load_smart_playlist_detail(&saved.source_id, smart_playlist_id)?;
            let load_ms = load_started.elapsed().as_millis() as u64;
            Ok(Some((detail, active_ms, load_ms)))
        })?
        else {
            return Ok(None);
        };
        let store_ms = store_started.elapsed().as_millis() as u64;
        let (track_count, playlist_name) = if let Some(detail) = detail.as_ref() {
            (
                detail.tracks.len(),
                Some(detail.smart_playlist.name.as_str().to_string()),
            )
        } else {
            (0, None)
        };
        let total_ms = total_started.elapsed().as_millis() as u64;
        if total_ms >= SLOW_SMART_PLAYLIST_DETAIL_MS {
            warn!(
                smart_playlist_id = %smart_playlist_id.as_str(),
                playlist_name = playlist_name.as_deref().unwrap_or(""),
                track_count,
                store_ms,
                active_ms,
                load_ms,
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
            .with_store(|store| store.missing_builtin_smart_playlists(&saved.source_id))
    }
}
