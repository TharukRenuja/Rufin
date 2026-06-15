use super::servers::*;
use super::*;

const EXTERNAL_MUSICBRAINZ_RELEASE_PREFIX: &str = "external:mb-release:";
const EXTERNAL_MUSICBRAINZ_RELEASE_GROUP_PREFIX: &str = "external:mb-release-group:";
const EXTERNAL_ALBUM_IDENTITY_TAG_VERSION: &str = "external-v2";

impl Store {
    pub fn load_album_identity_candidates(
        &self,
        server_id: &ServerId,
        limit: usize,
    ) -> StoreResult<Vec<AlbumIdentityCandidate>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "
            WITH candidates AS (
                SELECT a.album_id, a.title, a.artist,
                       COALESCE(g.value, NULLIF(TRIM(a.musicbrainz_release_group_id), ''))
                         AS musicbrainz_release_group_id,
                       COALESCE(i.value, NULLIF(TRIM(a.musicbrainz_album_id), ''))
                         AS musicbrainz_album_id
                FROM albums a
                LEFT JOIN entity_grouping_keys g
                  ON g.server_id = a.server_id
                 AND g.entity_kind = 'album'
                 AND g.entity_id = a.album_id
                 AND g.namespace = 'musicbrainz:release_group'
                LEFT JOIN entity_identity_keys i
                  ON i.server_id = a.server_id
                 AND i.entity_kind = 'album'
                 AND i.entity_id = a.album_id
                 AND i.namespace = 'musicbrainz:release'
                WHERE a.server_id = ?1
                  AND a.release_types_json = '[]'
            ),
            lookup AS (
                SELECT album_id, title, artist, musicbrainz_album_id,
                       musicbrainz_release_group_id,
                       CASE
                         WHEN TRIM(COALESCE(musicbrainz_release_group_id, '')) <> ''
                           THEN 'release-group:' || TRIM(musicbrainz_release_group_id)
                         ELSE 'release:' || TRIM(musicbrainz_album_id)
                       END AS identity_key
                FROM candidates
                WHERE (
                    TRIM(COALESCE(musicbrainz_release_group_id, '')) <> ''
                    OR TRIM(COALESCE(musicbrainz_album_id, '')) <> ''
                )
            )
            SELECT album_id, title, artist, musicbrainz_album_id,
                   musicbrainz_release_group_id, identity_key
            FROM lookup
            WHERE NOT EXISTS (
                SELECT 1
                FROM entity_resolver_state state
                WHERE state.server_id = ?1
                  AND state.entity_kind = 'album'
                  AND state.purpose = 'release_metadata'
                  AND state.resolver_namespace = 'musicbrainz'
                  AND state.resolver_value = lookup.identity_key
                  AND state.status = 'missing'
              )
            ORDER BY album_id
            LIMIT ?2
            ",
        )?;
        let rows = statement.query_map(params![server_id.as_str(), limit as i64], |row| {
            Ok(AlbumIdentityCandidate {
                album_id: AlbumId::new(row.get::<_, String>(0)?),
                title: row.get(1)?,
                artist: row.get(2)?,
                musicbrainz_album_id: optional_string_column(row, 3)?,
                musicbrainz_release_group_id: optional_string_column(row, 4)?,
                identity_key: row.get(5)?,
            })
        })?;
        collect_rows(rows)
    }

    pub fn load_album_release_type_lookup_candidates(
        &self,
        server_id: &ServerId,
        limit: usize,
    ) -> StoreResult<Vec<AlbumReleaseTypeLookupCandidate>> {
        Ok(self
            .load_album_identity_candidates(server_id, limit)?
            .into_iter()
            .map(|candidate| AlbumReleaseTypeLookupCandidate {
                album_id: candidate.album_id,
                title: candidate.title,
                artist: candidate.artist,
                musicbrainz_album_id: candidate.musicbrainz_album_id,
                musicbrainz_release_group_id: candidate.musicbrainz_release_group_id,
                lookup_key: candidate.identity_key,
            })
            .collect())
    }

    pub fn update_album_release_metadata(
        &self,
        server_id: &ServerId,
        album_id: &AlbumId,
        release_types: &[String],
        is_compilation: Option<bool>,
    ) -> StoreResult<()> {
        self.update_album_identity_metadata(server_id, album_id, release_types, is_compilation)
    }

    pub fn update_album_identity_metadata(
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
        let identity_key = self.album_resolver_key(server_id, album_id)?;
        self.connection.execute(
            "
            INSERT INTO entity_facts (
                server_id, entity_kind, entity_id, fact_key,
                value_json, source, status, updated_at
            )
            VALUES (?1, 'album', ?2, 'release_types', ?3, 'musicbrainz', 'resolved', CURRENT_TIMESTAMP)
            ON CONFLICT(server_id, entity_kind, entity_id, fact_key, source) DO UPDATE SET
                value_json = excluded.value_json,
                status = excluded.status,
                updated_at = excluded.updated_at
            ",
            params![server_id.as_str(), album_id.as_str(), release_types_json],
        )?;
        self.connection.execute(
            "
            DELETE FROM entity_facts
            WHERE server_id = ?1
              AND entity_kind = 'album'
              AND entity_id = ?2
              AND fact_key = 'is_compilation'
              AND source = 'musicbrainz'
            ",
            params![server_id.as_str(), album_id.as_str()],
        )?;
        if let Some(is_compilation) = is_compilation {
            let value_json = if is_compilation == 1 { "true" } else { "false" };
            self.connection.execute(
                "
                INSERT INTO entity_facts (
                    server_id, entity_kind, entity_id, fact_key,
                    value_json, source, status, updated_at
                )
                VALUES (?1, 'album', ?2, 'is_compilation', ?3, 'musicbrainz', 'resolved', CURRENT_TIMESTAMP)
                ON CONFLICT(server_id, entity_kind, entity_id, fact_key, source) DO UPDATE SET
                    value_json = excluded.value_json,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                ",
                params![server_id.as_str(), album_id.as_str(), value_json],
            )?;
        }
        self.connection.execute(
            "
            DELETE FROM entity_resolver_state
            WHERE server_id = ?1
              AND entity_kind = 'album'
              AND purpose = 'release_metadata'
              AND resolver_namespace = 'musicbrainz'
              AND resolver_value = ?2
            ",
            params![server_id.as_str(), identity_key],
        )?;
        if self.table_exists("album_release_type_lookup_misses")? {
            self.connection.execute(
                "
                DELETE FROM album_release_type_lookup_misses
                WHERE server_id = ?1
                  AND album_id = ?2
                ",
                params![server_id.as_str(), album_id.as_str()],
            )?;
        }
        Ok(())
    }

    pub fn save_album_release_type_lookup_miss(
        &self,
        server_id: &ServerId,
        album_id: &AlbumId,
        lookup_key: &str,
        reason: &str,
    ) -> StoreResult<()> {
        self.save_album_identity_miss(server_id, album_id, lookup_key, reason)
    }

    pub fn save_album_identity_miss(
        &self,
        server_id: &ServerId,
        _album_id: &AlbumId,
        identity_key: &str,
        reason: &str,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO entity_resolver_state (
                server_id, entity_kind, purpose, resolver_namespace,
                resolver_value, status, reason, updated_at
            )
            VALUES (?1, 'album', 'release_metadata', 'musicbrainz', ?2, 'missing', ?3, CURRENT_TIMESTAMP)
            ON CONFLICT(
                server_id, entity_kind, purpose, resolver_namespace, resolver_value
            ) DO UPDATE SET
                status = excluded.status,
                reason = excluded.reason,
                updated_at = excluded.updated_at
            ",
            params![
                server_id.as_str(),
                identity_key,
                reason.chars().take(500).collect::<String>()
            ],
        )?;
        Ok(())
    }

    fn album_resolver_key(&self, server_id: &ServerId, album_id: &AlbumId) -> StoreResult<String> {
        Ok(self.connection.query_row(
            "
            SELECT CASE
                     WHEN TRIM(COALESCE(g.value, a.musicbrainz_release_group_id, '')) <> ''
                       THEN 'release-group:' || TRIM(COALESCE(g.value, a.musicbrainz_release_group_id))
                     WHEN TRIM(COALESCE(i.value, a.musicbrainz_album_id, '')) <> ''
                       THEN 'release:' || TRIM(COALESCE(i.value, a.musicbrainz_album_id))
                     ELSE 'album:' || LOWER(TRIM(a.artist)) || ':' || LOWER(TRIM(a.title))
                   END
            FROM albums a
            LEFT JOIN entity_grouping_keys g
              ON g.server_id = a.server_id
             AND g.entity_kind = 'album'
             AND g.entity_id = a.album_id
             AND g.namespace = 'musicbrainz:release_group'
            LEFT JOIN entity_identity_keys i
              ON i.server_id = a.server_id
             AND i.entity_kind = 'album'
             AND i.entity_id = a.album_id
             AND i.namespace = 'musicbrainz:release'
            WHERE a.server_id = ?1
              AND a.album_id = ?2
            ",
            params![server_id.as_str(), album_id.as_str()],
            |row| row.get(0),
        )?)
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
                  AND image_origin IN ('source', 'unknown', 'external')
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

    pub(super) fn bind_album_fallback_image_refs(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<usize> {
        let mut fallback_statement = self.connection.prepare(
            "
            WITH candidates AS (
                SELECT a.album_id, t.image_item_id, t.image_tag,
                       CASE WHEN t.image_item_id LIKE 'external:%' THEN 1 ELSE 0 END AS external_rank,
                       COALESCE(t.disc_number, 0) AS disc_number,
                       COALESCE(t.track_number, 0) AS track_number,
                       t.title, t.track_id
                FROM albums a
                JOIN tracks t
                  ON t.server_id = a.server_id AND t.album_id = a.album_id
                WHERE a.server_id = ?1
                  AND (
                      a.image_item_id IS NULL
                      OR a.image_item_id LIKE 'external:%'
                  )
                  AND t.image_item_id IS NOT NULL
                  AND t.image_origin IN ('source', 'unknown', 'external')
                  AND (
                      a.image_item_id IS NULL
                      OR t.image_item_id NOT LIKE 'external:%'
                  )
            )
            SELECT c.album_id, c.image_item_id, c.image_tag
            FROM candidates c
            WHERE NOT EXISTS (
                  SELECT 1
                  FROM candidates earlier
                  WHERE earlier.album_id = c.album_id
                    AND (
                        earlier.external_rank < c.external_rank
                        OR (
                            earlier.external_rank = c.external_rank
                            AND earlier.disc_number < c.disc_number
                        )
                        OR (
                            earlier.external_rank = c.external_rank
                            AND earlier.disc_number = c.disc_number
                            AND earlier.track_number < c.track_number
                        )
                        OR (
                            earlier.external_rank = c.external_rank
                            AND earlier.disc_number = c.disc_number
                            AND earlier.track_number = c.track_number
                            AND earlier.title COLLATE NOCASE < c.title COLLATE NOCASE
                        )
                        OR (
                            earlier.external_rank = c.external_rank
                            AND earlier.disc_number = c.disc_number
                            AND earlier.track_number = c.track_number
                            AND earlier.title = c.title
                            AND earlier.track_id < c.track_id
                        )
                    )
            )
            ORDER BY c.album_id
            ",
        )?;
        let fallbacks = collect_rows(fallback_statement.query_map(
            params![server_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    optional_string_column(row, 2)?,
                ))
            },
        )?)?;
        if fallbacks.is_empty() {
            return Ok(0);
        }

        let mut bound = 0;
        let mut update_statement = self.connection.prepare(
            "
            UPDATE albums
            SET image_item_id = ?3,
                image_tag = ?4,
                image_origin = 'fallback'
            WHERE server_id = ?1
              AND album_id = ?2
              AND (
                  image_item_id IS NULL
                  OR image_item_id LIKE 'external:%'
              )
              AND (
                  image_item_id IS NOT ?3
                  OR image_tag IS NOT ?4
              )
            ",
        )?;
        for (album_id, image_item_id, image_tag) in fallbacks {
            bound += update_statement.execute(params![
                server_id.as_str(),
                album_id,
                image_item_id,
                image_tag,
            ])?;
        }
        Ok(bound)
    }

    pub(super) fn bind_album_external_identity_image_refs(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<usize> {
        let mut statement = self.connection.prepare(
            "
            SELECT album_id, musicbrainz_release_group_id, musicbrainz_album_id
            FROM albums
            WHERE server_id = ?1
              AND image_item_id IS NULL
              AND (
                  TRIM(COALESCE(musicbrainz_release_group_id, '')) <> ''
                  OR TRIM(COALESCE(musicbrainz_album_id, '')) <> ''
              )
            ORDER BY album_id
            ",
        )?;
        let candidates =
            collect_rows(statement.query_map(params![server_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    optional_string_column(row, 1)?,
                    optional_string_column(row, 2)?,
                ))
            })?)?;
        if candidates.is_empty() {
            return Ok(0);
        }

        let mut bound = 0;
        let mut update_statement = self.connection.prepare(
            "
            UPDATE albums
            SET image_item_id = ?3,
                image_tag = ?4,
                image_origin = 'external'
            WHERE server_id = ?1
              AND album_id = ?2
              AND image_item_id IS NULL
            ",
        )?;
        for (album_id, release_group_id, release_id) in candidates {
            let Some(image_ref) = external_album_identity_image_ref(
                release_group_id.as_deref(),
                release_id.as_deref(),
            ) else {
                continue;
            };
            let (image_item_id, image_tag) = image_ref_parts(Some(&image_ref));
            bound += update_statement.execute(params![
                server_id.as_str(),
                album_id,
                image_item_id,
                image_tag,
            ])?;
        }
        Ok(bound)
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
                  AND image_origin IN ('source', 'unknown', 'external')
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

    pub(super) fn bind_track_album_fallback_image_refs(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<usize> {
        self.connection
            .execute(
                "
                UPDATE tracks
                SET image_item_id = (
                        SELECT a.image_item_id
                        FROM albums a
                        WHERE a.server_id = tracks.server_id
                          AND a.album_id = tracks.album_id
                          AND a.image_item_id IS NOT NULL
                    ),
                    image_tag = (
                        SELECT a.image_tag
                        FROM albums a
                        WHERE a.server_id = tracks.server_id
                          AND a.album_id = tracks.album_id
                          AND a.image_item_id IS NOT NULL
                    ),
                    image_origin = 'fallback'
                WHERE server_id = ?1
                  AND image_item_id IS NULL
                  AND EXISTS (
                      SELECT 1
                      FROM albums a
                      WHERE a.server_id = tracks.server_id
                        AND a.album_id = tracks.album_id
                        AND a.image_item_id IS NOT NULL
                  )
                ",
                params![server_id.as_str()],
            )
            .map_err(Into::into)
    }

    pub(super) fn attach_artist_fallback_image_refs(
        &self,
        server_id: &ServerId,
        artists: &mut [Artist],
        album_artist: bool,
    ) -> StoreResult<()> {
        let missing_ids = artists
            .iter()
            .filter(|artist| repairable_artist_image_ref(artist.image_ref.as_ref()))
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
            let server_params = if album_artist { 7 } else { 6 };
            let mut values = Vec::with_capacity(chunk.len() + server_params);
            values.extend(chunk.iter().map(String::as_str));
            values.extend(std::iter::repeat_n(server_id.as_str(), server_params));

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
            if let Some(image_ref) = fallback_by_artist.remove(artist.id.as_str()) {
                artist.image_ref = Some(image_ref);
            }
        }
        Ok(())
    }

    pub(super) fn bind_artist_fallback_image_refs(
        &self,
        server_id: &ServerId,
        album_artist: bool,
    ) -> StoreResult<usize> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let mut bound = 0;
        let mut offset = 0;
        let mut statement = self.connection.prepare(&format!(
            "
            UPDATE {table}
            SET image_item_id = ?3,
                image_tag = ?4,
                image_origin = 'fallback'
            WHERE server_id = ?1
              AND artist_id = ?2
              AND (
                  image_item_id IS NULL
                  OR image_item_id LIKE 'external:%'
              )
              AND (
                  image_item_id IS NOT ?3
                  OR image_tag IS NOT ?4
              )
            "
        ))?;
        loop {
            let mut artists =
                self.load_artists_repairable_image_ref(server_id, album_artist, offset, 500)?;
            if artists.is_empty() {
                break;
            }
            self.attach_artist_fallback_image_refs(server_id, &mut artists, album_artist)?;
            let mut unchanged = 0;
            for artist in artists {
                let Some(image_ref) = artist.image_ref else {
                    unchanged += 1;
                    continue;
                };
                let (image_item_id, image_tag) = image_ref_parts(Some(&image_ref));
                let changed = statement.execute(params![
                    server_id.as_str(),
                    artist.id.as_str(),
                    image_item_id,
                    image_tag,
                ])?;
                bound += changed;
                if changed == 0 {
                    unchanged += 1;
                }
            }
            offset += unchanged;
        }
        Ok(bound)
    }

    fn load_artists_repairable_image_ref(
        &self,
        server_id: &ServerId,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let artist_filter = artist_list_filter(album_artist);
        let sql = format!(
            "
            SELECT artist_id, name, album_count, track_count, favorite,
                   last_played, play_count, user_rating, image_item_id, image_tag
            FROM {table}
            WHERE server_id = ?1
              AND (
                  image_item_id IS NULL
                  OR image_item_id LIKE 'external:%'
              )
              {artist_filter}
            ORDER BY name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            "
        );
        let mut statement = self.connection.prepare(&sql)?;
        collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            artist_from_row,
        )?)
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
        let entity_kind = match table {
            "track_artist_links" => "artist",
            "album_artist_links" => "album_artist",
            _ => "artist",
        };
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT {id_column}, artist_id, name,
                       (
                           SELECT i.value
                           FROM entity_identity_keys i
                           WHERE i.server_id = {table}.server_id
                             AND i.entity_kind = ?
                             AND i.entity_id = {table}.artist_id
                             AND i.namespace = 'musicbrainz:artist'
                           ORDER BY i.updated_at DESC, i.value
                           LIMIT 1
                       ) AS musicbrainz_artist_id
                FROM {table}
                WHERE server_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY position
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 2);
            values.push(entity_kind);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ArtistCredit {
                        id: ArtistId::new(row.get::<_, String>(1)?),
                        name: row.get::<_, String>(2)?,
                        musicbrainz_artist_id: row.get::<_, Option<String>>(3)?,
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

fn repairable_artist_image_ref(image_ref: Option<&ImageRef>) -> bool {
    image_ref
        .map(|image_ref| image_ref.item_id.starts_with("external:"))
        .unwrap_or(true)
}

fn external_album_identity_image_ref(
    release_group_id: Option<&str>,
    release_id: Option<&str>,
) -> Option<ImageRef> {
    if let Some(group_id) = release_group_id.and_then(valid_external_identity_value) {
        return Some(musicbrainz_image_ref(
            EXTERNAL_MUSICBRAINZ_RELEASE_GROUP_PREFIX,
            group_id,
        ));
    }
    release_id
        .and_then(valid_external_identity_value)
        .map(|release_id| musicbrainz_image_ref(EXTERNAL_MUSICBRAINZ_RELEASE_PREFIX, release_id))
}

fn musicbrainz_image_ref(prefix: &str, id: &str) -> ImageRef {
    let item_id = format!("{prefix}{id}");
    let tag = format!(
        "{EXTERNAL_ALBUM_IDENTITY_TAG_VERSION}-{:016x}",
        stable_album_hash(id, prefix)
    );
    ImageRef::new(item_id, Some(tag))
}

fn valid_external_identity_value(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(value)
}

fn stable_album_hash(artist: &str, album: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in artist
        .as_bytes()
        .iter()
        .copied()
        .chain([0])
        .chain(album.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
