use super::servers::*;
use super::*;

impl Store {
    pub(super) fn count_fts_matches(
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

    pub(super) fn count_track_fts_matches(
        &self,
        server_id: &ServerId,
        query: &str,
    ) -> StoreResult<usize> {
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

    pub(super) fn count_artist_fts_matches(
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

    pub(super) fn count(&self, table: &str, server_id: &ServerId) -> StoreResult<usize> {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE server_id = ?1");
        let count = self
            .connection
            .query_row(&sql, params![server_id.as_str()], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(count.max(0) as usize)
    }

    pub(super) fn count_tracks_in_music_folder(
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

    pub(super) fn count_albums_in_music_folder(
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

    pub(super) fn count_artists(
        &self,
        server_id: &ServerId,
        album_artist: bool,
    ) -> StoreResult<usize> {
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
}
