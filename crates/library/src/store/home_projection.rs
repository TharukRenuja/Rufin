use super::sources::*;
use super::*;
use crate::{HomeGenre, HomeOverview};

impl Store {
    pub(super) fn create_home_projection_schema(&self) -> StoreResult<()> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS home_projection_state (
                source_id TEXT PRIMARY KEY REFERENCES sources(source_id) ON DELETE CASCADE,
                pending_sync_mask INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        Ok(())
    }

    pub(super) fn home_write_mask(&self, source_id: &SourceId) -> StoreResult<i64> {
        self.connection
            .query_row(
                "SELECT pending_sync_mask
                 FROM home_projection_state
                 WHERE source_id = ?1",
                params![source_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|mask| mask.unwrap_or(0))
            .map_err(StoreError::from)
    }

    pub(super) fn mark_home_write(
        &self,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<()> {
        self.connection.execute(
            "INSERT INTO home_projection_state (source_id, pending_sync_mask)
             VALUES (?1, ?2)
             ON CONFLICT(source_id) DO UPDATE SET
                 pending_sync_mask = pending_sync_mask | excluded.pending_sync_mask",
            params![source_id.as_str(), home_section_kind_mask(kind)],
        )?;
        Ok(())
    }

    pub(super) fn clear_home_write_mask(&self, source_id: &SourceId) -> StoreResult<()> {
        self.connection.execute(
            "DELETE FROM home_projection_state WHERE source_id = ?1",
            params![source_id.as_str()],
        )?;
        Ok(())
    }

    pub(crate) fn load_home_overview_projection(
        &self,
        source_id: &SourceId,
        genre_limit: usize,
    ) -> StoreResult<HomeOverview> {
        let sections = self.load_home_sections_inner(source_id)?;
        let genres = self.load_home_genres(source_id, genre_limit)?;
        let showcase_fallback = if sections.iter().any(|section| !section.albums.is_empty()) {
            None
        } else {
            self.load_sparse_home_album(source_id, &sections)?
        };
        Ok(HomeOverview {
            sections,
            genres,
            showcase_fallback,
        })
    }

    pub(super) fn load_home_showcase_fallback(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<Option<Album>> {
        let has_home_album = self.connection.query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM home_section_items
                 WHERE source_id = ?1 AND item_type = 'album'
             )",
            params![source_id.as_str()],
            |row| row.get::<_, bool>(0),
        )?;
        if has_home_album {
            return Ok(None);
        }
        self.load_sparse_home_album(source_id, &self.load_home_sections_inner(source_id)?)
    }

    fn load_sparse_home_album(
        &self,
        source_id: &SourceId,
        sections: &[HomeSection],
    ) -> StoreResult<Option<Album>> {
        let home_track_album_id = sections
            .iter()
            .flat_map(|section| section.tracks.iter())
            .map(|track| track.album_id.clone())
            .next();
        let album_id = match home_track_album_id {
            Some(album_id) => Some(album_id),
            None => self
                .connection
                .query_row(
                    "SELECT album_id FROM albums
                     WHERE source_id = ?1
                     ORDER BY date_added DESC, title COLLATE NOCASE, album_id
                     LIMIT 1",
                    params![source_id.as_str()],
                    |row| row.get::<_, String>(0).map(AlbumId::new),
                )
                .optional()?,
        };
        let Some(album_id) = album_id else {
            return Ok(None);
        };
        Ok(self
            .load_albums_by_ids_inner(source_id, std::slice::from_ref(&album_id))?
            .into_iter()
            .next())
    }

    fn load_home_sections_inner(&self, source_id: &SourceId) -> StoreResult<Vec<HomeSection>> {
        let album_sql = format!(
            "
            SELECT a.album_id, a.title, a.artist, a.artist_id, a.year, a.release_date,
                   a.date_added, a.last_played, a.play_count, a.user_rating,
                   a.track_count, a.duration_seconds, {favorite} AS favorite, a.color_seed,
                   a.image_item_id, a.image_tag, h.section_kind
            FROM home_section_items h
            JOIN albums a
              ON a.source_id = h.source_id
             AND a.album_id = h.item_id
            WHERE h.source_id = ?1
              AND h.item_type = 'album'
            ORDER BY h.section_kind, h.position
            ",
            favorite = effective_album_favorite_sql("a"),
        );
        let mut album_statement = self.connection.prepare(&album_sql)?;
        let album_rows = collect_rows(
            album_statement.query_map(params![source_id.as_str()], |row| {
                Ok((row.get::<_, String>(16)?, album_from_row(row)?))
            })?,
        )?;

        let track_sql = format!(
            "
            SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                   t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
                   t.duration_seconds, {favorite} AS favorite, t.disc_number, t.track_number,
                   t.image_item_id, t.image_tag, t.bpm, t.local_path, t.source_format,
                   t.comment, t.skip_count, h.section_kind
            FROM home_section_items h
            JOIN tracks t
              ON t.source_id = h.source_id
             AND t.track_id = h.item_id
            LEFT JOIN source_library_preferences preferences
              ON preferences.source_id = t.source_id
            WHERE h.source_id = ?1
              AND h.item_type = 'track'
              AND (preferences.selected_music_folder_id IS NULL OR EXISTS (
                  SELECT 1
                  FROM track_music_folders tmf
                  WHERE tmf.source_id = t.source_id
                    AND tmf.track_id = t.track_id
                    AND tmf.folder_id = preferences.selected_music_folder_id
              ))
            ORDER BY h.section_kind, h.position
            ",
            favorite = effective_track_favorite_sql("t"),
        );
        let mut track_statement = self.connection.prepare(&track_sql)?;
        let track_rows = collect_rows(
            track_statement.query_map(params![source_id.as_str()], |row| {
                Ok((row.get::<_, String>(23)?, track_from_row(row)?))
            })?,
        )?;

        let mut seen_albums = HashSet::new();
        let mut albums = album_rows
            .iter()
            .filter(|(_, album)| seen_albums.insert(album.id.clone()))
            .map(|(_, album)| album.clone())
            .collect::<Vec<_>>();
        let mut seen_tracks = HashSet::new();
        let mut tracks = track_rows
            .iter()
            .filter(|(_, track)| seen_tracks.insert(track.id.clone()))
            .map(|(_, track)| track.clone())
            .collect::<Vec<_>>();
        self.attach_album_metadata(source_id, &mut albums)?;
        self.attach_track_metadata(source_id, &mut tracks)?;

        let albums_by_id = albums
            .into_iter()
            .map(|album| (album.id.clone(), album))
            .collect::<HashMap<_, _>>();
        let tracks_by_id = tracks
            .into_iter()
            .map(|track| (track.id.clone(), track))
            .collect::<HashMap<_, _>>();
        let mut sections = home_section_kinds()
            .into_iter()
            .map(|kind| {
                (
                    kind,
                    HomeSection {
                        kind,
                        albums: Vec::new(),
                        tracks: Vec::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        for (kind, album) in album_rows {
            let kind = home_section_kind_from_key(&kind)?;
            if let Some(hydrated) = albums_by_id.get(&album.id) {
                sections
                    .get_mut(&kind)
                    .unwrap()
                    .albums
                    .push(hydrated.clone());
            }
        }
        for (kind, track) in track_rows {
            let kind = home_section_kind_from_key(&kind)?;
            if let Some(hydrated) = tracks_by_id.get(&track.id) {
                sections
                    .get_mut(&kind)
                    .unwrap()
                    .tracks
                    .push(hydrated.clone());
            }
        }
        Ok(home_section_kinds()
            .into_iter()
            .filter_map(|kind| sections.remove(&kind))
            .filter(|section| !section.albums.is_empty() || !section.tracks.is_empty())
            .collect())
    }

    fn load_home_genres(&self, source_id: &SourceId, limit: usize) -> StoreResult<Vec<HomeGenre>> {
        let mut statement = self.connection.prepare(
            "
            SELECT genre_id, name, album_count, track_count
            FROM genres g
            WHERE g.source_id = ?1
              AND (
                  EXISTS (
                      SELECT 1 FROM album_genres ag
                      WHERE ag.source_id = g.source_id AND ag.genre_name = g.name
                  )
                  OR EXISTS (
                      SELECT 1 FROM track_genres tg
                      WHERE tg.source_id = g.source_id AND tg.genre_name = g.name
                  )
              )
            ORDER BY name COLLATE NOCASE
            LIMIT ?2
            ",
        )?;
        collect_rows(
            statement.query_map(params![source_id.as_str(), limit as i64], |row| {
                Ok(HomeGenre {
                    id: GenreId::new(row.get::<_, String>(0)?),
                    name: row.get(1)?,
                    album_count: u32_from_i64(row.get(2)?),
                    track_count: u32_from_i64(row.get(3)?),
                })
            })?,
        )
    }

    pub fn load_home_sections(&self, source_id: &SourceId) -> StoreResult<Vec<HomeSection>> {
        self.read_snapshot(|store| store.load_home_sections_inner(source_id))
    }
    pub(super) fn load_home_section_inner(
        &self,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Option<HomeSection>> {
        let section = HomeSection {
            kind,
            albums: self.load_home_section_albums(source_id, kind)?,
            tracks: self.load_home_section_tracks(source_id, kind)?,
        };
        Ok((!section.albums.is_empty() || !section.tracks.is_empty()).then_some(section))
    }
    pub(super) fn load_home_membership_from(
        &self,
        table: &str,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Vec<(String, i64, String)>> {
        let sql = format!(
            "
            SELECT item_type, position, item_id
            FROM {table}
            WHERE source_id = ?1
              AND section_kind = ?2
            ORDER BY item_type, position, item_id
            "
        );
        let mut statement = self.connection.prepare(&sql)?;
        collect_rows(statement.query_map(
            params![source_id.as_str(), home_section_kind_key(kind)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?)
    }
    pub fn load_home_section_prefetch(
        &self,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<Option<HomeSection>> {
        self.read_snapshot(|store| store.load_home_section_prefetch_inner(source_id, kind))
    }
    fn load_home_section_prefetch_inner(
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
                       t.image_item_id, t.image_tag, t.bpm, t.local_path, t.source_format
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
                       t.image_item_id, t.image_tag, t.bpm
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
}

pub(super) fn home_section_kind_mask(kind: HomeSectionKind) -> i64 {
    match kind {
        HomeSectionKind::Explore => 1 << 0,
        HomeSectionKind::MostPlayed => 1 << 1,
        HomeSectionKind::NewlyAdded => 1 << 2,
        HomeSectionKind::RecentlyPlayed => 1 << 3,
        HomeSectionKind::RecentlyReleased => 1 << 4,
    }
}
