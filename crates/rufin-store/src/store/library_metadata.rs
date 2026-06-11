use super::servers::*;
use super::*;

impl Store {
    pub fn load_album_release_type_lookup_candidates(
        &self,
        server_id: &ServerId,
        limit: usize,
    ) -> StoreResult<Vec<AlbumReleaseTypeLookupCandidate>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "
            SELECT album_id, title, artist, musicbrainz_album_id,
                   musicbrainz_release_group_id, lookup_key
            FROM (
                SELECT album_id, title, artist, musicbrainz_album_id,
                       musicbrainz_release_group_id,
                       CASE
                         WHEN TRIM(COALESCE(musicbrainz_release_group_id, '')) <> ''
                           THEN 'release-group:' || TRIM(musicbrainz_release_group_id)
                         ELSE 'release:' || TRIM(musicbrainz_album_id)
                       END AS lookup_key
                FROM albums
                WHERE server_id = ?1
                  AND release_types_json = '[]'
                  AND (
                    TRIM(COALESCE(musicbrainz_release_group_id, '')) <> ''
                    OR TRIM(COALESCE(musicbrainz_album_id, '')) <> ''
                  )
            ) AS candidates
            WHERE NOT EXISTS (
                SELECT 1
                FROM album_release_type_lookup_misses misses
                WHERE misses.server_id = ?1
                  AND misses.album_id = candidates.album_id
                  AND misses.lookup_key = candidates.lookup_key
              )
            ORDER BY album_id
            LIMIT ?2
            ",
        )?;
        let rows = statement.query_map(params![server_id.as_str(), limit as i64], |row| {
            Ok(AlbumReleaseTypeLookupCandidate {
                album_id: AlbumId::new(row.get::<_, String>(0)?),
                title: row.get(1)?,
                artist: row.get(2)?,
                musicbrainz_album_id: optional_string_column(row, 3)?,
                musicbrainz_release_group_id: optional_string_column(row, 4)?,
                lookup_key: row.get(5)?,
            })
        })?;
        collect_rows(rows)
    }

    pub fn update_album_release_metadata(
        &self,
        server_id: &ServerId,
        album_id: &AlbumId,
        release_types: &[String],
        is_compilation: Option<bool>,
    ) -> StoreResult<()> {
        let release_types_json = album_release_types_json(release_types)?;
        let is_compilation = is_compilation.map(|value| if value { 1_i64 } else { 0_i64 });
        self.connection.execute(
            "
            UPDATE albums
            SET release_types_json = ?3,
                is_compilation = ?4
            WHERE server_id = ?1
              AND album_id = ?2
            ",
            params![
                server_id.as_str(),
                album_id.as_str(),
                release_types_json,
                is_compilation
            ],
        )?;
        self.connection.execute(
            "
            DELETE FROM album_release_type_lookup_misses
            WHERE server_id = ?1
              AND album_id = ?2
            ",
            params![server_id.as_str(), album_id.as_str()],
        )?;
        Ok(())
    }

    pub fn save_album_release_type_lookup_miss(
        &self,
        server_id: &ServerId,
        album_id: &AlbumId,
        lookup_key: &str,
        reason: &str,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO album_release_type_lookup_misses (
                server_id, album_id, lookup_key, reason, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id, album_id, lookup_key) DO UPDATE SET
                reason = excluded.reason,
                updated_at = excluded.updated_at
            ",
            params![
                server_id.as_str(),
                album_id.as_str(),
                lookup_key,
                reason.chars().take(500).collect::<String>()
            ],
        )?;
        Ok(())
    }

    pub(super) fn attach_album_genres(
        &self,
        server_id: &ServerId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
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

    pub(super) fn attach_album_metadata(
        &self,
        server_id: &ServerId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        self.attach_album_genres(server_id, albums)?;
        self.attach_album_release_metadata(server_id, albums)?;
        self.album_track_fallback(server_id, albums)?;
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

    pub(super) fn attach_album_release_metadata(
        &self,
        server_id: &ServerId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        if albums.is_empty() {
            return Ok(());
        }
        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let mut metadata_by_album =
            HashMap::<String, (Vec<String>, Option<bool>, Option<String>, Option<String>)>::new();
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT album_id, release_types_json, is_compilation,
                       musicbrainz_album_id, musicbrainz_release_group_id
                FROM albums
                WHERE server_id = ?
                  AND album_id IN ({placeholders})
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    album_release_types_from_json(row.get::<_, Option<String>>(1)?, 1)?,
                    row.get::<_, Option<i64>>(2)?.map(|value| value == 1),
                    optional_string_column(row, 3)?,
                    optional_string_column(row, 4)?,
                ))
            })?;
            for row in rows {
                let (
                    album_id,
                    release_types,
                    is_compilation,
                    musicbrainz_album_id,
                    musicbrainz_release_group_id,
                ) = row?;
                metadata_by_album.insert(
                    album_id,
                    (
                        release_types,
                        is_compilation,
                        musicbrainz_album_id,
                        musicbrainz_release_group_id,
                    ),
                );
            }
        }

        for album in albums {
            if let Some((
                release_types,
                is_compilation,
                musicbrainz_album_id,
                musicbrainz_release_group_id,
            )) = metadata_by_album.remove(album.id.as_str())
            {
                album.release_types = release_types;
                album.is_compilation = is_compilation;
                album.musicbrainz_album_id = musicbrainz_album_id;
                album.musicbrainz_release_group_id = musicbrainz_release_group_id;
            }
        }
        Ok(())
    }

    pub(super) fn album_track_fallback(
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

    pub fn load_album_image_refs(
        &self,
        server_id: &ServerId,
        album_ids: &[AlbumId],
    ) -> StoreResult<HashMap<AlbumId, ImageRef>> {
        let mut unique_ids = Vec::<AlbumId>::new();
        for album_id in album_ids {
            if !unique_ids.iter().any(|existing| existing == album_id) {
                unique_ids.push(album_id.clone());
            }
        }
        if unique_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut image_refs = HashMap::<AlbumId, ImageRef>::new();
        for chunk in unique_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT album_id, image_item_id, image_tag
                FROM albums
                WHERE server_id = ?
                  AND album_id IN ({placeholders})
                  AND image_item_id IS NOT NULL
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(AlbumId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    AlbumId::new(row.get::<_, String>(0)?),
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (album_id, image_ref) = row?;
                image_refs.entry(album_id).or_insert(image_ref);
            }
        }

        let missing_ids = unique_ids
            .iter()
            .filter(|album_id| !image_refs.contains_key(*album_id))
            .cloned()
            .collect::<Vec<_>>();
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
            values.extend(chunk.iter().map(AlbumId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    AlbumId::new(row.get::<_, String>(0)?),
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (album_id, image_ref) = row?;
                image_refs.entry(album_id).or_insert(image_ref);
            }
        }
        Ok(image_refs)
    }

    pub(super) fn attach_track_genres(
        &self,
        server_id: &ServerId,
        tracks: &mut [Track],
    ) -> StoreResult<()> {
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

    pub(super) fn attach_track_metadata(
        &self,
        server_id: &ServerId,
        tracks: &mut [Track],
    ) -> StoreResult<()> {
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

    pub(super) fn attach_artist_fallback_image_refs(
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

    pub fn load_artist_fallback_albums(
        &self,
        server_id: &ServerId,
        album_artist: bool,
        artist_ids: &[ArtistId],
    ) -> StoreResult<HashMap<ArtistId, Album>> {
        let mut fallback_by_artist = HashMap::<ArtistId, Album>::new();
        if artist_ids.is_empty() {
            return Ok(fallback_by_artist);
        }

        for chunk in artist_ids.chunks(500) {
            let values_placeholders = std::iter::repeat_n("(?)", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = artist_fallback_albums_sql(album_artist, &values_placeholders);
            let mut values = Vec::with_capacity(chunk.len() + if album_artist { 2 } else { 4 });
            values.extend(chunk.iter().map(ArtistId::as_str));
            values.extend(std::iter::repeat_n(
                server_id.as_str(),
                if album_artist { 2 } else { 4 },
            ));

            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    ArtistId::new(row.get::<_, String>(16)?),
                    album_from_row(row)?,
                ))
            })?;
            for row in rows {
                let (artist_id, album) = row?;
                fallback_by_artist.entry(artist_id).or_insert(album);
            }
        }

        Ok(fallback_by_artist)
    }

    pub(super) fn load_genre_links(
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

    pub(super) fn load_artist_links(
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
}
