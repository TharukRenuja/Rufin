use super::sources::collect_rows;
use super::*;

impl Store {
    pub(super) fn expand_artwork_projection_delta(
        &self,
        source_id: &SourceId,
        delta: &mut LibraryDelta,
    ) -> StoreResult<()> {
        let mut track_roots = Vec::new();
        merge_ids(&mut track_roots, delta.tracks.added.clone());
        merge_ids(&mut track_roots, delta.tracks.deleted.clone());
        merge_ids(&mut track_roots, delta.tracks.fields.clone());
        merge_ids(&mut track_roots, delta.tracks.cover_refs.clone());

        let mut collection_track_roots = track_roots.clone();
        merge_ids(&mut collection_track_roots, delta.tracks.metadata.clone());

        let mut album_roots = Vec::new();
        merge_ids(&mut album_roots, delta.albums.added.clone());
        merge_ids(&mut album_roots, delta.albums.deleted.clone());
        merge_ids(&mut album_roots, delta.albums.fields.clone());
        merge_ids(&mut album_roots, delta.albums.links.clone());
        merge_ids(&mut album_roots, delta.albums.cover_refs.clone());
        merge_ids(
            &mut album_roots,
            self.artwork_album_ids_for_tracks(source_id, &track_roots)?,
        );

        let mut artist_cover_roots = Vec::new();
        merge_ids(&mut artist_cover_roots, delta.artists.added.clone());
        merge_ids(&mut artist_cover_roots, delta.artists.deleted.clone());
        merge_ids(&mut artist_cover_roots, delta.artists.cover_refs.clone());
        merge_ids(
            &mut album_roots,
            self.artwork_album_ids_for_artists(source_id, &artist_cover_roots, false)?,
        );

        let mut album_artist_cover_roots = Vec::new();
        merge_ids(
            &mut album_artist_cover_roots,
            delta.album_artists.added.clone(),
        );
        merge_ids(
            &mut album_artist_cover_roots,
            delta.album_artists.deleted.clone(),
        );
        merge_ids(
            &mut album_artist_cover_roots,
            delta.album_artists.cover_refs.clone(),
        );
        merge_ids(
            &mut album_roots,
            self.artwork_album_ids_for_artists(source_id, &album_artist_cover_roots, true)?,
        );

        merge_ids(&mut delta.albums.cover_refs, album_roots.clone());

        let album_tracks = self.artwork_track_ids_for_albums(source_id, &album_roots)?;
        merge_ids(&mut delta.tracks.cover_refs, album_tracks.clone());
        merge_ids(&mut collection_track_roots, album_tracks);

        let mut related_artist_ids = Vec::new();
        merge_ids(&mut related_artist_ids, delta.artists.links.clone());
        merge_ids(&mut related_artist_ids, delta.album_artists.links.clone());
        merge_ids(
            &mut related_artist_ids,
            self.artwork_artist_ids_for_albums(source_id, &album_roots)?,
        );
        merge_ids(
            &mut related_artist_ids,
            self.artwork_artist_ids_for_tracks(source_id, &collection_track_roots)?,
        );
        merge_ids(
            &mut delta.artists.cover_refs,
            self.existing_artwork_artist_ids(source_id, &related_artist_ids, false)?,
        );
        merge_ids(
            &mut delta.album_artists.cover_refs,
            self.existing_artwork_artist_ids(source_id, &related_artist_ids, true)?,
        );

        merge_ids(
            &mut delta.genres.cover_refs,
            self.artwork_genre_ids_for_relations(source_id, &album_roots, &collection_track_roots)?,
        );
        merge_ids(
            &mut delta.playlists.cover_refs,
            self.artwork_playlist_ids_for_tracks(source_id, &collection_track_roots)?,
        );
        if !delta.home_changed
            && self.home_artwork_depends_on(source_id, &album_roots, &collection_track_roots)?
        {
            delta.home_changed = true;
        }
        Ok(())
    }

    fn artwork_album_ids_for_tracks(
        &self,
        source_id: &SourceId,
        track_ids: &[TrackId],
    ) -> StoreResult<Vec<AlbumId>> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&track_ids.iter().map(TrackId::as_str).collect::<Vec<_>>())?;
        let mut statement = self.connection.prepare(
            "WITH wanted(track_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             )
             SELECT DISTINCT t.album_id
             FROM wanted w
             CROSS JOIN tracks t
             WHERE t.source_id = ?1 AND t.track_id = w.track_id
             ORDER BY t.album_id",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(AlbumId::new)
            })?,
        )
    }

    fn artwork_album_ids_for_artists(
        &self,
        source_id: &SourceId,
        artist_ids: &[ArtistId],
        album_artist: bool,
    ) -> StoreResult<Vec<AlbumId>> {
        if artist_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&artist_ids.iter().map(ArtistId::as_str).collect::<Vec<_>>())?;
        let sql = if album_artist {
            "WITH wanted(artist_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             ), candidates(album_id) AS (
                 SELECT a.album_id
                 FROM wanted w
                 CROSS JOIN albums a
                 WHERE a.source_id = ?1 AND a.artist_id = w.artist_id
                 UNION
                 SELECT aal.album_id
                 FROM wanted w
                 CROSS JOIN album_artist_links aal
                 WHERE aal.source_id = ?1 AND aal.artist_id = w.artist_id
             )
             SELECT album_id FROM candidates ORDER BY album_id"
        } else {
            "WITH wanted(artist_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             )
             SELECT DISTINCT a.album_id
             FROM wanted w
             CROSS JOIN albums a
             WHERE a.source_id = ?1 AND a.artist_id = w.artist_id
             ORDER BY a.album_id"
        };
        let mut statement = self.connection.prepare(sql)?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(AlbumId::new)
            })?,
        )
    }

    fn artwork_track_ids_for_albums(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
    ) -> StoreResult<Vec<TrackId>> {
        if album_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&album_ids.iter().map(AlbumId::as_str).collect::<Vec<_>>())?;
        let mut statement = self.connection.prepare(
            "WITH wanted(album_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             )
             SELECT t.track_id
             FROM wanted w
             CROSS JOIN tracks t
             WHERE t.source_id = ?1 AND t.album_id = w.album_id
             ORDER BY t.track_id",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(TrackId::new)
            })?,
        )
    }

    fn artwork_artist_ids_for_albums(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
    ) -> StoreResult<Vec<ArtistId>> {
        if album_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&album_ids.iter().map(AlbumId::as_str).collect::<Vec<_>>())?;
        let mut statement = self.connection.prepare(
            "WITH wanted(album_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             ), candidates(artist_id) AS (
                 SELECT a.artist_id
                 FROM wanted w
                 CROSS JOIN albums a
                 WHERE a.source_id = ?1 AND a.album_id = w.album_id
                   AND a.artist_id IS NOT NULL
                 UNION
                 SELECT aal.artist_id
                 FROM wanted w
                 CROSS JOIN album_artist_links aal
                 WHERE aal.source_id = ?1 AND aal.album_id = w.album_id
             )
             SELECT artist_id FROM candidates ORDER BY artist_id",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(ArtistId::new)
            })?,
        )
    }

    fn artwork_artist_ids_for_tracks(
        &self,
        source_id: &SourceId,
        track_ids: &[TrackId],
    ) -> StoreResult<Vec<ArtistId>> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&track_ids.iter().map(TrackId::as_str).collect::<Vec<_>>())?;
        let mut statement = self.connection.prepare(
            "WITH wanted(track_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             ), candidates(artist_id) AS (
                 SELECT t.artist_id
                 FROM wanted w
                 CROSS JOIN tracks t
                 WHERE t.source_id = ?1 AND t.track_id = w.track_id
                   AND t.artist_id IS NOT NULL
                 UNION
                 SELECT tal.artist_id
                 FROM wanted w
                 CROSS JOIN track_artist_links tal
                 WHERE tal.source_id = ?1 AND tal.track_id = w.track_id
             )
             SELECT artist_id FROM candidates ORDER BY artist_id",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(ArtistId::new)
            })?,
        )
    }

    fn existing_artwork_artist_ids(
        &self,
        source_id: &SourceId,
        artist_ids: &[ArtistId],
        album_artist: bool,
    ) -> StoreResult<Vec<ArtistId>> {
        if artist_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&artist_ids.iter().map(ArtistId::as_str).collect::<Vec<_>>())?;
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let sql = format!(
            "WITH wanted(artist_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             )
             SELECT a.artist_id
             FROM wanted w
             CROSS JOIN {table} a
             WHERE a.source_id = ?1 AND a.artist_id = w.artist_id
             ORDER BY a.artist_id"
        );
        let mut statement = self.connection.prepare(&sql)?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(ArtistId::new)
            })?,
        )
    }

    fn artwork_genre_ids_for_relations(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
        track_ids: &[TrackId],
    ) -> StoreResult<Vec<GenreId>> {
        if album_ids.is_empty() && track_ids.is_empty() {
            return Ok(Vec::new());
        }
        let albums =
            serde_json::to_string(&album_ids.iter().map(AlbumId::as_str).collect::<Vec<_>>())?;
        let tracks =
            serde_json::to_string(&track_ids.iter().map(TrackId::as_str).collect::<Vec<_>>())?;
        let mut statement = self.connection.prepare(
            "WITH wanted_albums(album_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             ), wanted_tracks(track_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?3)
             ), names(name) AS (
                 SELECT ag.genre_name
                 FROM wanted_albums w
                 CROSS JOIN album_genres ag
                 WHERE ag.source_id = ?1 AND ag.album_id = w.album_id
                 UNION
                 SELECT tg.genre_name
                 FROM wanted_tracks w
                 CROSS JOIN track_genres tg
                 WHERE tg.source_id = ?1 AND tg.track_id = w.track_id
             )
             SELECT DISTINCT g.genre_id
             FROM names n
             CROSS JOIN genres g
             WHERE g.source_id = ?1 AND g.name = n.name COLLATE NOCASE
             ORDER BY g.genre_id",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), albums, tracks], |row| {
                row.get::<_, String>(0).map(GenreId::new)
            })?,
        )
    }

    fn artwork_playlist_ids_for_tracks(
        &self,
        source_id: &SourceId,
        track_ids: &[TrackId],
    ) -> StoreResult<Vec<PlaylistId>> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&track_ids.iter().map(TrackId::as_str).collect::<Vec<_>>())?;
        let mut statement = self.connection.prepare(
            "WITH wanted(track_id) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             )
             SELECT DISTINCT pt.playlist_id
             FROM wanted w
             CROSS JOIN playlist_tracks pt
             WHERE pt.source_id = ?1 AND pt.track_id = w.track_id
             ORDER BY pt.playlist_id",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(PlaylistId::new)
            })?,
        )
    }

    fn home_artwork_depends_on(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
        track_ids: &[TrackId],
    ) -> StoreResult<bool> {
        if album_ids.is_empty() && track_ids.is_empty() {
            return Ok(false);
        }
        let albums =
            serde_json::to_string(&album_ids.iter().map(AlbumId::as_str).collect::<Vec<_>>())?;
        let tracks =
            serde_json::to_string(&track_ids.iter().map(TrackId::as_str).collect::<Vec<_>>())?;
        for table in ["home_section_items", "home_section_prefetch_items"] {
            let sql = format!(
                "SELECT EXISTS(
                     SELECT 1 FROM {table} h
                     WHERE h.source_id = ?1
                       AND (
                           (h.item_type = 'album' AND h.item_id IN (
                               SELECT CAST(value AS TEXT) FROM json_each(?2)
                           ))
                           OR
                           (h.item_type = 'track' AND h.item_id IN (
                               SELECT CAST(value AS TEXT) FROM json_each(?3)
                           ))
                       )
                 )"
            );
            if self.connection.query_row(
                &sql,
                params![source_id.as_str(), albums, tracks],
                |row| row.get::<_, bool>(0),
            )? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn genre_ids_for_names(
        &self,
        source_id: &SourceId,
        names: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> StoreResult<Vec<GenreId>> {
        let mut unique = Vec::<String>::new();
        let mut seen = HashSet::<String>::new();
        for name in names {
            let name = name.as_ref().trim();
            if !name.is_empty() && seen.insert(name.to_string()) {
                unique.push(name.to_string());
            }
        }
        if unique.is_empty() {
            return Ok(Vec::new());
        }
        let wanted = serde_json::to_string(&unique)?;
        let mut statement = self.connection.prepare(
            "WITH wanted(name) AS (
                 SELECT CAST(value AS TEXT) FROM json_each(?2)
             )
             SELECT g.genre_id
             FROM wanted w
             CROSS JOIN genres g
             WHERE g.source_id = ?1 AND g.name = w.name COLLATE NOCASE
             ORDER BY g.genre_id",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), wanted], |row| {
                row.get::<_, String>(0).map(GenreId::new)
            })?,
        )
    }
}
