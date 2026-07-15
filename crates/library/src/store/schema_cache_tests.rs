use std::{collections::BTreeSet, fs, path::PathBuf};

use super::test_support::*;
use crate::compare_tracks;
use crate::{
    ActivityOutcome, LEGACY_ACTIVITY_PERIOD, LocalLibraryDelta, LocalManifestDelta, PagedResponse,
    PlaybackCheckpointRecord, PlaylistWriteMode, SourceEntityKind, SourceObjectMapping, StoreError,
    TrackActivitySummary, local_file_source_object_id,
};
use crate::{
    AlbumId, ArtistCredit, ArtistId, HomeSection, HomeSectionKind, LocalCueTrackSource,
    LocalFileFacts, LocalManifestCover, LocalManifestCoverKind, LocalManifestEntry,
    SmartPlaylistBuiltin, SourceFeatureOwner, SourceId, TrackId,
};

const OLD_SOURCE_ID_COLUMN_TABLES: &[&str] = &[
    "queue_snapshots",
    "sources",
    "source_local_access",
    "source_music_folders",
    "track_music_folders",
    "track_local_matches",
    "track_activity",
    "source_library_preferences",
    "active_source",
    "sync_state",
    "albums",
    "tracks",
    "artists",
    "album_artists",
    "genres",
    "playlists",
    "smart_playlists",
    "smart_playlist_seed_state",
    "album_genres",
    "track_genres",
    "track_moods",
    "album_artist_links",
    "track_artist_links",
    "playlist_tracks",
    "item_favorite_overrides",
    "home_section_items",
    "home_section_prefetch_items",
    "lyrics_cache",
    "cover_cache",
    "external_image_lookup_misses",
    "collection_cover_refs",
    "local_file_manifest",
    "local_track_manifest_data",
    "local_artwork_manifest",
    "source_objects",
    "entities",
    "entity_identity_keys",
    "entity_grouping_keys",
    "entity_facts",
    "entity_resolver_state",
    "entity_content_refs",
    "entity_links",
];

fn simulate_pre_artwork_owner_schema(connection: &rusqlite::Connection) {
    for table in [
        "albums",
        "tracks",
        "artists",
        "album_artists",
        "genres",
        "playlists",
    ] {
        if !table_has_column(connection, table, "image_origin") {
            connection
                .execute(
                    &format!(
                        "ALTER TABLE {table} ADD COLUMN image_origin TEXT NOT NULL DEFAULT 'unknown'"
                    ),
                    [],
                )
                .expect("restore image origin column");
        }
    }
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS cover_cache (
                source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
                item_id TEXT NOT NULL,
                image_tag TEXT NOT NULL,
                size INTEGER NOT NULL,
                path TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (source_id, item_id, image_tag, size)
            );
            CREATE TABLE IF NOT EXISTS external_image_lookup_misses (
                source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
                item_id TEXT NOT NULL,
                image_tag TEXT NOT NULL,
                size INTEGER NOT NULL,
                reason TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (source_id, item_id, image_tag, size)
            );
            CREATE TABLE IF NOT EXISTS collection_cover_refs (
                source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
                collection_type TEXT NOT NULL,
                collection_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                image_item_id TEXT NOT NULL,
                image_tag TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (source_id, collection_type, collection_id, position)
            );
            CREATE TABLE IF NOT EXISTS entity_content_refs (
                source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                content_kind TEXT NOT NULL,
                content_key TEXT NOT NULL,
                source TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (source_id, entity_kind, entity_id, content_kind, source)
            );
            CREATE TABLE IF NOT EXISTS content_cache_entries (
                cache_scope TEXT NOT NULL,
                content_kind TEXT NOT NULL,
                content_key TEXT NOT NULL,
                variant TEXT NOT NULL,
                status TEXT NOT NULL,
                path_or_value TEXT,
                source TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (cache_scope, content_kind, content_key, variant)
            );
            ",
        )
        .expect("restore pre-artwork-owner schema");
}

fn simulate_pre_playback_owner_schema(connection: &rusqlite::Connection) {
    simulate_pre_artwork_owner_schema(connection);
    connection
        .execute_batch(
            "
            CREATE TABLE queue_snapshots (
                source_id TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO queue_snapshots (source_id, value, updated_at)
            SELECT source_id, payload, updated_at
            FROM playback_checkpoints;

            CREATE TABLE track_activity (
                source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                play_count INTEGER NOT NULL DEFAULT 0,
                last_played TEXT,
                skip_count INTEGER NOT NULL DEFAULT 0,
                play_recorded_session TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (source_id, track_id)
            );
            INSERT INTO track_activity (
                source_id, track_id, play_count, last_played, skip_count,
                play_recorded_session, updated_at
            )
            SELECT source_id, track_id, SUM(qualified_plays), MAX(last_played_at),
                   SUM(skips), NULL, MAX(updated_at)
            FROM track_activity_period
            GROUP BY source_id, track_id;

            DROP TABLE playback_checkpoints;
            DROP TABLE track_activity_period;
            ",
        )
        .expect("simulate pre-playback-owner schema");
}

fn simulate_pre_provider_payload_schema(connection: &rusqlite::Connection) {
    simulate_pre_playback_owner_schema(connection);
    connection
        .execute_batch(
            "
            ALTER TABLE sources ADD COLUMN base_url TEXT NOT NULL DEFAULT '';
            ALTER TABLE sources ADD COLUMN user_id TEXT NOT NULL DEFAULT '';
            ALTER TABLE sources ADD COLUMN username TEXT NOT NULL DEFAULT '';
            ALTER TABLE sources ADD COLUMN trust_invalid_cert INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE sources ADD COLUMN use_jellyfin_instant_mix INTEGER NOT NULL DEFAULT 0;
            UPDATE sources
            SET base_url = COALESCE(json_extract(provider_payload, '$.base_url'), ''),
                user_id = COALESCE(json_extract(provider_payload, '$.user_id'), ''),
                username = COALESCE(json_extract(provider_payload, '$.username'), ''),
                trust_invalid_cert = CASE
                    WHEN json_extract(provider_payload, '$.trust_invalid_cert') = 1 THEN 1
                    ELSE 0
                END,
                use_jellyfin_instant_mix = CASE
                    WHEN json_extract(provider_payload, '$.use_jellyfin_instant_mix') = 1 THEN 1
                    ELSE 0
                END;
            ALTER TABLE sources DROP COLUMN provider_payload;
            ",
        )
        .expect("simulate pre-provider-payload schema");
}

fn simulate_pre_source_identity_schema(connection: &rusqlite::Connection) {
    simulate_pre_provider_payload_schema(connection);
    connection
        .execute_batch(
            "
            PRAGMA foreign_keys = OFF;
            ALTER TABLE sources RENAME TO servers;
            ALTER TABLE servers RENAME COLUMN kind TO provider;
            ALTER TABLE source_local_access RENAME TO server_local_access;
            ALTER TABLE source_music_folders RENAME TO server_music_folders;
            ALTER TABLE source_library_preferences RENAME TO server_library_preferences;
            ALTER TABLE active_source RENAME TO active_server;
            ALTER TABLE source_objects RENAME COLUMN source_object_kind TO source_kind;
            UPDATE entities SET source = 'provider' WHERE source = 'source';
            UPDATE entity_identity_keys SET source = 'provider' WHERE source = 'source';
            UPDATE entity_grouping_keys SET source = 'provider' WHERE source = 'source';
            UPDATE entity_facts SET source = 'provider' WHERE source = 'source';
            UPDATE entity_content_refs SET source = 'provider' WHERE source = 'source';
            UPDATE content_cache_entries SET source = 'provider' WHERE source = 'source';
            ",
        )
        .expect("simulate pre-source-identity schema");
    for table in OLD_SOURCE_ID_COLUMN_TABLES {
        rename_column_if_exists(
            connection,
            table_before_source_identity(table),
            "source_id",
            "server_id",
        );
    }
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .expect("finish pre-source-identity schema simulation");
}

fn table_before_source_identity(table: &str) -> &str {
    match table {
        "sources" => "servers",
        "source_local_access" => "server_local_access",
        "source_music_folders" => "server_music_folders",
        "source_library_preferences" => "server_library_preferences",
        "active_source" => "active_server",
        _ => table,
    }
}

fn rename_column_if_exists(connection: &rusqlite::Connection, table: &str, from: &str, to: &str) {
    if table_has_column(connection, table, from) && !table_has_column(connection, table, to) {
        connection
            .execute(
                &format!("ALTER TABLE {table} RENAME COLUMN {from} TO {to}"),
                [],
            )
            .expect("rename schema column");
    }
}

fn table_has_column(connection: &rusqlite::Connection, table: &str, column: &str) -> bool {
    let Ok(mut statement) = connection.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query table info");
    columns
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect columns")
        .iter()
        .any(|name| name == column)
}

#[test]
fn current_schema_initializes_empty_database() {
    let store = Store::open_memory().expect("open store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert!(
        store
            .table_has_column("sources", "provider_payload")
            .expect("column lookup"),
        "sources.provider_payload should exist"
    );
    for column in [
        "base_url",
        "user_id",
        "username",
        "trust_invalid_cert",
        "use_jellyfin_instant_mix",
    ] {
        assert!(
            !store
                .table_has_column("sources", column)
                .expect("column lookup"),
            "sources.{column} should not exist"
        );
    }
    for column in ["cache_revision", "last_all_completed_at"] {
        assert!(
            store
                .table_has_column("sync_state", column)
                .expect("column lookup"),
            "sync_state.{column} should exist"
        );
    }
    for column in [
        "release_types_json",
        "is_compilation",
        "musicbrainz_album_id",
        "musicbrainz_release_group_id",
    ] {
        assert!(
            store
                .table_has_column("albums", column)
                .expect("column lookup"),
            "albums.{column} should exist"
        );
    }
    assert!(
        store
            .table_has_column("genres", "duration_seconds")
            .expect("column lookup"),
        "genres.duration_seconds should exist"
    );
    assert!(
        store
            .table_has_column("tracks", "bpm")
            .expect("column lookup"),
        "tracks.bpm should exist"
    );
    assert!(
        store.table_exists("track_moods").expect("table lookup"),
        "track_moods should exist"
    );
    assert!(
        store
            .table_exists("item_favorite_overrides")
            .expect("table lookup"),
        "item_favorite_overrides should exist"
    );
    for table in [
        "source_objects",
        "entities",
        "entity_identity_keys",
        "entity_grouping_keys",
        "entity_facts",
        "entity_resolver_state",
        "entity_links",
    ] {
        assert!(
            store.table_exists(table).expect("table lookup"),
            "{table} should exist"
        );
    }
    for column in [
        "parent_source_object_id",
        "cue_path",
        "cue_revision",
        "cue_track_index",
        "segment_start_ms",
        "segment_end_ms",
    ] {
        assert!(
            store
                .table_has_column("source_objects", column)
                .expect("column lookup"),
            "source_objects.{column} should exist"
        );
    }
    assert!(
        !store
            .table_exists("album_release_type_lookup_misses")
            .expect("table lookup"),
        "release metadata misses should use entity_resolver_state"
    );
    assert!(
        !store.table_exists("album_identity").expect("table lookup"),
        "album identity should use shared entity tables"
    );
    assert!(
        store
            .table_has_column("playlists", "top_genres_json")
            .expect("column lookup"),
        "playlists.top_genres_json should exist"
    );
    assert!(
        store
            .table_has_column("playlists", "owner")
            .expect("column lookup"),
        "playlists.owner should exist"
    );
    for table in [
        "cover_cache",
        "external_image_lookup_misses",
        "collection_cover_refs",
        "entity_content_refs",
        "content_cache_entries",
    ] {
        assert!(
            !store.table_exists(table).expect("table lookup"),
            "{table} should not exist"
        );
    }
    for table in [
        "albums",
        "tracks",
        "artists",
        "album_artists",
        "genres",
        "playlists",
    ] {
        assert!(
            !store
                .table_has_column(table, "image_origin")
                .expect("column lookup"),
            "{table}.image_origin should not exist"
        );
    }
    assert!(store.foreign_keys_enabled().expect("foreign keys"));
    assert!(store.fts5_available().expect("fts5 table"));
    assert!(
        !store.table_exists("app_settings").expect("table lookup"),
        "settings are persisted outside the SQLite store"
    );
}

#[test]
fn version_27_removes_artwork_mirrors_without_losing_owned_facts() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "artwork-owner-migration"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = stored_source_with_id("local:server:artwork-migration");
    let albums = (1..=4).map(album_with_image).collect::<Vec<_>>();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&saved).expect("save source");
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        store
            .upsert_albums(&saved.source_id, &albums, generation)
            .expect("save albums");
        store
            .connection
            .execute(
                "
                INSERT INTO local_artwork_manifest (
                    source_id, cover_item_id, manifest_version, source_kind, source_path,
                    source_size, mtime_seconds, mtime_nanos, content_hash, revision,
                    scan_generation
                )
                VALUES (?1, 'local-cover-one', 1, 'file', '/music/cover.jpg',
                        123, 456, 789, 'content-hash', 'file-revision', ?2)
                ",
                rusqlite::params![saved.source_id.as_str(), generation],
            )
            .expect("save Local artwork fact");
    }

    let connection = rusqlite::Connection::open(&path).expect("open version 27 database");
    simulate_pre_artwork_owner_schema(&connection);
    connection
        .execute_batch(
            "
            UPDATE albums SET image_origin = 'source' WHERE album_id = 'album-1';
            UPDATE albums SET image_origin = 'unknown' WHERE album_id = 'album-2';
            UPDATE albums SET image_origin = 'fallback' WHERE album_id = 'album-3';
            UPDATE albums SET image_origin = 'external' WHERE album_id = 'album-4';
            ",
        )
        .expect("restore version 27 artwork schema");
    connection
        .execute(
            "INSERT INTO cover_cache VALUES (?1, 'item', 'tag', 300, '/cache/cover', CURRENT_TIMESTAMP)",
            rusqlite::params![saved.source_id.as_str()],
        )
        .expect("seed cover cache");
    connection
        .execute(
            "INSERT INTO external_image_lookup_misses VALUES (?1, 'item', 'tag', 300, 'missing', CURRENT_TIMESTAMP)",
            rusqlite::params![saved.source_id.as_str()],
        )
        .expect("seed external miss");
    connection
        .execute(
            "INSERT INTO collection_cover_refs VALUES (?1, 'genre', 'genre:1', 0, 'item', 'tag', CURRENT_TIMESTAMP)",
            rusqlite::params![saved.source_id.as_str()],
        )
        .expect("seed collection mirror");
    connection
        .execute(
            "INSERT INTO entity_content_refs VALUES (?1, 'album', 'album:1', 'cover', 'key', 'source', CURRENT_TIMESTAMP)",
            rusqlite::params![saved.source_id.as_str()],
        )
        .expect("seed content ref");
    connection
        .execute(
            "INSERT INTO content_cache_entries VALUES ('artwork', 'cover', 'key', '300', 'ready', '/cache/cover', 'source', CURRENT_TIMESTAMP)",
            [],
        )
        .expect("seed content cache");
    connection
        .pragma_update(None, "user_version", 27)
        .expect("mark version 27");
    drop(connection);

    let store = Store::open(&path).expect("migrate artwork owner schema");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    let migrated = store
        .load_albums(&saved.source_id, 0, 10)
        .expect("load migrated albums")
        .items;
    assert_eq!(migrated[0].image_ref, albums[0].image_ref);
    assert_eq!(migrated[1].image_ref, albums[1].image_ref);
    assert_eq!(migrated[2].image_ref, None);
    assert_eq!(migrated[3].image_ref, None);
    for table in [
        "cover_cache",
        "external_image_lookup_misses",
        "collection_cover_refs",
        "entity_content_refs",
        "content_cache_entries",
    ] {
        assert!(!store.table_exists(table).expect("table lookup"), "{table}");
    }
    for table in [
        "albums",
        "tracks",
        "artists",
        "album_artists",
        "genres",
        "playlists",
    ] {
        assert!(
            !store
                .table_has_column(table, "image_origin")
                .expect("column lookup"),
            "{table}"
        );
    }
    let local_fact = store
        .connection
        .query_row(
            "
            SELECT source_kind, source_path, revision, scan_generation
            FROM local_artwork_manifest
            WHERE source_id = ?1 AND cover_item_id = 'local-cover-one'
            ",
            rusqlite::params![saved.source_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .expect("load Local artwork fact");
    assert_eq!(
        local_fact,
        (
            "file".to_string(),
            "/music/cover.jpg".to_string(),
            "file-revision".to_string(),
            1
        )
    );
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn version_25_migrates_sources_to_opaque_provider_payload() {
    let store = Store::open_memory().expect("open store");
    let mut jellyfin = stored_source_with_id("jellyfin:server:migration");
    jellyfin.name = "Jellyfin Migration".to_string();
    jellyfin.provider_payload = r#"{"version":1,"base_url":"https://music.example","user_id":"user","username":"demo","trust_invalid_cert":true,"use_jellyfin_instant_mix":true}"#
        .to_string();
    let mut unknown = stored_source_with_id("future:source:migration");
    unknown.kind = "future-provider".to_string();
    unknown.name = "Unknown Migration".to_string();
    unknown.provider_payload = r#"{"version":1,"base_url":"file:///music","user_id":"","username":"","trust_invalid_cert":false,"use_jellyfin_instant_mix":false}"#
        .to_string();

    store.save_source(&jellyfin).expect("save Jellyfin source");
    store.save_source(&unknown).expect("save unknown source");
    store
        .set_active_source(&unknown.source_id)
        .expect("set active source");
    let local_access = SourceLocalAccess {
        source_id: unknown.source_id.clone(),
        root_path: "/music".to_string(),
        path_replace_from: Some("/server/music".to_string()),
        path_replace_to: Some("/music".to_string()),
    };
    store
        .save_source_local_access(&local_access)
        .expect("save local access");

    simulate_pre_provider_payload_schema(&store.connection);
    store
        .connection
        .pragma_update(None, "user_version", 25)
        .expect("simulate version 25");

    store.migrate().expect("migrate version 25");

    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert_eq!(
        store
            .stored_source(&jellyfin.source_id)
            .expect("load Jellyfin source"),
        Some(jellyfin)
    );
    assert_eq!(
        store
            .stored_source(&unknown.source_id)
            .expect("load unknown source"),
        Some(unknown.clone())
    );
    assert_eq!(store.active_source().expect("active source"), Some(unknown));
    assert_eq!(
        store
            .source_local_access(&local_access.source_id)
            .expect("load local access"),
        Some(local_access)
    );
    for column in [
        "base_url",
        "user_id",
        "username",
        "trust_invalid_cert",
        "use_jellyfin_instant_mix",
    ] {
        assert!(
            !store
                .table_has_column("sources", column)
                .expect("column lookup"),
            "sources.{column} should be removed"
        );
    }
    let foreign_key_violations = store
        .connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("foreign key check");
    assert_eq!(foreign_key_violations, 0);
}

#[test]
fn version_24_migrates_sync_state_and_source_objects() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save source");
    store
        .connection
        .execute(
            "UPDATE sync_state SET last_completed_at = '2026-07-01 12:00:00' WHERE source_id = ?1",
            rusqlite::params![saved.source_id.as_str()],
        )
        .expect("seed completion time");
    let old_mapping = SourceObjectMapping {
        source_object_id: "provider-album-1".to_string(),
        entity_kind: SourceEntityKind::Album,
        entity_id: "album-1".to_string(),
    };
    seed_source_object_mappings(
        &store,
        &saved.source_id,
        std::slice::from_ref(&old_mapping),
        4,
    );
    let local_source_object_id = local_file_source_object_id("/music", "track.flac");
    store
        .connection
        .execute(
            "
            INSERT INTO source_objects (
                source_id, source_object_id, entity_kind, source_object_kind,
                source_path, sync_generation
            ) VALUES (?1, ?2, '', 'local_file', '/music/track.flac', 4)
            ",
            rusqlite::params![saved.source_id.as_str(), local_source_object_id.as_str()],
        )
        .expect("seed local source object");
    simulate_pre_provider_payload_schema(&store.connection);
    store
        .connection
        .execute_batch(
            "
            DROP INDEX source_objects_entity_idx;
            DROP INDEX source_objects_parent_idx;
            ALTER TABLE source_objects RENAME TO source_objects_v25;
            CREATE TABLE source_objects (
                source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
                source_object_id TEXT NOT NULL,
                entity_kind TEXT,
                entity_id TEXT,
                source_object_kind TEXT NOT NULL,
                source_path TEXT,
                parent_source_object_id TEXT,
                cue_path TEXT,
                cue_revision TEXT,
                cue_track_index INTEGER,
                segment_start_ms INTEGER,
                segment_end_ms INTEGER,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                sync_generation INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                CHECK (
                    source_object_kind != 'cue_track'
                    OR (
                        entity_kind IS NOT NULL
                        AND entity_id IS NOT NULL
                        AND parent_source_object_id IS NOT NULL
                        AND cue_path IS NOT NULL
                        AND cue_revision IS NOT NULL
                        AND cue_track_index IS NOT NULL
                        AND segment_start_ms IS NOT NULL
                        AND segment_end_ms IS NOT NULL
                    )
                ),
                PRIMARY KEY (source_id, source_object_id)
            );
            CREATE INDEX source_objects_entity_idx
                ON source_objects(source_id, entity_kind, entity_id);
            CREATE INDEX source_objects_parent_idx
                ON source_objects(source_id, parent_source_object_id);
            INSERT INTO source_objects (
                source_id, source_object_id, entity_kind, entity_id, source_object_kind,
                source_path, parent_source_object_id, cue_path, cue_revision,
                cue_track_index, segment_start_ms, segment_end_ms, metadata_json,
                sync_generation, updated_at
            )
            SELECT source_id, source_object_id, NULLIF(entity_kind, ''), entity_id,
                   source_object_kind, source_path, parent_source_object_id, cue_path,
                   cue_revision, cue_track_index, segment_start_ms, segment_end_ms,
                   metadata_json, sync_generation, updated_at
            FROM source_objects_v25;
            DROP TABLE source_objects_v25;
            ALTER TABLE sync_state DROP COLUMN last_all_completed_at;
            ALTER TABLE sync_state DROP COLUMN cache_revision;
            PRAGMA user_version = 24;
            ",
        )
        .expect("simulate version 24");

    store.migrate().expect("migrate version 24");

    assert_eq!(store.schema_version().expect("schema version"), 28);
    let state = store.sync_state(&saved.source_id).expect("sync state");
    assert_eq!(state.cache_revision, 0);
    assert_eq!(
        state.last_all_completed_at.as_deref(),
        Some("2026-07-01 12:00:00")
    );
    assert_eq!(
        store
            .source_object_mappings(&saved.source_id, &old_mapping.source_object_id)
            .expect("load migrated source object"),
        vec![old_mapping]
    );
    let local_source = store
        .connection
        .query_row(
            "SELECT entity_kind, source_object_kind FROM source_objects WHERE source_id = ?1 AND source_object_id = ?2",
            rusqlite::params![saved.source_id.as_str(), local_source_object_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("load migrated local source object");
    assert_eq!(local_source, (String::new(), "local_file".to_string()));
}

#[test]
fn one_source_object_maps_to_both_artist_roles() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save source");
    let source_object_id = "provider-artist-1";
    let mappings = vec![
        SourceObjectMapping {
            source_object_id: source_object_id.to_string(),
            entity_kind: SourceEntityKind::Artist,
            entity_id: "artist-1".to_string(),
        },
        SourceObjectMapping {
            source_object_id: source_object_id.to_string(),
            entity_kind: SourceEntityKind::AlbumArtist,
            entity_id: "album-artist-1".to_string(),
        },
    ];

    seed_source_object_mappings(&store, &saved.source_id, &mappings, 7);

    assert_eq!(
        store
            .source_object_mappings(&saved.source_id, source_object_id)
            .expect("load source object mappings"),
        vec![mappings[1].clone(), mappings[0].clone()]
    );
}
#[test]
fn schema_create_indexes() {
    let store = Store::open_memory().expect("open store");
    for (table, index) in [
        ("albums", "albums_source_title_nocase_idx"),
        ("albums", "albums_source_artist_idx"),
        ("tracks", "tracks_source_artist_idx"),
        ("artists", "artists_source_name_nocase_idx"),
        ("album_artists", "album_artists_source_name_nocase_idx"),
        ("genres", "genres_source_name_nocase_idx"),
        ("playlists", "playlists_source_name_nocase_idx"),
        ("playlist_tracks", "playlist_tracks_order_idx"),
        ("album_genres", "album_genres_source_genre_idx"),
        ("track_genres", "track_genres_source_genre_idx"),
        ("album_artist_links", "album_artist_links_source_artist_idx"),
        ("track_artist_links", "track_artist_links_source_artist_idx"),
        ("track_music_folders", "track_music_folders_folder_idx"),
        ("track_music_folders", "track_music_folders_track_idx"),
        ("track_local_matches", "track_local_matches_track_idx"),
        ("local_file_manifest", "local_file_manifest_track_idx"),
        ("local_file_manifest", "local_file_manifest_album_idx"),
        ("local_file_manifest", "local_file_manifest_generation_idx"),
        ("local_file_manifest", "local_file_manifest_root_idx"),
        (
            "local_artwork_manifest",
            "local_artwork_manifest_source_idx",
        ),
        ("source_objects", "source_objects_entity_idx"),
        ("source_objects", "source_objects_parent_idx"),
    ] {
        assert!(index_exists(&store, table, index), "{index} should exist");
    }
}
fn local_manifest_entry() -> LocalManifestEntry {
    let album = album(1);
    let mut track = track(1, &album);
    track.local_path = Some("/music/Album/track.mp3".to_string());
    track.source_format = Some("mp3".to_string());
    LocalManifestEntry {
        facts: LocalFileFacts {
            path: PathBuf::from("/music/Album/track.mp3"),
            root_path: PathBuf::from("/music"),
            relative_path: "Album/track.mp3".to_string(),
            file_size: 123,
            mtime_seconds: 456,
            mtime_nanos: 789,
            inode: Some(10),
            device: Some(20),
        },
        track,
        album_artist: "Artist".to_string(),
        musicbrainz_album_id: Some("mb-album-one".to_string()),
        musicbrainz_release_group_id: Some("mb-group-one".to_string()),
        cover: Some(LocalManifestCover {
            item_id: "local:cover:file%3A%2Fmusic%2FAlbum%2Fcover.jpg".to_string(),
            kind: LocalManifestCoverKind::File,
            source_path: PathBuf::from("/music/Album/cover.jpg"),
            revision: "file:cover-one".to_string(),
            embedded_index: None,
        }),
        metadata_hash: "metadata-one".to_string(),
        search_hash: "search-one".to_string(),
    }
}

fn seed_local_manifest(
    store: &Store,
    source_id: &SourceId,
    generation: i64,
    entries: &[LocalManifestEntry],
) {
    store
        .write_batch(|connection| {
            store.require_current_sync_generation(source_id, generation)?;
            super::local_manifest::apply_local_manifest_delta_on_connection(
                connection,
                source_id,
                generation,
                &LocalManifestDelta {
                    upserted_entries: entries.to_vec(),
                    ..LocalManifestDelta::default()
                },
            )
        })
        .expect("seed Local manifest");
}

fn seed_source_object_mappings(
    store: &Store,
    source_id: &SourceId,
    mappings: &[SourceObjectMapping],
    generation: i64,
) {
    for mapping in mappings {
        store
            .connection
            .execute(
                "
                INSERT INTO source_objects (
                    source_id, source_object_id, entity_kind, entity_id,
                    source_object_kind, metadata_json, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, 'source', '{}', ?5)
                ",
                rusqlite::params![
                    source_id.as_str(),
                    mapping.source_object_id.as_str(),
                    mapping.entity_kind.as_str(),
                    mapping.entity_id.as_str(),
                    generation,
                ],
            )
            .expect("seed source object mapping");
    }
}

fn entity_key_count(
    store: &Store,
    source_id: &SourceId,
    entity_kind: &str,
    namespace: &str,
    value: &str,
) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entity_identity_keys
            WHERE source_id = ?1
              AND entity_kind = ?2
              AND namespace = ?3
              AND value = ?4
            ",
            rusqlite::params![source_id.as_str(), entity_kind, namespace, value],
            |row| row.get(0),
        )
        .expect("count identity keys")
}

fn grouping_key_count(
    store: &Store,
    source_id: &SourceId,
    entity_kind: &str,
    namespace: &str,
    value: &str,
) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entity_grouping_keys
            WHERE source_id = ?1
              AND entity_kind = ?2
              AND namespace = ?3
              AND value = ?4
            ",
            rusqlite::params![source_id.as_str(), entity_kind, namespace, value],
            |row| row.get(0),
        )
        .expect("count grouping keys")
}

fn entity_fact_count(
    store: &Store,
    source_id: &SourceId,
    entity_kind: &str,
    entity_id: &str,
    fact_key: &str,
) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entity_facts
            WHERE source_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND fact_key = ?4
            ",
            rusqlite::params![source_id.as_str(), entity_kind, entity_id, fact_key],
            |row| row.get(0),
        )
        .expect("count entity facts")
}

#[test]
fn file_store_reset() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "reset"
    ));
    let _cleanup = fs::remove_file(&path);
    let connection = rusqlite::Connection::open(&path).expect("open old connection");
    connection
        .execute_batch(
            "
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO schema_migrations (version) VALUES (12);
                CREATE TABLE stale_cache (value TEXT NOT NULL);
                INSERT INTO stale_cache VALUES ('old row');
                CREATE TABLE servers (
                    server_id TEXT PRIMARY KEY,
                    provider TEXT NOT NULL,
                    name TEXT NOT NULL,
                    base_url TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    username TEXT NOT NULL,
                    trust_invalid_cert INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO servers (
                    server_id, provider, name, base_url, user_id, username, trust_invalid_cert
                )
                VALUES (
                    'jellyfin:server:old', 'jellyfin', 'Old Server',
                    'https://music.example', 'user', 'demo', 0
                );
                ",
        )
        .expect("seed old schema");
    drop(connection);
    let store = Store::open(&path).expect("open reset store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert!(store.foreign_keys_enabled().expect("foreign keys"));
    assert!(store.fts5_available().expect("fts5 table"));
    assert!(
        !store
            .table_exists("schema_migrations")
            .expect("table lookup")
    );
    assert!(!store.table_exists("stale_cache").expect("table lookup"));
    assert!(store.table_exists("sources").expect("table lookup"));
    assert!(store.list_sources().expect("list sources").is_empty());
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn user_version_ten() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "incomplete"
    ));
    let _cleanup = fs::remove_file(&path);
    let connection = rusqlite::Connection::open(&path).expect("open incomplete connection");
    connection
        .execute_batch(
            "
                PRAGMA user_version = 10;
                CREATE TABLE servers (
                    server_id TEXT PRIMARY KEY,
                    provider TEXT NOT NULL,
                    name TEXT NOT NULL,
                    base_url TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    username TEXT NOT NULL,
                    trust_invalid_cert INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO servers (
                    server_id, provider, name, base_url, user_id, username, trust_invalid_cert
                )
                VALUES (
                    'jellyfin:server:old', 'jellyfin', 'Old Server',
                    'https://music.example', 'user', 'demo', 0
                );
                ",
        )
        .expect("seed incomplete schema");
    drop(connection);
    let store = Store::open(&path).expect("open reset store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert!(store.table_exists("tracks").expect("table lookup"));
    assert!(store.list_sources().expect("list sources").is_empty());
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn schema_reopen_preserves_opaque_source_payload() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "preserve-current"
    ));
    let _cleanup = fs::remove_file(&path);
    let mut saved = stored_source_with_id("future:source:opaque");
    saved.kind = "future-provider".to_string();
    saved.provider_payload =
        "{\n  \"version\": 99, \"future\": [3, 2, 1], \"extension\": true\n}".to_string();
    {
        let store = Store::open(&path).expect("open store");
        store.save_source(&saved).expect("save source");
        store
            .set_active_source(&saved.source_id)
            .expect("set active source");
    }

    let store = Store::open(&path).expect("reopen store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert_eq!(
        store.list_sources().expect("list sources"),
        vec![saved.clone()]
    );
    assert_eq!(store.active_source().expect("active source"), Some(saved));
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn schema_upgrade_servers() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "v18-upgrade"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = stored_source();
    let genre_name = "Genre 1".to_string();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&saved).expect("save server");
        store
            .set_active_source(&saved.source_id)
            .expect("set active server");
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        let mut album = album(1);
        album.genres = vec![genre_name.clone()];
        let mut first_track = track(1, &album);
        first_track.genres = vec![genre_name.clone()];
        first_track.duration_seconds = 180;
        let mut second_track = track(2, &album);
        second_track.genres = vec![genre_name.clone()];
        second_track.duration_seconds = 240;
        let mut cached_genre = genre(1, None);
        cached_genre.name = genre_name.clone();
        cached_genre.duration_seconds = 0;
        LibraryObservation {
            albums: vec![album],
            tracks: vec![first_track, second_track],
            genres: vec![cached_genre],
            ..LibraryObservation::default()
        }
        .commit(&store, &saved.source_id, generation)
        .expect("commit library");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    simulate_pre_source_identity_schema(&connection);
    connection
        .execute_batch(
            "
                ALTER TABLE genres DROP COLUMN duration_seconds;
                ALTER TABLE servers DROP COLUMN use_jellyfin_instant_mix;
                PRAGMA user_version = 18;
                ",
        )
        .expect("simulate previous schema");
    drop(connection);

    let store = Store::open(&path).expect("open upgraded store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert_eq!(
        store.list_sources().expect("list sources"),
        vec![saved.clone()]
    );
    assert_eq!(
        store.active_source().expect("active server"),
        Some(saved.clone())
    );
    assert!(
        store
            .table_has_column("genres", "duration_seconds")
            .expect("column lookup"),
        "genres.duration_seconds should exist after migration"
    );
    let genres = store
        .load_genres(&saved.source_id, 0, 10)
        .expect("load genres")
        .items;
    assert_eq!(genres[0].duration_seconds, 420);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn schema_upgrade_preserves_queue_snapshot_json() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "queue-snapshot-source-identity"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = stored_source();
    let track = track(1, &album(1));
    let occurrence_id = "queue-entry:legacy-track";
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&saved).expect("save source");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    simulate_pre_source_identity_schema(&connection);
    let legacy_payload = serde_json::json!({
        "server_id": saved.source_id,
        "entries": [{
            "id": occurrence_id,
            "track_id": track.id,
            "album_id": track.album_id,
            "title": track.title,
            "artist": track.artist,
            "artist_id": track.artist_id,
            "album": track.album,
            "year": track.year,
            "duration_seconds": track.duration_seconds,
            "favorite": track.favorite,
            "image_ref": track.image_ref,
            "local_path": track.local_path,
            "source_format": track.source_format,
            "origin": { "Manual": {} }
        }],
        "current_index": 0,
        "repeat_mode": "All",
        "shuffle": { "enabled": false, "seed": 0 },
        "shuffle_order": [0],
        "progress_seconds": 0
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO queue_snapshots (server_id, value) VALUES (?1, ?2)",
            rusqlite::params![saved.source_id.as_str(), legacy_payload],
        )
        .expect("write legacy queue snapshot value");
    connection
        .execute_batch("PRAGMA user_version = 22;")
        .expect("simulate previous schema");
    drop(connection);

    let store = Store::open(&path).expect("open upgraded store");
    let checkpoint = store
        .load_playback_checkpoint(&saved.source_id)
        .expect("load upgraded checkpoint")
        .expect("playback checkpoint");
    assert_eq!(checkpoint.source_id, saved.source_id);
    assert_eq!(checkpoint.revision, 0);
    assert_eq!(
        checkpoint.selected_occurrence_id.as_deref(),
        Some(occurrence_id)
    );
    assert_eq!(checkpoint.progress_millis, 0);
    assert_eq!(checkpoint.repeat_mode, "All");
    assert!(!checkpoint.shuffle_enabled);
    assert_eq!(checkpoint.payload, legacy_payload);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn schema_upgrade_moves_lifetime_activity_to_legacy_period() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "activity-period-migration"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = stored_source();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&saved).expect("save source");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    simulate_pre_playback_owner_schema(&connection);
    connection
        .execute(
            "
            INSERT INTO track_activity (
                source_id, track_id, play_count, last_played, skip_count,
                play_recorded_session
            )
            VALUES (?1, ?2, 5, '2026-06-30T20:00:00Z', 2, 'retired-session')
            ",
            rusqlite::params![saved.source_id.as_str(), TrackId::fake(1).as_str()],
        )
        .expect("seed legacy activity");
    connection
        .execute_batch("PRAGMA user_version = 26;")
        .expect("simulate previous schema");
    drop(connection);

    let store = Store::open(&path).expect("open upgraded store");
    let summary = store
        .track_activity_summary(&saved.source_id, &TrackId::fake(1))
        .expect("load migrated activity");
    assert_eq!(summary.qualified_plays, 5);
    assert_eq!(summary.skips, 2);
    assert_eq!(
        summary.last_played_at.as_deref(),
        Some("2026-06-30T20:00:00Z")
    );
    let period = store
        .connection
        .query_row(
            "SELECT period FROM track_activity_period WHERE source_id = ?1 AND track_id = ?2",
            rusqlite::params![saved.source_id.as_str(), TrackId::fake(1).as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load migrated activity period");
    assert_eq!(period, LEGACY_ACTIVITY_PERIOD);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn schema_upgrade_backfills_artist_label_links() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "artist-label-link-backfill"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = stored_source();
    let mut album = album(4);
    album.artist_id = None;
    let mut track = track(1, &album);
    track.artist = album.artist.clone();
    track.artist_id = None;
    let mut artist = artist(1, None);
    artist.name = album.artist.clone();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&saved).expect("save source");
        let generation = store.begin_sync(&saved.source_id).expect("begin sync");
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![track.clone()],
            artists: vec![artist.clone()],
            ..LibraryObservation::default()
        }
        .commit(&store, &saved.source_id, generation)
        .expect("commit library");
        simulate_pre_provider_payload_schema(&store.connection);
        store
            .connection
            .execute_batch("PRAGMA user_version = 23;")
            .expect("simulate previous schema");
    }

    let store = Store::open(&path).expect("open upgraded store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    let track_link_count: i64 = store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM track_artist_links
            WHERE source_id = ?1
              AND track_id = ?2
              AND artist_id = ?3
            ",
            rusqlite::params![
                saved.source_id.as_str(),
                track.id.as_str(),
                artist.id.as_str()
            ],
            |row| row.get(0),
        )
        .expect("track artist link count");
    let album_link_count: i64 = store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM album_artist_links
            WHERE source_id = ?1
              AND album_id = ?2
              AND artist_id = ?3
            ",
            rusqlite::params![
                saved.source_id.as_str(),
                album.id.as_str(),
                artist.id.as_str()
            ],
            |row| row.get(0),
        )
        .expect("album artist link count");
    assert_eq!(track_link_count, 1);
    assert_eq!(album_link_count, 1);
    let detail = store
        .load_artist_detail(&saved.source_id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.albums.len(), 1);
    assert_eq!(detail.albums[0].id, album.id);
    assert_eq!(detail.tracks.len(), 1);
    assert_eq!(detail.tracks[0].id, track.id);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn schema_twenty_local_playlists_migrate_to_store_owner() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "playlist-owner-migration"
    ));
    let _cleanup = fs::remove_file(&path);
    let mut local = stored_source_with_id("local:server:test");
    local.kind = "local".to_string();
    let remote = stored_source_with_id("jellyfin:server:test");
    let local_playlist = playlist(1, None);
    let remote_playlist = playlist(2, None);
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&local).expect("save local");
        store.save_source(&remote).expect("save remote");
        let local_generation = store
            .begin_sync(&local.source_id)
            .expect("begin local sync");
        let remote_generation = store
            .begin_sync(&remote.source_id)
            .expect("begin remote sync");
        store
            .upsert_playlists(
                &local.source_id,
                std::slice::from_ref(&local_playlist),
                local_generation,
            )
            .expect("upsert pre-migration local playlist");
        store
            .upsert_playlists(
                &remote.source_id,
                std::slice::from_ref(&remote_playlist),
                remote_generation,
            )
            .expect("upsert pre-migration remote playlist");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    simulate_pre_source_identity_schema(&connection);
    connection
        .execute_batch(
            "
            ALTER TABLE playlists DROP COLUMN owner;
            PRAGMA user_version = 20;
            ",
        )
        .expect("simulate schema twenty");
    drop(connection);

    let store = Store::open(&path).expect("open upgraded store");
    assert_eq!(
        store
            .playlist_owner(&local.source_id, &local_playlist.id)
            .expect("local playlist owner"),
        Some(SourceFeatureOwner::Store)
    );
    assert_eq!(
        store
            .playlist_owner(&remote.source_id, &remote_playlist.id)
            .expect("remote playlist owner"),
        Some(SourceFeatureOwner::Native)
    );
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn schema_twenty_one_local_favorites_seed_overrides() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "favorite-override-migration"
    ));
    let _cleanup = fs::remove_file(&path);
    let mut local = stored_source_with_id("local:server:favorites");
    local.kind = "local".to_string();
    let remote = stored_source_with_id("jellyfin:server:favorites");
    let mut local_album = album(1);
    local_album.favorite = true;
    let mut remote_album = album(2);
    remote_album.favorite = true;
    let mut local_track = track(1, &local_album);
    local_track.favorite = true;
    let mut remote_track = track(2, &remote_album);
    remote_track.favorite = true;
    let mut local_artist = artist(1, None);
    local_artist.favorite = true;
    let mut remote_artist = artist(2, None);
    remote_artist.favorite = true;
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&local).expect("save local");
        store.save_source(&remote).expect("save remote");
        let local_generation = store
            .begin_sync(&local.source_id)
            .expect("begin local sync");
        let remote_generation = store
            .begin_sync(&remote.source_id)
            .expect("begin remote sync");
        store
            .upsert_albums(
                &local.source_id,
                std::slice::from_ref(&local_album),
                local_generation,
            )
            .expect("upsert local album");
        store
            .upsert_albums(
                &remote.source_id,
                std::slice::from_ref(&remote_album),
                remote_generation,
            )
            .expect("upsert remote album");
        store
            .upsert_tracks(
                &local.source_id,
                std::slice::from_ref(&local_track),
                local_generation,
            )
            .expect("upsert local track");
        store
            .upsert_tracks(
                &remote.source_id,
                std::slice::from_ref(&remote_track),
                remote_generation,
            )
            .expect("upsert remote track");
        store
            .upsert_artists(
                &local.source_id,
                std::slice::from_ref(&local_artist),
                false,
                local_generation,
            )
            .expect("upsert local artist");
        store
            .upsert_artists(
                &remote.source_id,
                std::slice::from_ref(&remote_artist),
                false,
                remote_generation,
            )
            .expect("upsert remote artist");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    simulate_pre_source_identity_schema(&connection);
    connection
        .execute_batch(
            "
            DROP TABLE item_favorite_overrides;
            PRAGMA user_version = 21;
            ",
        )
        .expect("simulate schema twenty one");
    drop(connection);

    let store = Store::open(&path).expect("open upgraded store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    let local_override_count = store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM item_favorite_overrides
            WHERE source_id = ?1
            ",
            rusqlite::params![local.source_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .expect("local override count");
    let remote_override_count = store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM item_favorite_overrides
            WHERE source_id = ?1
            ",
            rusqlite::params![remote.source_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .expect("remote override count");
    assert_eq!(local_override_count, 3);
    assert_eq!(remote_override_count, 0);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn schema_seventeen_resets() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "v17-reset"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = stored_source();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_source(&saved).expect("save server");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    connection
        .pragma_update(None, "user_version", 17)
        .expect("set unsupported schema version");
    drop(connection);

    let store = Store::open(&path).expect("open reset store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert!(store.list_sources().expect("list sources").is_empty());
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}

#[test]
fn future_user_version() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "future"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = stored_source();
    {
        let store = Store::open(&path).expect("open store");
        store.save_source(&saved).expect("save server");
    }
    let connection = rusqlite::Connection::open(&path).expect("open future connection");
    connection
        .pragma_update(None, "user_version", 29)
        .expect("set future schema version");
    drop(connection);

    let store = Store::open(&path).expect("open reset store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    assert!(store.list_sources().expect("list sources").is_empty());
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn schema_use_mode() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "wal"
    ));
    let _cleanup = fs::remove_file(&path);
    let store = Store::open(&path).expect("open file store");
    assert_eq!(store.journal_mode().expect("journal mode"), "wal");
    drop(store);
    let _cleanup = fs::remove_file(path);
}
#[test]
fn store_configures_busy_timeout() {
    let store = Store::open_memory().expect("open store");
    assert_eq!(store.busy_timeout_ms().expect("busy timeout"), 30_000);
}
#[test]
fn shared_write_gate_serializes_connections_before_sqlite_busy_handling() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "shared-write-gate"
    ));
    let _cleanup = fs::remove_file(&path);
    let gate = crate::StoreWriteGate::default();
    let holder = Store::open_with_write_gate(&path, gate.clone()).expect("open holder");
    let contender = Store::open_with_write_gate(&path, gate.clone()).expect("open contender");
    contender
        .connection
        .busy_timeout(std::time::Duration::ZERO)
        .expect("disable SQLite busy wait");
    let source = stored_source();
    let source_id = source.source_id.clone();
    holder.save_source(&source).expect("save source");
    holder.set_active_source(&source_id).expect("select source");
    let album = album(1);
    let track = track(1, &album);
    let track_id = track.id.clone();
    let setup_generation = holder.begin_sync(&source_id).expect("begin setup sync");
    LibraryObservation {
        albums: vec![album],
        tracks: vec![track],
        ..LibraryObservation::default()
    }
    .commit(&holder, &source_id, setup_generation)
    .expect("seed library");
    let generation = holder.begin_sync(&source_id).expect("begin held sync");
    let base_cache_revision = holder
        .source_cache_revision(&source_id)
        .expect("read cache revision");
    let reader = Store::open_fast_read(&path).expect("open concurrent reader");

    let (holder_entered_tx, holder_entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder_source_id = source_id.clone();
    let holder_thread = std::thread::spawn(move || {
        holder.finish_library_sync(
            &holder_source_id,
            generation,
            base_cache_revision,
            true,
            || {
                holder_entered_tx.send(()).expect("report holder");
                release_rx.recv().expect("release holder");
                Ok(crate::LibraryDelta::default())
            },
        )
    });
    holder_entered_rx
        .recv()
        .expect("holder entered transaction");
    assert_eq!(
        reader
            .active_source()
            .expect("read while writer is held")
            .expect("active source")
            .source_id,
        source_id,
        "WAL readers must remain independent from the write gate"
    );

    let (contender_finished_tx, contender_finished_rx) = std::sync::mpsc::channel();
    let contender_source_id = source_id.clone();
    let contender_track_id = track_id.clone();
    let contender_thread = std::thread::spawn(move || {
        let result = contender.set_track_favorite_for_owner(
            &contender_source_id,
            &contender_track_id,
            true,
            SourceFeatureOwner::Native,
        );
        contender_finished_tx.send(()).expect("report contender");
        result
    });
    assert!(
        contender_finished_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err(),
        "ordinary mutations must wait at the Store gate"
    );
    release_tx.send(()).expect("release writer");
    holder_thread
        .join()
        .expect("join holder")
        .expect("holder commit");
    contender_thread
        .join()
        .expect("join contender")
        .expect("contender commit without SQLite busy retry");
    contender_finished_rx
        .recv()
        .expect("contender finished after holder");
    let verifier = Store::open_with_write_gate(&path, gate).expect("open verifier");
    assert!(
        verifier
            .load_tracks(&source_id, 0, 1)
            .expect("load track")
            .items[0]
            .favorite
    );

    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn store_recognizes_writer_contention_by_sqlite_code() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "writer-contention"
    ));
    let _cleanup = fs::remove_file(&path);
    let holder = Store::open(&path).expect("open lock holder");
    holder
        .connection
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold writer slot");
    let contender = rusqlite::Connection::open(&path).expect("open contender");
    contender
        .busy_timeout(std::time::Duration::ZERO)
        .expect("disable contender wait");

    let error = contender
        .execute_batch("BEGIN IMMEDIATE")
        .expect_err("writer slot should be busy");
    assert!(StoreError::from(error).is_contention());

    holder
        .connection
        .execute_batch("ROLLBACK")
        .expect("release writer slot");
    drop(contender);
    drop(holder);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn store_fast_read_has_no_busy_timeout() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "fast-read"
    ));
    let _cleanup = fs::remove_file(&path);
    {
        let store = Store::open(&path).expect("open file store");
        assert_eq!(store.schema_version().expect("schema version"), 28);
    }
    let store = Store::open_fast_read(&path).expect("open fast read store");
    assert_eq!(store.busy_timeout_ms().expect("busy timeout"), 0);
    assert_eq!(store.schema_version().expect("schema version"), 28);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn current_schema_migrate_is_read_only() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "current-migrate-read-only"
    ));
    let _cleanup = fs::remove_file(&path);
    {
        let store = Store::open(&path).expect("open file store");
        assert_eq!(store.schema_version().expect("schema version"), 28);
    }

    let store =
        Store::open_file(&path, crate::StoreWriteGate::default()).expect("open current store");
    store
        .connection
        .pragma_update(None, "query_only", true)
        .expect("enable query-only mode");
    store.migrate().expect("migrate current store");
    assert_eq!(store.schema_version().expect("schema version"), 28);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn playback_checkpoint_round_trips_opaque_payload() {
    let store = Store::open_memory().expect("open store");
    let source = stored_source();
    store.save_source(&source).expect("save source");
    let checkpoint = PlaybackCheckpointRecord {
        source_id: source.source_id.clone(),
        revision: 7,
        selected_occurrence_id: Some("occurrence-2".to_string()),
        progress_millis: 12_345,
        repeat_mode: "One".to_string(),
        shuffle_enabled: true,
        payload: r#"{"session":"opaque"}"#.to_string(),
    };
    store
        .save_playback_checkpoint(&checkpoint)
        .expect("save playback checkpoint");

    assert_eq!(
        store
            .load_playback_checkpoint(&source.source_id)
            .expect("load playback checkpoint"),
        Some(checkpoint)
    );
    assert_eq!(
        store
            .load_playback_checkpoint(&SourceId::fake(2))
            .expect("load missing checkpoint"),
        None
    );
}

#[test]
fn deleting_a_playback_checkpoint_is_source_scoped() {
    let store = Store::open_memory().expect("open store");
    let first = stored_source_with_id("source:first");
    let second = stored_source_with_id("source:second");
    for source in [&first, &second] {
        store.save_source(source).expect("save source");
        store
            .save_playback_checkpoint(&PlaybackCheckpointRecord {
                source_id: source.source_id.clone(),
                revision: 1,
                selected_occurrence_id: None,
                progress_millis: 0,
                repeat_mode: "Off".to_string(),
                shuffle_enabled: false,
                payload: "opaque".to_string(),
            })
            .expect("save playback checkpoint");
    }

    assert!(
        store
            .delete_playback_checkpoint(&first.source_id)
            .expect("delete playback checkpoint")
    );
    assert!(
        store
            .load_playback_checkpoint(&first.source_id)
            .expect("load deleted checkpoint")
            .is_none()
    );
    assert!(
        store
            .load_playback_checkpoint(&second.source_id)
            .expect("load retained checkpoint")
            .is_some()
    );
    assert!(
        !store
            .delete_playback_checkpoint(&first.source_id)
            .expect("delete missing checkpoint")
    );
}

#[test]
fn playback_progress_requires_matching_revision_and_occurrence() {
    let store = Store::open_memory().expect("open store");
    let source = stored_source();
    store.save_source(&source).expect("save source");
    let checkpoint = PlaybackCheckpointRecord {
        source_id: source.source_id.clone(),
        revision: 7,
        selected_occurrence_id: Some("occurrence-2".to_string()),
        progress_millis: 12_345,
        repeat_mode: "Off".to_string(),
        shuffle_enabled: false,
        payload: "opaque payload".to_string(),
    };
    store
        .save_playback_checkpoint(&checkpoint)
        .expect("save playback checkpoint");

    assert!(
        store
            .save_playback_progress(&source.source_id, 7, "occurrence-2", 73_000)
            .expect("save playback progress")
    );
    for (revision, occurrence) in [(6, "occurrence-2"), (7, "occurrence-1")] {
        assert!(
            !store
                .save_playback_progress(&source.source_id, revision, occurrence, 99_000,)
                .expect("ignore stale playback progress")
        );
    }
    let saved = store
        .load_playback_checkpoint(&source.source_id)
        .expect("load playback checkpoint")
        .expect("saved playback checkpoint");
    assert_eq!(saved.progress_millis, 73_000);
    assert_eq!(saved.payload, checkpoint.payload);
}

#[test]
fn playback_state_updates_scalars_and_rejects_delayed_structure() {
    let store = Store::open_memory().expect("open store");
    let source = stored_source();
    store.save_source(&source).expect("save source");
    let checkpoint = PlaybackCheckpointRecord {
        source_id: source.source_id.clone(),
        revision: 7,
        selected_occurrence_id: Some("occurrence-2".to_string()),
        progress_millis: 12_345,
        repeat_mode: "Off".to_string(),
        shuffle_enabled: false,
        payload: "opaque payload".to_string(),
    };
    store
        .save_playback_checkpoint(&checkpoint)
        .expect("save playback checkpoint");

    assert!(
        store
            .save_playback_state(
                &source.source_id,
                7,
                Some("occurrence-4"),
                44_000,
                "All",
                true,
            )
            .expect("save playback state")
    );
    assert!(
        !store
            .save_playback_state(&source.source_id, 6, None, 0, "One", false)
            .expect("ignore stale playback state")
    );
    for revision in [7, 6] {
        store
            .save_playback_checkpoint(&PlaybackCheckpointRecord {
                source_id: source.source_id.clone(),
                revision,
                selected_occurrence_id: Some("occurrence-2".to_string()),
                progress_millis: 0,
                repeat_mode: "Off".to_string(),
                shuffle_enabled: false,
                payload: format!("delayed revision {revision}"),
            })
            .expect("ignore delayed structural checkpoint");
    }
    let saved = store
        .load_playback_checkpoint(&source.source_id)
        .expect("load playback checkpoint")
        .expect("saved playback checkpoint");
    assert_eq!(
        saved.selected_occurrence_id.as_deref(),
        Some("occurrence-4")
    );
    assert_eq!(saved.progress_millis, 44_000);
    assert_eq!(saved.repeat_mode, "All");
    assert!(saved.shuffle_enabled);
    assert_eq!(saved.payload, checkpoint.payload);
}

#[test]
fn activity_outcomes_upsert_periods_and_aggregate_lifetime() {
    let store = Store::open_memory().expect("open store");
    let source = stored_source();
    let track_id = TrackId::fake(1);
    store.save_source(&source).expect("save source");
    for outcome in [
        ActivityOutcome {
            source_id: source.source_id.clone(),
            period: "2026-06".to_string(),
            track_id: track_id.clone(),
            qualified_plays: 1,
            skips: 0,
            last_played_at: Some(1_782_849_600),
        },
        ActivityOutcome {
            source_id: source.source_id.clone(),
            period: "2026-07".to_string(),
            track_id: track_id.clone(),
            qualified_plays: 1,
            skips: 1,
            last_played_at: Some(1_783_850_400),
        },
        ActivityOutcome {
            source_id: source.source_id.clone(),
            period: "2026-07".to_string(),
            track_id: track_id.clone(),
            qualified_plays: 0,
            skips: 1,
            last_played_at: None,
        },
    ] {
        store
            .record_activity_outcome(&outcome)
            .expect("record activity outcome");
    }

    assert_eq!(
        store
            .track_activity_summary(&source.source_id, &track_id)
            .expect("load activity summary"),
        TrackActivitySummary {
            qualified_plays: 2,
            skips: 2,
            last_played_at: Some("2026-07-12 10:00:00".to_string()),
        }
    );
}

#[test]
fn schema_trip_token() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    store
        .set_active_source(&saved.source_id)
        .expect("set active server");
    assert_eq!(store.active_source().expect("active server"), Some(saved));
}
#[test]
fn schema_load_source() {
    let store = Store::open_memory().expect("open store");
    let playback = stored_source_with_id("server:playback");
    let active = stored_source_with_id("server:active");
    store.save_source(&playback).expect("save playback server");
    store.save_source(&active).expect("save active server");
    store
        .set_active_source(&active.source_id)
        .expect("set active server");

    assert_eq!(
        store
            .stored_source(&playback.source_id)
            .expect("load requested server"),
        Some(playback)
    );
}
#[test]
fn schema_clear_lifecycle() {
    let case = StoreCase::with_source_id("local:server:manifest");
    let entry = local_manifest_entry();
    let first_generation = case.start_sync("begin first sync");

    seed_local_manifest(
        &case,
        &case.id,
        first_generation,
        std::slice::from_ref(&entry),
    );

    assert_eq!(
        case.load_local_manifest(&case.id).expect("load manifest"),
        vec![entry.clone()]
    );

    case.clear_library_cache(&case.id)
        .expect("clear library cache");
    assert!(
        case.load_local_manifest(&case.id)
            .expect("load cleared manifest")
            .is_empty()
    );

    let second_generation = case.start_sync("begin second sync");
    seed_local_manifest(
        &case,
        &case.id,
        second_generation,
        std::slice::from_ref(&entry),
    );
    case.forget_source(&case.id).expect("forget server");
    assert!(
        case.load_local_manifest(&case.id)
            .expect("load forgotten manifest")
            .is_empty()
    );
}

#[test]
fn schema_track_commit() {
    let case = StoreCase::with_source_id("local:server:rollback");
    let album = album(1);
    let mut kept = track(1, &album);
    kept.local_path = Some("/music/Album/kept.mp3".to_string());
    let mut removed = track(2, &album);
    removed.local_path = Some("/music/Album/removed.mp3".to_string());
    let mut kept_entry = local_manifest_entry();
    kept_entry.track = kept.clone();
    kept_entry.facts.path = PathBuf::from("/music/Album/kept.mp3");
    kept_entry.facts.relative_path = "Album/kept.mp3".to_string();
    kept_entry.metadata_hash = "metadata-kept".to_string();
    kept_entry.search_hash = "search-kept".to_string();
    let mut removed_entry = kept_entry.clone();
    removed_entry.track = removed.clone();
    removed_entry.facts.path = PathBuf::from("/music/Album/removed.mp3");
    removed_entry.facts.relative_path = "Album/removed.mp3".to_string();
    removed_entry.metadata_hash = "metadata-removed".to_string();
    removed_entry.search_hash = "search-removed".to_string();
    let first_generation = case.start_sync("begin first sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![kept.clone(), removed.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    seed_local_manifest(
        &case,
        &case.id,
        first_generation,
        &[kept_entry.clone(), removed_entry],
    );
    let failed_generation = case.start_sync("begin failed sync");
    let mut duplicate_manifest = kept_entry.clone();
    duplicate_manifest.track.id = TrackId::fake(99);
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    let error = case.commit_local_library_delta(
        &case.id,
        failed_generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            deleted_track_ids: vec![removed.id.clone()],
            current_album_ids: vec![album.id.clone()],
            dirty_albums: vec![album],
            manifest: LocalManifestDelta {
                upserted_entries: vec![kept_entry, duplicate_manifest],
                ..LocalManifestDelta::default()
            },
            ..LocalLibraryDelta::default()
        },
    );

    let _error = error.expect_err("duplicate manifest should fail");
    let tracks = case
        .load_tracks(&case.id, 0, 10)
        .expect("tracks after failed delta");
    assert_eq!(tracks.total, 2);
    assert_eq!(
        case.track_local_path(&case.id, &removed.id)
            .expect("removed path after failed delta")
            .as_deref(),
        Some("/music/Album/removed.mp3")
    );
}

#[test]
fn local_delta_does_not_rewrite_unchanged_manifest_siblings() {
    let case = StoreCase::with_source_id("local:server:keyed-manifest");
    let mut album = album(1);
    let album_artist_id = album.artist_id.clone().expect("album artist id");
    album.album_artist_credits = vec![credit(album_artist_id.clone(), &album.artist)];
    let mut changed_entry = local_manifest_entry();
    changed_entry.track = track(1, &album);
    changed_entry.track.album_artist_credits = album.album_artist_credits.clone();
    changed_entry.track.local_path = Some("/music/Album/first.mp3".to_string());
    changed_entry.facts.path = PathBuf::from("/music/Album/first.mp3");
    changed_entry.facts.relative_path = "Album/first.mp3".to_string();
    let mut sibling_entry = changed_entry.clone();
    sibling_entry.track = track(2, &album);
    sibling_entry.track.album_artist_credits = album.album_artist_credits.clone();
    sibling_entry.track.local_path = Some("/music/Album/sibling.mp3".to_string());
    sibling_entry.facts.path = PathBuf::from("/music/Album/sibling.mp3");
    sibling_entry.facts.relative_path = "Album/sibling.mp3".to_string();
    sibling_entry.metadata_hash = "metadata-sibling".to_string();
    sibling_entry.search_hash = "search-sibling".to_string();
    let home = HomeSection {
        kind: HomeSectionKind::NewlyAdded,
        albums: vec![album.clone()],
        tracks: vec![changed_entry.track.clone(), sibling_entry.track.clone()],
    };

    let first_generation = case.start_sync("begin first sync");
    seed_local_manifest(
        &case,
        &case.id,
        first_generation,
        &[changed_entry.clone(), sibling_entry.clone()],
    );
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![changed_entry.track.clone(), sibling_entry.track.clone()],
            home_sections: vec![home.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );

    let sibling_source_object_id = local_file_source_object_id("/music", "Album/sibling.mp3");
    case.connection
        .execute_batch(
            "
            CREATE TEMP TABLE local_manifest_sibling_writes (kind TEXT NOT NULL);
            CREATE TEMP TRIGGER audit_sibling_file_update
            AFTER UPDATE ON local_file_manifest
            WHEN OLD.path = '/music/Album/sibling.mp3'
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('file update'); END;
            CREATE TEMP TRIGGER audit_sibling_file_delete
            AFTER DELETE ON local_file_manifest
            WHEN OLD.path = '/music/Album/sibling.mp3'
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('file delete'); END;
            CREATE TEMP TRIGGER audit_sibling_track_update
            AFTER UPDATE ON local_track_manifest_data
            WHEN OLD.track_id = 'track-2'
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('track update'); END;
            CREATE TEMP TRIGGER audit_sibling_track_delete
            AFTER DELETE ON local_track_manifest_data
            WHEN OLD.track_id = 'track-2'
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('track delete'); END;
            CREATE TEMP TRIGGER audit_unchanged_home_insert
            AFTER INSERT ON home_section_items
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('home insert'); END;
            CREATE TEMP TRIGGER audit_unchanged_home_update
            AFTER UPDATE ON home_section_items
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('home update'); END;
            CREATE TEMP TRIGGER audit_unchanged_home_delete
            AFTER DELETE ON home_section_items
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('home delete'); END;
            ",
        )
        .expect("install manifest audit");
    case.connection
        .execute_batch(&format!(
            "
            CREATE TEMP TRIGGER audit_sibling_source_update
            AFTER UPDATE ON source_objects
            WHEN OLD.source_object_id = '{sibling_source_object_id}'
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('source update'); END;
            CREATE TEMP TRIGGER audit_sibling_source_delete
            AFTER DELETE ON source_objects
            WHEN OLD.source_object_id = '{sibling_source_object_id}'
            BEGIN INSERT INTO local_manifest_sibling_writes VALUES ('source delete'); END;
            "
        ))
        .expect("install source audit");

    changed_entry.track.bpm = Some(123);
    changed_entry.metadata_hash = "metadata-changed".to_string();
    let second_generation = case.start_sync("begin changed sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            tracks: vec![changed_entry.track.clone()],
            current_album_ids: vec![album.id.clone()],
            current_artist_ids: vec![album_artist_id.clone()],
            current_album_artist_ids: vec![album_artist_id.clone()],
            dirty_albums: vec![album.clone()],
            home_sections: vec![home.clone()],
            manifest: LocalManifestDelta {
                upserted_entries: vec![changed_entry.clone()],
                ..LocalManifestDelta::default()
            },
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit changed manifest entry");

    let sibling_writes = case
        .connection
        .query_row(
            "SELECT COUNT(*) FROM local_manifest_sibling_writes",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("count sibling writes");
    assert_eq!(sibling_writes, 0);
    assert_eq!(
        case.load_local_manifest(&case.id).expect("load manifest"),
        vec![changed_entry.clone(), sibling_entry]
    );
    let third_generation = case.start_sync("begin identical sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    let committed = case
        .commit_local_library_delta(
            &case.id,
            third_generation,
            base_cache_revision,
            true,
            LocalLibraryDelta {
                tracks: vec![changed_entry.track],
                current_album_ids: vec![album.id.clone()],
                current_artist_ids: vec![album_artist_id.clone()],
                current_album_artist_ids: vec![album_artist_id],
                dirty_albums: vec![album],
                home_sections: vec![home],
                ..LocalLibraryDelta::default()
            },
        )
        .expect("commit identical Local input");

    assert!(committed.delta.is_empty(), "{:?}", committed.delta);
    assert_eq!(
        case.connection
            .query_row(
                "SELECT COUNT(*) FROM local_manifest_sibling_writes",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count unchanged writes"),
        0
    );
}

#[test]
fn local_deletion_keeps_durable_track_state_and_external_mappings() {
    let case = StoreCase::with_source_id("local:server:durable-delete");
    let album = album(1);
    let mut entry = local_manifest_entry();
    entry.track = track(1, &album);
    entry.track.local_path = Some(entry.facts.path.to_string_lossy().into_owned());
    let first_generation = case.start_sync("begin first sync");
    seed_local_manifest(
        &case,
        &case.id,
        first_generation,
        std::slice::from_ref(&entry),
    );
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album],
            tracks: vec![entry.track.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    seed_source_object_mappings(
        &case,
        &case.id,
        &[SourceObjectMapping {
            source_object_id: "external-source-key".to_string(),
            entity_kind: SourceEntityKind::Track,
            entity_id: entry.track.id.as_str().to_string(),
        }],
        first_generation,
    );
    case.connection
        .execute_batch(&format!(
            "
            INSERT INTO entity_identity_keys (
                source_id, entity_kind, namespace, value, entity_id, source, strength
            ) VALUES (
                '{}', 'track', 'external:track', 'external-track-one', '{}', 'musicbrainz', 100
            );
            INSERT INTO entity_facts (
                source_id, entity_kind, entity_id, fact_key, value_json, source, status
            ) VALUES (
                '{}', 'track', '{}', 'external_fact', 'true', 'musicbrainz', 'resolved'
            );
            INSERT INTO entity_links (
                source_id, entity_kind, entity_id, namespace, url, label, source, status
            ) VALUES (
                '{}', 'track', '{}', 'external:track',
                'https://example.invalid/track/external-track-one', NULL,
                'musicbrainz', 'resolved'
            );
            ",
            case.id.as_str(),
            entry.track.id.as_str(),
            case.id.as_str(),
            entry.track.id.as_str(),
            case.id.as_str(),
            entry.track.id.as_str(),
        ))
        .expect("save external entity state");
    case.record_activity_outcome(&ActivityOutcome {
        source_id: case.id.clone(),
        period: "2026-07".to_string(),
        track_id: entry.track.id.clone(),
        qualified_plays: 1,
        skips: 0,
        last_played_at: Some(1_783_850_400),
    })
    .expect("record local play");
    case.set_track_favorite_for_owner(&case.id, &entry.track.id, true, SourceFeatureOwner::Store)
        .expect("save favorite override");

    let second_generation = case.start_sync("begin delete sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            deleted_track_ids: vec![entry.track.id.clone()],
            manifest: LocalManifestDelta {
                deleted_paths: vec![entry.facts.path.clone()],
                ..LocalManifestDelta::default()
            },
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit local deletion");

    assert!(
        case.load_track(&case.id, &entry.track.id)
            .expect("load deleted track")
            .is_none()
    );
    assert!(
        case.load_local_manifest(&case.id)
            .expect("load manifest")
            .is_empty()
    );
    assert_eq!(
        case.source_object_mappings(&case.id, "external-source-key")
            .expect("load source mapping")
            .len(),
        1
    );
    for table in ["entity_identity_keys", "entity_facts", "entity_links"] {
        let count = case
            .connection
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table} WHERE source_id = ?1 AND entity_kind = 'track' AND entity_id = ?2 AND source = 'musicbrainz'"
                ),
                rusqlite::params![case.id.as_str(), entry.track.id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .expect("count external entity state");
        assert_eq!(count, 1, "{table}");
    }
    let durable_rows = case
        .connection
        .query_row(
            "
            SELECT
                (SELECT COUNT(*) FROM track_activity_period WHERE source_id = ?1 AND track_id = ?2) +
                (SELECT COUNT(*) FROM item_favorite_overrides WHERE source_id = ?1 AND item_kind = 'track' AND item_id = ?2)
            ",
            rusqlite::params![case.id.as_str(), entry.track.id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .expect("count durable track state");
    assert_eq!(durable_rows, 2);
    let local_file_rows = case
        .connection
        .query_row(
            "SELECT COUNT(*) FROM source_objects WHERE source_id = ?1 AND source_object_id = ?2",
            rusqlite::params![
                case.id.as_str(),
                local_file_source_object_id("/music", "Album/track.mp3")
            ],
            |row| row.get::<_, i64>(0),
        )
        .expect("count deleted file mapping");
    assert_eq!(local_file_rows, 0);
}

#[test]
fn schema_local_track_observations_preserve_app_owned_state() {
    let case = StoreCase::with_source_id("local:server:favorites");
    let mut album = album(1);
    album.favorite = false;
    let mut changed_track = track(10, &album);
    changed_track.favorite = false;
    let mut metadata_track = track(11, &album);
    metadata_track.favorite = false;
    let mut library_artist = artist(20, None);
    library_artist.id = changed_track.artist_id.clone().expect("track artist id");
    library_artist.name = changed_track.artist.clone();
    library_artist.favorite = false;
    let mut album_artist = artist(21, None);
    album_artist.id = album.artist_id.clone().expect("album artist id");
    album_artist.name = album.artist.clone();
    album_artist.favorite = false;
    let first_generation = case.start_sync("begin first sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![changed_track.clone(), metadata_track.clone()],
            artists: vec![library_artist.clone()],
            album_artists: vec![album_artist.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    let store_playlist = playlist(30, None);
    case.upsert_playlists_with_mode(
        &case.id,
        std::slice::from_ref(&store_playlist),
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist");
    case.upsert_playlist_entries_with_mode(
        &case.id,
        &store_playlist.id,
        &schema_track_test(&store_playlist.id, std::slice::from_ref(&metadata_track)),
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist entries");
    case.record_activity_outcome(&ActivityOutcome {
        source_id: case.id.clone(),
        period: "2026-07".to_string(),
        track_id: metadata_track.id.clone(),
        qualified_plays: 1,
        skips: 0,
        last_played_at: Some(1_783_850_400),
    })
    .expect("record local play");
    case.set_album_favorite_for_owner(&case.id, &album.id, true, SourceFeatureOwner::Store)
        .expect("favorite album override");
    case.set_track_favorite_for_owner(&case.id, &changed_track.id, true, SourceFeatureOwner::Store)
        .expect("favorite changed track override");
    case.set_track_favorite_for_owner(
        &case.id,
        &metadata_track.id,
        true,
        SourceFeatureOwner::Store,
    )
    .expect("favorite metadata track override");
    case.set_artist_favorite_for_owner(
        &case.id,
        &library_artist.id,
        true,
        SourceFeatureOwner::Store,
    )
    .expect("favorite artist override");
    case.set_artist_favorite_for_owner(&case.id, &album_artist.id, true, SourceFeatureOwner::Store)
        .expect("favorite album artist override");

    let mut incoming_album = album.clone();
    incoming_album.favorite = false;
    incoming_album.track_count += 1;
    let mut incoming_changed_track = changed_track.clone();
    incoming_changed_track.favorite = false;
    incoming_changed_track.title = "Changed local title".to_string();
    let mut incoming_metadata_track = metadata_track.clone();
    incoming_metadata_track.favorite = false;
    incoming_metadata_track.duration_seconds += 1;
    let mut new_track = track(12, &album);
    new_track.favorite = false;
    let mut incoming_artist = library_artist.clone();
    incoming_artist.favorite = false;
    incoming_artist.track_count += 1;
    let mut incoming_album_artist = album_artist.clone();
    incoming_album_artist.favorite = false;
    let second_generation = case.start_sync("begin second sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            tracks: vec![
                incoming_changed_track.clone(),
                incoming_metadata_track.clone(),
                new_track.clone(),
            ],
            current_album_ids: vec![album.id.clone()],
            current_artist_ids: vec![library_artist.id.clone()],
            current_album_artist_ids: vec![album_artist.id.clone()],
            dirty_albums: vec![incoming_album.clone()],
            dirty_artists: vec![incoming_artist.clone()],
            dirty_album_artists: vec![incoming_album_artist.clone()],
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit local delta");

    let loaded_changed = case
        .load_track(&case.id, &changed_track.id)
        .expect("load changed track")
        .expect("changed track");
    let loaded_metadata = case
        .load_track(&case.id, &metadata_track.id)
        .expect("load metadata track")
        .expect("metadata track");
    let loaded_new = case
        .load_track(&case.id, &new_track.id)
        .expect("load new track")
        .expect("new track");
    let loaded_albums = case
        .load_albums(&case.id, 0, 10)
        .expect("load albums")
        .items;
    let loaded_artists = case
        .load_artists(&case.id, false, 0, 10)
        .expect("load artists")
        .items;
    let loaded_album_artists = case
        .load_artists(&case.id, true, 0, 10)
        .expect("load album artists")
        .items;
    let loaded_album = loaded_albums
        .iter()
        .find(|candidate| candidate.id == album.id)
        .expect("album");
    let loaded_artist = loaded_artists
        .iter()
        .find(|candidate| candidate.id == library_artist.id)
        .expect("artist");
    let loaded_album_artist = loaded_album_artists
        .iter()
        .find(|candidate| candidate.id == album_artist.id)
        .expect("album artist");

    assert_eq!(loaded_changed.title, incoming_changed_track.title);
    assert!(loaded_changed.favorite);
    assert_eq!(
        loaded_metadata.duration_seconds,
        incoming_metadata_track.duration_seconds
    );
    assert!(loaded_metadata.favorite);
    assert!(!loaded_new.favorite);
    assert_eq!(loaded_album.track_count, incoming_album.track_count);
    assert!(loaded_album.favorite);
    assert_eq!(loaded_artist.track_count, incoming_artist.track_count);
    assert!(loaded_artist.favorite);
    assert_eq!(
        loaded_album_artist.album_count,
        incoming_album_artist.album_count
    );
    assert!(loaded_album_artist.favorite);
    let stored_playlist = case
        .load_playlist_detail(&case.id, &store_playlist.id)
        .expect("load store playlist")
        .expect("store playlist");
    assert_eq!(stored_playlist.entries.len(), 1);
    assert_eq!(stored_playlist.entries[0].track.id, metadata_track.id);
    assert_eq!(
        stored_playlist.playlist.duration_seconds,
        incoming_metadata_track.duration_seconds
    );
    let activity = case
        .track_activity_summary(&case.id, &metadata_track.id)
        .expect("load track activity");
    assert_eq!(activity.qualified_plays, 1);
    assert!(activity.last_played_at.is_some());
}

#[test]
fn artwork_delta_update() {
    let case = StoreCase::open();
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    album.image_ref = Some(image_ref("local:cover:file:album", "cover-one"));
    let mut track = track(1, &album);
    track.local_path = Some("/music/Album/track.mp3".to_string());
    let first_generation = case.start_sync("begin first sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![track.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    let mut updated_album = album.clone();
    updated_album.image_ref = Some(image_ref("local:cover:file:album", "cover-two"));
    let mut artwork_track = track.clone();
    artwork_track.image_ref = updated_album.image_ref.clone();
    let second_generation = case.start_sync("begin artwork sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            tracks: vec![artwork_track],
            current_album_ids: vec![updated_album.id.clone()],
            dirty_albums: vec![updated_album],
            manifest: LocalManifestDelta {
                upserted_entries: vec![local_manifest_entry()],
                ..LocalManifestDelta::default()
            },
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit artwork delta");

    let loaded = case
        .load_track(&case.id, &track.id)
        .expect("load track")
        .expect("track");
    assert_eq!(
        loaded
            .image_ref
            .as_ref()
            .and_then(|image| image.tag.as_deref()),
        Some("cover-two")
    );
}

#[test]
fn schema_update_id() {
    let case = StoreCase::open();
    let mut first_album = album(1);
    first_album.artist = "Primary Artist".to_string();
    first_album.artist_id = Some(ArtistId::fake(10));
    let mut second_album = album(2);
    second_album.title = first_album.title.clone();
    second_album.artist = first_album.artist.clone();
    second_album.artist_id = first_album.artist_id.clone();
    let credited_artist_id = ArtistId::fake(20);
    let mut track = super::test_support::track(1, &first_album);
    track.artist = first_album.artist.clone();
    track.artist_id = first_album.artist_id.clone();
    track.artist_credits = vec![credit(credited_artist_id.clone(), "Featured Artist")];
    track.local_path = Some("/music/Album/track.mp3".to_string());
    let first_generation = case.start_sync("begin first sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![first_album.clone()],
            tracks: vec![track.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    assert_eq!(
        track_artist_link_album_id(&case, &case.id, &track.id, &credited_artist_id),
        first_album.id
    );

    let mut updated_track = track.clone();
    updated_track.album_id = second_album.id.clone();
    let second_generation = case.start_sync("begin album move sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            tracks: vec![updated_track.clone()],
            current_album_ids: vec![second_album.id.clone()],
            dirty_albums: vec![second_album.clone()],
            manifest: LocalManifestDelta {
                upserted_entries: vec![local_manifest_entry()],
                ..LocalManifestDelta::default()
            },
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit changed track delta");

    assert_eq!(
        track_artist_link_album_id(&case, &case.id, &track.id, &credited_artist_id),
        second_album.id
    );
    let detail = case
        .load_artist_detail(&case.id, &credited_artist_id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(
        detail
            .appears_on
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>(),
        vec![second_album.id]
    );
    assert_eq!(
        detail
            .tracks
            .iter()
            .map(|track| track.album_id.clone())
            .collect::<Vec<_>>(),
        vec![updated_track.album_id]
    );
}

fn track_artist_link_album_id(
    store: &Store,
    source_id: &SourceId,
    track_id: &TrackId,
    artist_id: &ArtistId,
) -> AlbumId {
    store
        .connection
        .query_row(
            "
            SELECT album_id
            FROM track_artist_links
            WHERE source_id = ?1 AND track_id = ?2 AND artist_id = ?3
            ",
            rusqlite::params![source_id.as_str(), track_id.as_str(), artist_id.as_str()],
            |row| row.get::<_, String>(0).map(AlbumId::new),
        )
        .expect("track artist link album id")
}

#[test]
fn local_access_status_counts_cached_mapping() {
    let case = StoreCase::open();
    let access = SourceLocalAccess {
        source_id: case.id.clone(),
        root_path: "/home/demo/Music".to_string(),
        path_replace_from: Some("/server/music".to_string()),
        path_replace_to: Some("/home/demo/Music".to_string()),
    };
    case.save_source_local_access(&access)
        .expect("save local access");
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut direct = track(1, &album);
    direct.local_path = Some("/mnt/library/direct.flac".to_string());
    let mut prefix = track(2, &album);
    prefix.local_path = Some("/server/music/Album/prefix.flac".to_string());
    let mut relative = track(3, &album);
    relative.local_path = Some("Album/relative.flac".to_string());
    let mut metadata = track(4, &album);
    metadata.local_path = Some("/server/music/Album/metadata.flac".to_string());
    let unmatched = track(5, &album);
    case.upsert_tracks(
        &case.id,
        &[direct, prefix, relative, metadata.clone(), unmatched],
        generation,
    )
    .expect("upsert tracks");
    case.replace_track_local_matches(
        &case.id,
        &[(
            metadata.id.clone(),
            "/home/demo/Music/Album/metadata.flac".to_string(),
            "metadata".to_string(),
        )],
    )
    .expect("replace local matches");

    let status = case
        .local_access_status_facts(&access)
        .expect("local access status");

    assert_eq!(status.total_track_count, 5);
    assert_eq!(status.direct_match_count, 1);
    assert_eq!(status.prefix_match_count, 3);
    assert_eq!(status.metadata_match_count, 1);
    assert_eq!(status.unmatched_count, 1);
    assert_eq!(
        status.sample_source_path.as_deref(),
        Some("/server/music/Album/metadata.flac")
    );
    assert_eq!(
        status.sample_metadata_path.as_deref(),
        Some("/home/demo/Music/Album/metadata.flac")
    );
}

#[test]
fn track_local_path_round_trips() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.local_path = Some("/home/me/Music/Track 1.flac".to_string());
    track.source_format = Some("flac".to_string());
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    assert_eq!(
        case.track_local_path(&case.id, &track.id)
            .expect("track local path"),
        track.local_path
    );
    assert_eq!(
        case.track_source_format(&case.id, &track.id)
            .expect("track source format"),
        track.source_format
    );
}
#[test]
fn schema_album_prefetch() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    case.upsert_albums(
        &case.id,
        &[album(1), album_with_image(2), album(3)],
        generation,
    )
    .expect("upsert albums");
    let albums = case
        .load_albums_without_image_ref(&case.id, 0, 10)
        .expect("load albums without image ref");
    assert_eq!(
        albums.into_iter().map(|album| album.id).collect::<Vec<_>>(),
        vec![AlbumId::fake(1), AlbumId::fake(3)]
    );
}
#[test]
fn schema_artist_prefetch() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    case.upsert_artists(
        &case.id,
        &[
            artist(1, None),
            artist(2, Some(image_ref("artist-two", "tag-two"))),
        ],
        false,
        generation,
    )
    .expect("upsert artists");
    let artists = case
        .load_artists_without_image_ref(&case.id, false, 0, 10)
        .expect("load artists without image ref");
    assert_eq!(
        artists
            .into_iter()
            .map(|artist| artist.id)
            .collect::<Vec<_>>(),
        vec![ArtistId::fake(1)]
    );
}
#[test]
fn artist_direct_image_ref_round_trips() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album_with_image(1);
    let track = track(1, &album);
    let artist_image = image_ref("artist-one", "artist-tag-one");
    let artist = artist(1, Some(artist_image.clone()));
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    let loaded = case
        .load_artists(&case.id, false, 0, 10)
        .expect("load artists")
        .items
        .remove(0);
    let detail = case
        .load_artist_detail(&case.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(loaded.image_ref, Some(artist_image.clone()));
    assert_eq!(detail.artist.image_ref, Some(artist_image));
}

#[test]
fn album_artist_provider_page_merges_shared_musicbrainz_identity() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let primary_id = ArtistId::new("jellyfin:artist:primary-name");
    let alias_id = ArtistId::new("jellyfin:artist:alias-name");
    let mut primary = artist(92, None);
    primary.id = primary_id.clone();
    primary.name = "Primary Name".to_string();
    primary.musicbrainz_artist_id = Some("mb-artist-shared".to_string());
    let mut alias = artist(93, None);
    alias.id = alias_id.clone();
    alias.name = "Alias Name".to_string();
    alias.musicbrainz_artist_id = Some("mb-artist-shared".to_string());
    let mut album = album(13);
    album.artist = "Alias Name".to_string();
    album.artist_id = Some(alias_id.clone());
    album.album_artist_credits = vec![credit(alias_id.clone(), "Alias Name")];
    let mut track = track(1, &album);
    track.album_artist_credits = album.album_artist_credits.clone();

    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let delta = case
        .upsert_artists_delta(&case.id, &[primary.clone(), alias], true, generation)
        .expect("upsert album artists");
    let loaded = case
        .load_artists(&case.id, true, 0, 10)
        .expect("load album artists");
    let alias_entity_id: String = case
        .connection
        .query_row(
            "
            SELECT entity_id
            FROM entity_identity_keys
            WHERE source_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'source:artist_id'
              AND value = ?2
            ",
            rusqlite::params![case.id.as_str(), alias_id.as_str()],
            |row| row.get(0),
        )
        .expect("alias identity");
    let alias_mbid_key_count: i64 = case
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entity_identity_keys
            WHERE source_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'musicbrainz:artist'
              AND entity_id = ?2
            ",
            rusqlite::params![case.id.as_str(), alias_id.as_str()],
            |row| row.get(0),
        )
        .expect("alias mbid keys");
    let alias_link_count: i64 = case
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM album_artist_links
            WHERE source_id = ?1
              AND artist_id = ?2
            ",
            rusqlite::params![case.id.as_str(), alias_id.as_str()],
            |row| row.get(0),
        )
        .expect("alias links");
    let canonical_link_count: i64 = case
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM album_artist_links
            WHERE source_id = ?1
              AND artist_id = ?2
            ",
            rusqlite::params![case.id.as_str(), primary_id.as_str()],
            |row| row.get(0),
        )
        .expect("canonical links");
    let detail = case
        .load_artist_detail(&case.id, &primary_id)
        .expect("load detail")
        .expect("detail");

    assert_eq!(delta.album_artists.added, vec![primary_id.clone()]);
    assert_eq!(loaded.total, 1);
    assert_eq!(loaded.items[0].id, primary_id);
    assert_eq!(loaded.items[0].name, "Primary Name");
    assert_eq!(alias_entity_id, loaded.items[0].id.as_str());
    assert_eq!(alias_mbid_key_count, 0);
    assert_eq!(alias_link_count, 0);
    assert_eq!(canonical_link_count, 1);
    assert_eq!(
        detail
            .albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>(),
        vec![album.id.clone()]
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album_artist",
            "musicbrainz:artist",
            "mb-artist-shared"
        ),
        1
    );

    let next_generation = case.start_sync("begin no-op sync");
    let album_delta = case
        .upsert_albums_delta(&case.id, std::slice::from_ref(&album), next_generation)
        .expect("upsert same alias album");
    let delta = case
        .upsert_artists_delta(
            &case.id,
            &[primary.clone(), {
                let mut artist = primary.clone();
                artist.id = alias_id;
                artist.name = "Alias Name".to_string();
                artist
            }],
            true,
            next_generation,
        )
        .expect("upsert same album artists");
    assert!(album_delta.albums.links.is_empty());
    assert!(delta.album_artists.is_empty());
}

#[test]
fn album_artist_provider_page_replaces_stale_musicbrainz_identity() {
    let case = StoreCase::open();
    let artist_id = ArtistId::new("jellyfin:artist:stale-mbid");
    let mut artist = artist(94, None);
    artist.id = artist_id;
    artist.name = "Changing Artist".to_string();
    artist.musicbrainz_artist_id = Some("mbid-before".to_string());
    let generation = case.start_sync("begin sync");
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), true, generation)
        .expect("upsert artist with mbid");
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album_artist",
            "musicbrainz:artist",
            "mbid-before"
        ),
        1
    );

    artist.musicbrainz_artist_id = Some("mbid-after".to_string());
    let next_generation = case.start_sync("begin next sync");
    case.upsert_artists(
        &case.id,
        std::slice::from_ref(&artist),
        true,
        next_generation,
    )
    .expect("upsert artist without mbid");

    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album_artist",
            "musicbrainz:artist",
            "mbid-before"
        ),
        0
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album_artist",
            "musicbrainz:artist",
            "mbid-after"
        ),
        1
    );
}

#[test]
fn artist_page_unknown_musicbrainz_id_preserves_credit_identity() {
    let case = StoreCase::open();
    let artist_id = ArtistId::new("local:artist:musicbrainz:credit-page");
    let album = album(14);
    let mut track = track(1, &album);
    track.artist_credits = vec![mbid_credit(
        artist_id.clone(),
        "Credit Page Artist",
        "credit-page-mbid",
    )];
    let generation = case.start_sync("begin sync");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let mut artist = artist(95, None);
    artist.id = artist_id;
    artist.name = "Credit Page Artist".to_string();
    artist.musicbrainz_artist_id = None;
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist page");

    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "artist",
            "musicbrainz:artist",
            "credit-page-mbid"
        ),
        1
    );
}

#[test]
fn schema_replace_local() {
    let case = StoreCase::open();
    let track_id = TrackId::fake(1);
    case.replace_track_local_matches(
        &case.id,
        &[(
            track_id.clone(),
            "/home/me/Music/Track 1.flac".to_string(),
            "metadata".to_string(),
        )],
    )
    .expect("replace local matches");
    case.connection
        .execute(
            "
            UPDATE track_local_matches
            SET updated_at = '2000-01-01 00:00:00'
            WHERE source_id = ?1 AND track_id = ?2
            ",
            rusqlite::params![case.id.as_str(), track_id.as_str()],
        )
        .expect("mark local match timestamp");
    case.replace_track_local_matches(
        &case.id,
        &[(
            track_id.clone(),
            "/home/me/Music/Track 1.flac".to_string(),
            "metadata".to_string(),
        )],
    )
    .expect("replace unchanged local matches");
    let updated_at = case
        .connection
        .query_row(
            "
            SELECT updated_at
            FROM track_local_matches
            WHERE source_id = ?1 AND track_id = ?2
            ",
            rusqlite::params![case.id.as_str(), track_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("local match timestamp");
    assert_eq!(updated_at, "2000-01-01 00:00:00");
    assert_eq!(
        case.track_local_match_path(&case.id, &track_id)
            .expect("match path")
            .as_deref(),
        Some("/home/me/Music/Track 1.flac")
    );
    assert_eq!(
        case.track_local_match_paths(&case.id).expect("match paths"),
        vec![(track_id.clone(), "/home/me/Music/Track 1.flac".to_string())]
    );
    case.replace_track_local_matches(&case.id, &[])
        .expect("clear local matches");
    assert_eq!(
        case.track_local_match_path(&case.id, &track_id)
            .expect("match path"),
        None
    );
    assert!(
        case.track_local_match_paths(&case.id)
            .expect("match paths")
            .is_empty()
    );
}
#[test]
fn schema_track_search() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_music_folders(&case.id, std::slice::from_ref(&folder), generation)
        .expect("upsert folder");
    case.upsert_track_music_folder_memberships(
        &case.id,
        &folder.id,
        std::slice::from_ref(&tracks[1]),
        generation,
    )
    .expect("upsert membership");
    case.set_selected_music_folder_id(&case.id, Some(&folder.id))
        .expect("select folder");
    let page = case.load_tracks(&case.id, 0, 10).expect("load tracks");
    let search = case
        .load_tracks_matching(&case.id, "Track", 0, 10)
        .expect("search tracks");
    let favorites = case.load_favorite_tracks(&case.id).expect("load favorites");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, tracks[1].id);
    assert_eq!(search.total, 1);
    assert_eq!(search.items[0].id, tracks[1].id);
    assert!(favorites.is_empty());
}

#[test]
fn schema_album_search_uses_selected_music_folder() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut outside_album = album(1);
    outside_album.title = "Shared Search Outside".to_string();
    outside_album.genres = vec!["Fallback Tag".to_string()];
    let mut inside_album = album(2);
    inside_album.title = "Shared Search Inside".to_string();
    inside_album.genres = vec!["Fallback Tag".to_string()];
    let albums = vec![outside_album, inside_album.clone()];
    let tracks = vec![track(1, &albums[0]), track(2, &albums[1])];
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    case.upsert_albums(&case.id, &albums, generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_music_folders(&case.id, std::slice::from_ref(&folder), generation)
        .expect("upsert folder");
    case.upsert_track_music_folder_memberships(
        &case.id,
        &folder.id,
        std::slice::from_ref(&tracks[1]),
        generation,
    )
    .expect("upsert membership");
    case.set_selected_music_folder_id(&case.id, Some(&folder.id))
        .expect("select folder");

    let page = case.load_albums(&case.id, 0, 10).expect("load albums");
    let fts_search = case
        .load_albums_matching(&case.id, "Shared Search", 0, 10)
        .expect("search albums with fts");
    let like_search = case
        .load_albums_matching(&case.id, "Fallback Tag", 0, 10)
        .expect("search albums with like");

    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, inside_album.id);
    assert_eq!(fts_search.total, 1);
    assert_eq!(fts_search.items[0].id, inside_album.id);
    assert_eq!(like_search.total, 1);
    assert_eq!(like_search.items[0].id, inside_album.id);
}

#[test]
fn schema_favorite_tracks_are_not_capped() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let tracks = (1..=525)
        .map(|number| {
            let mut track = track(number, &album);
            track.favorite = true;
            track
        })
        .collect::<Vec<_>>();
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");

    let favorites = case.load_favorite_tracks(&case.id).expect("load favorites");

    assert_eq!(track_id_set(&favorites), track_id_set(&tracks));
}

#[test]
fn schema_folder_favorite_tracks_are_not_capped() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    let selected_tracks = (1..=525)
        .map(|number| {
            let mut track = track(number, &album);
            track.favorite = true;
            track
        })
        .collect::<Vec<_>>();
    let mut outside_track = track(900, &album);
    outside_track.favorite = true;
    let mut tracks = selected_tracks.clone();
    tracks.push(outside_track);
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_music_folders(&case.id, std::slice::from_ref(&folder), generation)
        .expect("upsert folder");
    case.upsert_track_music_folder_memberships(&case.id, &folder.id, &selected_tracks, generation)
        .expect("upsert memberships");
    case.set_selected_music_folder_id(&case.id, Some(&folder.id))
        .expect("select folder");

    let favorites = case.load_favorite_tracks(&case.id).expect("load favorites");

    assert_eq!(track_id_set(&favorites), track_id_set(&selected_tracks));
}

fn track_id_set(tracks: &[Track]) -> BTreeSet<String> {
    tracks
        .iter()
        .map(|track| track.id.as_str().to_string())
        .collect()
}

fn mbid_credit(id: ArtistId, name: &str, mbid: &str) -> ArtistCredit {
    ArtistCredit {
        id,
        name: name.to_string(),
        musicbrainz_artist_id: Some(mbid.to_string()),
    }
}

#[test]
fn schema_filter_folder() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_music_folders(&case.id, std::slice::from_ref(&folder), generation)
        .expect("upsert folder");
    case.upsert_track_music_folder_memberships(
        &case.id,
        &folder.id,
        std::slice::from_ref(&tracks[1]),
        generation,
    )
    .expect("upsert membership");
    case.set_selected_music_folder_id(&case.id, Some(&folder.id))
        .expect("select folder");
    let loaded = case
        .load_track(&case.id, &tracks[0].id)
        .expect("load track")
        .expect("track");
    assert_eq!(loaded.id, tracks[0].id);
}
#[test]
fn schema_stale_sync() {
    let case = StoreCase::open();
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    let first_generation = case.start_sync("begin sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            music_folders: vec![(folder.clone(), Vec::new())],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    case.set_selected_music_folder_id(&case.id, Some(&folder.id))
        .expect("select folder");
    let second_generation = case.start_sync("begin next sync");
    case.commit_library(
        second_generation,
        LibraryObservation::default(),
        "commit empty library",
    );
    assert!(
        case.list_music_folders(&case.id)
            .expect("list folders")
            .is_empty()
    );
    assert_eq!(
        case.selected_music_folder_id(&case.id)
            .expect("selected folder"),
        None
    );
}

#[test]
fn schema_store_playlist_survives_native_sync_prune() {
    let case = StoreCase::open();
    let native = playlist(1, None);
    let mut store_owned = playlist(2, None);
    store_owned.owner = Some(SourceFeatureOwner::Store);

    let first_generation = case.start_sync("begin sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            playlists: vec![PlaylistDetail {
                playlist: native,
                tracks: Vec::new(),
                entries: Vec::new(),
            }],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    case.upsert_playlists_with_mode(
        &case.id,
        std::slice::from_ref(&store_owned),
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist");

    let second_generation = case.start_sync("begin next sync");
    case.commit_library(
        second_generation,
        LibraryObservation::default(),
        "commit empty library",
    );

    let playlists = case
        .load_playlists(&case.id, 0, 10)
        .expect("load playlists")
        .items;
    assert_eq!(playlists.len(), 1);
    assert_eq!(playlists[0], store_owned);
    assert_eq!(
        case.playlist_owner(&case.id, &playlists[0].id)
            .expect("playlist owner"),
        Some(SourceFeatureOwner::Store)
    );
}

#[test]
fn schema_store_playlist_rehydrates_entries_for_returning_tracks() {
    let case = StoreCase::open();
    let album = album(1);
    let first = track(1, &album);
    let second = track(2, &album);
    let store_owned = playlist(1, None);
    let entries = schema_track_test(&store_owned.id, &[first.clone(), second.clone()]);
    let first_generation = case.start_sync("begin sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![first.clone(), second.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    case.upsert_playlists_with_mode(
        &case.id,
        std::slice::from_ref(&store_owned),
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist");
    case.upsert_playlist_entries_with_mode(
        &case.id,
        &store_owned.id,
        &entries,
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist entries");

    let second_generation = case.start_sync("begin next sync");
    case.commit_library(
        second_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![first.clone()],
            ..LibraryObservation::default()
        },
        "commit second library",
    );

    let detail = case
        .load_playlist_detail(&case.id, &store_owned.id)
        .expect("load playlist")
        .expect("playlist");
    assert_eq!(detail.entries.len(), 1);
    assert_eq!(detail.entries[0].track.id, first.id);
    assert_eq!(detail.playlist.track_count, 1);
    assert_eq!(detail.playlist.duration_seconds, first.duration_seconds);

    let third_generation = case.start_sync("begin reload sync");
    case.commit_library(
        third_generation,
        LibraryObservation {
            albums: vec![album],
            tracks: vec![first.clone(), second.clone()],
            ..LibraryObservation::default()
        },
        "commit reloaded library",
    );

    let reloaded = case
        .load_playlist_detail(&case.id, &store_owned.id)
        .expect("load reloaded playlist")
        .expect("playlist");
    assert_eq!(reloaded.entries.len(), 2);
    assert_eq!(reloaded.entries[0].track.id, first.id);
    assert_eq!(reloaded.entries[1].track.id, second.id);
    assert_eq!(reloaded.playlist.track_count, 2);
}

#[test]
fn schema_local_delta_preserves_store_playlist_membership_for_returning_tracks() {
    let case = StoreCase::open();
    let album = album(1);
    let first = track(1, &album);
    let second = track(2, &album);
    let store_owned = playlist(1, None);
    let entries = schema_track_test(&store_owned.id, &[first.clone(), second.clone()]);
    let first_generation = case.start_sync("begin sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![first.clone(), second.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    case.upsert_playlists_with_mode(
        &case.id,
        std::slice::from_ref(&store_owned),
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist");
    case.upsert_playlist_entries_with_mode(
        &case.id,
        &store_owned.id,
        &entries,
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist entries");

    let second_generation = case.start_sync("begin local delete sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    let commit = case
        .commit_local_library_delta(
            &case.id,
            second_generation,
            base_cache_revision,
            true,
            LocalLibraryDelta {
                deleted_track_ids: vec![second.id.clone()],
                current_album_ids: vec![album.id.clone()],
                dirty_albums: vec![album.clone()],
                ..LocalLibraryDelta::default()
            },
        )
        .expect("commit local delete");
    assert_eq!(commit.delta.playlists.entries, vec![store_owned.id.clone()]);
    assert_eq!(
        commit.delta.playlists.cover_refs,
        vec![store_owned.id.clone()]
    );

    let detail = case
        .load_playlist_detail(&case.id, &store_owned.id)
        .expect("load playlist")
        .expect("playlist");
    assert_eq!(detail.entries.len(), 1);
    assert_eq!(detail.entries[0].track.id, first.id);
    assert_eq!(detail.playlist.track_count, 1);

    let third_generation = case.start_sync("begin local return sync");
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        third_generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            tracks: vec![second.clone()],
            current_album_ids: vec![album.id.clone()],
            dirty_albums: vec![album.clone()],
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit local return");

    let reloaded = case
        .load_playlist_detail(&case.id, &store_owned.id)
        .expect("load reloaded playlist")
        .expect("playlist");
    assert_eq!(reloaded.entries.len(), 2);
    assert_eq!(reloaded.entries[0].track.id, first.id);
    assert_eq!(reloaded.entries[1].track.id, second.id);
    assert_eq!(reloaded.playlist.track_count, 2);
}

#[test]
fn schema_clear_library_cache_preserves_store_playlist_membership() {
    let case = StoreCase::open();
    let album = album(1);
    let track = track(1, &album);
    let store_owned = playlist(1, None);
    let entries = schema_track_test(&store_owned.id, std::slice::from_ref(&track));
    let first_generation = case.start_sync("begin sync");
    case.commit_library(
        first_generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![track.clone()],
            ..LibraryObservation::default()
        },
        "commit first library",
    );
    case.upsert_playlists_with_mode(
        &case.id,
        std::slice::from_ref(&store_owned),
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist");
    case.upsert_playlist_entries_with_mode(
        &case.id,
        &store_owned.id,
        &entries,
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist entries");

    case.clear_library_cache(&case.id)
        .expect("clear library cache");
    let cleared = case
        .load_playlist_detail(&case.id, &store_owned.id)
        .expect("load cleared playlist")
        .expect("playlist preserved");
    assert!(cleared.entries.is_empty());
    assert_eq!(cleared.playlist.track_count, 0);
    assert_eq!(
        case.playlist_owner(&case.id, &store_owned.id)
            .expect("playlist owner"),
        Some(SourceFeatureOwner::Store)
    );

    let second_generation = case.start_sync("begin reload sync");
    case.commit_library(
        second_generation,
        LibraryObservation {
            albums: vec![album],
            tracks: vec![track.clone()],
            ..LibraryObservation::default()
        },
        "commit reloaded library",
    );
    let reloaded = case
        .load_playlist_detail(&case.id, &store_owned.id)
        .expect("load reloaded playlist")
        .expect("playlist preserved");
    assert_eq!(reloaded.entries.len(), 1);
    assert_eq!(reloaded.entries[0].track.id, track.id);
    assert_eq!(reloaded.playlist.track_count, 1);
}

#[test]
fn schema_native_playlist_upsert_rejects_store_owned_collision() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let store_owned = playlist(1, None);
    case.upsert_playlists_with_mode(
        &case.id,
        std::slice::from_ref(&store_owned),
        PlaylistWriteMode::StoreOwned,
    )
    .expect("upsert store playlist");

    let mut native = store_owned.clone();
    native.name = "Native Collision".to_string();
    let error = case
        .upsert_playlists(&case.id, std::slice::from_ref(&native), generation)
        .expect_err("native upsert should reject store owner collision");

    assert!(matches!(error, StoreError::InvalidPlaylistOwner(_)));
    assert_eq!(
        case.load_playlist_detail(&case.id, &store_owned.id)
            .expect("load playlist")
            .expect("playlist")
            .playlist
            .name,
        store_owned.name
    );
}

#[test]
fn album_release_type_lookup_candidates_skip_cached_and_misses() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut release_group_album = album(1);
    release_group_album.musicbrainz_album_id = Some("release-one".to_string());
    release_group_album.musicbrainz_release_group_id = Some("group-one".to_string());
    let mut release_album = album(2);
    release_album.musicbrainz_album_id = Some("release-two".to_string());
    let mut cached_album = album(3);
    cached_album.musicbrainz_release_group_id = Some("group-three".to_string());
    cached_album.release_types = vec!["album".to_string()];
    let missing_album = album(4);
    case.commit_library(
        generation,
        LibraryObservation {
            albums: vec![
                release_group_album.clone(),
                release_album.clone(),
                cached_album,
                missing_album,
            ],
            ..LibraryObservation::default()
        },
        "commit library",
    );

    let candidates = case
        .load_album_identity_candidates(&case.id, 10)
        .expect("load candidates");
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].album_id, release_group_album.id);
    assert_eq!(candidates[0].identity_key, "release-group:group-one");
    assert_eq!(candidates[1].album_id, release_album.id);
    assert_eq!(candidates[1].identity_key, "release:release-two");

    case.save_album_identity_miss(
        &case.id,
        &release_group_album.id,
        "release-group:group-one",
        "not found",
    )
    .expect("save miss");
    case.update_album_identity_metadata(
        &case.id,
        &release_album.id,
        &["single".to_string()],
        Some(false),
    )
    .expect("update release metadata");

    assert!(
        case.load_album_identity_candidates(&case.id, 10)
            .expect("reload candidates")
            .is_empty()
    );
    let albums = case.load_albums(&case.id, 0, 10).expect("load albums");
    let updated_album = albums
        .items
        .iter()
        .find(|album| album.id == release_album.id)
        .expect("updated album");
    assert_eq!(updated_album.release_types, vec!["single".to_string()]);
    assert_eq!(updated_album.is_compilation, Some(false));
}

#[test]
fn projection_writes_populate_entity_identity_rows() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut album = album(1);
    album.musicbrainz_album_id = Some("release-one".to_string());
    album.musicbrainz_release_group_id = Some("group-one".to_string());
    album.release_types = vec!["album".to_string()];
    album.is_compilation = Some(false);
    album.image_ref = Some(image_ref("album-cover-one", "album-cover-tag"));
    let mut track = track(1, &album);
    track.musicbrainz_recording_id = Some("recording-one".to_string());
    track.musicbrainz_release_track_id = Some("release-track-one".to_string());
    track.artist_credits = vec![ArtistCredit {
        id: ArtistId::new("local:artist:musicbrainz:artist-one"),
        name: "Example Artist".to_string(),
        musicbrainz_artist_id: Some("artist-one".to_string()),
    }];

    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert tracks");

    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album",
            "musicbrainz:release",
            "release-one"
        ),
        1
    );
    assert_eq!(
        grouping_key_count(
            &case,
            &case.id,
            "album",
            "musicbrainz:release_group",
            "group-one"
        ),
        1
    );
    assert_eq!(
        entity_fact_count(&case, &case.id, "album", album.id.as_str(), "release_types"),
        1
    );
    assert_eq!(
        grouping_key_count(
            &case,
            &case.id,
            "track",
            "musicbrainz:recording",
            "recording-one"
        ),
        1
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "track",
            "musicbrainz:release_track",
            "release-track-one"
        ),
        1
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "artist",
            "musicbrainz:artist",
            "artist-one"
        ),
        1
    );
}

#[test]
fn projection_loads_artist_credit_musicbrainz_ids() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album_artist_id = ArtistId::new("local:artist:musicbrainz:album-artist-one");
    let track_artist_id = ArtistId::new("local:artist:musicbrainz:track-artist-one");
    let mut album = album(1);
    album.artist_id = Some(album_artist_id.clone());
    album.album_artist_credits = vec![mbid_credit(
        album_artist_id.clone(),
        "Album Artist",
        "album-artist-one",
    )];
    album.artist_credits = vec![mbid_credit(
        track_artist_id.clone(),
        "Track Artist",
        "track-artist-one",
    )];
    let mut track = track(1, &album);
    track.artist_id = Some(track_artist_id.clone());
    track.artist_credits = album.artist_credits.clone();
    track.album_artist_credits = album.album_artist_credits.clone();

    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");

    let loaded_track = case
        .load_track(&case.id, &track.id)
        .expect("load track")
        .expect("track");
    let loaded_album = case
        .load_albums(&case.id, 0, 10)
        .expect("load albums")
        .items
        .into_iter()
        .find(|loaded| loaded.id == album.id)
        .expect("album");

    assert_eq!(
        loaded_track.artist_credits[0]
            .musicbrainz_artist_id
            .as_deref(),
        Some("track-artist-one")
    );
    assert_eq!(
        loaded_track.album_artist_credits[0]
            .musicbrainz_artist_id
            .as_deref(),
        Some("album-artist-one")
    );
    assert_eq!(
        loaded_album.album_artist_credits[0]
            .musicbrainz_artist_id
            .as_deref(),
        Some("album-artist-one")
    );
}

#[test]
fn projection_preserves_and_replaces_artist_credit_musicbrainz_ids() {
    let case = StoreCase::open();
    let artist_id = ArtistId::new("local:artist:musicbrainz:credit-one");
    let album = album(1);
    let mut track = track(1, &album);
    track.artist_id = Some(artist_id.clone());
    track.artist_credits = vec![mbid_credit(artist_id.clone(), "Credit Artist", "old-mbid")];
    let first_generation = case.start_sync("begin first sync");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), first_generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), first_generation)
        .expect("upsert first track");

    let mut unknown_track = track.clone();
    unknown_track.artist_credits = vec![credit(artist_id.clone(), "Credit Artist")];
    let second_generation = case.start_sync("begin second sync");
    case.upsert_tracks(
        &case.id,
        std::slice::from_ref(&unknown_track),
        second_generation,
    )
    .expect("upsert unknown track");
    let loaded_unknown = case
        .load_track(&case.id, &track.id)
        .expect("load unknown track")
        .expect("track");
    assert_eq!(
        loaded_unknown.artist_credits[0]
            .musicbrainz_artist_id
            .as_deref(),
        Some("old-mbid")
    );

    let mut changed_track = unknown_track.clone();
    changed_track.artist_credits = vec![mbid_credit(artist_id, "Credit Artist", "new-mbid")];
    let third_generation = case.start_sync("begin third sync");
    case.upsert_tracks(
        &case.id,
        std::slice::from_ref(&changed_track),
        third_generation,
    )
    .expect("upsert changed track");
    let loaded_changed = case
        .load_track(&case.id, &track.id)
        .expect("load changed track")
        .expect("track");

    assert_eq!(
        loaded_changed.artist_credits[0]
            .musicbrainz_artist_id
            .as_deref(),
        Some("new-mbid")
    );
    assert_eq!(
        entity_key_count(&case, &case.id, "artist", "musicbrainz:artist", "old-mbid"),
        0
    );
    assert_eq!(
        entity_key_count(&case, &case.id, "artist", "musicbrainz:artist", "new-mbid"),
        1
    );
}

#[test]
fn local_track_observation_refreshes_track_identity_rows() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.local_path = Some("/music/Album/track.mp3".to_string());
    track.musicbrainz_recording_id = Some("recording-old".to_string());
    track.musicbrainz_release_track_id = Some("release-track-old".to_string());
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert tracks");

    let next_generation = case.start_sync("begin next sync");
    track.musicbrainz_recording_id = Some("recording-new".to_string());
    track.musicbrainz_release_track_id = Some("release-track-new".to_string());
    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        next_generation,
        base_cache_revision,
        false,
        LocalLibraryDelta {
            tracks: vec![track.clone()],
            current_album_ids: vec![album.id.clone()],
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit local track observation");

    assert_eq!(
        grouping_key_count(
            &case,
            &case.id,
            "track",
            "musicbrainz:recording",
            "recording-old"
        ),
        0
    );
    assert_eq!(
        grouping_key_count(
            &case,
            &case.id,
            "track",
            "musicbrainz:recording",
            "recording-new"
        ),
        1
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "track",
            "musicbrainz:release_track",
            "release-track-old"
        ),
        0
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "track",
            "musicbrainz:release_track",
            "release-track-new"
        ),
        1
    );
}

#[test]
fn album_identity_change_clears_stale_release_metadata() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut album = album(1);
    album.musicbrainz_album_id = Some("release-old".to_string());
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert albums");
    case.update_album_identity_metadata(&case.id, &album.id, &["single".to_string()], Some(false))
        .expect("save resolved metadata");

    let next_generation = case.start_sync("begin next sync");
    album.musicbrainz_album_id = Some("release-new".to_string());
    album.release_types.clear();
    album.is_compilation = None;
    case.upsert_albums(&case.id, std::slice::from_ref(&album), next_generation)
        .expect("upsert changed album identity");

    let loaded = case
        .load_album_detail(&case.id, &album.id)
        .expect("load album")
        .expect("album")
        .0;
    assert_eq!(loaded.musicbrainz_album_id.as_deref(), Some("release-new"));
    assert!(loaded.release_types.is_empty());
    assert_eq!(loaded.is_compilation, None);
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album",
            "musicbrainz:release",
            "release-old"
        ),
        0
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album",
            "musicbrainz:release",
            "release-new"
        ),
        1
    );
    assert_eq!(
        entity_fact_count(&case, &case.id, "album", album.id.as_str(), "release_types"),
        0
    );
}

#[test]
fn local_delta_commit_writes_cue_track_source_objects() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.local_path = Some("/music/album.flac".to_string());
    let mut manifest_entry = local_manifest_entry();
    manifest_entry.track = track.clone();
    manifest_entry.facts.path = PathBuf::from("/music/album.cue#track=01");
    manifest_entry.facts.relative_path = "album.cue#track=01".to_string();
    let cue_source = LocalCueTrackSource {
        source_object_id: "local:cue:track:1".to_string(),
        track_id: track.id.clone(),
        source_path: "/music/album.flac".to_string(),
        root_path: "/music".to_string(),
        relative_path: "album.flac".to_string(),
        cue_path: "/music/album.cue".to_string(),
        cue_revision: "cue-revision-one".to_string(),
        cue_track_index: 1,
        segment_start_ms: 12345,
        segment_end_ms: 67890,
        sync_generation: generation,
    };

    let base_cache_revision = case
        .source_cache_revision(&case.id)
        .expect("cache revision");
    case.commit_local_library_delta(
        &case.id,
        generation,
        base_cache_revision,
        true,
        LocalLibraryDelta {
            tracks: vec![track.clone()],
            current_album_ids: vec![album.id.clone()],
            dirty_albums: vec![album],
            manifest: LocalManifestDelta {
                upserted_entries: vec![manifest_entry],
                ..LocalManifestDelta::default()
            },
            cue_track_sources: vec![cue_source.clone()],
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit cue delta");

    let source = case
        .load_track_source_object(&case.id, &track.id)
        .expect("load track source object")
        .expect("source object");
    assert_eq!(source.source_object_kind, "cue_track");
    assert_eq!(source.source_object_id, cue_source.source_object_id);
    assert_eq!(source.source_path.as_deref(), Some("/music/album.flac"));
    assert_eq!(source.segment_start_ms, Some(12345));
    assert_eq!(source.segment_end_ms, Some(67890));
}

#[test]
fn schema_trip_model() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let artist = artist(1, Some(image_ref("artist-one", "artist-tag")));
    let genre = genre(1, Some(image_ref("genre-one", "genre-tag")));
    let mut album = album_with_image(1);
    album.genres = vec![genre.name.clone()];
    let track = track(1, &album);
    let playlist = playlist(1, Some(image_ref("playlist-one", "playlist-tag")));
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    case.upsert_genres(&case.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    case.upsert_playlists(&case.id, std::slice::from_ref(&playlist), generation)
        .expect("upsert playlist");
    assert_eq!(
        case.load_albums(&case.id, 0, 1).expect("load albums").items[0].image_ref,
        album.image_ref
    );
    assert_eq!(
        case.load_tracks(&case.id, 0, 1).expect("load tracks").items[0].image_ref,
        track.image_ref
    );
    assert_eq!(
        case.load_artists(&case.id, false, 0, 1)
            .expect("load artists")
            .items[0]
            .image_ref,
        artist.image_ref
    );
    assert_eq!(
        case.load_genres(&case.id, 0, 1).expect("load genres").items[0].image_ref,
        genre.image_ref
    );
    assert_eq!(
        case.load_playlists(&case.id, 0, 1)
            .expect("load playlists")
            .items[0]
            .image_ref,
        playlist.image_ref
    );
}

#[test]
fn collection_album_artwork_is_ordered_and_live() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut albums = (1..=5).map(album_with_image).collect::<Vec<_>>();
    let genre = genre(1, None);
    for album in &mut albums {
        album.genres = vec![genre.name.clone()];
    }
    let tracks = albums
        .iter()
        .enumerate()
        .map(|(index, album)| track(index as u32 + 1, album))
        .collect::<Vec<_>>();
    let playlist = playlist(1, None);
    let entries = schema_track_test(
        &playlist.id,
        &[
            tracks[0].clone(),
            tracks[0].clone(),
            tracks[1].clone(),
            tracks[2].clone(),
            tracks[3].clone(),
        ],
    );
    case.commit_library(
        generation,
        LibraryObservation {
            albums: albums.clone(),
            tracks: tracks.clone(),
            genres: vec![genre],
            playlists: vec![PlaylistDetail {
                playlist,
                tracks: entries.iter().map(|entry| entry.track.clone()).collect(),
                entries,
            }],
            ..LibraryObservation::default()
        },
        "commit library",
    );

    let genre_page = case.load_genres(&case.id, 0, 20).expect("load genres");
    let playlist_page = case
        .load_playlists(&case.id, 0, 20)
        .expect("load playlists");
    assert_eq!(
        genre_page.items[0]
            .representative_albums
            .iter()
            .filter_map(|album| album.image_ref.clone())
            .collect::<Vec<_>>(),
        albums
            .iter()
            .filter_map(|album| album.image_ref.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        playlist_page.items[0]
            .representative_albums
            .iter()
            .filter_map(|album| album.image_ref.clone())
            .collect::<Vec<_>>(),
        vec![
            tracks[0].image_ref.clone().expect("first cover"),
            tracks[1].image_ref.clone().expect("second cover"),
            tracks[2].image_ref.clone().expect("third cover"),
            tracks[3].image_ref.clone().expect("fourth cover"),
        ]
    );

    let mut changed_album = albums[0].clone();
    changed_album.image_ref = Some(image_ref("changed-cover", "changed-tag"));
    case.upsert_albums(&case.id, std::slice::from_ref(&changed_album), generation)
        .expect("change album");
    let updated = case.load_genres(&case.id, 0, 20).expect("reload genres");
    let mut expected = genre_page.items[0]
        .representative_albums
        .iter()
        .filter_map(|album| album.image_ref.clone())
        .collect::<Vec<_>>();
    expected[0] = changed_album.image_ref.expect("changed cover");
    assert_eq!(
        updated.items[0]
            .representative_albums
            .iter()
            .filter_map(|album| album.image_ref.clone())
            .collect::<Vec<_>>(),
        expected,
        "relationship artwork should be derived from current facts"
    );
}

#[test]
fn coverless_album_relationships_keep_external_identity_inputs() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut album = album(1);
    album.genres = vec!["Ambient".to_string()];
    album.musicbrainz_album_id = Some("11111111-1111-1111-1111-111111111111".to_string());
    album.musicbrainz_release_group_id = Some("22222222-2222-2222-2222-222222222222".to_string());
    let mut track = track(1, &album);
    track.moods = vec!["Focus".to_string()];
    track.play_count = Some(1);
    let artist = artist(1, None);
    let mut genre = genre(1, None);
    genre.name = album.genres[0].clone();
    let playlist = playlist(1, None);
    let entries = schema_track_test(&playlist.id, std::slice::from_ref(&track));
    case.commit_library(
        generation,
        LibraryObservation {
            albums: vec![album.clone()],
            tracks: vec![track.clone()],
            artists: vec![artist],
            genres: vec![genre],
            playlists: vec![PlaylistDetail {
                playlist,
                tracks: vec![track],
                entries,
            }],
            ..LibraryObservation::default()
        },
        "commit library",
    );

    let loaded_track = case
        .load_track(&case.id, &TrackId::fake(1))
        .expect("load track")
        .expect("track");
    let loaded_album = loaded_track.album_artwork.expect("album artwork facts");
    assert_eq!(loaded_album.image_ref, None);
    assert_eq!(loaded_album.title, album.title);
    assert_eq!(loaded_album.artist, album.artist);
    assert_eq!(
        loaded_album.musicbrainz_album_id,
        album.musicbrainz_album_id
    );
    assert_eq!(
        loaded_album.musicbrainz_release_group_id,
        album.musicbrainz_release_group_id
    );

    let artist_page = case
        .load_artists(&case.id, false, 0, 10)
        .expect("load artists");
    let genre_page = case.load_genres(&case.id, 0, 10).expect("load genres");
    let mood_page = case.load_moods(&case.id, 0, 10).expect("load moods");
    let playlist_page = case
        .load_playlists(&case.id, 0, 10)
        .expect("load playlists");
    let smart_page = case
        .load_smart_playlists(&case.id, 0, 10)
        .expect("load smart playlists");
    let smart = smart_page
        .items
        .iter()
        .find(|playlist| playlist.builtin == Some(SmartPlaylistBuiltin::MostPlayed))
        .expect("most played");

    for representatives in [
        artist_page.items[0].representative_albums.as_slice(),
        genre_page.items[0].representative_albums.as_slice(),
        mood_page.items[0].representative_albums.as_slice(),
        playlist_page.items[0].representative_albums.as_slice(),
        smart.representative_albums.as_slice(),
    ] {
        assert_eq!(representatives.len(), 1);
        assert_eq!(representatives[0].id, album.id);
        assert_eq!(representatives[0].image_ref, None);
        assert_eq!(
            representatives[0].musicbrainz_album_id,
            album.musicbrainz_album_id
        );
        assert_eq!(
            representatives[0].musicbrainz_release_group_id,
            album.musicbrainz_release_group_id
        );
    }
}

fn schema_track_test(playlist_id: &PlaylistId, tracks: &[Track]) -> Vec<PlaylistEntry> {
    tracks
        .iter()
        .enumerate()
        .map(|(position, track)| PlaylistEntry {
            entry_id: format!("{}:{position}", playlist_id.as_str()),
            track: track.clone(),
        })
        .collect()
}
#[test]
fn canonical_album_art_is_derived_without_persisting_fallback() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let fallback_image = image_ref("album-track-cover", "album-track-tag");
    let mut first_track = track(1, &album);
    first_track.image_ref = Some(fallback_image.clone());
    let mut second_track = track(2, &album);
    second_track.image_ref = Some(image_ref("later-track-cover", "later-track-tag"));
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(
        &case.id,
        &[first_track.clone(), second_track.clone()],
        generation,
    )
    .expect("upsert tracks");

    let albums = case.load_albums(&case.id, 0, 25).expect("load albums");
    let detail = case
        .load_album_detail(&case.id, &album.id)
        .expect("load detail")
        .expect("detail");

    assert_eq!(
        case.load_raw_album_image_refs(&case.id)
            .expect("load physical album art")
            .get(&album.id),
        Some(&None)
    );
    assert_eq!(albums.items[0].image_ref.as_ref(), Some(&fallback_image));
    assert_eq!(detail.0.image_ref.as_ref(), Some(&fallback_image));
    assert_eq!(
        detail
            .1
            .iter()
            .map(|track| track.image_ref.clone())
            .collect::<Vec<_>>(),
        vec![first_track.image_ref, second_track.image_ref]
    );
    assert!(detail.1.iter().all(|track| {
        track
            .album_artwork
            .as_ref()
            .and_then(|artwork| artwork.image_ref.as_ref())
            == Some(&fallback_image)
    }));
    let serialized = serde_json::to_value(&detail.1[0]).expect("serialize hydrated track");
    assert!(serialized.get("album_artwork").is_none());
}

#[test]
fn canonical_album_art_uses_related_source_artist_without_persisting_it() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let artist_image = image_ref("artist-folder-cover", "artist-folder-tag");
    let artist = artist(1, Some(artist_image.clone()));
    case.upsert_artists(&case.id, &[artist], false, generation)
        .expect("upsert artist");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");

    let projected = case.load_albums(&case.id, 0, 1).expect("load albums");

    assert_eq!(
        case.load_raw_album_image_refs(&case.id)
            .expect("load physical album art")
            .get(&album.id),
        Some(&None)
    );
    assert_eq!(projected.items[0].image_ref, Some(artist_image));
}

#[test]
fn paged_read_return() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let albums = (1..=505).map(album).collect::<Vec<_>>();
    let tracks = (1..=1005)
        .map(|number| track(number, &albums[(number as usize - 1) % albums.len()]))
        .collect::<Vec<_>>();
    case.upsert_albums(&case.id, &albums, generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    let album_page = case
        .load_albums(&case.id, 500, 10)
        .expect("load album page");
    let track_page = case
        .load_tracks(&case.id, 1000, 10)
        .expect("load track page");
    assert_eq!(album_page.total, 505);
    assert_eq!(album_page.items.len(), 5);
    assert_eq!(track_page.total, 1005);
    assert_eq!(track_page.items.len(), 5);
}
#[test]
fn schema_keep_boundaries() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut first_album = album(1);
    first_album.title = "Alpha Album".to_string();
    let mut second_album = album(2);
    second_album.title = "Beta Album".to_string();
    let mut tracks = vec![
        track(1, &second_album),
        track(2, &first_album),
        track(3, &first_album),
        track(4, &second_album),
    ];
    for track in &mut tracks {
        track.title = format!("Needle {}", track.track_number);
    }
    case.upsert_albums(&case.id, &[first_album, second_album], generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");

    let full_page = case
        .load_tracks_sorted(&case.id, TrackSort::Album, false, 0, 10)
        .expect("load full sorted page");
    let first_page = case
        .load_tracks_sorted(&case.id, TrackSort::Album, false, 0, 2)
        .expect("load first sorted page");
    let second_page = case
        .load_tracks_sorted(&case.id, TrackSort::Album, false, 2, 2)
        .expect("load second sorted page");
    let combined_ids = first_page
        .items
        .iter()
        .chain(second_page.items.iter())
        .map(|track| track.id.clone())
        .collect::<Vec<_>>();
    let full_ids = full_page
        .items
        .iter()
        .map(|track| track.id.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        full_ids,
        vec![
            tracks[1].id.clone(),
            tracks[2].id.clone(),
            tracks[0].id.clone(),
            tracks[3].id.clone()
        ]
    );
    assert_eq!(combined_ids, full_ids);

    let search_page = case
        .load_tracks_matching_sorted(&case.id, "Needle", TrackSort::Album, false, 0, 10)
        .expect("load sorted search page");
    assert_eq!(
        search_page
            .items
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        full_ids
    );
}

#[test]
fn in_memory_track_order_matches_store_order_for_every_sort() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin track sort parity sync");
    let mut first_album = album(1);
    first_album.title = "zeta album".to_string();
    first_album.artist = "zeta artist".to_string();
    first_album.album_artist_credits = vec![
        credit(ArtistId::fake(11), "Zulu credit"),
        credit(ArtistId::fake(12), "Alpha credit"),
    ];
    let mut second_album = album(2);
    second_album.title = "Alpha Album".to_string();
    second_album.artist = "Alpha Artist".to_string();
    second_album.album_artist_credits = vec![credit(ArtistId::fake(13), "Beta credit")];
    let mut tracks = vec![
        track(1, &first_album),
        track(2, &first_album),
        track(3, &second_album),
        track(4, &second_album),
    ];
    for (index, track) in tracks.iter_mut().enumerate() {
        track.album_artist_credits = if index < 2 {
            first_album.album_artist_credits.clone()
        } else {
            second_album.album_artist_credits.clone()
        };
        track.title = ["zeta", "Alpha", "beta", "Älpha"][index].to_string();
        track.release_date = Some(format!("2026-01-0{}", index + 1));
        track.date_added = Some(format!("2026-02-0{}", 4 - index));
        track.last_played = Some(format!("2026-03-0{}T10:00:00Z", index + 1));
        track.play_count = Some([4, 1, 3, 2][index]);
        track.user_rating = Some([2, 4, 1, 3][index]);
        track.bpm = Some([120, 90, 110, 100][index]);
        track.genres = match index {
            0 => vec!["Zulu".to_string(), "alpha".to_string()],
            1 => vec!["Beta".to_string()],
            2 => vec!["delta".to_string(), "Charlie".to_string()],
            _ => vec!["Echo".to_string()],
        };
    }
    case.upsert_albums(
        &case.id,
        &[first_album.clone(), second_album.clone()],
        generation,
    )
    .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    let hydrated = case
        .load_tracks(&case.id, 0, 10)
        .expect("load hydrated tracks")
        .items;
    let sorts = [
        TrackSort::Title,
        TrackSort::TrackNumber,
        TrackSort::Artist,
        TrackSort::AlbumArtist,
        TrackSort::Album,
        TrackSort::Year,
        TrackSort::ReleaseDate,
        TrackSort::DateAdded,
        TrackSort::LastPlayed,
        TrackSort::PlayCount,
        TrackSort::UserRating,
        TrackSort::Genre,
        TrackSort::Bpm,
        TrackSort::Duration,
        TrackSort::Favorite,
    ];

    for sort in sorts {
        for descending in [false, true] {
            let actual = case
                .load_tracks_sorted(&case.id, sort, descending, 0, 10)
                .expect("load Store-sorted tracks")
                .items;
            let mut expected = hydrated.clone();
            expected.sort_by(|left, right| compare_tracks(left, right, sort, descending));
            assert_eq!(
                actual
                    .iter()
                    .map(|track| track.id.clone())
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|track| track.id.clone())
                    .collect::<Vec<_>>(),
                "Store/in-memory mismatch for {sort:?}, descending={descending}"
            );
        }
    }
}

#[test]
fn bpm_sort_projects_values_with_stable_null_last_order() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin bpm sort sync");
    let album = album(1);
    let mut tracks = (1..=4)
        .map(|number| track(number, &album))
        .collect::<Vec<_>>();
    for track in &mut tracks {
        track.title = "Needle!".to_string();
        track.track_number = 1;
    }
    tracks[0].bpm = Some(120);
    tracks[1].bpm = Some(90);
    tracks[2].bpm = None;
    tracks[3].bpm = Some(120);
    case.upsert_albums(&case.id, &[album], generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");

    let assert_order = |page: PagedResponse<Track>, expected: &[usize]| {
        assert_eq!(
            page.items
                .iter()
                .map(|track| (track.id.clone(), track.bpm))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|index| (tracks[*index].id.clone(), tracks[*index].bpm))
                .collect::<Vec<_>>()
        );
    };

    assert_order(
        case.load_tracks_sorted(&case.id, TrackSort::Bpm, false, 0, 10)
            .expect("load BPM-sorted tracks"),
        &[1, 0, 3, 2],
    );
    assert_order(
        case.load_tracks_sorted(&case.id, TrackSort::Bpm, true, 0, 10)
            .expect("load descending BPM-sorted tracks"),
        &[3, 0, 1, 2],
    );
    assert_order(
        case.load_tracks_matching_sorted(&case.id, "Needle", TrackSort::Bpm, false, 0, 10)
            .expect("load FTS BPM-sorted tracks"),
        &[1, 0, 3, 2],
    );
    assert_order(
        case.load_tracks_matching_sorted(&case.id, "!", TrackSort::Bpm, false, 0, 10)
            .expect("load LIKE BPM-sorted tracks"),
        &[1, 0, 3, 2],
    );
}
#[test]
fn paged_search_read() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut albums = (1..=505).map(album).collect::<Vec<_>>();
    albums[504].genres = vec!["Needle Genre".to_string()];
    let tracks = (1..=1005)
        .map(|number| track(number, &albums[(number as usize - 1) % albums.len()]))
        .collect::<Vec<_>>();
    let artists = (1..=505)
        .map(|number| artist(number, None))
        .collect::<Vec<_>>();
    let album_artists = artists.clone();
    let mut genres = (1..=505)
        .map(|number| genre(number, None))
        .collect::<Vec<_>>();
    genres[504].name = "Needle Genre".to_string();
    genres[504].track_count = 1;
    genres[504].duration_seconds = 180;
    let playlists = (1..=505)
        .map(|number| playlist(number, None))
        .collect::<Vec<_>>();
    case.upsert_albums(&case.id, &albums, generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_artists(&case.id, &artists, false, generation)
        .expect("upsert artists");
    case.upsert_artists(&case.id, &album_artists, true, generation)
        .expect("upsert album artists");
    case.upsert_genres(&case.id, &genres, generation)
        .expect("upsert genres");
    case.upsert_playlists(&case.id, &playlists, generation)
        .expect("upsert playlists");
    let album_page = case
        .load_albums_matching(&case.id, "Needle Genre", 0, 10)
        .expect("search albums");
    let track_page = case
        .load_tracks_matching(&case.id, "Track 1005", 0, 10)
        .expect("search tracks");
    let artist_page = case
        .load_artists_matching(&case.id, false, "Artist 505", 0, 10)
        .expect("search artists");
    let album_artist_page = case
        .load_artists_matching(&case.id, true, "Artist 505", 0, 10)
        .expect("search album artists");
    let genre_page = case
        .load_genres_matching(&case.id, "Needle Genre", 0, 10)
        .expect("search genres");
    let genre_ids = case
        .load_genre_ids_by_name(
            &case.id,
            &[
                "needle genre".to_string(),
                genres[0].name.clone(),
                "Missing Genre".to_string(),
            ],
        )
        .expect("resolve exact genre links in one read");
    let playlist_page = case
        .load_playlists_matching(&case.id, "Playlist 505", 0, 10)
        .expect("search playlists");
    assert_eq!(album_page.items, vec![albums[504].clone()]);
    assert_eq!(track_page.items[0].id, tracks[1004].id);
    assert_eq!(artist_page.items, vec![artists[504].clone()]);
    assert_eq!(album_artist_page.items, vec![album_artists[504].clone()]);
    assert_eq!(genre_page.items.len(), 1);
    assert_eq!(genre_page.items[0].id, genres[504].id);
    assert_eq!(genre_page.items[0].name, genres[504].name);
    assert_eq!(
        genre_ids,
        std::collections::HashMap::from([("needle genre".to_string(), genres[504].id.clone(),)])
    );
    assert_eq!(
        genre_page.items[0]
            .representative_albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>(),
        vec![albums[504].id.clone()]
    );
    assert_eq!(playlist_page.items, vec![playlists[504].clone()]);
}
#[test]
fn playlist_detail_stores_ordered_tracks() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let track_one = track(1, &album);
    let track_two = track(2, &album);
    let playlist = playlist(1, None);
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(
        &case.id,
        &[track_one.clone(), track_two.clone()],
        generation,
    )
    .expect("upsert tracks");
    case.upsert_playlists(&case.id, std::slice::from_ref(&playlist), generation)
        .expect("upsert playlist");
    case.upsert_playlist_tracks(
        &case.id,
        &playlist.id,
        &[track_two.clone(), track_one.clone()],
        generation,
    )
    .expect("upsert playlist tracks");
    let detail = case
        .load_playlist_detail(&case.id, &playlist.id)
        .expect("load playlist detail")
        .expect("playlist detail");
    assert_eq!(detail.playlist.id, playlist.id);
    assert_eq!(detail.playlist.name, playlist.name);
    assert_eq!(
        detail
            .playlist
            .representative_albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>(),
        vec![album.id]
    );
    assert_eq!(
        detail
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![track_two.id, track_one.id]
    );
}

#[test]
fn playlist_entries_derive_cached_stats() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut track_one = track(1, &album);
    track_one.duration_seconds = 120;
    track_one.genres = vec!["Rock".to_string(), "Pop".to_string()];
    let mut track_two = track(2, &album);
    track_two.duration_seconds = 210;
    track_two.genres = vec!["Rock".to_string()];
    let mut playlist = playlist(1, None);
    playlist.track_count = 0;
    playlist.duration_seconds = 0;
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(
        &case.id,
        &[track_one.clone(), track_two.clone()],
        generation,
    )
    .expect("upsert tracks");
    case.upsert_playlists(&case.id, std::slice::from_ref(&playlist), generation)
        .expect("upsert playlist");
    let delta = case
        .upsert_playlist_entries_delta(
            &case.id,
            &playlist.id,
            &[
                PlaylistEntryKey {
                    entry_id: "entry-one".to_string(),
                    track_id: track_one.id,
                },
                PlaylistEntryKey {
                    entry_id: "entry-two".to_string(),
                    track_id: track_two.id,
                },
            ],
            generation,
        )
        .expect("upsert entries");

    assert_eq!(delta.playlists.entries, vec![playlist.id.clone()]);
    let page = case
        .load_playlists(&case.id, 0, 10)
        .expect("load playlists");
    assert_eq!(page.items[0].track_count, 2);
    assert_eq!(page.items[0].duration_seconds, 330);
    assert_eq!(page.items[0].top_genres, vec!["Rock", "Pop"]);
    let detail = case
        .load_playlist_detail(&case.id, &playlist.id)
        .expect("load playlist detail")
        .expect("playlist detail");
    assert_eq!(detail.playlist.track_count, 2);
    assert_eq!(detail.playlist.duration_seconds, 330);
    assert_eq!(detail.playlist.top_genres, vec!["Rock", "Pop"]);
}

#[test]
fn track_genre_change_refreshes_playlist_top_genres() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut track_one = track(1, &album);
    track_one.genres = vec!["Rock".to_string()];
    let mut track_two = track(2, &album);
    track_two.genres = vec!["Rock".to_string()];
    let playlist = playlist(1, None);
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(
        &case.id,
        &[track_one.clone(), track_two.clone()],
        generation,
    )
    .expect("upsert tracks");
    case.upsert_playlists(&case.id, std::slice::from_ref(&playlist), generation)
        .expect("upsert playlist");
    case.upsert_playlist_entries_delta(
        &case.id,
        &playlist.id,
        &[
            PlaylistEntryKey {
                entry_id: "entry-one".to_string(),
                track_id: track_one.id,
            },
            PlaylistEntryKey {
                entry_id: "entry-two".to_string(),
                track_id: track_two.id.clone(),
            },
        ],
        generation,
    )
    .expect("upsert entries");

    track_two.genres = vec!["Pop".to_string()];
    let delta = case
        .upsert_tracks_delta(&case.id, std::slice::from_ref(&track_two), generation)
        .expect("update track");

    assert_eq!(delta.playlists.entries, vec![playlist.id.clone()]);
    let detail = case
        .load_playlist_detail(&case.id, &playlist.id)
        .expect("load playlist detail")
        .expect("playlist detail");
    assert_eq!(detail.playlist.top_genres, vec!["Pop", "Rock"]);
}
