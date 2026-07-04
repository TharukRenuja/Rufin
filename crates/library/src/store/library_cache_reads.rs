use super::library_track_sort::*;
use super::sources::*;
use super::*;

impl Store {
    pub fn load_home_sections(&self, source_id: &SourceId) -> StoreResult<Vec<HomeSection>> {
        let sections = home_section_kinds()
            .into_iter()
            .map(|kind| {
                Ok(HomeSection {
                    kind,
                    albums: self.load_home_section_albums(source_id, kind)?,
                    tracks: self.load_home_section_tracks(source_id, kind)?,
                })
            })
            .collect::<StoreResult<Vec<_>>>()?;
        Ok(sections
            .into_iter()
            .filter(|section| !section.albums.is_empty() || !section.tracks.is_empty())
            .collect())
    }
    pub fn load_home_section_prefetch(
        &self,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Option<HomeSection>> {
        let section = HomeSection {
            kind,
            albums: self.load_home_section_albums_from(
                "home_section_prefetch_items",
                source_id,
                kind,
            )?,
            tracks: self.load_home_section_tracks_from(
                "home_section_prefetch_items",
                source_id,
                kind,
            )?,
        };
        if section.albums.is_empty() && section.tracks.is_empty() {
            Ok(None)
        } else {
            Ok(Some(section))
        }
    }
    pub(super) fn load_home_section_albums(
        &self,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Vec<Album>> {
        self.load_home_section_albums_from("home_section_items", source_id, kind)
    }
    pub(super) fn load_home_section_albums_from(
        &self,
        table: &str,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Vec<Album>> {
        let sql = format!(
            "
            SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date,
                   a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite, a.color_seed,
                   a.image_item_id, a.image_tag
            FROM {table} h
            JOIN albums a
              ON a.source_id = h.source_id
             AND a.album_id = h.item_id
            WHERE h.source_id = ?1
              AND h.section_kind = ?2
              AND h.item_type = 'album'
            ORDER BY h.position
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut albums = collect_rows(statement.query_map(
            params![source_id.as_str(), home_section_kind_key(kind)],
            album_from_row,
        )?)?;
        self.attach_album_metadata(source_id, &mut albums)?;
        Ok(albums)
    }
    pub(super) fn load_home_section_tracks(
        &self,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Vec<Track>> {
        self.load_home_section_tracks_from("home_section_items", source_id, kind)
    }
    pub(super) fn load_home_section_tracks_from(
        &self,
        table: &str,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Vec<Track>> {
        let selected_folder = self.selected_music_folder_id(source_id)?;
        let mut tracks = if let Some(folder_id) = selected_folder.as_ref() {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number, t.track_number,
                       t.image_item_id, t.image_tag, t.local_path, t.source_format
                FROM {table} h
                JOIN tracks t
                  ON t.source_id = h.source_id
                 AND t.track_id = h.item_id
                WHERE h.source_id = ?1
                  AND h.section_kind = ?2
                  AND h.item_type = 'track'
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.source_id = t.source_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?3
                  )
                ORDER BY h.position
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![
                    source_id.as_str(),
                    home_section_kind_key(kind),
                    folder_id.as_str()
                ],
                track_from_row,
            )?)?
        } else {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number, t.track_number,
                       t.image_item_id, t.image_tag
                FROM {table} h
                JOIN tracks t
                  ON t.source_id = h.source_id
                 AND t.track_id = h.item_id
                WHERE h.source_id = ?1
                  AND h.section_kind = ?2
                  AND h.item_type = 'track'
                ORDER BY h.position
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![source_id.as_str(), home_section_kind_key(kind)],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(source_id, &mut tracks)?;
        Ok(tracks)
    }
    pub fn load_albums(
        &self,
        source_id: &SourceId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Album>> {
        let selected_folder = self.selected_music_folder_id(source_id)?;
        let total = if let Some(folder_id) = selected_folder.as_ref() {
            self.count_albums_in_music_folder(source_id, folder_id)?
        } else {
            self.count("albums", source_id)?
        };
        let mut items = if let Some(folder_id) = selected_folder.as_ref() {
            let sql = format!(
                "
                SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date, a.date_added,
                       a.last_played, a.play_count, a.user_rating, a.track_count, a.duration_seconds,
                       {favorite} AS favorite, a.color_seed, a.image_item_id, a.image_tag
                FROM albums a
                WHERE a.source_id = ?1
                  AND EXISTS (
                      SELECT 1
                      FROM tracks t
                      JOIN track_music_folders tmf
                        ON tmf.source_id = t.source_id AND tmf.track_id = t.track_id
                      WHERE t.source_id = a.source_id
                        AND t.album_id = a.album_id
                        AND tmf.folder_id = ?4
                  )
                ORDER BY a.title COLLATE NOCASE
                LIMIT ?2 OFFSET ?3
                ",
                favorite = effective_album_favorite_sql("a"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![
                    source_id.as_str(),
                    limit as i64,
                    offset as i64,
                    folder_id.as_str()
                ],
                album_from_row,
            )?)?
        } else {
            let sql = format!(
                "
                SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date,
                       a.date_added, a.last_played, a.play_count, a.user_rating,
                       a.track_count, a.duration_seconds, {favorite} AS favorite,
                       a.color_seed, a.image_item_id, a.image_tag
                FROM albums a
                WHERE a.source_id = ?1
                ORDER BY title COLLATE NOCASE
                LIMIT ?2 OFFSET ?3
                ",
                favorite = effective_album_favorite_sql("a"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![source_id.as_str(), limit as i64, offset as i64],
                album_from_row,
            )?)?
        };
        self.attach_album_metadata(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }
    pub fn load_albums_by_ids(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
    ) -> StoreResult<Vec<Album>> {
        let mut unique_ids = Vec::<AlbumId>::new();
        for album_id in album_ids {
            if !unique_ids.iter().any(|existing| existing == album_id) {
                unique_ids.push(album_id.clone());
            }
        }
        if unique_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut albums = Vec::new();
        for chunk in unique_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date,
                       a.date_added, a.last_played, a.play_count, a.user_rating,
                       a.track_count, a.duration_seconds, {favorite} AS favorite,
                       a.color_seed, a.image_item_id, a.image_tag
                FROM albums a
                WHERE a.source_id = ?
                  AND a.album_id IN ({placeholders})
                ",
                favorite = effective_album_favorite_sql("a"),
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(source_id.as_str());
            values.extend(chunk.iter().map(AlbumId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            albums.extend(collect_rows(
                statement.query_map(rusqlite::params_from_iter(values), album_from_row)?,
            )?);
        }
        self.attach_album_metadata(source_id, &mut albums)?;
        Ok(albums)
    }
    pub fn load_albums_without_image_ref(
        &self,
        source_id: &SourceId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<Album>> {
        let sql = format!(
            "
            SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date,
                   a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite,
                   a.color_seed, a.image_item_id, a.image_tag
            FROM albums a
            WHERE a.source_id = ?1
              AND a.image_item_id IS NULL
            ORDER BY a.title COLLATE NOCASE, a.album_id
            LIMIT ?2 OFFSET ?3
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut albums = collect_rows(statement.query_map(
            params![source_id.as_str(), limit as i64, offset as i64],
            album_from_row,
        )?)?;
        self.attach_album_metadata(source_id, &mut albums)?;
        Ok(albums)
    }
    pub fn load_albums_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Album>> {
        let Some(pattern) = like_pattern(query) else {
            return self.load_albums(source_id, offset, limit);
        };
        if let Some(query) = fts_query(query) {
            let total = self.count_album_fts_matches(source_id, &query)?;
            if total > 0 {
                return self.search_albums_page(source_id, &query, offset, limit, total);
            }
        }
        self.load_albums_like(source_id, &pattern, offset, limit)
    }
    pub fn load_album_detail(
        &self,
        source_id: &SourceId,
        album_id: &AlbumId,
    ) -> StoreResult<Option<(Album, Vec<Track>)>> {
        let album = self
            .connection
            .query_row(
                &format!(
                    "
                SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date,
                       a.date_added, a.last_played, a.play_count, a.user_rating,
                       a.track_count, a.duration_seconds, {} AS favorite,
                       a.color_seed, a.image_item_id, a.image_tag
                FROM albums a
                WHERE a.source_id = ?1 AND a.album_id = ?2
                ",
                    effective_album_favorite_sql("a")
                ),
                params![source_id.as_str(), album_id.as_str()],
                album_from_row,
            )
            .optional()?;
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
                  AND t.album_id = ?2
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.source_id = t.source_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?3
                  )
                ORDER BY t.disc_number, t.track_number, t.title COLLATE NOCASE
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![source_id.as_str(), album_id.as_str(), folder_id.as_str()],
                track_from_row,
            )?)?
        } else {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number,
                       t.track_number, t.image_item_id, t.image_tag, t.local_path, t.source_format
                FROM tracks t
                WHERE t.source_id = ?1 AND t.album_id = ?2
                ORDER BY t.disc_number, t.track_number, t.title COLLATE NOCASE
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![source_id.as_str(), album_id.as_str()],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(source_id, &mut tracks)?;
        let mut album = match album {
            Some(album) => album,
            None if tracks.is_empty() => return Ok(None),
            None => synthesize_album_from_tracks(album_id, &tracks),
        };
        if selected_folder.is_some() && tracks.is_empty() {
            return Ok(None);
        }
        self.attach_album_metadata(source_id, std::slice::from_mut(&mut album))?;
        Ok(Some((album, tracks)))
    }
    pub fn load_tracks_for_albums(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
    ) -> StoreResult<HashMap<AlbumId, Vec<Track>>> {
        let mut by_album = HashMap::<AlbumId, Vec<Track>>::new();
        if album_ids.is_empty() {
            return Ok(by_album);
        }
        for chunk in album_ids.chunks(200) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number,
                       t.track_number, t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.source_id = ?
                  AND t.album_id IN ({placeholders})
                ORDER BY t.album_id, t.disc_number, t.track_number, t.title COLLATE NOCASE
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(source_id.as_str());
            values.extend(chunk.iter().map(AlbumId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let mut tracks =
                collect_rows(statement.query_map(params_from_iter(values), track_from_row)?)?;
            self.attach_track_metadata(source_id, &mut tracks)?;
            for track in tracks {
                by_album
                    .entry(track.album_id.clone())
                    .or_default()
                    .push(track);
            }
        }
        Ok(by_album)
    }
    pub fn load_artist_detail(
        &self,
        source_id: &SourceId,
        artist_id: &ArtistId,
    ) -> StoreResult<Option<CachedArtistDetail>> {
        let artist = self
            .connection
            .query_row(
                &format!(
                    "
                SELECT a.artist_id, a.name, a.album_count, a.track_count,
                       {} AS favorite, a.last_played, a.play_count, a.user_rating,
                       a.image_item_id, a.image_tag
                FROM artists a
                WHERE a.source_id = ?1 AND a.artist_id = ?2
                ",
                    effective_artist_favorite_sql("a", false)
                ),
                params![source_id.as_str(), artist_id.as_str()],
                artist_from_row,
            )
            .optional()?;
        let artist = match artist {
            Some(artist) => Some(artist),
            None => self
                .connection
                .query_row(
                    &format!(
                        "
                    SELECT a.artist_id, a.name, a.album_count, a.track_count,
                           {} AS favorite, a.last_played, a.play_count, a.user_rating,
                           a.image_item_id, a.image_tag
                    FROM album_artists a
                    WHERE a.source_id = ?1 AND a.artist_id = ?2
                    ",
                        effective_artist_favorite_sql("a", true)
                    ),
                    params![source_id.as_str(), artist_id.as_str()],
                    artist_from_row,
                )
                .optional()?,
        };
        let artist_name_lower = artist
            .as_ref()
            .map(|artist| artist.name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_lowercase);
        let sql = format!(
            "
            SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date,
                   a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite,
                   a.color_seed, a.image_item_id, a.image_tag
            FROM albums a
            WHERE a.source_id = ?1
              AND (
                  a.artist_id = ?2
                  OR EXISTS (
                      SELECT 1
                      FROM album_artist_links aal
                      WHERE aal.source_id = a.source_id
                        AND aal.album_id = a.album_id
                        AND aal.artist_id = ?2
                  )
                  OR (
                      ?3 IS NOT NULL
                      AND LOWER(a.artist) = ?3
                  )
              )
            ORDER BY a.year, a.title COLLATE NOCASE
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut albums_statement = self.connection.prepare(&sql)?;
        let mut albums = collect_rows(albums_statement.query_map(
            params![
                source_id.as_str(),
                artist_id.as_str(),
                artist_name_lower.as_deref()
            ],
            album_from_row,
        )?)?;
        self.attach_album_metadata(source_id, &mut albums)?;
        let sql = format!(
            "
            SELECT DISTINCT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.release_date, t.date_added, t.last_played,
                   t.play_count, t.user_rating, t.duration_seconds, {favorite} AS favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM tracks t
            LEFT JOIN albums a
                ON a.source_id = t.source_id AND a.album_id = t.album_id
            WHERE t.source_id = ?1
              AND (
                  t.artist_id = ?2
                  OR EXISTS (
                      SELECT 1
                      FROM track_artist_links tal
                      WHERE tal.source_id = t.source_id
                        AND tal.track_id = t.track_id
                        AND tal.artist_id = ?2
                  )
                  OR a.artist_id = ?2
                  OR EXISTS (
                      SELECT 1
                      FROM album_artist_links aal
                      WHERE aal.source_id = t.source_id
                        AND aal.album_id = t.album_id
                        AND aal.artist_id = ?2
                  )
                  OR (
                      ?3 IS NOT NULL
                      AND (
                          LOWER(t.artist) = ?3
                          OR LOWER(a.artist) = ?3
                      )
                  )
              )
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                     t.title COLLATE NOCASE
            ",
            favorite = effective_track_favorite_sql("t"),
        );
        let mut tracks_statement = self.connection.prepare(&sql)?;
        let mut tracks = collect_rows(tracks_statement.query_map(
            params![
                source_id.as_str(),
                artist_id.as_str(),
                artist_name_lower.as_deref()
            ],
            track_from_row,
        )?)?;
        self.attach_track_metadata(source_id, &mut tracks)?;
        let appears_on = self.artist_appears_on_albums(
            source_id,
            artist_id,
            artist_name_lower.as_deref(),
            &albums,
            &tracks,
        )?;
        let artist = match artist {
            Some(artist) => artist,
            None if albums.is_empty() && tracks.is_empty() => return Ok(None),
            None => synthesize_artist_from_links(artist_id, &albums, &appears_on, &tracks),
        };
        Ok(Some(CachedArtistDetail {
            artist,
            albums,
            appears_on,
            tracks,
        }))
    }
    pub(super) fn artist_appears_on_albums(
        &self,
        source_id: &SourceId,
        artist_id: &ArtistId,
        artist_name_lower: Option<&str>,
        albums: &[Album],
        tracks: &[Track],
    ) -> StoreResult<Vec<Album>> {
        let mut album_ids = Vec::new();
        let mut statement = self.connection.prepare(
            "
            SELECT DISTINCT album_id
            FROM track_artist_links
            WHERE source_id = ?1 AND artist_id = ?2
            ORDER BY album_id
            ",
        )?;
        let linked_album_ids = collect_rows(
            statement.query_map(params![source_id.as_str(), artist_id.as_str()], |row| {
                row.get::<_, String>(0).map(AlbumId::new)
            })?,
        )?;
        for album_id in linked_album_ids {
            if albums.iter().any(|album| album.id == album_id) || album_ids.contains(&album_id) {
                continue;
            }
            album_ids.push(album_id);
        }
        for track in tracks
            .iter()
            .filter(|track| track_matches_artist(track, artist_id, artist_name_lower))
        {
            if albums.iter().any(|album| album.id == track.album_id)
                || album_ids.contains(&track.album_id)
            {
                continue;
            }
            album_ids.push(track.album_id.clone());
        }
        let mut appears_on = Vec::new();
        for album_id in album_ids {
            let album = match self.load_album_detail(source_id, &album_id)? {
                Some((album, _tracks)) => album,
                None => {
                    let album_tracks = tracks
                        .iter()
                        .filter(|track| track.album_id == album_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    synthesize_album_from_tracks(&album_id, &album_tracks)
                }
            };
            appears_on.push(album);
        }
        appears_on.sort_by(|left, right| {
            left.year
                .cmp(&right.year)
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
        });
        Ok(appears_on)
    }
    pub fn load_tracks(
        &self,
        source_id: &SourceId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        self.load_tracks_sorted(source_id, LibraryField::Title, false, offset, limit)
    }
    pub fn load_tracks_sorted(
        &self,
        source_id: &SourceId,
        sort_key: LibraryField,
        descending: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let selected_folder = self.selected_music_folder_id(source_id)?;
        let total = if let Some(folder_id) = selected_folder.as_ref() {
            self.count_tracks_in_music_folder(source_id, folder_id)?
        } else {
            self.count("tracks", source_id)?
        };
        let order_by = track_order_by_sql("t", sort_key, descending);
        let mut items = if let Some(folder_id) = selected_folder.as_ref() {
            let sql = format!(
                "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {favorite} AS favorite, t.disc_number, t.track_number,
                       t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.source_id = ?1
                  AND EXISTS (
                      SELECT 1
                      FROM track_music_folders tmf
                      WHERE tmf.source_id = t.source_id
                        AND tmf.track_id = t.track_id
                        AND tmf.folder_id = ?4
                  )
                ORDER BY {order_by}
                LIMIT ?2 OFFSET ?3
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![
                    source_id.as_str(),
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
                       t.duration_seconds, {favorite} AS favorite, t.disc_number,
                       t.track_number, t.image_item_id, t.image_tag
                FROM tracks t
                WHERE t.source_id = ?1
                ORDER BY {order_by}
                LIMIT ?2 OFFSET ?3
                ",
                favorite = effective_track_favorite_sql("t"),
            );
            let mut statement = self.connection.prepare(&sql)?;
            collect_rows(statement.query_map(
                params![source_id.as_str(), limit as i64, offset as i64],
                track_from_row,
            )?)?
        };
        self.attach_track_metadata(source_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }
    pub fn load_track(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<Option<Track>> {
        let mut track = self
            .connection
            .query_row(
                &format!(
                    "
                SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                       t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                       t.duration_seconds, {} AS favorite, t.disc_number, t.track_number,
                       t.image_item_id, t.image_tag, t.local_path, t.source_format
                FROM tracks t
                WHERE t.source_id = ?1 AND t.track_id = ?2
                ",
                    effective_track_favorite_sql("t")
                ),
                params![source_id.as_str(), track_id.as_str()],
                track_from_row,
            )
            .optional()?;
        if let Some(track) = track.as_mut() {
            self.attach_track_metadata(source_id, std::slice::from_mut(track))?;
        }
        Ok(track)
    }
    pub fn load_track_ids_with_prefix(
        &self,
        source_id: &SourceId,
        prefix: &str,
    ) -> StoreResult<Vec<TrackId>> {
        let mut statement = self.connection.prepare(
            "
            SELECT track_id
            FROM tracks
            WHERE source_id = ?1
              AND track_id LIKE ?2 || '%'
            ORDER BY track_id
            ",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), prefix], |row| {
                row.get::<_, String>(0).map(TrackId::new)
            })?,
        )
    }
    pub fn tracks_with_prefix_have_album_prefix_mismatch(
        &self,
        source_id: &SourceId,
        track_prefix: &str,
        album_prefix: &str,
    ) -> StoreResult<bool> {
        self.connection
            .query_row(
                "
                SELECT EXISTS(
                    SELECT 1
                    FROM tracks
                    WHERE source_id = ?1
                      AND track_id LIKE ?2 || '%'
                      AND album_id NOT LIKE ?3 || '%'
                )
                ",
                params![source_id.as_str(), track_prefix, album_prefix],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }
    pub fn track_local_path(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<Option<String>> {
        self.connection
            .query_row(
                "
                SELECT local_path
                FROM tracks
                WHERE source_id = ?1 AND track_id = ?2
                ",
                params![source_id.as_str(), track_id.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(StoreError::from)
    }
    pub fn track_source_format(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<Option<String>> {
        self.connection
            .query_row(
                "
                SELECT source_format
                FROM tracks
                WHERE source_id = ?1 AND track_id = ?2
                ",
                params![source_id.as_str(), track_id.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(StoreError::from)
    }
    pub fn load_tracks_for_local_matching(&self, source_id: &SourceId) -> StoreResult<Vec<Track>> {
        let sql = format!(
            "
            SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                   t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                   t.duration_seconds, {favorite} AS favorite, t.disc_number, t.track_number,
                   t.image_item_id, t.image_tag, t.local_path, t.source_format
            FROM tracks t
            WHERE t.source_id = ?1
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number, t.title COLLATE NOCASE
            ",
            favorite = effective_track_favorite_sql("t"),
        );
        let mut statement = self.connection.prepare(&sql)?;
        collect_rows(statement.query_map(params![source_id.as_str()], track_from_row)?)
    }
    pub fn load_tracks_matching(
        &self,
        source_id: &SourceId,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let Some(pattern) = like_pattern(query) else {
            return self.load_tracks(source_id, offset, limit);
        };
        if let Some(query) = fts_query(query) {
            let total = self.count_track_fts_matches(source_id, &query)?;
            if total > 0 {
                return self.search_tracks_page(source_id, &query, offset, limit, total);
            }
        }
        self.load_tracks_like(source_id, &pattern, offset, limit)
    }
    pub fn load_artists(
        &self,
        source_id: &SourceId,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let artist_filter = artist_list_filter_for_alias(album_artist, "a");
        let total = self.count_artists(source_id, album_artist)?;
        let sql = format!(
            "
            SELECT a.artist_id, a.name, a.album_count, a.track_count,
                   {favorite} AS favorite, a.last_played, a.play_count,
                   a.user_rating, a.image_item_id, a.image_tag
            FROM {table} a
            WHERE a.source_id = ?1
              {artist_filter}
            ORDER BY a.name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
            favorite = effective_artist_favorite_sql("a", album_artist),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let items = collect_rows(statement.query_map(
            params![source_id.as_str(), limit as i64, offset as i64],
            artist_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total))
    }
    pub fn load_artists_without_image_ref(
        &self,
        source_id: &SourceId,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let artist_filter = artist_list_filter_for_alias(album_artist, "a");
        let sql = format!(
            "
            SELECT a.artist_id, a.name, a.album_count, a.track_count,
                   {favorite} AS favorite, a.last_played, a.play_count,
                   a.user_rating, a.image_item_id, a.image_tag
            FROM {table} a
            WHERE a.source_id = ?1
              AND a.image_item_id IS NULL
              {artist_filter}
            ORDER BY a.name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
            favorite = effective_artist_favorite_sql("a", album_artist),
        );
        let mut statement = self.connection.prepare(&sql)?;
        collect_rows(statement.query_map(
            params![source_id.as_str(), limit as i64, offset as i64],
            artist_from_row,
        )?)
    }
    pub fn load_artists_matching(
        &self,
        source_id: &SourceId,
        album_artist: bool,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Artist>> {
        let Some(pattern) = like_pattern(query) else {
            return self.load_artists(source_id, album_artist, offset, limit);
        };
        let item_type = if album_artist {
            "album_artist"
        } else {
            "artist"
        };
        if let Some(query) = fts_query(query) {
            let total =
                self.count_artist_fts_matches(source_id, album_artist, item_type, &query)?;
            if total > 0 {
                return self.search_artists_page(
                    source_id,
                    album_artist,
                    &query,
                    offset,
                    limit,
                    total,
                );
            }
        }
        self.load_artists_like(source_id, album_artist, &pattern, offset, limit)
    }
}
