//! Recovery for an unusable installed Store.
//!
//! Every unusable Store is preserved and replaced. A recognizable current
//! Store also contributes independently readable Rufin-owned rows; source and
//! cache facts always return through the ordinary library refresh.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::types::Value;
use rusqlite::{Connection, Error as SqliteError, ErrorCode, OpenFlags, params, params_from_iter};

use crate::{
    PlaybackCheckpoint, PlaybackOccurrenceId, PlaybackQueueRowsSnapshot, PlaybackState, SourceId,
};

use super::{StoreError, StoreResult, schema};

static REPAIR_FILE_NUMBER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoreRepairReport {
    /// The original database. Its `-wal` and `-shm` files, when present, use
    /// the same preserved path.
    pub preserved_store: PathBuf,
    pub recovered_rows: usize,
    pub skipped_rows: usize,
    /// A missing or unreadable durable table is reported without preventing
    /// other independent families from being recovered.
    pub unreadable_families: Vec<&'static str>,
}

pub(crate) fn caused_by_store_contents(error: &StoreError) -> bool {
    match error {
        StoreError::UnsupportedSchema { .. } | StoreError::InvalidFinalSchema(_) => true,
        StoreError::Sqlite(SqliteError::SqliteFailure(code, _)) => matches!(
            code.code,
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase
        ),
        _ => false,
    }
}

pub(crate) fn repair(path: &Path) -> StoreResult<StoreRepairReport> {
    let source = salvage_source(path);
    let prepared_path = unique_sibling(path, "repairing")?;
    let preserved_path = unique_sibling(path, "damaged")?;
    let result = prepare_replacement(source.as_ref(), &prepared_path, preserved_path.clone())
        .and_then(|report| {
            drop(source);
            publish_replacement(path, &prepared_path, &preserved_path)?;
            Ok(report)
        });
    if result.is_err() {
        remove_store_files(&prepared_path);
    }
    result
}

fn salvage_source(path: &Path) -> Option<Connection> {
    let source = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    source.pragma_update(None, "query_only", true).ok()?;
    let application_id = source
        .pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0))
        .ok()?;
    let user_version = source
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .ok()?;
    (application_id == schema::APPLICATION_ID && user_version == schema::SCHEMA_VERSION)
        .then_some(source)
}

fn prepare_replacement(
    source: Option<&Connection>,
    prepared_path: &Path,
    preserved_path: PathBuf,
) -> StoreResult<StoreRepairReport> {
    let destination = Connection::open_with_flags(
        prepared_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    destination.pragma_update(None, "foreign_keys", true)?;
    destination.pragma_update(None, "synchronous", "FULL")?;
    schema::initialize(&destination)?;

    let mut report = StoreRepairReport {
        preserved_store: preserved_path,
        ..StoreRepairReport::default()
    };
    if let Some(source) = source {
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "Local favorites",
                select: "SELECT source_id, item_kind, item_id FROM local_favorites",
                insert: "INSERT INTO local_favorites(source_id, item_kind, item_id)
                     VALUES (?1, ?2, ?3)",
            },
        )?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "User ratings",
                select: "SELECT source_id, item_kind, item_id, rating FROM user_ratings",
                insert: "INSERT INTO user_ratings(source_id, item_kind, item_id, rating)
                     VALUES (?1, ?2, ?3, ?4)",
            },
        )?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "Pending favorites",
                select: "SELECT source_id, item_kind, item_id, favorite,
                                previous_favorite, attempts, next_attempt_at
                         FROM pending_favorites",
                insert: "INSERT INTO pending_favorites(
                             source_id, item_kind, item_id, favorite,
                             previous_favorite, attempts, next_attempt_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            },
        )?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "Loudness measurements",
                select: "SELECT
                        source_id, scope, item_id, analysis_key,
                        integrated_lufs, true_peak
                     FROM loudness_measurements",
                insert: "INSERT INTO loudness_measurements(
                        source_id, scope, item_id, analysis_key,
                        integrated_lufs, true_peak
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            },
        )?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "Local playlists",
                select: "SELECT source_id, playlist_id, name
                     FROM local_playlists
                     ORDER BY source_id, playlist_id",
                insert: "INSERT INTO local_playlists(source_id, playlist_id, name)
                     VALUES (?1, ?2, ?3)",
            },
        )?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "Local playlist entries",
                select: "SELECT
                        source_id, playlist_id, position, occurrence_id, track_id
                     FROM local_playlist_entries
                     ORDER BY source_id, playlist_id, position",
                insert: "INSERT INTO local_playlist_entries(
                        source_id, playlist_id, position, occurrence_id, track_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
            },
        )?;
        salvage_smart_playlists(source, &destination, &mut report)?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "Local imports",
                select: "SELECT source_id, track_id, first_seen_at FROM local_imports",
                insert: "INSERT INTO local_imports(source_id, track_id, first_seen_at)
                     VALUES (?1, ?2, ?3)",
            },
        )?;
        salvage_playback(source, &destination, &mut report)?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "listening aggregates",
                select: "SELECT
                        source_id, period, item_kind, item_id, display_name,
                        display_context, play_count, skip_count, last_played_at
                     FROM listening_aggregates",
                insert: "INSERT INTO listening_aggregates(
                        source_id, period, item_kind, item_id, display_name,
                        display_context, play_count, skip_count, last_played_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            },
        )?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "recent plays",
                select: "SELECT
                        play_id, source_id, track_id, track_title, artist_name,
                        album_title, played_at
                     FROM recent_plays",
                insert: "INSERT INTO recent_plays(
                        play_id, source_id, track_id, track_title, artist_name,
                        album_title, played_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            },
        )?;
        salvage_simple_family(
            source,
            &destination,
            &mut report,
            SimpleFamily {
                name: "pending external scrobbles",
                select: "SELECT
                        service, account_id, play_id, track_title, artist_name,
                        album_title, duration_millis, started_at, attempts,
                        next_attempt_at, last_error
                     FROM pending_scrobbles",
                insert: "INSERT INTO pending_scrobbles(
                        service, account_id, play_id, track_title, artist_name,
                        album_title, duration_millis, started_at, attempts,
                        next_attempt_at, last_error
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
                     )",
            },
        )?;
    }

    destination.execute_batch("PRAGMA optimize;")?;
    drop(destination);
    sync_file(prepared_path)?;
    Ok(report)
}

struct SimpleFamily {
    name: &'static str,
    select: &'static str,
    insert: &'static str,
}

fn salvage_simple_family(
    source: &Connection,
    destination: &Connection,
    report: &mut StoreRepairReport,
    family: SimpleFamily,
) -> StoreResult<()> {
    let mut statement = match source.prepare(family.select) {
        Ok(statement) => statement,
        Err(_) => {
            report.unreadable_families.push(family.name);
            return Ok(());
        }
    };
    let column_count = statement.column_count();
    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(_) => {
            report.unreadable_families.push(family.name);
            return Ok(());
        }
    };
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(_) => {
                report.unreadable_families.push(family.name);
                break;
            }
        };
        let values = (0..column_count)
            .map(|index| row.get::<_, Value>(index))
            .collect::<Result<Vec<_>, _>>();
        let Ok(values) = values else {
            report.skipped_rows += 1;
            continue;
        };
        match destination.execute(family.insert, params_from_iter(values.iter())) {
            Ok(1) => report.recovered_rows += 1,
            Ok(_) => report.skipped_rows += 1,
            Err(error) if is_constraint(&error) => report.skipped_rows += 1,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn salvage_smart_playlists(
    source: &Connection,
    destination: &Connection,
    report: &mut StoreRepairReport,
) -> StoreResult<()> {
    const FAMILY: &str = "smart playlists";
    let mut statement = match source.prepare(
        "SELECT
            source_id, smart_playlist_id, name, builtin_key,
            definition_json, position
         FROM smart_playlists
         ORDER BY source_id, position, smart_playlist_id",
    ) {
        Ok(statement) => statement,
        Err(_) => {
            report.unreadable_families.push(FAMILY);
            return Ok(());
        }
    };
    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(_) => {
            report.unreadable_families.push(FAMILY);
            return Ok(());
        }
    };
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(_) => {
                report.unreadable_families.push(FAMILY);
                break;
            }
        };
        let values = (|| {
            let source_id = row.get::<_, String>(0)?;
            let smart_playlist_id = row.get::<_, String>(1)?;
            let name = row.get::<_, String>(2)?;
            let builtin_key = row.get::<_, Option<String>>(3)?;
            let definition_json = row.get::<_, String>(4)?;
            let position = row.get::<_, i64>(5)?;
            Ok::<_, SqliteError>((
                source_id,
                smart_playlist_id,
                name,
                builtin_key,
                definition_json,
                position,
            ))
        })();
        let Ok((source_id, smart_playlist_id, name, builtin_key, definition_json, position)) =
            values
        else {
            report.skipped_rows += 1;
            continue;
        };
        let valid_definition = serde_json::from_str(&definition_json)
            .ok()
            .and_then(|definition| {
                crate::smart_playlists::validated_smart_playlist_json(&definition).ok()
            });
        if source_id.is_empty()
            || smart_playlist_id.is_empty()
            || name.is_empty()
            || u32::try_from(position).is_err()
            || builtin_key
                .as_deref()
                .is_some_and(|key| crate::SmartPlaylistBuiltin::from_key(key).is_none())
        {
            report.skipped_rows += 1;
            continue;
        }
        let Some(definition_json) = valid_definition else {
            report.skipped_rows += 1;
            continue;
        };
        match destination.execute(
            "INSERT INTO smart_playlists(
                source_id, smart_playlist_id, name, builtin_key,
                definition_json, position
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                source_id,
                smart_playlist_id,
                name,
                builtin_key,
                definition_json,
                position
            ],
        ) {
            Ok(1) => report.recovered_rows += 1,
            Ok(_) => report.skipped_rows += 1,
            Err(error) if is_constraint(&error) => report.skipped_rows += 1,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn salvage_playback(
    source: &Connection,
    destination: &Connection,
    report: &mut StoreRepairReport,
) -> StoreResult<()> {
    const FAMILY: &str = "Playback checkpoints";
    let mut statement = match source.prepare(
        "SELECT
            queue.source_id, queue.revision, queue.rows_json,
            queue.traversal_json, state.selected_occurrence_id,
            state.progress_millis
         FROM playback_queues AS queue
         JOIN playback_state AS state
           ON state.source_id = queue.source_id
          AND state.revision = queue.revision
         ORDER BY queue.source_id",
    ) {
        Ok(statement) => statement,
        Err(_) => {
            report.unreadable_families.push(FAMILY);
            return Ok(());
        }
    };
    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(_) => {
            report.unreadable_families.push(FAMILY);
            return Ok(());
        }
    };
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(_) => {
                report.unreadable_families.push(FAMILY);
                break;
            }
        };
        let values = (|| {
            Ok::<_, SqliteError>((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })();
        let Ok((source_id, revision, rows_json, traversal_json, selected, progress_millis)) =
            values
        else {
            report.skipped_rows += 1;
            continue;
        };
        let checkpoint = playback_checkpoint(
            &source_id,
            revision,
            &rows_json,
            &traversal_json,
            selected.as_deref(),
            progress_millis,
        );
        if checkpoint.is_none() {
            report.skipped_rows += 1;
            continue;
        }

        let transaction = destination.unchecked_transaction()?;
        let result = transaction
            .execute(
                "INSERT INTO playback_queues(
                    source_id, revision, rows_json, traversal_json
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![source_id, revision, rows_json, traversal_json],
            )
            .and_then(|_| {
                transaction.execute(
                    "INSERT INTO playback_state(
                        source_id, revision, selected_occurrence_id,
                        progress_millis
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![source_id, revision, selected, progress_millis],
                )
            });
        match result {
            Ok(1) => {
                transaction.commit()?;
                report.recovered_rows += 1;
            }
            Ok(_) => {
                transaction.rollback()?;
                report.skipped_rows += 1;
            }
            Err(error) if is_constraint(&error) => {
                transaction.rollback()?;
                report.skipped_rows += 1;
            }
            Err(error) => {
                transaction.rollback()?;
                return Err(error.into());
            }
        }
    }
    Ok(())
}

fn playback_checkpoint(
    source_id: &str,
    revision: i64,
    rows_json: &str,
    traversal_json: &str,
    selected: Option<&str>,
    progress_millis: i64,
) -> Option<PlaybackCheckpoint> {
    if source_id.is_empty() || selected.is_some_and(str::is_empty) {
        return None;
    }
    let rows = serde_json::from_str::<PlaybackQueueRowsSnapshot>(rows_json).ok()?;
    let checkpoint = PlaybackCheckpoint {
        source_id: SourceId::new(source_id),
        revision: revision.try_into().ok()?,
        queue: rows.with_traversal(
            serde_json::from_str::<Vec<PlaybackOccurrenceId>>(traversal_json).ok()?,
        ),
        state: PlaybackState {
            selected: selected.map(PlaybackOccurrenceId::new),
            progress_millis: progress_millis.try_into().ok()?,
        },
    };
    crate::playback_state::validate_checkpoint(&checkpoint)
        .is_ok()
        .then_some(checkpoint)
}

fn is_constraint(error: &SqliteError) -> bool {
    matches!(
        error,
        SqliteError::SqliteFailure(code, _) if code.code == ErrorCode::ConstraintViolation
    )
}

fn unique_sibling(path: &Path, suffix: &str) -> StoreResult<PathBuf> {
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
    let file_name = path
        .file_name()
        .ok_or_else(|| StoreError::InvalidFinalSchema("Store path has no file name".to_string()))?;
    loop {
        let number = REPAIR_FILE_NUMBER.fetch_add(1, Ordering::Relaxed);
        let mut candidate_name = OsString::from(file_name);
        candidate_name.push(format!(".{suffix}-{}-{number}", std::process::id()));
        let candidate = parent.map_or_else(
            || PathBuf::from(&candidate_name),
            |parent| parent.join(&candidate_name),
        );
        if !candidate.exists()
            && !sidecar(&candidate, "-wal").exists()
            && !sidecar(&candidate, "-shm").exists()
        {
            return Ok(candidate);
        }
    }
}

fn publish_replacement(
    store_path: &Path,
    prepared_path: &Path,
    preserved_path: &Path,
) -> StoreResult<()> {
    fs::rename(store_path, preserved_path)?;
    let mut moved_sidecars = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let source = sidecar(store_path, suffix);
        if !source.exists() {
            continue;
        }
        let destination = sidecar(preserved_path, suffix);
        if let Err(error) = fs::rename(&source, &destination) {
            for (source, destination) in moved_sidecars.into_iter().rev() {
                let _ = fs::rename(destination, source);
            }
            let _ = fs::rename(preserved_path, store_path);
            return Err(error.into());
        }
        moved_sidecars.push((source, destination));
    }
    if let Err(error) = fs::rename(prepared_path, store_path) {
        for (source, destination) in moved_sidecars.into_iter().rev() {
            let _ = fs::rename(destination, source);
        }
        let _ = fs::rename(preserved_path, store_path);
        return Err(error.into());
    }
    sync_parent(store_path)
}

fn remove_store_files(path: &Path) {
    for path in [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ] {
        let _ = fs::remove_file(path);
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn sync_file(path: &Path) -> StoreResult<()> {
    fs::OpenOptions::new().write(true).open(path)?.sync_all()?;
    Ok(())
}

fn sync_parent(path: &Path) -> StoreResult<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
