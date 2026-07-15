use super::sources::{canonical_album_artists_for_write, collect_rows, home_membership};
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncCommit {
    pub delta: LibraryDelta,
    pub cache_revision: i64,
}

#[derive(Clone, Debug)]
pub struct LibrarySync {
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub album_artists: Vec<Artist>,
    pub genres: Vec<Genre>,
    pub playlists: Vec<PlaylistSnapshot>,
    pub home_sections: Vec<HomeSection>,
    pub mappings: Vec<SourceObjectMapping>,
    pub coverage: SyncCoverage,
    pub local_access: Option<LocalAccessUpdate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicFolderSnapshot {
    pub folder: MusicFolder,
    pub track_ids: Vec<TrackId>,
}

#[derive(Clone, Debug)]
pub struct LocalAccessUpdate {
    pub manifest: LocalManifestDelta,
    pub matches: Vec<(TrackId, String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackFolderMembership {
    pub track_id: TrackId,
    pub folder_ids: Vec<MusicFolderId>,
}

/// All may remove any missing library row; Finite removes only the listed known objects
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncCoverage {
    All {
        music_folders: Vec<MusicFolderSnapshot>,
    },
    Finite {
        tombstones: Vec<SourceObjectMapping>,
        track_folders: Vec<TrackFolderMembership>,
    },
}

#[derive(Default)]
struct FiniteDeletions {
    mappings: Vec<SourceObjectMapping>,
    tracks: Vec<TrackId>,
    playlists: Vec<PlaylistId>,
}

#[derive(Default)]
pub(super) struct AggregateStats {
    albums: HashMap<String, (i64, i64)>,
    artists: HashMap<String, (i64, i64)>,
    album_artists: HashMap<String, (i64, i64)>,
    genres: HashMap<String, (i64, i64, i64)>,
}

enum HomeWrite {
    Visible,
    Prefetch,
    Promote,
}

pub(super) fn load_aggregate_stats(
    store: &Store,
    source_id: &SourceId,
) -> StoreResult<AggregateStats> {
    Ok(AggregateStats {
        albums: two_column_stats(
            store,
            source_id,
            "albums",
            "album_id",
            "track_count",
            "duration_seconds",
        )?,
        artists: two_column_stats(
            store,
            source_id,
            "artists",
            "artist_id",
            "album_count",
            "track_count",
        )?,
        album_artists: two_column_stats(
            store,
            source_id,
            "album_artists",
            "artist_id",
            "album_count",
            "track_count",
        )?,
        genres: three_column_stats(
            store,
            source_id,
            "genres",
            "genre_id",
            "album_count",
            "track_count",
            "duration_seconds",
        )?,
    })
}

pub(super) fn merge_aggregate_stats(
    store: &Store,
    source_id: &SourceId,
    before: &AggregateStats,
    delta: &mut LibraryDelta,
) -> StoreResult<()> {
    let after = load_aggregate_stats(store, source_id)?;
    for (id, stats) in &after.albums {
        let id = AlbumId::new(id.clone());
        if before
            .albums
            .get(id.as_str())
            .is_some_and(|before| before != stats)
            && !delta.albums.stats.contains(&id)
        {
            delta.albums.stats.push(id);
        }
    }
    for (id, stats) in &after.artists {
        let id = ArtistId::new(id.clone());
        if before
            .artists
            .get(id.as_str())
            .is_some_and(|before| before != stats)
            && !delta.artists.stats.contains(&id)
        {
            delta.artists.stats.push(id);
        }
    }
    for (id, stats) in &after.album_artists {
        let id = ArtistId::new(id.clone());
        if before
            .album_artists
            .get(id.as_str())
            .is_some_and(|before| before != stats)
            && !delta.album_artists.stats.contains(&id)
        {
            delta.album_artists.stats.push(id);
        }
    }
    for (id, stats) in &after.genres {
        let id = GenreId::new(id.clone());
        if before
            .genres
            .get(id.as_str())
            .is_some_and(|before| before != stats)
            && !delta.genres.stats.contains(&id)
        {
            delta.genres.stats.push(id);
        }
    }
    Ok(())
}

fn two_column_stats(
    store: &Store,
    source_id: &SourceId,
    table: &str,
    id_column: &str,
    first: &str,
    second: &str,
) -> StoreResult<HashMap<String, (i64, i64)>> {
    let sql = format!("SELECT {id_column}, {first}, {second} FROM {table} WHERE source_id = ?1");
    let mut statement = store.connection.prepare(&sql)?;
    let rows = collect_rows(statement.query_map(params![source_id.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        ))
    })?)?;
    Ok(rows.into_iter().collect())
}

fn three_column_stats(
    store: &Store,
    source_id: &SourceId,
    table: &str,
    id_column: &str,
    first: &str,
    second: &str,
    third: &str,
) -> StoreResult<HashMap<String, (i64, i64, i64)>> {
    let sql =
        format!("SELECT {id_column}, {first}, {second}, {third} FROM {table} WHERE source_id = ?1");
    let mut statement = store.connection.prepare(&sql)?;
    let rows = collect_rows(statement.query_map(params![source_id.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ),
        ))
    })?)?;
    Ok(rows.into_iter().collect())
}

impl Store {
    pub fn replace_home_section(
        &self,
        source_id: &SourceId,
        generation: i64,
        base_cache_revision: i64,
        section: &HomeSection,
    ) -> StoreResult<SyncCommit> {
        self.commit_home_write(
            source_id,
            generation,
            base_cache_revision,
            section,
            HomeWrite::Visible,
        )
    }

    pub fn save_home_section_prefetch(
        &self,
        source_id: &SourceId,
        generation: i64,
        base_cache_revision: i64,
        section: &HomeSection,
    ) -> StoreResult<SyncCommit> {
        self.commit_home_write(
            source_id,
            generation,
            base_cache_revision,
            section,
            HomeWrite::Prefetch,
        )
    }

    pub fn promote_home_section(
        &self,
        source_id: &SourceId,
        generation: i64,
        base_cache_revision: i64,
        section: &HomeSection,
    ) -> StoreResult<SyncCommit> {
        self.commit_home_write(
            source_id,
            generation,
            base_cache_revision,
            section,
            HomeWrite::Promote,
        )
    }

    fn commit_home_write(
        &self,
        source_id: &SourceId,
        generation: i64,
        base_cache_revision: i64,
        section: &HomeSection,
        write: HomeWrite,
    ) -> StoreResult<SyncCommit> {
        self.write_batch(|_| {
            self.require_current_sync_generation(source_id, generation)?;
            self.require_source_cache_revision(source_id, base_cache_revision)?;
            self.require_home_entities(source_id, section)?;
            let mut collector = LibraryDeltaCollector::new();
            match write {
                HomeWrite::Visible | HomeWrite::Promote => {
                    let before = self.load_home_membership_from(
                        "home_section_items",
                        source_id,
                        section.kind,
                    )?;
                    let visible_changed = before != home_membership(section);
                    if visible_changed {
                        self.upsert_home_section(source_id, section, generation)?;
                        collector.merge(LibraryDelta {
                            home_changed: true,
                            ..LibraryDelta::default()
                        });
                    }
                    if matches!(write, HomeWrite::Promote) {
                        let prefetched =
                            self.load_home_section_prefetch(source_id, section.kind)?;
                        if prefetched.is_some() {
                            self.clear_home_section_prefetch(source_id, section.kind)?;
                        }
                    }
                }
                HomeWrite::Prefetch => {
                    let before = self.load_home_membership_from(
                        "home_section_prefetch_items",
                        source_id,
                        section.kind,
                    )?;
                    let changed = before != home_membership(section);
                    if changed {
                        self.upsert_home_section_prefetch(source_id, section, generation)?;
                    }
                }
            }
            let delta = collector.finish();
            let cache_revision =
                self.advance_source_cache_revision(source_id, base_cache_revision)?;
            self.connection.execute(
                "
                UPDATE sync_state
                SET last_completed_at = CURRENT_TIMESTAMP
                WHERE source_id = ?1
                ",
                params![source_id.as_str()],
            )?;
            Ok(SyncCommit {
                delta,
                cache_revision,
            })
        })
    }

    fn require_home_entities(
        &self,
        source_id: &SourceId,
        section: &HomeSection,
    ) -> StoreResult<()> {
        for (kind, table, id_column, entity_id) in section
            .albums
            .iter()
            .map(|album| ("album", "albums", "album_id", album.id.as_str()))
            .chain(
                section
                    .tracks
                    .iter()
                    .map(|track| ("track", "tracks", "track_id", track.id.as_str())),
            )
        {
            let present = self.connection.query_row(
                &format!(
                    "SELECT EXISTS (
                         SELECT 1 FROM {table}
                         WHERE source_id = ?1 AND {id_column} = ?2
                     ) AND EXISTS (
                         SELECT 1 FROM source_objects
                         WHERE source_id = ?1
                           AND entity_kind = ?3
                           AND entity_id = ?2
                     )"
                ),
                params![source_id.as_str(), entity_id, kind],
                |row| row.get::<_, bool>(0),
            )?;
            if !present {
                return Err(StoreError::InvalidSyncBatch(format!(
                    "Home references unmapped {kind} {entity_id}"
                )));
            }
        }
        Ok(())
    }
}

impl Store {
    pub(super) fn finish_library_sync(
        &self,
        source_id: &SourceId,
        generation: i64,
        base_cache_revision: i64,
        complete_coverage: bool,
        apply: impl FnOnce() -> StoreResult<LibraryDelta>,
    ) -> StoreResult<SyncCommit> {
        // Save library rows, refresh aggregate facts, and advance the cache revision all at once.
        self.write_batch(|_| {
            self.require_current_sync_generation(source_id, generation)?;
            self.require_source_cache_revision(source_id, base_cache_revision)?;
            let aggregate_stats_before = complete_coverage
                .then(|| load_aggregate_stats(self, source_id))
                .transpose()?;
            let mut delta = apply()?;
            if !delta.is_empty()
                && let Some(aggregate_stats_before) = aggregate_stats_before.as_ref()
            {
                self.refresh_library_counts(source_id)?;
                merge_aggregate_stats(self, source_id, aggregate_stats_before, &mut delta)?;
            } else if !delta.is_empty() {
                self.refresh_finite_stats(source_id, &mut delta)?;
            }
            let cache_revision =
                self.advance_source_cache_revision(source_id, base_cache_revision)?;
            self.connection.execute(
                "
                UPDATE sync_state
                SET status = 'idle',
                    generation = ?2,
                    last_completed_at = CURRENT_TIMESTAMP,
                    last_all_completed_at = CASE
                        WHEN ?3 THEN CURRENT_TIMESTAMP
                        ELSE last_all_completed_at
                    END,
                    last_error = NULL
                WHERE source_id = ?1
                ",
                params![source_id.as_str(), generation, complete_coverage],
            )?;
            Ok(SyncCommit {
                delta,
                cache_revision,
            })
        })
    }

    fn refresh_finite_stats(
        &self,
        source_id: &SourceId,
        delta: &mut LibraryDelta,
    ) -> StoreResult<()> {
        for album_id in delta.albums.links.clone() {
            let (track_count, duration_seconds) = self.connection.query_row(
                "SELECT COUNT(*), COALESCE(SUM(duration_seconds), 0)
                 FROM tracks WHERE source_id = ?1 AND album_id = ?2",
                params![source_id.as_str(), album_id.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            let changed = self.connection.execute(
                "UPDATE albums SET track_count = ?3, duration_seconds = ?4
                 WHERE source_id = ?1 AND album_id = ?2
                   AND (track_count != ?3 OR duration_seconds != ?4)",
                params![
                    source_id.as_str(),
                    album_id.as_str(),
                    track_count,
                    duration_seconds,
                ],
            )?;
            if changed != 0 && !delta.albums.stats.contains(&album_id) {
                delta.albums.stats.push(album_id);
            }
        }

        for artist_id in delta.artists.links.clone() {
            let (track_count, album_count) = self.connection.query_row(
                "SELECT COUNT(DISTINCT track_id), COUNT(DISTINCT album_id)
                 FROM (
                     SELECT track_id, album_id FROM tracks
                     WHERE source_id = ?1 AND artist_id = ?2
                     UNION
                     SELECT tracks.track_id, tracks.album_id
                     FROM track_artist_links links
                     JOIN tracks ON tracks.source_id = links.source_id
                                AND tracks.track_id = links.track_id
                     WHERE links.source_id = ?1 AND links.artist_id = ?2
                 )",
                params![source_id.as_str(), artist_id.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            let changed = self.connection.execute(
                "UPDATE artists SET album_count = ?3, track_count = ?4
                 WHERE source_id = ?1 AND artist_id = ?2
                   AND (album_count != ?3 OR track_count != ?4)",
                params![
                    source_id.as_str(),
                    artist_id.as_str(),
                    album_count,
                    track_count,
                ],
            )?;
            if changed != 0 && !delta.artists.stats.contains(&artist_id) {
                delta.artists.stats.push(artist_id);
            }
        }

        for artist_id in delta.album_artists.links.clone() {
            let (album_count, track_count) = self.connection.query_row(
                "SELECT COUNT(DISTINCT albums.album_id), COALESCE(SUM(albums.track_count), 0)
                 FROM albums
                 WHERE albums.source_id = ?1
                   AND (
                       albums.artist_id = ?2
                       OR EXISTS (
                           SELECT 1 FROM album_artist_links links
                           WHERE links.source_id = albums.source_id
                             AND links.album_id = albums.album_id
                             AND links.artist_id = ?2
                       )
                   )",
                params![source_id.as_str(), artist_id.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            let changed = self.connection.execute(
                "UPDATE album_artists SET album_count = ?3, track_count = ?4
                 WHERE source_id = ?1 AND artist_id = ?2
                   AND (album_count != ?3 OR track_count != ?4)",
                params![
                    source_id.as_str(),
                    artist_id.as_str(),
                    album_count,
                    track_count,
                ],
            )?;
            if changed != 0 && !delta.album_artists.stats.contains(&artist_id) {
                delta.album_artists.stats.push(artist_id);
            }
        }

        for genre_id in delta.genres.links.clone() {
            let genre_name = self
                .connection
                .query_row(
                    "SELECT name FROM genres WHERE source_id = ?1 AND genre_id = ?2",
                    params![source_id.as_str(), genre_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(genre_name) = genre_name else {
                continue;
            };
            let (album_count, track_count, duration_seconds) = self.connection.query_row(
                "SELECT
                     (SELECT COUNT(*) FROM (
                         SELECT albums.album_id
                         FROM album_genres links
                         JOIN albums ON albums.source_id = links.source_id
                                    AND albums.album_id = links.album_id
                         WHERE links.source_id = ?1 AND links.genre_name = ?2
                         UNION
                         SELECT albums.album_id
                         FROM track_genres links
                         JOIN tracks ON tracks.source_id = links.source_id
                                    AND tracks.track_id = links.track_id
                         JOIN albums ON albums.source_id = tracks.source_id
                                    AND albums.album_id = tracks.album_id
                         WHERE links.source_id = ?1 AND links.genre_name = ?2
                     )),
                     COUNT(DISTINCT tracks.track_id),
                     COALESCE(SUM(tracks.duration_seconds), 0)
                 FROM track_genres links
                 JOIN tracks ON tracks.source_id = links.source_id
                            AND tracks.track_id = links.track_id
                 WHERE links.source_id = ?1 AND links.genre_name = ?2",
                params![source_id.as_str(), genre_name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )?;
            let changed = self.connection.execute(
                "UPDATE genres
                 SET album_count = ?3, track_count = ?4, duration_seconds = ?5
                 WHERE source_id = ?1 AND genre_id = ?2
                   AND (album_count != ?3 OR track_count != ?4 OR duration_seconds != ?5)",
                params![
                    source_id.as_str(),
                    genre_id.as_str(),
                    album_count,
                    track_count,
                    duration_seconds,
                ],
            )?;
            if changed != 0 && !delta.genres.stats.contains(&genre_id) {
                delta.genres.stats.push(genre_id);
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct ObservedIds {
    albums: HashSet<String>,
    tracks: HashSet<String>,
    artists: HashSet<String>,
    album_artists: HashSet<String>,
    genres: HashSet<String>,
    playlists: HashSet<String>,
}

impl ObservedIds {
    fn from_sync(sync: &LibrarySync) -> Self {
        Self {
            albums: sync
                .albums
                .iter()
                .map(|item| item.id.as_str().to_string())
                .collect(),
            tracks: sync
                .tracks
                .iter()
                .map(|item| item.id.as_str().to_string())
                .collect(),
            artists: sync
                .artists
                .iter()
                .map(|item| item.id.as_str().to_string())
                .collect(),
            album_artists: sync
                .album_artists
                .iter()
                .map(|item| item.id.as_str().to_string())
                .collect(),
            genres: sync
                .genres
                .iter()
                .map(|item| item.id.as_str().to_string())
                .collect(),
            playlists: sync
                .playlists
                .iter()
                .map(|item| item.playlist.id.as_str().to_string())
                .collect(),
        }
    }
}

impl Store {
    pub fn commit_library_sync(
        &self,
        source_id: &SourceId,
        generation: i64,
        base_cache_revision: i64,
        sync: LibrarySync,
    ) -> StoreResult<SyncCommit> {
        let complete = matches!(sync.coverage, SyncCoverage::All { .. });
        self.finish_library_sync(source_id, generation, base_cache_revision, complete, || {
            self.apply_library_sync(source_id, generation, sync)
        })
    }

    fn apply_library_sync(
        &self,
        source_id: &SourceId,
        generation: i64,
        mut sync: LibrarySync,
    ) -> StoreResult<LibraryDelta> {
        self.normalize_album_artists(source_id, &mut sync.album_artists, &mut sync.mappings)?;
        if let SyncCoverage::Finite { track_folders, .. } = &sync.coverage {
            self.require_finite_memberships(source_id, track_folders)?;
            self.require_finite_mapping_stability(source_id, &sync.mappings)?;
        }
        let observed = ObservedIds::from_sync(&sync);
        let complete = matches!(sync.coverage, SyncCoverage::All { .. });
        let mut collector = LibraryDeltaCollector::new();
        collector.merge(self.upsert_albums_delta(source_id, &sync.albums, generation)?);
        collector.merge(self.upsert_tracks_delta(source_id, &sync.tracks, generation)?);
        collector.merge(self.upsert_artists_delta(source_id, &sync.artists, false, generation)?);
        collector.merge(self.upsert_artists_delta(
            source_id,
            &sync.album_artists,
            true,
            generation,
        )?);
        collector.merge(self.upsert_genres_delta(source_id, &sync.genres, generation)?);
        let playlists = sync
            .playlists
            .iter()
            .map(|detail| detail.playlist.clone())
            .collect::<Vec<_>>();
        collector.merge(self.upsert_playlists_delta(source_id, &playlists, generation)?);
        for detail in &sync.playlists {
            collector.merge(self.upsert_playlist_entries_delta(
                source_id,
                &detail.playlist.id,
                &detail.entries,
                generation,
            )?);
        }
        if complete {
            collector.merge(self.upsert_home_sections_delta(
                source_id,
                &sync.home_sections,
                generation,
            )?);
        } else {
            for section in &sync.home_sections {
                let before =
                    self.load_home_membership_from("home_section_items", source_id, section.kind)?;
                if before != home_membership(section) {
                    self.upsert_home_section(source_id, section, generation)?;
                    collector.merge(LibraryDelta {
                        home_changed: true,
                        ..LibraryDelta::default()
                    });
                }
            }
        }
        self.upsert_source_mappings(source_id, generation, &sync.mappings)?;
        match &sync.coverage {
            SyncCoverage::All { music_folders } => self.apply_complete_coverage(
                source_id,
                generation,
                &observed,
                &sync.mappings,
                music_folders,
                &mut collector,
            )?,
            SyncCoverage::Finite {
                tombstones,
                track_folders,
            } => {
                let deletions =
                    self.resolve_finite_deletions(source_id, tombstones, &sync.mappings)?;
                collector.merge(self.apply_finite_coverage(
                    source_id,
                    generation,
                    &deletions,
                    track_folders,
                )?);
            }
        }
        let local_matches_changed =
            self.apply_local_access_update(source_id, generation, sync.local_access)?;
        let mut delta = collector.finish();
        delta.local_matches_changed = local_matches_changed;
        self.expand_artwork_projection_delta(source_id, &mut delta)?;
        Ok(delta)
    }

    fn require_finite_memberships(
        &self,
        source_id: &SourceId,
        track_folders: &[TrackFolderMembership],
    ) -> StoreResult<()> {
        let mut tracks = HashSet::new();
        for membership in track_folders {
            if !tracks.insert(&membership.track_id) {
                return Err(StoreError::NeedsFullSync);
            }
            let mut folders = HashSet::new();
            for folder_id in &membership.folder_ids {
                if !folders.insert(folder_id) {
                    return Err(StoreError::NeedsFullSync);
                }
                let exists = self.connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM source_music_folders
                     WHERE source_id = ?1 AND folder_id = ?2)",
                    params![source_id.as_str(), folder_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )?;
                if !exists {
                    return Err(StoreError::NeedsFullSync);
                }
            }
        }
        Ok(())
    }

    fn require_finite_mapping_stability(
        &self,
        source_id: &SourceId,
        mappings: &[SourceObjectMapping],
    ) -> StoreResult<()> {
        for mapping in mappings {
            let current = self
                .connection
                .query_row(
                    "SELECT entity_id, source_object_kind
                     FROM source_objects
                     WHERE source_id = ?1 AND source_object_id = ?2 AND entity_kind = ?3",
                    params![
                        source_id.as_str(),
                        mapping.source_object_id,
                        mapping.entity_kind.as_str(),
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            if current
                .is_some_and(|(entity_id, kind)| kind != "source" || entity_id != mapping.entity_id)
            {
                return Err(StoreError::NeedsFullSync);
            }
        }
        Ok(())
    }

    fn normalize_album_artists(
        &self,
        source_id: &SourceId,
        artists: &mut Vec<Artist>,
        mappings: &mut [SourceObjectMapping],
    ) -> StoreResult<()> {
        let canonical = canonical_album_artists_for_write(&self.connection, source_id, artists)?;
        let mut aliases = HashMap::<String, String>::new();
        for artist in &canonical {
            let canonical_id = artist.artist.id.as_str().to_string();
            aliases.insert(canonical_id.clone(), canonical_id.clone());
            for alias in &artist.alias_ids {
                aliases.insert(alias.as_str().to_string(), canonical_id.clone());
            }
        }
        *artists = canonical.into_iter().map(|artist| artist.artist).collect();
        for mapping in mappings {
            if mapping.entity_kind == SourceEntityKind::AlbumArtist
                && let Some(canonical_id) = aliases.get(&mapping.entity_id)
            {
                mapping.entity_id.clone_from(canonical_id);
            }
        }
        Ok(())
    }

    fn upsert_source_mappings(
        &self,
        source_id: &SourceId,
        generation: i64,
        mappings: &[SourceObjectMapping],
    ) -> StoreResult<()> {
        let mut current_statement = self.connection.prepare(
            "SELECT source_object_id, entity_kind, entity_id, metadata_json
             FROM source_objects
             WHERE source_id = ?1 AND source_object_kind = 'source'",
        )?;
        let current = collect_rows(current_statement.query_map(
            params![source_id.as_str()],
            |row| {
                let source_object_id = row.get::<_, String>(0)?;
                let kind = row.get::<_, String>(1)?;
                let entity_id = row.get::<_, String>(2)?;
                let metadata_is_empty = row.get::<_, String>(3)? == "{}";
                Ok(SourceEntityKind::parse(&kind)
                    .map(|kind| ((source_object_id, kind), (entity_id, metadata_is_empty))))
            },
        )?)?
        .into_iter()
        .flatten()
        .collect::<HashMap<_, _>>();
        let mut statement = self.connection.prepare(
            "INSERT INTO source_objects (
                source_id, source_object_id, entity_kind, entity_id,
                source_object_kind, metadata_json, sync_generation, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'source', '{}', ?5, CURRENT_TIMESTAMP)
             ON CONFLICT(source_id, source_object_id, entity_kind) DO UPDATE SET
                entity_id = excluded.entity_id,
                source_object_kind = excluded.source_object_kind,
                metadata_json = excluded.metadata_json,
                sync_generation = excluded.sync_generation,
                updated_at = excluded.updated_at
             WHERE source_objects.entity_id != excluded.entity_id
                OR source_objects.source_object_kind != excluded.source_object_kind
                OR source_objects.metadata_json != excluded.metadata_json",
        )?;
        for mapping in mappings {
            let key = (
                mapping.source_object_id.as_str().to_string(),
                mapping.entity_kind,
            );
            if current
                .get(&key)
                .is_some_and(|(entity_id, metadata_is_empty)| {
                    entity_id == &mapping.entity_id && *metadata_is_empty
                })
            {
                continue;
            }
            statement.execute(params![
                source_id.as_str(),
                mapping.source_object_id,
                mapping.entity_kind.as_str(),
                mapping.entity_id,
                generation,
            ])?;
        }
        Ok(())
    }

    fn apply_local_access_update(
        &self,
        source_id: &SourceId,
        generation: i64,
        update: Option<LocalAccessUpdate>,
    ) -> StoreResult<bool> {
        let Some(update) = update else {
            return Ok(false);
        };
        let changed = self.replace_track_local_matches(source_id, &update.matches)?;
        super::local_manifest::apply_local_manifest_delta_on_connection(
            &self.connection,
            source_id,
            generation,
            &update.manifest,
        )?;
        Ok(changed)
    }

    fn resolve_finite_deletions(
        &self,
        source_id: &SourceId,
        tombstones: &[SourceObjectMapping],
        incoming: &[SourceObjectMapping],
    ) -> StoreResult<FiniteDeletions> {
        let mut seen = HashSet::new();
        let mut current = Vec::new();
        for mapping in tombstones {
            if !matches!(
                mapping.entity_kind,
                SourceEntityKind::Track | SourceEntityKind::Playlist
            ) {
                return Err(StoreError::NeedsFullSync);
            }
            if !seen.insert((mapping.source_object_id.as_str(), mapping.entity_kind)) {
                return Err(StoreError::NeedsFullSync);
            }
            let also_present = incoming.iter().any(|candidate| {
                candidate.source_object_id == mapping.source_object_id
                    && candidate.entity_kind == mapping.entity_kind
            });
            if also_present {
                return Err(StoreError::NeedsFullSync);
            }
            let stored = self
                .connection
                .query_row(
                    "SELECT entity_id, source_object_kind
                 FROM source_objects
                 WHERE source_id = ?1 AND source_object_id = ?2 AND entity_kind = ?3",
                    params![
                        source_id.as_str(),
                        mapping.source_object_id,
                        mapping.entity_kind.as_str(),
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            match stored {
                None => {}
                Some((entity_id, kind)) if entity_id == mapping.entity_id && kind == "source" => {
                    current.push(mapping.clone());
                }
                Some(_) => return Err(StoreError::NeedsFullSync),
            }
        }

        Ok(FiniteDeletions {
            tracks: self.entities_without_surviving_mapping(
                source_id,
                SourceEntityKind::Track,
                &current,
            )?,
            playlists: self.entities_without_surviving_mapping(
                source_id,
                SourceEntityKind::Playlist,
                &current,
            )?,
            mappings: current,
        })
    }

    fn entities_without_surviving_mapping<Id>(
        &self,
        source_id: &SourceId,
        kind: SourceEntityKind,
        tombstones: &[SourceObjectMapping],
    ) -> StoreResult<Vec<Id>>
    where
        Id: From<String>,
    {
        let mut entity_ids = tombstones
            .iter()
            .filter(|mapping| mapping.entity_kind == kind)
            .map(|mapping| mapping.entity_id.clone())
            .collect::<Vec<_>>();
        entity_ids.sort();
        entity_ids.dedup();
        let mut removed = Vec::new();
        for entity_id in entity_ids {
            let mut statement = self.connection.prepare(
                "SELECT source_object_id
                 FROM source_objects
                 WHERE source_id = ?1 AND entity_kind = ?2 AND entity_id = ?3
                   AND source_object_kind = 'source'",
            )?;
            let mappings = collect_rows(statement.query_map(
                params![source_id.as_str(), kind.as_str(), entity_id],
                |row| row.get::<_, String>(0),
            )?)?;
            if !mappings.is_empty()
                && mappings.iter().all(|source_object_id| {
                    tombstones.iter().any(|mapping| {
                        mapping.entity_kind == kind
                            && mapping.entity_id == entity_id
                            && mapping.source_object_id == *source_object_id
                    })
                })
            {
                removed.push(Id::from(entity_id));
            }
        }
        Ok(removed)
    }

    fn delete_source_tracks(
        &self,
        source_id: &SourceId,
        track_ids: &[TrackId],
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        for track_id in track_ids {
            let Some(track) = self.load_track_for_delta(source_id, track_id)? else {
                continue;
            };
            delta.tracks.deleted.push(track.id);
            delta.albums.links.push(track.album_id);
            delta.artists.links.extend(track.artist_id);
            delta
                .artists
                .links
                .extend(track.artist_credits.into_iter().map(|credit| credit.id));
            delta.album_artists.links.extend(
                track
                    .album_artist_credits
                    .into_iter()
                    .map(|credit| credit.id),
            );
            delta
                .genres
                .links
                .extend(self.genre_ids_for_names(source_id, &track.genres)?);
        }
        let deletion = self.delete_track_rows(
            source_id,
            track_ids,
            super::local_manifest::TrackEntitySource::Source,
        )?;
        delta.playlists.entries = deletion.playlist_ids.clone();
        delta.playlists.cover_refs = deletion.playlist_ids;
        delta.home_changed = deletion.home_changed;
        delta.folders_changed = deletion.folders_changed;
        Ok(delta)
    }

    fn delete_native_playlists(
        &self,
        source_id: &SourceId,
        playlist_ids: &[PlaylistId],
    ) -> StoreResult<LibraryDelta> {
        let mut deleted = Vec::new();
        for playlist_id in playlist_ids {
            let exists = self.connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM playlists
                 WHERE source_id = ?1 AND playlist_id = ?2 AND owner = 'native')",
                params![source_id.as_str(), playlist_id.as_str()],
                |row| row.get::<_, bool>(0),
            )?;
            if exists {
                deleted.push(playlist_id.clone());
            }
        }
        super::library_auxiliary_cache::delete_playlist_rows(
            &self.connection,
            source_id,
            playlist_ids,
            SourceFeatureOwner::Native,
        )?;
        Ok(LibraryDelta {
            playlists: PlaylistDelta {
                deleted,
                ..PlaylistDelta::default()
            },
            ..LibraryDelta::default()
        })
    }

    fn apply_complete_coverage(
        &self,
        source_id: &SourceId,
        generation: i64,
        observed: &ObservedIds,
        mappings: &[SourceObjectMapping],
        snapshots: &[MusicFolderSnapshot],
        collector: &mut LibraryDeltaCollector,
    ) -> StoreResult<()> {
        self.prune_unobserved_source_mappings(source_id, mappings)?;
        let mut missing = self.missing_library_delta(source_id, observed)?;
        let has_missing = !missing.is_empty();
        let track_ids = std::mem::take(&mut missing.tracks.deleted);
        let playlist_ids = std::mem::take(&mut missing.playlists.deleted);
        collector.merge(self.delete_source_tracks(source_id, &track_ids)?);
        collector.merge(self.delete_native_playlists(source_id, &playlist_ids)?);
        if has_missing {
            self.prune_missing_library(source_id, &missing)?;
        }
        collector.merge(missing);

        let mut folders_before = self.list_music_folders(source_id)?;
        folders_before.sort_by(|left, right| left.id.cmp(&right.id));
        let memberships_before = self.folder_memberships(source_id)?;
        let mut folders = snapshots
            .iter()
            .map(|snapshot| snapshot.folder.clone())
            .collect::<Vec<_>>();
        folders.sort_by(|left, right| left.id.cmp(&right.id));
        let mut memberships = snapshots
            .iter()
            .flat_map(|snapshot| {
                snapshot.track_ids.iter().map(|track_id| {
                    (
                        snapshot.folder.id.as_str().to_string(),
                        track_id.as_str().to_string(),
                    )
                })
            })
            .collect::<Vec<_>>();
        memberships.sort();
        let folders_changed = folders_before != folders;
        let memberships_changed = memberships_before != memberships;
        if folders_changed || memberships_changed {
            self.replace_folder_projection(
                source_id,
                generation,
                &folders,
                &memberships,
                folders_changed,
                memberships_changed,
            )?;
            collector.merge(LibraryDelta {
                folders_changed: true,
                ..LibraryDelta::default()
            });
        }
        Ok(())
    }

    fn apply_finite_coverage(
        &self,
        source_id: &SourceId,
        generation: i64,
        deletions: &FiniteDeletions,
        track_folders: &[TrackFolderMembership],
    ) -> StoreResult<LibraryDelta> {
        let mut collector = LibraryDeltaCollector::new();
        let mut folders_changed = false;
        for membership in track_folders {
            let current = self.connection.query_row(
                "SELECT GROUP_CONCAT(folder_id, char(31)) FROM (
                     SELECT folder_id FROM track_music_folders
                     WHERE source_id = ?1 AND track_id = ?2 ORDER BY folder_id)",
                params![source_id.as_str(), membership.track_id.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )?;
            let mut wanted = membership
                .folder_ids
                .iter()
                .map(MusicFolderId::as_str)
                .collect::<Vec<_>>();
            wanted.sort_unstable();
            if current
                .as_deref()
                .unwrap_or("")
                .split('\u{1f}')
                .filter(|id| !id.is_empty())
                .collect::<Vec<_>>()
                != wanted
            {
                self.connection.execute(
                    "DELETE FROM track_music_folders WHERE source_id = ?1 AND track_id = ?2",
                    params![source_id.as_str(), membership.track_id.as_str()],
                )?;
                for folder_id in &membership.folder_ids {
                    self.connection.execute(
                        "INSERT INTO track_music_folders (source_id, track_id, folder_id, sync_generation)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![source_id.as_str(), membership.track_id.as_str(), folder_id.as_str(), generation],
                    )?;
                }
                folders_changed = true;
            }
        }
        if folders_changed {
            collector.merge(LibraryDelta {
                folders_changed: true,
                ..LibraryDelta::default()
            });
        }
        collector.merge(self.delete_source_tracks(source_id, &deletions.tracks)?);
        collector.merge(self.delete_native_playlists(source_id, &deletions.playlists)?);
        for mapping in &deletions.mappings {
            self.connection.execute(
                "DELETE FROM source_objects WHERE source_id = ?1 AND source_object_id = ?2 AND entity_kind = ?3 AND entity_id = ?4 AND source_object_kind = 'source'",
                params![source_id.as_str(), mapping.source_object_id, mapping.entity_kind.as_str(), mapping.entity_id],
            )?;
        }
        Ok(collector.finish())
    }

    fn replace_folder_projection(
        &self,
        source_id: &SourceId,
        generation: i64,
        folders: &[MusicFolder],
        memberships: &[(String, String)],
        replace_folders: bool,
        replace_memberships: bool,
    ) -> StoreResult<()> {
        if replace_folders {
            self.connection.execute(
                "DELETE FROM source_music_folders WHERE source_id = ?1",
                params![source_id.as_str()],
            )?;
            self.upsert_music_folders(source_id, folders, generation)?;
        }
        if replace_memberships {
            self.connection.execute(
                "DELETE FROM track_music_folders WHERE source_id = ?1",
                params![source_id.as_str()],
            )?;
            let mut insert = self.connection.prepare(
                "
                INSERT INTO track_music_folders (
                    source_id, track_id, folder_id, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4)
                ",
            )?;
            for (folder_id, track_id) in memberships {
                insert.execute(params![source_id.as_str(), track_id, folder_id, generation,])?;
            }
        }
        self.connection.execute(
            "
            UPDATE source_library_preferences
            SET selected_music_folder_id = NULL,
                updated_at = CURRENT_TIMESTAMP
            WHERE source_id = ?1
              AND selected_music_folder_id IS NOT NULL
              AND NOT EXISTS (
                  SELECT 1 FROM source_music_folders
                  WHERE source_id = ?1
                    AND folder_id = source_library_preferences.selected_music_folder_id
              )
            ",
            params![source_id.as_str()],
        )?;
        Ok(())
    }

    fn unobserved_ids(
        &self,
        source_id: &SourceId,
        table: &str,
        id_column: &str,
        native_playlist_only: bool,
        observed: &HashSet<String>,
    ) -> StoreResult<Vec<String>> {
        let owner_filter = if native_playlist_only {
            "AND item.owner = 'native'"
        } else {
            ""
        };
        let sql = format!(
            "
            SELECT item.{id_column}
            FROM {table} item
            WHERE item.source_id = ?1
              {owner_filter}
            ORDER BY item.{id_column}
            "
        );
        let mut statement = self.connection.prepare(&sql)?;
        Ok(
            collect_rows(statement.query_map(params![source_id.as_str()], |row| row.get(0))?)?
                .into_iter()
                .filter(|id| !observed.contains(id))
                .collect(),
        )
    }

    fn missing_library_delta(
        &self,
        source_id: &SourceId,
        observed: &ObservedIds,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        delta.tracks.deleted = self
            .unobserved_ids(source_id, "tracks", "track_id", false, &observed.tracks)?
            .into_iter()
            .map(TrackId::new)
            .collect();
        delta.albums.deleted = self
            .unobserved_ids(source_id, "albums", "album_id", false, &observed.albums)?
            .into_iter()
            .map(AlbumId::new)
            .collect();
        delta.artists.deleted = self
            .unobserved_ids(source_id, "artists", "artist_id", false, &observed.artists)?
            .into_iter()
            .map(ArtistId::new)
            .collect();
        delta.album_artists.deleted = self
            .unobserved_ids(
                source_id,
                "album_artists",
                "artist_id",
                false,
                &observed.album_artists,
            )?
            .into_iter()
            .map(ArtistId::new)
            .collect();
        delta.genres.deleted = self
            .unobserved_ids(source_id, "genres", "genre_id", false, &observed.genres)?
            .into_iter()
            .map(GenreId::new)
            .collect();
        delta.playlists.deleted = self
            .unobserved_ids(
                source_id,
                "playlists",
                "playlist_id",
                true,
                &observed.playlists,
            )?
            .into_iter()
            .map(PlaylistId::new)
            .collect();
        self.expand_artwork_projection_delta(source_id, &mut delta)?;
        Ok(delta)
    }

    fn delete_ids(
        &self,
        source_id: &SourceId,
        table: &str,
        id_column: &str,
        ids: &[String],
    ) -> StoreResult<()> {
        for chunk in ids.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "DELETE FROM {table} WHERE source_id = ? AND {id_column} IN ({placeholders})"
            );
            let mut values = vec![Value::Text(source_id.as_str().to_string())];
            values.extend(chunk.iter().cloned().map(Value::Text));
            self.connection.execute(&sql, params_from_iter(values))?;
        }
        Ok(())
    }

    fn prune_missing_library(
        &self,
        source_id: &SourceId,
        missing: &LibraryDelta,
    ) -> StoreResult<()> {
        self.delete_ids(
            source_id,
            "albums",
            "album_id",
            &missing
                .albums
                .deleted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )?;
        self.delete_ids(
            source_id,
            "artists",
            "artist_id",
            &missing
                .artists
                .deleted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )?;
        self.delete_ids(
            source_id,
            "album_artists",
            "artist_id",
            &missing
                .album_artists
                .deleted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )?;
        self.delete_ids(
            source_id,
            "genres",
            "genre_id",
            &missing
                .genres
                .deleted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )?;
        for (table, owner_table, id_column) in [
            ("album_genres", "albums", "album_id"),
            ("album_artist_links", "albums", "album_id"),
        ] {
            let sql = format!(
                "
                DELETE FROM {table}
                WHERE source_id = ?1
                  AND NOT EXISTS (
                      SELECT 1
                      FROM {owner_table}
                      WHERE {owner_table}.source_id = {table}.source_id
                        AND {owner_table}.{id_column} = {table}.{id_column}
                  )
                "
            );
            self.connection.execute(&sql, params![source_id.as_str()])?;
        }
        self.connection.execute(
            "DELETE FROM library_fts
             WHERE source_id = ?1 AND item_type = 'album'
               AND NOT EXISTS (
                   SELECT 1 FROM albums
                   WHERE albums.source_id = library_fts.source_id
                     AND albums.album_id = library_fts.item_id
               )",
            params![source_id.as_str()],
        )?;
        self.connection.execute(
            "
            DELETE FROM library_fts
            WHERE source_id = ?1
              AND item_type IN ('artist', 'album_artist')
              AND NOT EXISTS (
                  SELECT 1 FROM artists
                  WHERE artists.source_id = library_fts.source_id
                    AND artists.artist_id = library_fts.item_id
                  UNION ALL
                  SELECT 1 FROM album_artists
                  WHERE album_artists.source_id = library_fts.source_id
                    AND album_artists.artist_id = library_fts.item_id
              )
            ",
            params![source_id.as_str()],
        )?;
        for (kind, table, id_column) in [
            ("track", "tracks", "track_id"),
            ("album", "albums", "album_id"),
            ("artist", "artists", "artist_id"),
            ("album_artist", "album_artists", "artist_id"),
        ] {
            for entity_table in [
                "entity_facts",
                "entity_grouping_keys",
                "entity_identity_keys",
                "entity_links",
                "entities",
            ] {
                self.connection.execute(
                    &format!(
                        "DELETE FROM {entity_table}
                         WHERE source_id = ?1 AND entity_kind = ?2
                           AND NOT EXISTS (
                               SELECT 1 FROM {table}
                               WHERE {table}.source_id = {entity_table}.source_id
                                 AND {table}.{id_column} = {entity_table}.entity_id
                           )"
                    ),
                    params![source_id.as_str(), kind],
                )?;
            }
        }
        Ok(())
    }

    fn folder_memberships(&self, source_id: &SourceId) -> StoreResult<Vec<(String, String)>> {
        let mut statement = self.connection.prepare(
            "
            SELECT folder_id, track_id
            FROM track_music_folders
            WHERE source_id = ?1
            ORDER BY folder_id, track_id
            ",
        )?;
        collect_rows(statement.query_map(params![source_id.as_str()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?)
    }

    fn prune_unobserved_source_mappings(
        &self,
        source_id: &SourceId,
        mappings: &[SourceObjectMapping],
    ) -> StoreResult<()> {
        let observed = mappings
            .iter()
            .map(|mapping| {
                (
                    mapping.source_object_id.as_str(),
                    mapping.entity_kind.as_str(),
                    mapping.entity_id.as_str(),
                )
            })
            .collect::<HashSet<_>>();
        let mut statement = self.connection.prepare(
            "SELECT source_object_id, entity_kind, entity_id
             FROM source_objects
             WHERE source_id = ?1 AND source_object_kind = 'source'",
        )?;
        let current = collect_rows(statement.query_map(params![source_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?)?;
        let mut delete = self.connection.prepare(
            "DELETE FROM source_objects
             WHERE source_id = ?1 AND source_object_id = ?2 AND entity_kind = ?3
               AND entity_id = ?4 AND source_object_kind = 'source'",
        )?;
        for (object_id, kind, entity_id) in current {
            if !observed.contains(&(object_id.as_str(), kind.as_str(), entity_id.as_str())) {
                delete.execute(params![source_id.as_str(), object_id, kind, entity_id])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{
        LibraryObservation, album, artist, credit, genre, image_ref, playlist, seed_cached_library,
        sqlite_sidecar_path, stored_source, track,
    };

    struct FiniteFixture {
        first_album: Album,
        second_album: Album,
        first_track: Track,
        second_track: Track,
        folder: MusicFolder,
        playlist: Playlist,
    }

    fn seed_finite_fixture(store: &Store, source_id: &SourceId) -> FiniteFixture {
        let first_album = album(1);
        let second_album = album(2);
        let first_track = track(1, &first_album);
        let second_track = track(2, &second_album);
        let folder = MusicFolder {
            id: MusicFolderId::new("music"),
            name: "Music".to_string(),
        };
        let playlist = playlist(1, None);
        let detail = PlaylistDetail {
            playlist: playlist.clone(),
            tracks: vec![first_track.clone()],
            entries: vec![PlaylistEntry {
                entry_id: "native-entry".to_string(),
                track: first_track.clone(),
            }],
        };
        let generation = store.begin_sync(source_id).expect("begin seed sync");
        LibraryObservation {
            albums: vec![first_album.clone(), second_album.clone()],
            tracks: vec![first_track.clone(), second_track.clone()],
            music_folders: vec![(
                folder.clone(),
                vec![first_track.clone(), second_track.clone()],
            )],
            playlists: vec![detail],
            home_sections: vec![HomeSection {
                kind: HomeSectionKind::NewlyAdded,
                albums: vec![first_album.clone(), second_album.clone()],
                tracks: vec![first_track.clone(), second_track.clone()],
            }],
            ..LibraryObservation::default()
        }
        .commit(store, source_id, generation)
        .expect("seed library");
        FiniteFixture {
            first_album,
            second_album,
            first_track,
            second_track,
            folder,
            playlist,
        }
    }

    fn mapping(kind: SourceEntityKind, entity_id: &str) -> SourceObjectMapping {
        SourceObjectMapping {
            source_object_id: entity_id.to_string(),
            entity_kind: kind,
            entity_id: entity_id.to_string(),
        }
    }

    fn small_library_sync(album: &Album, track: &Track) -> LibrarySync {
        let artist = artist(1, None);
        LibrarySync {
            albums: vec![album.clone()],
            tracks: vec![track.clone()],
            artists: vec![artist.clone()],
            album_artists: vec![artist.clone()],
            genres: Vec::new(),
            playlists: Vec::new(),
            home_sections: Vec::new(),
            mappings: [
                mapping(SourceEntityKind::Album, album.id.as_str()),
                mapping(SourceEntityKind::Track, track.id.as_str()),
                mapping(SourceEntityKind::Artist, artist.id.as_str()),
                mapping(SourceEntityKind::AlbumArtist, artist.id.as_str()),
            ]
            .into(),
            coverage: SyncCoverage::All {
                music_folders: Vec::new(),
            },
            local_access: None,
        }
    }

    fn finite_sync(
        tracks: Vec<Track>,
        mappings: Vec<SourceObjectMapping>,
        tombstones: Vec<SourceObjectMapping>,
        track_folders: Vec<TrackFolderMembership>,
    ) -> LibrarySync {
        LibrarySync {
            albums: Vec::new(),
            tracks,
            artists: Vec::new(),
            album_artists: Vec::new(),
            genres: Vec::new(),
            playlists: Vec::new(),
            home_sections: Vec::new(),
            mappings,
            coverage: SyncCoverage::Finite {
                tombstones,
                track_folders,
            },
            local_access: None,
        }
    }

    fn local_manifest_entry(number: u32, path: &str) -> LocalManifestEntry {
        let album = album(number);
        let mut track = track(number, &album);
        track.local_path = Some(path.to_string());
        LocalManifestEntry {
            facts: LocalFileFacts {
                path: PathBuf::from(path),
                root_path: PathBuf::from("/music"),
                relative_path: path.trim_start_matches("/music/").to_string(),
                file_size: u64::from(number),
                mtime_seconds: i64::from(number),
                mtime_nanos: 0,
                inode: None,
                device: None,
            },
            track,
            album_artist: "Artist".to_string(),
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
            cover: None,
            metadata_hash: format!("metadata-{number}"),
            search_hash: format!("search-{number}"),
        }
    }

    #[test]
    fn remote_commit_publishes_local_access_in_the_same_revision() {
        let store = Store::open_memory().expect("open Store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let album = album(1);
        let track = track(1, &album);
        let manifest = local_manifest_entry(10, "/music/local.flac");
        let local_match = (
            track.id.clone(),
            "/music/local.flac".to_string(),
            "metadata".to_string(),
        );
        let mut sync = small_library_sync(&album, &track);
        sync.local_access = Some(LocalAccessUpdate {
            manifest: LocalManifestDelta {
                upserted_entries: vec![manifest.clone()],
                deleted_paths: Vec::new(),
            },
            matches: vec![local_match.clone()],
        });

        let commit = store
            .commit_library_sync(&saved.source_id, generation, 0, sync)
            .expect("commit sync");

        assert_eq!(commit.cache_revision, 1);
        assert!(commit.delta.local_matches_changed);
        assert_eq!(
            store
                .track_local_match_path(&saved.source_id, &track.id)
                .expect("load match"),
            Some(local_match.1)
        );
        assert_eq!(
            store
                .load_local_manifest(&saved.source_id)
                .expect("load manifest"),
            vec![manifest]
        );
    }

    fn seed_local_access_projection(
        store: &Store,
        saved: &StoredSource,
        access: &SourceLocalAccess,
    ) -> (Track, LocalManifestEntry) {
        assert!(
            store
                .save_source_local_access(access)
                .expect("save local access")
        );
        let album = album(1);
        let track = track(1, &album);
        let manifest = local_manifest_entry(1, "/music/old.flac");
        let generation = store.begin_sync(&saved.source_id).expect("begin seed sync");
        let mut sync = small_library_sync(&album, &track);
        sync.local_access = Some(LocalAccessUpdate {
            manifest: LocalManifestDelta {
                upserted_entries: vec![manifest.clone()],
                deleted_paths: Vec::new(),
            },
            matches: vec![(
                track.id.clone(),
                "/music/old.flac".to_string(),
                "metadata".to_string(),
            )],
        });
        store
            .commit_library_sync(&saved.source_id, generation, 1, sync)
            .expect("commit seed sync");
        (track, manifest)
    }

    fn replacement_sync(
        store: &Store,
        saved: &StoredSource,
    ) -> (i64, i64, LibrarySync, PagedResponse<Track>) {
        let committed = store
            .load_tracks(&saved.source_id, 0, 10)
            .expect("load committed tracks");
        let base_cache_revision = store
            .source_cache_revision(&saved.source_id)
            .expect("load base revision");
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let replacement_album = album(2);
        let replacement_track = track(2, &replacement_album);
        let mut sync = small_library_sync(&replacement_album, &replacement_track);
        sync.local_access = Some(LocalAccessUpdate {
            manifest: LocalManifestDelta {
                upserted_entries: vec![local_manifest_entry(2, "/music/new.flac")],
                deleted_paths: vec![PathBuf::from("/music/old.flac")],
            },
            matches: vec![(
                replacement_track.id,
                "/music/new.flac".to_string(),
                "metadata".to_string(),
            )],
        });
        (generation, base_cache_revision, sync, committed)
    }

    #[test]
    fn saving_local_access_rejects_an_older_sync() {
        let store = Store::open_memory().expect("open Store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let first = SourceLocalAccess {
            source_id: saved.source_id.clone(),
            root_path: "/music/old".to_string(),
            path_replace_from: Some("/server/old".to_string()),
            path_replace_to: Some("/music/old".to_string()),
        };
        let (old_track, _old_manifest) = seed_local_access_projection(&store, &saved, &first);
        let (generation, base_revision, sync, committed_tracks) = replacement_sync(&store, &saved);
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("load revision");
        let replacement = SourceLocalAccess {
            source_id: saved.source_id.clone(),
            root_path: "/music/new".to_string(),
            path_replace_from: Some("/server/new".to_string()),
            path_replace_to: Some("/music/new".to_string()),
        };

        assert!(
            store
                .save_source_local_access(&replacement)
                .expect("replace local access")
        );
        assert_eq!(
            store
                .source_cache_revision(&saved.source_id)
                .expect("load changed revision"),
            revision + 1
        );
        assert_eq!(
            store
                .source_local_access(&saved.source_id)
                .expect("load local access"),
            Some(replacement.clone())
        );
        assert_eq!(
            store
                .track_local_match_path(&saved.source_id, &old_track.id)
                .expect("load cleared match"),
            None
        );
        assert!(
            store
                .load_local_manifest(&saved.source_id)
                .expect("load cleared manifest")
                .is_empty()
        );
        assert!(
            !store
                .save_source_local_access(&replacement)
                .expect("save unchanged local access")
        );
        assert_eq!(
            store
                .source_cache_revision(&saved.source_id)
                .expect("load unchanged revision"),
            revision + 1
        );
        assert!(matches!(
            store.commit_library_sync(&saved.source_id, generation, base_revision, sync),
            Err(StoreError::StaleCacheRevision { .. })
        ));
        assert_eq!(
            store
                .load_tracks(&saved.source_id, 0, 10)
                .expect("reload tracks"),
            committed_tracks
        );
    }

    #[test]
    fn clearing_local_access_rejects_an_older_sync() {
        let store = Store::open_memory().expect("open Store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let access = SourceLocalAccess {
            source_id: saved.source_id.clone(),
            root_path: "/music/old".to_string(),
            path_replace_from: None,
            path_replace_to: Some("/music/old".to_string()),
        };
        let (old_track, _old_manifest) = seed_local_access_projection(&store, &saved, &access);
        let (generation, base_revision, sync, committed_tracks) = replacement_sync(&store, &saved);
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("load revision");

        assert!(
            store
                .clear_source_local_access(&saved.source_id)
                .expect("clear local access")
        );
        assert_eq!(
            store
                .source_cache_revision(&saved.source_id)
                .expect("load changed revision"),
            revision + 1
        );
        assert_eq!(
            store
                .source_local_access(&saved.source_id)
                .expect("load local access"),
            None
        );
        assert_eq!(
            store
                .track_local_match_path(&saved.source_id, &old_track.id)
                .expect("load cleared match"),
            None
        );
        assert!(
            store
                .load_local_manifest(&saved.source_id)
                .expect("load cleared manifest")
                .is_empty()
        );
        assert!(
            !store
                .clear_source_local_access(&saved.source_id)
                .expect("clear unchanged local access")
        );
        assert_eq!(
            store
                .source_cache_revision(&saved.source_id)
                .expect("load unchanged revision"),
            revision + 1
        );
        assert!(matches!(
            store.commit_library_sync(&saved.source_id, generation, base_revision, sync),
            Err(StoreError::StaleCacheRevision { .. })
        ));
        assert_eq!(
            store
                .load_tracks(&saved.source_id, 0, 10)
                .expect("reload tracks"),
            committed_tracks
        );
    }

    #[test]
    fn read_snapshot_cannot_mix_rows_across_a_commit() {
        let path = std::env::temp_dir().join(format!(
            "library-read-snapshot-{}.sqlite",
            std::process::id()
        ));
        let _cleanup = std::fs::remove_file(&path);
        let writer = Store::open(&path).expect("open writer");
        let saved = stored_source();
        writer.save_source(&saved).expect("save source");
        seed_cached_library(&writer, &saved.source_id);

        let generation = writer.begin_sync(&saved.source_id).expect("begin sync");
        let replacement_album = album(2);
        let replacement_track = track(2, &replacement_album);
        let sync = small_library_sync(&replacement_album, &replacement_track);

        let reader = Store::open(&path).expect("open reader");
        let (albums, tracks) = reader
            .read_snapshot(|reader| {
                let albums = reader.load_albums(&saved.source_id, 0, 10)?.items;
                writer.commit_library_sync(&saved.source_id, generation, 1, sync)?;
                let tracks = reader.load_tracks(&saved.source_id, 0, 10)?.items;
                Ok((albums, tracks))
            })
            .expect("read one cache revision");
        assert_eq!(albums[0].id, AlbumId::fake(1));
        assert_eq!(tracks[0].id, TrackId::fake(1));

        let albums = reader
            .load_albums(&saved.source_id, 0, 10)
            .expect("read new albums")
            .items;
        let tracks = reader
            .load_tracks(&saved.source_id, 0, 10)
            .expect("read new tracks")
            .items;
        assert_eq!(albums[0].id, replacement_album.id);
        assert_eq!(tracks[0].id, replacement_track.id);

        drop(reader);
        drop(writer);
        let _cleanup = std::fs::remove_file(&path);
        let _cleanup = std::fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
        let _cleanup = std::fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
    }

    #[test]
    fn successful_commit_replaces_cross_collection_cache_and_reports_delta() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let initial_album = album(1);
        let initial_track = track(1, &initial_album);
        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin initial sync");
        store
            .commit_library_sync(
                &saved.source_id,
                generation,
                0,
                small_library_sync(&initial_album, &initial_track),
            )
            .expect("commit initial sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("initial revision");

        let replacement_artist = artist(2, None);
        let replacement_genre = genre(2, None);
        let mut replacement_album = album(2);
        replacement_album.artist = replacement_artist.name.clone();
        replacement_album.artist_id = Some(replacement_artist.id.clone());
        replacement_album.album_artist_credits = vec![credit(
            replacement_artist.id.clone(),
            &replacement_artist.name,
        )];
        replacement_album.genres = vec![replacement_genre.name.clone()];
        let replacement_track = track(2, &replacement_album);
        let replacement_playlist = playlist(2, None);
        let playlist_detail = PlaylistDetail {
            playlist: replacement_playlist.clone(),
            tracks: vec![replacement_track.clone()],
            entries: vec![PlaylistEntry {
                entry_id: "entry-two".to_string(),
                track: replacement_track.clone(),
            }],
        };
        let folder = MusicFolder {
            id: MusicFolderId::new("folder-two"),
            name: "Folder Two".to_string(),
        };
        let home = HomeSection {
            kind: HomeSectionKind::NewlyAdded,
            albums: vec![replacement_album.clone()],
            tracks: vec![replacement_track.clone()],
        };
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let committed = LibraryObservation {
            albums: vec![replacement_album.clone()],
            tracks: vec![replacement_track.clone()],
            artists: vec![replacement_artist.clone()],
            album_artists: vec![replacement_artist.clone()],
            genres: vec![replacement_genre.clone()],
            music_folders: vec![(folder.clone(), vec![replacement_track.clone()])],
            playlists: vec![playlist_detail],
            home_sections: vec![home],
        }
        .commit(&store, &saved.source_id, generation)
        .expect("commit sync");
        assert_eq!(
            committed.delta.tracks.added,
            vec![replacement_track.id.clone()]
        );
        assert_eq!(committed.delta.tracks.deleted, vec![TrackId::fake(1)]);
        assert_eq!(
            committed.delta.albums.added,
            vec![replacement_album.id.clone()]
        );
        assert_eq!(committed.delta.albums.deleted, vec![AlbumId::fake(1)]);
        assert_eq!(
            committed.delta.artists.added,
            vec![replacement_artist.id.clone()]
        );
        assert_eq!(committed.delta.artists.deleted, vec![ArtistId::fake(1)]);
        assert_eq!(
            committed.delta.album_artists.added,
            vec![replacement_artist.id.clone()]
        );
        assert_eq!(
            committed.delta.album_artists.deleted,
            vec![ArtistId::fake(1)]
        );
        assert_eq!(
            committed.delta.genres.added,
            vec![replacement_genre.id.clone()]
        );
        assert_eq!(
            committed.delta.playlists.added,
            vec![replacement_playlist.id.clone()]
        );
        assert!(committed.delta.home_changed);
        assert!(committed.delta.folders_changed);
        let tracks = store
            .load_tracks(&saved.source_id, 0, 10)
            .expect("load committed tracks")
            .items;
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, replacement_track.id);
        assert_eq!(tracks[0].album_id, replacement_track.album_id);
        let playlist = store
            .load_playlist_detail(&saved.source_id, &replacement_playlist.id)
            .expect("load playlist")
            .expect("playlist detail");
        assert_eq!(playlist.entries.len(), 1);
        assert_eq!(playlist.entries[0].entry_id, "entry-two");
        assert_eq!(playlist.entries[0].track.id, replacement_track.id);
        assert_eq!(
            store
                .list_music_folders(&saved.source_id)
                .expect("load folders"),
            vec![folder]
        );
        let state = store.sync_state(&saved.source_id).expect("sync state");
        assert_eq!(state.cache_revision, revision + 1);
        assert!(state.last_completed_at.is_some());
        assert_eq!(state.last_all_completed_at, state.last_completed_at);
    }

    #[test]
    fn identical_full_sync_has_empty_delta() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let mut cached_album = album(1);
        cached_album.artist_credits = vec![credit(ArtistId::fake(9), "Guest Artist")];
        let cached_track = track(1, &cached_album);

        for expected_empty in [false, true] {
            let generation = store.begin_sync(&saved.source_id).expect("begin sync");
            let revision = store
                .source_cache_revision(&saved.source_id)
                .expect("cache revision");
            let committed = store
                .commit_library_sync(
                    &saved.source_id,
                    generation,
                    revision,
                    small_library_sync(&cached_album, &cached_track),
                )
                .expect("commit sync");
            assert_eq!(committed.delta.is_empty(), expected_empty);
        }
    }

    #[test]
    fn large_identical_full_sync_stays_bounded() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let album = album(1);
        let tracks = (1..=2_500)
            .map(|number| track(number, &album))
            .collect::<Vec<_>>();
        let build_sync = || LibrarySync {
            albums: vec![album.clone()],
            tracks: tracks.clone(),
            artists: Vec::new(),
            album_artists: Vec::new(),
            genres: Vec::new(),
            playlists: Vec::new(),
            home_sections: Vec::new(),
            mappings: std::iter::once(SourceObjectMapping {
                source_object_id: album.id.as_str().to_string(),
                entity_kind: SourceEntityKind::Album,
                entity_id: album.id.as_str().to_string(),
            })
            .chain(tracks.iter().map(|track| SourceObjectMapping {
                source_object_id: track.id.as_str().to_string(),
                entity_kind: SourceEntityKind::Track,
                entity_id: track.id.as_str().to_string(),
            }))
            .collect(),
            coverage: SyncCoverage::All {
                music_folders: Vec::new(),
            },
            local_access: None,
        };

        let generation = store.begin_sync(&saved.source_id).expect("begin seed sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        store
            .commit_library_sync(&saved.source_id, generation, revision, build_sync())
            .expect("commit seed sync");

        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin measured sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        let started = std::time::Instant::now();
        let commit = store
            .commit_library_sync(&saved.source_id, generation, revision, build_sync())
            .expect("commit measured sync");
        assert!(commit.delta.is_empty());
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "a warm 2,500-track no-op sync took {elapsed:?}"
        );
    }

    #[test]
    fn selected_folder_filter_does_not_make_identical_home_look_changed() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let album = album(1);
        let track = track(1, &album);
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        store
            .commit_library_sync(
                &saved.source_id,
                generation,
                0,
                small_library_sync(&album, &track),
            )
            .expect("seed library");
        let section = HomeSection {
            kind: HomeSectionKind::Explore,
            albums: Vec::new(),
            tracks: vec![track],
        };
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        store
            .replace_home_section(&saved.source_id, generation, revision, &section)
            .expect("save Home");
        let folder = MusicFolder {
            id: MusicFolderId::new("other-folder"),
            name: "Other folder".to_string(),
        };
        store
            .upsert_music_folders(&saved.source_id, std::slice::from_ref(&folder), generation)
            .expect("save folder");
        store
            .set_selected_music_folder_id(&saved.source_id, Some(&folder.id))
            .expect("select folder");
        assert!(
            store
                .load_home_sections(&saved.source_id)
                .expect("filtered Home")
                .is_empty()
        );
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");

        let commit = store
            .replace_home_section(&saved.source_id, generation, revision, &section)
            .expect("save identical Home");

        assert!(commit.delta.is_empty());
    }

    #[test]
    fn changed_track_cover_updates_relation_query_and_track_delta() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let mut album = album(1);
        let genre = genre(7, None);
        album.genres = vec![genre.name.clone()];
        let mut track = track(1, &album);
        track.image_ref = Some(image_ref("track-cover-one", "one"));

        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin first sync");
        let mut first_sync = small_library_sync(&album, &track);
        first_sync.genres = vec![genre.clone()];
        store
            .commit_library_sync(&saved.source_id, generation, 0, first_sync)
            .expect("commit first sync");

        track.image_ref = Some(image_ref("track-cover-two", "two"));
        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin second sync");
        let mut second_sync = small_library_sync(&album, &track);
        second_sync.genres = vec![genre.clone()];
        let commit = store
            .commit_library_sync(&saved.source_id, generation, 1, second_sync)
            .expect("commit second sync");
        let cached_album = store
            .load_albums(&saved.source_id, 0, 10)
            .expect("load albums")
            .items
            .into_iter()
            .find(|cached| cached.id == album.id)
            .expect("cached album");

        assert_eq!(cached_album.image_ref.as_ref(), track.image_ref.as_ref());
        let cached_track = store
            .load_track(&saved.source_id, &track.id)
            .expect("load track")
            .expect("cached track");
        assert_eq!(
            cached_track
                .album_artwork
                .as_ref()
                .and_then(|artwork| artwork.image_ref.as_ref()),
            track.image_ref.as_ref()
        );
        assert_eq!(commit.delta.tracks.cover_refs, vec![track.id.clone()]);
        assert_eq!(commit.delta.albums.cover_refs, vec![album.id.clone()]);
        assert_eq!(commit.delta.genres.cover_refs, vec![genre.id]);
    }

    #[test]
    fn changed_album_cover_invalidates_hydrated_track_artwork() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let mut album = album(1);
        album.image_ref = Some(image_ref("album-cover-one", "one"));
        let mut track = track(1, &album);
        track.image_ref = None;

        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin first sync");
        store
            .commit_library_sync(
                &saved.source_id,
                generation,
                0,
                small_library_sync(&album, &track),
            )
            .expect("commit first sync");

        album.image_ref = Some(image_ref("album-cover-two", "two"));
        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin second sync");
        let commit = store
            .commit_library_sync(
                &saved.source_id,
                generation,
                1,
                small_library_sync(&album, &track),
            )
            .expect("commit second sync");
        let cached_track = store
            .load_track(&saved.source_id, &track.id)
            .expect("load track")
            .expect("cached track");

        assert_eq!(commit.delta.albums.cover_refs, vec![album.id.clone()]);
        assert_eq!(commit.delta.tracks.cover_refs, vec![track.id.clone()]);
        assert_eq!(
            cached_track
                .album_artwork
                .as_ref()
                .and_then(|artwork| artwork.image_ref.as_ref()),
            album.image_ref.as_ref()
        );
    }

    #[test]
    fn album_artist_aliases_across_pages_keep_both_source_keys() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let mut first_artist = artist(1, None);
        first_artist.musicbrainz_artist_id = Some("shared-mbid".to_string());
        let mut second_artist = artist(2, None);
        second_artist.musicbrainz_artist_id = Some("shared-mbid".to_string());
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let sync = LibrarySync {
            albums: Vec::new(),
            tracks: Vec::new(),
            artists: Vec::new(),
            album_artists: vec![first_artist.clone(), second_artist.clone()],
            genres: Vec::new(),
            playlists: Vec::new(),
            home_sections: Vec::new(),
            mappings: vec![
                SourceObjectMapping {
                    source_object_id: "native-one".to_string(),
                    entity_kind: SourceEntityKind::AlbumArtist,
                    entity_id: first_artist.id.as_str().to_string(),
                },
                SourceObjectMapping {
                    source_object_id: "native-two".to_string(),
                    entity_kind: SourceEntityKind::AlbumArtist,
                    entity_id: second_artist.id.as_str().to_string(),
                },
            ],
            coverage: SyncCoverage::All {
                music_folders: Vec::new(),
            },
            local_access: None,
        };
        store
            .commit_library_sync(&saved.source_id, generation, 0, sync)
            .expect("commit sync");

        let artists = store
            .load_artists(&saved.source_id, true, 0, 10)
            .expect("load album artists")
            .items;
        assert_eq!(artists.len(), 1);
        for key in ["native-one", "native-two"] {
            assert_eq!(
                store
                    .source_object_mappings(&saved.source_id, key)
                    .expect("load source mapping")[0]
                    .entity_id,
                artists[0].id.as_str()
            );
        }
    }

    #[test]
    fn folder_membership_change_is_part_of_the_commit_delta() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let cached_album = album(1);
        let cached_track = track(1, &cached_album);
        let first_folder = MusicFolder {
            id: MusicFolderId::new("folder-one"),
            name: "Folder One".to_string(),
        };
        let second_folder = MusicFolder {
            id: MusicFolderId::new("folder-two"),
            name: "Folder Two".to_string(),
        };

        for (folder, expect_empty) in [(&first_folder, false), (&second_folder, false)] {
            let generation = store.begin_sync(&saved.source_id).expect("begin sync");
            let mut sync = small_library_sync(&cached_album, &cached_track);
            sync.mappings.extend([
                mapping(SourceEntityKind::MusicFolder, first_folder.id.as_str()),
                mapping(SourceEntityKind::MusicFolder, second_folder.id.as_str()),
            ]);
            sync.coverage = SyncCoverage::All {
                music_folders: [&first_folder, &second_folder]
                    .into_iter()
                    .map(|candidate| MusicFolderSnapshot {
                        folder: candidate.clone(),
                        track_ids: (candidate.id == folder.id)
                            .then(|| cached_track.id.clone())
                            .into_iter()
                            .collect(),
                    })
                    .collect(),
            };
            let revision = store
                .source_cache_revision(&saved.source_id)
                .expect("cache revision");
            let committed = store
                .commit_library_sync(&saved.source_id, generation, revision, sync)
                .expect("commit sync");
            assert_eq!(committed.delta.is_empty(), expect_empty);
            assert!(committed.delta.folders_changed);
        }
    }

    #[test]
    fn deleted_track_reports_store_playlist_repair() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let cached_album = album(1);
        let cached_track = track(1, &cached_album);
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        store
            .commit_library_sync(
                &saved.source_id,
                generation,
                0,
                small_library_sync(&cached_album, &cached_track),
            )
            .expect("seed library");

        let stored_playlist = playlist(9, None);
        store
            .upsert_playlists_with_mode(
                &saved.source_id,
                std::slice::from_ref(&stored_playlist),
                PlaylistWriteMode::StoreOwned,
            )
            .expect("save Store playlist");
        store
            .upsert_playlist_entries_with_mode(
                &saved.source_id,
                &stored_playlist.id,
                &[PlaylistEntry {
                    entry_id: "stored-entry".to_string(),
                    track: cached_track.clone(),
                }],
                PlaylistWriteMode::StoreOwned,
            )
            .expect("save Store playlist entry");

        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let mut sync = small_library_sync(&cached_album, &cached_track);
        sync.tracks.clear();
        sync.mappings
            .retain(|mapping| mapping.entity_kind != SourceEntityKind::Track);
        let committed = store
            .commit_library_sync(&saved.source_id, generation, 1, sync)
            .expect("commit sync");

        assert_eq!(
            committed.delta.playlists.entries,
            vec![stored_playlist.id.clone()]
        );
        assert_eq!(
            committed.delta.playlists.cover_refs,
            vec![stored_playlist.id.clone()]
        );
        let stored_playlist = store
            .load_playlists(&saved.source_id, 0, 10)
            .expect("load playlists")
            .items
            .into_iter()
            .find(|playlist| playlist.id == stored_playlist.id)
            .expect("Store playlist remains");
        assert_eq!(stored_playlist.track_count, 0);
    }

    #[test]
    fn changed_cache_revision_rejects_an_older_commit() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        seed_cached_library(&store, &saved.source_id);
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let staged_album = album(2);
        let staged_track = track(2, &staged_album);
        let sync = small_library_sync(&staged_album, &staged_track);

        store
            .write_batch(|_| {
                let revision = store.source_cache_revision(&saved.source_id)?;
                let mut live_track = track(1, &album(1));
                live_track.title = "Live mutation".to_string();
                store.upsert_tracks(
                    &saved.source_id,
                    std::slice::from_ref(&live_track),
                    generation,
                )?;
                store.advance_source_cache_revision(&saved.source_id, revision)?;
                Ok(())
            })
            .expect("commit live mutation");

        let error = store
            .commit_library_sync(&saved.source_id, generation, 1, sync)
            .expect_err("reject stale stage");
        assert!(matches!(error, StoreError::StaleCacheRevision { .. }));
        let tracks = store
            .load_tracks(&saved.source_id, 0, 10)
            .expect("load live tracks")
            .items;
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Live mutation");
    }

    #[test]
    fn local_commit_returns_the_common_sync_result() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let cached_album = album(1);
        let cached_track = track(1, &cached_album);
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let base_cache_revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");

        let committed = store
            .commit_local_library_delta(
                &saved.source_id,
                generation,
                base_cache_revision,
                true,
                LocalLibraryDelta {
                    tracks: vec![cached_track.clone()],
                    current_album_ids: vec![cached_album.id.clone()],
                    dirty_albums: vec![cached_album.clone()],
                    ..LocalLibraryDelta::default()
                },
            )
            .expect("commit local sync");

        assert_eq!(committed.delta.tracks.added, vec![cached_track.id]);
        assert_eq!(committed.delta.albums.added, vec![cached_album.id]);
        assert!(!committed.delta.home_changed);
    }

    #[test]
    fn finite_track_update_preserves_siblings_and_complete_sync_time() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let fixture = seed_finite_fixture(&store, &saved.source_id);
        store
            .connection
            .execute(
                "UPDATE sync_state
                 SET last_completed_at = '2000-01-01 00:00:00',
                     last_all_completed_at = '2000-01-01 00:00:00'
                 WHERE source_id = ?1",
                params![saved.source_id.as_str()],
            )
            .expect("set old completion time");
        let mut changed = fixture.first_track.clone();
        changed.title = "Changed title".to_string();
        changed.duration_seconds += 30;
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        let committed = store
            .commit_library_sync(
                &saved.source_id,
                generation,
                revision,
                finite_sync(
                    vec![changed.clone()],
                    vec![mapping(SourceEntityKind::Track, changed.id.as_str())],
                    Vec::new(),
                    vec![TrackFolderMembership {
                        track_id: changed.id.clone(),
                        folder_ids: vec![fixture.folder.id.clone()],
                    }],
                ),
            )
            .expect("commit finite sync");

        assert_eq!(committed.cache_revision, revision + 1);
        assert!(committed.delta.tracks.fields.contains(&changed.id));
        assert_eq!(
            store
                .load_track(&saved.source_id, &changed.id)
                .expect("load changed track")
                .expect("changed track exists")
                .title,
            "Changed title"
        );
        let mut sibling = store
            .load_track(&saved.source_id, &fixture.second_track.id)
            .expect("load sibling")
            .expect("sibling exists");
        sibling.album_artwork = None;
        assert_eq!(sibling, fixture.second_track);
        let album = store
            .load_albums(&saved.source_id, 0, 10)
            .expect("load albums")
            .items
            .into_iter()
            .find(|album| album.id == fixture.first_album.id)
            .expect("updated album");
        assert_eq!(album.duration_seconds, changed.duration_seconds);
        let state = store.sync_state(&saved.source_id).expect("sync state");
        assert_ne!(
            state.last_completed_at.as_deref(),
            Some("2000-01-01 00:00:00")
        );
        assert_eq!(
            state.last_all_completed_at.as_deref(),
            Some("2000-01-01 00:00:00")
        );
    }

    #[test]
    fn finite_track_tombstone_repairs_references_without_deleting_siblings() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let fixture = seed_finite_fixture(&store, &saved.source_id);
        let stored_playlist = playlist(9, None);
        store
            .replace_playlist_snapshot(
                &saved.source_id,
                &stored_playlist,
                &[PlaylistEntry {
                    entry_id: "stored-entry".to_string(),
                    track: fixture.first_track.clone(),
                }],
                PlaylistWriteMode::StoreOwned,
            )
            .expect("save Store playlist");
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        let committed = store
            .commit_library_sync(
                &saved.source_id,
                generation,
                revision,
                finite_sync(
                    Vec::new(),
                    Vec::new(),
                    vec![mapping(
                        SourceEntityKind::Track,
                        fixture.first_track.id.as_str(),
                    )],
                    Vec::new(),
                ),
            )
            .expect("commit tombstone");

        assert_eq!(
            committed.delta.tracks.deleted,
            vec![fixture.first_track.id.clone()]
        );
        assert!(committed.delta.home_changed);
        assert!(committed.delta.folders_changed);
        assert!(
            committed
                .delta
                .playlists
                .entries
                .contains(&fixture.playlist.id)
        );
        assert!(
            committed
                .delta
                .playlists
                .entries
                .contains(&stored_playlist.id)
        );
        assert_eq!(
            store
                .load_track(&saved.source_id, &fixture.first_track.id)
                .expect("load removed track"),
            None
        );
        let mut sibling = store
            .load_track(&saved.source_id, &fixture.second_track.id)
            .expect("load sibling")
            .expect("sibling exists");
        sibling.album_artwork = None;
        assert_eq!(sibling, fixture.second_track);
        assert!(
            store
                .playlist_entry_keys(&saved.source_id, &fixture.playlist.id)
                .expect("native entries")
                .is_empty()
        );
        assert_eq!(
            store
                .playlist_entry_keys(&saved.source_id, &stored_playlist.id)
                .expect("Store entries"),
            vec![("stored-entry".to_string(), fixture.first_track.id.clone())]
        );
        let home = store
            .load_home_sections(&saved.source_id)
            .expect("load Home");
        assert!(home.iter().all(|section| {
            section
                .tracks
                .iter()
                .all(|track| track.id != fixture.first_track.id)
        }));
        let folder_rows = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM track_music_folders
                 WHERE source_id = ?1 AND track_id = ?2",
                params![saved.source_id.as_str(), fixture.first_track.id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .expect("folder rows");
        assert_eq!(folder_rows, 0);
        assert!(
            store
                .source_object_mappings(&saved.source_id, fixture.first_track.id.as_str(),)
                .expect("source mappings")
                .is_empty()
        );

        let generation = store.begin_sync(&saved.source_id).expect("repeat sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        let repeated = store
            .commit_library_sync(
                &saved.source_id,
                generation,
                revision,
                finite_sync(
                    Vec::new(),
                    Vec::new(),
                    vec![mapping(
                        SourceEntityKind::Track,
                        fixture.first_track.id.as_str(),
                    )],
                    Vec::new(),
                ),
            )
            .expect("repeat tombstone");
        assert!(repeated.delta.is_empty());
    }

    #[test]
    fn finite_native_playlist_tombstone_preserves_store_playlists() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let fixture = seed_finite_fixture(&store, &saved.source_id);
        let stored_playlist = playlist(9, None);
        store
            .replace_playlist_snapshot(
                &saved.source_id,
                &stored_playlist,
                &[PlaylistEntry {
                    entry_id: "stored-entry".to_string(),
                    track: fixture.second_track,
                }],
                PlaylistWriteMode::StoreOwned,
            )
            .expect("save Store playlist");
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        let committed = store
            .commit_library_sync(
                &saved.source_id,
                generation,
                revision,
                finite_sync(
                    Vec::new(),
                    Vec::new(),
                    vec![mapping(
                        SourceEntityKind::Playlist,
                        fixture.playlist.id.as_str(),
                    )],
                    Vec::new(),
                ),
            )
            .expect("commit tombstone");

        assert_eq!(
            committed.delta.playlists.deleted,
            vec![fixture.playlist.id.clone()]
        );
        let playlists = store
            .load_playlists(&saved.source_id, 0, 10)
            .expect("load playlists")
            .items;
        assert!(
            playlists
                .iter()
                .all(|playlist| playlist.id != fixture.playlist.id)
        );
        assert!(
            playlists
                .iter()
                .any(|playlist| playlist.id == stored_playlist.id)
        );
    }

    #[test]
    fn finite_tombstone_preserves_an_alias_and_rejects_remapping_it() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let fixture = seed_finite_fixture(&store, &saved.source_id);
        store
            .connection
            .execute(
                "INSERT INTO source_objects (
                     source_id, source_object_id, entity_kind, entity_id,
                     source_object_kind, sync_generation
                 ) VALUES (?1, 'track-alias', 'track', ?2, 'source', 1)",
                params![saved.source_id.as_str(), fixture.first_track.id.as_str()],
            )
            .expect("seed alias");

        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        let committed = store
            .commit_library_sync(
                &saved.source_id,
                generation,
                revision,
                finite_sync(
                    Vec::new(),
                    Vec::new(),
                    vec![mapping(
                        SourceEntityKind::Track,
                        fixture.first_track.id.as_str(),
                    )],
                    Vec::new(),
                ),
            )
            .expect("commit tombstone");

        assert!(committed.delta.tracks.deleted.is_empty());
        let mut aliased = store
            .load_track(&saved.source_id, &fixture.first_track.id)
            .expect("load aliased track")
            .expect("aliased track exists");
        aliased.album_artwork = None;
        assert_eq!(aliased, fixture.first_track);
        assert!(
            store
                .source_object_mappings(&saved.source_id, fixture.first_track.id.as_str())
                .expect("load removed mapping")
                .is_empty()
        );

        let generation = store.begin_sync(&saved.source_id).expect("begin remap");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        let error = store
            .commit_library_sync(
                &saved.source_id,
                generation,
                revision,
                finite_sync(
                    vec![fixture.second_track.clone()],
                    vec![SourceObjectMapping {
                        source_object_id: "track-alias".to_string(),
                        entity_kind: SourceEntityKind::Track,
                        entity_id: fixture.second_track.id.as_str().to_string(),
                    }],
                    Vec::new(),
                    vec![TrackFolderMembership {
                        track_id: fixture.second_track.id.clone(),
                        folder_ids: vec![fixture.folder.id],
                    }],
                ),
            )
            .expect_err("reject remap");
        assert!(matches!(error, StoreError::NeedsFullSync));
        assert_eq!(
            store
                .source_object_mappings(&saved.source_id, "track-alias")
                .expect("load alias")[0]
                .entity_id,
            fixture.first_track.id.as_str()
        );
    }

    #[test]
    fn full_commit_prunes_stale_native_source_mappings() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let fixture = seed_finite_fixture(&store, &saved.source_id);
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let removed_album_id = fixture.first_album.id.clone();
        let removed_track_id = fixture.first_track.id.clone();
        let commit = LibraryObservation {
            albums: vec![fixture.second_album],
            tracks: vec![fixture.second_track],
            ..LibraryObservation::default()
        }
        .commit(&store, &saved.source_id, generation)
        .expect("commit complete sync");

        assert!(commit.delta.albums.cover_refs.contains(&removed_album_id));
        assert!(commit.delta.tracks.cover_refs.contains(&removed_track_id));

        assert!(
            store
                .source_object_mappings(&saved.source_id, fixture.first_track.id.as_str(),)
                .expect("load removed mapping")
                .is_empty()
        );
        assert_eq!(
            store
                .load_track(&saved.source_id, &fixture.first_track.id)
                .expect("load removed track"),
            None
        );
    }

    #[test]
    fn full_commit_keeps_a_track_without_an_enumerated_album() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let album = album(1);
        let track = track(1, &album);
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");

        LibraryObservation {
            tracks: vec![track.clone()],
            ..LibraryObservation::default()
        }
        .commit(&store, &saved.source_id, generation)
        .expect("commit track-only library");

        let (loaded_album, loaded_tracks) = store
            .load_album_detail(&saved.source_id, &track.album_id)
            .expect("load synthetic album")
            .expect("synthetic album exists");
        assert_eq!(loaded_album.id, track.album_id);
        assert_eq!(loaded_album.title, track.album);
        assert_eq!(loaded_tracks, vec![track]);
    }

    #[test]
    fn local_coverage_owns_verification_time_and_cue_dependencies() {
        let store = Store::open_memory().expect("open store");
        let saved = stored_source();
        store.save_source(&saved).expect("save source");
        let first = LocalCueDependency {
            cue_path: PathBuf::from("/music/album.cue"),
            source_path: PathBuf::from("/music/missing.flac"),
        };
        let second = LocalCueDependency {
            cue_path: PathBuf::from("/music/other.cue"),
            source_path: PathBuf::from("/music/other.flac"),
        };
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        store
            .commit_local_library_delta(
                &saved.source_id,
                generation,
                revision,
                true,
                LocalLibraryDelta {
                    cue_dependencies: vec![first.clone()],
                    ..LocalLibraryDelta::default()
                },
            )
            .expect("commit complete Local scan");
        assert_eq!(
            store
                .load_local_cue_dependencies(&saved.source_id)
                .expect("load dependencies"),
            vec![first.clone()]
        );
        store
            .connection
            .execute(
                "UPDATE sync_state
                 SET last_completed_at = '2000-01-01 00:00:00',
                     last_all_completed_at = '2000-01-01 00:00:00'
                 WHERE source_id = ?1",
                params![saved.source_id.as_str()],
            )
            .expect("set old completion time");
        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin bounded sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        store
            .commit_local_library_delta(
                &saved.source_id,
                generation,
                revision,
                false,
                LocalLibraryDelta {
                    cue_dependencies: vec![second.clone()],
                    ..LocalLibraryDelta::default()
                },
            )
            .expect("commit bounded Local scan");
        assert_eq!(
            store
                .load_local_cue_dependencies(&saved.source_id)
                .expect("load retained dependencies"),
            vec![first]
        );
        assert_eq!(
            store
                .sync_state(&saved.source_id)
                .expect("sync state")
                .last_all_completed_at
                .as_deref(),
            Some("2000-01-01 00:00:00")
        );
        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin complete sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        store
            .commit_local_library_delta(
                &saved.source_id,
                generation,
                revision,
                true,
                LocalLibraryDelta {
                    cue_dependencies: vec![second.clone()],
                    ..LocalLibraryDelta::default()
                },
            )
            .expect("replace dependencies");
        assert_eq!(
            store
                .load_local_cue_dependencies(&saved.source_id)
                .expect("load replaced dependencies"),
            vec![second]
        );
        let generation = store
            .begin_sync(&saved.source_id)
            .expect("begin clearing sync");
        let revision = store
            .source_cache_revision(&saved.source_id)
            .expect("cache revision");
        store
            .commit_local_library_delta(
                &saved.source_id,
                generation,
                revision,
                true,
                LocalLibraryDelta::default(),
            )
            .expect("clear dependencies");
        assert!(
            store
                .load_local_cue_dependencies(&saved.source_id)
                .expect("load cleared dependencies")
                .is_empty()
        );
    }
}
