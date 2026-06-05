use std::{fs, path::PathBuf};

use super::PRE_SMART_PLAYLISTS_SCHEMA_VERSION;
use super::servers::COLLECTION_COVER_GENRE;
use super::test_support::*;
use crate::LocalLibraryDelta;
use rufin_core::{
    LocalFileFacts, LocalManifestCover, LocalManifestCoverKind, LocalManifestEntry, TrackId,
};
#[test]
fn current_schema_initializes_empty_database() {
    let store = Store::open_memory().expect("open store");
    assert_eq!(store.schema_version().expect("schema version"), 13);
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
#[test]
fn file_store_reset() {
    let path = std::env::temp_dir().join(format!(
        "rufin-store-test-{}-{}.sqlite",
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
    assert_eq!(store.schema_version().expect("schema version"), 13);
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
        "rufin-store-test-{}-{}.sqlite",
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
    assert_eq!(store.schema_version().expect("schema version"), 13);
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
        "rufin-store-test-{}-{}.sqlite",
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
    assert_eq!(store.schema_version().expect("schema version"), 13);
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
        "rufin-store-test-{}-{}.sqlite",
        std::process::id(),
        "v10-upgrade"
    ));
    let _cleanup = fs::remove_file(&path);
    let saved = saved_server();
    {
        let store = Store::open(&path).expect("open current store");
        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active server");
    }
    let connection = rusqlite::Connection::open(&path).expect("open previous connection");
    connection
        .execute_batch(
            "
                DROP TABLE track_activity;
                DROP TABLE smart_playlists;
                DROP TABLE smart_playlist_seed_state;
                ",
        )
        .expect("remove smart playlist schema");
    connection
        .pragma_update(None, "user_version", PRE_SMART_PLAYLISTS_SCHEMA_VERSION)
        .expect("set previous schema version");
    drop(connection);

    let store = Store::open(&path).expect("open upgraded store");
    assert_eq!(store.schema_version().expect("schema version"), 13);
    assert_eq!(
        store.list_servers().expect("list servers"),
        vec![saved.clone()]
    );
    assert_eq!(
        store.active_server().expect("active server"),
        Some(saved.clone())
    );
    assert!(store.table_exists("track_activity").expect("table lookup"));
    assert!(store.table_exists("smart_playlists").expect("table lookup"));
    assert!(
        store
            .table_exists("smart_playlist_seed_state")
            .expect("table lookup")
    );
    assert!(
        store
            .table_exists("collection_cover_refs")
            .expect("table lookup")
    );
    assert!(
        store
            .table_exists("local_file_manifest")
            .expect("table lookup")
    );
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn future_user_version() {
    let path = std::env::temp_dir().join(format!(
        "rufin-store-test-{}-{}.sqlite",
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
        .pragma_update(None, "user_version", 14)
        .expect("set future schema version");
    drop(connection);

    let store = Store::open(&path).expect("open reset store");
    assert_eq!(store.schema_version().expect("schema version"), 13);
    assert!(store.list_servers().expect("list servers").is_empty());
    drop(store);
    let _cleanup = fs::remove_file(&path);
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
    let _cleanup = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
}
#[test]
fn schema_use_mode() {
    let path = std::env::temp_dir().join(format!(
        "rufin-store-test-{}-{}.sqlite",
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
    let store = Store::open_memory().expect("open store");
    let saved = saved_server_with_id("local:server:manifest");
    store.save_server(&saved).expect("save server");
    let entry = local_manifest_entry();

    store
        .replace_local_manifest(&saved.server.id, 1, std::slice::from_ref(&entry))
        .expect("replace manifest");

    assert_eq!(
        store
            .load_local_manifest(&saved.server.id)
            .expect("load manifest"),
        vec![entry.clone()]
    );

    store
        .clear_library_cache(&saved.server.id)
        .expect("clear library cache");
    assert!(
        store
            .load_local_manifest(&saved.server.id)
            .expect("load cleared manifest")
            .is_empty()
    );

    store
        .replace_local_manifest(&saved.server.id, 2, std::slice::from_ref(&entry))
        .expect("replace manifest again");
    store
        .forget_server(&saved.server.id)
        .expect("forget server");
    assert!(
        store
            .load_local_manifest(&saved.server.id)
            .expect("load forgotten manifest")
            .is_empty()
    );
}
#[test]
fn schema_track_commit() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server_with_id("local:server:rollback");
    store.save_server(&saved).expect("save server");
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
    let first_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin first sync");
    store
        .upsert_albums(
            &saved.server.id,
            std::slice::from_ref(&album),
            first_generation,
        )
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.server.id,
            &[kept.clone(), removed.clone()],
            first_generation,
        )
        .expect("upsert tracks");
    store
        .complete_sync(&saved.server.id, first_generation)
        .expect("complete first sync");
    store
        .replace_local_manifest(
            &saved.server.id,
            first_generation,
            &[kept_entry.clone(), removed_entry],
        )
        .expect("replace manifest");
    let failed_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin failed sync");
    let mut duplicate_manifest = kept_entry.clone();
    duplicate_manifest.track.id = TrackId::fake(99);
    let error = store.commit_local_library_delta(
        &saved.server.id,
        failed_generation,
        LocalLibraryDelta {
            deleted_track_ids: vec![removed.id.clone()],
            current_album_ids: vec![album.id.clone()],
            dirty_albums: vec![album],
            manifest_entries: vec![kept_entry, duplicate_manifest],
            ..LocalLibraryDelta::default()
        },
    );

    assert!(error.is_err());
    let tracks = store
        .load_tracks(&saved.server.id, 0, 10)
        .expect("tracks after failed delta");
    assert_eq!(tracks.total, 2);
    assert_eq!(
        store
            .track_local_path(&saved.server.id, &removed.id)
            .expect("removed path after failed delta")
            .as_deref(),
        Some("/music/Album/removed.mp3")
    );
}

#[test]
fn artwork_delta_update() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    album.image_ref = Some(image_ref("local:cover:file:album", "cover-one"));
    let mut track = track(1, &album);
    track.local_path = Some("/music/Album/track.mp3".to_string());
    let first_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin first sync");
    store
        .upsert_albums(
            &saved.server.id,
            std::slice::from_ref(&album),
            first_generation,
        )
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.server.id,
            std::slice::from_ref(&track),
            first_generation,
        )
        .expect("upsert track");
    store
        .complete_sync(&saved.server.id, first_generation)
        .expect("complete first sync");
    let fts_rowid = library_fts_rowid(&store, &saved.server.id, &track.id);
    let genre_rowid = track_genre_rowid(&store, &saved.server.id, &track.id, "Dream Pop");
    let artist_rowid = track_artist_link_rowid(
        &store,
        &saved.server.id,
        &track.id,
        track.artist_id.as_ref().expect("artist id"),
    );

    let mut updated_album = album.clone();
    updated_album.image_ref = Some(image_ref("local:cover:file:album", "cover-two"));
    let mut artwork_track = track.clone();
    artwork_track.image_ref = updated_album.image_ref.clone();
    let second_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin artwork sync");
    store
        .commit_local_library_delta(
            &saved.server.id,
            second_generation,
            LocalLibraryDelta {
                artwork_tracks: vec![artwork_track],
                current_album_ids: vec![updated_album.id.clone()],
                dirty_albums: vec![updated_album],
                manifest_entries: vec![local_manifest_entry()],
                ..LocalLibraryDelta::default()
            },
        )
        .expect("commit artwork delta");

    let loaded = store
        .load_track(&saved.server.id, &track.id)
        .expect("load track")
        .expect("track");
    assert_eq!(
        loaded
            .image_ref
            .as_ref()
            .and_then(|image| image.tag.as_deref()),
        Some("cover-two")
    );
    assert_eq!(
        library_fts_rowid(&store, &saved.server.id, &track.id),
        fts_rowid
    );
    assert_eq!(
        track_genre_rowid(&store, &saved.server.id, &track.id, "Dream Pop"),
        genre_rowid
    );
    assert_eq!(
        track_artist_link_rowid(
            &store,
            &saved.server.id,
            &track.id,
            track.artist_id.as_ref().expect("artist id"),
        ),
        artist_rowid
    );
}

#[test]
fn meta_delta_update() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let mut track = super::test_support::track(1, &album);
    track.local_path = Some("/music/Album/track.mp3".to_string());
    let mut retained_track = super::test_support::track(2, &album);
    retained_track.local_path = Some("/music/Album/retained.mp3".to_string());
    let first_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin first sync");
    store
        .upsert_albums(
            &saved.server.id,
            std::slice::from_ref(&album),
            first_generation,
        )
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.server.id,
            &[track.clone(), retained_track.clone()],
            first_generation,
        )
        .expect("upsert track");
    store
        .complete_sync(&saved.server.id, first_generation)
        .expect("complete first sync");
    let fts_rowid = library_fts_rowid(&store, &saved.server.id, &track.id);
    let genre_rowid = track_genre_rowid(&store, &saved.server.id, &track.id, "Dream Pop");
    let artist_rowid = track_artist_link_rowid(
        &store,
        &saved.server.id,
        &track.id,
        track.artist_id.as_ref().expect("artist id"),
    );
    let retained_track_generation =
        track_table_generation(&store, "tracks", &saved.server.id, &retained_track.id);
    let retained_genre_generation =
        track_table_generation(&store, "track_genres", &saved.server.id, &retained_track.id);
    let retained_artist_generation = track_table_generation(
        &store,
        "track_artist_links",
        &saved.server.id,
        &retained_track.id,
    );

    let mut updated_track = track.clone();
    updated_track.duration_seconds += 1;
    let second_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin metadata sync");
    store
        .commit_local_library_delta(
            &saved.server.id,
            second_generation,
            LocalLibraryDelta {
                metadata_tracks: vec![updated_track.clone()],
                current_album_ids: vec![album.id.clone()],
                dirty_albums: vec![album],
                manifest_entries: vec![local_manifest_entry()],
                ..LocalLibraryDelta::default()
            },
        )
        .expect("commit metadata delta");

    let loaded = store
        .load_track(&saved.server.id, &track.id)
        .expect("load track")
        .expect("track");
    assert_eq!(loaded.duration_seconds, updated_track.duration_seconds);
    assert_eq!(
        library_fts_rowid(&store, &saved.server.id, &track.id),
        fts_rowid
    );
    assert_eq!(
        track_genre_rowid(&store, &saved.server.id, &track.id, "Dream Pop"),
        genre_rowid
    );
    assert_eq!(
        track_artist_link_rowid(
            &store,
            &saved.server.id,
            &track.id,
            track.artist_id.as_ref().expect("artist id"),
        ),
        artist_rowid
    );
    assert_eq!(
        track_table_generation(&store, "tracks", &saved.server.id, &retained_track.id),
        retained_track_generation
    );
    assert_eq!(
        track_table_generation(&store, "track_genres", &saved.server.id, &retained_track.id),
        retained_genre_generation
    );
    assert_eq!(
        track_table_generation(
            &store,
            "track_artist_links",
            &saved.server.id,
            &retained_track.id
        ),
        retained_artist_generation
    );
}

#[test]
fn schema_update_id() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
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
    let first_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin first sync");
    store
        .upsert_albums(
            &saved.server.id,
            std::slice::from_ref(&first_album),
            first_generation,
        )
        .expect("upsert first album");
    store
        .upsert_tracks(
            &saved.server.id,
            std::slice::from_ref(&track),
            first_generation,
        )
        .expect("upsert track");
    store
        .complete_sync(&saved.server.id, first_generation)
        .expect("complete first sync");
    assert_eq!(
        track_artist_link_album_id(&store, &saved.server.id, &track.id, &credited_artist_id),
        first_album.id
    );

    let mut updated_track = track.clone();
    updated_track.album_id = second_album.id.clone();
    let second_generation = store
        .begin_sync(&saved.server.id)
        .expect("begin album move sync");
    store
        .commit_local_library_delta(
            &saved.server.id,
            second_generation,
            LocalLibraryDelta {
                changed_tracks: vec![updated_track.clone()],
                current_album_ids: vec![second_album.id.clone()],
                dirty_albums: vec![second_album.clone()],
                manifest_entries: vec![local_manifest_entry()],
                ..LocalLibraryDelta::default()
            },
        )
        .expect("commit changed track delta");

    assert_eq!(
        track_artist_link_album_id(&store, &saved.server.id, &track.id, &credited_artist_id),
        second_album.id
    );
    let detail = store
        .load_artist_detail(&saved.server.id, &credited_artist_id)
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
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let access = ServerLocalAccess {
        server_id: saved.server.id.clone(),
        root_path: "/home/me/Music".to_string(),
        path_replace_from: Some("/media/music".to_string()),
        path_replace_to: Some("/home/me/Music".to_string()),
    };
    store
        .save_server_local_access(&access)
        .expect("save local access");
    assert_eq!(
        store
            .server_local_access(&saved.server.id)
            .expect("load local access"),
        Some(access)
    );
}
#[test]
fn track_local_path_round_trips() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.local_path = Some("/home/me/Music/Track 1.flac".to_string());
    track.source_format = Some("flac".to_string());
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    assert_eq!(
        store
            .track_local_path(&saved.server.id, &track.id)
            .expect("track local path"),
        track.local_path
    );
    assert_eq!(
        store
            .track_source_format(&saved.server.id, &track.id)
            .expect("track source format"),
        track.source_format
    );
}
#[test]
fn schema_album_prefetch() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    store
        .upsert_albums(
            &saved.server.id,
            &[album(1), album_with_image(2), album(3)],
            generation,
        )
        .expect("upsert albums");
    let albums = store
        .load_albums_without_image_ref(&saved.server.id, 0, 10)
        .expect("load albums without image ref");
    assert_eq!(
        albums.into_iter().map(|album| album.id).collect::<Vec<_>>(),
        vec![AlbumId::fake(1), AlbumId::fake(3)]
    );
}
#[test]
fn schema_artist_prefetch() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    store
        .upsert_artists(
            &saved.server.id,
            &[
                artist(1, None),
                artist(2, Some(image_ref("artist-two", "tag-two"))),
            ],
            false,
            generation,
        )
        .expect("upsert artists");
    let artists = store
        .load_artists_without_image_ref(&saved.server.id, false, 0, 10)
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
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album_with_image(1);
    let track = track(1, &album);
    let artist = artist(1, None);
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    let loaded = store
        .load_artists(&saved.server.id, false, 0, 10)
        .expect("load artists")
        .items
        .remove(0);
    let matching = store
        .load_artists_matching(&saved.server.id, false, "Artist 1", 0, 10)
        .expect("search artists")
        .items
        .remove(0);
    let global_search = store
        .search_library(&saved.server.id, "Artist 1", 10)
        .expect("search library");
    let detail = store
        .load_artist_detail(&saved.server.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(loaded.image_ref, album.image_ref);
    assert_eq!(matching.image_ref, album.image_ref);
    assert_eq!(global_search.artists[0].image_ref, album.image_ref);
    assert_eq!(detail.artist.image_ref, album.image_ref);
}
#[test]
fn schema_win_fallback() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album_with_image(1);
    let track = track(1, &album);
    let artist_image = image_ref("artist-one", "artist-tag-one");
    let artist = artist(1, Some(artist_image.clone()));
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    let loaded = store
        .load_artists(&saved.server.id, false, 0, 10)
        .expect("load artists")
        .items
        .remove(0);
    let detail = store
        .load_artist_detail(&saved.server.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(loaded.image_ref, Some(artist_image.clone()));
    assert_eq!(detail.artist.image_ref, Some(artist_image));
}
#[test]
fn album_artist_image() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album_artist_id = ArtistId::fake(8);
    let mut album = album_with_image(8);
    album.artist_id = Some(ArtistId::fake(99));
    album.album_artist_credits = vec![credit(album_artist_id.clone(), "Linked Album Artist")];
    let mut album_artist = artist(8, None);
    album_artist.name = "Linked Album Artist".to_string();
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&album_artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    let loaded = store
        .load_artists(&saved.server.id, true, 0, 10)
        .expect("load album artists")
        .items
        .into_iter()
        .find(|artist| artist.id == album_artist_id)
        .expect("album artist");
    let matching = store
        .load_artists_matching(&saved.server.id, true, "Linked Album Artist", 0, 10)
        .expect("search album artists")
        .items
        .remove(0);
    assert_eq!(loaded.image_ref, album.image_ref);
    assert_eq!(matching.image_ref, album.image_ref);
}
#[test]
fn schema_replace_local() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let track_id = TrackId::fake(1);
    store
        .replace_track_local_matches(
            &saved.server.id,
            &[(
                track_id.clone(),
                "/home/me/Music/Track 1.flac".to_string(),
                "metadata".to_string(),
            )],
        )
        .expect("replace local matches");
    store
        .connection
        .execute(
            "
            UPDATE track_local_matches
            SET updated_at = '2000-01-01 00:00:00'
            WHERE server_id = ?1 AND track_id = ?2
            ",
            rusqlite::params![saved.server.id.as_str(), track_id.as_str()],
        )
        .expect("mark local match timestamp");
    store
        .replace_track_local_matches(
            &saved.server.id,
            &[(
                track_id.clone(),
                "/home/me/Music/Track 1.flac".to_string(),
                "metadata".to_string(),
            )],
        )
        .expect("replace unchanged local matches");
    let updated_at = store
        .connection
        .query_row(
            "
            SELECT updated_at
            FROM track_local_matches
            WHERE server_id = ?1 AND track_id = ?2
            ",
            rusqlite::params![saved.server.id.as_str(), track_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("local match timestamp");
    assert_eq!(updated_at, "2000-01-01 00:00:00");
    assert_eq!(
        store
            .track_local_match_path(&saved.server.id, &track_id)
            .expect("match path")
            .as_deref(),
        Some("/home/me/Music/Track 1.flac")
    );
    assert_eq!(
        store
            .track_local_match_paths(&saved.server.id)
            .expect("match paths"),
        vec![(track_id.clone(), "/home/me/Music/Track 1.flac".to_string())]
    );
    store
        .replace_track_local_matches(&saved.server.id, &[])
        .expect("clear local matches");
    assert_eq!(
        store
            .track_local_match_path(&saved.server.id, &track_id)
            .expect("match path"),
        None
    );
    assert!(
        store
            .track_local_match_paths(&saved.server.id)
            .expect("match paths")
            .is_empty()
    );
}
#[test]
fn schema_track_search() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_music_folders(&saved.server.id, std::slice::from_ref(&folder), generation)
        .expect("upsert folder");
    store
        .upsert_track_music_folder_memberships(
            &saved.server.id,
            &folder.id,
            std::slice::from_ref(&tracks[1]),
            generation,
        )
        .expect("upsert membership");
    store
        .set_selected_music_folder_id(&saved.server.id, Some(&folder.id))
        .expect("select folder");
    let page = store
        .load_tracks(&saved.server.id, 0, 10)
        .expect("load tracks");
    let search = store
        .load_tracks_matching(&saved.server.id, "Track", 0, 10)
        .expect("search tracks");
    let favorites = store
        .load_favorite_tracks(&saved.server.id)
        .expect("load favorites");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, tracks[1].id);
    assert_eq!(search.total, 1);
    assert_eq!(search.items[0].id, tracks[1].id);
    assert!(favorites.is_empty());
}
#[test]
fn schema_filter_folder() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_music_folders(&saved.server.id, std::slice::from_ref(&folder), generation)
        .expect("upsert folder");
    store
        .upsert_track_music_folder_memberships(
            &saved.server.id,
            &folder.id,
            std::slice::from_ref(&tracks[1]),
            generation,
        )
        .expect("upsert membership");
    store
        .set_selected_music_folder_id(&saved.server.id, Some(&folder.id))
        .expect("select folder");
    let loaded = store
        .load_track(&saved.server.id, &tracks[0].id)
        .expect("load track")
        .expect("track");
    assert_eq!(loaded.id, tracks[0].id);
}
#[test]
fn schema_stale_sync() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let folder = MusicFolder {
        id: MusicFolderId::fake(1),
        name: "Music".to_string(),
    };
    let first_generation = store.begin_sync(&saved.server.id).expect("begin sync");
    store
        .upsert_music_folders(
            &saved.server.id,
            std::slice::from_ref(&folder),
            first_generation,
        )
        .expect("upsert folder");
    store
        .set_selected_music_folder_id(&saved.server.id, Some(&folder.id))
        .expect("select folder");
    store
        .complete_sync(&saved.server.id, first_generation)
        .expect("complete first sync");
    let second_generation = store.begin_sync(&saved.server.id).expect("begin next sync");
    store
        .complete_sync(&saved.server.id, second_generation)
        .expect("complete second sync");
    assert!(
        store
            .list_music_folders(&saved.server.id)
            .expect("list folders")
            .is_empty()
    );
    assert_eq!(
        store
            .selected_music_folder_id(&saved.server.id)
            .expect("selected folder"),
        None
    );
}
#[test]
fn schema_trip_page() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .complete_sync(&saved.server.id, generation)
        .expect("complete sync");
    let albums = store
        .load_albums(&saved.server.id, 0, 25)
        .expect("load albums");
    let detail = store
        .load_album_detail(&saved.server.id, &album.id)
        .expect("load detail")
        .expect("detail");
    assert_eq!(albums.total, 1);
    assert_eq!(albums.items, vec![album.clone()]);
    assert_eq!(detail.0, album);
    assert_eq!(detail.1, tracks);
}
#[test]
fn schema_trip_model() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let artist = artist(1, Some(image_ref("artist-one", "artist-tag")));
    let genre = genre(1, Some(image_ref("genre-one", "genre-tag")));
    let mut album = album_with_image(1);
    album.genres = vec![genre.name.clone()];
    let track = track(1, &album);
    let playlist = playlist(1, Some(image_ref("playlist-one", "playlist-tag")));
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .upsert_genres(&saved.server.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    store
        .upsert_playlists(
            &saved.server.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    assert_eq!(
        store
            .load_albums(&saved.server.id, 0, 1)
            .expect("load albums")
            .items[0]
            .image_ref,
        album.image_ref
    );
    assert_eq!(
        store
            .load_tracks(&saved.server.id, 0, 1)
            .expect("load tracks")
            .items[0]
            .image_ref,
        track.image_ref
    );
    assert_eq!(
        store
            .load_artists(&saved.server.id, false, 0, 1)
            .expect("load artists")
            .items[0]
            .image_ref,
        artist.image_ref
    );
    assert_eq!(
        store
            .load_genres(&saved.server.id, 0, 1)
            .expect("load genres")
            .items[0]
            .image_ref,
        genre.image_ref
    );
    assert_eq!(
        store
            .load_playlists(&saved.server.id, 0, 1)
            .expect("load playlists")
            .items[0]
            .image_ref,
        playlist.image_ref
    );
}

#[test]
fn schema_collection_playlist() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
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
    store
        .upsert_albums(&saved.server.id, &albums, generation)
        .expect("upsert albums");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_genres(&saved.server.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    store
        .upsert_playlists(
            &saved.server.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_entries(&saved.server.id, &playlist.id, &entries, generation)
        .expect("upsert playlist entries");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    store
        .complete_sync(&saved.server.id, generation)
        .expect("complete sync");

    let genre_page = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load genres");
    let playlist_page = store
        .load_playlists(&saved.server.id, 0, 20)
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
    store
        .upsert_albums(
            &saved.server.id,
            std::slice::from_ref(&changed_album),
            generation,
        )
        .expect("change album");
    let cached_again = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load cached genres");
    assert_eq!(
        cached_again.items[0].image_refs,
        genre_page.items[0].image_refs
    );
}

#[test]
fn schema_repair_cache() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
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

    store
        .upsert_albums(&saved.server.id, &albums, generation)
        .expect("upsert albums");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_genres(&saved.server.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    store
        .upsert_playlists(
            &saved.server.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");

    genre.image_refs.clear();
    assert_eq!(
        store
            .load_genres(&saved.server.id, 0, 20)
            .expect("load stale genres")
            .items[0]
            .image_refs,
        genre.image_refs
    );

    store
        .ensure_collection_cover_refs(&saved.server.id)
        .expect("ensure cover refs");
    assert_eq!(
        store
            .load_genres(&saved.server.id, 0, 20)
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
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
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

    store
        .upsert_albums(&saved.server.id, &albums, generation)
        .expect("upsert albums");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_genres(
            &saved.server.id,
            &[first_genre.clone(), second_genre.clone()],
            generation,
        )
        .expect("upsert genres");
    store
        .complete_sync(&saved.server.id, generation)
        .expect("complete sync");
    store
        .connection
        .execute(
            "
            DELETE FROM collection_cover_refs
            WHERE server_id = ?1
              AND collection_type = ?2
              AND collection_id = ?3
            ",
            rusqlite::params![
                saved.server.id.as_str(),
                COLLECTION_COVER_GENRE,
                second_genre.id.as_str()
            ],
        )
        .expect("drop one genre cover cache row");

    let partial = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load partially cached genres");
    assert!(partial.items[0].image_refs.len() >= 4);
    assert!(
        partial.items[1].image_refs.is_empty(),
        "second genre should simulate an interrupted cover-ref cache"
    );

    store
        .ensure_collection_cover_refs(&saved.server.id)
        .expect("ensure cover refs");
    let repaired = store
        .load_genres(&saved.server.id, 0, 20)
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
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let fallback_image = image_ref("album-track-cover", "album-track-tag");
    let mut first_track = track(1, &album);
    first_track.image_ref = Some(fallback_image.clone());
    let second_track = track(2, &album);
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.server.id,
            &[first_track.clone(), second_track.clone()],
            generation,
        )
        .expect("upsert tracks");

    let albums = store
        .load_albums(&saved.server.id, 0, 25)
        .expect("load albums");
    let detail = store
        .load_album_detail(&saved.server.id, &album.id)
        .expect("load detail")
        .expect("detail");

    assert_eq!(albums.items[0].image_ref, Some(fallback_image.clone()));
    assert_eq!(detail.0.image_ref, Some(fallback_image));
    assert_eq!(detail.1, vec![first_track, second_track]);
}
#[test]
fn paged_read_return() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let albums = (1..=505).map(album).collect::<Vec<_>>();
    let tracks = (1..=1005)
        .map(|number| track(number, &albums[(number as usize - 1) % albums.len()]))
        .collect::<Vec<_>>();
    store
        .upsert_albums(&saved.server.id, &albums, generation)
        .expect("upsert albums");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    let album_page = store
        .load_albums(&saved.server.id, 500, 10)
        .expect("load album page");
    let track_page = store
        .load_tracks(&saved.server.id, 1000, 10)
        .expect("load track page");
    assert_eq!(album_page.total, 505);
    assert_eq!(album_page.items.len(), 5);
    assert_eq!(track_page.total, 1005);
    assert_eq!(track_page.items.len(), 5);
}
#[test]
fn schema_keep_boundaries() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
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
    store
        .upsert_albums(&saved.server.id, &[first_album, second_album], generation)
        .expect("upsert albums");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");

    let full_page = store
        .load_tracks_sorted(&saved.server.id, LibraryField::Album, false, 0, 10)
        .expect("load full sorted page");
    let first_page = store
        .load_tracks_sorted(&saved.server.id, LibraryField::Album, false, 0, 2)
        .expect("load first sorted page");
    let second_page = store
        .load_tracks_sorted(&saved.server.id, LibraryField::Album, false, 2, 2)
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

    let search_page = store
        .load_tracks_matching_sorted(
            &saved.server.id,
            "Needle",
            LibraryField::Album,
            false,
            0,
            10,
        )
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
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
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
    let playlists = (1..=505)
        .map(|number| playlist(number, None))
        .collect::<Vec<_>>();
    store
        .upsert_albums(&saved.server.id, &albums, generation)
        .expect("upsert albums");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_artists(&saved.server.id, &artists, false, generation)
        .expect("upsert artists");
    store
        .upsert_artists(&saved.server.id, &album_artists, true, generation)
        .expect("upsert album artists");
    store
        .upsert_genres(&saved.server.id, &genres, generation)
        .expect("upsert genres");
    store
        .upsert_playlists(&saved.server.id, &playlists, generation)
        .expect("upsert playlists");
    let album_page = store
        .load_albums_matching(&saved.server.id, "Needle Genre", 0, 10)
        .expect("search albums");
    let track_page = store
        .load_tracks_matching(&saved.server.id, "Track 1005", 0, 10)
        .expect("search tracks");
    let artist_page = store
        .load_artists_matching(&saved.server.id, false, "Artist 505", 0, 10)
        .expect("search artists");
    let album_artist_page = store
        .load_artists_matching(&saved.server.id, true, "Artist 505", 0, 10)
        .expect("search album artists");
    let genre_page = store
        .load_genres_matching(&saved.server.id, "Needle Genre", 0, 10)
        .expect("search genres");
    let playlist_page = store
        .load_playlists_matching(&saved.server.id, "Playlist 505", 0, 10)
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
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let track_one = track(1, &album);
    let track_two = track(2, &album);
    let playlist = playlist(1, None);
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.server.id,
            &[track_one.clone(), track_two.clone()],
            generation,
        )
        .expect("upsert tracks");
    store
        .upsert_playlists(
            &saved.server.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_tracks(
            &saved.server.id,
            &playlist.id,
            &[track_two.clone(), track_one.clone()],
            generation,
        )
        .expect("upsert playlist tracks");
    let detail = store
        .load_playlist_detail(&saved.server.id, &playlist.id)
        .expect("load playlist detail")
        .expect("playlist detail");
    assert_eq!(detail.playlist, playlist);
    assert_eq!(detail.tracks, vec![track_two, track_one]);
}
