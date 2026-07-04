use super::sources::album_release_types_json;
use super::*;

const LOCAL_STRESS_TRACK_ID_PREFIX: &str = "local:stress-track:";

pub fn local_file_source_object_id(root_path: &str, relative_path: &str) -> String {
    format!("local:file:{root_path}\u{1f}{relative_path}")
}

pub(super) fn upsert_source_object_on_connection(
    connection: &Connection,
    source_id: &SourceId,
    source: &SourceObject,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO source_objects (
            source_id, source_object_id, entity_kind, entity_id, source_object_kind,
            source_path, parent_source_object_id, cue_path, cue_revision,
            cue_track_index, segment_start_ms, segment_end_ms, metadata_json,
            sync_generation, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, '{}', ?13, CURRENT_TIMESTAMP)
        ON CONFLICT(source_id, source_object_id) DO UPDATE SET
            entity_kind = excluded.entity_kind,
            entity_id = excluded.entity_id,
            source_object_kind = excluded.source_object_kind,
            source_path = excluded.source_path,
            parent_source_object_id = excluded.parent_source_object_id,
            cue_path = excluded.cue_path,
            cue_revision = excluded.cue_revision,
            cue_track_index = excluded.cue_track_index,
            segment_start_ms = excluded.segment_start_ms,
            segment_end_ms = excluded.segment_end_ms,
            metadata_json = excluded.metadata_json,
            sync_generation = excluded.sync_generation,
            updated_at = excluded.updated_at
        ",
        params![
            source_id.as_str(),
            source.source_object_id.as_str(),
            source.entity_kind.as_deref(),
            source.entity_id.as_deref(),
            source.source_object_kind.as_str(),
            source.source_path.as_deref(),
            source.parent_source_object_id.as_deref(),
            source.cue_path.as_deref(),
            source.cue_revision.as_deref(),
            source.cue_track_index,
            source.segment_start_ms,
            source.segment_end_ms,
            source.sync_generation,
        ],
    )?;
    Ok(())
}

pub(super) fn ensure_local_file_source_parent(
    connection: &Connection,
    source_id: &SourceId,
    source_object_id: &str,
) -> StoreResult<()> {
    let exists = connection.query_row(
        "
        SELECT EXISTS(
            SELECT 1
            FROM source_objects
            WHERE source_id = ?1
              AND source_object_id = ?2
              AND source_object_kind = 'local_file'
        )
        ",
        params![source_id.as_str(), source_object_id],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(StoreError::InvalidSourceObject(format!(
            "cue parent source object is not a local file: {source_object_id}"
        )))
    }
}

pub(super) fn source_object_from_row(row: &Row<'_>) -> rusqlite::Result<SourceObject> {
    Ok(SourceObject {
        source_object_id: row.get(0)?,
        entity_kind: row.get(1)?,
        entity_id: row.get(2)?,
        source_object_kind: row.get(3)?,
        source_path: row.get(4)?,
        parent_source_object_id: row.get(5)?,
        cue_path: row.get(6)?,
        cue_revision: row.get(7)?,
        cue_track_index: row.get(8)?,
        segment_start_ms: row.get(9)?,
        segment_end_ms: row.get(10)?,
        sync_generation: row.get(11)?,
    })
}

pub(super) fn delete_track_entity_rows(
    connection: &Connection,
    source_id: &SourceId,
    track_ids: &[TrackId],
) -> StoreResult<()> {
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
        for (table, column) in track_entity_tables() {
            let sql = format!(
                "DELETE FROM {table} WHERE source_id = ? AND entity_kind = 'track' AND {column} IN ({placeholders})"
            );
            connection.execute(&sql, params_from_iter(values.clone()))?;
        }
    }
    Ok(())
}

pub(super) fn delete_track_entity_rows_not_in_temp(
    connection: &Connection,
    source_id: &SourceId,
    temp_table: &str,
) -> StoreResult<()> {
    for (table, column) in track_entity_tables() {
        let sql = format!(
            "
            DELETE FROM {table}
            WHERE source_id = ?1
              AND entity_kind = 'track'
              AND {column} NOT IN (SELECT id FROM {temp_table})
            "
        );
        connection.execute(&sql, params![source_id.as_str()])?;
    }
    Ok(())
}

fn track_entity_tables() -> [(&'static str, &'static str); 7] {
    [
        ("entity_content_refs", "entity_id"),
        ("entity_facts", "entity_id"),
        ("entity_grouping_keys", "entity_id"),
        ("entity_identity_keys", "entity_id"),
        ("entity_links", "entity_id"),
        ("entities", "entity_id"),
        ("source_objects", "entity_id"),
    ]
}

pub(super) fn upsert_album_entity_data_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    album: &Album,
) -> StoreResult<()> {
    let existing_release_id = load_entity_identity_value_on_connection(
        connection,
        source_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release",
    )?;
    let existing_group_id = load_entity_grouping_value_on_connection(
        connection,
        source_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release_group",
    )?;
    let release_id = clean_identity_value(album.musicbrainz_album_id.as_deref());
    let group_id = clean_identity_value(album.musicbrainz_release_group_id.as_deref());
    let identity_changed =
        existing_release_id.as_deref() != release_id || existing_group_id.as_deref() != group_id;
    upsert_entity_on_connection(
        connection,
        source_id,
        "album",
        album.id.as_str(),
        "source",
        None,
    )?;
    upsert_identity_key_on_connection(
        connection,
        source_id,
        "album",
        "source:album_id",
        album.id.as_str(),
        album.id.as_str(),
        "source",
    )?;
    delete_identity_key_on_connection(
        connection,
        source_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release",
    )?;
    delete_grouping_key_on_connection(
        connection,
        source_id,
        "album",
        album.id.as_str(),
        "musicbrainz:release_group",
    )?;
    if identity_changed {
        delete_resolved_album_metadata_facts_on_connection(
            connection,
            source_id,
            album.id.as_str(),
        )?;
    }
    if let Some(release_id) = release_id {
        upsert_identity_key_on_connection(
            connection,
            source_id,
            "album",
            "musicbrainz:release",
            release_id,
            album.id.as_str(),
            "source",
        )?;
    }
    if let Some(group_id) = group_id {
        upsert_grouping_key_on_connection(
            connection,
            source_id,
            "album",
            "musicbrainz:release_group",
            group_id,
            album.id.as_str(),
            "source",
        )?;
    }
    let release_types_json = album_release_types_json(&album.release_types)?;
    if release_types_json != "[]" {
        upsert_fact_on_connection(
            connection,
            source_id,
            "album",
            album.id.as_str(),
            "release_types",
            &release_types_json,
            "source",
        )?;
    }
    if let Some(is_compilation) = album.is_compilation {
        upsert_fact_on_connection(
            connection,
            source_id,
            "album",
            album.id.as_str(),
            "is_compilation",
            if is_compilation { "true" } else { "false" },
            "source",
        )?;
    }
    if let Some(content_key) = image_ref_content_key(album.image_ref.as_ref()) {
        upsert_content_ref_on_connection(
            connection,
            source_id,
            "album",
            album.id.as_str(),
            "cover",
            &content_key,
            "source",
        )?;
    }
    Ok(())
}

fn load_entity_identity_value_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<Option<String>> {
    connection
        .query_row(
            "
            SELECT value
            FROM entity_identity_keys
            WHERE source_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND namespace = ?4
            LIMIT 1
            ",
            params![source_id.as_str(), entity_kind, entity_id, namespace],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_entity_grouping_value_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<Option<String>> {
    connection
        .query_row(
            "
            SELECT value
            FROM entity_grouping_keys
            WHERE source_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND namespace = ?4
            LIMIT 1
            ",
            params![source_id.as_str(), entity_kind, entity_id, namespace],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::from)
}

fn delete_identity_key_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM entity_identity_keys
        WHERE source_id = ?1
          AND entity_kind = ?2
          AND entity_id = ?3
          AND namespace = ?4
        ",
        params![source_id.as_str(), entity_kind, entity_id, namespace],
    )?;
    Ok(())
}

fn delete_grouping_key_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM entity_grouping_keys
        WHERE source_id = ?1
          AND entity_kind = ?2
          AND entity_id = ?3
          AND namespace = ?4
        ",
        params![source_id.as_str(), entity_kind, entity_id, namespace],
    )?;
    Ok(())
}

fn delete_resolved_album_metadata_facts_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    album_id: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM entity_facts
        WHERE source_id = ?1
          AND entity_kind = 'album'
          AND entity_id = ?2
          AND source = 'musicbrainz'
          AND fact_key IN ('release_types', 'is_compilation')
        ",
        params![source_id.as_str(), album_id],
    )?;
    Ok(())
}

pub(super) fn upsert_track_entity_data_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    track: &Track,
) -> StoreResult<()> {
    let source = if track.local_path.is_some() {
        "local"
    } else {
        "source"
    };
    upsert_entity_on_connection(
        connection,
        source_id,
        "track",
        track.id.as_str(),
        source,
        None,
    )?;
    upsert_identity_key_on_connection(
        connection,
        source_id,
        "track",
        "source:track_id",
        track.id.as_str(),
        track.id.as_str(),
        source,
    )?;
    delete_identity_key_on_connection(
        connection,
        source_id,
        "track",
        track.id.as_str(),
        "local:path",
    )?;
    delete_identity_key_on_connection(
        connection,
        source_id,
        "track",
        track.id.as_str(),
        "musicbrainz:release_track",
    )?;
    delete_grouping_key_on_connection(
        connection,
        source_id,
        "track",
        track.id.as_str(),
        "musicbrainz:recording",
    )?;
    let stress_track =
        cfg!(debug_assertions) && track.id.as_str().starts_with(LOCAL_STRESS_TRACK_ID_PREFIX);
    if !stress_track && let Some(path) = clean_identity_value(track.local_path.as_deref()) {
        upsert_identity_key_on_connection(
            connection,
            source_id,
            "track",
            "local:path",
            path,
            track.id.as_str(),
            "local",
        )?;
    }
    if let Some(recording_id) = clean_identity_value(track.musicbrainz_recording_id.as_deref()) {
        upsert_grouping_key_on_connection(
            connection,
            source_id,
            "track",
            "musicbrainz:recording",
            recording_id,
            track.id.as_str(),
            source,
        )?;
    }
    if let Some(track_id) = clean_identity_value(track.musicbrainz_release_track_id.as_deref()) {
        upsert_identity_key_on_connection(
            connection,
            source_id,
            "track",
            "musicbrainz:release_track",
            track_id,
            track.id.as_str(),
            source,
        )?;
    }
    if let Some(content_key) = image_ref_content_key(track.image_ref.as_ref()) {
        upsert_content_ref_on_connection(
            connection,
            source_id,
            "track",
            track.id.as_str(),
            "cover",
            &content_key,
            source,
        )?;
    }
    Ok(())
}

pub(super) fn upsert_artist_credit_entity_data_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    artist: &ArtistCredit,
) -> StoreResult<()> {
    upsert_artist_entity_keys_on_connection(
        connection,
        source_id,
        entity_kind,
        artist.id.as_str(),
        artist.musicbrainz_artist_id.as_deref(),
        false,
    )
}

pub(super) fn upsert_artist_entity_data_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    artist: &Artist,
) -> StoreResult<()> {
    upsert_artist_entity_keys_on_connection(
        connection,
        source_id,
        entity_kind,
        artist.id.as_str(),
        artist.musicbrainz_artist_id.as_deref(),
        false,
    )
}

fn upsert_artist_entity_keys_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    artist_id: &str,
    musicbrainz_artist_id: Option<&str>,
    replace_musicbrainz_artist_id: bool,
) -> StoreResult<()> {
    upsert_entity_on_connection(
        connection,
        source_id,
        entity_kind,
        artist_id,
        "source",
        None,
    )?;
    upsert_identity_key_on_connection(
        connection,
        source_id,
        entity_kind,
        "source:artist_id",
        artist_id,
        artist_id,
        "source",
    )?;
    let artist_id_value = clean_identity_value(musicbrainz_artist_id);
    if replace_musicbrainz_artist_id || artist_id_value.is_some() {
        delete_identity_key_on_connection(
            connection,
            source_id,
            entity_kind,
            artist_id,
            "musicbrainz:artist",
        )?;
    }
    if let Some(artist_id_value) = artist_id_value {
        upsert_identity_key_on_connection(
            connection,
            source_id,
            entity_kind,
            "musicbrainz:artist",
            artist_id_value,
            artist_id,
            "source",
        )?;
    }
    Ok(())
}

fn upsert_entity_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    source: &str,
    source_object_id: Option<&str>,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO entities (
            source_id, entity_kind, entity_id, source, source_object_id, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
        ON CONFLICT(source_id, entity_kind, entity_id) DO UPDATE SET
            source = excluded.source,
            source_object_id = COALESCE(excluded.source_object_id, entities.source_object_id),
            updated_at = excluded.updated_at
        ",
        params![
            source_id.as_str(),
            entity_kind,
            entity_id,
            source,
            source_object_id,
        ],
    )?;
    Ok(())
}

fn upsert_identity_key_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    namespace: &str,
    value: &str,
    entity_id: &str,
    source: &str,
) -> StoreResult<()> {
    let Some(value) = clean_identity_value(Some(value)) else {
        return Ok(());
    };
    connection.execute(
        "
        INSERT INTO entity_identity_keys (
            source_id, entity_kind, namespace, value, entity_id, source, strength, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 100, CURRENT_TIMESTAMP)
        ON CONFLICT(source_id, entity_kind, namespace, value) DO UPDATE SET
            entity_id = excluded.entity_id,
            source = excluded.source,
            strength = excluded.strength,
            updated_at = excluded.updated_at
        ",
        params![
            source_id.as_str(),
            entity_kind,
            namespace,
            value,
            entity_id,
            source,
        ],
    )?;
    Ok(())
}

fn upsert_grouping_key_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    namespace: &str,
    value: &str,
    entity_id: &str,
    source: &str,
) -> StoreResult<()> {
    let Some(value) = clean_identity_value(Some(value)) else {
        return Ok(());
    };
    connection.execute(
        "
        INSERT INTO entity_grouping_keys (
            source_id, entity_kind, namespace, value, entity_id, source, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
        ON CONFLICT(source_id, entity_kind, namespace, value, entity_id) DO UPDATE SET
            source = excluded.source,
            updated_at = excluded.updated_at
        ",
        params![
            source_id.as_str(),
            entity_kind,
            namespace,
            value,
            entity_id,
            source,
        ],
    )?;
    Ok(())
}

fn upsert_fact_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    fact_key: &str,
    value_json: &str,
    source: &str,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO entity_facts (
            source_id, entity_kind, entity_id, fact_key, value_json, source, status, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'resolved', CURRENT_TIMESTAMP)
        ON CONFLICT(source_id, entity_kind, entity_id, fact_key, source) DO UPDATE SET
            value_json = excluded.value_json,
            status = excluded.status,
            updated_at = excluded.updated_at
        ",
        params![
            source_id.as_str(),
            entity_kind,
            entity_id,
            fact_key,
            value_json,
            source,
        ],
    )?;
    Ok(())
}

fn upsert_content_ref_on_connection(
    connection: &rusqlite::Connection,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    content_kind: &str,
    content_key: &str,
    source: &str,
) -> StoreResult<()> {
    let Some(content_key) = clean_identity_value(Some(content_key)) else {
        return Ok(());
    };
    connection.execute(
        "
        INSERT INTO entity_content_refs (
            source_id, entity_kind, entity_id, content_kind, content_key, source, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
        ON CONFLICT(source_id, entity_kind, entity_id, content_kind, source) DO UPDATE SET
            content_key = excluded.content_key,
            updated_at = excluded.updated_at
        ",
        params![
            source_id.as_str(),
            entity_kind,
            entity_id,
            content_kind,
            content_key,
            source,
        ],
    )?;
    Ok(())
}

fn image_ref_content_key(image_ref: Option<&ImageRef>) -> Option<String> {
    let image_ref = image_ref?;
    let item_id = clean_identity_value(Some(image_ref.item_id.as_str()))?;
    let tag = image_ref.tag.as_deref().unwrap_or("");
    Some(format!("{item_id}\u{1f}{tag}"))
}

fn clean_identity_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
