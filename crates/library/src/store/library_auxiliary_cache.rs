use super::sources::*;
use super::*;

impl Store {
    pub fn load_track_genre_names(&self, source_id: &SourceId) -> StoreResult<Vec<String>> {
        let mut statement = self.connection.prepare(
            "
            SELECT DISTINCT genre_name
            FROM track_genres
            WHERE source_id = ?1
              AND TRIM(genre_name) != ''
            ORDER BY genre_name COLLATE NOCASE
            ",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str()], |row| row.get::<_, String>(0))?,
        )
    }

    pub fn load_track_mood_names(&self, source_id: &SourceId) -> StoreResult<Vec<String>> {
        let mut statement = self.connection.prepare(
            "
            SELECT DISTINCT mood_name
            FROM track_moods
            WHERE source_id = ?1
              AND TRIM(mood_name) != ''
            ORDER BY mood_name COLLATE NOCASE
            ",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str()], |row| row.get::<_, String>(0))?,
        )
    }

    pub fn load_genres(
        &self,
        source_id: &SourceId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Genre>> {
        let total = self.count_linked_genres(source_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT genre_id, name, album_count, track_count, duration_seconds,
                   image_item_id, image_tag
            FROM genres g
            WHERE g.source_id = ?1
              AND (
                  EXISTS (
                      SELECT 1
                      FROM album_genres ag
                      WHERE ag.source_id = g.source_id AND ag.genre_name = g.name
                  )
                  OR EXISTS (
                      SELECT 1
                      FROM track_genres tg
                      WHERE tg.source_id = g.source_id AND tg.genre_name = g.name
                  )
              )
            ORDER BY name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![source_id.as_str(), limit as i64, offset as i64],
            genre_from_row,
        )?)?;
        self.attach_genre_cover_image_refs(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }
    pub fn load_genres_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Genre>> {
        let Some(pattern) = like_pattern(query) else {
            return self.load_genres(source_id, offset, limit);
        };
        let total = self.count_linked_genres_like(source_id, &pattern)?;
        let mut statement = self.connection.prepare(
            "
            SELECT genre_id, name, album_count, track_count, duration_seconds,
                   image_item_id, image_tag
            FROM genres g
            WHERE g.source_id = ?1
              AND LOWER(g.name) LIKE ?2 ESCAPE '\\'
              AND (
                  EXISTS (
                      SELECT 1
                      FROM album_genres ag
                      WHERE ag.source_id = g.source_id AND ag.genre_name = g.name
                  )
                  OR EXISTS (
                      SELECT 1
                      FROM track_genres tg
                      WHERE tg.source_id = g.source_id AND tg.genre_name = g.name
                  )
              )
            ORDER BY name COLLATE NOCASE
            LIMIT ?3 OFFSET ?4
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![source_id.as_str(), pattern, limit as i64, offset as i64],
            genre_from_row,
        )?)?;
        self.attach_genre_cover_image_refs(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_moods(
        &self,
        source_id: &SourceId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Mood>> {
        let total = self.count_moods(source_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT tm.mood_name, tm.mood_name,
                   COUNT(DISTINCT tm.track_id),
                   COALESCE(SUM(t.duration_seconds), 0),
                   NULL, NULL
            FROM track_moods tm
            JOIN tracks t
                ON t.source_id = tm.source_id AND t.track_id = tm.track_id
            WHERE tm.source_id = ?1
              AND TRIM(tm.mood_name) != ''
            GROUP BY tm.mood_name
            ORDER BY tm.mood_name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![source_id.as_str(), limit as i64, offset as i64],
            mood_from_row,
        )?)?;
        self.attach_mood_cover_image_refs(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_moods_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Mood>> {
        let Some(pattern) = like_pattern(query) else {
            return self.load_moods(source_id, offset, limit);
        };
        let total = self.count_moods_like(source_id, &pattern)?;
        let mut statement = self.connection.prepare(
            "
            SELECT tm.mood_name, tm.mood_name,
                   COUNT(DISTINCT tm.track_id),
                   COALESCE(SUM(t.duration_seconds), 0),
                   NULL, NULL
            FROM track_moods tm
            JOIN tracks t
                ON t.source_id = tm.source_id AND t.track_id = tm.track_id
            WHERE tm.source_id = ?1
              AND TRIM(tm.mood_name) != ''
              AND LOWER(tm.mood_name) LIKE ?2 ESCAPE '\\'
            GROUP BY tm.mood_name
            ORDER BY tm.mood_name COLLATE NOCASE
            LIMIT ?3 OFFSET ?4
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![source_id.as_str(), pattern, limit as i64, offset as i64],
            mood_from_row,
        )?)?;
        self.attach_mood_cover_image_refs(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_tracks_by_genre_name(
        &self,
        source_id: &SourceId,
        genre_name: &str,
        limit: usize,
    ) -> StoreResult<Vec<Track>> {
        let sql = format!(
            "
            SELECT DISTINCT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.release_date, t.date_added, t.last_played,
                   t.play_count, t.user_rating, t.duration_seconds, {favorite} AS favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag,
                   t.local_path, t.source_format
            FROM track_genres tg
            JOIN tracks t
                ON t.source_id = tg.source_id AND t.track_id = tg.track_id
            WHERE tg.source_id = ?1 AND tg.genre_name = ?2
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                     t.title COLLATE NOCASE
            LIMIT ?3
            ",
            favorite = effective_track_favorite_sql("t"),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut tracks = collect_rows(statement.query_map(
            params![source_id.as_str(), genre_name, limit as i64],
            track_from_row,
        )?)?;
        self.attach_track_metadata(source_id, &mut tracks)?;
        Ok(tracks)
    }
    pub(super) fn count_linked_genres(&self, source_id: &SourceId) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM genres g
                WHERE g.source_id = ?1
                  AND (
                      EXISTS (
                          SELECT 1
                          FROM album_genres ag
                          WHERE ag.source_id = g.source_id AND ag.genre_name = g.name
                      )
                      OR EXISTS (
                          SELECT 1
                          FROM track_genres tg
                          WHERE tg.source_id = g.source_id AND tg.genre_name = g.name
                      )
                  )
                ",
                params![source_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map(u32_from_i64)
            .map(|count| count as usize)
            .map_err(StoreError::from)
    }
    pub(super) fn count_linked_genres_like(
        &self,
        source_id: &SourceId,
        pattern: &str,
    ) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM genres g
                WHERE g.source_id = ?1
                  AND LOWER(g.name) LIKE ?2 ESCAPE '\\'
                  AND (
                      EXISTS (
                          SELECT 1
                          FROM album_genres ag
                          WHERE ag.source_id = g.source_id AND ag.genre_name = g.name
                      )
                      OR EXISTS (
                          SELECT 1
                          FROM track_genres tg
                          WHERE tg.source_id = g.source_id AND tg.genre_name = g.name
                      )
                  )
                ",
                params![source_id.as_str(), pattern],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as usize)
            .map_err(StoreError::from)
    }

    fn count_moods(&self, source_id: &SourceId) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM (
                    SELECT 1
                    FROM track_moods
                    WHERE source_id = ?1
                      AND TRIM(mood_name) != ''
                    GROUP BY mood_name
                )
                ",
                params![source_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as usize)
            .map_err(StoreError::from)
    }

    fn count_moods_like(&self, source_id: &SourceId, pattern: &str) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM (
                    SELECT 1
                    FROM track_moods
                    WHERE source_id = ?1
                      AND TRIM(mood_name) != ''
                      AND LOWER(mood_name) LIKE ?2 ESCAPE '\\'
                    GROUP BY mood_name
                )
                ",
                params![source_id.as_str(), pattern],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as usize)
            .map_err(StoreError::from)
    }
    pub fn load_playlists(
        &self,
        source_id: &SourceId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Playlist>> {
        let total = self.count("playlists", source_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT playlist_id, name, track_count, duration_seconds, top_genres_json,
                   owner, image_item_id, image_tag
            FROM playlists
            WHERE source_id = ?1
            ORDER BY name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![source_id.as_str(), limit as i64, offset as i64],
            playlist_from_row,
        )?)?;
        self.attach_playlist_cover_image_refs(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }
    pub fn load_playlists_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Playlist>> {
        let Some(pattern) = like_pattern(query) else {
            return self.load_playlists(source_id, offset, limit);
        };
        if let Some(query) = fts_query(query) {
            let total = self.count_fts_matches(source_id, "playlist", &query)?;
            if total > 0 {
                return self.search_playlists_page(source_id, &query, offset, limit, total);
            }
        }
        self.load_playlists_like(source_id, &pattern, offset, limit)
    }
    pub fn upsert_playlist_tracks(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        let entries = tracks
            .iter()
            .enumerate()
            .map(|(position, track)| PlaylistEntry {
                entry_id: format!("{}:{position}", track.id.as_str()),
                track: track.clone(),
            })
            .collect::<Vec<_>>();
        self.upsert_playlist_entries(source_id, playlist_id, &entries, generation)
    }
    pub fn upsert_playlist_entries(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        entries: &[PlaylistEntry],
        generation: i64,
    ) -> StoreResult<()> {
        self.upsert_playlist_entries_with_mode(
            source_id,
            playlist_id,
            entries,
            PlaylistWriteMode::NativeSync { generation },
        )
    }

    pub fn upsert_playlist_entries_with_mode(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        entries: &[PlaylistEntry],
        mode: PlaylistWriteMode,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let owner = mode.owner();
            let generation = mode.sync_generation();
            ensure_playlist_owner(connection, source_id, playlist_id, owner)?;
            connection.execute(
                "DELETE FROM playlist_tracks WHERE source_id = ?1 AND playlist_id = ?2",
                params![source_id.as_str(), playlist_id.as_str()],
            )?;
            let mut statement = connection.prepare(
                "
                INSERT INTO playlist_tracks (
                    source_id, playlist_id, entry_id, track_id, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(source_id, playlist_id, entry_id) DO UPDATE SET
                    track_id = excluded.track_id,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for (position, entry) in entries.iter().enumerate() {
                statement.execute(params![
                    source_id.as_str(),
                    playlist_id.as_str(),
                    entry.entry_id,
                    entry.track.id.as_str(),
                    position as i64,
                    generation,
                ])?;
            }
            refresh_playlist_stats(connection, source_id, playlist_id)?;
            refresh_playlist_refs(connection, source_id, playlist_id)?;
            Ok(())
        })
    }

    pub fn upsert_playlist_entries_delta(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        entries: &[PlaylistEntry],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let before_playlist = self.load_playlist_for_delta(source_id, playlist_id)?;
        let before = self.playlist_entry_keys(source_id, playlist_id)?;
        self.upsert_playlist_entries(source_id, playlist_id, entries, generation)?;
        let after = self.playlist_entry_keys(source_id, playlist_id)?;
        let after_playlist = self.load_playlist_for_delta(source_id, playlist_id)?;
        let changed = before != after || playlist_stats_changed(before_playlist, after_playlist);
        Ok(LibraryDelta {
            playlists: PlaylistDelta {
                entries: changed.then(|| playlist_id.clone()).into_iter().collect(),
                cover_refs: changed.then(|| playlist_id.clone()).into_iter().collect(),
                ..PlaylistDelta::default()
            },
            ..LibraryDelta::default()
        })
    }

    pub fn playlist_entry_keys(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<Vec<(String, TrackId)>> {
        let mut statement = self.connection.prepare(
            "
            SELECT entry_id, track_id
            FROM playlist_tracks
            WHERE source_id = ?1 AND playlist_id = ?2
            ORDER BY position
            ",
        )?;
        collect_rows(statement.query_map(
            params![source_id.as_str(), playlist_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    TrackId::new(row.get::<_, String>(1)?),
                ))
            },
        )?)
    }

    pub fn playlist_entry_keys_for_playlists(
        &self,
        source_id: &SourceId,
        playlist_ids: &[PlaylistId],
    ) -> StoreResult<HashMap<PlaylistId, Vec<(String, TrackId)>>> {
        if playlist_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = std::iter::repeat_n("?", playlist_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "
            SELECT playlist_id, entry_id, track_id
            FROM playlist_tracks
            WHERE source_id = ?1
              AND playlist_id IN ({placeholders})
            ORDER BY playlist_id, position
            "
        );
        let mut parameters = Vec::with_capacity(playlist_ids.len() + 1);
        parameters.push(source_id.as_str().to_string());
        parameters.extend(
            playlist_ids
                .iter()
                .map(|playlist_id| playlist_id.as_str().to_string()),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut keys = HashMap::<PlaylistId, Vec<(String, TrackId)>>::new();
        for row in statement.query_map(params_from_iter(parameters.iter()), |row| {
            Ok((
                PlaylistId::new(row.get::<_, String>(0)?),
                row.get::<_, String>(1)?,
                TrackId::new(row.get::<_, String>(2)?),
            ))
        })? {
            let (playlist_id, entry_id, track_id) = row?;
            keys.entry(playlist_id)
                .or_default()
                .push((entry_id, track_id));
        }
        Ok(keys)
    }

    pub fn playlist_owner(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<Option<SourceFeatureOwner>> {
        playlist_owner_on_connection(&self.connection, source_id, playlist_id)
    }

    pub fn load_playlist_detail(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<Option<PlaylistDetail>> {
        let playlist = self
            .connection
            .query_row(
                "
                SELECT playlist_id, name, track_count, duration_seconds, top_genres_json,
                       owner, image_item_id, image_tag
                FROM playlists
                WHERE source_id = ?1 AND playlist_id = ?2
                ",
                params![source_id.as_str(), playlist_id.as_str()],
                playlist_from_row,
            )
            .optional()?;
        let Some(mut playlist) = playlist else {
            return Ok(None);
        };
        let sql = format!(
            "
            SELECT pt.entry_id,
                   t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.release_date, t.date_added, t.last_played,
                   t.play_count, t.user_rating, t.duration_seconds, {favorite} AS favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM playlist_tracks pt
            JOIN tracks t
                ON t.source_id = pt.source_id AND t.track_id = pt.track_id
            WHERE pt.source_id = ?1 AND pt.playlist_id = ?2
            ORDER BY pt.position
            ",
            favorite = effective_track_favorite_sql("t"),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut entries = collect_rows(statement.query_map(
            params![source_id.as_str(), playlist_id.as_str()],
            playlist_entry_from_row,
        )?)?;
        let mut tracks = entries
            .iter()
            .map(|entry| entry.track.clone())
            .collect::<Vec<_>>();
        self.attach_track_metadata(source_id, &mut tracks)?;
        for (entry, track) in entries.iter_mut().zip(tracks.iter().cloned()) {
            entry.track = track;
        }
        playlist.image_refs = self.load_collection_cover_refs(
            source_id,
            COLLECTION_COVER_PLAYLIST,
            playlist.id.as_str(),
        )?;
        if playlist.image_ref.is_none() {
            playlist.image_ref = playlist.image_refs.first().cloned();
        }
        Ok(Some(PlaylistDetail {
            playlist,
            tracks,
            entries,
        }))
    }
    pub fn load_genre_detail(
        &self,
        source_id: &SourceId,
        genre_id: &GenreId,
    ) -> StoreResult<Option<CachedGenreDetail>> {
        let genre = self
            .connection
            .query_row(
                "
                SELECT genre_id, name,
                       album_count, track_count, duration_seconds,
                       image_item_id, image_tag
                FROM genres
                WHERE source_id = ?1 AND genre_id = ?2
                ",
                params![source_id.as_str(), genre_id.as_str()],
                genre_from_row,
            )
            .optional()?;
        let Some(mut genre) = genre else {
            return Ok(None);
        };
        let sql = format!(
            "
            SELECT DISTINCT a.album_id, a.title, a.artist, a.artist_id, a.year,
                   a.release_date, a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite, a.color_seed,
                   a.image_item_id, a.image_tag
            FROM albums a
            WHERE a.source_id = ?1
              AND (
                  EXISTS (
                      SELECT 1
                      FROM album_genres ag
                      WHERE ag.source_id = a.source_id
                        AND ag.album_id = a.album_id
                        AND ag.genre_name = ?2
                  )
                  OR EXISTS (
                      SELECT 1
                      FROM track_genres tg
                      JOIN tracks t
                          ON t.source_id = tg.source_id AND t.track_id = tg.track_id
                      WHERE tg.source_id = a.source_id
                        AND t.album_id = a.album_id
                        AND tg.genre_name = ?2
                  )
              )
            ORDER BY a.title COLLATE NOCASE
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut albums_statement = self.connection.prepare(&sql)?;
        let mut albums = collect_rows(albums_statement.query_map(
            params![source_id.as_str(), genre.name.as_str()],
            album_from_row,
        )?)?;
        self.attach_album_metadata(source_id, &mut albums)?;
        let sql = format!(
            "
            SELECT DISTINCT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.release_date, t.date_added, t.last_played,
                   t.play_count, t.user_rating, t.duration_seconds, {favorite} AS favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM track_genres tg
            JOIN tracks t
                ON t.source_id = tg.source_id AND t.track_id = tg.track_id
            WHERE tg.source_id = ?1 AND tg.genre_name = ?2
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                     t.title COLLATE NOCASE
            ",
            favorite = effective_track_favorite_sql("t"),
        );
        let mut tracks_statement = self.connection.prepare(&sql)?;
        let mut tracks = collect_rows(tracks_statement.query_map(
            params![source_id.as_str(), genre.name.as_str()],
            track_from_row,
        )?)?;
        self.attach_track_metadata(source_id, &mut tracks)?;
        genre.image_refs =
            self.load_collection_cover_refs(source_id, COLLECTION_COVER_GENRE, genre.id.as_str())?;
        if genre.image_ref.is_none() {
            genre.image_ref = genre.image_refs.first().cloned();
        }
        Ok(Some(CachedGenreDetail {
            genre,
            albums,
            tracks,
        }))
    }

    pub fn load_mood_detail(
        &self,
        source_id: &SourceId,
        mood_id: &MoodId,
    ) -> StoreResult<Option<CachedMoodDetail>> {
        let mood = self
            .connection
            .query_row(
                "
                SELECT tm.mood_name,
                       COUNT(DISTINCT tm.track_id),
                       COALESCE(SUM(t.duration_seconds), 0)
                FROM track_moods tm
                JOIN tracks t
                    ON t.source_id = tm.source_id AND t.track_id = tm.track_id
                WHERE tm.source_id = ?1
                  AND tm.mood_name = ?2
                  AND TRIM(tm.mood_name) != ''
                GROUP BY tm.mood_name
                ",
                params![source_id.as_str(), mood_id.as_str()],
                |row| {
                    Ok(Mood {
                        id: mood_id.clone(),
                        name: row.get(0)?,
                        track_count: u32_from_i64(row.get(1)?),
                        duration_seconds: u32_from_i64(row.get(2)?),
                        image_refs: Vec::new(),
                        image_ref: None,
                    })
                },
            )
            .optional()?;
        let Some(mut mood) = mood else {
            return Ok(None);
        };
        let sql = format!(
            "
            SELECT DISTINCT a.album_id, a.title, a.artist, a.artist_id, a.year,
                   a.release_date, a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite, a.color_seed,
                   a.image_item_id, a.image_tag
            FROM albums a
            JOIN tracks t
                ON t.source_id = a.source_id AND t.album_id = a.album_id
            JOIN track_moods tm
                ON tm.source_id = t.source_id AND tm.track_id = t.track_id
            WHERE a.source_id = ?1
              AND tm.mood_name = ?2
            ORDER BY a.title COLLATE NOCASE
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut albums_statement = self.connection.prepare(&sql)?;
        let mut albums = collect_rows(albums_statement.query_map(
            params![source_id.as_str(), mood.name.as_str()],
            album_from_row,
        )?)?;
        self.attach_album_metadata(source_id, &mut albums)?;
        let sql = format!(
            "
            SELECT DISTINCT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.release_date, t.date_added, t.last_played,
                   t.play_count, t.user_rating, t.duration_seconds, {favorite} AS favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM track_moods tm
            JOIN tracks t
                ON t.source_id = tm.source_id AND t.track_id = tm.track_id
            WHERE tm.source_id = ?1 AND tm.mood_name = ?2
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                     t.title COLLATE NOCASE
            ",
            favorite = effective_track_favorite_sql("t"),
        );
        let mut tracks_statement = self.connection.prepare(&sql)?;
        let mut tracks = collect_rows(tracks_statement.query_map(
            params![source_id.as_str(), mood.name.as_str()],
            track_from_row,
        )?)?;
        self.attach_track_metadata(source_id, &mut tracks)?;
        mood.image_refs = self.load_mood_cover_refs(source_id, &mood.name)?;
        if mood.image_ref.is_none() {
            mood.image_ref = mood.image_refs.first().cloned();
        }
        Ok(Some(CachedMoodDetail {
            mood,
            albums,
            tracks,
        }))
    }

    fn attach_mood_cover_image_refs(
        &self,
        source_id: &SourceId,
        moods: &mut [Mood],
    ) -> StoreResult<()> {
        for mood in moods {
            mood.image_refs = self.load_mood_cover_refs(source_id, &mood.name)?;
            if mood.image_ref.is_none() {
                mood.image_ref = mood.image_refs.first().cloned();
            }
        }
        Ok(())
    }

    fn load_mood_cover_refs(
        &self,
        source_id: &SourceId,
        mood_name: &str,
    ) -> StoreResult<Vec<ImageRef>> {
        let mut statement = self.connection.prepare(
            "
            SELECT DISTINCT t.image_item_id, t.image_tag
            FROM track_moods tm
            JOIN tracks t
                ON t.source_id = tm.source_id AND t.track_id = tm.track_id
            WHERE tm.source_id = ?1
              AND tm.mood_name = ?2
              AND t.image_item_id IS NOT NULL
              AND TRIM(t.image_item_id) != ''
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                     t.title COLLATE NOCASE
            LIMIT 4
            ",
        )?;
        let refs = collect_rows(
            statement.query_map(params![source_id.as_str(), mood_name], |row| {
                image_ref_from_row(row, 0, 1)
            })?,
        )?;
        Ok(refs.into_iter().flatten().collect())
    }
    pub fn load_favorite_tracks(&self, source_id: &SourceId) -> StoreResult<Vec<Track>> {
        let selected_folder = self.selected_music_folder_id(source_id)?;
        let mut tracks = if let Some(folder_id) = selected_folder.as_ref() {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number,
                       t.track_number, t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.source_id = ?1
                  AND {favorite} = 1
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.source_id = t.source_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?2
                )
                ORDER BY t.title COLLATE NOCASE
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![source_id.as_str(), folder_id.as_str()],
                track_from_row,
            )?)?
        } else {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number,
                       t.track_number, t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.source_id = ?1 AND {favorite} = 1
                ORDER BY title COLLATE NOCASE
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(params![source_id.as_str()], track_from_row)?)?
        };
        self.attach_track_metadata(source_id, &mut tracks)?;
        Ok(tracks)
    }
    pub fn set_album_favorite(
        &self,
        source_id: &SourceId,
        album_id: &AlbumId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.connection.execute(
            "UPDATE albums SET favorite = ?3 WHERE source_id = ?1 AND album_id = ?2",
            params![source_id.as_str(), album_id.as_str(), bool_to_i64(favorite)],
        )?;
        Ok(())
    }
    pub fn set_album_favorite_for_owner(
        &self,
        source_id: &SourceId,
        album_id: &AlbumId,
        favorite: bool,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            Self::set_favorite_for_owner(
                connection,
                source_id,
                "album",
                album_id.as_str(),
                favorite,
                owner,
            )
        })
    }
    pub fn set_track_favorite(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.connection.execute(
            "UPDATE tracks SET favorite = ?3 WHERE source_id = ?1 AND track_id = ?2",
            params![source_id.as_str(), track_id.as_str(), bool_to_i64(favorite)],
        )?;
        Ok(())
    }
    pub fn set_track_favorite_for_owner(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        favorite: bool,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            Self::set_favorite_for_owner(
                connection,
                source_id,
                "track",
                track_id.as_str(),
                favorite,
                owner,
            )
        })
    }
    pub fn set_artist_favorite(
        &self,
        source_id: &SourceId,
        artist_id: &ArtistId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.connection.execute(
            "UPDATE artists SET favorite = ?3 WHERE source_id = ?1 AND artist_id = ?2",
            params![
                source_id.as_str(),
                artist_id.as_str(),
                bool_to_i64(favorite)
            ],
        )?;
        self.connection.execute(
            "UPDATE album_artists SET favorite = ?3 WHERE source_id = ?1 AND artist_id = ?2",
            params![
                source_id.as_str(),
                artist_id.as_str(),
                bool_to_i64(favorite)
            ],
        )?;
        Ok(())
    }
    pub fn set_artist_favorite_for_owner(
        &self,
        source_id: &SourceId,
        artist_id: &ArtistId,
        favorite: bool,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            Self::set_favorite_for_owner(
                connection,
                source_id,
                "artist",
                artist_id.as_str(),
                favorite,
                owner,
            )?;
            Self::set_favorite_for_owner(
                connection,
                source_id,
                "album_artist",
                artist_id.as_str(),
                favorite,
                owner,
            )
        })
    }
    fn set_favorite_for_owner(
        connection: &Connection,
        source_id: &SourceId,
        kind: &str,
        item_id: &str,
        favorite: bool,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        match owner {
            SourceFeatureOwner::Native => {
                Self::delete_favorite_override(connection, source_id, kind, item_id)?;
                Self::write_favorite_column(connection, source_id, kind, item_id, favorite)
            }
            SourceFeatureOwner::Store => {
                Self::upsert_favorite_override(connection, source_id, kind, item_id, favorite)?;
                Self::write_favorite_column(connection, source_id, kind, item_id, favorite)
            }
        }
    }
    fn upsert_favorite_override(
        connection: &Connection,
        source_id: &SourceId,
        kind: &str,
        item_id: &str,
        favorite: bool,
    ) -> StoreResult<()> {
        let _validated = favorite_item_kind_to_table(kind)?;
        connection.execute(
            "
            INSERT INTO item_favorite_overrides (
                source_id, item_kind, item_id, favorite, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
            ON CONFLICT(source_id, item_kind, item_id) DO UPDATE SET
                favorite = excluded.favorite,
                updated_at = CURRENT_TIMESTAMP
            ",
            params![source_id.as_str(), kind, item_id, bool_to_i64(favorite)],
        )?;
        Ok(())
    }
    fn delete_favorite_override(
        connection: &Connection,
        source_id: &SourceId,
        kind: &str,
        item_id: &str,
    ) -> StoreResult<()> {
        let _validated = favorite_item_kind_to_table(kind)?;
        connection.execute(
            "
            DELETE FROM item_favorite_overrides
            WHERE source_id = ?1 AND item_kind = ?2 AND item_id = ?3
            ",
            params![source_id.as_str(), kind, item_id],
        )?;
        Ok(())
    }
    fn write_favorite_column(
        connection: &Connection,
        source_id: &SourceId,
        kind: &str,
        item_id: &str,
        favorite: bool,
    ) -> StoreResult<()> {
        let (table, id_column) = favorite_item_kind_to_table(kind)?;
        connection.execute(
            &format!(
                "
                UPDATE {table}
                SET favorite = ?3
                WHERE source_id = ?1 AND {id_column} = ?2
                "
            ),
            params![source_id.as_str(), item_id, bool_to_i64(favorite)],
        )?;
        Ok(())
    }
    pub fn rename_playlist(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        name: &str,
    ) -> StoreResult<()> {
        self.rename_playlist_with_owner(source_id, playlist_id, name, SourceFeatureOwner::Native)
    }

    pub fn rename_playlist_with_owner(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        name: &str,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        let changed = self.connection.execute(
            "UPDATE playlists SET name = ?3 WHERE source_id = ?1 AND playlist_id = ?2 AND owner = ?4",
            params![
                source_id.as_str(),
                playlist_id.as_str(),
                name,
                playlist_owner_to_str(owner),
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::InvalidPlaylistOwner(format!(
                "playlist {} is not owned by {}",
                playlist_id.as_str(),
                playlist_owner_to_str(owner)
            )));
        }
        self.connection.execute(
            "DELETE FROM library_fts WHERE source_id = ?1 AND item_type = 'playlist' AND item_id = ?2",
            params![source_id.as_str(), playlist_id.as_str()],
        )?;
        self.connection.execute(
            "INSERT INTO library_fts (source_id, item_type, item_id, title, subtitle)
             VALUES (?1, 'playlist', ?2, ?3, '')",
            params![source_id.as_str(), playlist_id.as_str(), name],
        )?;
        Ok(())
    }

    pub fn delete_playlist(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<()> {
        self.delete_playlist_with_owner(source_id, playlist_id, SourceFeatureOwner::Native)
    }

    pub fn delete_playlist_with_owner(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        ensure_playlist_owner(&self.connection, source_id, playlist_id, owner)?;
        self.connection.execute(
            "DELETE FROM playlist_tracks WHERE source_id = ?1 AND playlist_id = ?2",
            params![source_id.as_str(), playlist_id.as_str()],
        )?;
        self.connection.execute(
            "DELETE FROM playlists WHERE source_id = ?1 AND playlist_id = ?2 AND owner = ?3",
            params![
                source_id.as_str(),
                playlist_id.as_str(),
                playlist_owner_to_str(owner),
            ],
        )?;
        self.connection.execute(
            "DELETE FROM library_fts WHERE source_id = ?1 AND item_type = 'playlist' AND item_id = ?2",
            params![source_id.as_str(), playlist_id.as_str()],
        )?;
        Ok(())
    }
    pub fn save_lyrics(&self, source_id: &SourceId, lyrics: &Lyrics) -> StoreResult<()> {
        let value = serde_json::to_string(lyrics)?;
        let source = match lyrics.source {
            LyricsSource::Server => "server",
            LyricsSource::Remote => "remote",
            LyricsSource::Local => "local",
        };
        self.connection.execute(
            "
            INSERT INTO lyrics_cache (source_id, track_id, source, value, updated_at)
            VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
            ON CONFLICT(source_id, track_id) DO UPDATE SET
                source = excluded.source,
                value = excluded.value,
                updated_at = excluded.updated_at
            ",
            params![source_id.as_str(), lyrics.track_id.as_str(), source, value],
        )?;
        Ok(())
    }
    pub fn load_lyrics(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<Option<Lyrics>> {
        let value = self
            .connection
            .query_row(
                "
                SELECT value
                FROM lyrics_cache
                WHERE source_id = ?1 AND track_id = ?2
                ",
                params![source_id.as_str(), track_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value
            .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
            .unwrap_or_else(|| Ok(None))
    }
    pub fn delete_remote_lyrics(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<bool> {
        let deleted = self.connection.execute(
            "
            DELETE FROM lyrics_cache
            WHERE source_id = ?1 AND track_id = ?2 AND source = 'remote'
            ",
            params![source_id.as_str(), track_id.as_str()],
        )?;
        Ok(deleted > 0)
    }
    pub fn search_library(
        &self,
        source_id: &SourceId,
        query: &str,
        limit: usize,
    ) -> StoreResult<SearchResults> {
        let Some(query) = fts_query(query) else {
            return Ok(SearchResults::default());
        };
        Ok(SearchResults {
            albums: self.search_albums(source_id, &query, limit)?,
            tracks: self.search_tracks(source_id, &query, limit)?,
            artists: self.search_artists(source_id, &query, limit)?,
            playlists: self.search_playlists(source_id, &query, limit)?,
        })
    }
    pub fn save_cover_cache_entry(&self, entry: &CoverCacheEntry) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO cover_cache (
                source_id, item_id, image_tag, size, path, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
            ON CONFLICT(source_id, item_id, image_tag, size) DO UPDATE SET
                path = excluded.path,
                updated_at = excluded.updated_at
            ",
            params![
                entry.source_id.as_str(),
                entry.item_id.as_str(),
                entry.image_tag.as_str(),
                i64::from(entry.size),
                entry.path.as_str(),
            ],
        )?;
        self.delete_external_image_lookup_miss(
            &entry.source_id,
            &entry.item_id,
            &entry.image_tag,
            entry.size,
        )?;
        Ok(())
    }
    pub fn load_cover_cache_entry(
        &self,
        source_id: &SourceId,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<Option<CoverCacheEntry>> {
        self.connection
            .query_row(
                "
                SELECT source_id, item_id, image_tag, size, path
                FROM cover_cache
                WHERE source_id = ?1 AND item_id = ?2 AND image_tag = ?3 AND size = ?4
                ",
                params![source_id.as_str(), item_id, image_tag, i64::from(size)],
                |row| {
                    Ok(CoverCacheEntry {
                        source_id: SourceId::new(row.get::<_, String>(0)?),
                        item_id: row.get(1)?,
                        image_tag: row.get(2)?,
                        size: u32_from_i64(row.get(3)?),
                        path: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn selected_source_cover_cache_missing(&self, source_id: &SourceId) -> StoreResult<bool> {
        let library_rows = self.connection.query_row(
            "
            SELECT (SELECT COUNT(*) FROM albums WHERE source_id = ?1)
                 + (SELECT COUNT(*) FROM tracks WHERE source_id = ?1)
            ",
            params![source_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        if library_rows == 0 {
            return Ok(true);
        }

        let mut statement = self.connection.prepare(
            "
            WITH selected_refs AS (
                SELECT image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                FROM albums
                WHERE source_id = ?1 AND image_item_id IS NOT NULL
                  AND image_item_id NOT LIKE 'external:%'
                UNION
                SELECT image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                FROM tracks
                WHERE source_id = ?1 AND image_item_id IS NOT NULL
                  AND image_item_id NOT LIKE 'external:%'
            )
            SELECT refs.item_id, refs.image_tag, cache.path
            FROM selected_refs refs
            LEFT JOIN cover_cache cache
              ON cache.source_id = ?1
             AND cache.item_id = refs.item_id
             AND cache.image_tag = refs.image_tag
             AND cache.size IN (256, 512)
            ",
        )?;
        let mut refs = HashMap::<(String, String), bool>::new();
        let rows = statement.query_map(params![source_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                optional_string_column(row, 2)?,
            ))
        })?;
        for row in rows {
            let (item_id, image_tag, path) = row?;
            let file_exists = path.as_deref().is_some_and(|path| Path::new(path).exists());
            refs.entry((item_id, image_tag))
                .and_modify(|exists| *exists |= file_exists)
                .or_insert(file_exists);
        }
        Ok(refs.values().any(|exists| !exists))
    }
    pub fn load_external_cover_cache_entry_by_content(
        &self,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<Option<CoverCacheEntry>> {
        if !item_id.starts_with("external:") {
            return Ok(None);
        }
        self.connection
            .query_row(
                "
                SELECT source_id, item_id, image_tag, size, path
                FROM cover_cache
                WHERE item_id = ?1 AND image_tag = ?2 AND size = ?3
                ORDER BY updated_at DESC, source_id
                LIMIT 1
                ",
                params![item_id, image_tag, i64::from(size)],
                |row| {
                    Ok(CoverCacheEntry {
                        source_id: SourceId::new(row.get::<_, String>(0)?),
                        item_id: row.get(1)?,
                        image_tag: row.get(2)?,
                        size: u32_from_i64(row.get(3)?),
                        path: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn save_external_cover_content_path(
        &self,
        item_id: &str,
        image_tag: &str,
        size: u32,
        path: &str,
    ) -> StoreResult<()> {
        if !item_id.starts_with("external:") {
            return Ok(());
        }
        self.connection.execute(
            "
            INSERT INTO content_cache_entries (
                cache_scope, content_kind, content_key, variant, status,
                path_or_value, source, updated_at
            )
            VALUES ('external', 'cover', ?1, ?2, 'ready', ?3, 'cover-art-policy', CURRENT_TIMESTAMP)
            ON CONFLICT(cache_scope, content_kind, content_key, variant) DO UPDATE SET
                status = excluded.status,
                path_or_value = excluded.path_or_value,
                source = excluded.source,
                updated_at = excluded.updated_at
            ",
            params![
                external_cover_content_key(item_id, image_tag),
                cover_content_variant(size),
                path
            ],
        )?;
        Ok(())
    }
    pub fn load_external_cover_content_path(
        &self,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<Option<String>> {
        if !item_id.starts_with("external:") {
            return Ok(None);
        }
        self.connection
            .query_row(
                "
                SELECT path_or_value
                FROM content_cache_entries
                WHERE cache_scope = 'external'
                  AND content_kind = 'cover'
                  AND content_key = ?1
                  AND variant = ?2
                  AND status = 'ready'
                ",
                params![
                    external_cover_content_key(item_id, image_tag),
                    cover_content_variant(size)
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn save_external_cover_content_miss(
        &self,
        item_id: &str,
        image_tag: &str,
        size: u32,
        reason: &str,
    ) -> StoreResult<()> {
        if !item_id.starts_with("external:") {
            return Ok(());
        }
        self.connection.execute(
            "
            INSERT INTO content_cache_entries (
                cache_scope, content_kind, content_key, variant, status,
                path_or_value, source, updated_at
            )
            VALUES ('external', 'cover', ?1, ?2, 'missing', ?3, 'cover-art-policy', CURRENT_TIMESTAMP)
            ON CONFLICT(cache_scope, content_kind, content_key, variant) DO UPDATE SET
                status = excluded.status,
                path_or_value = excluded.path_or_value,
                source = excluded.source,
                updated_at = excluded.updated_at
            ",
            params![external_cover_content_key(item_id, image_tag), cover_content_variant(size), reason],
        )?;
        Ok(())
    }
    pub fn load_external_cover_content_miss(
        &self,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<bool> {
        if !item_id.starts_with("external:") {
            return Ok(false);
        }
        let found = self.connection.query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM content_cache_entries
                WHERE cache_scope = 'external'
                  AND content_kind = 'cover'
                  AND content_key = ?1
                  AND variant = ?2
                  AND status = 'missing'
            )
            ",
            params![
                external_cover_content_key(item_id, image_tag),
                cover_content_variant(size)
            ],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(found)
    }
    pub fn delete_external_cover_content_miss(
        &self,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<()> {
        if !item_id.starts_with("external:") {
            return Ok(());
        }
        self.connection.execute(
            "
            DELETE FROM content_cache_entries
            WHERE cache_scope = 'external'
              AND content_kind = 'cover'
              AND content_key = ?1
              AND variant = ?2
              AND status = 'missing'
            ",
            params![
                external_cover_content_key(item_id, image_tag),
                cover_content_variant(size)
            ],
        )?;
        Ok(())
    }
    pub fn delete_cover_cache_entry(
        &self,
        source_id: &SourceId,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            DELETE FROM cover_cache
            WHERE source_id = ?1 AND item_id = ?2 AND image_tag = ?3 AND size = ?4
            ",
            params![source_id.as_str(), item_id, image_tag, i64::from(size)],
        )?;
        Ok(())
    }
    pub fn prune_stale_image_cache_entries(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<Vec<CoverCacheEntry>> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                WITH live_image_refs AS (
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM albums WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM tracks WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM artists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM album_artists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM genres WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM playlists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM collection_cover_refs WHERE image_item_id IS NOT NULL
                )
                SELECT source_id, item_id, image_tag, size, path
                FROM cover_cache cache
                WHERE cache.source_id = ?1
                  AND cache.item_id NOT LIKE 'external:%'
                  AND NOT EXISTS (
                      SELECT 1
                      FROM live_image_refs
                      WHERE live_image_refs.source_id = cache.source_id
                        AND live_image_refs.item_id = cache.item_id
                        AND live_image_refs.image_tag = cache.image_tag
                  )
                ",
            )?;
            let pruned_entries = collect_rows(
                statement.query_map(params![source_id.as_str()], |row| {
                    Ok(CoverCacheEntry {
                        source_id: SourceId::new(row.get::<_, String>(0)?),
                        item_id: row.get(1)?,
                        image_tag: row.get(2)?,
                        size: u32_from_i64(row.get(3)?),
                        path: row.get(4)?,
                    })
                })?,
            )?;
            connection.execute(
                "
                WITH live_image_refs AS (
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM albums WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM tracks WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM artists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM album_artists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM genres WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM playlists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM collection_cover_refs WHERE image_item_id IS NOT NULL
                )
                DELETE FROM cover_cache
                WHERE source_id = ?1
                  AND item_id NOT LIKE 'external:%'
                  AND NOT EXISTS (
                      SELECT 1
                      FROM live_image_refs
                      WHERE live_image_refs.source_id = cover_cache.source_id
                        AND live_image_refs.item_id = cover_cache.item_id
                        AND live_image_refs.image_tag = cover_cache.image_tag
                  )
                ",
                params![source_id.as_str()],
            )?;
            connection.execute(
                "
                WITH live_image_refs AS (
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM albums WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM tracks WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM artists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM album_artists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM genres WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM playlists WHERE image_item_id IS NOT NULL
                    UNION
                    SELECT source_id, image_item_id AS item_id, COALESCE(image_tag, 'untagged') AS image_tag
                    FROM collection_cover_refs WHERE image_item_id IS NOT NULL
                )
                DELETE FROM external_image_lookup_misses
                WHERE source_id = ?1
                  AND item_id NOT LIKE 'external:%'
                  AND NOT EXISTS (
                      SELECT 1
                      FROM live_image_refs
                      WHERE live_image_refs.source_id = external_image_lookup_misses.source_id
                        AND live_image_refs.item_id = external_image_lookup_misses.item_id
                        AND live_image_refs.image_tag = external_image_lookup_misses.image_tag
                  )
                ",
                params![source_id.as_str()],
            )?;
            Ok(pruned_entries)
        })
    }

    pub fn prune_external_images(
        &self,
        source_id: &SourceId,
        live_refs: &[ImageRef],
        prune_all_external: bool,
    ) -> StoreResult<Vec<CoverCacheEntry>> {
        const EXTERNAL_MISS_TTL: &str = "-30 days";

        self.write_batch(|connection| {
            connection.execute(
                "
                CREATE TEMP TABLE IF NOT EXISTS live_generated_external_image_refs (
                    item_id TEXT NOT NULL,
                    image_tag TEXT NOT NULL,
                    PRIMARY KEY (item_id, image_tag)
                )
                ",
                [],
            )?;
            connection.execute("DELETE FROM live_generated_external_image_refs", [])?;
            {
                let mut insert_live = connection.prepare(
                    "
                    INSERT INTO live_generated_external_image_refs (item_id, image_tag)
                    VALUES (?1, ?2)
                    ON CONFLICT(item_id, image_tag) DO NOTHING
                    ",
                )?;
                for image_ref in live_refs {
                    let tag = image_ref.tag.as_deref().unwrap_or("untagged");
                    insert_live.execute(params![image_ref.item_id.as_str(), tag])?;
                }
            }

            let prune_all = bool_to_i64(prune_all_external);
            let mut statement = connection.prepare(
                "
                SELECT source_id, item_id, image_tag, size, path
                FROM cover_cache cache
                WHERE cache.source_id = ?1
                  AND cache.item_id LIKE 'external:%'
                  AND (
                      ?2 = 1 OR NOT EXISTS (
                          SELECT 1
                          FROM live_generated_external_image_refs live
                          WHERE live.item_id = cache.item_id
                            AND live.image_tag = cache.image_tag
                      )
                  )
                ",
            )?;
            let pruned_entries = collect_rows(statement.query_map(
                params![source_id.as_str(), prune_all],
                |row| {
                    Ok(CoverCacheEntry {
                        source_id: SourceId::new(row.get::<_, String>(0)?),
                        item_id: row.get(1)?,
                        image_tag: row.get(2)?,
                        size: u32_from_i64(row.get(3)?),
                        path: row.get(4)?,
                    })
                },
            )?)?;
            connection.execute(
                "
                DELETE FROM cover_cache
                WHERE source_id = ?1
                  AND item_id LIKE 'external:%'
                  AND (
                      ?2 = 1 OR NOT EXISTS (
                          SELECT 1
                          FROM live_generated_external_image_refs live
                          WHERE live.item_id = cover_cache.item_id
                            AND live.image_tag = cover_cache.image_tag
                      )
                  )
                ",
                params![source_id.as_str(), prune_all],
            )?;
            connection.execute(
                "
                DELETE FROM external_image_lookup_misses
                WHERE source_id = ?1
                  AND item_id LIKE 'external:%'
                  AND (
                      ?2 = 1 OR (
                          updated_at <= datetime('now', ?3)
                          AND NOT EXISTS (
                              SELECT 1
                              FROM live_generated_external_image_refs live
                              WHERE live.item_id = external_image_lookup_misses.item_id
                                AND live.image_tag = external_image_lookup_misses.image_tag
                          )
                      )
                  )
                ",
                params![source_id.as_str(), prune_all, EXTERNAL_MISS_TTL],
            )?;
            Ok(pruned_entries)
        })
    }
    pub fn save_external_image_lookup_miss(
        &self,
        source_id: &SourceId,
        item_id: &str,
        image_tag: &str,
        size: u32,
        reason: &str,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO external_image_lookup_misses (
                source_id, item_id, image_tag, size, reason, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
            ON CONFLICT(source_id, item_id, image_tag, size) DO UPDATE SET
                reason = excluded.reason,
                updated_at = excluded.updated_at
            ",
            params![
                source_id.as_str(),
                item_id,
                image_tag,
                i64::from(size),
                reason,
            ],
        )?;
        Ok(())
    }
    pub fn load_external_image_lookup_miss(
        &self,
        source_id: &SourceId,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<bool> {
        let found = self.connection.query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM external_image_lookup_misses
                WHERE source_id = ?1 AND item_id = ?2 AND image_tag = ?3 AND size = ?4
            )
            ",
            params![source_id.as_str(), item_id, image_tag, i64::from(size)],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(found)
    }
    pub fn load_external_image_lookup_miss_by_content(
        &self,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<bool> {
        if !item_id.starts_with("external:") {
            return Ok(false);
        }
        let found = self.connection.query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM external_image_lookup_misses
                WHERE item_id = ?1 AND image_tag = ?2 AND size = ?3
            )
            ",
            params![item_id, image_tag, i64::from(size)],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(found)
    }
    pub fn delete_external_image_lookup_miss(
        &self,
        source_id: &SourceId,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            DELETE FROM external_image_lookup_misses
            WHERE source_id = ?1 AND item_id = ?2 AND image_tag = ?3 AND size = ?4
            ",
            params![source_id.as_str(), item_id, image_tag, i64::from(size)],
        )?;
        Ok(())
    }
    pub fn clear_external_image_lookup_misses(&self, source_id: &SourceId) -> StoreResult<()> {
        self.connection.execute(
            "
            DELETE FROM external_image_lookup_misses
            WHERE source_id = ?1
            ",
            params![source_id.as_str()],
        )?;
        Ok(())
    }
    pub fn schema_version(&self) -> StoreResult<i64> {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .map_err(StoreError::from)
    }
    pub fn foreign_keys_enabled(&self) -> StoreResult<bool> {
        let enabled = self
            .connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?;
        Ok(enabled == 1)
    }
    pub fn journal_mode(&self) -> StoreResult<String> {
        self.connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(StoreError::from)
    }
    pub fn busy_timeout_ms(&self) -> StoreResult<i64> {
        self.connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .map_err(StoreError::from)
    }
    pub fn fts5_available(&self) -> StoreResult<bool> {
        let exists = self.connection.query_row(
            "
            SELECT COUNT(*)
            FROM sqlite_master
            WHERE type = 'table' AND name = 'library_fts'
            ",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(exists == 1)
    }
    pub(super) fn search_albums(
        &self,
        source_id: &SourceId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Album>> {
        self.search_albums_page(source_id, query, 0, limit, limit)
            .map(|page| page.items)
    }
    pub(super) fn search_albums_page(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
        total: usize,
    ) -> StoreResult<PagedResponse<Album>> {
        let selected_folder = self.selected_music_folder_id(source_id)?;
        let folder_id = selected_folder.as_ref().map(|folder_id| folder_id.as_str());
        let sql = format!(
            "
            SELECT a.album_id, a.title, a.artist, a.artist_id, a.year,
                   a.release_date, a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite, a.color_seed,
                   a.image_item_id, a.image_tag
            FROM library_fts f
            JOIN albums a
                ON a.source_id = f.source_id AND a.album_id = f.item_id
            WHERE f.source_id = ?1
              AND f.item_type = 'album'
              AND library_fts MATCH ?2
              AND (
                  ?5 IS NULL OR EXISTS (
                      SELECT 1
                      FROM tracks t
                      JOIN track_music_folders tmf
                        ON tmf.source_id = t.source_id AND tmf.track_id = t.track_id
                      WHERE t.source_id = a.source_id
                        AND t.album_id = a.album_id
                        AND tmf.folder_id = ?5
                  )
              )
            ORDER BY bm25(library_fts)
            LIMIT ?3 OFFSET ?4
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut albums = collect_rows(statement.query_map(
            params![
                source_id.as_str(),
                query,
                limit as i64,
                offset as i64,
                folder_id
            ],
            album_from_row,
        )?)?;
        self.attach_album_metadata(source_id, &mut albums)?;
        Ok(PagedResponse::new(albums, total))
    }
    pub(super) fn load_albums_like(
        &self,
        source_id: &SourceId,
        pattern: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Album>> {
        let selected_folder = self.selected_music_folder_id(source_id)?;
        let folder_id = selected_folder.as_ref().map(|folder_id| folder_id.as_str());
        let total = self.connection.query_row(
            "
            SELECT COUNT(*)
            FROM albums a
            WHERE a.source_id = ?1
              AND (
                  LOWER(a.title) LIKE ?2 ESCAPE '\\'
                  OR LOWER(a.artist) LIKE ?2 ESCAPE '\\'
                  OR CAST(a.year AS TEXT) LIKE ?2 ESCAPE '\\'
                  OR EXISTS (
                      SELECT 1
                      FROM album_genres ag
                      WHERE ag.source_id = a.source_id
                        AND ag.album_id = a.album_id
                        AND LOWER(ag.genre_name) LIKE ?2 ESCAPE '\\'
                  )
              )
              AND (
                  ?3 IS NULL OR EXISTS (
                      SELECT 1
                      FROM tracks t
                      JOIN track_music_folders tmf
                        ON tmf.source_id = t.source_id AND tmf.track_id = t.track_id
                      WHERE t.source_id = a.source_id
                        AND t.album_id = a.album_id
                        AND tmf.folder_id = ?3
                  )
              )
            ",
            params![source_id.as_str(), pattern, folder_id],
            |row| row.get::<_, i64>(0),
        )?;
        let sql = format!(
            "
            SELECT a.album_id, a.title, a.artist, a.artist_id, a.year,
                   a.release_date, a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite, a.color_seed,
                   a.image_item_id, a.image_tag
            FROM albums a
            WHERE a.source_id = ?1
              AND (
                  LOWER(a.title) LIKE ?2 ESCAPE '\\'
                  OR LOWER(a.artist) LIKE ?2 ESCAPE '\\'
                  OR CAST(a.year AS TEXT) LIKE ?2 ESCAPE '\\'
                  OR EXISTS (
                      SELECT 1
                      FROM album_genres ag
                      WHERE ag.source_id = a.source_id
                        AND ag.album_id = a.album_id
                        AND LOWER(ag.genre_name) LIKE ?2 ESCAPE '\\'
                  )
              )
              AND (
                  ?5 IS NULL OR EXISTS (
                      SELECT 1
                      FROM tracks t
                      JOIN track_music_folders tmf
                        ON tmf.source_id = t.source_id AND tmf.track_id = t.track_id
                      WHERE t.source_id = a.source_id
                        AND t.album_id = a.album_id
                        AND tmf.folder_id = ?5
                  )
              )
            ORDER BY a.title COLLATE NOCASE
            LIMIT ?3 OFFSET ?4
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut albums = collect_rows(statement.query_map(
            params![
                source_id.as_str(),
                pattern,
                limit as i64,
                offset as i64,
                folder_id
            ],
            album_from_row,
        )?)?;
        self.attach_album_metadata(source_id, &mut albums)?;
        Ok(PagedResponse::new(albums, total.max(0) as usize))
    }
    pub(super) fn attach_genre_cover_image_refs(
        &self,
        source_id: &SourceId,
        genres: &mut [Genre],
    ) -> StoreResult<()> {
        let ids = genres
            .iter()
            .map(|genre| genre.id.as_str().to_string())
            .collect::<Vec<_>>();
        let refs_by_id =
            self.load_collection_cover_refs_map(source_id, COLLECTION_COVER_GENRE, &ids)?;
        for genre in genres {
            genre.image_refs = refs_by_id
                .get(genre.id.as_str())
                .cloned()
                .unwrap_or_default();
            if genre.image_ref.is_none() {
                genre.image_ref = genre.image_refs.first().cloned();
            }
        }
        Ok(())
    }
    pub(super) fn attach_playlist_cover_image_refs(
        &self,
        source_id: &SourceId,
        playlists: &mut [Playlist],
    ) -> StoreResult<()> {
        let ids = playlists
            .iter()
            .map(|playlist| playlist.id.as_str().to_string())
            .collect::<Vec<_>>();
        let refs_by_id =
            self.load_collection_cover_refs_map(source_id, COLLECTION_COVER_PLAYLIST, &ids)?;
        for playlist in playlists {
            playlist.image_refs = refs_by_id
                .get(playlist.id.as_str())
                .cloned()
                .unwrap_or_default();
            if playlist.image_ref.is_none() {
                playlist.image_ref = playlist.image_refs.first().cloned();
            }
        }
        Ok(())
    }
    pub(super) fn load_collection_cover_refs(
        &self,
        source_id: &SourceId,
        collection_type: &str,
        collection_id: &str,
    ) -> StoreResult<Vec<ImageRef>> {
        self.load_collection_cover_refs_map(
            source_id,
            collection_type,
            &[collection_id.to_string()],
        )
        .map(|mut refs_by_id| refs_by_id.remove(collection_id).unwrap_or_default())
    }
    fn load_collection_cover_refs_map(
        &self,
        source_id: &SourceId,
        collection_type: &str,
        collection_ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<ImageRef>>> {
        if collection_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = std::iter::repeat_n("?", collection_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "
            SELECT collection_id, image_item_id, image_tag
            FROM collection_cover_refs
            WHERE source_id = ?
              AND collection_type = ?
              AND collection_id IN ({placeholders})
            ORDER BY collection_id, position
            "
        );
        let mut values = Vec::with_capacity(collection_ids.len() + 2);
        values.push(Value::Text(source_id.as_str().to_string()));
        values.push(Value::Text(collection_type.to_string()));
        values.extend(collection_ids.iter().cloned().map(Value::Text));
        let mut statement = self.connection.prepare(&sql)?;
        let rows = collect_rows(statement.query_map(params_from_iter(values), |row| {
            Ok((
                row.get::<_, String>(0)?,
                collection_cover_ref_from_row(row, 1, 2)?,
            ))
        })?)?;
        let mut refs_by_id: HashMap<String, Vec<ImageRef>> = HashMap::new();
        for (collection_id, image_ref) in rows {
            refs_by_id.entry(collection_id).or_default().push(image_ref);
        }
        Ok(refs_by_id)
    }
    pub(super) fn refresh_collection_cover_refs(&self, source_id: &SourceId) -> StoreResult<()> {
        self.write_batch(|connection| refresh_collection_refs(connection, source_id))
    }
    pub fn ensure_collection_cover_refs(&self, source_id: &SourceId) -> StoreResult<()> {
        let genre_refs_complete =
            collection_cover_refs_complete(&self.connection, source_id, COLLECTION_COVER_GENRE)?;
        let playlist_refs_complete =
            collection_cover_refs_complete(&self.connection, source_id, COLLECTION_COVER_PLAYLIST)?;
        if !genre_refs_complete || !playlist_refs_complete {
            self.refresh_collection_cover_refs(source_id)?;
        }

        if !collection_cover_refs_cached(
            &self.connection,
            source_id,
            COLLECTION_COVER_SMART_PLAYLIST,
        )? {
            self.refresh_smart_playlist_cover_refs(source_id)?;
        }
        Ok(())
    }
    pub(super) fn search_tracks(
        &self,
        source_id: &SourceId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Track>> {
        self.search_tracks_page(source_id, query, 0, limit, limit)
            .map(|page| page.items)
    }
}

fn collection_cover_refs_complete(
    connection: &Connection,
    source_id: &SourceId,
    collection_type: &str,
) -> StoreResult<bool> {
    match collection_type {
        COLLECTION_COVER_GENRE => genre_cover_refs_complete(connection, source_id),
        COLLECTION_COVER_PLAYLIST => playlist_cover_refs_complete(connection, source_id),
        _ => collection_cover_refs_cached(connection, source_id, collection_type),
    }
}

fn collection_cover_refs_cached(
    connection: &Connection,
    source_id: &SourceId,
    collection_type: &str,
) -> StoreResult<bool> {
    connection
        .query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM collection_cover_refs
                WHERE source_id = ?1
                  AND collection_type = ?2
            )
            ",
            params![source_id.as_str(), collection_type],
            |row| row.get::<_, bool>(0),
        )
        .map_err(Into::into)
}

fn genre_cover_refs_complete(connection: &Connection, source_id: &SourceId) -> StoreResult<bool> {
    connection
        .query_row(
            "
            SELECT NOT EXISTS(
                SELECT 1
                FROM genres g
                WHERE g.source_id = ?1
                  AND (
                      EXISTS (
                          SELECT 1
                          FROM album_genres ag
                          WHERE ag.source_id = g.source_id AND ag.genre_name = g.name
                      )
                      OR EXISTS (
                          SELECT 1
                          FROM track_genres tg
                          WHERE tg.source_id = g.source_id AND tg.genre_name = g.name
                      )
                  )
                  AND (
                      g.image_item_id IS NOT NULL
                      OR EXISTS (
                          SELECT 1
                          FROM album_genres ag
                          JOIN albums a
                              ON a.source_id = ag.source_id AND a.album_id = ag.album_id
                          WHERE ag.source_id = g.source_id
                            AND ag.genre_name = g.name
                            AND a.image_item_id IS NOT NULL
                      )
                      OR EXISTS (
                          SELECT 1
                          FROM track_genres tg
                          JOIN tracks t
                              ON t.source_id = tg.source_id AND t.track_id = tg.track_id
                          LEFT JOIN albums a
                              ON a.source_id = t.source_id AND a.album_id = t.album_id
                          WHERE tg.source_id = g.source_id
                            AND tg.genre_name = g.name
                            AND COALESCE(t.image_item_id, a.image_item_id) IS NOT NULL
                      )
                  )
                  AND NOT EXISTS (
                      SELECT 1
                      FROM collection_cover_refs ccr
                      WHERE ccr.source_id = g.source_id
                        AND ccr.collection_type = ?2
                        AND ccr.collection_id = g.genre_id
                  )
            )
            ",
            params![source_id.as_str(), COLLECTION_COVER_GENRE],
            |row| row.get::<_, bool>(0),
        )
        .map_err(Into::into)
}

fn playlist_cover_refs_complete(
    connection: &Connection,
    source_id: &SourceId,
) -> StoreResult<bool> {
    connection
        .query_row(
            "
            SELECT NOT EXISTS(
                SELECT 1
                FROM playlists p
                WHERE p.source_id = ?1
                  AND (
                      p.image_item_id IS NOT NULL
                      OR EXISTS (
                          SELECT 1
                          FROM playlist_tracks pt
                          JOIN tracks t
                              ON t.source_id = pt.source_id AND t.track_id = pt.track_id
                          LEFT JOIN albums a
                              ON a.source_id = t.source_id AND a.album_id = t.album_id
                          WHERE pt.source_id = p.source_id
                            AND pt.playlist_id = p.playlist_id
                            AND COALESCE(t.image_item_id, a.image_item_id) IS NOT NULL
                      )
                  )
                  AND NOT EXISTS (
                      SELECT 1
                      FROM collection_cover_refs ccr
                      WHERE ccr.source_id = p.source_id
                        AND ccr.collection_type = ?2
                        AND ccr.collection_id = p.playlist_id
                  )
            )
            ",
            params![source_id.as_str(), COLLECTION_COVER_PLAYLIST],
            |row| row.get::<_, bool>(0),
        )
        .map_err(Into::into)
}

pub(super) fn refresh_collection_refs(
    connection: &Connection,
    source_id: &SourceId,
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM collection_cover_refs
        WHERE source_id = ?1
          AND collection_type IN (?2, ?3)
        ",
        params![
            source_id.as_str(),
            COLLECTION_COVER_GENRE,
            COLLECTION_COVER_PLAYLIST,
        ],
    )?;
    let genres = genre_cover_sources(connection, source_id)?;
    for (genre_id, genre_name) in genres {
        let image_refs = genre_cover_refs(connection, source_id, &genre_id, &genre_name)?;
        replace_collection_refs(
            connection,
            source_id,
            COLLECTION_COVER_GENRE,
            &genre_id,
            &image_refs,
        )?;
    }
    let playlist_ids = playlist_cover_sources(connection, source_id)?;
    for playlist_id in playlist_ids {
        refresh_playlist_refs(connection, source_id, &PlaylistId::new(playlist_id))?;
    }
    Ok(())
}

fn genre_cover_sources(
    connection: &Connection,
    source_id: &SourceId,
) -> StoreResult<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "
        SELECT genre_id, name
        FROM genres g
        WHERE g.source_id = ?1
          AND (
              EXISTS (
                  SELECT 1
                  FROM album_genres ag
                  WHERE ag.source_id = g.source_id AND ag.genre_name = g.name
              )
              OR EXISTS (
                  SELECT 1
                  FROM track_genres tg
                  WHERE tg.source_id = g.source_id AND tg.genre_name = g.name
              )
          )
        ORDER BY name COLLATE NOCASE
        ",
    )?;
    collect_rows(statement.query_map(params![source_id.as_str()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?)
}

fn genre_cover_refs(
    connection: &Connection,
    source_id: &SourceId,
    genre_id: &str,
    genre_name: &str,
) -> StoreResult<Vec<ImageRef>> {
    let mut image_refs = Vec::new();
    let mut album_statement = connection.prepare(
        "
        SELECT a.image_item_id, a.image_tag
        FROM album_genres ag
        JOIN albums a
            ON a.source_id = ag.source_id AND a.album_id = ag.album_id
        WHERE ag.source_id = ?1
          AND ag.genre_name = ?2
          AND a.image_item_id IS NOT NULL
        ORDER BY a.title COLLATE NOCASE
        LIMIT 16
        ",
    )?;
    let album_refs = collect_rows(
        album_statement.query_map(params![source_id.as_str(), genre_name], |row| {
            collection_cover_ref_from_row(row, 0, 1)
        })?,
    )?;
    image_refs.extend(album_refs);

    let mut track_statement = connection.prepare(
        "
        SELECT COALESCE(t.image_item_id, a.image_item_id) AS image_item_id,
               CASE
                   WHEN t.image_item_id IS NOT NULL THEN t.image_tag
                   ELSE a.image_tag
               END AS image_tag
        FROM track_genres tg
        JOIN tracks t
            ON t.source_id = tg.source_id AND t.track_id = tg.track_id
        LEFT JOIN albums a
            ON a.source_id = t.source_id AND a.album_id = t.album_id
        WHERE tg.source_id = ?1
          AND tg.genre_name = ?2
          AND COALESCE(t.image_item_id, a.image_item_id) IS NOT NULL
        ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                 t.title COLLATE NOCASE
        LIMIT ?3
        ",
    )?;
    let track_refs = collect_rows(
        track_statement.query_map(params![source_id.as_str(), genre_name, 16_i64,], |row| {
            collection_cover_ref_from_row(row, 0, 1)
        })?,
    )?;
    image_refs.extend(track_refs);
    if image_refs.is_empty()
        && let Some(image_ref) =
            collection_direct_image_ref(connection, "genres", "genre_id", source_id, genre_id)?
    {
        image_refs.push(image_ref);
    }
    Ok(image_refs)
}

fn playlist_cover_sources(
    connection: &Connection,
    source_id: &SourceId,
) -> StoreResult<Vec<String>> {
    let mut statement = connection.prepare(
        "
        SELECT playlist_id
        FROM playlists
        WHERE source_id = ?1
        ORDER BY name COLLATE NOCASE
        ",
    )?;
    collect_rows(statement.query_map(params![source_id.as_str()], |row| row.get::<_, String>(0))?)
}

pub(super) fn refresh_playlist_refs(
    connection: &Connection,
    source_id: &SourceId,
    playlist_id: &PlaylistId,
) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "
        SELECT COALESCE(t.image_item_id, a.image_item_id) AS image_item_id,
               CASE
                   WHEN t.image_item_id IS NOT NULL THEN t.image_tag
                   ELSE a.image_tag
               END AS image_tag
        FROM playlist_tracks pt
        JOIN tracks t
            ON t.source_id = pt.source_id AND t.track_id = pt.track_id
        LEFT JOIN albums a
            ON a.source_id = t.source_id AND a.album_id = t.album_id
        WHERE pt.source_id = ?1
          AND pt.playlist_id = ?2
          AND COALESCE(t.image_item_id, a.image_item_id) IS NOT NULL
        ORDER BY pt.position
        LIMIT 16
        ",
    )?;
    let image_refs = collect_rows(
        statement.query_map(params![source_id.as_str(), playlist_id.as_str()], |row| {
            collection_cover_ref_from_row(row, 0, 1)
        })?,
    )?;
    let image_refs = if image_refs.is_empty() {
        collection_direct_image_ref(
            connection,
            "playlists",
            "playlist_id",
            source_id,
            playlist_id.as_str(),
        )?
        .into_iter()
        .collect()
    } else {
        image_refs
    };
    replace_collection_refs(
        connection,
        source_id,
        COLLECTION_COVER_PLAYLIST,
        playlist_id.as_str(),
        &image_refs,
    )
}

pub(super) fn refresh_playlist_stats(
    connection: &Connection,
    source_id: &SourceId,
    playlist_id: &PlaylistId,
) -> StoreResult<()> {
    let top_genres = playlist_top_genres_json(connection, source_id, playlist_id)?;
    connection.execute(
        "
        UPDATE playlists
        SET track_count = (
                SELECT COUNT(*)
                FROM playlist_tracks pt
                JOIN tracks t
                    ON t.source_id = pt.source_id AND t.track_id = pt.track_id
                WHERE pt.source_id = ?1 AND pt.playlist_id = ?2
            ),
            duration_seconds = (
                SELECT COALESCE(SUM(t.duration_seconds), 0)
                FROM playlist_tracks pt
                JOIN tracks t
                    ON t.source_id = pt.source_id AND t.track_id = pt.track_id
                WHERE pt.source_id = ?1 AND pt.playlist_id = ?2
            ),
            top_genres_json = ?3
        WHERE source_id = ?1 AND playlist_id = ?2
        ",
        params![source_id.as_str(), playlist_id.as_str(), top_genres],
    )?;
    Ok(())
}

fn playlist_owner_on_connection(
    connection: &Connection,
    source_id: &SourceId,
    playlist_id: &PlaylistId,
) -> StoreResult<Option<SourceFeatureOwner>> {
    connection
        .query_row(
            "
            SELECT owner
            FROM playlists
            WHERE source_id = ?1 AND playlist_id = ?2
            ",
            params![source_id.as_str(), playlist_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|owner| playlist_owner_from_str(&owner))
        .transpose()
}

fn ensure_playlist_owner(
    connection: &Connection,
    source_id: &SourceId,
    playlist_id: &PlaylistId,
    owner: SourceFeatureOwner,
) -> StoreResult<()> {
    match playlist_owner_on_connection(connection, source_id, playlist_id)? {
        Some(current) if current == owner => Ok(()),
        Some(current) => Err(StoreError::InvalidPlaylistOwner(format!(
            "playlist {} is owned by {}, not {}",
            playlist_id.as_str(),
            playlist_owner_to_str(current),
            playlist_owner_to_str(owner)
        ))),
        None => Err(StoreError::InvalidPlaylistOwner(format!(
            "playlist {} was not found",
            playlist_id.as_str()
        ))),
    }
}

fn playlist_top_genres_json(
    connection: &Connection,
    source_id: &SourceId,
    playlist_id: &PlaylistId,
) -> StoreResult<String> {
    let mut statement = connection.prepare(
        "
        SELECT tg.genre_name
        FROM playlist_tracks pt
        JOIN track_genres tg
            ON tg.source_id = pt.source_id AND tg.track_id = pt.track_id
        WHERE pt.source_id = ?1 AND pt.playlist_id = ?2
        GROUP BY tg.genre_name
        ORDER BY COUNT(*) DESC, LOWER(tg.genre_name)
        LIMIT 2
        ",
    )?;
    let genres = collect_rows(
        statement.query_map(params![source_id.as_str(), playlist_id.as_str()], |row| {
            row.get::<_, String>(0)
        })?,
    )?;
    string_vec_json(&genres)
}

pub(super) fn playlist_stats_changed(before: Option<Playlist>, after: Option<Playlist>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            before.track_count != after.track_count
                || before.duration_seconds != after.duration_seconds
                || before.top_genres != after.top_genres
        }
        (None, Some(after)) => {
            after.track_count > 0 || after.duration_seconds > 0 || !after.top_genres.is_empty()
        }
        (Some(before), None) => {
            before.track_count > 0 || before.duration_seconds > 0 || !before.top_genres.is_empty()
        }
        (None, None) => false,
    }
}

fn external_cover_content_key(item_id: &str, image_tag: &str) -> String {
    format!("{item_id}\u{1f}{image_tag}")
}

fn cover_content_variant(size: u32) -> String {
    format!("size:{size}")
}

fn collection_direct_image_ref(
    connection: &Connection,
    table: &str,
    id_column: &str,
    source_id: &SourceId,
    collection_id: &str,
) -> StoreResult<Option<ImageRef>> {
    let sql = format!(
        "
        SELECT image_item_id, image_tag
        FROM {table}
        WHERE source_id = ?1
          AND {id_column} = ?2
          AND image_item_id IS NOT NULL
        LIMIT 1
        "
    );
    connection
        .query_row(&sql, params![source_id.as_str(), collection_id], |row| {
            collection_cover_ref_from_row(row, 0, 1)
        })
        .optional()
        .map_err(Into::into)
}
