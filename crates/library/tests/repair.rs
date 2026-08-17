use std::fs;

use library::{Libraries, SourceId};
use rusqlite::Connection;

#[test]
fn schema_30_store_is_preserved_and_replaced_without_blocking_startup() {
    let directory = tempfile::tempdir().expect("temporary recovery directory");
    let store_path = directory.path().join("rufin-store.sqlite");
    let old_store = Connection::open(&store_path).expect("create old Rufin Store");
    old_store
        .execute_batch(
            "PRAGMA application_id = 0;
             PRAGMA user_version = 30;
             CREATE TABLE sources(
                 source_id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL,
                 name TEXT,
                 provider_payload TEXT
             );
             INSERT INTO sources VALUES (
                 'local:legacy', 'local', 'Local', '{}'
             );",
        )
        .expect("create schema 30 Rufin Store");
    drop(old_store);
    let before = fs::read(&store_path).expect("read old Rufin Store");

    let (library, repair) = Libraries::open_with_repair(&store_path)
        .expect("replace old Rufin Store without blocking startup");
    let repair = repair.expect("old Store replacement report");
    assert_eq!(
        fs::read(&repair.preserved_store).expect("read preserved old Store"),
        before,
        "the schema 30 Store remains available beside its replacement"
    );
    assert!(
        library
            .load_source(&SourceId::new("local"))
            .expect("read replacement Store")
            .is_none()
    );
}

#[test]
fn newer_additive_store_is_not_treated_as_damage() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let store_path = directory.path().join("rufin-store.sqlite");
    let (library, repair) = Libraries::open_with_repair(&store_path).expect("create Store");
    assert!(repair.is_none());
    drop(library);
    let connection = Connection::open(&store_path).expect("open Store directly");
    connection
        .execute_batch(
            "CREATE TABLE future_facts(value TEXT) STRICT;
             PRAGMA user_version = 41;",
        )
        .expect("prepare newer additive Store");
    drop(connection);

    let (library, repair) =
        Libraries::open_with_repair(&store_path).expect("open newer additive Store");
    assert!(repair.is_none(), "a newer schema is not a damaged Store");
    assert!(
        library
            .load_source(&SourceId::new("local"))
            .expect("read known Store tables")
            .is_none()
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

    let (unknown_library, unknown_repair) = Libraries::open_with_repair(&unknown_path)
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
    let (malformed_library, malformed_repair) = Libraries::open_with_repair(&malformed_path)
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
