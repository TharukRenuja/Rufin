use std::fs;

use library::{Library, LibraryError, PlaybackLoad, ScrobbleService, SourceId};
use rusqlite::Connection;

#[test]
fn missing_source_table_repairs_without_touching_configuration_or_durable_rows() {
    let directory = tempfile::tempdir().expect("temporary repair directory");
    let store_path = directory.path().join("rufin-store.sqlite");
    let settings_path = directory.path().join("settings.json");
    let settings = br#"{"sources":{"configured":[{"source_id":"local"}]},"private_mode":true}"#;
    fs::write(&settings_path, settings).expect("write external Settings");

    drop(Library::open(&store_path).expect("create final Store"));
    let connection = Connection::open(&store_path).expect("open final Store for fixture");
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .expect("set fixture busy timeout");
    insert_source_and_user_rows(&connection);
    connection
        .execute_batch(
            "INSERT INTO lyrics_cache(
                source_id, track_id, role, language, script, origin,
                input_version, input_digest, payload, cached_at
             ) VALUES (
                'local', 'track:1', 'primary', '', '', 'source',
                1, zeroblob(32), 'cached lyrics', 10
             );
             INSERT INTO album_release_info(
                source_id, album_id, exact_identity_key, lookup_state,
                release_types_json, is_compilation
             ) VALUES ('local', 'album:1', 'identity:1', 'missing', NULL, NULL);
             DROP TABLE albums;",
        )
        .expect("damage one rebuildable table");
    drop(connection);

    assert!(matches!(
        Library::open(&store_path),
        Err(LibraryError::InvalidStore(_))
    ));

    let (library, repair) =
        Library::open_with_repair(&store_path).expect("repair identified final Store");
    let report = repair.expect("repair report");
    assert_eq!(report.recovered_rows, 9);
    assert_eq!(
        report.skipped_rows, 1,
        "one unreadable user row must not prevent independent salvage"
    );
    assert!(report.unreadable_families.is_empty());
    assert_eq!(
        fs::read(&settings_path).expect("reread Settings"),
        settings,
        "Library repair must not write external configuration"
    );
    assert!(
        report.preserved_store.exists(),
        "the damaged Store remains recoverable"
    );

    assert!(
        library
            .load_source(&SourceId::new("local"))
            .expect("load rebuilt source state")
            .is_none(),
        "ordinary source startup must rebuild discarded source facts"
    );
    assert!(matches!(
        library
            .load_playback(&SourceId::new("local"))
            .expect("load preserved Playback"),
        PlaybackLoad::Ready(_)
    ));
    assert_eq!(
        library
            .due_scrobbles(ScrobbleService::LastFm, "listener", 20, 10)
            .expect("inspect blocked scrobble delivery")
            .len(),
        0,
        "credential-blocked work remains queued rather than becoming due"
    );
    drop(library);

    let repaired = Connection::open(&store_path).expect("inspect repaired Store");
    for table in [
        "source_libraries",
        "albums",
        "tracks",
        "artists",
        "genres",
        "music_folders",
        "source_playlists",
        "source_playlist_entries",
        "local_files",
        "local_access_files",
        "lyrics_cache",
        "album_release_info",
    ] {
        assert_eq!(
            row_count(&repaired, table),
            0,
            "{table} is rebuildable and must start empty after repair"
        );
    }
    for table in [
        "local_favorites",
        "local_playlists",
        "local_playlist_entries",
        "smart_playlists",
        "local_imports",
        "playback_queues",
        "playback_state",
        "listening_aggregates",
        "recent_plays",
        "pending_scrobbles",
    ] {
        assert_eq!(
            row_count(&repaired, table),
            1,
            "{table} is Rufin-owned durable data"
        );
    }
    assert_eq!(
        repaired
            .query_row(
                "SELECT next_attempt_at, last_error
                 FROM pending_scrobbles
                 WHERE service = 'lastfm'
                   AND account_id = 'listener'
                   AND play_id = 'play:1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .expect("read preserved blocked scrobble"),
        (None, Some("session expired".to_string()))
    );

    let preserved = Connection::open(&report.preserved_store).expect("open preserved Store");
    assert_eq!(
        preserved
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'albums'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("inspect preserved schema"),
        0,
        "the report points to the damaged original rather than another fresh Store"
    );
}

#[test]
fn unsupported_and_unidentifiable_stores_are_preserved_and_rebuilt() {
    let directory = tempfile::tempdir().expect("temporary recovery directory");
    let unknown_path = directory.path().join("unknown.sqlite");
    let unknown = Connection::open(&unknown_path).expect("create unknown Store");
    unknown
        .execute_batch(
            "PRAGMA application_id = 0;
             PRAGMA user_version = 13;
             CREATE TABLE unrelated(value TEXT);",
        )
        .expect("create unknown schema");
    drop(unknown);
    let unknown_before = fs::read(&unknown_path).expect("read unknown Store");

    let (unknown_library, unknown_repair) = Library::open_with_repair(&unknown_path)
        .expect("replace unsupported Store with a current Store");
    let unknown_repair = unknown_repair.expect("unsupported Store recovery report");
    assert_eq!(
        fs::read(&unknown_repair.preserved_store).expect("read preserved unsupported Store"),
        unknown_before,
        "unsupported Store contents remain available beside the replacement"
    );
    assert!(
        unknown_library
            .load_source(&SourceId::new("local"))
            .expect("read replacement Store")
            .is_none()
    );

    let malformed_path = directory.path().join("malformed.sqlite");
    let malformed = b"this is not a SQLite database";
    fs::write(&malformed_path, malformed).expect("write malformed Store");
    let (malformed_library, malformed_repair) = Library::open_with_repair(&malformed_path)
        .expect("replace malformed Store with a current Store");
    let malformed_repair = malformed_repair.expect("malformed Store recovery report");
    assert_eq!(
        fs::read(&malformed_repair.preserved_store).expect("read preserved malformed Store"),
        malformed,
        "malformed Store contents remain available beside the replacement"
    );
    assert!(
        malformed_library
            .load_source(&SourceId::new("local"))
            .expect("read replacement Store")
            .is_none()
    );
}

fn insert_source_and_user_rows(connection: &Connection) {
    connection
        .execute_batch(
            "INSERT INTO source_libraries(
                source_id, input_version, input_digest, content_digest,
                home_digest, home_json, accepted_at
             ) VALUES (
                'local', 1, zeroblob(32), zeroblob(32),
                zeroblob(32), '{}', 10
             );
             INSERT INTO local_favorites(source_id, item_kind, item_id)
             VALUES ('local', 'track', 'track:1');
             INSERT INTO local_playlists(source_id, playlist_id, name)
             VALUES ('local', 'playlist:1', 'Saved mix');
             INSERT INTO local_playlist_entries(
                source_id, playlist_id, position, occurrence_id, track_id
             ) VALUES ('local', 'playlist:1', 0, 'occurrence:1', 'track:1');
             INSERT INTO smart_playlists(
                source_id, smart_playlist_id, name, builtin_key,
                definition_json, position
             ) VALUES (
                'local', 'smart:1', 'Saved smart playlist', NULL,
                '{\"sort_field\":\"Title\",\"descending\":false}', 0
             );
             INSERT INTO smart_playlists(
                source_id, smart_playlist_id, name, builtin_key,
                definition_json, position
             ) VALUES (
                'local', 'smart:invalid', 'Unreadable smart playlist', NULL,
                '{}', 1
             );
             INSERT INTO local_imports(source_id, track_id, first_seen_at)
             VALUES ('local', 'track:1', 10);
             INSERT INTO playback_queues(source_id, revision, payload_json)
             VALUES (
                'local', 1,
                '{\"occurrences\":[],\"fallback_tracks\":[],\"traversal\":[]}'
             );
             INSERT INTO playback_state(
                source_id, revision, selected_occurrence_id, progress_millis
             ) VALUES ('local', 1, NULL, 0);
             INSERT INTO listening_aggregates(
                source_id, period, item_kind, item_id, display_name,
                display_context, play_count, skip_count, last_played_at
             ) VALUES (
                'local', 'lifetime', 'track', 'track:1', 'Track one',
                'Artist one', 2, 0, 10
             );
             INSERT INTO recent_plays(
                play_id, source_id, track_id, track_title, artist_name,
                album_title, played_at
             ) VALUES (
                'play:1', 'local', 'track:1', 'Track one', 'Artist one',
                'Album one', 10
             );
             INSERT INTO pending_scrobbles(
                service, account_id, play_id, track_title, artist_name,
                album_title, duration_millis, started_at, attempts,
                next_attempt_at, last_error
             ) VALUES (
                'lastfm', 'listener', 'play:1', 'Track one', 'Artist one',
                'Album one', 180000, 10, 0, NULL, 'session expired'
             );",
        )
        .expect("insert final Store fixture rows");
}

fn row_count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|error| panic!("count {table}: {error}"))
}
