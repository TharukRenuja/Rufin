impl AppController {
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
            .with_store(|store| {
                let sort = settings.library_list(rufin_core::LibraryListKey::Tracks);
                store.load_tracks_sorted(
                    &saved.server.id,
                    sort.sort_key,
                    sort.descending,
                    offset,
                    limit,
                )
            })
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
            .with_store(|store| {
                let sort = settings.library_list(rufin_core::LibraryListKey::Tracks);
                store.load_tracks_matching_sorted(
                    &saved.server.id,
                    query,
                    sort.sort_key,
                    sort.descending,
                    offset,
                    limit,
                )
            })
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
        let mut page = self
            .store
            .with_store(|store| store.load_artists(&saved.server.id, album_artist, offset, limit))?;
        normalize_artist_collection_image_refs(
            &self.store,
            &saved,
            &mut page.items,
            album_artist,
            &settings,
        )?;
        Ok(page)
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
        let mut page = self.store.with_store(|store| {
            store.load_artists_matching(&saved.server.id, album_artist, query, offset, limit)
        })?;
        normalize_artist_collection_image_refs(
            &self.store,
            &saved,
            &mut page.items,
            album_artist,
            &settings,
        )?;
        Ok(page)
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
}
