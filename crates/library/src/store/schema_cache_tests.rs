use std::{collections::BTreeSet, fs, path::PathBuf};

use super::servers::COLLECTION_COVER_GENRE;
use super::test_support::*;
use crate::{
    CueTrackSourceObject, LocalFileSourceObject, LocalLibraryDelta, StoreError,
    local_file_source_object_id,
};
use domain::{
    AlbumId, ArtistCredit, ArtistId, LocalCueTrackSource, LocalFileFacts, LocalManifestCover,
    LocalManifestCoverKind, LocalManifestEntry, ServerId, TrackId,
};
#[test]
fn current_schema_initializes_empty_database() {
    let store = Store::open_memory().expect("open store");
    assert_eq!(store.schema_version().expect("schema version"), 19);
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
    for table in [
        "source_objects",
        "entities",
        "entity_identity_keys",
        "entity_grouping_keys",
        "entity_facts",
        "entity_resolver_state",
        "entity_content_refs",
        "entity_links",
        "content_cache_entries",
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
    for table in [
        "albums",
        "tracks",
        "artists",
        "album_artists",
        "genres",
        "playlists",
    ] {
        assert!(
            store
                .table_has_column(table, "image_origin")
                .expect("column lookup"),
            "{table}.image_origin should exist"
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
fn schema_create_indexes() {
    let store = Store::open_memory().expect("open store");
    for (table, index) in [
        ("albums", "albums_server_title_nocase_idx"),
        ("albums", "albums_server_artist_idx"),
        ("tracks", "tracks_server_artist_idx"),
        ("artists", "artists_server_name_nocase_idx"),
        ("album_artists", "album_artists_server_name_nocase_idx"),
        ("genres", "genres_server_name_nocase_idx"),
        ("playlists", "playlists_server_name_nocase_idx"),
        ("playlist_tracks", "playlist_tracks_order_idx"),
        ("album_genres", "album_genres_server_genre_idx"),
        ("track_genres", "track_genres_server_genre_idx"),
        ("collection_cover_refs", "collection_cover_refs_lookup_idx"),
        ("album_artist_links", "album_artist_links_server_artist_idx"),
        ("track_artist_links", "track_artist_links_server_artist_idx"),
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

fn entity_key_count(
    store: &Store,
    server_id: &ServerId,
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
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND namespace = ?3
              AND value = ?4
            ",
            rusqlite::params![server_id.as_str(), entity_kind, namespace, value],
            |row| row.get(0),
        )
        .expect("count identity keys")
}

fn grouping_key_count(
    store: &Store,
    server_id: &ServerId,
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
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND namespace = ?3
              AND value = ?4
            ",
            rusqlite::params![server_id.as_str(), entity_kind, namespace, value],
            |row| row.get(0),
        )
        .expect("count grouping keys")
}

fn entity_fact_count(
    store: &Store,
    server_id: &ServerId,
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
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND fact_key = ?4
            ",
            rusqlite::params![server_id.as_str(), entity_kind, entity_id, fact_key],
            |row| row.get(0),
        )
        .expect("count entity facts")
}

fn entity_row_count(
    store: &Store,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entities
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
            ",
            rusqlite::params![server_id.as_str(), entity_kind, entity_id],
            |row| row.get(0),
        )
        .expect("count entities")
}

fn content_ref_count(
    store: &Store,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    content_kind: &str,
) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entity_content_refs
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND content_kind = ?4
            ",
            rusqlite::params![server_id.as_str(), entity_kind, entity_id, content_kind],
            |row| row.get(0),
        )
        .expect("count content refs")
}

fn entity_link_count(
    store: &Store,
    server_id: &ServerId,
    entity_kind: &str,
    entity_id: &str,
    namespace: &str,
) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entity_links
            WHERE server_id = ?1
              AND entity_kind = ?2
              AND entity_id = ?3
              AND namespace = ?4
            ",
            rusqlite::params![server_id.as_str(), entity_kind, entity_id, namespace],
            |row| row.get(0),
        )
        .expect("count entity links")
}

fn selected_image_origin(
    store: &Store,
    server_id: &ServerId,
    table: &str,
    id_column: &str,
    id: &str,
) -> String {
    store
        .connection
        .query_row(
            &format!(
                "
                SELECT image_origin
                FROM {table}
                WHERE server_id = ?1 AND {id_column} = ?2
                "
            ),
            rusqlite::params![server_id.as_str(), id],
            |row| row.get(0),
        )
        .expect("selected image origin")
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
    assert_eq!(store.schema_version().expect("schema version"), 19);
    assert!(store.foreign_keys_enabled().expect("foreign keys"));
    assert!(store.fts5_available().expect("fts5 table"));
    assert!(
        !store
            .table_exists("schema_migrations")
            .expect("table lookup")
    );
    assert!(!store.table_exists("stale_cache").expect("table lookup"));
    assert!(store.table_exists("servers").expect("table lookup"));
    assert!(store.list_servers().expect("list servers").is_empty());
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
    assert_eq!(store.schema_version().expect("schema version"), 19);
    assert!(store.table_exists("tracks").expect("table lookup"));
    assert!(store.list_servers().expect("list servers").is_empty());
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn schema_reopen_servers() {
    let path = std::env::temp_dir().join(format!(
        "library-test-{}-{}.sqlite",
        std::process::id(),
        "preserve-current"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = saved_server();
    {
        let store = Store::open(&path).expect("open store");
        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active server");
    }

    let store = Store::open(&path).expect("reopen store");
    assert_eq!(store.schema_version().expect("schema version"), 19);
    assert_eq!(
        store.list_servers().expect("list servers"),
        vec![saved.clone()]
    );
    assert_eq!(store.active_server().expect("active server"), Some(saved));
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
    let saved = saved_server();
    let genre_name = "Genre 1".to_string();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
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
        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, &[first_track, second_track], generation)
            .expect("upsert tracks");
        store
            .upsert_genres(&saved.server.id, &[cached_genre], generation)
            .expect("upsert genre");
        store
            .complete_sync(&saved.server.id, generation)
            .expect("complete sync");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    connection
        .execute_batch(
            "
                ALTER TABLE genres DROP COLUMN duration_seconds;
                PRAGMA user_version = 18;
                ",
        )
        .expect("simulate previous schema");
    drop(connection);

    let store = Store::open(&path).expect("open upgraded store");
    assert_eq!(store.schema_version().expect("schema version"), 19);
    assert_eq!(
        store.list_servers().expect("list servers"),
        vec![saved.clone()]
    );
    assert_eq!(
        store.active_server().expect("active server"),
        Some(saved.clone())
    );
    assert!(
        store
            .table_has_column("genres", "duration_seconds")
            .expect("column lookup"),
        "genres.duration_seconds should exist after migration"
    );
    let genres = store
        .load_genres(&saved.server.id, 0, 10)
        .expect("load genres")
        .items;
    assert_eq!(genres[0].duration_seconds, 420);
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
    let saved = saved_server();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_server(&saved).expect("save server");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    connection
        .pragma_update(None, "user_version", 17)
        .expect("set unsupported schema version");
    drop(connection);

    let store = Store::open(&path).expect("open reset store");
    assert_eq!(store.schema_version().expect("schema version"), 19);
    assert!(store.list_servers().expect("list servers").is_empty());
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
    let saved = saved_server();
    {
        let store = Store::open(&path).expect("open store");
        store.save_server(&saved).expect("save server");
    }
    let connection = rusqlite::Connection::open(&path).expect("open future connection");
    connection
        .pragma_update(None, "user_version", 20)
        .expect("set future schema version");
    drop(connection);

    let store = Store::open(&path).expect("open reset store");
    assert_eq!(store.schema_version().expect("schema version"), 19);
    assert!(store.list_servers().expect("list servers").is_empty());
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
    assert_eq!(store.busy_timeout_ms().expect("busy timeout"), 5_000);
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
        assert_eq!(store.schema_version().expect("schema version"), 19);
    }
    let store = Store::open_fast_read(&path).expect("open fast read store");
    assert_eq!(store.busy_timeout_ms().expect("busy timeout"), 0);
    assert_eq!(store.schema_version().expect("schema version"), 19);
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn schema_trip_server() {
    let store = Store::open_memory().expect("open store");
    let server_id = ServerId::fake(1);
    let mut queue = QueueEngine::new(server_id.clone());
    queue.append(&track(1, &album(1)));
    store
        .save_queue_snapshot(&queue.snapshot())
        .expect("save queue snapshot");
    assert_eq!(
        store
            .load_queue_snapshot(&server_id)
            .expect("load queue snapshot"),
        Some(queue.snapshot())
    );
    assert_eq!(
        store
            .load_queue_snapshot(&ServerId::fake(2))
            .expect("load queue snapshot"),
        None
    );
}

#[test]
fn queue_progress_updates_saved_current_entry() {
    let store = Store::open_memory().expect("open store");
    let server_id = ServerId::fake(1);
    let mut queue = QueueEngine::new(server_id.clone());
    queue.append(&track(1, &album(1)));
    queue.append(&track(2, &album(2)));
    queue.next_track();
    store
        .save_queue_snapshot(&queue.snapshot())
        .expect("save queue snapshot");

    let current = queue.current().expect("current entry");
    assert!(
        store
            .save_queue_progress(&server_id, &current.id, &current.track_id, 73)
            .expect("save queue progress")
    );

    let saved = store
        .load_queue_snapshot(&server_id)
        .expect("load queue snapshot")
        .expect("saved queue");
    assert_eq!(saved.entries, queue.snapshot().entries);
    assert_eq!(saved.current_index, queue.snapshot().current_index);
    assert_eq!(saved.progress_seconds, 73);

    assert!(
        !store
            .save_queue_progress(
                &server_id,
                &queue.entries()[0].id,
                &queue.entries()[0].track_id,
                99
            )
            .expect("ignore stale queue progress")
    );
    assert_eq!(
        store
            .load_queue_snapshot(&server_id)
            .expect("load queue snapshot")
            .expect("saved queue")
            .progress_seconds,
        73
    );
}

#[test]
fn schema_trip_token() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    store
        .set_active_server(&saved.server.id)
        .expect("set active server");
    assert_eq!(store.active_server().expect("active server"), Some(saved));
}
#[test]
fn schema_load_source() {
    let store = Store::open_memory().expect("open store");
    let playback = saved_server_with_id("server:playback");
    let active = saved_server_with_id("server:active");
    store.save_server(&playback).expect("save playback server");
    store.save_server(&active).expect("save active server");
    store
        .set_active_server(&active.server.id)
        .expect("set active server");

    assert_eq!(
        store
            .saved_server(&playback.server.id)
            .expect("load requested server"),
        Some(playback)
    );
}
#[test]
fn schema_clear_lifecycle() {
    let case = StoreCase::with_server_id("local:server:manifest");
    let entry = local_manifest_entry();

    case.replace_local_manifest(&case.id, 1, std::slice::from_ref(&entry))
        .expect("replace manifest");

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

    case.replace_local_manifest(&case.id, 2, std::slice::from_ref(&entry))
        .expect("replace manifest again");
    case.forget_server(&case.id).expect("forget server");
    assert!(
        case.load_local_manifest(&case.id)
            .expect("load forgotten manifest")
            .is_empty()
    );
}
#[test]
fn schema_track_commit() {
    let case = StoreCase::with_server_id("local:server:rollback");
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
    case.upsert_albums(&case.id, std::slice::from_ref(&album), first_generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &[kept.clone(), removed.clone()], first_generation)
        .expect("upsert tracks");
    case.finish_sync(first_generation, "complete first sync");
    case.replace_local_manifest(
        &case.id,
        first_generation,
        &[kept_entry.clone(), removed_entry],
    )
    .expect("replace manifest");
    let failed_generation = case.start_sync("begin failed sync");
    let mut duplicate_manifest = kept_entry.clone();
    duplicate_manifest.track.id = TrackId::fake(99);
    let error = case.commit_local_library_delta(
        &case.id,
        failed_generation,
        LocalLibraryDelta {
            deleted_track_ids: vec![removed.id.clone()],
            current_track_ids: vec![kept.id.clone()],
            current_album_ids: vec![album.id.clone()],
            dirty_albums: vec![album],
            manifest_entries: vec![kept_entry, duplicate_manifest],
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
fn schema_local_delta_preserves_favorite_flags() {
    let case = StoreCase::with_server_id("local:server:favorites");
    let mut album = album(1);
    album.favorite = true;
    let mut changed_track = track(10, &album);
    changed_track.favorite = true;
    let mut metadata_track = track(11, &album);
    metadata_track.favorite = true;
    let mut library_artist = artist(20, None);
    library_artist.id = ArtistId::new("local:artist:favorites-artist");
    library_artist.favorite = true;
    let mut album_artist = artist(21, None);
    album_artist.id = ArtistId::new("local:album-artist:favorites-artist");
    album_artist.favorite = true;
    let first_generation = case.start_sync("begin first sync");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), first_generation)
        .expect("upsert album");
    case.upsert_tracks(
        &case.id,
        &[changed_track.clone(), metadata_track.clone()],
        first_generation,
    )
    .expect("upsert tracks");
    case.upsert_artists(
        &case.id,
        std::slice::from_ref(&library_artist),
        false,
        first_generation,
    )
    .expect("upsert artist");
    case.upsert_artists(
        &case.id,
        std::slice::from_ref(&album_artist),
        true,
        first_generation,
    )
    .expect("upsert album artist");
    case.finish_sync(first_generation, "complete first sync");

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
    incoming_album_artist.album_count += 1;
    let second_generation = case.start_sync("begin second sync");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        LocalLibraryDelta {
            changed_tracks: vec![incoming_changed_track.clone(), new_track.clone()],
            metadata_tracks: vec![incoming_metadata_track.clone()],
            current_track_ids: vec![
                changed_track.id.clone(),
                metadata_track.id.clone(),
                new_track.id.clone(),
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
    case.upsert_albums(&case.id, std::slice::from_ref(&album), first_generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), first_generation)
        .expect("upsert track");
    case.finish_sync(first_generation, "complete first sync");
    let fts_rowid = library_fts_rowid(&case, &case.id, &track.id);
    let genre_rowid = track_genre_rowid(&case, &case.id, &track.id, "Dream Pop");
    let artist_rowid = track_artist_link_rowid(
        &case,
        &case.id,
        &track.id,
        track.artist_id.as_ref().expect("artist id"),
    );

    let mut updated_album = album.clone();
    updated_album.image_ref = Some(image_ref("local:cover:file:album", "cover-two"));
    let mut artwork_track = track.clone();
    artwork_track.image_ref = updated_album.image_ref.clone();
    let second_generation = case.start_sync("begin artwork sync");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        LocalLibraryDelta {
            artwork_tracks: vec![artwork_track],
            current_track_ids: vec![track.id.clone()],
            current_album_ids: vec![updated_album.id.clone()],
            dirty_albums: vec![updated_album],
            manifest_entries: vec![local_manifest_entry()],
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
    assert_eq!(library_fts_rowid(&case, &case.id, &track.id), fts_rowid);
    assert_eq!(
        track_genre_rowid(&case, &case.id, &track.id, "Dream Pop"),
        genre_rowid
    );
    assert_eq!(
        track_artist_link_rowid(
            &case,
            &case.id,
            &track.id,
            track.artist_id.as_ref().expect("artist id"),
        ),
        artist_rowid
    );
}

#[test]
fn meta_delta_update() {
    let case = StoreCase::open();
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let mut track = super::test_support::track(1, &album);
    track.local_path = Some("/music/Album/track.mp3".to_string());
    let mut retained_track = super::test_support::track(2, &album);
    retained_track.local_path = Some("/music/Album/retained.mp3".to_string());
    let first_generation = case.start_sync("begin first sync");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), first_generation)
        .expect("upsert album");
    case.upsert_tracks(
        &case.id,
        &[track.clone(), retained_track.clone()],
        first_generation,
    )
    .expect("upsert track");
    case.finish_sync(first_generation, "complete first sync");
    let fts_rowid = library_fts_rowid(&case, &case.id, &track.id);
    let genre_rowid = track_genre_rowid(&case, &case.id, &track.id, "Dream Pop");
    let artist_rowid = track_artist_link_rowid(
        &case,
        &case.id,
        &track.id,
        track.artist_id.as_ref().expect("artist id"),
    );
    let retained_track_generation =
        track_table_generation(&case, "tracks", &case.id, &retained_track.id);
    let retained_genre_generation =
        track_table_generation(&case, "track_genres", &case.id, &retained_track.id);
    let retained_artist_generation =
        track_table_generation(&case, "track_artist_links", &case.id, &retained_track.id);

    let mut updated_track = track.clone();
    updated_track.duration_seconds += 1;
    let second_generation = case.start_sync("begin metadata sync");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        LocalLibraryDelta {
            metadata_tracks: vec![updated_track.clone()],
            current_track_ids: vec![track.id.clone(), retained_track.id.clone()],
            current_album_ids: vec![album.id.clone()],
            dirty_albums: vec![album],
            manifest_entries: vec![local_manifest_entry()],
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit metadata delta");

    let loaded = case
        .load_track(&case.id, &track.id)
        .expect("load track")
        .expect("track");
    assert_eq!(loaded.duration_seconds, updated_track.duration_seconds);
    assert_eq!(library_fts_rowid(&case, &case.id, &track.id), fts_rowid);
    assert_eq!(
        track_genre_rowid(&case, &case.id, &track.id, "Dream Pop"),
        genre_rowid
    );
    assert_eq!(
        track_artist_link_rowid(
            &case,
            &case.id,
            &track.id,
            track.artist_id.as_ref().expect("artist id"),
        ),
        artist_rowid
    );
    assert_eq!(
        track_table_generation(&case, "tracks", &case.id, &retained_track.id),
        retained_track_generation
    );
    assert_eq!(
        track_table_generation(&case, "track_genres", &case.id, &retained_track.id),
        retained_genre_generation
    );
    assert_eq!(
        track_table_generation(&case, "track_artist_links", &case.id, &retained_track.id),
        retained_artist_generation
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
    case.upsert_albums(
        &case.id,
        std::slice::from_ref(&first_album),
        first_generation,
    )
    .expect("upsert first album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), first_generation)
        .expect("upsert track");
    case.finish_sync(first_generation, "complete first sync");
    assert_eq!(
        track_artist_link_album_id(&case, &case.id, &track.id, &credited_artist_id),
        first_album.id
    );

    let mut updated_track = track.clone();
    updated_track.album_id = second_album.id.clone();
    let second_generation = case.start_sync("begin album move sync");
    case.commit_local_library_delta(
        &case.id,
        second_generation,
        LocalLibraryDelta {
            changed_tracks: vec![updated_track.clone()],
            current_track_ids: vec![updated_track.id.clone()],
            current_album_ids: vec![second_album.id.clone()],
            dirty_albums: vec![second_album.clone()],
            manifest_entries: vec![local_manifest_entry()],
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

fn library_fts_rowid(store: &Store, server_id: &ServerId, track_id: &TrackId) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT rowid
            FROM library_fts
            WHERE server_id = ?1 AND item_type = 'track' AND item_id = ?2
            ",
            rusqlite::params![server_id.as_str(), track_id.as_str()],
            |row| row.get(0),
        )
        .expect("library fts rowid")
}

fn track_genre_rowid(store: &Store, server_id: &ServerId, track_id: &TrackId, genre: &str) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT rowid
            FROM track_genres
            WHERE server_id = ?1 AND track_id = ?2 AND genre_name = ?3
            ",
            rusqlite::params![server_id.as_str(), track_id.as_str(), genre],
            |row| row.get(0),
        )
        .expect("track genre rowid")
}

fn track_artist_link_rowid(
    store: &Store,
    server_id: &ServerId,
    track_id: &TrackId,
    artist_id: &ArtistId,
) -> i64 {
    store
        .connection
        .query_row(
            "
            SELECT rowid
            FROM track_artist_links
            WHERE server_id = ?1 AND track_id = ?2 AND artist_id = ?3
            ",
            rusqlite::params![server_id.as_str(), track_id.as_str(), artist_id.as_str()],
            |row| row.get(0),
        )
        .expect("track artist link rowid")
}

fn track_artist_link_album_id(
    store: &Store,
    server_id: &ServerId,
    track_id: &TrackId,
    artist_id: &ArtistId,
) -> AlbumId {
    store
        .connection
        .query_row(
            "
            SELECT album_id
            FROM track_artist_links
            WHERE server_id = ?1 AND track_id = ?2 AND artist_id = ?3
            ",
            rusqlite::params![server_id.as_str(), track_id.as_str(), artist_id.as_str()],
            |row| row.get::<_, String>(0).map(AlbumId::new),
        )
        .expect("track artist link album id")
}

fn track_table_generation(
    store: &Store,
    table: &str,
    server_id: &ServerId,
    track_id: &TrackId,
) -> i64 {
    store
        .connection
        .query_row(
            &format!(
                "
                SELECT sync_generation
                FROM {table}
                WHERE server_id = ?1
                  AND track_id = ?2
                LIMIT 1
                "
            ),
            rusqlite::params![server_id.as_str(), track_id.as_str()],
            |row| row.get(0),
        )
        .expect("track table generation")
}

#[test]
fn server_local_access_round_trips() {
    let case = StoreCase::open();
    let access = ServerLocalAccess {
        server_id: case.id.clone(),
        root_path: "/home/me/Music".to_string(),
        path_replace_from: Some("/media/music".to_string()),
        path_replace_to: Some("/home/me/Music".to_string()),
    };
    case.save_server_local_access(&access)
        .expect("save local access");
    assert_eq!(
        case.server_local_access(&case.id)
            .expect("load local access"),
        Some(access)
    );
}

#[test]
fn local_access_status_counts_cached_mapping() {
    let case = StoreCase::open();
    let access = ServerLocalAccess {
        server_id: case.id.clone(),
        root_path: "/home/demo/Music".to_string(),
        path_replace_from: Some("/server/music".to_string()),
        path_replace_to: Some("/home/demo/Music".to_string()),
    };
    case.save_server_local_access(&access)
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
        status.sample_server_path.as_deref(),
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
fn artist_image_use() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album_with_image(1);
    let track = track(1, &album);
    let artist = artist(1, None);
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    case.finish_sync(generation, "complete sync");
    let loaded = case
        .load_artists(&case.id, false, 0, 10)
        .expect("load artists")
        .items
        .remove(0);
    let matching = case
        .load_artists_matching(&case.id, false, "Artist 1", 0, 10)
        .expect("search artists")
        .items
        .remove(0);
    let global_search = case
        .search_library(&case.id, "Artist 1", 10)
        .expect("search library");
    let detail = case
        .load_artist_detail(&case.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(loaded.image_ref, album.image_ref);
    assert_eq!(matching.image_ref, album.image_ref);
    assert_eq!(global_search.artists[0].image_ref, album.image_ref);
    assert_eq!(detail.artist.image_ref, album.image_ref);
}
#[test]
fn album_external_fallback_repairs_to_track_source_ref() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let track_ref = image_ref("track-source-cover", "track-source-tag");
    let mut album = album(1);
    album.image_ref = Some(image_ref(
        "external:mb-release-group:group-one",
        "external-tag-one",
    ));
    let mut track = track(1, &album);
    track.image_ref = Some(track_ref.clone());
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.finish_sync(generation, "complete sync");

    let loaded = case
        .load_albums(&case.id, 0, 10)
        .expect("load albums")
        .items
        .remove(0);

    assert_eq!(loaded.image_ref.as_ref(), Some(&track_ref));
}
#[test]
fn album_source_ref_survives_track_fallback_repair() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album_ref = image_ref("album-source-cover", "album-source-tag");
    let track_ref = image_ref("track-source-cover", "track-source-tag");
    let mut album = album(1);
    album.image_ref = Some(album_ref.clone());
    let mut track = track(1, &album);
    track.image_ref = Some(track_ref);
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.finish_sync(generation, "complete sync");

    let loaded = case
        .load_albums(&case.id, 0, 10)
        .expect("load albums")
        .items
        .remove(0);

    assert_eq!(loaded.image_ref.as_ref(), Some(&album_ref));
}
#[test]
fn artist_source_fallback_wins_over_external_album_ref() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let external_ref = image_ref("external:mb-release-group:group-one", "external-tag-one");
    let mut single = album(1);
    single.title = "Example Single".to_string();
    single.year = 2000;
    single.image_ref = Some(external_ref);
    let mut album = album_with_image(2);
    album.title = "Example Album".to_string();
    album.year = 2001;
    let tracks = vec![track(1, &single), track(2, &album)];
    let artist = artist(1, None);
    case.upsert_albums(&case.id, &[single, album.clone()], generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    case.finish_sync(generation, "complete sync");

    let loaded = case
        .load_artists(&case.id, false, 0, 10)
        .expect("load artists")
        .items
        .remove(0);
    let detail = case
        .load_artist_detail(&case.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");

    assert_eq!(loaded.image_ref, album.image_ref);
    assert_eq!(detail.artist.image_ref, album.image_ref);
}
#[test]
fn artist_external_fallback_repairs_to_source_ref() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let external_ref = image_ref("external:mb-release-group:group-one", "external-tag-one");
    let mut single = album(1);
    single.title = "Example Single".to_string();
    single.year = 2000;
    single.image_ref = Some(external_ref.clone());
    let mut album = album_with_image(2);
    album.title = "Example Album".to_string();
    album.year = 2001;
    let tracks = vec![track(1, &single), track(2, &album)];
    let artist = artist(1, Some(external_ref));
    case.upsert_albums(&case.id, &[single, album.clone()], generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    case.finish_sync(generation, "complete sync");

    let loaded = case
        .load_artists(&case.id, false, 0, 10)
        .expect("load artists")
        .items
        .remove(0);
    let detail = case
        .load_artist_detail(&case.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");

    assert_eq!(loaded.image_ref, album.image_ref);
    assert_eq!(detail.artist.image_ref, album.image_ref);
}
#[test]
fn schema_win_fallback() {
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
fn album_artist_image() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album_artist_id = ArtistId::fake(8);
    let mut album = album_with_image(8);
    album.artist_id = Some(ArtistId::fake(99));
    album.album_artist_credits = vec![credit(album_artist_id.clone(), "Linked Album Artist")];
    let mut album_artist = artist(8, None);
    album_artist.name = "Linked Album Artist".to_string();
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_artists(
        &case.id,
        std::slice::from_ref(&album_artist),
        true,
        generation,
    )
    .expect("upsert album artist");
    case.finish_sync(generation, "complete sync");
    let loaded = case
        .load_artists(&case.id, true, 0, 10)
        .expect("load album artists")
        .items
        .into_iter()
        .find(|artist| artist.id == album_artist_id)
        .expect("album artist");
    let matching = case
        .load_artists_matching(&case.id, true, "Linked Album Artist", 0, 10)
        .expect("search album artists")
        .items
        .remove(0);
    assert_eq!(loaded.image_ref, album.image_ref);
    assert_eq!(matching.image_ref, album.image_ref);
}

#[test]
fn album_artist_provider_page_does_not_merge_label_only_projection() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let linked_id = ArtistId::new("jellyfin:artist:linked-album-artist");
    let page_id = ArtistId::new("jellyfin:artist:provider-page-album-artist");
    let mut album = album_with_image(9);
    album.artist = "Linked Album Artist".to_string();
    album.artist_id = Some(linked_id.clone());
    album.album_artist_credits = vec![credit(linked_id.clone(), "Linked Album Artist")];
    let mut track = track(1, &album);
    track.album_artist_credits = album.album_artist_credits.clone();
    let page_image = image_ref("provider-page-album-artist", "provider-page-tag");
    let mut page_artist = artist(90, Some(page_image.clone()));
    page_artist.id = page_id.clone();
    page_artist.name = "Linked Album Artist".to_string();
    page_artist.favorite = true;
    page_artist.play_count = Some(7);
    page_artist.user_rating = Some(80);

    case.upsert_artists(
        &case.id,
        std::slice::from_ref(&page_artist),
        true,
        generation,
    )
    .expect("seed provider page artist");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.refresh_library_counts(&case.id)
        .expect("refresh counts");
    case.upsert_artists_delta(
        &case.id,
        std::slice::from_ref(&page_artist),
        true,
        generation,
    )
    .expect("upsert provider page artist");
    case.finish_sync(generation, "complete sync");

    let loaded = case
        .load_artists(&case.id, true, 0, 10)
        .expect("load album artists");
    let matching = case
        .load_artists_matching(&case.id, true, "Linked Album Artist", 0, 10)
        .expect("search album artists");
    assert_eq!(loaded.total, 2);
    assert_eq!(matching.total, 2);
    assert!(loaded.items.iter().any(|artist| artist.id == linked_id));
    assert!(loaded.items.iter().any(|artist| artist.id == page_id));

    let next_generation = case.start_sync("begin no-op sync");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), next_generation)
        .expect("upsert same album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), next_generation)
        .expect("upsert same track");
    let delta = case
        .upsert_artists_delta(
            &case.id,
            std::slice::from_ref(&page_artist),
            true,
            next_generation,
        )
        .expect("upsert same provider page artist");
    assert!(delta.album_artists.is_empty());
}

#[test]
fn album_artist_provider_page_does_not_merge_ambiguous_linked_names() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let first_id = ArtistId::new("jellyfin:artist:first-linked");
    let second_id = ArtistId::new("jellyfin:artist:second-linked");
    let page_id = ArtistId::new("jellyfin:artist:ambiguous-page");
    let mut first_album = album(10);
    first_album.artist = "Shared Name".to_string();
    first_album.artist_id = Some(first_id.clone());
    first_album.album_artist_credits = vec![credit(first_id.clone(), "Shared Name")];
    let mut second_album = album(11);
    second_album.artist = "Shared Name".to_string();
    second_album.artist_id = Some(second_id.clone());
    second_album.album_artist_credits = vec![credit(second_id.clone(), "Shared Name")];
    let mut first_track = track(1, &first_album);
    first_track.album_artist_credits = first_album.album_artist_credits.clone();
    let mut second_track = track(2, &second_album);
    second_track.album_artist_credits = second_album.album_artist_credits.clone();
    let mut page_artist = artist(91, None);
    page_artist.id = page_id.clone();
    page_artist.name = "Shared Name".to_string();

    case.upsert_albums(&case.id, &[first_album, second_album], generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &[first_track, second_track], generation)
        .expect("upsert tracks");
    case.refresh_library_counts(&case.id)
        .expect("refresh counts");
    let linked_count: i64 = case
        .connection
        .query_row(
            "
            SELECT COUNT(DISTINCT artist_id)
            FROM album_artist_links
            WHERE server_id = ?1
              AND LOWER(TRIM(name)) = LOWER(TRIM('Shared Name'))
            ",
            rusqlite::params![case.id.as_str()],
            |row| row.get(0),
        )
        .expect("linked count");
    assert_eq!(linked_count, 2);
    case.upsert_artists(
        &case.id,
        std::slice::from_ref(&page_artist),
        true,
        generation,
    )
    .expect("upsert provider page artist");

    let loaded = case
        .load_artists(&case.id, true, 0, 10)
        .expect("load album artists");
    let ids = loaded
        .items
        .iter()
        .filter(|artist| artist.name == "Shared Name")
        .map(|artist| artist.id.clone())
        .collect::<BTreeSet<_>>();

    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&first_id));
    assert!(ids.contains(&second_id));
    assert!(ids.contains(&page_id));
}

#[test]
fn album_artist_provider_page_merges_unique_relation_backed_musicbrainz_split() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let linked_id = ArtistId::new("jellyfin:artist:linked-credit");
    let page_id = ArtistId::new("jellyfin:artist:provider-page");
    let mut album = album(12);
    album.artist = "Credit Artist".to_string();
    album.artist_id = Some(linked_id.clone());
    album.album_artist_credits = vec![credit(linked_id.clone(), "Credit Artist")];
    let mut track = track(1, &album);
    track.album_artist_credits = album.album_artist_credits.clone();
    let mut page_artist = artist(92, None);
    page_artist.id = page_id.clone();
    page_artist.name = "Credit Artist".to_string();
    page_artist.musicbrainz_artist_id = Some("mb-credit-artist".to_string());

    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.refresh_library_counts(&case.id)
        .expect("repair linked album artist");
    let delta = case
        .upsert_artists_delta(
            &case.id,
            std::slice::from_ref(&page_artist),
            true,
            generation,
        )
        .expect("upsert provider page artist");
    let loaded = case
        .load_artists(&case.id, true, 0, 10)
        .expect("load album artists");
    let alias_entity_id: String = case
        .connection
        .query_row(
            "
            SELECT entity_id
            FROM entity_identity_keys
            WHERE server_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'source:artist_id'
              AND value = ?2
            ",
            rusqlite::params![case.id.as_str(), page_id.as_str()],
            |row| row.get(0),
        )
        .expect("alias identity");
    let linked_mbid_count: i64 = case
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM entity_identity_keys
            WHERE server_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'musicbrainz:artist'
              AND entity_id = ?2
              AND value = 'mb-credit-artist'
            ",
            rusqlite::params![case.id.as_str(), linked_id.as_str()],
            |row| row.get(0),
        )
        .expect("linked mbid keys");
    let alias_row_count: i64 = case
        .connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM album_artists
            WHERE server_id = ?1
              AND artist_id = ?2
            ",
            rusqlite::params![case.id.as_str(), page_id.as_str()],
            |row| row.get(0),
        )
        .expect("alias rows");

    assert!(delta.album_artists.is_empty());
    assert_eq!(loaded.total, 1);
    assert_eq!(loaded.items[0].id, linked_id);
    assert_eq!(alias_entity_id, loaded.items[0].id.as_str());
    assert_eq!(linked_mbid_count, 1);
    assert_eq!(alias_row_count, 0);
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
            WHERE server_id = ?1
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
            WHERE server_id = ?1
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
            WHERE server_id = ?1
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
            WHERE server_id = ?1
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
fn album_artist_musicbrainz_fallback_merges_to_provider_artist() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let fallback_id = ArtistId::new("jellyfin:artist:musicbrainz:mb-artist-one");
    let provider_id = ArtistId::new("jellyfin:artist:artist-one");
    let mut album = album(1);
    album.artist = "Example Artist".to_string();
    album.artist_id = Some(fallback_id.clone());
    album.album_artist_credits = vec![credit(fallback_id.clone(), "Example Artist")];

    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");

    let mut artist = artist(1, None);
    artist.id = provider_id.clone();
    artist.name = "Example Artist".to_string();
    artist.musicbrainz_artist_id = Some("mb-artist-one".to_string());
    case.upsert_artists(&case.id, &[artist], true, generation)
        .expect("upsert album artist");
    case.refresh_library_counts(&case.id)
        .expect("refresh counts");

    let detail = case
        .load_album_detail(&case.id, &album.id)
        .expect("load album detail")
        .expect("album detail");

    assert_eq!(detail.0.artist_id.as_ref(), Some(&provider_id));
    assert_eq!(detail.0.album_artist_credits[0].id, provider_id);
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
fn album_artist_repair_splits_compound_credit_when_track_artists_match() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let first_id = ArtistId::new("jellyfin:artist:compound-first");
    let second_id = ArtistId::new("jellyfin:artist:compound-second");
    let compound_id = ArtistId::new("jellyfin:artist:compound-alias");
    let mut album = album(12);
    album.artist = "Primary Artist / Score Artist".to_string();
    album.artist_id = Some(compound_id.clone());
    album.album_artist_credits = vec![credit(compound_id.clone(), "Primary Artist / Score Artist")];
    let mut track = track(1, &album);
    track.artist_credits = vec![
        credit(first_id.clone(), "Primary Artist"),
        credit(second_id.clone(), "Score Artist"),
    ];
    track.album_artist_credits = album.album_artist_credits.clone();

    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.refresh_library_counts(&case.id)
        .expect("refresh counts");

    let loaded = case
        .load_artists(&case.id, true, 0, 10)
        .expect("load album artists");
    let ids = loaded
        .items
        .iter()
        .map(|artist| artist.id.clone())
        .collect::<BTreeSet<_>>();

    assert!(ids.contains(&first_id));
    assert!(ids.contains(&second_id));
    assert!(!ids.contains(&compound_id));
    let first = loaded
        .items
        .iter()
        .find(|artist| artist.id == first_id)
        .expect("first resolved artist");
    assert_eq!(first.name, "Primary Artist");

    let next_generation = case.start_sync("begin no-op sync");
    let delta = case
        .upsert_albums_delta(&case.id, std::slice::from_ref(&album), next_generation)
        .expect("upsert same compound album");
    assert!(delta.albums.links.is_empty());
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
            WHERE server_id = ?1 AND track_id = ?2
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
            WHERE server_id = ?1 AND track_id = ?2
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
    case.upsert_music_folders(&case.id, std::slice::from_ref(&folder), first_generation)
        .expect("upsert folder");
    case.set_selected_music_folder_id(&case.id, Some(&folder.id))
        .expect("select folder");
    case.finish_sync(first_generation, "complete first sync");
    let second_generation = case.start_sync("begin next sync");
    case.finish_sync(second_generation, "complete second sync");
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
fn schema_trip_page() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut album = album(1);
    album.release_types = vec!["album".to_string(), "ep".to_string()];
    album.is_compilation = Some(false);
    album.musicbrainz_album_id = Some("mb-album-one".to_string());
    album.musicbrainz_release_group_id = Some("mb-group-one".to_string());
    let tracks = vec![track(1, &album), track(2, &album)];
    let selected_ref = ImageRef::new(
        "external:mb-release-group:mb-group-one",
        Some("external-v2-46c4966fcc822df3".to_string()),
    );
    let mut expected_album = album.clone();
    expected_album.image_ref = Some(selected_ref.clone());
    let expected_tracks = tracks
        .iter()
        .cloned()
        .map(|mut track| {
            track.image_ref = Some(selected_ref.clone());
            track
        })
        .collect::<Vec<_>>();
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.finish_sync(generation, "complete sync");
    let albums = case.load_albums(&case.id, 0, 25).expect("load albums");
    let detail = case
        .load_album_detail(&case.id, &album.id)
        .expect("load detail")
        .expect("detail");
    assert_eq!(albums.total, 1);
    assert_eq!(albums.items, vec![expected_album.clone()]);
    assert_eq!(detail.0, expected_album);
    assert_eq!(detail.1, expected_tracks);
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
    case.upsert_albums(
        &case.id,
        &[
            release_group_album.clone(),
            release_album.clone(),
            cached_album,
            missing_album,
        ],
        generation,
    )
    .expect("upsert albums");
    case.finish_sync(generation, "complete sync");

    let candidates = case
        .load_album_release_type_lookup_candidates(&case.id, 10)
        .expect("load candidates");
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].album_id, release_group_album.id);
    assert_eq!(candidates[0].lookup_key, "release-group:group-one");
    assert_eq!(candidates[1].album_id, release_album.id);
    assert_eq!(candidates[1].lookup_key, "release:release-two");

    case.save_album_release_type_lookup_miss(
        &case.id,
        &release_group_album.id,
        "release-group:group-one",
        "not found",
    )
    .expect("save miss");
    case.update_album_release_metadata(
        &case.id,
        &release_album.id,
        &["single".to_string()],
        Some(false),
    )
    .expect("update release metadata");

    assert!(
        case.load_album_release_type_lookup_candidates(&case.id, 10)
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
    assert_eq!(
        content_ref_count(&case, &case.id, "album", album.id.as_str(), "cover"),
        1
    );
    assert_eq!(
        content_ref_count(&case, &case.id, "track", track.id.as_str(), "cover"),
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
fn complete_sync_prunes_generic_entity_rows_for_deleted_items() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut album = album(1);
    album.musicbrainz_album_id = Some("release-one".to_string());
    album.musicbrainz_release_group_id = Some("group-one".to_string());
    let mut track = track(1, &album);
    track.musicbrainz_recording_id = Some("recording-one".to_string());
    track.musicbrainz_release_track_id = Some("release-track-one".to_string());
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert tracks");
    case.finish_sync(generation, "complete first sync");
    case.connection
        .execute(
            "
            INSERT INTO entity_links (
                server_id, entity_kind, entity_id, namespace, url, label, source, status
            )
            VALUES (?1, 'album', ?2, 'lastfm:album', ?3, 'Last.fm', 'lastfm', 'resolved')
            ",
            rusqlite::params![
                case.id.as_str(),
                album.id.as_str(),
                "https://www.last.fm/music/Example+Artist/Example+Album"
            ],
        )
        .expect("insert album link");
    assert_eq!(
        entity_link_count(&case, &case.id, "album", album.id.as_str(), "lastfm:album"),
        1
    );

    let next_generation = case.start_sync("begin next sync");
    case.finish_sync(next_generation, "complete empty sync");

    assert_eq!(
        entity_row_count(&case, &case.id, "album", album.id.as_str()),
        0
    );
    assert_eq!(
        entity_row_count(&case, &case.id, "track", track.id.as_str()),
        0
    );
    assert_eq!(
        entity_key_count(
            &case,
            &case.id,
            "album",
            "musicbrainz:release",
            "release-one"
        ),
        0
    );
    assert_eq!(
        grouping_key_count(
            &case,
            &case.id,
            "track",
            "musicbrainz:recording",
            "recording-one"
        ),
        0
    );
    assert_eq!(
        entity_link_count(&case, &case.id, "album", album.id.as_str(), "lastfm:album"),
        0
    );
}

#[test]
fn local_metadata_update_refreshes_track_identity_rows() {
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
    case.update_local_track_metadata_rows(&case.id, std::slice::from_ref(&track), next_generation)
        .expect("update local metadata");

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
fn local_manifest_writes_physical_file_source_objects() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let entry = local_manifest_entry();
    case.upsert_albums(&case.id, std::slice::from_ref(&album(1)), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&entry.track), generation)
        .expect("upsert track");
    case.replace_local_manifest(&case.id, generation, std::slice::from_ref(&entry))
        .expect("replace manifest");

    let source_object_id = local_file_source_object_id("/music", "Album/track.mp3");
    let source = case
        .load_source_object(&case.id, &source_object_id)
        .expect("load source object")
        .expect("source object");
    assert_eq!(source.source_kind, "local_file");
    assert_eq!(source.entity_kind, None);
    assert_eq!(source.entity_id, None);
    assert_eq!(
        source.source_path.as_deref(),
        Some("/music/Album/track.mp3")
    );

    let entity_source_object_id = case
        .connection
        .query_row(
            "
            SELECT source_object_id
            FROM entities
            WHERE server_id = ?1
              AND entity_kind = 'track'
              AND entity_id = ?2
            ",
            rusqlite::params![case.id.as_str(), entry.track.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load entity source object id");
    assert_eq!(entity_source_object_id, source_object_id);
}

#[test]
fn typed_source_object_api_writes_cue_segment_shape() {
    let case = StoreCase::open();
    let parent_id = case
        .upsert_local_file_source_object(
            &case.id,
            &LocalFileSourceObject {
                source_path: "/music/album.flac".to_string(),
                root_path: "/music".to_string(),
                relative_path: "album.flac".to_string(),
                sync_generation: 7,
            },
        )
        .expect("upsert file source object");
    let cue = CueTrackSourceObject {
        source_object_id: "local:cue:track:1".to_string(),
        track_id: TrackId::new("track-cue-1"),
        source_path: "/music/album.flac".to_string(),
        parent_source_object_id: parent_id.clone(),
        cue_path: "/music/album.cue".to_string(),
        cue_revision: "cue-revision-one".to_string(),
        cue_track_index: 1,
        segment_start_ms: 12345,
        segment_end_ms: 67890,
        sync_generation: 7,
    };
    case.upsert_cue_track_source_object(&case.id, &cue)
        .expect("upsert cue source object");

    let file_source = case
        .load_source_object(&case.id, &parent_id)
        .expect("load file source object")
        .expect("file source object");
    assert_eq!(file_source.source_kind, "local_file");
    assert_eq!(file_source.entity_kind, None);
    assert_eq!(file_source.entity_id, None);

    let cue_source = case
        .load_source_object(&case.id, &cue.source_object_id)
        .expect("load cue source object")
        .expect("cue source object");
    assert_eq!(cue_source.source_kind, "cue_track");
    assert_eq!(cue_source.entity_kind.as_deref(), Some("track"));
    assert_eq!(cue_source.entity_id.as_deref(), Some("track-cue-1"));
    assert_eq!(
        cue_source.parent_source_object_id.as_deref(),
        Some(parent_id.as_str())
    );
    assert_eq!(cue_source.cue_path.as_deref(), Some("/music/album.cue"));
    assert_eq!(cue_source.cue_revision.as_deref(), Some("cue-revision-one"));
    assert_eq!(cue_source.cue_track_index, Some(1));
    assert_eq!(cue_source.segment_start_ms, Some(12345));
    assert_eq!(cue_source.segment_end_ms, Some(67890));

    let entity_source_object_id = case
        .connection
        .query_row(
            "
            SELECT source_object_id
            FROM entities
            WHERE server_id = ?1
              AND entity_kind = 'track'
              AND entity_id = ?2
            ",
            rusqlite::params![case.id.as_str(), cue.track_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load cue entity source object id");
    assert_eq!(entity_source_object_id, cue.source_object_id);
}

#[test]
fn local_delta_commit_writes_cue_track_source_objects() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.local_path = Some("/music/album.flac".to_string());
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

    case.commit_local_library_delta(
        &case.id,
        generation,
        LocalLibraryDelta {
            changed_tracks: vec![track.clone()],
            current_track_ids: vec![track.id.clone()],
            current_album_ids: vec![album.id.clone()],
            dirty_albums: vec![album],
            cue_track_sources: vec![cue_source.clone()],
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit cue delta");

    let source = case
        .load_track_source_object(&case.id, &track.id)
        .expect("load track source object")
        .expect("source object");
    assert_eq!(source.source_kind, "cue_track");
    assert_eq!(source.source_object_id, cue_source.source_object_id);
    assert_eq!(source.source_path.as_deref(), Some("/music/album.flac"));
    assert_eq!(source.segment_start_ms, Some(12345));
    assert_eq!(source.segment_end_ms, Some(67890));
}

#[test]
fn local_delta_commit_writes_stress_cue_track_source_objects() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut stress_album = album.clone();
    stress_album.id = AlbumId::new(format!("local:stress-album:1:{}", album.id.as_str()));
    let mut track = track(1, &album);
    track.local_path = Some("/music/album.flac".to_string());
    let mut stress_track = track.clone();
    stress_track.id = TrackId::new(format!("local:stress-track:1:{}", track.id.as_str()));
    stress_track.album_id = stress_album.id.clone();
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

    case.commit_local_library_delta(
        &case.id,
        generation,
        LocalLibraryDelta {
            changed_tracks: vec![track.clone(), stress_track.clone()],
            current_track_ids: vec![track.id.clone(), stress_track.id.clone()],
            current_album_ids: vec![album.id.clone(), stress_album.id.clone()],
            dirty_albums: vec![album, stress_album],
            cue_track_sources: vec![cue_source.clone()],
            ..LocalLibraryDelta::default()
        },
    )
    .expect("commit stress cue delta");

    let source = case
        .load_track_source_object(&case.id, &stress_track.id)
        .expect("load stress track source object")
        .expect("stress source object");
    assert_eq!(source.source_kind, "cue_track");
    assert_eq!(
        source.source_object_id,
        format!(
            "{}\u{1f}stress:{}",
            cue_source.source_object_id,
            stress_track.id.as_str()
        )
    );
    assert_eq!(source.entity_id.as_deref(), Some(stress_track.id.as_str()));
    assert_eq!(source.source_path.as_deref(), Some("/music/album.flac"));
    assert_eq!(source.cue_path.as_deref(), Some("/music/album.cue"));
    assert_eq!(source.segment_start_ms, Some(12345));
    assert_eq!(source.segment_end_ms, Some(67890));
}

#[test]
fn track_source_lookup_prefers_cue_source_over_stale_entity_pointer() {
    let case = StoreCase::open();
    let parent_id = case
        .upsert_local_file_source_object(
            &case.id,
            &LocalFileSourceObject {
                source_path: "/music/album.flac".to_string(),
                root_path: "/music".to_string(),
                relative_path: "album.flac".to_string(),
                sync_generation: 1,
            },
        )
        .expect("upsert backing source");
    let stale_id = case
        .upsert_local_file_source_object(
            &case.id,
            &LocalFileSourceObject {
                source_path: "/music/album.cue#track=01".to_string(),
                root_path: "/music".to_string(),
                relative_path: "album.cue#track=01".to_string(),
                sync_generation: 1,
            },
        )
        .expect("upsert stale source");
    let track_id = TrackId::new("track-cue-1");
    case.upsert_cue_track_source_object(
        &case.id,
        &CueTrackSourceObject {
            source_object_id: "local:cue:track:1".to_string(),
            track_id: track_id.clone(),
            source_path: "/music/album.flac".to_string(),
            parent_source_object_id: parent_id,
            cue_path: "/music/album.cue".to_string(),
            cue_revision: "cue-revision-one".to_string(),
            cue_track_index: 1,
            segment_start_ms: 12345,
            segment_end_ms: 67890,
            sync_generation: 1,
        },
    )
    .expect("upsert cue source");
    case.connection
        .execute(
            "
            UPDATE entities
            SET source_object_id = ?3
            WHERE server_id = ?1
              AND entity_kind = 'track'
              AND entity_id = ?2
            ",
            rusqlite::params![case.id.as_str(), track_id.as_str(), stale_id],
        )
        .expect("reset entity pointer");

    let source = case
        .load_track_source_object(&case.id, &track_id)
        .expect("load source")
        .expect("source");

    assert_eq!(source.source_kind, "cue_track");
    assert_eq!(source.segment_start_ms, Some(12345));
    assert_eq!(source.segment_end_ms, Some(67890));
}

#[test]
fn cue_track_source_object_requires_local_file_parent() {
    let case = StoreCase::open();
    let error = case
        .upsert_cue_track_source_object(
            &case.id,
            &CueTrackSourceObject {
                source_object_id: "local:cue:track:1".to_string(),
                track_id: TrackId::new("track-cue-1"),
                source_path: "/music/album.flac".to_string(),
                parent_source_object_id: "local:file:missing".to_string(),
                cue_path: "/music/album.cue".to_string(),
                cue_revision: "cue-revision-one".to_string(),
                cue_track_index: 1,
                segment_start_ms: 12345,
                segment_end_ms: 67890,
                sync_generation: 7,
            },
        )
        .expect_err("missing parent should fail");
    assert!(matches!(error, StoreError::InvalidSourceObject(_)));
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
fn schema_collection_playlist() {
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
    case.upsert_albums(&case.id, &albums, generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_genres(&case.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    case.upsert_playlists(&case.id, std::slice::from_ref(&playlist), generation)
        .expect("upsert playlist");
    case.upsert_playlist_entries(&case.id, &playlist.id, &entries, generation)
        .expect("upsert playlist entries");
    case.refresh_library_counts(&case.id)
        .expect("refresh counts");
    case.finish_sync(generation, "complete sync");

    let genre_page = case.load_genres(&case.id, 0, 20).expect("load genres");
    let playlist_page = case
        .load_playlists(&case.id, 0, 20)
        .expect("load playlists");
    assert_eq!(
        genre_page.items[0].image_refs,
        albums
            .iter()
            .take(4)
            .filter_map(|album| album.image_ref.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        playlist_page.items[0].image_refs,
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
    let cached_again = case
        .load_genres(&case.id, 0, 20)
        .expect("load cached genres");
    assert_eq!(
        cached_again.items[0].image_refs,
        genre_page.items[0].image_refs
    );
}

#[test]
fn schema_repair_cache() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut albums = (1..=4).map(album_with_image).collect::<Vec<_>>();
    let mut genre = genre(1, None);
    for album in &mut albums {
        album.genres = vec![genre.name.clone()];
    }
    let tracks = albums
        .iter()
        .enumerate()
        .map(|(index, album)| track(index as u32 + 1, album))
        .collect::<Vec<_>>();
    let playlist = playlist(1, Some(image_ref("playlist-cover", "playlist-tag")));

    case.upsert_albums(&case.id, &albums, generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_genres(&case.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    case.upsert_playlists(&case.id, std::slice::from_ref(&playlist), generation)
        .expect("upsert playlist");

    genre.image_refs.clear();
    assert_eq!(
        case.load_genres(&case.id, 0, 20)
            .expect("load stale genres")
            .items[0]
            .image_refs,
        genre.image_refs
    );

    case.ensure_collection_cover_refs(&case.id)
        .expect("ensure cover refs");
    assert_eq!(
        case.load_genres(&case.id, 0, 20)
            .expect("load repaired genres")
            .items[0]
            .image_refs,
        albums
            .iter()
            .take(4)
            .filter_map(|album| album.image_ref.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn schema_repair_genre() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut albums = (1..=8).map(album_with_image).collect::<Vec<_>>();
    let first_genre = genre(1, None);
    let second_genre = genre(2, None);
    for album in &mut albums[..4] {
        album.genres = vec![first_genre.name.clone()];
    }
    for album in &mut albums[4..] {
        album.genres = vec![second_genre.name.clone()];
    }
    let tracks = albums
        .iter()
        .enumerate()
        .map(|(index, album)| track(index as u32 + 1, album))
        .collect::<Vec<_>>();

    case.upsert_albums(&case.id, &albums, generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_genres(
        &case.id,
        &[first_genre.clone(), second_genre.clone()],
        generation,
    )
    .expect("upsert genres");
    case.finish_sync(generation, "complete sync");
    case.connection
        .execute(
            "
            DELETE FROM collection_cover_refs
            WHERE server_id = ?1
              AND collection_type = ?2
              AND collection_id = ?3
            ",
            rusqlite::params![
                case.id.as_str(),
                COLLECTION_COVER_GENRE,
                second_genre.id.as_str()
            ],
        )
        .expect("drop one genre cover cache row");

    let partial = case
        .load_genres(&case.id, 0, 20)
        .expect("load partially cached genres");
    assert!(partial.items[0].image_refs.len() >= 4);
    assert!(
        partial.items[1].image_refs.is_empty(),
        "second genre should simulate an interrupted cover-ref cache"
    );

    case.ensure_collection_cover_refs(&case.id)
        .expect("ensure cover refs");
    let repaired = case
        .load_genres(&case.id, 0, 20)
        .expect("load repaired genres");
    assert_eq!(
        repaired.items[1].image_refs,
        albums[4..]
            .iter()
            .take(4)
            .filter_map(|album| album.image_ref.clone())
            .collect::<Vec<_>>()
    );
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
fn schema_track_missing() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let fallback_image = image_ref("album-track-cover", "album-track-tag");
    let mut first_track = track(1, &album);
    first_track.image_ref = Some(fallback_image.clone());
    let second_track = track(2, &album);
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

    assert_eq!(albums.items[0].image_ref, Some(fallback_image.clone()));
    assert_eq!(detail.0.image_ref, Some(fallback_image));
    assert_eq!(detail.1, vec![first_track, second_track]);
}

#[test]
fn selected_image_origin_marks_source_fallback_and_external_refs() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");

    let source_album = album(1);
    let track_image = image_ref("track-source-cover", "track-source-tag");
    let mut source_track = track(1, &source_album);
    source_track.image_ref = Some(track_image);
    let mut external_album = album(2);
    external_album.musicbrainz_release_group_id =
        Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());

    case.upsert_albums(
        &case.id,
        &[source_album.clone(), external_album.clone()],
        generation,
    )
    .expect("upsert albums");
    case.upsert_tracks(&case.id, &[source_track.clone()], generation)
        .expect("upsert tracks");

    assert_eq!(
        selected_image_origin(
            &case,
            &case.id,
            "tracks",
            "track_id",
            source_track.id.as_str(),
        ),
        "source"
    );

    case.finish_sync(generation, "complete sync");

    assert_eq!(
        selected_image_origin(
            &case,
            &case.id,
            "albums",
            "album_id",
            source_album.id.as_str(),
        ),
        "fallback"
    );
    assert_eq!(
        selected_image_origin(
            &case,
            &case.id,
            "albums",
            "album_id",
            external_album.id.as_str(),
        ),
        "external"
    );
}

#[test]
fn source_artist_image_seeds_album_before_external_identity() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");

    let artist_image = image_ref("artist-source-cover", "artist-source-tag");
    let artist = artist(1, Some(artist_image.clone()));
    let mut album = album(1);
    album.musicbrainz_release_group_id = Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());
    let track = track(1, &album);

    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.finish_sync(generation, "complete sync");

    let detail = case
        .load_album_detail(&case.id, &album.id)
        .expect("load detail")
        .expect("detail");

    assert_eq!(detail.0.image_ref, Some(artist_image.clone()));
    assert_eq!(detail.1[0].image_ref, Some(artist_image));
    assert_eq!(
        selected_image_origin(&case, &case.id, "albums", "album_id", album.id.as_str(),),
        "fallback"
    );
    assert_eq!(
        selected_image_origin(&case, &case.id, "tracks", "track_id", track.id.as_str(),),
        "fallback"
    );
}

#[test]
fn fallback_artist_image_does_not_seed_album_fallback() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");

    let album = album(1);
    let artist = artist(1, None);
    let track = track(1, &album);
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.connection
        .execute(
            "
            UPDATE artists
            SET image_item_id = 'derived-artist-cover',
                image_tag = 'derived-artist-tag',
                image_origin = 'fallback'
            WHERE server_id = ?1 AND artist_id = ?2
            ",
            rusqlite::params![case.id.as_str(), artist.id.as_str()],
        )
        .expect("mark derived artist image");

    case.refresh_library_counts(&case.id)
        .expect("refresh counts");

    let album_image: Option<String> = case
        .connection
        .query_row(
            "
            SELECT image_item_id
            FROM albums
            WHERE server_id = ?1 AND album_id = ?2
            ",
            rusqlite::params![case.id.as_str(), album.id.as_str()],
            |row| row.get(0),
        )
        .expect("album image");

    assert_eq!(album_image, None);
}

#[test]
fn fallback_image_origin_does_not_seed_album_or_artist_fallbacks() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");

    let album = album(1);
    let artist = artist(1, None);
    let track = track(1, &album);
    case.upsert_artists(&case.id, std::slice::from_ref(&artist), false, generation)
        .expect("upsert artist");
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    case.connection
        .execute(
            "
            UPDATE tracks
            SET image_item_id = 'derived-track-cover',
                image_tag = 'derived-track-tag',
                image_origin = 'fallback'
            WHERE server_id = ?1 AND track_id = ?2
            ",
            rusqlite::params![case.id.as_str(), track.id.as_str()],
        )
        .expect("mark derived track image");

    case.refresh_library_counts(&case.id)
        .expect("refresh counts");

    let album_image: Option<String> = case
        .connection
        .query_row(
            "
            SELECT image_item_id
            FROM albums
            WHERE server_id = ?1 AND album_id = ?2
            ",
            rusqlite::params![case.id.as_str(), album.id.as_str()],
            |row| row.get(0),
        )
        .expect("album image");
    let artist_image: Option<String> = case
        .connection
        .query_row(
            "
            SELECT image_item_id
            FROM artists
            WHERE server_id = ?1 AND artist_id = ?2
            ",
            rusqlite::params![case.id.as_str(), artist.id.as_str()],
            |row| row.get(0),
        )
        .expect("artist image");

    assert_eq!(album_image, None);
    assert_eq!(artist_image, None);
    assert_eq!(
        selected_image_origin(&case, &case.id, "albums", "album_id", album.id.as_str(),),
        "unknown"
    );
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
        .load_tracks_sorted(&case.id, LibraryField::Album, false, 0, 10)
        .expect("load full sorted page");
    let first_page = case
        .load_tracks_sorted(&case.id, LibraryField::Album, false, 0, 2)
        .expect("load first sorted page");
    let second_page = case
        .load_tracks_sorted(&case.id, LibraryField::Album, false, 2, 2)
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
        .load_tracks_matching_sorted(&case.id, "Needle", LibraryField::Album, false, 0, 10)
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
    let playlist_page = case
        .load_playlists_matching(&case.id, "Playlist 505", 0, 10)
        .expect("search playlists");
    assert_eq!(album_page.items, vec![albums[504].clone()]);
    assert_eq!(track_page.items, vec![tracks[1004].clone()]);
    assert_eq!(artist_page.items, vec![artists[504].clone()]);
    assert_eq!(album_artist_page.items, vec![album_artists[504].clone()]);
    assert_eq!(genre_page.items, vec![genres[504].clone()]);
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
    assert_eq!(detail.playlist, playlist);
    assert_eq!(detail.tracks, vec![track_two, track_one]);
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
                PlaylistEntry {
                    entry_id: "entry-one".to_string(),
                    track: track_one,
                },
                PlaylistEntry {
                    entry_id: "entry-two".to_string(),
                    track: track_two,
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
            PlaylistEntry {
                entry_id: "entry-one".to_string(),
                track: track_one,
            },
            PlaylistEntry {
                entry_id: "entry-two".to_string(),
                track: track_two.clone(),
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
