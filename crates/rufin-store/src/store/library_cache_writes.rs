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
            match self.load_album_for_delta(server_id, &album.id)? {
                Some(existing) if existing == *album => {}
                Some(existing) => {
                    if album_stats_changed(&existing, album) {
                        delta.albums.stats.push(album.id.clone());
                    }
                    if album_links_changed(&existing, album) {
                        delta.albums.links.push(album.id.clone());
                    }
                    if existing.image_ref != album.image_ref {
                        delta.albums.cover_refs.push(album.id.clone());
                    }
                    if album_fields_changed(&existing, album) {
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
        for artist in artists {
            match self.load_artist_for_delta(server_id, &artist.id, album_artist)? {
                Some(existing) if existing == *artist => {}
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
                    if existing != *artist {
                        entity.fields.push(artist.id.clone());
                    }
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
                    if existing.track_count != playlist.track_count
                        || existing.duration_seconds != playlist.duration_seconds
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
            let mut statement = connection.prepare(
                "
                INSERT INTO albums (
                    server_id, album_id, title, artist, artist_id, year, release_date,
                    date_added, last_played, play_count, user_rating, track_count,
                    duration_seconds, favorite, color_seed, image_item_id, image_tag,
                    sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
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

            for album in albums {
                let (image_item_id, image_tag) = image_ref_parts(album.image_ref.as_ref());
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
                    generation,
                ])?;
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

            for artist in artists {
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
                    track_count = excluded.track_count,
                    duration_seconds = excluded.duration_seconds,
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
                       favorite, color_seed, image_item_id, image_tag
                FROM albums
                WHERE server_id = ?1 AND album_id = ?2
                ",
                params![server_id.as_str(), album_id.as_str()],
                album_from_row,
            )
            .optional()?;
        if let Some(album) = album.as_mut() {
            self.attach_album_genres(server_id, std::slice::from_mut(album))?;
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

    fn load_playlist_for_delta(
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
        || left.album_artist_credits != right.album_artist_credits
        || left.artist_credits != right.artist_credits
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
}

fn track_changed(left: &Track, right: &Track) -> bool {
    track_fields_changed(left, right)
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
        || left.last_played != right.last_played
        || left.play_count != right.play_count
        || left.user_rating != right.user_rating
        || left.duration_seconds != right.duration_seconds
        || left.disc_number != right.disc_number
        || left.track_number != right.track_number
        || left.local_path != right.local_path
        || left.source_format != right.source_format
        || left.comment != right.comment
        || left.skip_count != right.skip_count
}

fn track_artist_links_changed(left: &Track, right: &Track) -> bool {
    left.artist_id != right.artist_id
        || (!right.artist_credits.is_empty() && left.artist_credits != right.artist_credits)
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
