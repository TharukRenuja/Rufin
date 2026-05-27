impl Store {
    fn search_tracks_page(
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
        query: &str,
        sort_key: LibraryField,
        descending: bool,
        offset: usize,
        limit: usize,
        total: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let selected_folder = self.selected_music_folder_id(server_id)?;
        let order_by = track_order_by_sql("t", sort_key, descending);
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
                    query,
                    limit as i64,
                    offset as i64,
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
                params![server_id.as_str(), query, limit as i64, offset as i64],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(server_id, &mut tracks)?;
        Ok(PagedResponse::new(tracks, total))
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
                    server_id, &query, sort_key, descending, offset, limit, total,
                );
            }
        }
        self.load_tracks_like_sorted(server_id, &pattern, sort_key, descending, offset, limit)
    }
    fn load_tracks_like(
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
    fn load_tracks_like_sorted(
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
    fn search_artists(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Artist>> {
        self.search_artists_page(server_id, false, query, 0, limit, limit)
            .map(|page| page.items)
    }
}
fn track_order_by_sql(alias: &str, field: LibraryField, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let expression = match field {
        LibraryField::TrackNumber => {
            return format!(
                "{alias}.disc_number {direction}, {alias}.track_number {direction}, {}",
                track_tiebreaker_order_sql(alias, direction)
            );
        }
        LibraryField::Artist => format!("{alias}.artist COLLATE NOCASE"),
        LibraryField::AlbumArtist => format!(
            "COALESCE((SELECT aal.name FROM album_artist_links aal WHERE aal.server_id = {alias}.server_id AND aal.album_id = {alias}.album_id ORDER BY aal.position LIMIT 1), {alias}.artist) COLLATE NOCASE"
        ),
        LibraryField::Album => format!("{alias}.album COLLATE NOCASE"),
        LibraryField::Year => format!("{alias}.year"),
        LibraryField::ReleaseDate => format!("{alias}.release_date"),
        LibraryField::DateAdded => format!("{alias}.date_added"),
        LibraryField::LastPlayed => format!("{alias}.last_played"),
        LibraryField::PlayCount => format!("{alias}.play_count"),
        LibraryField::UserRating => format!("{alias}.user_rating"),
        LibraryField::Genre => format!(
            "(SELECT tg.genre_name FROM track_genres tg WHERE tg.server_id = {alias}.server_id AND tg.track_id = {alias}.track_id ORDER BY tg.genre_name COLLATE NOCASE LIMIT 1) COLLATE NOCASE"
        ),
        LibraryField::Duration => format!("{alias}.duration_seconds"),
        LibraryField::Favorite => format!("{alias}.favorite"),
        _ => format!("{alias}.title COLLATE NOCASE"),
    };
    let missing = match field {
        LibraryField::ReleaseDate
        | LibraryField::DateAdded
        | LibraryField::LastPlayed
        | LibraryField::PlayCount
        | LibraryField::UserRating => format!("{expression} IS NULL ASC, "),
        _ => String::new(),
    };
    format!(
        "{missing}{expression} {direction}, {}",
        track_tiebreaker_order_sql(alias, direction)
    )
}
fn track_tiebreaker_order_sql(alias: &str, direction: &str) -> String {
    format!(
        "{alias}.album COLLATE NOCASE {direction}, {alias}.disc_number {direction}, {alias}.track_number {direction}, {alias}.title COLLATE NOCASE {direction}, {alias}.track_id {direction}"
    )
}
impl Store {
    fn search_artists_page(
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
        let mut items = collect_rows(statement.query_map(
            params![
                server_id.as_str(),
                item_type,
                query,
                limit as i64,
                offset as i64
            ],
            artist_from_row,
        )?)?;
        self.attach_artist_fallback_image_refs(server_id, &mut items, album_artist)?;
        Ok(PagedResponse::new(items, total))
    }
    fn load_artists_like(
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
        let mut items = collect_rows(statement.query_map(
            params![server_id.as_str(), pattern, limit as i64, offset as i64],
            artist_from_row,
        )?)?;
        self.attach_artist_fallback_image_refs(server_id, &mut items, album_artist)?;
        Ok(PagedResponse::new(items, total.max(0) as usize))
    }
    fn search_playlists(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Playlist>> {
        self.search_playlists_page(server_id, query, 0, limit, limit)
            .map(|page| page.items)
    }
    fn search_playlists_page(
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
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64, offset as i64],
            playlist_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total))
    }
    fn load_playlists_like(
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
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), pattern, limit as i64, offset as i64],
            playlist_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total.max(0) as usize))
    }
    fn count_fts_matches(
        &self,
        server_id: &ServerId,
        item_type: &str,
        query: &str,
    ) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM library_fts
                WHERE server_id = ?1
                  AND item_type = ?2
                  AND library_fts MATCH ?3
                ",
                params![server_id.as_str(), item_type, query],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as usize)
            .map_err(StoreError::from)
    }
    fn count_track_fts_matches(&self, server_id: &ServerId, query: &str) -> StoreResult<usize> {
        let selected_folder = self.selected_music_folder_id(server_id)?;
        if let Some(folder_id) = selected_folder.as_ref() {
            self.connection
                .query_row(
                    "
                    SELECT COUNT(*)
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
                            AND tmf.folder_id = ?3
                      )
                    ",
                    params![server_id.as_str(), query, folder_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count.max(0) as usize)
                .map_err(StoreError::from)
        } else {
            self.count_fts_matches(server_id, "track", query)
        }
    }
    fn count_artist_fts_matches(
        &self,
        server_id: &ServerId,
        album_artist: bool,
        item_type: &str,
        query: &str,
    ) -> StoreResult<usize> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let artist_filter = artist_list_filter_for_alias(album_artist, "a");
        let sql = format!(
            "
            SELECT COUNT(*)
            FROM library_fts f
            JOIN {table} a
                ON a.server_id = f.server_id AND a.artist_id = f.item_id
            WHERE f.server_id = ?1
              AND f.item_type = ?2
              AND library_fts MATCH ?3
              {artist_filter}
            "
        );
        self.connection
            .query_row(&sql, params![server_id.as_str(), item_type, query], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count.max(0) as usize)
            .map_err(StoreError::from)
    }
    fn attach_album_genres(&self, server_id: &ServerId, albums: &mut [Album]) -> StoreResult<()> {
        if albums.is_empty() {
            return Ok(());
        }
        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let genres = self.load_genre_links(server_id, "album_genres", "album_id", &ids)?;
        for album in albums {
            album.genres = genres.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }
    fn attach_album_metadata(&self, server_id: &ServerId, albums: &mut [Album]) -> StoreResult<()> {
        self.attach_album_genres(server_id, albums)?;
        self.attach_album_track_fallback_image_refs(server_id, albums)?;
        if albums.is_empty() {
            return Ok(());
        }
        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let credits = self.load_artist_links(server_id, "album_artist_links", "album_id", &ids)?;
        for album in albums {
            album.album_artist_credits =
                credits.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }
    fn attach_album_track_fallback_image_refs(
        &self,
        server_id: &ServerId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        let missing_ids = albums
            .iter()
            .filter(|album| album.image_ref.is_none())
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        if missing_ids.is_empty() {
            return Ok(());
        }

        let mut fallback_by_album = HashMap::<String, ImageRef>::new();
        for chunk in missing_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT album_id, image_item_id, image_tag
                FROM tracks
                WHERE server_id = ?
                  AND album_id IN ({placeholders})
                  AND image_item_id IS NOT NULL
                ORDER BY album_id, disc_number, track_number, title COLLATE NOCASE
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (album_id, image_ref) = row?;
                fallback_by_album.entry(album_id).or_insert(image_ref);
            }
        }

        for album in albums {
            if album.image_ref.is_none()
                && let Some(image_ref) = fallback_by_album.remove(album.id.as_str())
            {
                album.image_ref = Some(image_ref);
            }
        }
        Ok(())
    }
    fn attach_track_genres(&self, server_id: &ServerId, tracks: &mut [Track]) -> StoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let genres = self.load_genre_links(server_id, "track_genres", "track_id", &ids)?;
        for track in tracks {
            track.genres = genres.get(track.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }
    fn attach_track_metadata(&self, server_id: &ServerId, tracks: &mut [Track]) -> StoreResult<()> {
        self.attach_track_genres(server_id, tracks)?;
        if tracks.is_empty() {
            return Ok(());
        }
        let track_ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let artist_credits =
            self.load_artist_links(server_id, "track_artist_links", "track_id", &track_ids)?;
        let album_ids = tracks
            .iter()
            .map(|track| track.album_id.as_str().to_string())
            .collect::<Vec<_>>();
        let album_artist_credits =
            self.load_artist_links(server_id, "album_artist_links", "album_id", &album_ids)?;
        for track in tracks {
            track.artist_credits = artist_credits
                .get(track.id.as_str())
                .cloned()
                .unwrap_or_default();
            track.album_artist_credits = album_artist_credits
                .get(track.album_id.as_str())
                .cloned()
                .unwrap_or_default();
        }
        Ok(())
    }
    fn attach_artist_fallback_image_refs(
        &self,
        server_id: &ServerId,
        artists: &mut [Artist],
        album_artist: bool,
    ) -> StoreResult<()> {
        let missing_ids = artists
            .iter()
            .filter(|artist| artist.image_ref.is_none())
            .map(|artist| artist.id.as_str().to_string())
            .collect::<Vec<_>>();
        if missing_ids.is_empty() {
            return Ok(());
        }
        let mut fallback_by_artist = HashMap::<String, ImageRef>::new();
        for chunk in missing_ids.chunks(500) {
            let values_placeholders = std::iter::repeat_n("(?)", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = artist_fallback_image_refs_sql(album_artist, &values_placeholders);
            let mut values = Vec::with_capacity(chunk.len() + if album_artist { 2 } else { 4 });
            values.extend(chunk.iter().map(String::as_str));
            values.extend(std::iter::repeat_n(
                server_id.as_str(),
                if album_artist { 2 } else { 4 },
            ));

            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (artist_id, image_ref) = row?;
                fallback_by_artist.entry(artist_id).or_insert(image_ref);
            }
        }
        for artist in artists {
            if artist.image_ref.is_none()
                && let Some(image_ref) = fallback_by_artist.remove(artist.id.as_str())
            {
                artist.image_ref = Some(image_ref);
            }
        }
        Ok(())
    }
    fn load_genre_links(
        &self,
        server_id: &ServerId,
        table: &str,
        id_column: &str,
        ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<String>>> {
        let mut by_item = HashMap::<String, Vec<String>>::new();
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT {id_column}, genre_name
                FROM {table}
                WHERE server_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY genre_name COLLATE NOCASE
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (item_id, genre_name) = row?;
                by_item.entry(item_id).or_default().push(genre_name);
            }
        }
        Ok(by_item)
    }
    fn load_artist_links(
        &self,
        server_id: &ServerId,
        table: &str,
        id_column: &str,
        ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<ArtistCredit>>> {
        let mut by_item = HashMap::<String, Vec<ArtistCredit>>::new();
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT {id_column}, artist_id, name
                FROM {table}
                WHERE server_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY position
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ArtistCredit {
                        id: ArtistId::new(row.get::<_, String>(1)?),
                        name: row.get::<_, String>(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (item_id, credit) = row?;
                by_item.entry(item_id).or_default().push(credit);
            }
        }
        Ok(by_item)
    }
    fn count(&self, table: &str, server_id: &ServerId) -> StoreResult<usize> {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE server_id = ?1");
        let count = self
            .connection
            .query_row(&sql, params![server_id.as_str()], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(count.max(0) as usize)
    }
    fn count_tracks_in_music_folder(
        &self,
        server_id: &ServerId,
        folder_id: &MusicFolderId,
    ) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM tracks t
                WHERE t.server_id = ?1
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.server_id = t.server_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?2
                  )
                ",
                params![server_id.as_str(), folder_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as usize)
            .map_err(StoreError::from)
    }
    fn count_albums_in_music_folder(
        &self,
        server_id: &ServerId,
        folder_id: &MusicFolderId,
    ) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM albums a
                WHERE a.server_id = ?1
                  AND EXISTS (
                      SELECT 1
                      FROM tracks t
                      JOIN track_music_folders tmf
                        ON tmf.server_id = t.server_id AND tmf.track_id = t.track_id
                      WHERE t.server_id = a.server_id
                        AND t.album_id = a.album_id
                        AND tmf.folder_id = ?2
                  )
                ",
                params![server_id.as_str(), folder_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as usize)
            .map_err(StoreError::from)
    }
    fn count_artists(&self, server_id: &ServerId, album_artist: bool) -> StoreResult<usize> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let artist_filter = artist_list_filter(album_artist);
        let sql = format!(
            "
            SELECT COUNT(*)
            FROM {table}
            WHERE server_id = ?1
              {artist_filter}
            "
        );
        let count = self
            .connection
            .query_row(&sql, params![server_id.as_str()], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(count.max(0) as usize)
    }
    fn prune_missing_items(&self, server_id: &ServerId, generation: i64) -> StoreResult<()> {
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
    fn configure_pragmas(&self, wal: bool) -> StoreResult<()> {
        self.connection.pragma_update(None, "foreign_keys", "ON")?;
        if wal {
            self.connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        Ok(())
    }
    fn write_batch<T>(
        &self,
        operation: impl FnOnce(&Connection) -> StoreResult<T>,
    ) -> StoreResult<T> {
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
