use super::identity::{
    delete_local_track_entity_rows, delete_source_track_entity_rows, local_file_source_object_id,
    source_object_from_row, upsert_source_object_on_connection,
};
use super::sources::{COLLECTION_COVER_GENRE, collect_rows, image_ref_from_row, u32_from_i64};
use super::*;

#[derive(Clone, Debug, Default)]
pub struct LocalManifestDelta {
    pub upserted_entries: Vec<LocalManifestEntry>,
    pub deleted_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct LocalLibraryDelta {
    pub tracks: Vec<Track>,
    pub deleted_track_ids: Vec<TrackId>,
    pub current_album_ids: Vec<AlbumId>,
    pub current_artist_ids: Vec<ArtistId>,
    pub current_album_artist_ids: Vec<ArtistId>,
    pub current_genre_ids: Vec<GenreId>,
    pub dirty_albums: Vec<Album>,
    pub dirty_artists: Vec<Artist>,
    pub dirty_album_artists: Vec<Artist>,
    pub dirty_genres: Vec<Genre>,
    pub home_sections: Vec<HomeSection>,
    pub manifest: LocalManifestDelta,
    pub cue_track_sources: Vec<LocalCueTrackSource>,
    pub cue_dependencies: Vec<LocalCueDependency>,
}

pub(super) struct TrackDeletion {
    pub(super) playlist_ids: Vec<PlaylistId>,
    pub(super) home_changed: bool,
    pub(super) folders_changed: bool,
}

pub(super) enum TrackEntitySource {
    Local,
    Source,
}

impl Store {
    pub fn load_raw_track_image_refs(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<HashMap<TrackId, Option<ImageRef>>> {
        let mut statement = self.connection.prepare(
            "
            SELECT track_id,
                   CASE WHEN image_origin = 'source' THEN image_item_id END,
                   CASE WHEN image_origin = 'source' THEN image_tag END
            FROM tracks
            WHERE source_id = ?1
            ",
        )?;
        Ok(
            collect_rows(statement.query_map(params![source_id.as_str()], |row| {
                Ok((
                    TrackId::new(row.get::<_, String>(0)?),
                    image_ref_from_row(row, 1, 2)?,
                ))
            })?)?
            .into_iter()
            .collect(),
        )
    }

    pub fn load_raw_album_image_refs(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<HashMap<AlbumId, Option<ImageRef>>> {
        let mut statement = self.connection.prepare(
            "
            SELECT album_id,
                   CASE WHEN image_origin = 'source' THEN image_item_id END,
                   CASE WHEN image_origin = 'source' THEN image_tag END
            FROM albums
            WHERE source_id = ?1
            ",
        )?;
        Ok(
            collect_rows(statement.query_map(params![source_id.as_str()], |row| {
                Ok((
                    AlbumId::new(row.get::<_, String>(0)?),
                    image_ref_from_row(row, 1, 2)?,
                ))
            })?)?
            .into_iter()
            .collect(),
        )
    }

    pub fn load_raw_artist_image_refs(
        &self,
        source_id: &SourceId,
        album_artist: bool,
    ) -> StoreResult<HashMap<ArtistId, Option<ImageRef>>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let mut statement = self.connection.prepare(&format!(
            "
            SELECT artist_id,
                   CASE WHEN image_origin = 'source' THEN image_item_id END,
                   CASE WHEN image_origin = 'source' THEN image_tag END
            FROM {table}
            WHERE source_id = ?1
            "
        ))?;
        Ok(
            collect_rows(statement.query_map(params![source_id.as_str()], |row| {
                Ok((
                    ArtistId::new(row.get::<_, String>(0)?),
                    image_ref_from_row(row, 1, 2)?,
                ))
            })?)?
            .into_iter()
            .collect(),
        )
    }

    pub fn load_local_manifest(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<Vec<LocalManifestEntry>> {
        let mut statement = self.connection.prepare(
            "
            SELECT f.path, f.root_path, f.relative_path, f.file_size,
                   f.mtime_seconds, f.mtime_nanos, f.inode, f.device,
                   d.track_json, d.album_artist, d.musicbrainz_album_id,
                   d.musicbrainz_release_group_id, d.cover_kind, d.cover_path,
                   d.cover_embedded_index, d.cover_revision, f.metadata_hash, f.search_hash,
                   a.cover_item_id
            FROM local_file_manifest f
            JOIN local_track_manifest_data d
              ON d.source_id = f.source_id
             AND d.track_id = f.track_id
             AND d.manifest_version = f.manifest_version
            LEFT JOIN local_artwork_manifest a
              ON a.source_id = f.source_id
             AND a.revision = d.cover_revision
             AND a.source_path = d.cover_path
             AND a.source_kind = d.cover_kind
             AND a.manifest_version = f.manifest_version
            WHERE f.source_id = ?1
              AND f.manifest_version = ?2
            ORDER BY f.path
            ",
        )?;
        let rows = collect_rows(statement.query_map(
            params![source_id.as_str(), LOCAL_MANIFEST_VERSION],
            |row| {
                let track_json: String = row.get(8)?;
                Ok((
                    LocalFileFacts {
                        path: PathBuf::from(row.get::<_, String>(0)?),
                        root_path: PathBuf::from(row.get::<_, String>(1)?),
                        relative_path: row.get(2)?,
                        file_size: u64_from_i64(row.get(3)?),
                        mtime_seconds: row.get(4)?,
                        mtime_nanos: u32_from_i64(row.get(5)?),
                        inode: row.get::<_, Option<i64>>(6)?.map(u64_from_i64),
                        device: row.get::<_, Option<i64>>(7)?.map(u64_from_i64),
                    },
                    track_json,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, String>(17)?,
                    row.get::<_, Option<String>>(18)?,
                ))
            },
        )?)?;
        Ok(rows
            .into_iter()
            .filter_map(
                |(
                    facts,
                    track_json,
                    album_artist,
                    musicbrainz_album_id,
                    musicbrainz_release_group_id,
                    cover_kind,
                    cover_path,
                    embedded_index,
                    cover_revision,
                    metadata_hash,
                    search_hash,
                    cover_item_id,
                )| {
                    let track = serde_json::from_str::<Track>(&track_json).ok()?;
                    let cover = match (cover_kind.as_deref(), cover_path, cover_revision) {
                        (Some(kind), Some(path), Some(revision)) => Some(LocalManifestCover {
                            item_id: cover_item_id?,
                            kind: local_manifest_cover_kind(kind)?,
                            source_path: PathBuf::from(path),
                            revision,
                            embedded_index,
                        }),
                        _ => None,
                    };
                    Some(LocalManifestEntry {
                        facts,
                        track,
                        album_artist,
                        musicbrainz_album_id,
                        musicbrainz_release_group_id,
                        cover,
                        metadata_hash,
                        search_hash,
                    })
                },
            )
            .collect())
    }

    pub fn load_local_cue_dependencies(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<Vec<LocalCueDependency>> {
        let mut statement = self.connection.prepare(
            "SELECT cue_path, source_path
             FROM source_objects
             WHERE source_id = ?1
               AND source_object_kind = 'cue_dependency'
             ORDER BY cue_path, source_path",
        )?;
        collect_rows(statement.query_map(params![source_id.as_str()], |row| {
            Ok(LocalCueDependency {
                cue_path: PathBuf::from(row.get::<_, String>(0)?),
                source_path: PathBuf::from(row.get::<_, String>(1)?),
            })
        })?)
    }

    pub fn load_track_source_object(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<Option<SourceObject>> {
        let mut statement = self.connection.prepare(
            "
            SELECT source_object_id, entity_kind, entity_id, source_object_kind, source_path,
                   parent_source_object_id, cue_path, cue_revision, cue_track_index,
                   segment_start_ms, segment_end_ms, sync_generation
            FROM (
                SELECT 0 AS priority, source.source_object_id, source.entity_kind,
                       source.entity_id, source.source_object_kind, source.source_path,
                       source.parent_source_object_id, source.cue_path, source.cue_revision,
                       source.cue_track_index, source.segment_start_ms, source.segment_end_ms,
                       source.sync_generation
                FROM source_objects source
                WHERE source.source_id = ?1
                  AND source.entity_kind = 'track'
                  AND source.entity_id = ?2
                  AND source.source_object_kind = 'cue_track'
                UNION ALL
                SELECT 1 AS priority, source.source_object_id, source.entity_kind,
                       source.entity_id, source.source_object_kind, source.source_path,
                       source.parent_source_object_id, source.cue_path, source.cue_revision,
                       source.cue_track_index, source.segment_start_ms, source.segment_end_ms,
                       source.sync_generation
                FROM entities entity
                JOIN source_objects source
                  ON source.source_id = entity.source_id
                 AND source.source_object_id = entity.source_object_id
                 AND source.entity_kind = entity.entity_kind
                 AND source.entity_id = entity.entity_id
                WHERE entity.source_id = ?1
                  AND entity.entity_kind = 'track'
                  AND entity.entity_id = ?2
            )
            ORDER BY priority
            LIMIT 1
            ",
        )?;
        statement
            .query_row(
                params![source_id.as_str(), track_id.as_str()],
                source_object_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub(super) fn delete_track_rows(
        &self,
        source_id: &SourceId,
        track_ids: &[TrackId],
        entity_source: TrackEntitySource,
    ) -> StoreResult<TrackDeletion> {
        if track_ids.is_empty() {
            return Ok(TrackDeletion {
                playlist_ids: Vec::new(),
                home_changed: false,
                folders_changed: false,
            });
        }
        self.write_batch(|connection| {
            let affected_playlist_ids =
                delete_playlist_track_ids(connection, source_id, track_ids)?;
            let folders_changed =
                track_rows_exist(connection, "track_music_folders", source_id, track_ids)?;
            for table in [
                "track_music_folders",
                "track_genres",
                "track_moods",
                "track_artist_links",
                "track_local_matches",
                "tracks",
            ] {
                delete_track_ids(connection, table, source_id, track_ids)?;
            }
            let home_changed =
                delete_home_track_ids(connection, "home_section_items", source_id, track_ids)? > 0;
            delete_home_track_ids(
                connection,
                "home_section_prefetch_items",
                source_id,
                track_ids,
            )?;
            match entity_source {
                TrackEntitySource::Local => {
                    delete_local_track_entity_rows(connection, source_id, track_ids)?;
                }
                TrackEntitySource::Source => {
                    delete_source_track_entity_rows(connection, source_id, track_ids)?;
                }
            }
            delete_track_fts_rows(connection, source_id, track_ids)?;
            for playlist_id in &affected_playlist_ids {
                super::library_auxiliary_cache::refresh_playlist_stats(
                    connection,
                    source_id,
                    playlist_id,
                )?;
                super::library_auxiliary_cache::refresh_playlist_refs(
                    connection,
                    source_id,
                    playlist_id,
                )?;
            }
            Ok(TrackDeletion {
                playlist_ids: affected_playlist_ids,
                home_changed,
                folders_changed,
            })
        })
    }

    pub fn commit_local_library_delta(
        &self,
        source_id: &SourceId,
        generation: i64,
        base_cache_revision: i64,
        complete_coverage: bool,
        delta: LocalLibraryDelta,
    ) -> StoreResult<SyncCommit> {
        self.finish_library_sync(
            source_id,
            generation,
            base_cache_revision,
            complete_coverage,
            || {
                self.apply_local_library_delta(
                    &self.connection,
                    source_id,
                    generation,
                    complete_coverage,
                    delta,
                )
            },
        )
    }

    fn apply_local_library_delta(
        &self,
        connection: &Connection,
        source_id: &SourceId,
        generation: i64,
        complete_coverage: bool,
        delta: LocalLibraryDelta,
    ) -> StoreResult<LibraryDelta> {
        let mut changes = LibraryDeltaCollector::new();
        changes.merge(self.upsert_home_sections_delta(
            source_id,
            &delta.home_sections,
            generation,
        )?);
        changes.merge(self.upsert_tracks_delta(source_id, &delta.tracks, generation)?);
        let mut deleted_tracks = Vec::new();
        for track_id in &delta.deleted_track_ids {
            if self.load_track_for_delta(source_id, track_id)?.is_some() {
                deleted_tracks.push(track_id.clone());
            }
        }
        let deletion = self.delete_track_rows(
            source_id,
            &delta.deleted_track_ids,
            TrackEntitySource::Local,
        )?;
        changes.merge(LibraryDelta {
            tracks: TrackDelta {
                deleted: deleted_tracks,
                ..TrackDelta::default()
            },
            playlists: PlaylistDelta {
                entries: deletion.playlist_ids.clone(),
                cover_refs: deletion.playlist_ids,
                ..PlaylistDelta::default()
            },
            home_changed: deletion.home_changed,
            folders_changed: deletion.folders_changed,
            ..LibraryDelta::default()
        });
        changes.merge(self.upsert_albums_delta(source_id, &delta.dirty_albums, generation)?);
        changes.merge(self.upsert_artists_delta(
            source_id,
            &delta.dirty_artists,
            false,
            generation,
        )?);
        changes.merge(self.upsert_artists_delta(
            source_id,
            &delta.dirty_album_artists,
            true,
            generation,
        )?);
        changes.merge(self.upsert_genres_delta(source_id, &delta.dirty_genres, generation)?);
        changes.merge(prune_stale_local_aggregate_rows(
            connection, source_id, &delta,
        )?);
        apply_local_manifest_delta_on_connection(
            connection,
            source_id,
            generation,
            &delta.manifest,
        )?;
        let changed_manifest_track_ids = delta
            .manifest
            .upserted_entries
            .iter()
            .map(|entry| entry.track.id.clone())
            .collect::<HashSet<_>>();
        let changed_cue_sources = delta
            .cue_track_sources
            .iter()
            .filter(|source| changed_manifest_track_ids.contains(&source.track_id))
            .cloned()
            .collect::<Vec<_>>();
        for cue_source in &changed_cue_sources {
            upsert_local_cue_source_on_connection(connection, source_id, generation, cue_source)?;
        }
        if complete_coverage {
            replace_local_cue_dependencies_on_connection(
                connection,
                source_id,
                generation,
                &delta.cue_dependencies,
            )?;
        }
        Ok(changes.finish())
    }
}

fn replace_local_cue_dependencies_on_connection(
    connection: &Connection,
    source_id: &SourceId,
    generation: i64,
    dependencies: &[LocalCueDependency],
) -> StoreResult<()> {
    connection.execute(
        "DELETE FROM source_objects
         WHERE source_id = ?1 AND source_object_kind = 'cue_dependency'",
        params![source_id.as_str()],
    )?;
    for dependency in dependencies {
        let cue_path = dependency.cue_path.to_string_lossy();
        let source_path = dependency.source_path.to_string_lossy();
        upsert_source_object_on_connection(
            connection,
            source_id,
            &SourceObject {
                source_object_id: format!("local:cue-dependency:{cue_path}\u{1f}{source_path}"),
                entity_kind: None,
                entity_id: None,
                source_object_kind: "cue_dependency".to_string(),
                source_path: Some(source_path.into_owned()),
                parent_source_object_id: None,
                cue_path: Some(cue_path.into_owned()),
                cue_revision: None,
                cue_track_index: None,
                segment_start_ms: None,
                segment_end_ms: None,
                sync_generation: generation,
            },
        )?;
    }
    Ok(())
}

fn upsert_local_cue_source_on_connection(
    connection: &Connection,
    source_id: &SourceId,
    generation: i64,
    source: &LocalCueTrackSource,
) -> StoreResult<()> {
    let parent_id = local_file_source_object_id(&source.root_path, &source.relative_path);
    upsert_source_object_on_connection(
        connection,
        source_id,
        &SourceObject {
            source_object_id: parent_id.clone(),
            entity_kind: None,
            entity_id: None,
            source_object_kind: "local_file".to_string(),
            source_path: Some(source.source_path.clone()),
            parent_source_object_id: None,
            cue_path: None,
            cue_revision: None,
            cue_track_index: None,
            segment_start_ms: None,
            segment_end_ms: None,
            sync_generation: generation,
        },
    )?;
    upsert_source_object_on_connection(
        connection,
        source_id,
        &SourceObject {
            source_object_id: source.source_object_id.clone(),
            entity_kind: Some("track".to_string()),
            entity_id: Some(source.track_id.as_str().to_string()),
            source_object_kind: "cue_track".to_string(),
            source_path: Some(source.source_path.clone()),
            parent_source_object_id: Some(parent_id),
            cue_path: Some(source.cue_path.clone()),
            cue_revision: Some(source.cue_revision.clone()),
            cue_track_index: Some(source.cue_track_index),
            segment_start_ms: Some(source.segment_start_ms),
            segment_end_ms: Some(source.segment_end_ms),
            sync_generation: generation,
        },
    )?;
    connection.execute(
        "
        INSERT INTO entities (source_id, entity_kind, entity_id, source, source_object_id)
        VALUES (?1, 'track', ?2, 'local', ?3)
        ON CONFLICT(source_id, entity_kind, entity_id) DO UPDATE SET
            source = 'local',
            source_object_id = excluded.source_object_id,
            updated_at = CURRENT_TIMESTAMP
        ",
        params![
            source_id.as_str(),
            source.track_id.as_str(),
            source.source_object_id.as_str(),
        ],
    )?;
    Ok(())
}

pub(super) fn apply_local_manifest_delta_on_connection(
    connection: &Connection,
    source_id: &SourceId,
    generation: i64,
    delta: &LocalManifestDelta,
) -> StoreResult<()> {
    let mut changed_paths = HashSet::new();
    let mut changed_track_ids = HashSet::new();
    for entry in &delta.upserted_entries {
        let path = entry.facts.path.to_string_lossy().into_owned();
        if !changed_paths.insert(path.clone()) {
            return Err(StoreError::InvalidSyncBatch(format!(
                "duplicate Local manifest path: {path}"
            )));
        }
        if !changed_track_ids.insert(entry.track.id.clone()) {
            return Err(StoreError::InvalidSyncBatch(format!(
                "duplicate Local manifest track: {}",
                entry.track.id.as_str()
            )));
        }
    }
    let mut deleted_paths = HashSet::new();
    for path in &delta.deleted_paths {
        let path = path.to_string_lossy().into_owned();
        if !deleted_paths.insert(path.clone()) {
            return Err(StoreError::InvalidSyncBatch(format!(
                "duplicate deleted Local manifest path: {path}"
            )));
        }
        if changed_paths.contains(&path) {
            return Err(StoreError::InvalidSyncBatch(format!(
                "Local manifest path is both changed and deleted: {path}"
            )));
        }
    }

    let mut delete_file =
        connection.prepare("DELETE FROM local_file_manifest WHERE source_id = ?1 AND path = ?2")?;
    for path in &delta.deleted_paths {
        delete_file.execute(params![source_id.as_str(), path.to_string_lossy().as_ref()])?;
    }
    drop(delete_file);

    upsert_local_manifest_entries_on_connection(
        connection,
        source_id,
        generation,
        &delta.upserted_entries,
    )?;
    connection.execute(
        "
        DELETE FROM local_track_manifest_data
        WHERE source_id = ?1
          AND track_id NOT IN (
              SELECT track_id
              FROM local_file_manifest
              WHERE source_id = ?1
          )
        ",
        params![source_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM local_artwork_manifest AS artwork
        WHERE artwork.source_id = ?1
          AND NOT EXISTS (
              SELECT 1
              FROM local_track_manifest_data AS track
              WHERE track.source_id = artwork.source_id
                AND track.cover_kind = artwork.source_kind
                AND track.cover_path = artwork.source_path
                AND track.cover_revision = artwork.revision
          )
        ",
        params![source_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM source_objects AS object
        WHERE object.source_id = ?1
          AND object.source_object_kind = 'local_file'
          AND NOT EXISTS (
              SELECT 1
              FROM local_file_manifest AS manifest
              WHERE manifest.source_id = object.source_id
                AND object.source_object_id =
                    'local:file:' || manifest.root_path || char(31) || manifest.relative_path
          )
          AND NOT EXISTS (
              SELECT 1
              FROM source_objects AS child
              WHERE child.source_id = object.source_id
                AND child.parent_source_object_id = object.source_object_id
                AND child.source_object_kind = 'cue_track'
          )
        ",
        params![source_id.as_str()],
    )?;
    Ok(())
}

fn upsert_local_manifest_entries_on_connection(
    connection: &Connection,
    source_id: &SourceId,
    generation: i64,
    entries: &[LocalManifestEntry],
) -> StoreResult<()> {
    let mut upsert_file = connection.prepare(
        "
        INSERT OR REPLACE INTO local_file_manifest (
            source_id, manifest_version, path, root_path, relative_path,
            file_size, mtime_seconds, mtime_nanos, inode, device, content_hash,
            track_id, album_id, source_format, metadata_hash, search_hash,
            artwork_revision, scan_generation, last_tag_read_at, last_seen_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL,
                ?11, ?12, ?13, ?14, ?15, ?16, ?17, NULL, CURRENT_TIMESTAMP)
        ",
    )?;
    let mut upsert_track = connection.prepare(
        "
        INSERT OR REPLACE INTO local_track_manifest_data (
            source_id, manifest_version, track_id, track_json, album_artist,
            musicbrainz_album_id, musicbrainz_release_group_id, cover_kind,
            cover_path, cover_embedded_index, cover_revision, metadata_hash,
            search_hash, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, CURRENT_TIMESTAMP)
        ",
    )?;
    let mut update_entity_source = connection.prepare(
        "
        UPDATE entities
        SET source = 'local',
            source_object_id = ?3,
            updated_at = CURRENT_TIMESTAMP
        WHERE source_id = ?1
          AND entity_kind = 'track'
          AND entity_id = ?2
        ",
    )?;
    let mut upsert_artwork = connection.prepare(
        "
        INSERT OR REPLACE INTO local_artwork_manifest (
            source_id, cover_item_id, manifest_version, source_kind, source_path,
            source_size, mtime_seconds, mtime_nanos, content_hash, revision,
            scan_generation
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10)
        ",
    )?;
    for entry in entries {
        let track_json = serde_json::to_string(&entry.track)?;
        let (cover_kind, cover_path, embedded_index, cover_revision) =
            local_manifest_cover_parts(entry.cover.as_ref());
        upsert_file.execute(params![
            source_id.as_str(),
            LOCAL_MANIFEST_VERSION,
            entry.facts.path.to_string_lossy().as_ref(),
            entry.facts.root_path.to_string_lossy().as_ref(),
            entry.facts.relative_path.as_str(),
            i64_from_u64(entry.facts.file_size),
            entry.facts.mtime_seconds,
            i64::from(entry.facts.mtime_nanos),
            entry.facts.inode.map(i64_from_u64),
            entry.facts.device.map(i64_from_u64),
            entry.track.id.as_str(),
            entry.track.album_id.as_str(),
            entry.track.source_format.as_deref(),
            entry.metadata_hash.as_str(),
            entry.search_hash.as_str(),
            cover_revision,
            generation,
        ])?;
        upsert_track.execute(params![
            source_id.as_str(),
            LOCAL_MANIFEST_VERSION,
            entry.track.id.as_str(),
            track_json,
            entry.album_artist.as_str(),
            entry.musicbrainz_album_id.as_deref(),
            entry.musicbrainz_release_group_id.as_deref(),
            cover_kind,
            cover_path,
            embedded_index,
            cover_revision,
            entry.metadata_hash.as_str(),
            entry.search_hash.as_str(),
        ])?;
        let root_path = entry.facts.root_path.to_string_lossy();
        let source_object_id =
            local_file_source_object_id(root_path.as_ref(), &entry.facts.relative_path);
        upsert_source_object_on_connection(
            connection,
            source_id,
            &SourceObject {
                source_object_id: source_object_id.clone(),
                entity_kind: None,
                entity_id: None,
                source_object_kind: "local_file".to_string(),
                source_path: Some(entry.facts.path.to_string_lossy().into_owned()),
                parent_source_object_id: None,
                cue_path: None,
                cue_revision: None,
                cue_track_index: None,
                segment_start_ms: None,
                segment_end_ms: None,
                sync_generation: generation,
            },
        )?;
        update_entity_source.execute(params![
            source_id.as_str(),
            entry.track.id.as_str(),
            source_object_id.as_str(),
        ])?;
        if let Some(cover) = &entry.cover {
            let facts = local_artwork_source_facts(&cover.source_path);
            upsert_artwork.execute(params![
                source_id.as_str(),
                cover.item_id.as_str(),
                LOCAL_MANIFEST_VERSION,
                local_manifest_cover_kind_key(cover.kind),
                cover.source_path.to_string_lossy().as_ref(),
                facts.as_ref().map(|facts| i64_from_u64(facts.file_size)),
                facts.as_ref().map(|facts| facts.mtime_seconds),
                facts.as_ref().map(|facts| i64::from(facts.mtime_nanos)),
                cover.revision.as_str(),
                generation,
            ])?;
        }
    }
    Ok(())
}

pub(super) fn clear_local_manifest_on_connection(
    connection: &Connection,
    source_id: &SourceId,
) -> StoreResult<()> {
    for table in [
        "local_artwork_manifest",
        "local_track_manifest_data",
        "local_file_manifest",
    ] {
        let sql = format!("DELETE FROM {table} WHERE source_id = ?1");
        connection.execute(&sql, params![source_id.as_str()])?;
    }
    connection.execute(
        "DELETE FROM source_objects WHERE source_id = ?1 AND source_object_kind IN ('local_file', 'cue_track', 'cue_dependency')",
        params![source_id.as_str()],
    )?;
    Ok(())
}

fn local_manifest_cover_parts(
    cover: Option<&LocalManifestCover>,
) -> (
    Option<&'static str>,
    Option<String>,
    Option<i64>,
    Option<&str>,
) {
    match cover {
        Some(cover) => (
            Some(local_manifest_cover_kind_key(cover.kind)),
            Some(cover.source_path.to_string_lossy().into_owned()),
            cover.embedded_index,
            Some(cover.revision.as_str()),
        ),
        None => (None, None, None, None),
    }
}

fn local_manifest_cover_kind(value: &str) -> Option<LocalManifestCoverKind> {
    match value {
        "file" => Some(LocalManifestCoverKind::File),
        "embedded" => Some(LocalManifestCoverKind::Embedded),
        _ => None,
    }
}

fn local_manifest_cover_kind_key(kind: LocalManifestCoverKind) -> &'static str {
    match kind {
        LocalManifestCoverKind::File => "file",
        LocalManifestCoverKind::Embedded => "embedded",
    }
}

fn local_artwork_source_facts(path: &Path) -> Option<LocalArtworkSourceFacts> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(LocalArtworkSourceFacts {
        file_size: metadata.len(),
        mtime_seconds: duration.as_secs().min(i64::MAX as u64) as i64,
        mtime_nanos: duration.subsec_nanos(),
    })
}

struct LocalArtworkSourceFacts {
    file_size: u64,
    mtime_seconds: i64,
    mtime_nanos: u32,
}

fn delete_track_ids(
    connection: &Connection,
    table: &str,
    source_id: &SourceId,
    track_ids: &[TrackId],
) -> StoreResult<()> {
    for chunk in track_ids.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("DELETE FROM {table} WHERE source_id = ? AND track_id IN ({placeholders})");
        let mut values = vec![Value::Text(source_id.as_str().to_string())];
        values.extend(
            chunk
                .iter()
                .map(|track_id| Value::Text(track_id.as_str().to_string())),
        );
        connection.execute(&sql, params_from_iter(values))?;
    }
    Ok(())
}

fn delete_playlist_track_ids(
    connection: &Connection,
    source_id: &SourceId,
    track_ids: &[TrackId],
) -> StoreResult<Vec<PlaylistId>> {
    let mut affected_playlist_ids = Vec::<PlaylistId>::new();
    for chunk in track_ids.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let mut values = vec![Value::Text(source_id.as_str().to_string())];
        values.extend(
            chunk
                .iter()
                .map(|track_id| Value::Text(track_id.as_str().to_string())),
        );
        let sql = format!(
            "
            SELECT DISTINCT playlist_id
            FROM playlist_tracks
            WHERE source_id = ?
              AND track_id IN ({placeholders})
            "
        );
        let mut statement = connection.prepare(&sql)?;
        for playlist_id in collect_rows(statement.query_map(params_from_iter(values), |row| {
            row.get::<_, String>(0).map(PlaylistId::new)
        })?)? {
            if !affected_playlist_ids.contains(&playlist_id) {
                affected_playlist_ids.push(playlist_id);
            }
        }

        let sql = format!(
            "
            DELETE FROM playlist_tracks
            WHERE source_id = ?
              AND track_id IN ({placeholders})
              AND playlist_id IN (
                  SELECT playlist_id
                  FROM playlists
                  WHERE source_id = ?
                    AND owner = 'native'
              )
            "
        );
        let mut values = vec![Value::Text(source_id.as_str().to_string())];
        values.extend(
            chunk
                .iter()
                .map(|track_id| Value::Text(track_id.as_str().to_string())),
        );
        values.push(Value::Text(source_id.as_str().to_string()));
        connection.execute(&sql, params_from_iter(values))?;
    }
    affected_playlist_ids.sort();
    Ok(affected_playlist_ids)
}

fn track_rows_exist(
    connection: &Connection,
    table: &str,
    source_id: &SourceId,
    track_ids: &[TrackId],
) -> StoreResult<bool> {
    for chunk in track_ids.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT EXISTS (
                 SELECT 1 FROM {table}
                 WHERE source_id = ? AND track_id IN ({placeholders})
             )"
        );
        let mut values = vec![Value::Text(source_id.as_str().to_string())];
        values.extend(
            chunk
                .iter()
                .map(|track_id| Value::Text(track_id.as_str().to_string())),
        );
        if connection.query_row(&sql, params_from_iter(values), |row| row.get(0))? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn delete_home_track_ids(
    connection: &Connection,
    table: &str,
    source_id: &SourceId,
    track_ids: &[TrackId],
) -> StoreResult<usize> {
    let mut deleted = 0;
    for chunk in track_ids.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "DELETE FROM {table} WHERE source_id = ? AND item_type = 'track' AND item_id IN ({placeholders})"
        );
        let mut values = vec![Value::Text(source_id.as_str().to_string())];
        values.extend(
            chunk
                .iter()
                .map(|track_id| Value::Text(track_id.as_str().to_string())),
        );
        deleted += connection.execute(&sql, params_from_iter(values))?;
    }
    Ok(deleted)
}

fn delete_track_fts_rows(
    connection: &Connection,
    source_id: &SourceId,
    track_ids: &[TrackId],
) -> StoreResult<()> {
    for chunk in track_ids.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "DELETE FROM library_fts WHERE source_id = ? AND item_type = 'track' AND item_id IN ({placeholders})"
        );
        let mut values = vec![Value::Text(source_id.as_str().to_string())];
        values.extend(
            chunk
                .iter()
                .map(|track_id| Value::Text(track_id.as_str().to_string())),
        );
        connection.execute(&sql, params_from_iter(values))?;
    }
    Ok(())
}

fn prune_stale_local_aggregate_rows(
    connection: &Connection,
    source_id: &SourceId,
    delta: &LocalLibraryDelta,
) -> StoreResult<LibraryDelta> {
    let mut pruned = LibraryDelta::default();
    replace_temp_id_set(
        connection,
        "local_current_album_ids",
        delta.current_album_ids.iter().map(AlbumId::as_str),
    )?;
    pruned.albums.deleted = ids_not_in_temp(
        connection,
        "albums",
        "album_id",
        source_id,
        "local_current_album_ids",
    )?
    .into_iter()
    .map(AlbumId::new)
    .collect();
    delete_rows_not_in_temp(
        connection,
        "album_genres",
        "album_id",
        source_id,
        "local_current_album_ids",
    )?;
    delete_rows_not_in_temp(
        connection,
        "album_artist_links",
        "album_id",
        source_id,
        "local_current_album_ids",
    )?;
    delete_rows_not_in_temp(
        connection,
        "albums",
        "album_id",
        source_id,
        "local_current_album_ids",
    )?;
    delete_fts_not_in_temp(connection, source_id, "album", "local_current_album_ids")?;

    replace_temp_id_set(
        connection,
        "local_current_artist_ids",
        delta.current_artist_ids.iter().map(ArtistId::as_str),
    )?;
    pruned.artists.deleted = ids_not_in_temp(
        connection,
        "artists",
        "artist_id",
        source_id,
        "local_current_artist_ids",
    )?
    .into_iter()
    .map(ArtistId::new)
    .collect();
    delete_rows_not_in_temp(
        connection,
        "artists",
        "artist_id",
        source_id,
        "local_current_artist_ids",
    )?;
    delete_fts_not_in_temp(connection, source_id, "artist", "local_current_artist_ids")?;

    replace_temp_id_set(
        connection,
        "local_current_album_artist_ids",
        delta.current_album_artist_ids.iter().map(ArtistId::as_str),
    )?;
    pruned.album_artists.deleted = ids_not_in_temp(
        connection,
        "album_artists",
        "artist_id",
        source_id,
        "local_current_album_artist_ids",
    )?
    .into_iter()
    .map(ArtistId::new)
    .collect();
    delete_rows_not_in_temp(
        connection,
        "album_artists",
        "artist_id",
        source_id,
        "local_current_album_artist_ids",
    )?;
    delete_fts_not_in_temp(
        connection,
        source_id,
        "album_artist",
        "local_current_album_artist_ids",
    )?;

    replace_temp_id_set(
        connection,
        "local_current_genre_ids",
        delta.current_genre_ids.iter().map(GenreId::as_str),
    )?;
    pruned.genres.deleted = ids_not_in_temp(
        connection,
        "genres",
        "genre_id",
        source_id,
        "local_current_genre_ids",
    )?
    .into_iter()
    .map(GenreId::new)
    .collect();
    delete_rows_not_in_temp(
        connection,
        "genres",
        "genre_id",
        source_id,
        "local_current_genre_ids",
    )?;
    delete_stale_refs(
        connection,
        source_id,
        COLLECTION_COVER_GENRE,
        "local_current_genre_ids",
    )?;
    Ok(pruned)
}

fn replace_temp_id_set<'a>(
    connection: &Connection,
    table: &str,
    ids: impl Iterator<Item = &'a str>,
) -> StoreResult<()> {
    connection.execute(
        &format!("CREATE TEMP TABLE IF NOT EXISTS {table} (id TEXT PRIMARY KEY)"),
        [],
    )?;
    connection.execute(&format!("DELETE FROM {table}"), [])?;
    let mut insert =
        connection.prepare(&format!("INSERT OR IGNORE INTO {table} (id) VALUES (?1)"))?;
    for id in ids {
        insert.execute(params![id])?;
    }
    Ok(())
}

fn delete_rows_not_in_temp(
    connection: &Connection,
    table: &str,
    id_column: &str,
    source_id: &SourceId,
    temp_table: &str,
) -> StoreResult<()> {
    let sql = format!(
        "
        DELETE FROM {table}
        WHERE source_id = ?1
          AND {id_column} NOT IN (SELECT id FROM {temp_table})
        "
    );
    connection.execute(&sql, params![source_id.as_str()])?;
    Ok(())
}

fn ids_not_in_temp(
    connection: &Connection,
    table: &str,
    id_column: &str,
    source_id: &SourceId,
    temp_table: &str,
) -> StoreResult<Vec<String>> {
    let sql = format!(
        "
        SELECT {id_column}
        FROM {table}
        WHERE source_id = ?1
          AND {id_column} NOT IN (SELECT id FROM {temp_table})
        ORDER BY {id_column}
        "
    );
    let mut statement = connection.prepare(&sql)?;
    collect_rows(statement.query_map(params![source_id.as_str()], |row| row.get(0))?)
}

fn delete_fts_not_in_temp(
    connection: &Connection,
    source_id: &SourceId,
    item_type: &str,
    temp_table: &str,
) -> StoreResult<()> {
    let sql = format!(
        "
        DELETE FROM library_fts
        WHERE source_id = ?1
          AND item_type = ?2
          AND item_id NOT IN (SELECT id FROM {temp_table})
        "
    );
    connection.execute(&sql, params![source_id.as_str(), item_type])?;
    Ok(())
}

fn delete_stale_refs(
    connection: &Connection,
    source_id: &SourceId,
    collection_type: &str,
    temp_table: &str,
) -> StoreResult<()> {
    let sql = format!(
        "
        DELETE FROM collection_cover_refs
        WHERE source_id = ?1
          AND collection_type = ?2
          AND collection_id NOT IN (SELECT id FROM {temp_table})
        "
    );
    connection.execute(&sql, params![source_id.as_str(), collection_type])?;
    Ok(())
}

fn i64_from_u64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn u64_from_i64(value: i64) -> u64 {
    value.max(0) as u64
}
