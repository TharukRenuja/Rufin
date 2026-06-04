use super::servers::{
    COLLECTION_COVER_GENRE, bool_to_i64, collect_rows, image_ref_parts, u32_from_i64,
};
use super::*;

#[derive(Clone, Debug, Default)]
pub struct LocalLibraryDelta {
    pub changed_tracks: Vec<Track>,
    pub metadata_tracks: Vec<Track>,
    pub artwork_tracks: Vec<Track>,
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
    pub manifest_entries: Vec<LocalManifestEntry>,
}

impl Store {
    pub fn load_local_manifest(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<Vec<LocalManifestEntry>> {
        let mut statement = self.connection.prepare(
            "
            SELECT f.path, f.root_path, f.relative_path, f.file_size,
                   f.mtime_seconds, f.mtime_nanos, f.inode, f.device,
                   d.track_json, d.album_artist, d.cover_kind, d.cover_path,
                   d.cover_embedded_index, d.cover_revision, f.metadata_hash, f.search_hash,
                   a.cover_item_id
            FROM local_file_manifest f
            JOIN local_track_manifest_data d
              ON d.server_id = f.server_id
             AND d.track_id = f.track_id
             AND d.manifest_version = f.manifest_version
            LEFT JOIN local_artwork_manifest a
              ON a.server_id = f.server_id
             AND a.revision = d.cover_revision
             AND a.source_path = d.cover_path
             AND a.source_kind = d.cover_kind
             AND a.manifest_version = f.manifest_version
            WHERE f.server_id = ?1
              AND f.manifest_version = ?2
            ORDER BY f.path
            ",
        )?;
        let rows = collect_rows(statement.query_map(
            params![server_id.as_str(), LOCAL_MANIFEST_VERSION],
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
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, Option<String>>(16)?,
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
                        cover,
                        metadata_hash,
                        search_hash,
                    })
                },
            )
            .collect())
    }

    pub fn replace_local_manifest(
        &self,
        server_id: &ServerId,
        generation: i64,
        entries: &[LocalManifestEntry],
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            clear_local_manifest_on_connection(connection, server_id)?;
            let mut insert_file = connection.prepare(
                "
                INSERT INTO local_file_manifest (
                    server_id, manifest_version, path, root_path, relative_path,
                    file_size, mtime_seconds, mtime_nanos, inode, device, content_hash,
                    track_id, album_id, source_format, metadata_hash, search_hash,
                    artwork_revision, scan_generation, last_tag_read_at, last_seen_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL,
                        ?11, ?12, ?13, ?14, ?15, ?16, ?17, NULL, CURRENT_TIMESTAMP)
                ",
            )?;
            let mut insert_track = connection.prepare(
                "
                INSERT INTO local_track_manifest_data (
                    server_id, manifest_version, track_id, track_json, album_artist,
                    cover_kind, cover_path, cover_embedded_index, cover_revision,
                    metadata_hash, search_hash, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, CURRENT_TIMESTAMP)
                ",
            )?;
            let mut insert_artwork = connection.prepare(
                "
                INSERT INTO local_artwork_manifest (
                    server_id, cover_item_id, manifest_version, source_kind, source_path,
                    source_size, mtime_seconds, mtime_nanos, content_hash, revision,
                    scan_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10)
                ON CONFLICT(server_id, cover_item_id) DO UPDATE SET
                    manifest_version = excluded.manifest_version,
                    source_kind = excluded.source_kind,
                    source_path = excluded.source_path,
                    source_size = excluded.source_size,
                    mtime_seconds = excluded.mtime_seconds,
                    mtime_nanos = excluded.mtime_nanos,
                    content_hash = excluded.content_hash,
                    revision = excluded.revision,
                    scan_generation = excluded.scan_generation
                ",
            )?;
            for entry in entries {
                let track_json = serde_json::to_string(&entry.track)?;
                let (cover_kind, cover_path, embedded_index, cover_revision) =
                    local_manifest_cover_parts(entry.cover.as_ref());
                insert_file.execute(params![
                    server_id.as_str(),
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
                insert_track.execute(params![
                    server_id.as_str(),
                    LOCAL_MANIFEST_VERSION,
                    entry.track.id.as_str(),
                    track_json,
                    entry.album_artist.as_str(),
                    cover_kind,
                    cover_path,
                    embedded_index,
                    cover_revision,
                    entry.metadata_hash.as_str(),
                    entry.search_hash.as_str(),
                ])?;
                if let Some(cover) = &entry.cover {
                    let facts = local_artwork_source_facts(&cover.source_path);
                    insert_artwork.execute(params![
                        server_id.as_str(),
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
        })
    }

    pub fn delete_local_track_rows(
        &self,
        server_id: &ServerId,
        track_ids: &[TrackId],
    ) -> StoreResult<()> {
        if track_ids.is_empty() {
            return Ok(());
        }
        self.write_batch(|connection| {
            for table in [
                "track_music_folders",
                "track_genres",
                "track_artist_links",
                "tracks",
            ] {
                delete_track_ids(connection, table, server_id, track_ids)?;
            }
            delete_track_fts_rows(connection, server_id, track_ids)?;
            Ok(())
        })
    }

    pub fn update_local_track_image_refs(
        &self,
        server_id: &ServerId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        self.write_batch(|connection| {
            let mut update_track = connection.prepare(
                "
                UPDATE tracks
                SET image_item_id = ?3,
                    image_tag = ?4,
                    sync_generation = ?5
                WHERE server_id = ?1 AND track_id = ?2
                ",
            )?;
            for track in tracks {
                let (image_item_id, image_tag) = image_ref_parts(track.image_ref.as_ref());
                update_track.execute(params![
                    server_id.as_str(),
                    track.id.as_str(),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
            }
            Ok(())
        })
    }

    pub fn update_local_track_metadata_rows(
        &self,
        server_id: &ServerId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        self.write_batch(|connection| {
            let mut missing_tracks = Vec::new();
            {
                let mut update_track = connection.prepare(
                    "
                    UPDATE tracks
                    SET album_id = ?3,
                        title = ?4,
                        artist = ?5,
                        artist_id = ?6,
                        album = ?7,
                        year = ?8,
                        release_date = ?9,
                        date_added = ?10,
                        last_played = ?11,
                        play_count = ?12,
                        user_rating = ?13,
                        duration_seconds = ?14,
                        favorite = ?15,
                        disc_number = ?16,
                        track_number = ?17,
                        image_item_id = ?18,
                        image_tag = ?19,
                        local_path = ?20,
                        source_format = ?21,
                        comment = ?22,
                        skip_count = ?23,
                        sync_generation = ?24
                    WHERE server_id = ?1 AND track_id = ?2
                    ",
                )?;
                for track in tracks {
                    let (image_item_id, image_tag) = image_ref_parts(track.image_ref.as_ref());
                    let updated = update_track.execute(params![
                        server_id.as_str(),
                        track.id.as_str(),
                        track.album_id.as_str(),
                        track.title,
                        track.artist,
                        track.artist_id.as_ref().map(ArtistId::as_str),
                        track.album,
                        i64::from(track.year),
                        track.release_date.as_deref(),
                        track.date_added.as_deref(),
                        track.last_played.as_deref(),
                        track.play_count.map(i64::from),
                        track.user_rating.map(i64::from),
                        i64::from(track.duration_seconds),
                        bool_to_i64(track.favorite),
                        i64::from(track.disc_number),
                        i64::from(track.track_number),
                        image_item_id,
                        image_tag,
                        track.local_path.as_deref(),
                        track.source_format.as_deref(),
                        track.comment.as_deref(),
                        track.skip_count.map(i64::from),
                        generation,
                    ])?;
                    if updated == 0 {
                        missing_tracks.push(track.clone());
                    }
                }
            }
            self.upsert_tracks(server_id, &missing_tracks, generation)?;
            Ok(())
        })
    }

    pub fn complete_unchanged_local_sync(
        &self,
        server_id: &ServerId,
        generation: i64,
    ) -> StoreResult<Vec<CoverCacheEntry>> {
        self.write_batch(|_| {
            let previous_generation = generation.saturating_sub(1);
            self.connection.execute(
                "
                UPDATE sync_state
                SET status = 'idle',
                    generation = ?2,
                    last_completed_at = CURRENT_TIMESTAMP,
                    last_error = NULL
                WHERE server_id = ?1
                ",
                params![server_id.as_str(), previous_generation],
            )?;
            self.prune_stale_image_cache_entries(server_id)
        })
    }

    pub fn commit_local_library_delta(
        &self,
        server_id: &ServerId,
        generation: i64,
        delta: LocalLibraryDelta,
    ) -> StoreResult<Vec<CoverCacheEntry>> {
        self.write_batch(|connection| {
            self.upsert_tracks(server_id, &delta.changed_tracks, generation)?;
            self.update_local_track_metadata_rows(server_id, &delta.metadata_tracks, generation)?;
            self.update_local_track_image_refs(server_id, &delta.artwork_tracks, generation)?;
            self.delete_local_track_rows(server_id, &delta.deleted_track_ids)?;
            self.upsert_albums(server_id, &delta.dirty_albums, generation)?;
            self.upsert_artists(server_id, &delta.dirty_artists, false, generation)?;
            self.upsert_artists(server_id, &delta.dirty_album_artists, true, generation)?;
            self.upsert_genres(server_id, &delta.dirty_genres, generation)?;
            self.upsert_home_sections(server_id, &delta.home_sections, generation)?;
            prune_stale_local_aggregate_rows(connection, server_id, &delta)?;
            let pruned_cover_entries = self.complete_local_sync(server_id, generation)?;
            self.replace_local_manifest(server_id, generation, &delta.manifest_entries)?;
            Ok(pruned_cover_entries)
        })
    }

    fn complete_local_sync(
        &self,
        server_id: &ServerId,
        generation: i64,
    ) -> StoreResult<Vec<CoverCacheEntry>> {
        self.refresh_collection_cover_refs(server_id)?;
        self.refresh_smart_playlist_cover_refs(server_id)?;
        let pruned_cover_entries = self.prune_stale_image_cache_entries(server_id)?;
        self.connection.execute(
            "
            UPDATE sync_state
            SET status = 'idle',
                generation = ?2,
                last_completed_at = CURRENT_TIMESTAMP,
                last_error = NULL
            WHERE server_id = ?1
            ",
            params![server_id.as_str(), generation],
        )?;
        Ok(pruned_cover_entries)
    }
}

pub(super) fn clear_local_manifest_on_connection(
    connection: &Connection,
    server_id: &ServerId,
) -> StoreResult<()> {
    for table in [
        "local_artwork_manifest",
        "local_track_manifest_data",
        "local_file_manifest",
    ] {
        let sql = format!("DELETE FROM {table} WHERE server_id = ?1");
        connection.execute(&sql, params![server_id.as_str()])?;
    }
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
    server_id: &ServerId,
    track_ids: &[TrackId],
) -> StoreResult<()> {
    for chunk in track_ids.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("DELETE FROM {table} WHERE server_id = ? AND track_id IN ({placeholders})");
        let mut values = vec![Value::Text(server_id.as_str().to_string())];
        values.extend(
            chunk
                .iter()
                .map(|track_id| Value::Text(track_id.as_str().to_string())),
        );
        connection.execute(&sql, params_from_iter(values))?;
    }
    Ok(())
}

fn delete_track_fts_rows(
    connection: &Connection,
    server_id: &ServerId,
    track_ids: &[TrackId],
) -> StoreResult<()> {
    for chunk in track_ids.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "DELETE FROM library_fts WHERE server_id = ? AND item_type = 'track' AND item_id IN ({placeholders})"
        );
        let mut values = vec![Value::Text(server_id.as_str().to_string())];
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
    server_id: &ServerId,
    delta: &LocalLibraryDelta,
) -> StoreResult<()> {
    replace_temp_id_set(
        connection,
        "local_current_album_ids",
        delta.current_album_ids.iter().map(AlbumId::as_str),
    )?;
    delete_rows_not_in_temp(
        connection,
        "album_genres",
        "album_id",
        server_id,
        "local_current_album_ids",
    )?;
    delete_rows_not_in_temp(
        connection,
        "album_artist_links",
        "album_id",
        server_id,
        "local_current_album_ids",
    )?;
    delete_rows_not_in_temp(
        connection,
        "albums",
        "album_id",
        server_id,
        "local_current_album_ids",
    )?;
    delete_fts_not_in_temp(connection, server_id, "album", "local_current_album_ids")?;

    replace_temp_id_set(
        connection,
        "local_current_artist_ids",
        delta.current_artist_ids.iter().map(ArtistId::as_str),
    )?;
    delete_rows_not_in_temp(
        connection,
        "artists",
        "artist_id",
        server_id,
        "local_current_artist_ids",
    )?;
    delete_fts_not_in_temp(connection, server_id, "artist", "local_current_artist_ids")?;

    replace_temp_id_set(
        connection,
        "local_current_album_artist_ids",
        delta.current_album_artist_ids.iter().map(ArtistId::as_str),
    )?;
    delete_rows_not_in_temp(
        connection,
        "album_artists",
        "artist_id",
        server_id,
        "local_current_album_artist_ids",
    )?;
    delete_fts_not_in_temp(
        connection,
        server_id,
        "album_artist",
        "local_current_album_artist_ids",
    )?;

    replace_temp_id_set(
        connection,
        "local_current_genre_ids",
        delta.current_genre_ids.iter().map(GenreId::as_str),
    )?;
    delete_rows_not_in_temp(
        connection,
        "genres",
        "genre_id",
        server_id,
        "local_current_genre_ids",
    )?;
    delete_stale_refs(
        connection,
        server_id,
        COLLECTION_COVER_GENRE,
        "local_current_genre_ids",
    )?;
    Ok(())
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
    server_id: &ServerId,
    temp_table: &str,
) -> StoreResult<()> {
    let sql = format!(
        "
        DELETE FROM {table}
        WHERE server_id = ?1
          AND {id_column} NOT IN (SELECT id FROM {temp_table})
        "
    );
    connection.execute(&sql, params![server_id.as_str()])?;
    Ok(())
}

fn delete_fts_not_in_temp(
    connection: &Connection,
    server_id: &ServerId,
    item_type: &str,
    temp_table: &str,
) -> StoreResult<()> {
    let sql = format!(
        "
        DELETE FROM library_fts
        WHERE server_id = ?1
          AND item_type = ?2
          AND item_id NOT IN (SELECT id FROM {temp_table})
        "
    );
    connection.execute(&sql, params![server_id.as_str(), item_type])?;
    Ok(())
}

fn delete_stale_refs(
    connection: &Connection,
    server_id: &ServerId,
    collection_type: &str,
    temp_table: &str,
) -> StoreResult<()> {
    let sql = format!(
        "
        DELETE FROM collection_cover_refs
        WHERE server_id = ?1
          AND collection_type = ?2
          AND collection_id NOT IN (SELECT id FROM {temp_table})
        "
    );
    connection.execute(&sql, params![server_id.as_str(), collection_type])?;
    Ok(())
}

fn i64_from_u64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn u64_from_i64(value: i64) -> u64 {
    value.max(0) as u64
}
