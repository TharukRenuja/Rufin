use super::sources::*;
use super::*;

const REPRESENTATIVE_RELATION_WINDOW: usize = 16;

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
        self.read_snapshot(|store| store.load_genres_inner(source_id, offset, limit))
    }
    fn load_genres_inner(
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
        self.attach_genre_representative_albums(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }
    pub fn load_genres_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Genre>> {
        self.read_snapshot(|store| {
            store.load_genres_matching_inner(source_id, query, offset, limit)
        })
    }
    fn load_genres_matching_inner(
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
        self.attach_genre_representative_albums(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_moods(
        &self,
        source_id: &SourceId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Mood>> {
        self.read_snapshot(|store| store.load_moods_inner(source_id, offset, limit))
    }
    fn load_moods_inner(
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
        self.attach_mood_representative_albums(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_moods_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Mood>> {
        self.read_snapshot(|store| store.load_moods_matching_inner(source_id, query, offset, limit))
    }
    fn load_moods_matching_inner(
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
        self.attach_mood_representative_albums(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_tracks_by_genre_name(
        &self,
        source_id: &SourceId,
        genre_name: &str,
        limit: usize,
    ) -> StoreResult<Vec<Track>> {
        self.read_snapshot(|store| {
            store.load_tracks_by_genre_name_inner(source_id, genre_name, limit)
        })
    }
    fn load_tracks_by_genre_name_inner(
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
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag, t.bpm,
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
        self.read_snapshot(|store| store.load_playlists_inner(source_id, offset, limit))
    }
    fn load_playlists_inner(
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
        self.attach_playlist_representative_albums(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }
    pub fn load_playlists_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Playlist>> {
        self.read_snapshot(|store| {
            store.load_playlists_matching_inner(source_id, query, offset, limit)
        })
    }
    fn load_playlists_matching_inner(
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
            if let PlaylistWriteMode::NativeSync { generation } = mode {
                self.require_current_sync_generation(source_id, generation)?;
            }
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
            Ok(())
        })
    }

    pub fn replace_playlist_snapshot(
        &self,
        source_id: &SourceId,
        playlist: &Playlist,
        entries: &[PlaylistEntry],
        mode: PlaylistWriteMode,
    ) -> StoreResult<()> {
        self.write_batch(|_| {
            let cache_revision = matches!(mode, PlaylistWriteMode::NativeSync { .. })
                .then(|| self.source_cache_revision(source_id))
                .transpose()?;
            self.upsert_playlists_with_mode(source_id, std::slice::from_ref(playlist), mode)?;
            self.upsert_playlist_entries_with_mode(source_id, &playlist.id, entries, mode)?;
            if let Some(cache_revision) = cache_revision {
                self.advance_source_cache_revision(source_id, cache_revision)?;
            }
            Ok(())
        })
    }

    pub(super) fn upsert_playlist_entries_delta(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        entries: &[PlaylistEntry],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let before_playlist = self.load_playlist_for_delta(source_id, playlist_id)?;
        let before = self.playlist_entry_keys(source_id, playlist_id)?;
        let wanted = entries
            .iter()
            .map(|entry| (entry.entry_id.clone(), entry.track.id.clone()))
            .collect::<Vec<_>>();
        if before == wanted {
            return Ok(LibraryDelta::default());
        }
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
        self.read_snapshot(|store| store.load_playlist_detail_inner(source_id, playlist_id))
    }
    fn load_playlist_detail_inner(
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
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag, t.bpm
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
        playlist.representative_albums =
            self.load_playlist_representative_albums(source_id, &playlist.id)?;
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
        self.read_snapshot(|store| store.load_genre_detail_inner(source_id, genre_id))
    }
    fn load_genre_detail_inner(
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
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag, t.bpm
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
        genre.representative_albums =
            self.load_genre_representative_albums(source_id, &genre.id)?;
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
        self.read_snapshot(|store| store.load_mood_detail_inner(source_id, mood_id))
    }
    fn load_mood_detail_inner(
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
                        representative_albums: Vec::new(),
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
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag, t.bpm
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
        mood.representative_albums = self.load_mood_representative_albums(source_id, &mood.name)?;
        Ok(Some(CachedMoodDetail {
            mood,
            albums,
            tracks,
        }))
    }

    fn attach_mood_representative_albums(
        &self,
        source_id: &SourceId,
        moods: &mut [Mood],
    ) -> StoreResult<()> {
        let names = moods
            .iter()
            .map(|mood| mood.name.clone())
            .collect::<Vec<_>>();
        let artwork_by_name = self.load_mood_representative_albums_map(source_id, &names)?;
        for mood in moods {
            mood.representative_albums =
                artwork_by_name.get(&mood.name).cloned().unwrap_or_default();
        }
        Ok(())
    }

    fn load_mood_representative_albums(
        &self,
        source_id: &SourceId,
        mood_name: &str,
    ) -> StoreResult<Vec<AlbumArtwork>> {
        self.load_mood_representative_albums_map(source_id, &[mood_name.to_string()])
            .map(|mut artwork| artwork.remove(mood_name).unwrap_or_default())
    }

    fn load_mood_representative_albums_map(
        &self,
        source_id: &SourceId,
        mood_names: &[String],
    ) -> StoreResult<HashMap<String, Vec<AlbumArtwork>>> {
        let mut artwork_by_name = HashMap::<String, Vec<AlbumArtwork>>::new();
        for chunk in mood_names.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = (0..chunk.len())
                .map(|index| format!("(?{})", index + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                WITH wanted(collection_id) AS (
                    VALUES {placeholders}
                )
                SELECT wanted.collection_id, t.album_id,
                       t.image_item_id, t.image_tag
                FROM wanted
                JOIN track_moods tm ON tm.rowid IN (
                    SELECT candidate.rowid
                    FROM track_moods candidate
                    JOIN tracks candidate_track
                      ON candidate_track.source_id = candidate.source_id
                     AND candidate_track.track_id = candidate.track_id
                    WHERE candidate.source_id = ?1
                      AND candidate.mood_name = wanted.collection_id
                    ORDER BY candidate_track.album COLLATE NOCASE,
                             candidate_track.disc_number,
                             candidate_track.track_number,
                             candidate_track.title COLLATE NOCASE,
                             candidate_track.track_id
                    LIMIT {REPRESENTATIVE_RELATION_WINDOW}
                )
                JOIN tracks t
                  ON t.source_id = tm.source_id AND t.track_id = tm.track_id
                ORDER BY wanted.collection_id, t.album COLLATE NOCASE,
                         t.disc_number, t.track_number,
                         t.title COLLATE NOCASE, t.track_id
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(Value::Text(source_id.as_str().to_string()));
            values.extend(chunk.iter().cloned().map(Value::Text));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = collect_rows(statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AlbumId::new(row.get::<_, String>(1)?),
                    image_ref_from_row(row, 2, 3)?,
                ))
            })?)?;
            append_collection_album_artwork(self, source_id, &mut artwork_by_name, rows)?;
        }
        Ok(artwork_by_name)
    }
    pub fn load_favorite_tracks(&self, source_id: &SourceId) -> StoreResult<Vec<Track>> {
        self.read_snapshot(|store| store.load_favorite_tracks_inner(source_id))
    }
    fn load_favorite_tracks_inner(&self, source_id: &SourceId) -> StoreResult<Vec<Track>> {
        let selected_folder = self.selected_music_folder_id(source_id)?;
        let mut tracks = if let Some(folder_id) = selected_folder.as_ref() {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number,
                       t.track_number, t.image_item_id, t.image_tag, t.bpm
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
                       t.track_number, t.image_item_id, t.image_tag, t.bpm
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
        self.write_batch(|connection| {
            connection.execute(
                "UPDATE albums SET favorite = ?3 WHERE source_id = ?1 AND album_id = ?2",
                params![source_id.as_str(), album_id.as_str(), bool_to_i64(favorite)],
            )?;
            Ok(())
        })
    }
    pub fn set_album_favorite_for_owner(
        &self,
        source_id: &SourceId,
        album_id: &AlbumId,
        favorite: bool,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let cache_revision = (owner == SourceFeatureOwner::Native)
                .then(|| self.source_cache_revision(source_id))
                .transpose()?;
            Self::set_favorite_for_owner(
                connection,
                source_id,
                "album",
                album_id.as_str(),
                favorite,
                owner,
            )?;
            if let Some(cache_revision) = cache_revision {
                self.advance_source_cache_revision(source_id, cache_revision)?;
            }
            Ok(())
        })
    }
    pub fn set_track_favorite(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "UPDATE tracks SET favorite = ?3 WHERE source_id = ?1 AND track_id = ?2",
                params![source_id.as_str(), track_id.as_str(), bool_to_i64(favorite)],
            )?;
            Ok(())
        })
    }
    pub fn set_track_favorite_for_owner(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        favorite: bool,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let cache_revision = (owner == SourceFeatureOwner::Native)
                .then(|| self.source_cache_revision(source_id))
                .transpose()?;
            Self::set_favorite_for_owner(
                connection,
                source_id,
                "track",
                track_id.as_str(),
                favorite,
                owner,
            )?;
            if let Some(cache_revision) = cache_revision {
                self.advance_source_cache_revision(source_id, cache_revision)?;
            }
            Ok(())
        })
    }
    pub fn set_artist_favorite(
        &self,
        source_id: &SourceId,
        artist_id: &ArtistId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "UPDATE artists SET favorite = ?3 WHERE source_id = ?1 AND artist_id = ?2",
                params![
                    source_id.as_str(),
                    artist_id.as_str(),
                    bool_to_i64(favorite)
                ],
            )?;
            connection.execute(
                "UPDATE album_artists SET favorite = ?3 WHERE source_id = ?1 AND artist_id = ?2",
                params![
                    source_id.as_str(),
                    artist_id.as_str(),
                    bool_to_i64(favorite)
                ],
            )?;
            Ok(())
        })
    }
    pub fn set_artist_favorite_for_owner(
        &self,
        source_id: &SourceId,
        artist_id: &ArtistId,
        favorite: bool,
        owner: SourceFeatureOwner,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let cache_revision = (owner == SourceFeatureOwner::Native)
                .then(|| self.source_cache_revision(source_id))
                .transpose()?;
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
            )?;
            if let Some(cache_revision) = cache_revision {
                self.advance_source_cache_revision(source_id, cache_revision)?;
            }
            Ok(())
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
                Self::upsert_favorite_override(connection, source_id, kind, item_id, favorite)
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
        self.write_batch(|_| {
            let cache_revision = (owner == SourceFeatureOwner::Native)
                .then(|| self.source_cache_revision(source_id))
                .transpose()?;
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
            if let Some(cache_revision) = cache_revision {
                self.advance_source_cache_revision(source_id, cache_revision)?;
            }
            Ok(())
        })
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
        self.write_batch(|_| {
            let cache_revision = (owner == SourceFeatureOwner::Native)
                .then(|| self.source_cache_revision(source_id))
                .transpose()?;
            ensure_playlist_owner(&self.connection, source_id, playlist_id, owner)?;
            delete_playlist_rows(
                &self.connection,
                source_id,
                std::slice::from_ref(playlist_id),
                owner,
            )?;
            if let Some(cache_revision) = cache_revision {
                self.advance_source_cache_revision(source_id, cache_revision)?;
            }
            Ok(())
        })
    }
    pub fn save_lyrics_payload(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        origin: &str,
        payload: &str,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
                INSERT INTO lyrics_cache (source_id, track_id, source, value, updated_at)
                VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
                ON CONFLICT(source_id, track_id) DO UPDATE SET
                    source = excluded.source,
                    value = excluded.value,
                    updated_at = excluded.updated_at
                ",
                params![source_id.as_str(), track_id.as_str(), origin, payload],
            )?;
            Ok(())
        })
    }
    pub fn load_lyrics_payload(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<Option<String>> {
        self.connection
            .query_row(
                "
                SELECT value
                FROM lyrics_cache
                WHERE source_id = ?1 AND track_id = ?2
                ",
                params![source_id.as_str(), track_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn delete_lyrics_payload(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        origin: &str,
    ) -> StoreResult<bool> {
        self.write_batch(|connection| {
            let deleted = connection.execute(
                "
                DELETE FROM lyrics_cache
                WHERE source_id = ?1 AND track_id = ?2 AND source = ?3
                ",
                params![source_id.as_str(), track_id.as_str(), origin],
            )?;
            Ok(deleted > 0)
        })
    }
    pub fn search_library(
        &self,
        source_id: &SourceId,
        query: &str,
        limit: usize,
    ) -> StoreResult<SearchResults> {
        self.read_snapshot(|store| store.search_library_inner(source_id, query, limit))
    }
    fn search_library_inner(
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

    pub(super) fn attach_genre_representative_albums(
        &self,
        source_id: &SourceId,
        genres: &mut [Genre],
    ) -> StoreResult<()> {
        let ids = genres
            .iter()
            .map(|genre| genre.id.as_str().to_string())
            .collect::<Vec<_>>();
        let artwork_by_id = self.load_genre_representative_albums_map(source_id, &ids)?;
        for genre in genres {
            genre.representative_albums = artwork_by_id
                .get(genre.id.as_str())
                .cloned()
                .unwrap_or_default();
        }
        Ok(())
    }

    fn load_genre_representative_albums(
        &self,
        source_id: &SourceId,
        genre_id: &GenreId,
    ) -> StoreResult<Vec<AlbumArtwork>> {
        self.load_genre_representative_albums_map(source_id, &[genre_id.as_str().to_string()])
            .map(|mut artwork| artwork.remove(genre_id.as_str()).unwrap_or_default())
    }

    fn load_genre_representative_albums_map(
        &self,
        source_id: &SourceId,
        genre_ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<AlbumArtwork>>> {
        let mut artwork_by_id = HashMap::<String, Vec<AlbumArtwork>>::new();
        for chunk in genre_ids.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = (0..chunk.len())
                .map(|index| format!("?{}", index + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                WITH wanted(collection_id, name) AS (
                    SELECT genre_id, name
                    FROM genres
                    WHERE source_id = ?1
                      AND genre_id IN ({placeholders})
                ), candidates AS (
                    SELECT g.collection_id,
                           a.album_id, NULL AS image_item_id, NULL AS image_tag,
                           0 AS priority, a.title AS title,
                           0 AS disc_number, 0 AS track_number,
                           a.album_id AS stable_id
                    FROM wanted g
                    JOIN albums a ON a.album_id IN (
                        SELECT ag.album_id
                        FROM album_genres ag
                        JOIN albums candidate
                          ON candidate.source_id = ag.source_id
                         AND candidate.album_id = ag.album_id
                        WHERE ag.source_id = ?1
                          AND ag.genre_name = g.name
                        ORDER BY candidate.title COLLATE NOCASE, candidate.album_id
                        LIMIT {REPRESENTATIVE_RELATION_WINDOW}
                    ) AND a.source_id = ?1
                    UNION ALL
                    SELECT g.collection_id,
                           t.album_id, t.image_item_id, t.image_tag,
                           1, t.album, t.disc_number, t.track_number, t.track_id
                    FROM wanted g
                    JOIN tracks t ON t.track_id IN (
                        SELECT tg.track_id
                        FROM track_genres tg
                        JOIN tracks candidate
                          ON candidate.source_id = tg.source_id
                         AND candidate.track_id = tg.track_id
                        WHERE tg.source_id = ?1
                          AND tg.genre_name = g.name
                        ORDER BY candidate.album COLLATE NOCASE,
                                 candidate.disc_number, candidate.track_number,
                                 candidate.title COLLATE NOCASE, candidate.track_id
                        LIMIT {REPRESENTATIVE_RELATION_WINDOW}
                    ) AND t.source_id = ?1
                )
                SELECT collection_id, album_id, image_item_id, image_tag
                FROM candidates
                ORDER BY collection_id, priority, title COLLATE NOCASE,
                         disc_number, track_number, stable_id
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(Value::Text(source_id.as_str().to_string()));
            values.extend(chunk.iter().cloned().map(Value::Text));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = collect_rows(statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AlbumId::new(row.get::<_, String>(1)?),
                    image_ref_from_row(row, 2, 3)?,
                ))
            })?)?;
            append_collection_album_artwork(self, source_id, &mut artwork_by_id, rows)?;
        }
        Ok(artwork_by_id)
    }

    pub(super) fn attach_playlist_representative_albums(
        &self,
        source_id: &SourceId,
        playlists: &mut [Playlist],
    ) -> StoreResult<()> {
        let ids = playlists
            .iter()
            .map(|playlist| playlist.id.as_str().to_string())
            .collect::<Vec<_>>();
        let artwork_by_id = self.load_playlist_representative_albums_map(source_id, &ids)?;
        for playlist in playlists {
            playlist.representative_albums = artwork_by_id
                .get(playlist.id.as_str())
                .cloned()
                .unwrap_or_default();
        }
        Ok(())
    }

    fn load_playlist_representative_albums(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<Vec<AlbumArtwork>> {
        self.load_playlist_representative_albums_map(source_id, &[playlist_id.as_str().to_string()])
            .map(|mut artwork| artwork.remove(playlist_id.as_str()).unwrap_or_default())
    }

    fn load_playlist_representative_albums_map(
        &self,
        source_id: &SourceId,
        playlist_ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<AlbumArtwork>>> {
        let mut artwork_by_id = HashMap::<String, Vec<AlbumArtwork>>::new();
        for chunk in playlist_ids.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let values_placeholders = (0..chunk.len())
                .map(|index| format!("(?{})", index + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                WITH wanted(collection_id) AS (
                    VALUES {values_placeholders}
                )
                SELECT wanted.collection_id, t.album_id,
                       t.image_item_id, t.image_tag
                FROM wanted
                JOIN playlist_tracks pt ON pt.rowid IN (
                    SELECT candidate.rowid
                    FROM playlist_tracks candidate
                    JOIN tracks candidate_track
                      ON candidate_track.source_id = candidate.source_id
                     AND candidate_track.track_id = candidate.track_id
                    WHERE candidate.source_id = ?1
                      AND candidate.playlist_id = wanted.collection_id
                    ORDER BY candidate.position, candidate.entry_id
                    LIMIT {REPRESENTATIVE_RELATION_WINDOW}
                )
                JOIN tracks t
                  ON t.source_id = pt.source_id AND t.track_id = pt.track_id
                ORDER BY wanted.collection_id, pt.position, pt.entry_id
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(Value::Text(source_id.as_str().to_string()));
            values.extend(chunk.iter().cloned().map(Value::Text));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = collect_rows(statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AlbumId::new(row.get::<_, String>(1)?),
                    image_ref_from_row(row, 2, 3)?,
                ))
            })?)?;
            append_collection_album_artwork(self, source_id, &mut artwork_by_id, rows)?;
        }
        Ok(artwork_by_id)
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

fn append_collection_album_artwork(
    store: &Store,
    source_id: &SourceId,
    artwork_by_id: &mut HashMap<String, Vec<AlbumArtwork>>,
    rows: Vec<(String, AlbumId, Option<ImageRef>)>,
) -> StoreResult<()> {
    let album_ids = rows
        .iter()
        .map(|(_, album_id, _)| album_id.clone())
        .collect::<Vec<_>>();
    let album_artwork = store.load_album_artwork_inner(source_id, &album_ids)?;
    for (collection_id, album_id, direct_ref) in rows {
        let Some(mut artwork) = album_artwork.get(&album_id).cloned() else {
            continue;
        };
        if direct_ref.is_some() {
            artwork.image_ref = direct_ref;
        }
        let representatives = artwork_by_id.entry(collection_id).or_default();
        if representatives.len() < REPRESENTATIVE_RELATION_WINDOW
            && !representatives
                .iter()
                .any(|existing| existing.id == artwork.id)
        {
            representatives.push(artwork);
        }
    }
    Ok(())
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

pub(super) fn delete_playlist_rows(
    connection: &Connection,
    source_id: &SourceId,
    playlist_ids: &[PlaylistId],
    owner: SourceFeatureOwner,
) -> StoreResult<()> {
    for playlist_id in playlist_ids {
        let owned = connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM playlists
             WHERE source_id = ?1 AND playlist_id = ?2 AND owner = ?3)",
            params![
                source_id.as_str(),
                playlist_id.as_str(),
                playlist_owner_to_str(owner),
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if !owned {
            continue;
        }
        connection.execute(
            "DELETE FROM playlist_tracks WHERE source_id = ?1 AND playlist_id = ?2",
            params![source_id.as_str(), playlist_id.as_str()],
        )?;
        connection.execute(
            "DELETE FROM playlists WHERE source_id = ?1 AND playlist_id = ?2 AND owner = ?3",
            params![
                source_id.as_str(),
                playlist_id.as_str(),
                playlist_owner_to_str(owner),
            ],
        )?;
        connection.execute(
            "DELETE FROM library_fts
             WHERE source_id = ?1 AND item_type = 'playlist' AND item_id = ?2",
            params![source_id.as_str(), playlist_id.as_str()],
        )?;
    }
    Ok(())
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
