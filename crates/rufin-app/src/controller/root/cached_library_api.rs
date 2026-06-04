use super::*;

impl AppController {
    pub fn cached_album_detail(
        &self,
        album_id: &AlbumId,
    ) -> Result<Option<(Album, Vec<Track>)>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        let Some((mut album, mut tracks)) = self
            .store
            .with_store(|store| store.load_album_detail(&saved.server.id, album_id))?
        else {
            return Ok(None);
        };
        scrub_source_album_image_refs(&saved, std::slice::from_mut(&mut album));
        scrub_source_track_image_refs(&saved, &mut tracks);
        external_metadata::normalize_album_detail(&mut album, &mut tracks, &settings);
        track_album_refs(
            &self.store,
            &saved,
            &mut tracks,
            std::slice::from_ref(&album),
        )?;
        Ok(Some((album, tracks)))
    }
    pub fn cached_album_tracks(
        &self,
        album_ids: &[AlbumId],
    ) -> Result<std::collections::HashMap<AlbumId, Vec<Track>>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(std::collections::HashMap::new());
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        let mut tracks_by_album = self
            .store
            .with_store(|store| store.load_tracks_for_albums(&saved.server.id, album_ids))?;
        for tracks in tracks_by_album.values_mut() {
            scrub_source_track_image_refs(&saved, tracks);
            external_metadata::normalize_tracks(tracks, &settings);
            track_album_refs(&self.store, &saved, tracks, &[])?;
        }
        Ok(tracks_by_album)
    }
    pub fn cached_artist_detail(
        &self,
        artist_id: &ArtistId,
    ) -> Result<Option<CachedArtistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        let Some(mut detail) = self
            .store
            .with_store(|store| store.load_artist_detail(&saved.server.id, artist_id))?
        else {
            return Ok(None);
        };
        scrub_source_artist_image_refs(&saved, std::slice::from_mut(&mut detail.artist));
        scrub_source_album_image_refs(&saved, &mut detail.albums);
        scrub_source_album_image_refs(&saved, &mut detail.appears_on);
        scrub_source_track_image_refs(&saved, &mut detail.tracks);
        normalize_artist_detail_image_refs(&mut detail, &settings);
        track_album_refs(&self.store, &saved, &mut detail.tracks, &detail.albums)?;
        Ok(Some(detail))
    }
    pub fn cached_playlist_detail(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Option<rufin_provider::PlaylistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        let Some(mut detail) = self
            .store
            .with_store(|store| store.load_playlist_detail(&saved.server.id, playlist_id))?
        else {
            return Ok(None);
        };
        scrub_source_track_image_refs(&saved, &mut detail.tracks);
        external_metadata::normalize_tracks(&mut detail.tracks, &settings);
        track_album_refs(&self.store, &saved, &mut detail.tracks, &[])?;
        let mut entry_tracks = detail
            .entries
            .iter()
            .map(|entry| entry.track.clone())
            .collect::<Vec<_>>();
        scrub_source_track_image_refs(&saved, &mut entry_tracks);
        external_metadata::normalize_tracks(&mut entry_tracks, &settings);
        track_album_refs(&self.store, &saved, &mut entry_tracks, &[])?;
        for (entry, track) in detail.entries.iter_mut().zip(entry_tracks) {
            entry.track = track;
        }
        Ok(Some(detail))
    }
    pub fn cached_genre_detail(
        &self,
        genre_id: &GenreId,
    ) -> Result<Option<CachedGenreDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        let Some(mut detail) = self
            .store
            .with_store(|store| store.load_genre_detail(&saved.server.id, genre_id))?
        else {
            return Ok(None);
        };
        scrub_source_album_image_refs(&saved, &mut detail.albums);
        scrub_source_track_image_refs(&saved, &mut detail.tracks);
        external_metadata::normalize_albums(&mut detail.albums, &settings);
        external_metadata::normalize_tracks(&mut detail.tracks, &settings);
        track_album_refs(&self.store, &saved, &mut detail.tracks, &detail.albums)?;
        Ok(Some(detail))
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
                scrub_source_album_image_refs(&saved, &mut page.items);
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
                scrub_source_album_image_refs(&saved, &mut page.items);
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
        let mut page = self.store.with_store(|store| {
            let sort = settings.library_list(rufin_core::LibraryListKey::Tracks);
            store.load_tracks_sorted(
                &saved.server.id,
                sort.sort_key,
                sort.descending,
                offset,
                limit,
            )
        })?;
        scrub_source_track_image_refs(&saved, &mut page.items);
        external_metadata::normalize_tracks(&mut page.items, &settings);
        track_album_refs(&self.store, &saved, &mut page.items, &[])?;
        Ok(page)
    }
    pub fn cached_track(&self, track_id: &TrackId) -> Result<Option<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        let Some(mut track) = self
            .store
            .with_store(|store| store.load_track(&saved.server.id, track_id))?
        else {
            return Ok(None);
        };
        scrub_source_track_image_refs(&saved, std::slice::from_mut(&mut track));
        external_metadata::normalize_track(&mut track, &settings);
        track_album_refs(&self.store, &saved, std::slice::from_mut(&mut track), &[])?;
        Ok(Some(track))
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
        let mut page = self.store.with_store(|store| {
            let sort = settings.library_list(rufin_core::LibraryListKey::Tracks);
            store.load_tracks_matching_sorted(
                &saved.server.id,
                query,
                sort.sort_key,
                sort.descending,
                offset,
                limit,
            )
        })?;
        scrub_source_track_image_refs(&saved, &mut page.items);
        external_metadata::normalize_tracks(&mut page.items, &settings);
        track_album_refs(&self.store, &saved, &mut page.items, &[])?;
        Ok(page)
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
        let mut page = self.store.with_store(|store| {
            store.load_artists(&saved.server.id, album_artist, offset, limit)
        })?;
        scrub_source_artist_image_refs(&saved, &mut page.items);
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
        scrub_source_artist_image_refs(&saved, &mut page.items);
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
            .map(|mut page| {
                scrub_source_genre_image_refs(&saved, &mut page.items);
                page
            })
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
            .map(|mut page| {
                scrub_source_genre_image_refs(&saved, &mut page.items);
                page
            })
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
            .map(|mut page| {
                scrub_source_playlist_image_refs(&saved, &mut page.items);
                page
            })
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
        self.store
            .with_store(|store| {
                store.load_playlists_matching(&saved.server.id, query, offset, limit)
            })
            .map(|mut page| {
                scrub_source_playlist_image_refs(&saved, &mut page.items);
                page
            })
    }
    pub fn cached_smart_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<SmartPlaylist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_smart_playlists(&saved.server.id, offset, limit))
            .map(|mut page| {
                scrub_smart_refs(&saved, &mut page.items);
                page
            })
    }
    pub fn cached_smart_playlist_detail(
        &self,
        smart_playlist_id: &SmartPlaylistId,
    ) -> Result<Option<SmartPlaylistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        self.store
            .with_store(|store| {
                store.load_smart_playlist_detail(&saved.server.id, smart_playlist_id)
            })
            .map(|mut detail| {
                if let Some(detail) = detail.as_mut() {
                    scrub_smart_refs(&saved, std::slice::from_mut(&mut detail.smart_playlist));
                    scrub_source_track_image_refs(&saved, &mut detail.tracks);
                }
                detail
            })
    }
    pub fn missing_builtin_smart_playlists(&self) -> Result<Vec<SmartPlaylistBuiltin>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(Vec::new());
        };
        self.store
            .with_store(|store| store.missing_builtin_smart_playlists(&saved.server.id))
    }
}
