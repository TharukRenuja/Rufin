//! The one released Store migration.
//!
//! This module reads schema 30 without exposing a legacy Store API. Rufin
//! merges its neutral configuration facts into Settings, then asks this handle
//! to prepare a fresh current Store containing only Rufin-owned user data.
//! Rebuildable source facts never participate in source recovery.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Deserialize;

use super::{StoreError, Worker, schema};
use crate::favorites::FavoriteValue;
use crate::{
    AlbumId, ArtistId, CueSegment, FavoriteItemId, ImageRef, MusicFolderId, PlaybackCheckpoint,
    PlaybackFallbackTrack, PlaybackOccurrence, PlaybackOccurrenceId, PlaybackProvenance,
    PlaybackQueueSnapshot, PlaybackState, Playlist, PlaylistEntry, PlaylistId, PlaylistSnapshot,
    SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistId, SmartPlaylistRecord,
    SmartPlaylistRule, SmartPlaylistSortField, SourceId, TrackId,
};

const RELEASED_SCHEMA_VERSION: i64 = 30;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schema30LocalAccess {
    pub root_path: String,
    pub server_prefix: Option<String>,
    pub local_prefix: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schema30Source {
    pub source_id: SourceId,
    pub kind: String,
    pub name: String,
    pub provider_payload: String,
    pub music_folder_id: Option<MusicFolderId>,
    pub local_access: Option<Schema30LocalAccess>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Schema30Repeat {
    Off,
    One,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Schema30PlaybackModes {
    pub repeat: Schema30Repeat,
    pub shuffle_enabled: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schema30Configuration {
    pub sources: Vec<Schema30Source>,
    pub active_source_id: Option<SourceId>,
    pub skipped_sources: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schema30AcceptedSource {
    pub source_id: SourceId,
    pub local: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schema30MigrationReport {
    pub playback_checkpoints: usize,
    pub local_favorites: usize,
    pub local_playlists: usize,
    pub smart_playlists: usize,
    pub activity_rows: usize,
    pub skipped_playback_checkpoints: usize,
    pub skipped_local_favorites: usize,
    pub skipped_local_playlists: usize,
    pub skipped_smart_playlists: usize,
    pub skipped_activity_rows: usize,
}

pub struct Schema30Migration {
    connection: Connection,
    configuration: Schema30Configuration,
}

impl Schema30Migration {
    pub fn open(path: impl AsRef<Path>) -> crate::LibraryResult<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(StoreError::from)?;
        connection
            .pragma_update(None, "query_only", true)
            .map_err(StoreError::from)?;
        require_released_schema(&connection)?;
        let configuration = read_configuration(&connection)?;
        Ok(Self {
            connection,
            configuration,
        })
    }

    pub fn configuration(&self) -> &Schema30Configuration {
        &self.configuration
    }

    pub fn playback_modes(&self, source_id: &SourceId) -> Option<Schema30PlaybackModes> {
        read_playback_modes(&self.connection, source_id)
    }

    pub fn prepare_store(
        &self,
        path: impl AsRef<Path>,
        accepted_sources: &[Schema30AcceptedSource],
    ) -> crate::LibraryResult<Schema30MigrationReport> {
        let path = path.as_ref();
        if path.exists() {
            return Err(crate::LibraryError::ReleasedMigration(format!(
                "prepared Store already exists at {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(StoreError::from)?;
        }

        let result = self.prepare_store_inner(path, accepted_sources);
        if result.is_err() {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(sidecar(path, "-wal"));
            let _ = fs::remove_file(sidecar(path, "-shm"));
        }
        result
    }

    fn prepare_store_inner(
        &self,
        path: &Path,
        accepted_sources: &[Schema30AcceptedSource],
    ) -> crate::LibraryResult<Schema30MigrationReport> {
        let accepted = accepted_sources
            .iter()
            .map(|source| (source.source_id.clone(), source.local))
            .collect::<HashMap<_, _>>();
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(StoreError::from)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(StoreError::from)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(StoreError::from)?;
        schema::initialize(&connection)?;
        let mut worker = Worker {
            connection,
            cleanup: Default::default(),
            cleanup_set: HashSet::<i64>::new(),
        };

        let cue = read_cue_segments(&self.connection, &accepted);
        let tracks = read_released_tracks(&self.connection, &accepted, &cue);
        let mut report = Schema30MigrationReport::default();
        import_playback(
            &self.connection,
            &mut worker,
            &accepted,
            &tracks,
            &cue,
            &mut report,
        );
        import_local_favorites(&self.connection, &mut worker, &accepted, &mut report);
        import_local_playlists(&self.connection, &mut worker, &accepted, &mut report);
        import_smart_playlists(&self.connection, &mut worker, &accepted, &mut report);
        import_activity(
            &self.connection,
            &mut worker.connection,
            &accepted,
            &tracks,
            &mut report,
        );

        worker
            .connection
            .execute_batch("PRAGMA optimize;")
            .map_err(StoreError::from)?;
        drop(worker);
        sync_file(path)?;
        Ok(report)
    }
}

fn require_released_schema(connection: &Connection) -> crate::LibraryResult<()> {
    let application_id = connection
        .pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0))
        .map_err(StoreError::from)?;
    let user_version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(StoreError::from)?;
    if application_id != 0 || user_version != RELEASED_SCHEMA_VERSION {
        return Err(crate::LibraryError::UnsupportedStore {
            application_id,
            user_version,
        });
    }

    let columns = table_columns(connection, "sources")?;
    let valid = required_column(&columns, "source_id", true, false)
        && required_column(&columns, "kind", false, true)
        && required_column(&columns, "name", false, true)
        && required_column(&columns, "provider_payload", false, true);
    if !valid {
        return Err(crate::LibraryError::ReleasedMigration(
            "schema-30 sources table does not have the released signature".to_string(),
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct TableColumn {
    name: String,
    kind: String,
    not_null: bool,
    primary_key: bool,
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<TableColumn>, StoreError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    Ok(statement
        .query_map([], |row| {
            Ok(TableColumn {
                name: row.get(1)?,
                kind: row.get(2)?,
                not_null: row.get::<_, i64>(3)? != 0,
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn required_column(columns: &[TableColumn], name: &str, primary_key: bool, not_null: bool) -> bool {
    columns.iter().any(|column| {
        column.name == name
            && column.kind.eq_ignore_ascii_case("TEXT")
            && (!primary_key || column.primary_key)
            && (!not_null || column.not_null)
    })
}

fn has_columns(connection: &Connection, table: &str, required: &[&str]) -> bool {
    table_columns(connection, table).is_ok_and(|columns| {
        required
            .iter()
            .all(|required| columns.iter().any(|column| column.name == *required))
    })
}

fn read_configuration(connection: &Connection) -> Result<Schema30Configuration, StoreError> {
    let mut configuration = Schema30Configuration::default();
    let mut statement = connection.prepare(
        "SELECT source_id, kind, name, provider_payload
         FROM sources
         ORDER BY name COLLATE NOCASE, source_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (source_id, kind, name, provider_payload) = row?;
        if source_id.trim().is_empty()
            || kind.trim().is_empty()
            || name.trim().is_empty()
            || provider_payload.trim().is_empty()
        {
            configuration.skipped_sources += 1;
            continue;
        }
        let source_id = SourceId::new(source_id);
        configuration.sources.push(Schema30Source {
            music_folder_id: read_music_folder(connection, &source_id),
            local_access: read_local_access(connection, &source_id),
            source_id,
            kind,
            name,
            provider_payload,
        });
    }

    if has_columns(connection, "active_source", &["singleton", "source_id"]) {
        configuration.active_source_id = connection
            .query_row(
                "SELECT source_id FROM active_source WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .filter(|source_id| !source_id.trim().is_empty())
            .map(SourceId::new)
            .filter(|selected| {
                configuration
                    .sources
                    .iter()
                    .any(|source| &source.source_id == selected)
            });
    }
    Ok(configuration)
}

fn read_playback_modes(
    connection: &Connection,
    source_id: &SourceId,
) -> Option<Schema30PlaybackModes> {
    if !has_columns(
        connection,
        "playback_checkpoints",
        &["source_id", "repeat_mode", "shuffle_enabled"],
    ) {
        return None;
    }
    let (repeat, shuffle_enabled) = connection
        .query_row(
            "SELECT repeat_mode, shuffle_enabled
             FROM playback_checkpoints
             WHERE source_id = ?1",
            [source_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    let repeat = match repeat.as_str() {
        "Off" => Schema30Repeat::Off,
        "One" => Schema30Repeat::One,
        "All" => Schema30Repeat::All,
        _ => return None,
    };
    Some(Schema30PlaybackModes {
        repeat,
        shuffle_enabled: match shuffle_enabled {
            0 => false,
            1 => true,
            _ => return None,
        },
    })
}

fn read_music_folder(connection: &Connection, source_id: &SourceId) -> Option<MusicFolderId> {
    if !has_columns(
        connection,
        "source_library_preferences",
        &["source_id", "selected_music_folder_id"],
    ) {
        return None;
    }
    connection
        .query_row(
            "SELECT selected_music_folder_id
             FROM source_library_preferences
             WHERE source_id = ?1",
            [source_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .map(MusicFolderId::new)
}

fn read_local_access(connection: &Connection, source_id: &SourceId) -> Option<Schema30LocalAccess> {
    if !has_columns(
        connection,
        "source_local_access",
        &[
            "source_id",
            "root_path",
            "path_replace_from",
            "path_replace_to",
        ],
    ) {
        return None;
    }
    connection
        .query_row(
            "SELECT root_path, path_replace_from, path_replace_to
             FROM source_local_access
             WHERE source_id = ?1",
            [source_id.as_str()],
            |row| {
                Ok(Schema30LocalAccess {
                    root_path: row.get(0)?,
                    server_prefix: row.get(1)?,
                    local_prefix: row.get(2)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
        .filter(|access| !access.root_path.trim().is_empty())
}

#[derive(Clone)]
struct ReleasedTrack {
    id: TrackId,
    album_id: Option<AlbumId>,
    title: String,
    artist: String,
    artist_id: Option<ArtistId>,
    album: String,
    year: u16,
    duration_seconds: u32,
    favorite: bool,
    disc_number: u16,
    track_number: u16,
    image_ref: Option<ImageRef>,
    source_path: Option<String>,
    source_format: Option<String>,
    cue: Option<CueSegment>,
}

#[derive(Clone)]
struct ReleasedCue {
    source_path: Option<String>,
    cue: CueSegment,
}

fn read_cue_segments(
    connection: &Connection,
    accepted: &HashMap<SourceId, bool>,
) -> HashMap<(SourceId, TrackId), ReleasedCue> {
    if !has_columns(
        connection,
        "source_objects",
        &[
            "source_id",
            "source_object_kind",
            "entity_kind",
            "entity_id",
            "source_path",
            "cue_path",
            "segment_start_ms",
            "segment_end_ms",
        ],
    ) {
        return HashMap::new();
    }
    let Ok(mut statement) = connection.prepare(
        "SELECT
            source_id, entity_id, source_path, cue_path,
            segment_start_ms, segment_end_ms
         FROM source_objects
         WHERE source_object_kind = 'cue_track'
           AND entity_kind = 'track'
         ORDER BY source_id, entity_id, source_object_id",
    ) else {
        return HashMap::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
        ))
    }) else {
        return HashMap::new();
    };
    let mut cues = HashMap::new();
    let mut ambiguous = HashSet::new();
    for row in rows.flatten() {
        let (source_id, track_id, source_path, cue_path, start, end) = row;
        if source_id.is_empty() || track_id.as_deref().is_none_or(str::is_empty) {
            continue;
        }
        let source_id = SourceId::new(source_id);
        if !accepted.get(&source_id).copied().unwrap_or(false) {
            continue;
        }
        let Some((track_id, cue_path, start, end)) = track_id
            .zip(cue_path)
            .zip(start)
            .zip(end)
            .map(|(((track_id, cue_path), start), end)| (track_id, cue_path, start, end))
        else {
            continue;
        };
        let Ok(start_millis) = u64::try_from(start) else {
            continue;
        };
        let Ok(end_millis) = u64::try_from(end) else {
            continue;
        };
        if cue_path.is_empty() || end_millis <= start_millis {
            continue;
        }
        let key = (source_id, TrackId::new(track_id));
        if cues.contains_key(&key) {
            cues.remove(&key);
            ambiguous.insert(key);
            continue;
        }
        if ambiguous.contains(&key) {
            continue;
        }
        cues.insert(
            key,
            ReleasedCue {
                source_path: source_path.filter(|path| !path.is_empty()),
                cue: CueSegment {
                    cue_path,
                    start_millis,
                    end_millis,
                },
            },
        );
    }
    cues
}

fn read_released_tracks(
    connection: &Connection,
    accepted: &HashMap<SourceId, bool>,
    cues: &HashMap<(SourceId, TrackId), ReleasedCue>,
) -> HashMap<(SourceId, TrackId), ReleasedTrack> {
    if !has_columns(
        connection,
        "tracks",
        &[
            "source_id",
            "track_id",
            "album_id",
            "title",
            "artist",
            "artist_id",
            "album",
            "year",
            "duration_seconds",
            "favorite",
            "disc_number",
            "track_number",
            "image_item_id",
            "image_tag",
            "local_path",
            "source_format",
        ],
    ) {
        return HashMap::new();
    }
    let Ok(mut statement) = connection.prepare(
        "SELECT
            source_id, track_id, album_id, title, artist, artist_id, album,
            year, duration_seconds, favorite, disc_number, track_number,
            image_item_id, image_tag, local_path, source_format
         FROM tracks
         ORDER BY source_id, track_id",
    ) else {
        return HashMap::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, i64>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
            row.get::<_, Option<String>>(12)?,
            row.get::<_, Option<String>>(13)?,
            row.get::<_, Option<String>>(14)?,
            row.get::<_, Option<String>>(15)?,
        ))
    }) else {
        return HashMap::new();
    };
    let mut tracks = HashMap::new();
    for row in rows.flatten() {
        let (
            source_id,
            track_id,
            album_id,
            title,
            artist,
            artist_id,
            album,
            year,
            duration,
            favorite,
            disc,
            number,
            image_item_id,
            image_tag,
            local_path,
            source_format,
        ) = row;
        if source_id.is_empty() || track_id.is_empty() || title.is_empty() {
            continue;
        }
        let source_id = SourceId::new(source_id);
        if !accepted.contains_key(&source_id) {
            continue;
        }
        let Ok(year) = u16::try_from(year) else {
            continue;
        };
        let Ok(duration_seconds) = u32::try_from(duration) else {
            continue;
        };
        let Ok(disc_number) = u16::try_from(disc) else {
            continue;
        };
        let Ok(track_number) = u16::try_from(number) else {
            continue;
        };
        let track_id = TrackId::new(track_id);
        let key = (source_id, track_id.clone());
        let cue = cues.get(&key);
        tracks.insert(
            key,
            ReleasedTrack {
                id: track_id,
                album_id: (!album_id.is_empty()).then(|| AlbumId::new(album_id)),
                title,
                artist,
                artist_id: artist_id.filter(|id| !id.is_empty()).map(ArtistId::new),
                album,
                year,
                duration_seconds,
                favorite: favorite != 0,
                disc_number,
                track_number,
                image_ref: image_item_id
                    .filter(|id| !id.is_empty())
                    .map(|id| ImageRef {
                        item_id: id,
                        tag: image_tag.filter(|tag| !tag.is_empty()),
                    }),
                source_path: cue
                    .and_then(|cue| cue.source_path.clone())
                    .or_else(|| local_path.filter(|path| !path.is_empty())),
                source_format: source_format.filter(|format| !format.is_empty()),
                cue: cue.map(|cue| cue.cue.clone()),
            },
        );
    }
    tracks
}

fn import_playback(
    source: &Connection,
    target: &mut Worker,
    accepted: &HashMap<SourceId, bool>,
    tracks: &HashMap<(SourceId, TrackId), ReleasedTrack>,
    cues: &HashMap<(SourceId, TrackId), ReleasedCue>,
    report: &mut Schema30MigrationReport,
) {
    if !has_columns(
        source,
        "playback_checkpoints",
        &[
            "source_id",
            "revision",
            "selected_occurrence_id",
            "progress_millis",
            "repeat_mode",
            "shuffle_enabled",
            "payload",
        ],
    ) {
        return;
    }
    let Ok(mut statement) = source.prepare(
        "SELECT
            source_id, revision, selected_occurrence_id, progress_millis,
            payload
         FROM playback_checkpoints
         ORDER BY source_id",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok(ReleasedCheckpointRow {
            source_id: row.get(0)?,
            revision: row.get(1)?,
            selected: row.get(2)?,
            progress_millis: row.get(3)?,
            payload: row.get(4)?,
        })
    }) else {
        return;
    };
    for row in rows {
        let Ok(row) = row else {
            report.skipped_playback_checkpoints += 1;
            continue;
        };
        if row.source_id.is_empty() {
            report.skipped_playback_checkpoints += 1;
            continue;
        }
        let source_id = SourceId::new(row.source_id.clone());
        if !accepted.contains_key(&source_id) {
            continue;
        }
        let checkpoint = decode_v1_checkpoint(&row, cues)
            .or_else(|| decode_legacy_checkpoint(&row, tracks, cues));
        let Some(checkpoint) = checkpoint else {
            report.skipped_playback_checkpoints += 1;
            continue;
        };
        if target.replace_playback(&checkpoint).is_ok() {
            report.playback_checkpoints += 1;
        } else {
            report.skipped_playback_checkpoints += 1;
        }
    }
}

struct ReleasedCheckpointRow {
    source_id: String,
    revision: i64,
    selected: Option<String>,
    progress_millis: i64,
    payload: String,
}

#[derive(Deserialize)]
struct ReleasedPayloadV1 {
    version: u16,
    entries: Vec<ReleasedSequenceEntry>,
    #[serde(default)]
    traversal: Vec<String>,
}

#[derive(Deserialize)]
struct ReleasedSequenceEntry {
    occurrence: String,
    track: ReleasedJsonTrack,
    provenance: ReleasedProvenance,
}

#[derive(Deserialize)]
enum ReleasedProvenance {
    Context {
        context_id: String,
        source_rank: usize,
    },
    Manual,
    Random,
    Radio,
    AutoDj,
    Legacy,
}

#[derive(Deserialize)]
struct ReleasedArtistCredit {
    id: ArtistId,
}

#[derive(Deserialize)]
struct ReleasedJsonTrack {
    id: TrackId,
    album_id: AlbumId,
    title: String,
    artist: String,
    artist_id: Option<ArtistId>,
    #[serde(default)]
    artist_credits: Vec<ReleasedArtistCredit>,
    #[serde(default)]
    album_artist_credits: Vec<ReleasedArtistCredit>,
    album: String,
    #[serde(default)]
    year: u16,
    duration_seconds: u32,
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    disc_number: u16,
    #[serde(default)]
    track_number: u16,
    image_ref: Option<ImageRef>,
    local_path: Option<String>,
    source_format: Option<String>,
    musicbrainz_recording_id: Option<String>,
}

fn decode_v1_checkpoint(
    row: &ReleasedCheckpointRow,
    cues: &HashMap<(SourceId, TrackId), ReleasedCue>,
) -> Option<PlaybackCheckpoint> {
    let payload = serde_json::from_str::<ReleasedPayloadV1>(&row.payload).ok()?;
    if payload.version != 1 {
        return None;
    }
    let source_id = SourceId::new(row.source_id.clone());
    let revision = u64::try_from(row.revision).ok()?;
    let progress_millis = u64::try_from(row.progress_millis).ok()?;
    let mut fallbacks = Vec::new();
    let mut seen_tracks = HashSet::new();
    let mut occurrences = Vec::with_capacity(payload.entries.len());
    for entry in payload.entries {
        if entry.occurrence.is_empty() {
            return None;
        }
        let track_id = entry.track.id.clone();
        if seen_tracks.insert(track_id.clone()) {
            fallbacks.push(fallback_from_json(&source_id, entry.track, cues));
        }
        occurrences.push(PlaybackOccurrence {
            id: PlaybackOccurrenceId::new(entry.occurrence),
            track_id,
            provenance: match entry.provenance {
                ReleasedProvenance::Context {
                    context_id,
                    source_rank,
                } => PlaybackProvenance::Context {
                    context_id,
                    source_rank,
                },
                ReleasedProvenance::Manual => PlaybackProvenance::Manual,
                ReleasedProvenance::Random => PlaybackProvenance::Random,
                ReleasedProvenance::Radio => PlaybackProvenance::Radio,
                ReleasedProvenance::AutoDj => PlaybackProvenance::AutoDj,
                ReleasedProvenance::Legacy => PlaybackProvenance::Legacy,
            },
        });
    }
    let checkpoint = PlaybackCheckpoint {
        source_id,
        revision,
        queue: PlaybackQueueSnapshot {
            occurrences,
            fallback_tracks: fallbacks,
            traversal: payload
                .traversal
                .into_iter()
                .filter(|id| !id.is_empty())
                .map(PlaybackOccurrenceId::new)
                .collect(),
        },
        state: PlaybackState {
            selected: row
                .selected
                .clone()
                .filter(|id| !id.is_empty())
                .map(PlaybackOccurrenceId::new),
            progress_millis,
        },
    };
    crate::playback_state::validate_checkpoint(&checkpoint)
        .ok()
        .map(|()| checkpoint)
}

fn fallback_from_json(
    source_id: &SourceId,
    track: ReleasedJsonTrack,
    cues: &HashMap<(SourceId, TrackId), ReleasedCue>,
) -> PlaybackFallbackTrack {
    let cue = cues.get(&(source_id.clone(), track.id.clone()));
    let primary_artist_id = track
        .artist_id
        .or_else(|| track.artist_credits.first().map(|credit| credit.id.clone()))
        .or_else(|| {
            track
                .album_artist_credits
                .first()
                .map(|credit| credit.id.clone())
        });
    PlaybackFallbackTrack {
        id: track.id,
        album_id: Some(track.album_id),
        primary_artist_id,
        title: track.title,
        artist: track.artist,
        album: track.album,
        year: track.year,
        duration_seconds: track.duration_seconds,
        favorite: track.favorite,
        track_number: track.track_number,
        disc_number: track.disc_number,
        image_ref: track.image_ref,
        local_artwork: None,
        musicbrainz_recording_id: track.musicbrainz_recording_id,
        source_format: track.source_format,
        source_path: cue
            .and_then(|cue| cue.source_path.clone())
            .or(track.local_path),
        cue: cue.map(|cue| cue.cue.clone()),
    }
}

#[derive(Deserialize)]
struct ReleasedLegacyQueue {
    #[serde(alias = "server_id")]
    source_id: String,
    entries: Vec<ReleasedLegacyEntry>,
    current_index: Option<usize>,
    shuffle: ReleasedLegacyShuffle,
    #[serde(default)]
    shuffle_order: Vec<usize>,
    #[serde(default)]
    progress_seconds: u32,
}

#[derive(Deserialize)]
struct ReleasedLegacyShuffle {
    enabled: bool,
}

#[derive(Deserialize)]
struct ReleasedLegacyEntry {
    id: String,
    track_id: TrackId,
    album_id: Option<AlbumId>,
    title: String,
    artist: String,
    artist_id: Option<ArtistId>,
    album: String,
    #[serde(default)]
    year: u16,
    duration_seconds: u32,
    #[serde(default)]
    favorite: bool,
    image_ref: Option<ImageRef>,
    local_path: Option<String>,
    source_format: Option<String>,
    origin: Option<ReleasedLegacyOrigin>,
}

#[derive(Deserialize)]
enum ReleasedLegacyOrigin {
    Source { shuffle_key: String },
    Manual {},
    Random {},
    AutoDj {},
    RestoredUnknown {},
}

fn decode_legacy_checkpoint(
    row: &ReleasedCheckpointRow,
    tracks: &HashMap<(SourceId, TrackId), ReleasedTrack>,
    cues: &HashMap<(SourceId, TrackId), ReleasedCue>,
) -> Option<PlaybackCheckpoint> {
    let snapshot = serde_json::from_str::<ReleasedLegacyQueue>(&row.payload).ok()?;
    if snapshot.source_id != row.source_id {
        return None;
    }
    let source_id = SourceId::new(snapshot.source_id);
    let selected = snapshot
        .current_index
        .and_then(|index| snapshot.entries.get(index))
        .map(|entry| PlaybackOccurrenceId::new(entry.id.clone()));
    let mut occurrences = Vec::with_capacity(snapshot.entries.len());
    let mut fallbacks = Vec::new();
    let mut seen_tracks = HashSet::new();
    for entry in snapshot.entries {
        if entry.id.is_empty() {
            return None;
        }
        let provenance = match entry.origin.as_ref() {
            Some(ReleasedLegacyOrigin::Source { shuffle_key }) => {
                released_source_context(shuffle_key).unwrap_or(PlaybackProvenance::Legacy)
            }
            Some(ReleasedLegacyOrigin::Manual {}) => PlaybackProvenance::Manual,
            Some(ReleasedLegacyOrigin::Random {}) => PlaybackProvenance::Random,
            Some(ReleasedLegacyOrigin::AutoDj {}) => PlaybackProvenance::AutoDj,
            Some(ReleasedLegacyOrigin::RestoredUnknown {}) | None => PlaybackProvenance::Legacy,
        };
        let track_id = entry.track_id.clone();
        if seen_tracks.insert(track_id.clone()) {
            let fallback = tracks
                .get(&(source_id.clone(), track_id.clone()))
                .map(fallback_from_released_track)
                .or_else(|| fallback_from_legacy_entry(&source_id, &entry, cues))?;
            fallbacks.push(fallback);
        }
        occurrences.push(PlaybackOccurrence {
            id: PlaybackOccurrenceId::new(entry.id),
            track_id,
            provenance,
        });
    }
    let traversal = if snapshot.shuffle.enabled {
        legacy_traversal(&snapshot.shuffle_order, &occurrences)?
    } else {
        Vec::new()
    };
    let checkpoint = PlaybackCheckpoint {
        source_id,
        revision: 1,
        queue: PlaybackQueueSnapshot {
            occurrences,
            fallback_tracks: fallbacks,
            traversal,
        },
        state: PlaybackState {
            selected,
            progress_millis: u64::from(snapshot.progress_seconds) * 1_000,
        },
    };
    crate::playback_state::validate_checkpoint(&checkpoint)
        .ok()
        .map(|()| checkpoint)
}

fn legacy_traversal(
    order: &[usize],
    occurrences: &[PlaybackOccurrence],
) -> Option<Vec<PlaybackOccurrenceId>> {
    if order.len() != occurrences.len() {
        return None;
    }
    let mut seen = HashSet::with_capacity(order.len());
    order
        .iter()
        .map(|index| {
            if !seen.insert(*index) {
                return None;
            }
            occurrences.get(*index).map(|entry| entry.id.clone())
        })
        .collect()
}

fn fallback_from_legacy_entry(
    source_id: &SourceId,
    entry: &ReleasedLegacyEntry,
    cues: &HashMap<(SourceId, TrackId), ReleasedCue>,
) -> Option<PlaybackFallbackTrack> {
    let cue = cues.get(&(source_id.clone(), entry.track_id.clone()));
    Some(PlaybackFallbackTrack {
        id: entry.track_id.clone(),
        album_id: entry.album_id.clone(),
        primary_artist_id: entry.artist_id.clone(),
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        album: entry.album.clone(),
        year: entry.year,
        duration_seconds: entry.duration_seconds,
        favorite: entry.favorite,
        track_number: 0,
        disc_number: 0,
        image_ref: entry.image_ref.clone(),
        local_artwork: None,
        musicbrainz_recording_id: None,
        source_format: entry.source_format.clone(),
        source_path: cue
            .and_then(|cue| cue.source_path.clone())
            .or_else(|| entry.local_path.clone()),
        cue: cue.map(|cue| cue.cue.clone()),
    })
}

fn fallback_from_released_track(track: &ReleasedTrack) -> PlaybackFallbackTrack {
    PlaybackFallbackTrack {
        id: track.id.clone(),
        album_id: track.album_id.clone(),
        primary_artist_id: track.artist_id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        year: track.year,
        duration_seconds: track.duration_seconds,
        favorite: track.favorite,
        track_number: track.track_number,
        disc_number: track.disc_number,
        image_ref: track.image_ref.clone(),
        local_artwork: None,
        musicbrainz_recording_id: None,
        source_format: track.source_format.clone(),
        source_path: track.source_path.clone(),
        cue: track.cue.clone(),
    }
}

fn released_source_context(shuffle_key: &str) -> Option<PlaybackProvenance> {
    let value = shuffle_key.strip_prefix("source-shuffle|source=")?;
    let (before_track, _) = value.rsplit_once("|track=")?;
    let (context_id, source_rank) = before_track.rsplit_once("|source-index=")?;
    Some(PlaybackProvenance::Context {
        context_id: context_id.to_string(),
        source_rank: source_rank.parse().ok()?,
    })
}

fn import_local_favorites(
    source: &Connection,
    target: &mut Worker,
    accepted: &HashMap<SourceId, bool>,
    report: &mut Schema30MigrationReport,
) {
    if !has_columns(
        source,
        "item_favorite_overrides",
        &["source_id", "item_kind", "item_id", "favorite"],
    ) {
        return;
    }
    let Ok(mut statement) = source.prepare(
        "SELECT source_id, item_kind, item_id
         FROM item_favorite_overrides
         WHERE favorite = 1
         ORDER BY source_id, item_kind, item_id",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) else {
        return;
    };
    let mut seen = HashSet::new();
    for row in rows {
        let Ok((source_id, kind, item_id)) = row else {
            report.skipped_local_favorites += 1;
            continue;
        };
        if source_id.is_empty() || item_id.is_empty() {
            report.skipped_local_favorites += 1;
            continue;
        }
        let source_id = SourceId::new(source_id);
        if !accepted.get(&source_id).copied().unwrap_or(false) {
            continue;
        }
        let favorite = match kind.as_str() {
            "track" => FavoriteItemId::Track(TrackId::new(item_id)),
            "album" => FavoriteItemId::Album(AlbumId::new(item_id)),
            "artist" | "album_artist" => FavoriteItemId::Artist(ArtistId::new(item_id)),
            _ => {
                report.skipped_local_favorites += 1;
                continue;
            }
        };
        if !seen.insert((source_id.clone(), favorite.clone())) {
            continue;
        }
        match target.set_favorite(&source_id, &favorite, true, true, None::<FavoriteValue>) {
            Ok(()) => report.local_favorites += 1,
            Err(_) => report.skipped_local_favorites += 1,
        }
    }
}

fn import_local_playlists(
    source: &Connection,
    target: &mut Worker,
    accepted: &HashMap<SourceId, bool>,
    report: &mut Schema30MigrationReport,
) {
    if !has_columns(
        source,
        "playlists",
        &["source_id", "playlist_id", "name", "owner"],
    ) || !has_columns(
        source,
        "playlist_tracks",
        &[
            "source_id",
            "playlist_id",
            "entry_id",
            "track_id",
            "position",
        ],
    ) {
        return;
    }
    let Ok(mut headers) = source.prepare(
        "SELECT source_id, playlist_id, name
         FROM playlists
         WHERE owner = 'store'
         ORDER BY source_id, name COLLATE NOCASE, playlist_id",
    ) else {
        return;
    };
    let Ok(rows) = headers.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) else {
        return;
    };
    for row in rows {
        let Ok((source_id, playlist_id, name)) = row else {
            report.skipped_local_playlists += 1;
            continue;
        };
        if source_id.is_empty() || playlist_id.is_empty() || name.trim().is_empty() {
            report.skipped_local_playlists += 1;
            continue;
        }
        let source_id = SourceId::new(source_id);
        if !accepted.get(&source_id).copied().unwrap_or(false) {
            continue;
        }
        let Ok(mut entries_statement) = source.prepare(
            "SELECT entry_id, track_id
             FROM playlist_tracks
             WHERE source_id = ?1 AND playlist_id = ?2
             ORDER BY position, entry_id",
        ) else {
            report.skipped_local_playlists += 1;
            continue;
        };
        let entries = entries_statement
            .query_map(params![source_id.as_str(), &playlist_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>());
        let Ok(entries) = entries else {
            report.skipped_local_playlists += 1;
            continue;
        };
        let mut occurrences = HashSet::new();
        let mut valid = true;
        let entries = entries
            .into_iter()
            .filter_map(|(occurrence_id, track_id)| {
                if occurrence_id.is_empty()
                    || track_id.is_empty()
                    || !occurrences.insert(occurrence_id.clone())
                {
                    valid = false;
                    return None;
                }
                Some(PlaylistEntry {
                    occurrence_id,
                    track_id: TrackId::new(track_id),
                })
            })
            .collect::<Vec<_>>();
        if !valid {
            report.skipped_local_playlists += 1;
            continue;
        }
        let snapshot = PlaylistSnapshot {
            playlist: Playlist {
                id: PlaylistId::new(playlist_id),
                name,
                image_ref: None,
            },
            entries,
        };
        match target.replace_local_playlist(&source_id, snapshot) {
            Ok(()) => report.local_playlists += 1,
            Err(_) => report.skipped_local_playlists += 1,
        }
    }
}

fn import_smart_playlists(
    source: &Connection,
    target: &mut Worker,
    accepted: &HashMap<SourceId, bool>,
    report: &mut Schema30MigrationReport,
) {
    if !has_columns(
        source,
        "smart_playlists",
        &[
            "source_id",
            "smart_playlist_id",
            "name",
            "builtin_key",
            "definition_json",
            "position",
        ],
    ) {
        return;
    }
    let Ok(mut statement) = source.prepare(
        "SELECT
            source_id, smart_playlist_id, name, builtin_key, definition_json
         FROM smart_playlists
         ORDER BY source_id, position, smart_playlist_id",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    }) else {
        return;
    };
    let mut positions = HashMap::<SourceId, u32>::new();
    for row in rows {
        let Ok((source_id, playlist_id, name, builtin, definition)) = row else {
            report.skipped_smart_playlists += 1;
            continue;
        };
        if source_id.is_empty() || playlist_id.is_empty() || name.trim().is_empty() {
            report.skipped_smart_playlists += 1;
            continue;
        }
        let source_id = SourceId::new(source_id);
        if !accepted.contains_key(&source_id) {
            continue;
        }
        let builtin = match builtin {
            Some(key) => match SmartPlaylistBuiltin::from_key(&key) {
                Some(builtin) => Some(builtin),
                None => {
                    report.skipped_smart_playlists += 1;
                    continue;
                }
            },
            None => None,
        };
        let Some(definition) = released_smart_playlist_definition(&definition) else {
            report.skipped_smart_playlists += 1;
            continue;
        };
        if crate::smart_playlists::validated_smart_playlist_json(&definition).is_err() {
            report.skipped_smart_playlists += 1;
            continue;
        }
        let position = positions.entry(source_id.clone()).or_default();
        let record = SmartPlaylistRecord {
            id: SmartPlaylistId::new(playlist_id),
            name,
            position: *position,
            builtin,
            definition,
        };
        match target.put_smart_playlist(&source_id, &record) {
            Ok(()) => {
                *position = position.saturating_add(1);
                report.smart_playlists += 1;
            }
            Err(_) => report.skipped_smart_playlists += 1,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedSmartPlaylistDefinition {
    root: ReleasedSmartPlaylistRuleGroup,
    sort_field: SmartPlaylistSortField,
    descending: bool,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedSmartPlaylistRuleGroup {
    mode: ReleasedSmartPlaylistMatchMode,
    rules: Vec<ReleasedSmartPlaylistRuleNode>,
}

#[derive(Deserialize)]
enum ReleasedSmartPlaylistMatchMode {
    All,
    Any,
}

#[derive(Deserialize)]
enum ReleasedSmartPlaylistRuleNode {
    Rule(SmartPlaylistRule),
}

fn released_smart_playlist_definition(value: &str) -> Option<SmartPlaylistDefinition> {
    let released = serde_json::from_str::<ReleasedSmartPlaylistDefinition>(value).ok()?;
    let mut rules = Vec::with_capacity(released.root.rules.len());
    for node in released.root.rules {
        let ReleasedSmartPlaylistRuleNode::Rule(rule) = node;
        rules.push(rule);
    }
    let (match_all, match_any) = match released.root.mode {
        ReleasedSmartPlaylistMatchMode::All => (rules, Vec::new()),
        ReleasedSmartPlaylistMatchMode::Any => (Vec::new(), rules),
    };
    Some(SmartPlaylistDefinition {
        match_all,
        match_any,
        sort_field: released.sort_field,
        descending: released.descending,
        limit: released.limit,
    })
}

#[derive(Default)]
struct ReleasedAggregate {
    lifetime_plays: u64,
    lifetime_skips: u64,
    last_played_at: Option<i64>,
    months: HashMap<String, u64>,
}

fn import_activity(
    source: &Connection,
    target: &mut Connection,
    accepted: &HashMap<SourceId, bool>,
    tracks: &HashMap<(SourceId, TrackId), ReleasedTrack>,
    report: &mut Schema30MigrationReport,
) {
    if !has_columns(
        source,
        "track_activity_period",
        &[
            "source_id",
            "period",
            "track_id",
            "qualified_plays",
            "skips",
            "last_played_at",
        ],
    ) {
        return;
    }
    let Ok(mut statement) = source.prepare(
        "SELECT
            source_id, period, track_id, qualified_plays, skips,
            unixepoch(last_played_at)
         FROM track_activity_period
         ORDER BY source_id, track_id, period",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, Option<i64>>(5)?,
        ))
    }) else {
        return;
    };
    let mut aggregates = HashMap::<(SourceId, TrackId), ReleasedAggregate>::new();
    for row in rows {
        let Ok((source_id, period, track_id, plays, skips, last_played_at)) = row else {
            report.skipped_activity_rows += 1;
            continue;
        };
        if source_id.is_empty() || track_id.is_empty() || plays < 0 || skips < 0 {
            report.skipped_activity_rows += 1;
            continue;
        }
        let source_id = SourceId::new(source_id);
        let track_id = TrackId::new(track_id);
        let key = (source_id, track_id);
        if !accepted.contains_key(&key.0) || !tracks.contains_key(&key) {
            continue;
        }
        let is_month = valid_month(&period);
        if period != "legacy" && !is_month {
            report.skipped_activity_rows += 1;
            continue;
        }
        let aggregate = aggregates.entry(key).or_default();
        let Ok(plays) = u64::try_from(plays) else {
            report.skipped_activity_rows += 1;
            continue;
        };
        let Ok(skips) = u64::try_from(skips) else {
            report.skipped_activity_rows += 1;
            continue;
        };
        let Some(lifetime_plays) = aggregate.lifetime_plays.checked_add(plays) else {
            report.skipped_activity_rows += 1;
            continue;
        };
        let Some(lifetime_skips) = aggregate.lifetime_skips.checked_add(skips) else {
            report.skipped_activity_rows += 1;
            continue;
        };
        aggregate.lifetime_plays = lifetime_plays;
        aggregate.lifetime_skips = lifetime_skips;
        aggregate.last_played_at = aggregate.last_played_at.max(last_played_at);
        if is_month {
            let month = aggregate.months.entry(period).or_default();
            let Some(total) = month.checked_add(plays) else {
                report.skipped_activity_rows += 1;
                continue;
            };
            *month = total;
        }
    }

    let Ok(transaction) = target.transaction() else {
        report.skipped_activity_rows += aggregates.len();
        return;
    };
    for ((source_id, track_id), aggregate) in aggregates {
        let Some(track) = tracks.get(&(source_id.clone(), track_id.clone())) else {
            continue;
        };
        let Ok(plays) = i64::try_from(aggregate.lifetime_plays) else {
            report.skipped_activity_rows += 1;
            continue;
        };
        let Ok(skips) = i64::try_from(aggregate.lifetime_skips) else {
            report.skipped_activity_rows += 1;
            continue;
        };
        if transaction
            .execute(
                "INSERT INTO listening_aggregates(
                    source_id, period, item_kind, item_id, display_name,
                    display_context, play_count, skip_count, last_played_at
                 ) VALUES (?1, 'lifetime', 'track', ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    source_id.as_str(),
                    track_id.as_str(),
                    track.title,
                    track.artist,
                    plays,
                    skips,
                    aggregate.last_played_at,
                ],
            )
            .is_err()
        {
            report.skipped_activity_rows += 1;
            continue;
        }
        report.activity_rows += 1;
        for (month, plays) in aggregate.months {
            let Ok(plays) = i64::try_from(plays) else {
                report.skipped_activity_rows += 1;
                continue;
            };
            match transaction.execute(
                "INSERT INTO listening_aggregates(
                    source_id, period, item_kind, item_id, display_name,
                    display_context, play_count, skip_count, last_played_at
                 ) VALUES (?1, ?2, 'track', ?3, ?4, ?5, ?6, NULL, NULL)",
                params![
                    source_id.as_str(),
                    month,
                    track_id.as_str(),
                    track.title,
                    track.artist,
                    plays,
                ],
            ) {
                Ok(_) => report.activity_rows += 1,
                Err(_) => report.skipped_activity_rows += 1,
            }
        }
    }
    if transaction.commit().is_err() {
        report.skipped_activity_rows += report.activity_rows;
        report.activity_rows = 0;
    }
}

fn valid_month(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 7
        || bytes[4] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let year = bytes[..4]
        .iter()
        .fold(0_u16, |year, digit| year * 10 + u16::from(digit - b'0'));
    let month = (bytes[5] - b'0') * 10 + bytes[6] - b'0';
    year >= 1970 && (1..=12).contains(&month)
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn sync_file(path: &Path) -> crate::LibraryResult<()> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(StoreError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_modes_follow_the_requested_source_not_the_released_selection() {
        let connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE sources (
                    source_id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    name TEXT NOT NULL,
                    provider_payload TEXT NOT NULL
                 );
                 CREATE TABLE active_source (
                    singleton INTEGER PRIMARY KEY,
                    source_id TEXT NOT NULL
                 );
                 CREATE TABLE playback_checkpoints (
                    source_id TEXT PRIMARY KEY,
                    repeat_mode TEXT NOT NULL,
                    shuffle_enabled INTEGER NOT NULL
                 );
                 INSERT INTO sources VALUES
                    ('source-a', 'Jellyfin', 'A', '{}'),
                    ('source-b', 'OpenSubsonic', 'B', '{}');
                 INSERT INTO active_source VALUES (1, 'source-a');
                 INSERT INTO playback_checkpoints VALUES
                    ('source-a', 'One', 1),
                    ('source-b', 'All', 0);",
            )
            .expect("create released configuration");

        let configuration = read_configuration(&connection).expect("read configuration");
        let migration = Schema30Migration {
            connection,
            configuration,
        };

        assert_eq!(
            migration.configuration().active_source_id.as_ref(),
            Some(&SourceId::new("source-a"))
        );
        assert_eq!(
            migration.playback_modes(&SourceId::new("source-b")),
            Some(Schema30PlaybackModes {
                repeat: Schema30Repeat::All,
                shuffle_enabled: false,
            })
        );
    }
}
