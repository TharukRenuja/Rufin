use super::sources::*;
use super::*;

impl Store {
    pub fn load_album_identity_candidates(
        &self,
        source_id: &SourceId,
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
                  ON g.source_id = a.source_id
                 AND g.entity_kind = 'album'
                 AND g.entity_id = a.album_id
                 AND g.namespace = 'musicbrainz:release_group'
                LEFT JOIN entity_identity_keys i
                  ON i.source_id = a.source_id
                 AND i.entity_kind = 'album'
                 AND i.entity_id = a.album_id
                 AND i.namespace = 'musicbrainz:release'
                WHERE a.source_id = ?1
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
                WHERE state.source_id = ?1
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
        let rows = statement.query_map(params![source_id.as_str(), limit as i64], |row| {
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

    pub fn update_album_identity_metadata(
        &self,
        source_id: &SourceId,
        album_id: &AlbumId,
        release_types: &[String],
        is_compilation: Option<bool>,
    ) -> StoreResult<()> {
        self.write_batch(|_| {
            let release_types_json = album_release_types_json(release_types)?;
            let is_compilation = is_compilation.map(|value| if value { 1_i64 } else { 0_i64 });
            self.connection.execute(
            "
            UPDATE albums
            SET release_types_json = ?3,
                is_compilation = ?4
            WHERE source_id = ?1
              AND album_id = ?2
            ",
            params![
                source_id.as_str(),
                album_id.as_str(),
                release_types_json,
                is_compilation
            ],
            )?;
            let identity_key = self.album_resolver_key(source_id, album_id)?;
            self.connection.execute(
            "
            INSERT INTO entity_facts (
                source_id, entity_kind, entity_id, fact_key,
                value_json, source, status, updated_at
            )
            VALUES (?1, 'album', ?2, 'release_types', ?3, 'musicbrainz', 'resolved', CURRENT_TIMESTAMP)
            ON CONFLICT(source_id, entity_kind, entity_id, fact_key, source) DO UPDATE SET
                value_json = excluded.value_json,
                status = excluded.status,
                updated_at = excluded.updated_at
            ",
            params![source_id.as_str(), album_id.as_str(), release_types_json],
            )?;
            self.connection.execute(
            "
            DELETE FROM entity_facts
            WHERE source_id = ?1
              AND entity_kind = 'album'
              AND entity_id = ?2
              AND fact_key = 'is_compilation'
              AND source = 'musicbrainz'
            ",
            params![source_id.as_str(), album_id.as_str()],
            )?;
            if let Some(is_compilation) = is_compilation {
                let value_json = if is_compilation == 1 { "true" } else { "false" };
                self.connection.execute(
                "
                INSERT INTO entity_facts (
                    source_id, entity_kind, entity_id, fact_key,
                    value_json, source, status, updated_at
                )
                VALUES (?1, 'album', ?2, 'is_compilation', ?3, 'musicbrainz', 'resolved', CURRENT_TIMESTAMP)
                ON CONFLICT(source_id, entity_kind, entity_id, fact_key, source) DO UPDATE SET
                    value_json = excluded.value_json,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                ",
                params![source_id.as_str(), album_id.as_str(), value_json],
                )?;
            }
            self.connection.execute(
            "
            DELETE FROM entity_resolver_state
            WHERE source_id = ?1
              AND entity_kind = 'album'
              AND purpose = 'release_metadata'
              AND resolver_namespace = 'musicbrainz'
              AND resolver_value = ?2
            ",
            params![source_id.as_str(), identity_key],
            )?;
            if self.table_exists("album_release_type_lookup_misses")? {
                self.connection.execute(
                "
                DELETE FROM album_release_type_lookup_misses
                WHERE source_id = ?1
                  AND album_id = ?2
                ",
                params![source_id.as_str(), album_id.as_str()],
                )?;
            }
            Ok(())
        })
    }

    pub fn save_album_identity_miss(
        &self,
        source_id: &SourceId,
        _album_id: &AlbumId,
        identity_key: &str,
        reason: &str,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
                INSERT INTO entity_resolver_state (
                    source_id, entity_kind, purpose, resolver_namespace,
                    resolver_value, status, reason, updated_at
                )
                VALUES (?1, 'album', 'release_metadata', 'musicbrainz', ?2, 'missing', ?3, CURRENT_TIMESTAMP)
                ON CONFLICT(
                    source_id, entity_kind, purpose, resolver_namespace, resolver_value
                ) DO UPDATE SET
                    status = excluded.status,
                    reason = excluded.reason,
                    updated_at = excluded.updated_at
                ",
                params![
                    source_id.as_str(),
                    identity_key,
                    reason.chars().take(500).collect::<String>()
                ],
            )?;
            Ok(())
        })
    }

    fn album_resolver_key(&self, source_id: &SourceId, album_id: &AlbumId) -> StoreResult<String> {
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
              ON g.source_id = a.source_id
             AND g.entity_kind = 'album'
             AND g.entity_id = a.album_id
             AND g.namespace = 'musicbrainz:release_group'
            LEFT JOIN entity_identity_keys i
              ON i.source_id = a.source_id
             AND i.entity_kind = 'album'
             AND i.entity_id = a.album_id
             AND i.namespace = 'musicbrainz:release'
            WHERE a.source_id = ?1
              AND a.album_id = ?2
            ",
            params![source_id.as_str(), album_id.as_str()],
            |row| row.get(0),
        )?)
    }

    pub(super) fn attach_album_genres(
        &self,
        source_id: &SourceId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        if albums.is_empty() {
            return Ok(());
        }
        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let genres = self.load_genre_links(source_id, "album_genres", "album_id", &ids)?;
        for album in albums {
            album.genres = genres.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_album_metadata(
        &self,
        source_id: &SourceId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        self.attach_album_genres(source_id, albums)?;
        self.attach_album_release_metadata(source_id, albums)?;
        self.attach_album_image_refs(source_id, albums)?;
        if albums.is_empty() {
            return Ok(());
        }
        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let credits = self.load_artist_links(source_id, "album_artist_links", "album_id", &ids)?;
        for album in albums {
            album.album_artist_credits =
                credits.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_album_release_metadata(
        &self,
        source_id: &SourceId,
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
                WHERE source_id = ?
                  AND album_id IN ({placeholders})
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(source_id.as_str());
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

    pub(super) fn load_album_artwork_inner(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
    ) -> StoreResult<HashMap<AlbumId, AlbumArtwork>> {
        let mut seen = HashSet::new();
        let mut unique_ids = Vec::<AlbumId>::new();
        for album_id in album_ids {
            if seen.insert(album_id.clone()) {
                unique_ids.push(album_id.clone());
            }
        }
        if unique_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut artwork = HashMap::<AlbumId, AlbumArtwork>::new();
        for chunk in unique_ids.chunks(500) {
            let placeholders = (0..chunk.len())
                .map(|index| format!("(?{})", index + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                WITH wanted(album_id) AS (VALUES {placeholders}),
                candidates AS (
                    SELECT a.album_id, a.image_item_id, a.image_tag,
                           0 AS priority, 0 AS disc_number, 0 AS track_number,
                           a.title AS title, a.album_id AS stable_id
                    FROM wanted w
                    CROSS JOIN albums a
                    WHERE a.source_id = ?1 AND a.album_id = w.album_id
                      AND a.image_item_id IS NOT NULL
                    UNION ALL
                    SELECT a.album_id, t.image_item_id, t.image_tag,
                           1, COALESCE(t.disc_number, 0), COALESCE(t.track_number, 0),
                           t.title, t.track_id
                    FROM wanted w
                    CROSS JOIN albums a
                    CROSS JOIN tracks t
                    WHERE a.source_id = ?1 AND a.album_id = w.album_id
                      AND t.source_id = a.source_id AND t.album_id = a.album_id
                      AND t.image_item_id IS NOT NULL
                    UNION ALL
                    SELECT a.album_id, ar.image_item_id, ar.image_tag,
                           2, 0, 0, ar.name, ar.artist_id
                    FROM wanted w
                    CROSS JOIN albums a
                    CROSS JOIN artists ar
                    WHERE a.source_id = ?1 AND a.album_id = w.album_id
                      AND ar.source_id = a.source_id AND ar.artist_id = a.artist_id
                      AND ar.image_item_id IS NOT NULL
                    UNION ALL
                    SELECT a.album_id, aa.image_item_id, aa.image_tag,
                           4, 0, 0, aa.name, aa.artist_id
                    FROM wanted w
                    CROSS JOIN albums a
                    CROSS JOIN album_artists aa
                    WHERE a.source_id = ?1 AND a.album_id = w.album_id
                      AND aa.source_id = a.source_id AND aa.artist_id = a.artist_id
                      AND aa.image_item_id IS NOT NULL
                    UNION ALL
                    SELECT a.album_id, aa.image_item_id, aa.image_tag,
                           3, 0, aal.position, aa.name, aa.artist_id
                    FROM wanted w
                    CROSS JOIN albums a
                    CROSS JOIN album_artist_links aal
                    CROSS JOIN album_artists aa
                    WHERE a.source_id = ?1 AND a.album_id = w.album_id
                      AND aal.source_id = a.source_id AND aal.album_id = a.album_id
                      AND aa.source_id = aal.source_id AND aa.artist_id = aal.artist_id
                      AND aa.image_item_id IS NOT NULL
                ), ranked AS (
                    SELECT album_id, image_item_id, image_tag,
                           ROW_NUMBER() OVER (
                               PARTITION BY album_id
                               ORDER BY priority, disc_number, track_number,
                                        title COLLATE NOCASE, stable_id
                           ) AS position
                    FROM candidates
                )
                SELECT a.album_id, a.title, a.artist, r.image_item_id, r.image_tag,
                       a.musicbrainz_album_id, a.musicbrainz_release_group_id
                FROM wanted w
                CROSS JOIN albums a
                LEFT JOIN ranked r ON r.album_id = a.album_id AND r.position = 1
                WHERE a.source_id = ?1 AND a.album_id = w.album_id
                ORDER BY a.album_id
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(source_id.as_str());
            values.extend(chunk.iter().map(AlbumId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    AlbumId::new(row.get::<_, String>(0)?),
                    AlbumArtwork {
                        id: AlbumId::new(row.get::<_, String>(0)?),
                        title: row.get(1)?,
                        artist: row.get(2)?,
                        image_ref: image_ref_from_row(row, 3, 4)?,
                        musicbrainz_album_id: optional_string_column(row, 5)?,
                        musicbrainz_release_group_id: optional_string_column(row, 6)?,
                    },
                ))
            })?;
            for row in rows {
                let (album_id, album_artwork) = row?;
                artwork.entry(album_id).or_insert(album_artwork);
            }
        }
        Ok(artwork)
    }

    fn attach_album_image_refs(
        &self,
        source_id: &SourceId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        let album_ids = albums
            .iter()
            .filter(|album| album.image_ref.is_none())
            .map(|album| album.id.clone())
            .collect::<Vec<_>>();
        if album_ids.is_empty() {
            return Ok(());
        }
        let mut artwork = self.load_album_artwork_inner(source_id, &album_ids)?;
        for album in albums {
            if album.image_ref.is_none()
                && let Some(image_ref) = artwork
                    .remove(&album.id)
                    .and_then(|artwork| artwork.image_ref)
            {
                album.image_ref = Some(image_ref);
            }
        }
        Ok(())
    }

    pub(super) fn attach_artist_representative_albums(
        &self,
        source_id: &SourceId,
        artists: &mut [Artist],
    ) -> StoreResult<()> {
        let artist_ids = artists
            .iter()
            .map(|artist| artist.id.as_str())
            .collect::<Vec<_>>();
        if artist_ids.is_empty() {
            return Ok(());
        }

        let wanted = serde_json::to_string(&artist_ids)?;
        let mut statement = self.connection.prepare(
            "
            WITH wanted(artist_id) AS (
                    SELECT CAST(value AS TEXT) FROM json_each(?2)
                 ),
                 candidates AS (
                    SELECT w.artist_id, a.album_id, 0 AS priority, a.year, a.title
                    FROM wanted w
                    CROSS JOIN albums a
                    WHERE a.source_id = ?1 AND a.artist_id = w.artist_id
                    UNION ALL
                    SELECT w.artist_id, a.album_id, 1 AS priority, a.year, a.title
                    FROM wanted w
                    CROSS JOIN album_artist_links aal
                    CROSS JOIN albums a
                    WHERE aal.source_id = ?1 AND aal.artist_id = w.artist_id
                      AND a.source_id = aal.source_id AND a.album_id = aal.album_id
                    UNION ALL
                    SELECT w.artist_id, a.album_id, 2 AS priority, a.year, a.title
                    FROM wanted w
                    CROSS JOIN tracks t
                    CROSS JOIN albums a
                    WHERE t.source_id = ?1 AND t.artist_id = w.artist_id
                      AND a.source_id = t.source_id AND a.album_id = t.album_id
                    UNION ALL
                    SELECT w.artist_id, a.album_id, 3 AS priority, a.year, a.title
                    FROM wanted w
                    CROSS JOIN track_artist_links tal
                    CROSS JOIN tracks t
                    CROSS JOIN albums a
                    WHERE tal.source_id = ?1 AND tal.artist_id = w.artist_id
                      AND t.source_id = tal.source_id AND t.track_id = tal.track_id
                      AND a.source_id = t.source_id AND a.album_id = t.album_id
                 ),
                 distinct_candidates AS (
                    SELECT artist_id, album_id, MIN(priority) AS priority,
                           MIN(year) AS year, MIN(title) AS title
                    FROM candidates
                    GROUP BY artist_id, album_id
                 ),
                 ranked AS (
                    SELECT artist_id, album_id,
                           ROW_NUMBER() OVER (
                               PARTITION BY artist_id
                               ORDER BY priority, year, title COLLATE NOCASE, album_id
                           ) AS position
                    FROM distinct_candidates
                 )
            SELECT artist_id, album_id
            FROM ranked
            WHERE position <= 4
            ORDER BY artist_id, position
            ",
        )?;
        let rows = statement.query_map(params![source_id.as_str(), wanted], |row| {
            Ok((
                ArtistId::new(row.get::<_, String>(0)?),
                AlbumId::new(row.get::<_, String>(1)?),
            ))
        })?;
        let mut albums_by_artist = HashMap::<ArtistId, Vec<AlbumId>>::new();
        for (artist_id, album_id) in collect_rows(rows)? {
            albums_by_artist
                .entry(artist_id)
                .or_default()
                .push(album_id);
        }
        let album_ids = albums_by_artist
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        let artwork = self.load_album_artwork_inner(source_id, &album_ids)?;
        for artist in artists {
            artist.representative_albums = albums_by_artist
                .remove(&artist.id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|album_id| artwork.get(&album_id).cloned())
                .collect();
        }
        Ok(())
    }

    pub(super) fn attach_track_genres(
        &self,
        source_id: &SourceId,
        tracks: &mut [Track],
    ) -> StoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let genres = self.load_genre_links(source_id, "track_genres", "track_id", &ids)?;
        for track in tracks {
            track.genres = genres.get(track.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_track_moods(
        &self,
        source_id: &SourceId,
        tracks: &mut [Track],
    ) -> StoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let moods =
            self.load_named_links(source_id, "track_moods", "track_id", "mood_name", &ids)?;
        for track in tracks {
            track.moods = moods.get(track.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_track_metadata(
        &self,
        source_id: &SourceId,
        tracks: &mut [Track],
    ) -> StoreResult<()> {
        self.attach_track_genres(source_id, tracks)?;
        self.attach_track_moods(source_id, tracks)?;
        if tracks.is_empty() {
            return Ok(());
        }
        let mut album_ids = tracks
            .iter()
            .map(|track| track.album_id.clone())
            .collect::<Vec<_>>();
        album_ids.sort_unstable();
        album_ids.dedup();
        let album_artwork = self.load_album_artwork_inner(source_id, &album_ids)?;
        let track_ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let artist_credits =
            self.load_artist_links(source_id, "track_artist_links", "track_id", &track_ids)?;
        let mut album_ids = tracks
            .iter()
            .map(|track| track.album_id.as_str().to_string())
            .collect::<Vec<_>>();
        album_ids.sort_unstable();
        album_ids.dedup();
        let album_artist_credits =
            self.load_artist_links(source_id, "album_artist_links", "album_id", &album_ids)?;
        for track in tracks {
            if let Some(artwork) = album_artwork.get(&track.album_id) {
                track.album_artwork = Some(artwork.clone());
            }
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

    pub fn hydrate_tracks(&self, source_id: &SourceId, tracks: &mut [Track]) -> StoreResult<()> {
        self.read_snapshot(|store| store.attach_track_metadata(source_id, tracks))
    }

    pub(super) fn load_genre_links(
        &self,
        source_id: &SourceId,
        table: &str,
        id_column: &str,
        ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<String>>> {
        self.load_named_links(source_id, table, id_column, "genre_name", ids)
    }

    pub(super) fn load_named_links(
        &self,
        source_id: &SourceId,
        table: &str,
        id_column: &str,
        name_column: &str,
        ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<String>>> {
        let mut by_item = HashMap::<String, Vec<String>>::new();
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT {id_column}, {name_column}
                FROM {table}
                WHERE source_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY {name_column} COLLATE NOCASE
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(source_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (item_id, name) = row?;
                by_item.entry(item_id).or_default().push(name);
            }
        }
        Ok(by_item)
    }

    pub(super) fn load_artist_links(
        &self,
        source_id: &SourceId,
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
                           WHERE i.source_id = {table}.source_id
                             AND i.entity_kind = ?
                             AND i.entity_id = {table}.artist_id
                             AND i.namespace = 'musicbrainz:artist'
                           ORDER BY i.updated_at DESC, i.value
                           LIMIT 1
                       ) AS musicbrainz_artist_id
                FROM {table}
                WHERE source_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY position
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 2);
            values.push(entity_kind);
            values.push(source_id.as_str());
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
