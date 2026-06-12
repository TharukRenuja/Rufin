use super::library_track_sort::*;
use super::servers::*;
use super::*;

struct SortedTrackSearchPage<'a> {
    query: &'a str,
    sort_key: LibraryField,
    descending: bool,
    offset: usize,
    limit: usize,
    total: usize,
}

impl Store {
    pub(super) fn search_tracks_page(
        &self,
        server_id: &ServerId,
        query: &str,
        offset: usize,
        limit: usize,
        total: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let selected_folder = self.selected_music_folder_id(server_id)?;
        let mut tracks = if let Some(folder_id) = selected_folder.as_ref() {
            let mut statement = self.connection.prepare(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                       t.album, t.year, t.release_date, t.date_added, t.last_played,
                       t.play_count, t.user_rating, t.duration_seconds, t.favorite,
                       t.disc_number, t.track_number, t.image_item_id, t.image_tag
                FROM library_fts f
                JOIN tracks t
                    ON t.server_id = f.server_id AND t.track_id = f.item_id
                WHERE f.server_id = ?1
                  AND f.item_type = 'track'
                  AND library_fts MATCH ?2
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.server_id = t.server_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?5
                  )
                ORDER BY bm25(library_fts)
                LIMIT ?3 OFFSET ?4
                ",
            )?;
            collect_rows(statement.query_map(
                params![
                    server_id.as_str(),
                    query,
                    limit as i64,
                    offset as i64,
                    folder_id.as_str()
                ],
                track_from_row,
            )?)?
        } else {
            let mut statement = self.connection.prepare(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                       t.album, t.year, t.release_date, t.date_added, t.last_played,
                       t.play_count, t.user_rating, t.duration_seconds, t.favorite,
                       t.disc_number, t.track_number, t.image_item_id, t.image_tag
                FROM library_fts f
                JOIN tracks t
                    ON t.server_id = f.server_id AND t.track_id = f.item_id
                WHERE f.server_id = ?1
                  AND f.item_type = 'track'
                  AND library_fts MATCH ?2
                ORDER BY bm25(library_fts)
                LIMIT ?3 OFFSET ?4
                ",
            )?;
            collect_rows(statement.query_map(
                params![server_id.as_str(), query, limit as i64, offset as i64],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(server_id, &mut tracks)?;
        Ok(PagedResponse::new(tracks, total))
    }
    fn search_tracks_page_sorted(
        &self,
        server_id: &ServerId,
        page: SortedTrackSearchPage<'_>,
    ) -> StoreResult<PagedResponse<Track>> {
        let selected_folder = self.selected_music_folder_id(server_id)?;
        let order_by = track_order_by_sql("t", page.sort_key, page.descending);
        let mut tracks = if let Some(folder_id) = selected_folder.as_ref() {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                       t.album, t.year, t.release_date, t.date_added, t.last_played,
                       t.play_count, t.user_rating, t.duration_seconds, t.favorite,
                       t.disc_number, t.track_number, t.image_item_id, t.image_tag
                FROM library_fts f
                JOIN tracks t
                    ON t.server_id = f.server_id AND t.track_id = f.item_id
                WHERE f.server_id = ?1
                  AND f.item_type = 'track'
                  AND library_fts MATCH ?2
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.server_id = t.server_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?5
                  )
                ORDER BY {order_by}
                LIMIT ?3 OFFSET ?4
                "
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![
                    server_id.as_str(),
                    page.query,
                    page.limit as i64,
                    page.offset as i64,
                    folder_id.as_str()
                ],
                track_from_row,
            )?)?
        } else {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                       t.album, t.year, t.release_date, t.date_added, t.last_played,
                       t.play_count, t.user_rating, t.duration_seconds, t.favorite,
                       t.disc_number, t.track_number, t.image_item_id, t.image_tag
                FROM library_fts f
                JOIN tracks t
                    ON t.server_id = f.server_id AND t.track_id = f.item_id
                WHERE f.server_id = ?1
                  AND f.item_type = 'track'
                  AND library_fts MATCH ?2
                ORDER BY {order_by}
                LIMIT ?3 OFFSET ?4
                "
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![
                    server_id.as_str(),
                    page.query,
                    page.limit as i64,
                    page.offset as i64
                ],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(server_id, &mut tracks)?;
        Ok(PagedResponse::new(tracks, page.total))
    }
    pub fn load_tracks_matching_sorted(
        &self,
        server_id: &ServerId,
        query: &str,
        sort_key: LibraryField,
        descending: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let Some(pattern) = like_pattern(query) else {
            return self.load_tracks_sorted(server_id, sort_key, descending, offset, limit);
        };
        if let Some(query) = fts_query(query) {
            let total = self.count_track_fts_matches(server_id, &query)?;
            if total > 0 {
                return self.search_tracks_page_sorted(
                    server_id,
                    SortedTrackSearchPage {
                        query: &query,
                        sort_key,
                        descending,
                        offset,
                        limit,
                        total,
                    },
                );
            }
        }
        self.load_tracks_like_sorted(server_id, &pattern, sort_key, descending, offset, limit)
    }
    pub(super) fn load_tracks_like(
        &self,
        server_id: &ServerId,
        pattern: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let selected_folder = self.selected_music_folder_id(server_id)?;
        let total = if let Some(folder_id) = selected_folder.as_ref() {
            self.connection.query_row(
                "
                SELECT COUNT(*)
                FROM tracks t
                WHERE t.server_id = ?1
                  AND (
                      LOWER(t.title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.album) LIKE ?2 ESCAPE '\\'
                      OR CAST(t.year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.server_id = t.server_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?3
                  )
                ",
                params![server_id.as_str(), pattern, folder_id.as_str()],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            self.connection.query_row(
                "
                SELECT COUNT(*)
                FROM tracks
                WHERE server_id = ?1
                  AND (
                      LOWER(title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(album) LIKE ?2 ESCAPE '\\'
                      OR CAST(year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                ",
                params![server_id.as_str(), pattern],
                |row| row.get::<_, i64>(0),
            )?
        };
        let mut tracks = if let Some(folder_id) = selected_folder.as_ref() {
            let mut statement = self.connection.prepare(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, t.favorite, t.disc_number, t.track_number, t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.server_id = ?1
                  AND (
                      LOWER(t.title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.album) LIKE ?2 ESCAPE '\\'
                      OR CAST(t.year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.server_id = t.server_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?5
                  )
                ORDER BY t.title COLLATE NOCASE
                LIMIT ?3 OFFSET ?4
                ",
            )?;
            collect_rows(statement.query_map(
                params![
                    server_id.as_str(),
                    pattern,
                    limit as i64,
                    offset as i64,
                    folder_id.as_str()
                ],
                track_from_row,
            )?)?
        } else {
            let mut statement = self.connection.prepare(
                "
                SELECT track_id, album_id, title, artist, artist_id, album, year,
                       release_date, date_added, last_played, play_count, user_rating,
                       duration_seconds, favorite, disc_number, track_number, image_item_id, image_tag
                FROM tracks
                WHERE server_id = ?1
                  AND (
                      LOWER(title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(album) LIKE ?2 ESCAPE '\\'
                      OR CAST(year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                ORDER BY title COLLATE NOCASE
                LIMIT ?3 OFFSET ?4
                ",
            )?;
            collect_rows(statement.query_map(
                params![server_id.as_str(), pattern, limit as i64, offset as i64],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(server_id, &mut tracks)?;
        Ok(PagedResponse::new(tracks, total.max(0) as usize))
    }
    pub(super) fn load_tracks_like_sorted(
        &self,
        server_id: &ServerId,
        pattern: &str,
        sort_key: LibraryField,
        descending: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let selected_folder = self.selected_music_folder_id(server_id)?;
        let total = if let Some(folder_id) = selected_folder.as_ref() {
            self.connection.query_row(
                "
                SELECT COUNT(*)
                FROM tracks t
                WHERE t.server_id = ?1
                  AND (
                      LOWER(t.title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.album) LIKE ?2 ESCAPE '\\'
                      OR CAST(t.year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.server_id = t.server_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?3
                  )
                ",
                params![server_id.as_str(), pattern, folder_id.as_str()],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            self.connection.query_row(
                "
                SELECT COUNT(*)
                FROM tracks
                WHERE server_id = ?1
                  AND (
                      LOWER(title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(album) LIKE ?2 ESCAPE '\\'
                      OR CAST(year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                ",
                params![server_id.as_str(), pattern],
                |row| row.get::<_, i64>(0),
            )?
        };
        let order_by = track_order_by_sql("t", sort_key, descending);
        let mut tracks = if let Some(folder_id) = selected_folder.as_ref() {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, t.favorite, t.disc_number, t.track_number, t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.server_id = ?1
                  AND (
                      LOWER(t.title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.album) LIKE ?2 ESCAPE '\\'
                      OR CAST(t.year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.server_id = t.server_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?5
                  )
                ORDER BY {order_by}
                LIMIT ?3 OFFSET ?4
                "
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![
                    server_id.as_str(),
                    pattern,
                    limit as i64,
                    offset as i64,
                    folder_id.as_str()
                ],
                track_from_row,
            )?)?
        } else {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, t.favorite, t.disc_number, t.track_number, t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.server_id = ?1
                  AND (
                      LOWER(t.title) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.artist) LIKE ?2 ESCAPE '\\'
                      OR LOWER(t.album) LIKE ?2 ESCAPE '\\'
                      OR CAST(t.year AS TEXT) LIKE ?2 ESCAPE '\\'
                  )
                ORDER BY {order_by}
                LIMIT ?3 OFFSET ?4
                "
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![server_id.as_str(), pattern, limit as i64, offset as i64],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(server_id, &mut tracks)?;
        Ok(PagedResponse::new(tracks, total.max(0) as usize))
    }
    pub(super) fn search_artists(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Artist>> {
        self.search_artists_page(server_id, false, query, 0, limit, limit)
            .map(|page| page.items)
    }
}
impl Store {
    pub(super) fn search_artists_page(
        &self,
        server_id: &ServerId,
        album_artist: bool,
        query: &str,
        offset: usize,
        limit: usize,
        total: usize,
    ) -> StoreResult<PagedResponse<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let item_type = if album_artist {
            "album_artist"
        } else {
            "artist"
        };
        let artist_filter = artist_list_filter_for_alias(album_artist, "a");
        let sql = format!(
            "
            SELECT a.artist_id, a.name, a.album_count, a.track_count, a.favorite,
                   a.last_played, a.play_count, a.user_rating, a.image_item_id, a.image_tag
            FROM library_fts f
            JOIN {table} a
                ON a.server_id = f.server_id AND a.artist_id = f.item_id
            WHERE f.server_id = ?1
              AND f.item_type = ?2
              AND library_fts MATCH ?3
              {artist_filter}
            ORDER BY bm25(library_fts)
            LIMIT ?4 OFFSET ?5
            "
        );
        let mut statement = self.connection.prepare(&sql)?;
        let items = collect_rows(statement.query_map(
            params![
                server_id.as_str(),
                item_type,
                query,
                limit as i64,
                offset as i64
            ],
            artist_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total))
    }
    pub(super) fn load_artists_like(
        &self,
        server_id: &ServerId,
        album_artist: bool,
        pattern: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let artist_filter = artist_list_filter(album_artist);
        let total_sql = format!(
            "
            SELECT COUNT(*)
            FROM {table}
            WHERE server_id = ?1
              AND LOWER(name) LIKE ?2 ESCAPE '\\'
              {artist_filter}
            "
        );
        let total =
            self.connection
                .query_row(&total_sql, params![server_id.as_str(), pattern], |row| {
                    row.get::<_, i64>(0)
                })?;
        let sql = format!(
            "
            SELECT artist_id, name, album_count, track_count, favorite,
                   last_played, play_count, user_rating, image_item_id, image_tag
            FROM {table}
            WHERE server_id = ?1
              AND LOWER(name) LIKE ?2 ESCAPE '\\'
              {artist_filter}
            ORDER BY name COLLATE NOCASE
            LIMIT ?3 OFFSET ?4
            "
        );
        let mut statement = self.connection.prepare(&sql)?;
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), pattern, limit as i64, offset as i64],
            artist_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total.max(0) as usize))
    }
    pub(super) fn search_playlists(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Playlist>> {
        self.search_playlists_page(server_id, query, 0, limit, limit)
            .map(|page| page.items)
    }
    pub(super) fn search_playlists_page(
        &self,
        server_id: &ServerId,
        query: &str,
        offset: usize,
        limit: usize,
        total: usize,
    ) -> StoreResult<PagedResponse<Playlist>> {
        let mut statement = self.connection.prepare(
            "
            SELECT p.playlist_id, p.name, p.track_count, p.duration_seconds,
                   p.image_item_id, p.image_tag
            FROM library_fts f
            JOIN playlists p
                ON p.server_id = f.server_id AND p.playlist_id = f.item_id
            WHERE f.server_id = ?1
              AND f.item_type = 'playlist'
              AND library_fts MATCH ?2
            ORDER BY bm25(library_fts)
            LIMIT ?3 OFFSET ?4
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64, offset as i64],
            playlist_from_row,
        )?)?;
        self.attach_playlist_cover_image_refs(server_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }
    pub(super) fn load_playlists_like(
        &self,
        server_id: &ServerId,
        pattern: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Playlist>> {
        let total = self.connection.query_row(
            "
            SELECT COUNT(*)
            FROM playlists
            WHERE server_id = ?1
              AND LOWER(name) LIKE ?2 ESCAPE '\\'
            ",
            params![server_id.as_str(), pattern],
            |row| row.get::<_, i64>(0),
        )?;
        let mut statement = self.connection.prepare(
            "
            SELECT playlist_id, name, track_count, duration_seconds, image_item_id, image_tag
            FROM playlists
            WHERE server_id = ?1
              AND LOWER(name) LIKE ?2 ESCAPE '\\'
            ORDER BY name COLLATE NOCASE
            LIMIT ?3 OFFSET ?4
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![server_id.as_str(), pattern, limit as i64, offset as i64],
            playlist_from_row,
        )?)?;
        self.attach_playlist_cover_image_refs(server_id, &mut items)?;
        Ok(PagedResponse::new(items, total.max(0) as usize))
    }
    pub(super) fn prune_missing_items(
        &self,
        server_id: &ServerId,
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            for table in [
                "albums",
                "tracks",
                "artists",
                "album_artists",
                "genres",
                "album_genres",
                "track_genres",
                "album_artist_links",
                "track_artist_links",
                "server_music_folders",
                "track_music_folders",
                "playlists",
                "playlist_tracks",
                "home_section_items",
            ] {
                let sql =
                    format!("DELETE FROM {table} WHERE server_id = ?1 AND sync_generation < ?2");
                connection.execute(&sql, params![server_id.as_str(), generation])?;
            }
            prune_missing_entity_rows(connection, server_id)?;

            connection.execute(
                "
                UPDATE server_library_preferences
                SET selected_music_folder_id = NULL,
                    updated_at = CURRENT_TIMESTAMP
                WHERE server_id = ?1
                  AND selected_music_folder_id IS NOT NULL
                  AND selected_music_folder_id NOT IN (
                      SELECT folder_id
                      FROM server_music_folders
                      WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;

            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type = 'album'
                  AND item_id NOT IN (
                    SELECT album_id FROM albums WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type = 'track'
                  AND item_id NOT IN (
                    SELECT track_id FROM tracks WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type IN ('artist', 'album_artist')
                  AND item_id NOT IN (
                    SELECT artist_id FROM artists WHERE server_id = ?1
                    UNION
                    SELECT artist_id FROM album_artists WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type = 'playlist'
                  AND item_id NOT IN (
                    SELECT playlist_id FROM playlists WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            Ok(())
        })
    }
    pub(super) fn configure_pragmas(&self, wal: bool) -> StoreResult<()> {
        self.connection.pragma_update(None, "foreign_keys", "ON")?;
        if wal {
            self.connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        Ok(())
    }
    pub(super) fn write_batch<T>(
        &self,
        operation: impl FnOnce(&Connection) -> StoreResult<T>,
    ) -> StoreResult<T> {
        if !self.connection.is_autocommit() {
            return operation(&self.connection);
        }
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let result = operation(&self.connection);
        match result {
            Ok(value) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _rollback = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}

fn prune_missing_entity_rows(connection: &Connection, server_id: &ServerId) -> StoreResult<()> {
    for (entity_kind, table, id_column) in [
        ("track", "tracks", "track_id"),
        ("album", "albums", "album_id"),
        ("artist", "artists", "artist_id"),
        ("album_artist", "album_artists", "artist_id"),
    ] {
        for entity_table in [
            "entity_content_refs",
            "entity_facts",
            "entity_grouping_keys",
            "entity_identity_keys",
            "entity_links",
            "entities",
        ] {
            let sql = format!(
                "
                DELETE FROM {entity_table}
                WHERE server_id = ?1
                  AND entity_kind = ?2
                  AND NOT EXISTS (
                      SELECT 1
                      FROM {table}
                      WHERE {table}.server_id = {entity_table}.server_id
                        AND {table}.{id_column} = {entity_table}.entity_id
                  )
                "
            );
            connection.execute(&sql, params![server_id.as_str(), entity_kind])?;
        }
    }
    Ok(())
}
