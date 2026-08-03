//! Final Store schema.
//!
//! Routes never query these tables. The Store writes bounded source
//! candidates and hydrates one selected LoadedLibrary from the newest accepted
//! candidate.

use rusqlite::{Connection, OptionalExtension};

use super::{StoreError, StoreResult};

pub(crate) const APPLICATION_ID: i64 = 1_381_320_270;
pub(crate) const SCHEMA_VERSION: i64 = 35;
const FIRST_STORE_SCHEMA_VERSION: i64 = 32;
const MUSIC_FOLDER_ARTWORK_SCHEMA_VERSION: i64 = 33;
const FILESYSTEM_IDENTITY_SCHEMA_VERSION: i64 = 34;

struct Migration {
    from_version: i64,
    to_version: i64,
    run: fn(&Connection) -> StoreResult<()>,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        from_version: FIRST_STORE_SCHEMA_VERSION,
        to_version: MUSIC_FOLDER_ARTWORK_SCHEMA_VERSION,
        run: migrate_schema_32,
    },
    Migration {
        from_version: MUSIC_FOLDER_ARTWORK_SCHEMA_VERSION,
        to_version: FILESYSTEM_IDENTITY_SCHEMA_VERSION,
        run: migrate_schema_33,
    },
    Migration {
        from_version: FILESYSTEM_IDENTITY_SCHEMA_VERSION,
        to_version: SCHEMA_VERSION,
        run: migrate_schema_34,
    },
];

const CREATE_SCHEMA: &str = r###"-- Rufin Store schema 35.
--
-- Product routes hydrate LoadedLibrary and do not query these tables for
-- sorting or filtering.

PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

PRAGMA application_id = 1381320270; -- "RUFN"
PRAGMA user_version = 35;

-- One row is one complete or in-progress source-library candidate. The newest
-- accepted library_id is current; there is no mutable head row.
--
-- Home has its own digest because a Home-only refresh must not invalidate the
-- canonical library digest.
CREATE TABLE source_libraries (
    library_id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    input_version INTEGER NOT NULL CHECK (input_version >= 1),
    input_digest BLOB NOT NULL CHECK (length(input_digest) = 32),
    content_digest BLOB CHECK (
        content_digest IS NULL OR length(content_digest) = 32
    ),
    freshness_version INTEGER CHECK (
        freshness_version IS NULL OR freshness_version >= 1
    ),
    freshness_marker BLOB CHECK (
        freshness_marker IS NULL OR length(freshness_marker) <= 65536
    ),
    home_digest BLOB CHECK (
        home_digest IS NULL OR length(home_digest) = 32
    ),
    home_json TEXT,
    accepted_at INTEGER CHECK (accepted_at IS NULL OR accepted_at >= 0),
    CHECK (
        (freshness_version IS NULL AND freshness_marker IS NULL)
        OR (freshness_version IS NOT NULL AND freshness_marker IS NOT NULL)
    ),
    CHECK (
        (home_digest IS NULL AND home_json IS NULL)
        OR (
            home_digest IS NOT NULL
            AND home_json IS NOT NULL
            AND length(CAST(home_json AS BLOB)) <= 16777216
            AND CASE
                WHEN json_valid(home_json)
                THEN json_type(home_json) = 'object'
                ELSE 0
            END
        )
    )
) STRICT;

-- Only one unfinished candidate may exist for a source.
CREATE UNIQUE INDEX source_libraries_one_unaccepted_idx
    ON source_libraries(source_id)
    WHERE accepted_at IS NULL;

-- This is the complete current-library lookup.
CREATE INDEX source_libraries_accepted_idx
    ON source_libraries(source_id, library_id DESC)
    WHERE accepted_at IS NOT NULL;

-- Album relationships stay in one bounded object so hydration parses them
-- once and builds every reverse index in memory.
CREATE TABLE albums (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    album_id TEXT NOT NULL CHECK (album_id <> ''),
    title TEXT NOT NULL CHECK (title <> ''),
    display_artist TEXT NOT NULL,
    year INTEGER NOT NULL CHECK (year BETWEEN 0 AND 65535),
    release_date TEXT,
    date_added TEXT,
    last_played TEXT,
    play_count INTEGER CHECK (
        play_count IS NULL OR play_count BETWEEN 0 AND 4294967295
    ),
    user_rating INTEGER CHECK (
        user_rating IS NULL OR user_rating BETWEEN 0 AND 100
    ),
    favorite INTEGER NOT NULL CHECK (favorite IN (0, 1)),
    image_item_id TEXT CHECK (image_item_id IS NULL OR image_item_id <> ''),
    image_tag TEXT CHECK (image_tag IS NULL OR image_tag <> ''),
    release_types_json TEXT NOT NULL CHECK (
        length(CAST(release_types_json AS BLOB)) <= 65536
        AND CASE
            WHEN json_valid(release_types_json)
            THEN json_type(release_types_json) = 'array'
            ELSE 0
        END
    ),
    is_compilation INTEGER CHECK (
        is_compilation IS NULL OR is_compilation IN (0, 1)
    ),
    musicbrainz_release_id TEXT CHECK (
        musicbrainz_release_id IS NULL OR musicbrainz_release_id <> ''
    ),
    musicbrainz_release_group_id TEXT CHECK (
        musicbrainz_release_group_id IS NULL
        OR musicbrainz_release_group_id <> ''
    ),
    local_artwork_kind TEXT CHECK (
        local_artwork_kind IS NULL
        OR local_artwork_kind IN ('file', 'embedded')
    ),
    local_artwork_path TEXT,
    local_artwork_picture_index INTEGER,
    local_artwork_revision TEXT,
    relations_json TEXT NOT NULL CHECK (
        length(CAST(relations_json AS BLOB)) <= 1048576
        AND CASE
            WHEN json_valid(relations_json)
            THEN json_type(relations_json) = 'object'
            ELSE 0
        END
    ),
    PRIMARY KEY (library_id, album_id),
    CHECK (image_tag IS NULL OR image_item_id IS NOT NULL),
    CHECK (
        (
            local_artwork_kind IS NULL
            AND local_artwork_path IS NULL
            AND local_artwork_picture_index IS NULL
            AND local_artwork_revision IS NULL
        )
        OR (
            local_artwork_kind = 'file'
            AND local_artwork_path IS NOT NULL
            AND local_artwork_path <> ''
            AND local_artwork_picture_index IS NULL
            AND local_artwork_revision IS NOT NULL
            AND local_artwork_revision <> ''
        )
        OR (
            local_artwork_kind = 'embedded'
            AND local_artwork_path IS NOT NULL
            AND local_artwork_path <> ''
            AND local_artwork_picture_index IS NOT NULL
            AND local_artwork_picture_index >= 0
            AND local_artwork_revision IS NOT NULL
            AND local_artwork_revision <> ''
        )
    )
) STRICT;

-- Track scalar facts remain directly auditable. Only ordered relationships use
-- JSON, and that document has a fixed one-row bound.
CREATE TABLE tracks (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    track_id TEXT NOT NULL CHECK (track_id <> ''),
    album_id TEXT CHECK (album_id IS NULL OR album_id <> ''),
    title TEXT NOT NULL CHECK (title <> ''),
    display_album TEXT NOT NULL,
    display_artist TEXT NOT NULL,
    year INTEGER NOT NULL CHECK (year BETWEEN 0 AND 65535),
    release_date TEXT,
    date_added TEXT,
    last_played TEXT,
    play_count INTEGER CHECK (
        play_count IS NULL OR play_count BETWEEN 0 AND 4294967295
    ),
    skip_count INTEGER CHECK (
        skip_count IS NULL OR skip_count BETWEEN 0 AND 4294967295
    ),
    user_rating INTEGER CHECK (
        user_rating IS NULL OR user_rating BETWEEN 0 AND 100
    ),
    duration_seconds INTEGER NOT NULL CHECK (
        duration_seconds BETWEEN 0 AND 4294967295
    ),
    favorite INTEGER NOT NULL CHECK (favorite IN (0, 1)),
    disc_number INTEGER NOT NULL CHECK (disc_number BETWEEN 0 AND 65535),
    track_number INTEGER NOT NULL CHECK (track_number BETWEEN 0 AND 65535),
    image_item_id TEXT CHECK (image_item_id IS NULL OR image_item_id <> ''),
    image_tag TEXT CHECK (image_tag IS NULL OR image_tag <> ''),
    source_format TEXT,
    comment TEXT,
    bpm INTEGER CHECK (bpm IS NULL OR bpm BETWEEN 0 AND 65535),
    musicbrainz_recording_id TEXT CHECK (
        musicbrainz_recording_id IS NULL OR musicbrainz_recording_id <> ''
    ),
    musicbrainz_release_track_id TEXT CHECK (
        musicbrainz_release_track_id IS NULL
        OR musicbrainz_release_track_id <> ''
    ),
    source_path TEXT CHECK (source_path IS NULL OR source_path <> ''),
    cue_path TEXT CHECK (cue_path IS NULL OR cue_path <> ''),
    cue_start_millis INTEGER CHECK (
        cue_start_millis IS NULL OR cue_start_millis >= 0
    ),
    cue_end_millis INTEGER CHECK (
        cue_end_millis IS NULL OR cue_end_millis >= 0
    ),
    local_artwork_kind TEXT CHECK (
        local_artwork_kind IS NULL
        OR local_artwork_kind IN ('file', 'embedded')
    ),
    local_artwork_path TEXT,
    local_artwork_picture_index INTEGER,
    local_artwork_revision TEXT,
    relations_json TEXT NOT NULL CHECK (
        length(CAST(relations_json AS BLOB)) <= 1048576
        AND CASE
            WHEN json_valid(relations_json)
            THEN json_type(relations_json) = 'object'
            ELSE 0
        END
    ),
    PRIMARY KEY (library_id, track_id),
    CHECK (image_tag IS NULL OR image_item_id IS NOT NULL),
    CHECK (
        (
            cue_path IS NULL
            AND cue_start_millis IS NULL
            AND cue_end_millis IS NULL
        )
        OR (
            cue_path IS NOT NULL
            AND cue_start_millis IS NOT NULL
            AND cue_end_millis IS NOT NULL
            AND cue_end_millis > cue_start_millis
        )
    ),
    CHECK (
        (
            local_artwork_kind IS NULL
            AND local_artwork_path IS NULL
            AND local_artwork_picture_index IS NULL
            AND local_artwork_revision IS NULL
        )
        OR (
            local_artwork_kind = 'file'
            AND local_artwork_path IS NOT NULL
            AND local_artwork_path <> ''
            AND local_artwork_picture_index IS NULL
            AND local_artwork_revision IS NOT NULL
            AND local_artwork_revision <> ''
        )
        OR (
            local_artwork_kind = 'embedded'
            AND local_artwork_path IS NOT NULL
            AND local_artwork_path <> ''
            AND local_artwork_picture_index IS NOT NULL
            AND local_artwork_picture_index >= 0
            AND local_artwork_revision IS NOT NULL
            AND local_artwork_revision <> ''
        )
    )
) STRICT;

CREATE TABLE artists (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    artist_id TEXT NOT NULL CHECK (artist_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    last_played TEXT,
    play_count INTEGER CHECK (
        play_count IS NULL OR play_count BETWEEN 0 AND 4294967295
    ),
    user_rating INTEGER CHECK (
        user_rating IS NULL OR user_rating BETWEEN 0 AND 100
    ),
    favorite INTEGER NOT NULL CHECK (favorite IN (0, 1)),
    image_item_id TEXT CHECK (image_item_id IS NULL OR image_item_id <> ''),
    image_tag TEXT CHECK (image_tag IS NULL OR image_tag <> ''),
    musicbrainz_artist_id TEXT CHECK (
        musicbrainz_artist_id IS NULL OR musicbrainz_artist_id <> ''
    ),
    local_artwork_kind TEXT CHECK (
        local_artwork_kind IS NULL
        OR local_artwork_kind IN ('file', 'embedded')
    ),
    local_artwork_path TEXT,
    local_artwork_picture_index INTEGER,
    local_artwork_revision TEXT,
    PRIMARY KEY (library_id, artist_id),
    CHECK (image_tag IS NULL OR image_item_id IS NOT NULL),
    CHECK (
        (
            local_artwork_kind IS NULL
            AND local_artwork_path IS NULL
            AND local_artwork_picture_index IS NULL
            AND local_artwork_revision IS NULL
        )
        OR (
            local_artwork_kind = 'file'
            AND local_artwork_path IS NOT NULL
            AND local_artwork_path <> ''
            AND local_artwork_picture_index IS NULL
            AND local_artwork_revision IS NOT NULL
            AND local_artwork_revision <> ''
        )
        OR (
            local_artwork_kind = 'embedded'
            AND local_artwork_path IS NOT NULL
            AND local_artwork_path <> ''
            AND local_artwork_picture_index IS NOT NULL
            AND local_artwork_picture_index >= 0
            AND local_artwork_revision IS NOT NULL
            AND local_artwork_revision <> ''
        )
    )
) STRICT;

CREATE TABLE genres (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    genre_id TEXT NOT NULL CHECK (genre_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    image_item_id TEXT CHECK (image_item_id IS NULL OR image_item_id <> ''),
    image_tag TEXT CHECK (image_tag IS NULL OR image_tag <> ''),
    PRIMARY KEY (library_id, genre_id),
    CHECK (image_tag IS NULL OR image_item_id IS NOT NULL)
) STRICT;

CREATE TABLE music_folders (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    folder_id TEXT NOT NULL CHECK (folder_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    image_item_id TEXT CHECK (image_item_id IS NULL OR image_item_id <> ''),
    image_tag TEXT CHECK (image_tag IS NULL OR image_tag <> ''),
    PRIMARY KEY (library_id, folder_id),
    CHECK (image_tag IS NULL OR image_item_id IS NOT NULL)
) STRICT;

-- A playlist header exists even when it has no entries.
CREATE TABLE source_playlists (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    playlist_id TEXT NOT NULL CHECK (playlist_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    image_item_id TEXT CHECK (image_item_id IS NULL OR image_item_id <> ''),
    image_tag TEXT CHECK (image_tag IS NULL OR image_tag <> ''),
    PRIMARY KEY (library_id, playlist_id),
    CHECK (image_tag IS NULL OR image_item_id IS NOT NULL)
) STRICT;

-- Position owns order. occurrence_id distinguishes duplicate Track entries.
CREATE TABLE source_playlist_entries (
    library_id INTEGER NOT NULL,
    playlist_id TEXT NOT NULL CHECK (playlist_id <> ''),
    position INTEGER NOT NULL CHECK (position >= 0),
    occurrence_id TEXT NOT NULL CHECK (occurrence_id <> ''),
    track_id TEXT NOT NULL CHECK (track_id <> ''),
    PRIMARY KEY (library_id, playlist_id, position),
    UNIQUE (library_id, playlist_id, occurrence_id),
    FOREIGN KEY (library_id, playlist_id)
        REFERENCES source_playlists(library_id, playlist_id)
        ON DELETE NO ACTION
) STRICT;

-- Local inventory stores observations needed for exact no-op and dependency
-- decisions. Canonical item facts stay in the item tables above.
-- metadata-fallback is a usable path-backed Track. unreadable audio and invalid
-- CUE rows stay as observations and never create canonical Tracks. Automatic
-- verification retries them after a fingerprint, dependency, or parser-version
-- change; explicit Resync retries all. A parsed CUE may still name missing
-- media.
CREATE TABLE local_files (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    path TEXT NOT NULL CHECK (path <> ''),
    root TEXT NOT NULL CHECK (root <> ''),
    relative_path TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN ('audio', 'cue', 'image', 'directory')
    ),
    size_bytes INTEGER CHECK (size_bytes IS NULL OR size_bytes >= 0),
    mtime_ns INTEGER NOT NULL,
    device_id INTEGER,
    inode INTEGER,
    -- The current audio/CUE parser writes version 1. Images and directories are
    -- observed without parsing and therefore keep this null.
    parse_version INTEGER CHECK (
        parse_version IS NULL OR parse_version >= 1
    ),
    read_state TEXT NOT NULL CHECK (
        read_state IN (
            'parsed',
            'metadata-fallback',
            'unreadable',
            'invalid',
            'observed'
        )
    ),
    dependencies_json TEXT NOT NULL CHECK (
        length(CAST(dependencies_json AS BLOB)) <= 2097152
        AND CASE
            WHEN json_valid(dependencies_json)
            THEN json_type(dependencies_json) = 'array'
            ELSE 0
        END
    ),
    PRIMARY KEY (library_id, path),
    CHECK (
        (kind = 'directory' AND size_bytes IS NULL)
        OR (kind <> 'directory' AND size_bytes IS NOT NULL)
    ),
    CHECK (
        (
            kind = 'audio'
            AND parse_version IS NOT NULL
            AND read_state IN ('parsed', 'metadata-fallback', 'unreadable')
        )
        OR (
            kind = 'cue'
            AND parse_version IS NOT NULL
            AND read_state IN ('parsed', 'invalid')
        )
        OR (
            kind IN ('image', 'directory')
            AND parse_version IS NULL
            AND read_state = 'observed'
        )
    ),
    CHECK (kind = 'cue' OR dependencies_json = '[]')
) STRICT;

-- Rebuildable files used to verify a configured remote-to-local mapping.
CREATE TABLE local_access_files (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    path TEXT NOT NULL CHECK (path <> ''),
    root TEXT NOT NULL CHECK (root <> ''),
    relative_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    mtime_ns INTEGER NOT NULL,
    device_id INTEGER,
    inode INTEGER,
    parser_version INTEGER NOT NULL CHECK (parser_version >= 1),
    title TEXT NOT NULL,
    album TEXT NOT NULL,
    artist TEXT NOT NULL,
    disc_number INTEGER NOT NULL CHECK (disc_number BETWEEN 0 AND 65535),
    track_number INTEGER NOT NULL CHECK (track_number BETWEEN 0 AND 65535),
    duration_seconds INTEGER NOT NULL CHECK (
        duration_seconds BETWEEN 0 AND 4294967295
    ),
    PRIMARY KEY (source_id, path)
) STRICT;

-- Presence is the complete Local favorite value. Absence means false.
CREATE TABLE local_favorites (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    item_kind TEXT NOT NULL CHECK (
        item_kind IN ('album', 'track', 'artist')
    ),
    item_id TEXT NOT NULL CHECK (item_id <> ''),
    PRIMARY KEY (source_id, item_kind, item_id)
) STRICT;

CREATE TABLE local_playlists (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    playlist_id TEXT NOT NULL CHECK (playlist_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    PRIMARY KEY (source_id, playlist_id)
) STRICT;

CREATE TABLE local_playlist_entries (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    playlist_id TEXT NOT NULL CHECK (playlist_id <> ''),
    position INTEGER NOT NULL CHECK (position >= 0),
    occurrence_id TEXT NOT NULL CHECK (occurrence_id <> ''),
    track_id TEXT NOT NULL CHECK (track_id <> ''),
    PRIMARY KEY (source_id, playlist_id, position),
    UNIQUE (source_id, playlist_id, occurrence_id),
    FOREIGN KEY (source_id, playlist_id)
        REFERENCES local_playlists(source_id, playlist_id)
        ON DELETE NO ACTION
) STRICT;

-- Smart playlist membership is derived in LoadedLibrary. Only the user-owned
-- definition and display order are durable.
CREATE TABLE smart_playlists (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    smart_playlist_id TEXT NOT NULL CHECK (smart_playlist_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    builtin_key TEXT CHECK (
        builtin_key IS NULL
        OR builtin_key IN ('most_played', 'never_played', 'most_skipped')
    ),
    definition_json TEXT NOT NULL CHECK (
        length(CAST(definition_json AS BLOB)) <= 262144
        AND CASE
            WHEN json_valid(definition_json)
            THEN json_type(definition_json) = 'object'
            ELSE 0
        END
    ),
    position INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (source_id, smart_playlist_id),
    UNIQUE (source_id, position)
) STRICT;

CREATE UNIQUE INDEX smart_playlists_builtin_idx
    ON smart_playlists(source_id, builtin_key)
    WHERE builtin_key IS NOT NULL;

-- First-seen history survives cache replacement and needs no date index.
CREATE TABLE local_imports (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    track_id TEXT NOT NULL CHECK (track_id <> ''),
    first_seen_at INTEGER NOT NULL CHECK (first_seen_at >= 0),
    PRIMARY KEY (source_id, track_id)
) STRICT;

-- Queue structure is separate from frequent progress and current-item writes.
CREATE TABLE playback_queues (
    source_id TEXT PRIMARY KEY CHECK (source_id <> ''),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    payload_json TEXT NOT NULL CHECK (
        length(CAST(payload_json AS BLOB)) <= 268435456
        AND CASE
            WHEN json_valid(payload_json)
            THEN json_type(payload_json) = 'object'
            ELSE 0
        END
    ),
    -- SQLite requires this exact unique parent key for playback_state's
    -- deferred two-column foreign key, even though source_id is already unique.
    UNIQUE (source_id, revision)
) STRICT;

-- The deferred revision key lets one transaction replace queue structure and
-- its narrow state without exposing a mismatched pair.
CREATE TABLE playback_state (
    source_id TEXT PRIMARY KEY CHECK (source_id <> ''),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    selected_occurrence_id TEXT CHECK (
        selected_occurrence_id IS NULL OR selected_occurrence_id <> ''
    ),
    progress_millis INTEGER NOT NULL CHECK (progress_millis >= 0),
    FOREIGN KEY (source_id, revision)
        REFERENCES playback_queues(source_id, revision)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- Lifetime and monthly activity share one row shape. Yearly results sum the
-- accepted YYYY-MM rows.
CREATE TABLE listening_aggregates (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    period TEXT NOT NULL CHECK (
        period = 'lifetime'
        OR (
            length(period) = 7
            AND substr(period, 5, 1) = '-'
            AND substr(period, 1, 4) NOT GLOB '*[^0-9]*'
            AND CAST(substr(period, 1, 4) AS INTEGER) BETWEEN 1970 AND 9999
            AND substr(period, 6, 2) IN (
                '01', '02', '03', '04', '05', '06',
                '07', '08', '09', '10', '11', '12'
            )
        )
    ),
    item_kind TEXT NOT NULL CHECK (
        item_kind IN ('track', 'artist', 'genre')
    ),
    item_id TEXT NOT NULL CHECK (item_id <> ''),
    display_name TEXT NOT NULL CHECK (display_name <> ''),
    display_context TEXT,
    play_count INTEGER NOT NULL CHECK (play_count >= 0),
    skip_count INTEGER CHECK (skip_count IS NULL OR skip_count >= 0),
    last_played_at INTEGER CHECK (
        last_played_at IS NULL OR last_played_at >= 0
    ),
    PRIMARY KEY (source_id, period, item_kind, item_id),
    CHECK (
        skip_count IS NULL
        OR (period = 'lifetime' AND item_kind = 'track')
    ),
    CHECK (
        last_played_at IS NULL
        OR period = 'lifetime'
    )
) STRICT;

-- This one index is both visible order and the deterministic latest-100 trim.
CREATE TABLE recent_plays (
    play_id TEXT PRIMARY KEY CHECK (play_id <> ''),
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    track_id TEXT NOT NULL CHECK (track_id <> ''),
    track_title TEXT NOT NULL CHECK (track_title <> ''),
    artist_name TEXT NOT NULL,
    album_title TEXT,
    played_at INTEGER NOT NULL CHECK (played_at >= 0)
) STRICT;

CREATE INDEX recent_plays_source_time_idx
    ON recent_plays(source_id, played_at DESC, play_id DESC);

CREATE TABLE pending_scrobbles (
    service TEXT NOT NULL CHECK (
        service IN ('lastfm', 'librefm', 'listenbrainz')
    ),
    account_id TEXT NOT NULL CHECK (account_id <> ''),
    play_id TEXT NOT NULL CHECK (play_id <> ''),
    track_title TEXT NOT NULL CHECK (track_title <> ''),
    artist_name TEXT NOT NULL CHECK (artist_name <> ''),
    album_title TEXT,
    duration_millis INTEGER NOT NULL CHECK (duration_millis >= 0),
    started_at INTEGER NOT NULL CHECK (started_at >= 0),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at INTEGER CHECK (
        next_attempt_at IS NULL OR next_attempt_at >= 0
    ),
    last_error TEXT CHECK (last_error IS NULL OR last_error <> ''),
    PRIMARY KEY (service, account_id, play_id),
    CHECK (
        (next_attempt_at IS NULL AND last_error IS NOT NULL)
        OR (next_attempt_at IS NOT NULL AND last_error IS NULL)
    )
) STRICT;

CREATE INDEX pending_scrobbles_due_idx
    ON pending_scrobbles(
        service,
        account_id,
        next_attempt_at,
        started_at,
        play_id
    )
    WHERE next_attempt_at IS NOT NULL;

-- Empty language and script are the canonical absent values, so the composite
-- primary key cannot admit duplicate NULL identities.
CREATE TABLE lyrics_cache (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    track_id TEXT NOT NULL CHECK (track_id <> ''),
    role TEXT NOT NULL CHECK (
        role <> '' AND length(CAST(role AS BLOB)) <= 64
    ),
    language TEXT NOT NULL DEFAULT '' CHECK (
        length(CAST(language AS BLOB)) <= 64
    ),
    script TEXT NOT NULL DEFAULT '' CHECK (
        length(CAST(script AS BLOB)) <= 64
    ),
    origin TEXT NOT NULL CHECK (origin IN ('source', 'external')),
    input_version INTEGER NOT NULL CHECK (input_version >= 1),
    input_digest BLOB NOT NULL CHECK (length(input_digest) = 32),
    payload TEXT NOT NULL CHECK (
        length(CAST(payload AS BLOB)) <= 8388608
    ),
    cached_at INTEGER NOT NULL CHECK (cached_at >= 0),
    PRIMARY KEY (source_id, track_id, role, language, script)
) STRICT;

CREATE INDEX lyrics_cache_eviction_idx
    ON lyrics_cache(cached_at);

-- One row is the current found or missing outcome for one Album. A changed
-- exact identity replaces it, so obsolete identities do not accumulate.
-- LoadedLibrary overlays a found result without mutating source facts.
CREATE TABLE album_release_info (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    album_id TEXT NOT NULL CHECK (album_id <> ''),
    exact_identity_key TEXT NOT NULL CHECK (exact_identity_key <> ''),
    lookup_state TEXT NOT NULL CHECK (
        lookup_state IN ('found', 'missing')
    ),
    release_types_json TEXT CHECK (
        release_types_json IS NULL
        OR length(CAST(release_types_json AS BLOB)) <= 65536
    ),
    is_compilation INTEGER CHECK (
        is_compilation IS NULL OR is_compilation IN (0, 1)
    ),
    PRIMARY KEY (source_id, album_id),
    CHECK (
        (
            lookup_state = 'missing'
            AND release_types_json IS NULL
            AND is_compilation IS NULL
        )
        OR (
            lookup_state = 'found'
            AND release_types_json IS NOT NULL
            AND CASE
                WHEN json_valid(release_types_json)
                THEN json_type(release_types_json) = 'array'
                    AND json_array_length(release_types_json) > 0
                ELSE 0
            END
        )
    )
) STRICT;

COMMIT;
"###;

const MIGRATE_SCHEMA_32: &str = r###"
BEGIN IMMEDIATE;

ALTER TABLE music_folders RENAME TO schema_32_music_folders;

CREATE TABLE music_folders (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    folder_id TEXT NOT NULL CHECK (folder_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    image_item_id TEXT CHECK (image_item_id IS NULL OR image_item_id <> ''),
    image_tag TEXT CHECK (image_tag IS NULL OR image_tag <> ''),
    PRIMARY KEY (library_id, folder_id),
    CHECK (image_tag IS NULL OR image_item_id IS NOT NULL)
) STRICT;

INSERT INTO music_folders(library_id, folder_id, name)
SELECT library_id, folder_id, name
FROM schema_32_music_folders;

DROP TABLE schema_32_music_folders;

PRAGMA user_version = 33;

COMMIT;
"###;

const MIGRATE_SCHEMA_33: &str = r###"
BEGIN IMMEDIATE;

ALTER TABLE local_files RENAME TO schema_33_local_files;

CREATE TABLE local_files (
    library_id INTEGER NOT NULL
        REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
    path TEXT NOT NULL CHECK (path <> ''),
    root TEXT NOT NULL CHECK (root <> ''),
    relative_path TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN ('audio', 'cue', 'image', 'directory')
    ),
    size_bytes INTEGER CHECK (size_bytes IS NULL OR size_bytes >= 0),
    mtime_ns INTEGER NOT NULL,
    device_id INTEGER,
    inode INTEGER,
    -- The current audio/CUE parser writes version 1. Images and directories are
    -- observed without parsing and therefore keep this null.
    parse_version INTEGER CHECK (
        parse_version IS NULL OR parse_version >= 1
    ),
    read_state TEXT NOT NULL CHECK (
        read_state IN (
            'parsed',
            'metadata-fallback',
            'unreadable',
            'invalid',
            'observed'
        )
    ),
    dependencies_json TEXT NOT NULL CHECK (
        length(CAST(dependencies_json AS BLOB)) <= 2097152
        AND CASE
            WHEN json_valid(dependencies_json)
            THEN json_type(dependencies_json) = 'array'
            ELSE 0
        END
    ),
    PRIMARY KEY (library_id, path),
    CHECK (
        (kind = 'directory' AND size_bytes IS NULL)
        OR (kind <> 'directory' AND size_bytes IS NOT NULL)
    ),
    CHECK (
        (
            kind = 'audio'
            AND parse_version IS NOT NULL
            AND read_state IN ('parsed', 'metadata-fallback', 'unreadable')
        )
        OR (
            kind = 'cue'
            AND parse_version IS NOT NULL
            AND read_state IN ('parsed', 'invalid')
        )
        OR (
            kind IN ('image', 'directory')
            AND parse_version IS NULL
            AND read_state = 'observed'
        )
    ),
    CHECK (kind = 'cue' OR dependencies_json = '[]')
) STRICT;

INSERT INTO local_files(
    library_id, path, root, relative_path, kind, size_bytes, mtime_ns,
    device_id, inode, parse_version, read_state, dependencies_json
)
SELECT
    library_id, path, root, relative_path, kind, size_bytes, mtime_ns,
    device_id, inode, parse_version, read_state, dependencies_json
FROM schema_33_local_files;

DROP TABLE schema_33_local_files;

ALTER TABLE local_access_files RENAME TO schema_33_local_access_files;

CREATE TABLE local_access_files (
    source_id TEXT NOT NULL CHECK (source_id <> ''),
    path TEXT NOT NULL CHECK (path <> ''),
    root TEXT NOT NULL CHECK (root <> ''),
    relative_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    mtime_ns INTEGER NOT NULL,
    device_id INTEGER,
    inode INTEGER,
    parser_version INTEGER NOT NULL CHECK (parser_version >= 1),
    title TEXT NOT NULL,
    album TEXT NOT NULL,
    artist TEXT NOT NULL,
    disc_number INTEGER NOT NULL CHECK (disc_number BETWEEN 0 AND 65535),
    track_number INTEGER NOT NULL CHECK (track_number BETWEEN 0 AND 65535),
    duration_seconds INTEGER NOT NULL CHECK (
        duration_seconds BETWEEN 0 AND 4294967295
    ),
    PRIMARY KEY (source_id, path)
) STRICT;

INSERT INTO local_access_files(
    source_id, path, root, relative_path, size_bytes, mtime_ns,
    device_id, inode, parser_version, title, album, artist,
    disc_number, track_number, duration_seconds
)
SELECT
    source_id, path, root, relative_path, size_bytes, mtime_ns,
    device_id, inode, parser_version, title, album, artist,
    disc_number, track_number, duration_seconds
FROM schema_33_local_access_files;

DROP TABLE schema_33_local_access_files;

PRAGMA user_version = 34;

COMMIT;
"###;

fn migrate_schema_32(connection: &Connection) -> StoreResult<()> {
    connection.execute_batch(MIGRATE_SCHEMA_32)?;
    Ok(())
}

fn migrate_schema_33(connection: &Connection) -> StoreResult<()> {
    connection.execute_batch(MIGRATE_SCHEMA_33)?;
    Ok(())
}

pub(crate) fn initialize(connection: &Connection) -> StoreResult<()> {
    connection.pragma_update(None, "foreign_keys", true)?;
    let application_id = pragma_i64(connection, "application_id")?;
    let mut user_version = pragma_i64(connection, "user_version")?;
    let has_schema = connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if (application_id, user_version, has_schema) == (0, 0, false) {
        connection.execute_batch(CREATE_SCHEMA)?;
    } else if application_id == APPLICATION_ID && has_schema {
        while user_version != SCHEMA_VERSION {
            let Some(migration) = MIGRATIONS
                .iter()
                .find(|migration| migration.from_version == user_version)
            else {
                return Err(StoreError::UnsupportedSchema {
                    application_id,
                    user_version,
                });
            };
            (migration.run)(connection)?;
            user_version = pragma_i64(connection, "user_version")?;
            if user_version != migration.to_version {
                return Err(StoreError::InvalidFinalSchema(format!(
                    "schema {} migration produced schema {user_version} instead of {}",
                    migration.from_version, migration.to_version
                )));
            }
        }
    } else {
        return Err(StoreError::UnsupportedSchema {
            application_id,
            user_version,
        });
    }

    validate(connection)
}

fn migrate_schema_34(connection: &Connection) -> StoreResult<()> {
    let transaction = connection.unchecked_transaction()?;
    backfill_recent_plays(&transaction)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn backfill_recent_plays(connection: &Connection) -> StoreResult<()> {
    connection.execute(
        "INSERT OR IGNORE INTO recent_plays(
             play_id, source_id, track_id, track_title, artist_name,
             album_title, played_at
         )
         SELECT printf(
                    'activity-last-play:%d:%s:%d:%s:%d',
                    length(source_id), source_id,
                    length(item_id), item_id,
                    last_played_at
                ),
                source_id, item_id, display_name,
                COALESCE(display_context, ''), NULL, last_played_at
         FROM (
             SELECT source_id, item_id, display_name, display_context,
                    last_played_at,
                    row_number() OVER (
                        PARTITION BY source_id
                        ORDER BY last_played_at DESC, item_id DESC
                    ) AS position
             FROM listening_aggregates
             WHERE period = 'lifetime'
               AND item_kind = 'track'
               AND last_played_at IS NOT NULL
         ) AS activity
         WHERE position <= 100
           AND NOT EXISTS (
               SELECT 1 FROM recent_plays
               WHERE recent_plays.source_id = activity.source_id
                 AND recent_plays.track_id = activity.item_id
                 AND recent_plays.played_at = activity.last_played_at
           )",
        [],
    )?;
    connection.execute(
        "DELETE FROM recent_plays
         WHERE play_id IN (
             SELECT play_id
             FROM (
                 SELECT play_id,
                        row_number() OVER (
                            PARTITION BY source_id
                            ORDER BY played_at DESC, play_id DESC
                        ) AS position
                 FROM recent_plays
             )
             WHERE position > 100
        )",
        [],
    )?;
    Ok(())
}

pub(crate) fn validate(connection: &Connection) -> StoreResult<()> {
    let application_id = pragma_i64(connection, "application_id")?;
    let user_version = pragma_i64(connection, "user_version")?;
    if application_id != APPLICATION_ID || user_version != SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema {
            application_id,
            user_version,
        });
    }

    let reference = Connection::open_in_memory()?;
    reference.pragma_update(None, "foreign_keys", true)?;
    reference.execute_batch(CREATE_SCHEMA)?;
    let expected = schema_inventory(&reference)?;
    let actual = schema_inventory(connection)?;
    if actual != expected {
        let mismatch = actual
            .iter()
            .zip(&expected)
            .position(|(actual, expected)| actual != expected)
            .map_or_else(
                || {
                    format!(
                        "object count {} instead of {}",
                        actual.len(),
                        expected.len()
                    )
                },
                |index| {
                    format!(
                        "object {} differs from the final schema",
                        actual
                            .get(index)
                            .map_or("<missing>", |object| object.1.as_str())
                    )
                },
            );
        return Err(StoreError::InvalidFinalSchema(format!(
            "schema inventory mismatch: {mismatch}"
        )));
    }

    let foreign_keys: i64 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(StoreError::InvalidFinalSchema(
            "foreign keys are disabled".to_string(),
        ));
    }
    Ok(())
}

type SchemaObject = (String, String, String, Option<String>);

fn schema_inventory(connection: &Connection) -> StoreResult<Vec<SchemaObject>> {
    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql
         FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%'
           AND type IN ('table', 'index', 'trigger', 'view')
         ORDER BY type, name",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn pragma_i64(connection: &Connection, name: &str) -> rusqlite::Result<i64> {
    connection.pragma_query_value(None, name, |row| row.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_registry_covers_every_supported_store_version() {
        let mut expected = FIRST_STORE_SCHEMA_VERSION;
        for migration in MIGRATIONS {
            assert_eq!(migration.from_version, expected);
            assert_eq!(migration.to_version, migration.from_version + 1);
            expected = migration.to_version;
        }
        assert_eq!(expected, SCHEMA_VERSION);
    }

    #[test]
    fn schema_33_filesystem_identities_are_preserved_when_the_range_checks_are_removed() {
        let connection = Connection::open_in_memory().expect("open Store");
        initialize(&connection).expect("initialize current Store");
        connection
            .execute(
                "INSERT INTO source_libraries(
                    source_id, input_version, input_digest, accepted_at
                 ) VALUES (?1, 1, ?2, 1)",
                rusqlite::params!["local:test", vec![1_u8; 32]],
            )
            .expect("write source library");
        connection
            .execute(
                "INSERT INTO local_files(
                    library_id, path, root, relative_path, kind, size_bytes,
                    mtime_ns, device_id, inode, parse_version, read_state,
                    dependencies_json
                 ) VALUES (
                    1, '/music/Track.flac', '/music', 'Track.flac', 'audio',
                    1024, 1, 7, 11, 1, 'parsed', '[]'
                 )",
                [],
            )
            .expect("write Local file");
        connection
            .execute(
                "INSERT INTO local_access_files(
                    source_id, path, root, relative_path, size_bytes, mtime_ns,
                    device_id, inode, parser_version, title, album, artist,
                    disc_number, track_number, duration_seconds
                 ) VALUES (
                    'local:test', '/music/Access.flac', '/music', 'Access.flac',
                    2048, 2, 13, 17, 1, 'Track', 'Album', 'Artist', 1, 2, 180
                 )",
                [],
            )
            .expect("write Local access file");
        connection
            .pragma_update(None, "user_version", MUSIC_FOLDER_ARTWORK_SCHEMA_VERSION)
            .expect("prepare schema 33 Store");

        initialize(&connection).expect("migrate schema 33 Store");

        let local_identity = connection
            .query_row("SELECT device_id, inode FROM local_files", [], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("read migrated Local identity");
        let access_identity = connection
            .query_row(
                "SELECT device_id, inode FROM local_access_files",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("read migrated Local access identity");
        assert_eq!(local_identity, (7, 11));
        assert_eq!(access_identity, (13, 17));
    }

    #[test]
    fn schema_34_backfills_history_from_the_activity_used_by_most_played() {
        let connection = Connection::open_in_memory().expect("open Store");
        initialize(&connection).expect("initialize current Store");
        connection
            .execute(
                "INSERT INTO listening_aggregates(
                     source_id, period, item_kind, item_id, display_name,
                     display_context, play_count, skip_count, last_played_at
                 ) VALUES (
                     'local:test', 'lifetime', 'track', 'track:old',
                     'Old Track', 'Old Artist', 3, 0, 1784887200
                 )",
                [],
            )
            .expect("write migrated activity");
        connection
            .execute(
                "INSERT INTO recent_plays(
                     play_id, source_id, track_id, track_title, artist_name,
                     album_title, played_at
                 ) VALUES (
                     'current-play', 'local:test', 'track:new', 'New Track',
                     'New Artist', 'New Album', 1800000000
                 )",
                [],
            )
            .expect("write a play recorded after migration");
        connection
            .pragma_update(None, "user_version", FILESYSTEM_IDENTITY_SCHEMA_VERSION)
            .expect("prepare schema 34 Store");

        initialize(&connection).expect("migrate schema 34 Store");

        let history = connection
            .prepare(
                "SELECT track_id, track_title, artist_name, played_at
                 FROM recent_plays
                 ORDER BY played_at",
            )
            .expect("prepare History read")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .expect("read History")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect History");
        assert_eq!(
            history,
            vec![
                (
                    "track:old".to_string(),
                    "Old Track".to_string(),
                    "Old Artist".to_string(),
                    1_784_887_200,
                ),
                (
                    "track:new".to_string(),
                    "New Track".to_string(),
                    "New Artist".to_string(),
                    1_800_000_000,
                ),
            ]
        );
        assert_eq!(
            pragma_i64(&connection, "user_version").expect("read Store version"),
            SCHEMA_VERSION
        );

        initialize(&connection).expect("reopen migrated Store");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM recent_plays", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count History after reopening"),
            2
        );
    }

    #[test]
    fn schema_34_history_backfill_keeps_the_latest_100_rows_per_source() {
        let connection = Connection::open_in_memory().expect("open Store");
        initialize(&connection).expect("initialize current Store");
        for number in 0..110 {
            connection
                .execute(
                    "INSERT INTO listening_aggregates(
                         source_id, period, item_kind, item_id, display_name,
                         display_context, play_count, skip_count, last_played_at
                     ) VALUES (
                         'local:test', 'lifetime', 'track', ?1, ?2,
                         'Artist', 1, 0, ?3
                     )",
                    rusqlite::params![
                        format!("track:{number}"),
                        format!("Track {number}"),
                        number,
                    ],
                )
                .expect("write migrated activity");
        }
        connection
            .pragma_update(None, "user_version", FILESYSTEM_IDENTITY_SCHEMA_VERSION)
            .expect("prepare schema 34 Store");

        initialize(&connection).expect("migrate schema 34 Store");

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*), MIN(played_at), MAX(played_at)
                     FROM recent_plays
                     WHERE source_id = 'local:test'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .expect("read bounded History"),
            (100, 10, 109)
        );
    }

    #[test]
    fn schema_32_music_folders_are_preserved_when_cover_columns_are_added() {
        let connection = Connection::open_in_memory().expect("open Store");
        initialize(&connection).expect("initialize current Store");
        connection
            .execute(
                "INSERT INTO source_libraries(
                    source_id, input_version, input_digest, accepted_at
                 ) VALUES (?1, 1, ?2, 1)",
                rusqlite::params!["source:test", vec![1_u8; 32]],
            )
            .expect("write source library");
        connection
            .execute(
                "INSERT INTO music_folders(library_id, folder_id, name)
                 VALUES (1, 'folder:test', 'Music')",
                [],
            )
            .expect("write music folder");
        connection
            .execute_batch(
                "ALTER TABLE music_folders RENAME TO schema_33_music_folders;
                 CREATE TABLE music_folders (
                     library_id INTEGER NOT NULL
                         REFERENCES source_libraries(library_id) ON DELETE NO ACTION,
                     folder_id TEXT NOT NULL CHECK (folder_id <> ''),
                     name TEXT NOT NULL CHECK (name <> ''),
                     PRIMARY KEY (library_id, folder_id)
                 ) STRICT;
                 INSERT INTO music_folders(library_id, folder_id, name)
                 SELECT library_id, folder_id, name
                 FROM schema_33_music_folders;
                 DROP TABLE schema_33_music_folders;
                 PRAGMA user_version = 32;",
            )
            .expect("prepare schema 32 Store");

        assert_eq!(
            pragma_i64(&connection, "user_version").expect("read prepared Store version"),
            FIRST_STORE_SCHEMA_VERSION
        );

        initialize(&connection).expect("migrate schema 32 Store");

        let folder = connection
            .query_row(
                "SELECT folder_id, name, image_item_id, image_tag
                 FROM music_folders",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .expect("read migrated music folder");
        assert_eq!(
            folder,
            ("folder:test".to_string(), "Music".to_string(), None, None)
        );
    }
}
