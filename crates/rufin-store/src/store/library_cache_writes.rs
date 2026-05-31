use super::servers::*;
use super::*;

impl Store {
    pub fn complete_sync(&self, server_id: &ServerId, generation: i64) -> StoreResult<()> {
        self.prune_missing_items(server_id, generation)?;
        self.refresh_collection_cover_refs(server_id)?;
        self.refresh_smart_playlist_cover_refs(server_id)?;
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
        Ok(())
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
                replace_collection_cover_refs_on_connection(
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
                replace_collection_cover_refs_on_connection(
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
            Self::insert_home_section_items_for_table(
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
        Self::insert_home_section_items_for_table(
            connection,
            "home_section_items",
            server_id,
            section,
            generation,
        )
    }
    pub(super) fn insert_home_section_items_for_table(
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
