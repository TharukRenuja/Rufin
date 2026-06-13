use super::servers::*;
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncCompleteDelta {
    pub pruned_cover_entries: Vec<CoverCacheEntry>,
    pub delta: LibraryDelta,
}

impl Store {
    pub fn complete_sync_delta(
        &self,
        server_id: &ServerId,
        generation: i64,
    ) -> StoreResult<SyncCompleteDelta> {
        let deleted = self.stale_library_ids(server_id, generation)?;
        let pruned_cover_entries = self.complete_sync(server_id, generation)?;
        Ok(SyncCompleteDelta {
            pruned_cover_entries,
            delta: deleted,
        })
    }

    pub fn upsert_albums_delta(
        &self,
        server_id: &ServerId,
        albums: &[Album],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        for album in albums {
            let delta_album = canonical_album_for_write(&self.connection, server_id, album)?;
            match self.load_album_for_delta(server_id, &album.id)? {
                Some(existing) if existing == delta_album => {}
                Some(existing) => {
                    if album_stats_changed(&existing, &delta_album) {
                        delta.albums.stats.push(album.id.clone());
                    }
                    if album_links_changed(&existing, &delta_album) {
                        delta.albums.links.push(album.id.clone());
                    }
                    if existing.image_ref != delta_album.image_ref {
                        delta.albums.cover_refs.push(album.id.clone());
                    }
                    if album_fields_changed(&existing, &delta_album) {
                        delta.albums.fields.push(album.id.clone());
                    }
                }
                None => delta.albums.added.push(album.id.clone()),
            }
        }
        self.upsert_albums(server_id, albums, generation)?;
        Ok(delta)
    }

    pub fn upsert_tracks_delta(
        &self,
        server_id: &ServerId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        for track in tracks {
            match self.load_track_for_delta(server_id, &track.id)? {
                Some(existing) => {
                    if !track_changed(&existing, track) {
                        continue;
                    }
                    if existing.favorite != track.favorite {
                        delta.tracks.favorite.push(track.id.clone());
                    }
                    if existing.image_ref != track.image_ref {
                        delta.tracks.cover_refs.push(track.id.clone());
                    }
                    if track_stats_changed(&existing, track) {
                        delta.tracks.stats.push(track.id.clone());
                    }
                    if track_fields_changed(&existing, track) {
                        delta.tracks.fields.push(track.id.clone());
                    }
                    if existing.album_id != track.album_id {
                        delta.albums.links.push(existing.album_id.clone());
                        delta.albums.links.push(track.album_id.clone());
                    }
                    if track_artist_links_changed(&existing, track) {
                        if let Some(artist_id) = existing.artist_id.clone() {
                            delta.artists.links.push(artist_id);
                        }
                        if let Some(artist_id) = track.artist_id.clone() {
                            delta.artists.links.push(artist_id);
                        }
                    }
                    if existing.genres != track.genres {
                        delta.genres.links.extend(
                            existing
                                .genres
                                .iter()
                                .chain(track.genres.iter())
                                .map(|name| GenreId::new(name.clone())),
                        );
                    }
                }
                None => {
                    delta.tracks.added.push(track.id.clone());
                    delta.albums.links.push(track.album_id.clone());
                    if let Some(artist_id) = track.artist_id.clone() {
                        delta.artists.links.push(artist_id);
                    }
                    delta
                        .genres
                        .links
                        .extend(track.genres.iter().map(|name| GenreId::new(name.clone())));
                }
            }
        }
        self.upsert_tracks(server_id, tracks, generation)?;
        Ok(delta)
    }

    pub fn upsert_artists_delta(
        &self,
        server_id: &ServerId,
        artists: &[Artist],
        album_artist: bool,
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        let canonical_artists;
        let delta_artists = if album_artist {
            canonical_artists =
                canonical_album_artists_for_write(&self.connection, server_id, artists)?
                    .into_iter()
                    .map(|artist| artist.artist)
                    .collect::<Vec<_>>();
            canonical_artists.as_slice()
        } else {
            artists
        };
        for artist in delta_artists {
            match self.load_artist_for_delta(server_id, &artist.id, album_artist)? {
                Some(existing) if !artist_projection_changed(&existing, artist) => {}
                Some(existing) => {
                    let entity = if album_artist {
                        &mut delta.album_artists
                    } else {
                        &mut delta.artists
                    };
                    if existing.album_count != artist.album_count
                        || existing.track_count != artist.track_count
                    {
                        entity.stats.push(artist.id.clone());
                    }
                    if existing.image_ref != artist.image_ref {
                        entity.cover_refs.push(artist.id.clone());
                    }
                    entity.fields.push(artist.id.clone());
                }
                None => {
                    if album_artist {
                        delta.album_artists.added.push(artist.id.clone());
                    } else {
                        delta.artists.added.push(artist.id.clone());
                    }
                }
            }
        }
        self.upsert_artists(server_id, artists, album_artist, generation)?;
        Ok(delta)
    }

    pub fn upsert_genres_delta(
        &self,
        server_id: &ServerId,
        genres: &[Genre],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        for genre in genres {
            match self.load_genre_for_delta(server_id, &genre.id)? {
                Some(existing) if existing == *genre => {}
                Some(existing) => {
                    if existing.album_count != genre.album_count
                        || existing.track_count != genre.track_count
                    {
                        delta.genres.stats.push(genre.id.clone());
                    }
                    if existing.image_ref != genre.image_ref
                        || existing.image_refs != genre.image_refs
                    {
                        delta.genres.cover_refs.push(genre.id.clone());
                    }
                    if existing != *genre {
                        delta.genres.fields.push(genre.id.clone());
                    }
                }
                None => delta.genres.added.push(genre.id.clone()),
            }
        }
        self.upsert_genres(server_id, genres, generation)?;
        Ok(delta)
    }

    pub fn upsert_playlists_delta(
        &self,
        server_id: &ServerId,
        playlists: &[Playlist],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        for playlist in playlists {
            match self.load_playlist_for_delta(server_id, &playlist.id)? {
                Some(existing) if existing == *playlist => {}
                Some(existing) => {
                    if !self.playlist_has_entries(server_id, &playlist.id)?
                        && (existing.track_count != playlist.track_count
                            || existing.duration_seconds != playlist.duration_seconds)
                    {
                        delta.playlists.entries.push(playlist.id.clone());
                    }
                    if existing.image_ref != playlist.image_ref
                        || existing.image_refs != playlist.image_refs
                    {
                        delta.playlists.cover_refs.push(playlist.id.clone());
                    }
                    if existing.name != playlist.name {
                        delta.playlists.fields.push(playlist.id.clone());
                    }
                }
                None => delta.playlists.added.push(playlist.id.clone()),
            }
        }
        self.upsert_playlists(server_id, playlists, generation)?;
        Ok(delta)
    }

    pub fn upsert_home_sections_delta(
        &self,
        server_id: &ServerId,
        sections: &[HomeSection],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let before = self.load_home_sections(server_id)?;
        self.upsert_home_sections(server_id, sections, generation)?;
        let after = self.load_home_sections(server_id)?;
        Ok(LibraryDelta {
            home_changed: home_keys(&before) != home_keys(&after),
            ..LibraryDelta::default()
        })
    }

    pub fn complete_sync(
        &self,
        server_id: &ServerId,
        generation: i64,
    ) -> StoreResult<Vec<CoverCacheEntry>> {
        self.write_batch(|_| {
            self.prune_missing_items(server_id, generation)?;
            self.bind_album_fallback_image_refs(server_id)?;
            self.bind_album_external_identity_image_refs(server_id)?;
            self.bind_track_album_fallback_image_refs(server_id)?;
            self.bind_artist_fallback_image_refs(server_id, false)?;
            self.bind_artist_fallback_image_refs(server_id, true)?;
            self.refresh_collection_cover_refs(server_id)?;
            self.refresh_smart_playlist_cover_refs(server_id)?;
            let pruned_cover_entries = self.prune_stale_image_cache_entries(server_id)?;
            self.connection.execute(
                "
                UPDATE sync_state
                SET status = 'idle',
                    generation = ?2,
                    last_completed_at = CURRENT_TIMESTAMP,
                    last_error = NULL
                WHERE server_id = ?1
                ",
                params![server_id.as_str(), generation],
            )?;
            Ok(pruned_cover_entries)
        })
    }

    pub fn repair_artwork_projections(&self, server_id: &ServerId) -> StoreResult<usize> {
        self.write_batch(|_| {
            let mut changed = 0;
            changed += self.bind_album_fallback_image_refs(server_id)?;
            changed += self.bind_album_external_identity_image_refs(server_id)?;
            changed += self.bind_track_album_fallback_image_refs(server_id)?;
            changed += self.bind_artist_fallback_image_refs(server_id, false)?;
            changed += self.bind_artist_fallback_image_refs(server_id, true)?;
            if changed > 0 {
                self.refresh_collection_cover_refs(server_id)?;
                self.refresh_smart_playlist_cover_refs(server_id)?;
            }
            Ok(changed)
        })
    }

    pub fn fail_sync(&self, server_id: &ServerId, error: &str) -> StoreResult<()> {
        self.connection.execute(
            "
            UPDATE sync_state
            SET status = 'error',
                last_error = ?2
            WHERE server_id = ?1
            ",
            params![server_id.as_str(), error],
        )?;
        Ok(())
    }
    pub fn clear_library_cache(&self, server_id: &ServerId) -> StoreResult<()> {
        self.write_batch(|connection| {
            clear_library_cache_on_connection(connection, server_id)?;
            connection.execute(
                "
                UPDATE sync_state
                SET generation = 0,
                    status = 'idle',
                    last_started_at = NULL,
                    last_completed_at = NULL,
                    last_error = NULL
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            Ok(())
        })
    }
    pub fn forget_server(&self, server_id: &ServerId) -> StoreResult<()> {
        self.write_batch(|connection| {
            clear_library_cache_on_connection(connection, server_id)?;
            connection.execute(
                "DELETE FROM queue_snapshots WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM active_server WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM sync_state WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM servers WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            Ok(())
        })
    }
    pub fn upsert_albums(
        &self,
        server_id: &ServerId,
        albums: &[Album],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let albums = albums
                .iter()
                .map(|album| canonical_album_for_write(connection, server_id, album))
                .collect::<StoreResult<Vec<_>>>()?;
            let mut statement = connection.prepare(
                "
                INSERT INTO albums (
                    server_id, album_id, title, artist, artist_id, year, release_date,
                    date_added, last_played, play_count, user_rating, track_count,
                    duration_seconds, favorite, color_seed, image_item_id, image_tag,
                    release_types_json, is_compilation, musicbrainz_album_id,
                    musicbrainz_release_group_id, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
                ON CONFLICT(server_id, album_id) DO UPDATE SET
                    title = excluded.title,
                    artist = excluded.artist,
                    artist_id = excluded.artist_id,
                    year = excluded.year,
                    release_date = excluded.release_date,
                    date_added = excluded.date_added,
                    last_played = excluded.last_played,
                    play_count = excluded.play_count,
                    user_rating = excluded.user_rating,
                    track_count = excluded.track_count,
                    duration_seconds = excluded.duration_seconds,
                    favorite = excluded.favorite,
                    color_seed = excluded.color_seed,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    release_types_json = CASE
                        WHEN excluded.release_types_json <> '[]' THEN excluded.release_types_json
                        WHEN COALESCE(NULLIF(excluded.musicbrainz_album_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_album_id, ''), '')
                         AND COALESCE(NULLIF(excluded.musicbrainz_release_group_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_release_group_id, ''), '')
                         AND albums.release_types_json <> '[]' THEN albums.release_types_json
                        ELSE COALESCE((
                            SELECT fact.value_json
                            FROM entity_facts fact
                            WHERE fact.server_id = albums.server_id
                              AND fact.entity_kind = 'album'
                              AND fact.entity_id = albums.album_id
                              AND fact.fact_key = 'release_types'
                              AND fact.status = 'resolved'
                              AND COALESCE(NULLIF(excluded.musicbrainz_album_id, ''), '') =
                                  COALESCE(NULLIF(albums.musicbrainz_album_id, ''), '')
                              AND COALESCE(NULLIF(excluded.musicbrainz_release_group_id, ''), '') =
                                  COALESCE(NULLIF(albums.musicbrainz_release_group_id, ''), '')
                        ), excluded.release_types_json)
                    END,
                    is_compilation = CASE
                        WHEN excluded.is_compilation IS NOT NULL THEN excluded.is_compilation
                        WHEN COALESCE(NULLIF(excluded.musicbrainz_album_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_album_id, ''), '')
                         AND COALESCE(NULLIF(excluded.musicbrainz_release_group_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_release_group_id, ''), '')
                         THEN COALESCE(albums.is_compilation, (
                            SELECT CASE fact.value_json
                                     WHEN 'true' THEN 1
                                     WHEN 'false' THEN 0
                                     ELSE NULL
                                   END
                            FROM entity_facts fact
                            WHERE fact.server_id = albums.server_id
                              AND fact.entity_kind = 'album'
                              AND fact.entity_id = albums.album_id
                              AND fact.fact_key = 'is_compilation'
                              AND fact.status = 'resolved'
                         ))
                        ELSE NULL
                    END,
                    musicbrainz_album_id = NULLIF(excluded.musicbrainz_album_id, ''),
                    musicbrainz_release_group_id = NULLIF(excluded.musicbrainz_release_group_id, ''),
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_genres = connection.prepare(
                "DELETE FROM album_genres WHERE server_id = ?1 AND album_id = ?2",
            )?;
            let mut delete_artist_links = connection.prepare(
                "DELETE FROM album_artist_links WHERE server_id = ?1 AND album_id = ?2",
            )?;
            let mut insert_genre = connection.prepare(
                "
                INSERT INTO album_genres (server_id, album_id, genre_name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(server_id, album_id, genre_name) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut insert_artist_link = connection.prepare(
                "
                INSERT INTO album_artist_links (
                    server_id, album_id, artist_id, name, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(server_id, album_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = 'album' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'album', ?2, ?3, ?4)
                ",
            )?;

            for album in &albums {
                let (image_item_id, image_tag) = image_ref_parts(album.image_ref.as_ref());
                let release_types_json = album_release_types_json(&album.release_types)?;
                statement.execute(params![
                    server_id.as_str(),
                    album.id.as_str(),
                    album.title,
                    album.artist,
                    album.artist_id.as_ref().map(ArtistId::as_str),
                    i64::from(album.year),
                    album.release_date.as_deref(),
                    album.date_added.as_deref(),
                    album.last_played.as_deref(),
                    album.play_count.map(i64::from),
                    album.user_rating.map(i64::from),
                    i64::from(album.track_count),
                    i64::from(album.duration_seconds),
                    bool_to_i64(album.favorite),
                    i64::from(album.color_seed),
                    image_item_id,
                    image_tag,
                    release_types_json,
                    album.is_compilation.map(bool_to_i64),
                    album.musicbrainz_album_id.as_deref(),
                    album.musicbrainz_release_group_id.as_deref(),
                    generation,
                ])?;
                upsert_album_entity_data_on_connection(connection, server_id, album)?;
                delete_genres.execute(params![server_id.as_str(), album.id.as_str()])?;
                delete_artist_links.execute(params![server_id.as_str(), album.id.as_str()])?;
                for genre in &album.genres {
                    if !genre.trim().is_empty() {
                        insert_genre.execute(params![
                            server_id.as_str(),
                            album.id.as_str(),
                            genre.trim(),
                            generation,
                        ])?;
                    }
                }
                for (position, artist) in album_artist_credits(album).iter().enumerate() {
                    upsert_artist_credit_entity_data_on_connection(
                        connection,
                        server_id,
                        "album_artist",
                        artist,
                    )?;
                    insert_artist_link.execute(params![
                        server_id.as_str(),
                        album.id.as_str(),
                        artist.id.as_str(),
                        artist.name.trim(),
                        position as i64,
                        generation,
                    ])?;
                }
                delete_fts.execute(params![server_id.as_str(), album.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    album.id.as_str(),
                    album.title,
                    album.artist,
                ])?;
            }
            Ok(())
        })
    }
    pub fn upsert_tracks(
        &self,
        server_id: &ServerId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO tracks (
                    server_id, track_id, album_id, title, artist, artist_id, album,
                    year, release_date, date_added, last_played, play_count, user_rating,
                    duration_seconds, favorite, disc_number, track_number,
                    image_item_id, image_tag, local_path, source_format, comment, skip_count,
                    sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
                ON CONFLICT(server_id, track_id) DO UPDATE SET
                    album_id = excluded.album_id,
                    title = excluded.title,
                    artist = excluded.artist,
                    artist_id = excluded.artist_id,
                    album = excluded.album,
                    year = excluded.year,
                    release_date = excluded.release_date,
                    date_added = excluded.date_added,
                    last_played = excluded.last_played,
                    play_count = excluded.play_count,
                    user_rating = excluded.user_rating,
                    duration_seconds = excluded.duration_seconds,
                    favorite = excluded.favorite,
                    disc_number = excluded.disc_number,
                    track_number = excluded.track_number,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    local_path = excluded.local_path,
                    source_format = excluded.source_format,
                    comment = excluded.comment,
                    skip_count = excluded.skip_count,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_genres = connection.prepare(
                "DELETE FROM track_genres WHERE server_id = ?1 AND track_id = ?2",
            )?;
            let mut delete_artist_links = connection.prepare(
                "DELETE FROM track_artist_links WHERE server_id = ?1 AND track_id = ?2",
            )?;
            let mut insert_genre = connection.prepare(
                "
                INSERT INTO track_genres (server_id, track_id, genre_name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(server_id, track_id, genre_name) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut insert_artist_link = connection.prepare(
                "
                INSERT INTO track_artist_links (
                    server_id, track_id, album_id, artist_id, name, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(server_id, track_id, artist_id) DO UPDATE SET
                    album_id = excluded.album_id,
                    name = excluded.name,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut insert_album_artist_link = connection.prepare(
                "
                INSERT INTO album_artist_links (
                    server_id, album_id, artist_id, name, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(server_id, album_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = 'track' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'track', ?2, ?3, ?4)
                ",
            )?;

            for track in tracks {
                let (image_item_id, image_tag) = image_ref_parts(track.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    track.id.as_str(),
                    track.album_id.as_str(),
                    track.title,
                    track.artist,
                    track.artist_id.as_ref().map(ArtistId::as_str),
                    track.album,
                    i64::from(track.year),
                    track.release_date.as_deref(),
                    track.date_added.as_deref(),
                    track.last_played.as_deref(),
                    track.play_count.map(i64::from),
                    track.user_rating.map(i64::from),
                    i64::from(track.duration_seconds),
                    bool_to_i64(track.favorite),
                    i64::from(track.disc_number),
                    i64::from(track.track_number),
                    image_item_id,
                    image_tag,
                    track.local_path.as_deref(),
                    track.source_format.as_deref(),
                    track.comment.as_deref(),
                    track.skip_count.map(i64::from),
                    generation,
                ])?;
                upsert_track_entity_data_on_connection(connection, server_id, track)?;
                delete_genres.execute(params![server_id.as_str(), track.id.as_str()])?;
                delete_artist_links.execute(params![server_id.as_str(), track.id.as_str()])?;
                for genre in &track.genres {
                    if !genre.trim().is_empty() {
                        insert_genre.execute(params![
                            server_id.as_str(),
                            track.id.as_str(),
                            genre.trim(),
                            generation,
                        ])?;
                    }
                }
                for (position, artist) in track_artist_credits(track).iter().enumerate() {
                    upsert_artist_credit_entity_data_on_connection(
                        connection, server_id, "artist", artist,
                    )?;
                    insert_artist_link.execute(params![
                        server_id.as_str(),
                        track.id.as_str(),
                        track.album_id.as_str(),
                        artist.id.as_str(),
                        artist.name.trim(),
                        position as i64,
                        generation,
                    ])?;
                }
                for (position, artist) in track.album_artist_credits.iter().enumerate() {
                    if artist.name.trim().is_empty() {
                        continue;
                    }
                    upsert_artist_credit_entity_data_on_connection(
                        connection,
                        server_id,
                        "album_artist",
                        artist,
                    )?;
                    insert_album_artist_link.execute(params![
                        server_id.as_str(),
                        track.album_id.as_str(),
                        artist.id.as_str(),
                        artist.name.trim(),
                        position as i64,
                        generation,
                    ])?;
                }
                delete_fts.execute(params![server_id.as_str(), track.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    track.id.as_str(),
                    track.title,
                    format!("{} {}", track.artist, track.album),
                ])?;
            }
            Ok(())
        })
    }
    pub fn refresh_library_counts(&self, server_id: &ServerId) -> StoreResult<()> {
        self.write_batch(|connection| {
            let generation = connection
                .query_row(
                    "SELECT generation FROM sync_state WHERE server_id = ?1",
                    params![server_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            repair_linked_artists(connection, server_id, generation)?;
            repair_linked_genres(connection, server_id, generation)?;
            self.bind_album_fallback_image_refs(server_id)?;
            self.bind_album_external_identity_image_refs(server_id)?;
            self.bind_track_album_fallback_image_refs(server_id)?;
            self.bind_artist_fallback_image_refs(server_id, false)?;
            self.bind_artist_fallback_image_refs(server_id, true)?;
            connection.execute(
                "
                UPDATE albums
                SET track_count = MAX(track_count, (
                    SELECT COUNT(*)
                    FROM tracks
                    WHERE tracks.server_id = albums.server_id
                      AND tracks.album_id = albums.album_id
                )),
                    duration_seconds = MAX(duration_seconds, (
                    SELECT COALESCE(SUM(duration_seconds), 0)
                    FROM tracks
                    WHERE tracks.server_id = albums.server_id
                      AND tracks.album_id = albums.album_id
                ))
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                UPDATE artists
                SET track_count = MAX(track_count, (
                    SELECT COUNT(DISTINCT tracks.track_id)
                    FROM tracks
                    LEFT JOIN track_artist_links tal
                        ON tal.server_id = tracks.server_id
                       AND tal.track_id = tracks.track_id
                       AND tal.artist_id = artists.artist_id
                    WHERE tracks.server_id = artists.server_id
                      AND (
                          tracks.artist_id = artists.artist_id
                          OR tal.artist_id IS NOT NULL
                      )
                )),
                    album_count = MAX(album_count, (
                    SELECT COUNT(DISTINCT tracks.album_id)
                    FROM tracks
                    LEFT JOIN track_artist_links tal
                        ON tal.server_id = tracks.server_id
                       AND tal.track_id = tracks.track_id
                       AND tal.artist_id = artists.artist_id
                    WHERE tracks.server_id = artists.server_id
                      AND (
                          tracks.artist_id = artists.artist_id
                          OR tal.artist_id IS NOT NULL
                      )
                ))
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                UPDATE album_artists
                SET track_count = MAX(track_count, (
                    SELECT COALESCE(SUM(track_count), 0)
                    FROM albums
                    WHERE albums.server_id = album_artists.server_id
                      AND (
                          albums.artist_id = album_artists.artist_id
                          OR (
                              TRIM(album_artists.name) != ''
                              AND LOWER(albums.artist) = LOWER(album_artists.name)
                          )
                          OR EXISTS (
                              SELECT 1
                              FROM album_artist_links aal
                              WHERE aal.server_id = albums.server_id
                                AND aal.album_id = albums.album_id
                                AND aal.artist_id = album_artists.artist_id
                          )
                      )
                )),
                    album_count = MAX(album_count, (
                    SELECT COUNT(DISTINCT album_id)
                    FROM albums
                    WHERE albums.server_id = album_artists.server_id
                      AND (
                          albums.artist_id = album_artists.artist_id
                          OR (
                              TRIM(album_artists.name) != ''
                              AND LOWER(albums.artist) = LOWER(album_artists.name)
                          )
                          OR EXISTS (
                              SELECT 1
                              FROM album_artist_links aal
                              WHERE aal.server_id = albums.server_id
                                AND aal.album_id = albums.album_id
                                AND aal.artist_id = album_artists.artist_id
                          )
                      )
                ))
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            refresh_genre_counts_on_connection(connection, server_id)?;
            Ok(())
        })
    }
    pub fn upsert_artists(
        &self,
        server_id: &ServerId,
        artists: &[Artist],
        album_artist: bool,
        generation: i64,
    ) -> StoreResult<()> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        self.write_batch(|connection| {
            let canonical_artists;
            let artists = if album_artist {
                canonical_artists =
                    canonical_album_artists_for_write(connection, server_id, artists)?;
                canonical_artists
                    .iter()
                    .map(|artist| (&artist.artist, artist.alias_ids.as_slice()))
                    .collect::<Vec<_>>()
            } else {
                artists
                    .iter()
                    .map(|artist| (artist, &[][..]))
                    .collect::<Vec<_>>()
            };
            let sql = format!(
                "
                INSERT INTO {table} (
                    server_id, artist_id, name, album_count, track_count, favorite,
                    last_played, play_count, user_rating, image_item_id, image_tag, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(server_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    favorite = excluded.favorite,
                    last_played = excluded.last_played,
                    play_count = excluded.play_count,
                    user_rating = excluded.user_rating,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                "
            );
            let mut statement = connection.prepare(&sql)?;
            let item_type = if album_artist {
                "album_artist"
            } else {
                "artist"
            };
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = ?2 AND item_id = ?3",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, ?2, ?3, ?4, '')
                ",
            )?;

            for (artist, alias_ids) in artists {
                let (image_item_id, image_tag) = image_ref_parts(artist.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    artist.id.as_str(),
                    artist.name,
                    i64::from(artist.album_count),
                    i64::from(artist.track_count),
                    bool_to_i64(artist.favorite),
                    artist.last_played.as_deref(),
                    artist.play_count.map(i64::from),
                    artist.user_rating.map(i64::from),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
                upsert_artist_entity_data_on_connection(
                    connection,
                    server_id,
                    if album_artist {
                        "album_artist"
                    } else {
                        "artist"
                    },
                    artist,
                )?;
                for alias_id in alias_ids {
                    apply_album_artist_alias(connection, server_id, &artist.id, alias_id)?;
                }
                delete_fts.execute(params![server_id.as_str(), item_type, artist.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    item_type,
                    artist.id.as_str(),
                    artist.name,
                ])?;
            }
            Ok(())
        })
    }
    pub fn upsert_genres(
        &self,
        server_id: &ServerId,
        genres: &[Genre],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO genres (
                    server_id, genre_id, name, album_count, track_count, image_item_id,
                    image_tag, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(server_id, genre_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for genre in genres {
                let (image_item_id, image_tag) = image_ref_parts(genre.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    genre.id.as_str(),
                    genre.name,
                    i64::from(genre.album_count),
                    i64::from(genre.track_count),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
                let cover_refs = if genre.image_refs.is_empty() {
                    genre.image_ref.iter().cloned().collect::<Vec<_>>()
                } else {
                    genre.image_refs.clone()
                };
                replace_collection_refs(
                    connection,
                    server_id,
                    COLLECTION_COVER_GENRE,
                    genre.id.as_str(),
                    &cover_refs,
                )?;
            }
            refresh_genre_counts_on_connection(connection, server_id)?;
            Ok(())
        })
    }
    pub fn upsert_playlists(
        &self,
        server_id: &ServerId,
        playlists: &[Playlist],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO playlists (
                    server_id, playlist_id, name, track_count, duration_seconds,
                    image_item_id, image_tag, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(server_id, playlist_id) DO UPDATE SET
                    name = excluded.name,
                    track_count = CASE
                        WHEN EXISTS (
                            SELECT 1
                            FROM playlist_tracks
                            WHERE server_id = excluded.server_id
                                AND playlist_id = excluded.playlist_id
                        ) THEN playlists.track_count
                        ELSE excluded.track_count
                    END,
                    duration_seconds = CASE
                        WHEN EXISTS (
                            SELECT 1
                            FROM playlist_tracks
                            WHERE server_id = excluded.server_id
                                AND playlist_id = excluded.playlist_id
                        ) THEN playlists.duration_seconds
                        ELSE excluded.duration_seconds
                    END,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = 'playlist' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'playlist', ?2, ?3, '')
                ",
            )?;

            for playlist in playlists {
                let (image_item_id, image_tag) = image_ref_parts(playlist.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    playlist.id.as_str(),
                    playlist.name,
                    i64::from(playlist.track_count),
                    i64::from(playlist.duration_seconds),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
                let cover_refs = if playlist.image_refs.is_empty() {
                    playlist.image_ref.iter().cloned().collect::<Vec<_>>()
                } else {
                    playlist.image_refs.clone()
                };
                replace_collection_refs(
                    connection,
                    server_id,
                    COLLECTION_COVER_PLAYLIST,
                    playlist.id.as_str(),
                    &cover_refs,
                )?;
                delete_fts.execute(params![server_id.as_str(), playlist.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    playlist.id.as_str(),
                    playlist.name,
                ])?;
            }
            Ok(())
        })
    }
    pub fn prune_playlists_except(
        &self,
        server_id: &ServerId,
        playlist_ids: &[PlaylistId],
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let keep = playlist_ids
                .iter()
                .map(|playlist_id| playlist_id.as_str().to_string())
                .collect::<Vec<_>>();
            let existing = {
                let mut statement = connection.prepare(
                    "
                    SELECT playlist_id
                    FROM playlists
                    WHERE server_id = ?1
                    ",
                )?;
                collect_rows(
                    statement
                        .query_map(params![server_id.as_str()], |row| row.get::<_, String>(0))?,
                )?
            };

            for playlist_id in existing {
                if keep.iter().any(|keep_id| keep_id == &playlist_id) {
                    continue;
                }
                connection.execute(
                    "
                    DELETE FROM playlist_tracks
                    WHERE server_id = ?1 AND playlist_id = ?2
                    ",
                    params![server_id.as_str(), playlist_id.as_str()],
                )?;
                connection.execute(
                    "
                    DELETE FROM playlists
                    WHERE server_id = ?1 AND playlist_id = ?2
                    ",
                    params![server_id.as_str(), playlist_id.as_str()],
                )?;
                connection.execute(
                    "
                    DELETE FROM collection_cover_refs
                    WHERE server_id = ?1
                      AND collection_type = ?2
                      AND collection_id = ?3
                    ",
                    params![
                        server_id.as_str(),
                        COLLECTION_COVER_PLAYLIST,
                        playlist_id.as_str(),
                    ],
                )?;
                connection.execute(
                    "
                    DELETE FROM library_fts
                    WHERE server_id = ?1
                      AND item_type = 'playlist'
                      AND item_id = ?2
                    ",
                    params![server_id.as_str(), playlist_id.as_str()],
                )?;
            }

            Ok(())
        })
    }
    pub fn upsert_home_sections(
        &self,
        server_id: &ServerId,
        sections: &[HomeSection],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "DELETE FROM home_section_items WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            for section in sections {
                Self::insert_home_section_items(connection, server_id, section, generation)?;
            }
            Ok(())
        })
    }
    pub fn upsert_home_section(
        &self,
        server_id: &ServerId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
                DELETE FROM home_section_items
                WHERE server_id = ?1
                  AND section_kind = ?2
                ",
                params![server_id.as_str(), home_section_kind_key(section.kind)],
            )?;
            Self::insert_home_section_items(connection, server_id, section, generation)
        })
    }
    pub fn upsert_home_section_prefetch(
        &self,
        server_id: &ServerId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
                DELETE FROM home_section_prefetch_items
                WHERE server_id = ?1
                  AND section_kind = ?2
                ",
                params![server_id.as_str(), home_section_kind_key(section.kind)],
            )?;
            Self::insert_home_items(
                connection,
                "home_section_prefetch_items",
                server_id,
                section,
                generation,
            )
        })
    }
    pub fn clear_home_section_prefetch(
        &self,
        server_id: &ServerId,
        kind: HomeSectionKind,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
                DELETE FROM home_section_prefetch_items
                WHERE server_id = ?1
                  AND section_kind = ?2
                ",
                params![server_id.as_str(), home_section_kind_key(kind)],
            )?;
            Ok(())
        })
    }
    pub(super) fn insert_home_section_items(
        connection: &Connection,
        server_id: &ServerId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        Self::insert_home_items(
            connection,
            "home_section_items",
            server_id,
            section,
            generation,
        )
    }
    pub(super) fn insert_home_items(
        connection: &Connection,
        table: &str,
        server_id: &ServerId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        let sql = format!(
            "
            INSERT INTO {table} (
                server_id, section_kind, item_type, item_id, position, sync_generation
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(server_id, section_kind, item_type, item_id) DO UPDATE SET
                position = excluded.position,
                sync_generation = excluded.sync_generation
            "
        );
        let mut insert_item = connection.prepare(&sql)?;
        let section_kind = home_section_kind_key(section.kind);
        for (position, album) in section.albums.iter().enumerate() {
            insert_item.execute(params![
                server_id.as_str(),
                section_kind,
                "album",
                album.id.as_str(),
                position as i64,
                generation,
            ])?;
        }
        for (position, track) in section.tracks.iter().enumerate() {
            insert_item.execute(params![
                server_id.as_str(),
                section_kind,
                "track",
                track.id.as_str(),
                position as i64,
                generation,
            ])?;
        }
        Ok(())
    }
}

impl Store {
    fn load_album_for_delta(
        &self,
        server_id: &ServerId,
        album_id: &AlbumId,
    ) -> StoreResult<Option<Album>> {
        let mut album = self
            .connection
            .query_row(
                "
                SELECT album_id, title, artist, artist_id, year, release_date, date_added,
                       last_played, play_count, user_rating, track_count, duration_seconds,
                       favorite, color_seed, image_item_id, image_tag,
                       release_types_json, is_compilation, musicbrainz_album_id,
                       musicbrainz_release_group_id
                FROM albums
                WHERE server_id = ?1 AND album_id = ?2
                ",
                params![server_id.as_str(), album_id.as_str()],
                album_from_row,
            )
            .optional()?;
        if let Some(album) = album.as_mut() {
            self.attach_album_genres(server_id, std::slice::from_mut(album))?;
            self.attach_album_release_metadata(server_id, std::slice::from_mut(album))?;
            let credits = self.load_artist_links(
                server_id,
                "album_artist_links",
                "album_id",
                &[album.id.as_str().to_string()],
            )?;
            album.album_artist_credits =
                credits.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(album)
    }

    fn load_artist_for_delta(
        &self,
        server_id: &ServerId,
        artist_id: &ArtistId,
        album_artist: bool,
    ) -> StoreResult<Option<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let sql = format!(
            "
            SELECT artist_id, name, album_count, track_count, favorite,
                   last_played, play_count, user_rating, image_item_id, image_tag
            FROM {table}
            WHERE server_id = ?1 AND artist_id = ?2
            "
        );
        self.connection
            .query_row(
                &sql,
                params![server_id.as_str(), artist_id.as_str()],
                artist_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn load_track_for_delta(
        &self,
        server_id: &ServerId,
        track_id: &TrackId,
    ) -> StoreResult<Option<Track>> {
        let mut track = self
            .connection
            .query_row(
                "
                SELECT track_id, album_id, title, artist, artist_id, album, year,
                       release_date, date_added, last_played, play_count, user_rating,
                       duration_seconds, favorite, disc_number, track_number,
                       image_item_id, image_tag, local_path, source_format, comment, skip_count
                FROM tracks
                WHERE server_id = ?1 AND track_id = ?2
                ",
                params![server_id.as_str(), track_id.as_str()],
                track_from_row,
            )
            .optional()?;
        if let Some(track) = track.as_mut() {
            self.attach_track_metadata(server_id, std::slice::from_mut(track))?;
        }
        Ok(track)
    }

    fn load_genre_for_delta(
        &self,
        server_id: &ServerId,
        genre_id: &GenreId,
    ) -> StoreResult<Option<Genre>> {
        let genre = self
            .connection
            .query_row(
                "
                SELECT genre_id, name, album_count, track_count, image_item_id, image_tag
                FROM genres
                WHERE server_id = ?1 AND genre_id = ?2
                ",
                params![server_id.as_str(), genre_id.as_str()],
                genre_from_row,
            )
            .optional()?;
        Ok(genre)
    }

    pub(super) fn load_playlist_for_delta(
        &self,
        server_id: &ServerId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<Option<Playlist>> {
        let playlist = self
            .connection
            .query_row(
                "
                SELECT playlist_id, name, track_count, duration_seconds, image_item_id, image_tag
                FROM playlists
                WHERE server_id = ?1 AND playlist_id = ?2
                ",
                params![server_id.as_str(), playlist_id.as_str()],
                playlist_from_row,
            )
            .optional()?;
        Ok(playlist)
    }

    fn playlist_has_entries(
        &self,
        server_id: &ServerId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<bool> {
        let has_entries = self.connection.query_row(
            "
            SELECT EXISTS (
                SELECT 1
                FROM playlist_tracks
                WHERE server_id = ?1 AND playlist_id = ?2
            )
            ",
            params![server_id.as_str(), playlist_id.as_str()],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(has_entries)
    }

    fn stale_library_ids(
        &self,
        server_id: &ServerId,
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        delta.tracks.deleted =
            self.stale_ids(server_id, "tracks", "track_id", generation, TrackId::new)?;
        delta.albums.deleted =
            self.stale_ids(server_id, "albums", "album_id", generation, AlbumId::new)?;
        delta.artists.deleted =
            self.stale_ids(server_id, "artists", "artist_id", generation, ArtistId::new)?;
        delta.album_artists.deleted = self.stale_ids(
            server_id,
            "album_artists",
            "artist_id",
            generation,
            ArtistId::new,
        )?;
        delta.genres.deleted =
            self.stale_ids(server_id, "genres", "genre_id", generation, GenreId::new)?;
        delta.playlists.deleted = self.stale_ids(
            server_id,
            "playlists",
            "playlist_id",
            generation,
            PlaylistId::new,
        )?;
        Ok(delta)
    }

    fn stale_ids<Id>(
        &self,
        server_id: &ServerId,
        table: &str,
        column: &str,
        generation: i64,
        id: impl Fn(String) -> Id,
    ) -> StoreResult<Vec<Id>> {
        let sql = format!(
            "
            SELECT {column}
            FROM {table}
            WHERE server_id = ?1 AND sync_generation < ?2
            ORDER BY {column}
            "
        );
        let mut statement = self.connection.prepare(&sql)?;
        collect_rows(
            statement.query_map(params![server_id.as_str(), generation], |row| {
                row.get::<_, String>(0).map(&id)
            })?,
        )
    }
}

fn album_stats_changed(left: &Album, right: &Album) -> bool {
    let provider_counts_changed = (right.track_count > left.track_count
        && left.track_count != right.track_count)
        || (right.duration_seconds > left.duration_seconds
            && left.duration_seconds != right.duration_seconds);
    provider_counts_changed
        || left.play_count != right.play_count
        || left.last_played != right.last_played
        || left.user_rating != right.user_rating
}

fn album_links_changed(left: &Album, right: &Album) -> bool {
    left.artist_id != right.artist_id
        || artist_credits_changed(&left.album_artist_credits, &right.album_artist_credits)
        || artist_credits_changed(&left.artist_credits, &right.artist_credits)
        || left.genres != right.genres
}

fn album_fields_changed(left: &Album, right: &Album) -> bool {
    left.title != right.title
        || left.artist != right.artist
        || left.year != right.year
        || left.release_date != right.release_date
        || left.date_added != right.date_added
        || left.favorite != right.favorite
        || left.color_seed != right.color_seed
        || left.release_types != right.release_types
        || left.is_compilation != right.is_compilation
        || left.musicbrainz_album_id != right.musicbrainz_album_id
        || left.musicbrainz_release_group_id != right.musicbrainz_release_group_id
}

fn upsert_album_entity_data_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    album: &Album,
) -> StoreResult<()> {
    let existing_release_id = load_entity_identity_value_on_connection(
        connection,
        server_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release",
    )?;
    let existing_group_id = load_entity_grouping_value_on_connection(
        connection,
        server_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release_group",
    )?;
    let release_id = clean_identity_value(album.musicbrainz_album_id.as_deref());
    let group_id = clean_identity_value(album.musicbrainz_release_group_id.as_deref());
    let identity_changed =
        existing_release_id.as_deref() != release_id || existing_group_id.as_deref() != group_id;
    upsert_entity_on_connection(
        connection,
        server_id,
        "album",
        album.id.as_str(),
        "provider",
        None,
    )?;
    upsert_identity_key_on_connection(
        connection,
        server_id,
        "album",
        "source:album_id",
        album.id.as_str(),
        album.id.as_str(),
        "provider",
    )?;
    delete_identity_key_on_connection(
        connection,
        server_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release",
    )?;
    delete_grouping_key_on_connection(
        connection,
        server_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release_group",
    )?;
    if identity_changed {
        delete_resolved_album_metadata_facts_on_connection(
            connection,
            server_id,
            album.id.as_str(),
        )?;
    }
    if let Some(release_id) = release_id {
        upsert_identity_key_on_connection(
            connection,
            server_id,
            "album",
            "musicbrainz:release",
            release_id,
            album.id.as_str(),
            "provider",
        )?;
    }
    if let Some(group_id) = group_id {
        upsert_grouping_key_on_connection(
            connection,
            server_id,
            "album",
            "musicbrainz:release_group",
            group_id,
            album.id.as_str(),
            "provider",
        )?;
    }
    let release_types_json = album_release_types_json(&album.release_types)?;
    if release_types_json != "[]" {
        upsert_fact_on_connection(
            connection,
            server_id,
            "album",
            album.id.as_str(),
            "release_types",
            &release_types_json,
            "provider",
        )?;
    }
    if let Some(is_compilation) = album.is_compilation {
        upsert_fact_on_connection(
            connection,
            server_id,
            "album",
            album.id.as_str(),
            "is_compilation",
            if is_compilation { "true" } else { "false" },
            "provider",
        )?;
    }
    if let Some(content_key) = image_ref_content_key(album.image_ref.as_ref()) {
        upsert_content_ref_on_connection(
            connection,
            server_id,
            "album",
            album.id.as_str(),
            "cover",
            &content_key,
            "provider",
        )?;
    }
    Ok(())
}

fn load_entity_identity_value_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<Option<String>> {
    connection
        .query_row(
            "
            SELECT value
            FROM entity_identity_keys
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND namespace = ?4
            LIMIT 1
            ",
            params![server_id.as_str(), entity_kind, entity_id, namespace],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_entity_grouping_value_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<Option<String>> {
    connection
        .query_row(
            "
            SELECT value
            FROM entity_grouping_keys
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND namespace = ?4
            LIMIT 1
            ",
            params![server_id.as_str(), entity_kind, entity_id, namespace],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::from)
}

fn delete_identity_key_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM entity_identity_keys
        WHERE server_id = ?1
          AND entity_kind = ?2
          AND entity_id = ?3
          AND namespace = ?4
        ",
        params![server_id.as_str(), entity_kind, entity_id, namespace],
    )?;
    Ok(())
}

fn delete_grouping_key_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM entity_grouping_keys
        WHERE server_id = ?1
          AND entity_kind = ?2
          AND entity_id = ?3
          AND namespace = ?4
        ",
        params![server_id.as_str(), entity_kind, entity_id, namespace],
    )?;
    Ok(())
}

fn delete_resolved_album_metadata_facts_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    album_id: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM entity_facts
        WHERE server_id = ?1
          AND entity_kind = 'album'
          AND entity_id = ?2
          AND source = 'musicbrainz'
          AND fact_key IN ('release_types', 'is_compilation')
        ",
        params![server_id.as_str(), album_id],
    )?;
    Ok(())
}

pub(super) fn upsert_track_entity_data_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    track: &Track,
) -> StoreResult<()> {
    let source = if track.local_path.is_some() {
        "local"
    } else {
        "provider"
    };
    upsert_entity_on_connection(
        connection,
        server_id,
        "track",
        track.id.as_str(),
        source,
        None,
    )?;
    upsert_identity_key_on_connection(
        connection,
        server_id,
        "track",
        "source:track_id",
        track.id.as_str(),
        track.id.as_str(),
        source,
    )?;
    delete_identity_key_on_connection(
        connection,
        server_id,
        "track",
        track.id.as_str(),
        "local:path",
    )?;
    delete_identity_key_on_connection(
        connection,
        server_id,
        "track",
        track.id.as_str(),
        "musicbrainz:release_track",
    )?;
    delete_grouping_key_on_connection(
        connection,
        server_id,
        "track",
        track.id.as_str(),
        "musicbrainz:recording",
    )?;
    if let Some(path) = clean_identity_value(track.local_path.as_deref()) {
        upsert_identity_key_on_connection(
            connection,
            server_id,
            "track",
            "local:path",
            path,
            track.id.as_str(),
            "local",
        )?;
    }
    if let Some(recording_id) = clean_identity_value(track.musicbrainz_recording_id.as_deref()) {
        upsert_grouping_key_on_connection(
            connection,
            server_id,
            "track",
            "musicbrainz:recording",
            recording_id,
            track.id.as_str(),
            source,
        )?;
    }
    if let Some(track_id) = clean_identity_value(track.musicbrainz_release_track_id.as_deref()) {
        upsert_identity_key_on_connection(
            connection,
            server_id,
            "track",
            "musicbrainz:release_track",
            track_id,
            track.id.as_str(),
            source,
        )?;
    }
    if let Some(content_key) = image_ref_content_key(track.image_ref.as_ref()) {
        upsert_content_ref_on_connection(
            connection,
            server_id,
            "track",
            track.id.as_str(),
            "cover",
            &content_key,
            source,
        )?;
    }
    Ok(())
}

pub(super) fn upsert_artist_credit_entity_data_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    artist: &ArtistCredit,
) -> StoreResult<()> {
    upsert_artist_entity_keys_on_connection(
        connection,
        server_id,
        entity_kind,
        artist.id.as_str(),
        artist.musicbrainz_artist_id.as_deref(),
        false,
    )
}

fn upsert_artist_entity_data_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    artist: &Artist,
) -> StoreResult<()> {
    upsert_artist_entity_keys_on_connection(
        connection,
        server_id,
        entity_kind,
        artist.id.as_str(),
        artist.musicbrainz_artist_id.as_deref(),
        false,
    )
}

fn upsert_artist_entity_keys_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    artist_id: &str,
    musicbrainz_artist_id: Option<&str>,
    replace_musicbrainz_artist_id: bool,
) -> StoreResult<()> {
    upsert_entity_on_connection(
        connection,
        server_id,
        entity_kind,
        artist_id,
        "provider",
        None,
    )?;
    upsert_identity_key_on_connection(
        connection,
        server_id,
        entity_kind,
        "source:artist_id",
        artist_id,
        artist_id,
        "provider",
    )?;
    let artist_id_value = clean_identity_value(musicbrainz_artist_id);
    if replace_musicbrainz_artist_id || artist_id_value.is_some() {
        delete_identity_key_on_connection(
            connection,
            server_id,
            entity_kind,
            artist_id,
            "musicbrainz:artist",
        )?;
    }
    if let Some(artist_id_value) = artist_id_value {
        upsert_identity_key_on_connection(
            connection,
            server_id,
            entity_kind,
            "musicbrainz:artist",
            artist_id_value,
            artist_id,
            "provider",
        )?;
    }
    Ok(())
}

fn upsert_entity_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    source: &str,
    source_object_id: Option<&str>,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO entities (
            server_id, entity_kind, entity_id, source, source_object_id, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
        ON CONFLICT(server_id, entity_kind, entity_id) DO UPDATE SET
            source = excluded.source,
            source_object_id = COALESCE(excluded.source_object_id, entities.source_object_id),
            updated_at = excluded.updated_at
        ",
        params![
            server_id.as_str(),
            entity_kind,
            entity_id,
            source,
            source_object_id,
        ],
    )?;
    Ok(())
}

fn upsert_identity_key_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    namespace: &str,
    value: &str,
    entity_id: &str,
    source: &str,
) -> StoreResult<()> {
    let Some(value) = clean_identity_value(Some(value)) else {
        return Ok(());
    };
    connection.execute(
        "
        INSERT INTO entity_identity_keys (
            server_id, entity_kind, namespace, value, entity_id, source, strength, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 100, CURRENT_TIMESTAMP)
        ON CONFLICT(server_id, entity_kind, namespace, value) DO UPDATE SET
            entity_id = excluded.entity_id,
            source = excluded.source,
            strength = excluded.strength,
            updated_at = excluded.updated_at
        ",
        params![
            server_id.as_str(),
            entity_kind,
            namespace,
            value,
            entity_id,
            source,
        ],
    )?;
    Ok(())
}

fn upsert_grouping_key_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    namespace: &str,
    value: &str,
    entity_id: &str,
    source: &str,
) -> StoreResult<()> {
    let Some(value) = clean_identity_value(Some(value)) else {
        return Ok(());
    };
    connection.execute(
        "
        INSERT INTO entity_grouping_keys (
            server_id, entity_kind, namespace, value, entity_id, source, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
        ON CONFLICT(server_id, entity_kind, namespace, value, entity_id) DO UPDATE SET
            source = excluded.source,
            updated_at = excluded.updated_at
        ",
        params![
            server_id.as_str(),
            entity_kind,
            namespace,
            value,
            entity_id,
            source,
        ],
    )?;
    Ok(())
}

fn upsert_fact_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    fact_key: &str,
    value_json: &str,
    source: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO entity_facts (
            server_id, entity_kind, entity_id, fact_key, value_json, source, status, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'resolved', CURRENT_TIMESTAMP)
        ON CONFLICT(server_id, entity_kind, entity_id, fact_key, source) DO UPDATE SET
            value_json = excluded.value_json,
            status = excluded.status,
            updated_at = excluded.updated_at
        ",
        params![
            server_id.as_str(),
            entity_kind,
            entity_id,
            fact_key,
            value_json,
            source,
        ],
    )?;
    Ok(())
}

fn upsert_content_ref_on_connection(
    connection: &rusqlite::Connection,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    content_kind: &str,
    content_key: &str,
    source: &str,
) -> StoreResult<()> {
    let Some(content_key) = clean_identity_value(Some(content_key)) else {
        return Ok(());
    };
    connection.execute(
        "
        INSERT INTO entity_content_refs (
            server_id, entity_kind, entity_id, content_kind, content_key, source, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
        ON CONFLICT(server_id, entity_kind, entity_id, content_kind, source) DO UPDATE SET
            content_key = excluded.content_key,
            updated_at = excluded.updated_at
        ",
        params![
            server_id.as_str(),
            entity_kind,
            entity_id,
            content_kind,
            content_key,
            source,
        ],
    )?;
    Ok(())
}

fn image_ref_content_key(image_ref: Option<&ImageRef>) -> Option<String> {
    let image_ref = image_ref?;
    let item_id = clean_identity_value(Some(image_ref.item_id.as_str()))?;
    let tag = image_ref.tag.as_deref().unwrap_or("");
    Some(format!("{item_id}\u{1f}{tag}"))
}

fn clean_identity_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn track_changed(left: &Track, right: &Track) -> bool {
    track_fields_changed(left, right)
        || track_stats_changed(left, right)
        || left.album_id != right.album_id
        || track_artist_links_changed(left, right)
        || left.genres != right.genres
        || left.favorite != right.favorite
        || left.image_ref != right.image_ref
}

fn track_fields_changed(left: &Track, right: &Track) -> bool {
    left.title != right.title
        || left.artist != right.artist
        || left.album != right.album
        || left.year != right.year
        || left.release_date != right.release_date
        || left.date_added != right.date_added
        || left.duration_seconds != right.duration_seconds
        || left.disc_number != right.disc_number
        || left.track_number != right.track_number
        || left.local_path != right.local_path
        || left.source_format != right.source_format
        || left.musicbrainz_recording_id != right.musicbrainz_recording_id
        || left.musicbrainz_release_track_id != right.musicbrainz_release_track_id
        || left.comment != right.comment
}

fn track_stats_changed(left: &Track, right: &Track) -> bool {
    left.last_played != right.last_played
        || left.play_count != right.play_count
        || left.user_rating != right.user_rating
        || left.skip_count != right.skip_count
}

fn track_artist_links_changed(left: &Track, right: &Track) -> bool {
    left.artist_id != right.artist_id
        || (!right.artist_credits.is_empty()
            && artist_credits_changed(&left.artist_credits, &right.artist_credits))
}

fn artist_credits_changed(left: &[ArtistCredit], right: &[ArtistCredit]) -> bool {
    left.len() != right.len()
        || left.iter().zip(right.iter()).any(|(left, right)| {
            left.id != right.id
                || left.name != right.name
                || right
                    .musicbrainz_artist_id
                    .as_ref()
                    .is_some_and(|right_id| left.musicbrainz_artist_id.as_ref() != Some(right_id))
        })
}

fn artist_projection_changed(left: &Artist, right: &Artist) -> bool {
    left.id != right.id
        || left.name != right.name
        || left.album_count != right.album_count
        || left.track_count != right.track_count
        || left.favorite != right.favorite
        || left.last_played != right.last_played
        || left.play_count != right.play_count
        || left.user_rating != right.user_rating
        || left.image_ref != right.image_ref
}

fn home_keys(sections: &[HomeSection]) -> Vec<(HomeSectionKind, &'static str, String)> {
    sections
        .iter()
        .flat_map(|section| {
            section
                .albums
                .iter()
                .map(|album| (section.kind, "album", album.id.as_str().to_string()))
                .chain(
                    section
                        .tracks
                        .iter()
                        .map(|track| (section.kind, "track", track.id.as_str().to_string())),
                )
        })
        .collect()
}

fn refresh_genre_counts_on_connection(
    connection: &Connection,
    server_id: &ServerId,
) -> StoreResult<()> {
    connection.execute(
        "
        UPDATE genres
        SET album_count = CASE
                WHEN EXISTS (
                    SELECT 1
                    FROM album_genres
                    WHERE album_genres.server_id = genres.server_id
                      AND album_genres.genre_name = genres.name
                )
                OR EXISTS (
                    SELECT 1
                    FROM track_genres
                    WHERE track_genres.server_id = genres.server_id
                      AND track_genres.genre_name = genres.name
                )
                THEN (
                    SELECT COUNT(DISTINCT album_id)
                    FROM (
                        SELECT albums.album_id
                        FROM album_genres
                        JOIN albums
                            ON albums.server_id = album_genres.server_id
                           AND albums.album_id = album_genres.album_id
                        WHERE album_genres.server_id = genres.server_id
                          AND album_genres.genre_name = genres.name
                        UNION
                        SELECT albums.album_id
                        FROM track_genres
                        JOIN tracks
                            ON tracks.server_id = track_genres.server_id
                           AND tracks.track_id = track_genres.track_id
                        JOIN albums
                            ON albums.server_id = tracks.server_id
                           AND albums.album_id = tracks.album_id
                        WHERE track_genres.server_id = genres.server_id
                          AND track_genres.genre_name = genres.name
                    ) linked_albums
                )
                ELSE album_count
            END,
            track_count = CASE
                WHEN EXISTS (
                    SELECT 1
                    FROM album_genres
                    WHERE album_genres.server_id = genres.server_id
                      AND album_genres.genre_name = genres.name
                )
                OR EXISTS (
                    SELECT 1
                    FROM track_genres
                    WHERE track_genres.server_id = genres.server_id
                      AND track_genres.genre_name = genres.name
                )
                THEN (
                    SELECT COUNT(DISTINCT track_id)
                    FROM track_genres
                    WHERE track_genres.server_id = genres.server_id
                      AND track_genres.genre_name = genres.name
                )
                ELSE track_count
            END
        WHERE server_id = ?1
        ",
        params![server_id.as_str()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::test_support::{album, track};
    use super::*;

    #[test]
    fn track_activity_changes_are_stats() {
        let album = album(1);
        let original = track(1, &album);
        let mut played = original.clone();
        played.play_count = Some(1);
        played.last_played = Some("2026-06-08T12:00:00Z".to_string());
        played.skip_count = Some(1);

        assert!(track_stats_changed(&original, &played));
        assert!(!track_fields_changed(&original, &played));

        let mut renamed = original.clone();
        renamed.title = "Updated title".to_string();
        assert!(track_fields_changed(&original, &renamed));
    }
}
