use super::servers::*;
use super::*;
use std::time::Duration;

const SCHEMA_MIGRATIONS: &[SchemaMigration] = &[SchemaMigration {
    from_version: MIN_SUPPORTED_SCHEMA_VERSION,
    run: migrate_to_genre_duration_schema,
}];
const MIN_SUPPORTED_SCHEMA_VERSION: i64 = 18;
const SCHEMA_TABLES: &[&str] = &[
    "queue_snapshots",
    "servers",
    "server_local_access",
    "server_music_folders",
    "track_music_folders",
    "track_local_matches",
    "track_activity",
    "server_library_preferences",
    "active_server",
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
    "album_artist_links",
    "track_artist_links",
    "playlist_tracks",
    "home_section_items",
    "home_section_prefetch_items",
    "lyrics_cache",
    "cover_cache",
    "external_image_lookup_misses",
    "library_fts",
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
    "content_cache_entries",
];
const SUPPORTED_SCHEMA_COLUMNS: &[(&str, &str)] = &[
    ("albums", "release_types_json"),
    ("albums", "musicbrainz_album_id"),
    ("albums", "musicbrainz_release_group_id"),
    ("tracks", "source_format"),
    ("tracks", "comment"),
    ("tracks", "skip_count"),
    ("playlists", "top_genres_json"),
    ("local_track_manifest_data", "musicbrainz_album_id"),
    ("local_track_manifest_data", "musicbrainz_release_group_id"),
    ("source_objects", "cue_path"),
    ("source_objects", "cue_track_index"),
    ("source_objects", "segment_start_ms"),
    ("source_objects", "segment_end_ms"),
    ("entity_content_refs", "content_kind"),
    ("entity_links", "namespace"),
    ("content_cache_entries", "cache_scope"),
    ("albums", "image_origin"),
    ("tracks", "image_origin"),
    ("artists", "image_origin"),
    ("album_artists", "image_origin"),
    ("genres", "image_origin"),
    ("playlists", "image_origin"),
];
const CURRENT_SCHEMA_COLUMNS: &[(&str, &str)] = &[("genres", "duration_seconds")];
const IMAGE_ORIGIN_TABLES: &[&str] = &[
    "albums",
    "tracks",
    "artists",
    "album_artists",
    "genres",
    "playlists",
];

struct SchemaMigration {
    from_version: i64,
    run: fn(&Store) -> StoreResult<()>,
}

impl SchemaMigration {
    fn to_version(&self) -> i64 {
        self.from_version + 1
    }
}

fn schema_migration_path_from(version: i64) -> Option<Vec<&'static SchemaMigration>> {
    schema_migration_path(version, SCHEMA_VERSION, SCHEMA_MIGRATIONS)
}

fn schema_migration_path(
    mut version: i64,
    target_version: i64,
    migrations: &'static [SchemaMigration],
) -> Option<Vec<&'static SchemaMigration>> {
    if version > target_version {
        return None;
    }
    let mut path = Vec::new();
    while version < target_version {
        let migration = migrations
            .iter()
            .find(|migration| migration.from_version == version)?;
        version = migration.to_version();
        path.push(migration);
    }
    Some(path)
}

fn migrate_to_genre_duration_schema(store: &Store) -> StoreResult<()> {
    store.ensure_column("genres", "duration_seconds", "INTEGER NOT NULL DEFAULT 0")?;
    store.connection.execute(
        "
        UPDATE genres
        SET duration_seconds = COALESCE((
            SELECT SUM(t.duration_seconds)
            FROM track_genres tg
            JOIN tracks t
                ON t.server_id = tg.server_id AND t.track_id = tg.track_id
            WHERE tg.server_id = genres.server_id
              AND tg.genre_name = genres.name
        ), 0)
        ",
        [],
    )?;
    Ok(())
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        let mut store = Self::open_file(path)?;
        if store.needs_reset()? {
            drop(store);
            reset_database_files(path)?;
            store = Self::open_file(path)?;
        }
        store.migrate()?;
        Ok(store)
    }
    pub fn open_memory() -> StoreResult<Self> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.configure_pragmas(true)?;
        store.initialize_schema()?;
        Ok(store)
    }
    pub fn open_fast_read(path: impl AsRef<Path>) -> StoreResult<Self> {
        let connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::ZERO)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { connection })
    }
    pub fn migrate(&self) -> StoreResult<()> {
        if !self.database_has_objects()? {
            return self.initialize_schema();
        }
        let version = self.schema_version()?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchemaVersion(version));
        }
        if version < SCHEMA_VERSION && !self.schema_is_complete_for_version(version)? {
            return Err(StoreError::IncompleteSchemaVersion(version));
        }
        let Some(migrations) = schema_migration_path_from(version) else {
            return Err(StoreError::UnsupportedSchemaVersion(version));
        };
        if !migrations.is_empty() {
            self.connection.execute_batch("BEGIN IMMEDIATE")?;
            let migration_result = (|| {
                for migration in migrations {
                    (migration.run)(self)?;
                    self.connection
                        .pragma_update(None, "user_version", migration.to_version())?;
                }
                Ok(())
            })();
            if let Err(error) = migration_result {
                let _rollback_result = self.connection.execute_batch("ROLLBACK");
                return Err(error);
            }
            self.connection.execute_batch("COMMIT")?;
        }
        self.initialize_schema()
    }
    pub(super) fn open_file(path: &Path) -> StoreResult<Self> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.configure_pragmas(true)?;
        Ok(store)
    }
    pub(super) fn needs_reset(&self) -> StoreResult<bool> {
        if !self.database_has_objects()? {
            return Ok(false);
        }
        let version = self.schema_version()?;
        if version > SCHEMA_VERSION {
            return Ok(true);
        }
        let schema_complete = if version == SCHEMA_VERSION {
            self.current_schema_is_complete()?
        } else {
            self.schema_is_complete_for_version(version)?
        };
        if !schema_complete {
            return Ok(true);
        }
        Ok(schema_migration_path_from(version).is_none())
    }
    pub(super) fn database_has_objects(&self) -> StoreResult<bool> {
        let exists = self.connection.query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM sqlite_master
                WHERE name NOT LIKE 'sqlite_%'
            )
            ",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(exists)
    }
    pub(super) fn current_schema_is_complete(&self) -> StoreResult<bool> {
        self.schema_is_complete_for_version(SCHEMA_VERSION)
    }
    fn schema_is_complete_for_version(&self, version: i64) -> StoreResult<bool> {
        match version {
            MIN_SUPPORTED_SCHEMA_VERSION => {
                self.schema_has_required_parts(SCHEMA_TABLES, SUPPORTED_SCHEMA_COLUMNS)
            }
            SCHEMA_VERSION => Ok(self
                .schema_has_required_parts(SCHEMA_TABLES, SUPPORTED_SCHEMA_COLUMNS)?
                && self.schema_has_required_parts(&[], CURRENT_SCHEMA_COLUMNS)?),
            _ => Ok(false),
        }
    }
    fn schema_has_required_parts(
        &self,
        tables: &[&str],
        columns: &[(&str, &str)],
    ) -> StoreResult<bool> {
        for table in tables {
            if !self.table_exists(table)? {
                return Ok(false);
            }
        }
        for (table, column) in columns {
            if !self.table_has_column(table, column)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn create_local_manifest_schema(&self) -> StoreResult<()> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS local_file_manifest (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                manifest_version INTEGER NOT NULL,
                path TEXT NOT NULL,
                root_path TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                mtime_seconds INTEGER NOT NULL,
                mtime_nanos INTEGER NOT NULL DEFAULT 0,
                inode INTEGER,
                device INTEGER,
                content_hash TEXT,
                track_id TEXT NOT NULL,
                album_id TEXT NOT NULL,
                source_format TEXT,
                metadata_hash TEXT NOT NULL,
                search_hash TEXT NOT NULL,
                artwork_revision TEXT,
                scan_generation INTEGER NOT NULL,
                last_tag_read_at TEXT,
                last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, path)
            );
            CREATE TABLE IF NOT EXISTS local_track_manifest_data (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                manifest_version INTEGER NOT NULL,
                track_id TEXT NOT NULL,
                track_json TEXT NOT NULL,
                album_artist TEXT NOT NULL,
                musicbrainz_album_id TEXT,
                musicbrainz_release_group_id TEXT,
                cover_kind TEXT,
                cover_path TEXT,
                cover_embedded_index INTEGER,
                cover_revision TEXT,
                metadata_hash TEXT NOT NULL,
                search_hash TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, track_id)
            );
            CREATE TABLE IF NOT EXISTS local_artwork_manifest (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                cover_item_id TEXT NOT NULL,
                manifest_version INTEGER NOT NULL,
                source_kind TEXT NOT NULL,
                source_path TEXT NOT NULL,
                source_size INTEGER,
                mtime_seconds INTEGER,
                mtime_nanos INTEGER,
                content_hash TEXT,
                revision TEXT NOT NULL,
                scan_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, cover_item_id)
            );
            CREATE INDEX IF NOT EXISTS local_file_manifest_track_idx
                ON local_file_manifest(server_id, track_id);
            CREATE INDEX IF NOT EXISTS local_file_manifest_album_idx
                ON local_file_manifest(server_id, album_id);
            CREATE INDEX IF NOT EXISTS local_file_manifest_generation_idx
                ON local_file_manifest(server_id, scan_generation);
            CREATE INDEX IF NOT EXISTS local_file_manifest_root_idx
                ON local_file_manifest(server_id, root_path);
            CREATE INDEX IF NOT EXISTS local_artwork_manifest_source_idx
                ON local_artwork_manifest(server_id, source_path);
            ",
        )?;
        self.ensure_column("local_track_manifest_data", "musicbrainz_album_id", "TEXT")?;
        self.ensure_column(
            "local_track_manifest_data",
            "musicbrainz_release_group_id",
            "TEXT",
        )?;
        Ok(())
    }
    pub(super) fn initialize_schema(&self) -> StoreResult<()> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS queue_snapshots (
                server_id TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS servers (
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
            CREATE TABLE IF NOT EXISTS server_local_access (
                server_id TEXT PRIMARY KEY REFERENCES servers(server_id) ON DELETE CASCADE,
                root_path TEXT NOT NULL,
                path_replace_from TEXT,
                path_replace_to TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS server_music_folders (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                folder_id TEXT NOT NULL,
                name TEXT NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, folder_id)
            );
            CREATE TABLE IF NOT EXISTS track_music_folders (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, track_id, folder_id)
            );
            CREATE TABLE IF NOT EXISTS track_local_matches (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                local_path TEXT NOT NULL,
                match_kind TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, track_id)
            );
            CREATE TABLE IF NOT EXISTS track_activity (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                play_count INTEGER NOT NULL DEFAULT 0,
                last_played TEXT,
                skip_count INTEGER NOT NULL DEFAULT 0,
                play_recorded_session TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, track_id)
            );
            CREATE TABLE IF NOT EXISTS server_library_preferences (
                server_id TEXT PRIMARY KEY REFERENCES servers(server_id) ON DELETE CASCADE,
                selected_music_folder_id TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS active_server (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS sync_state (
                server_id TEXT PRIMARY KEY REFERENCES servers(server_id) ON DELETE CASCADE,
                generation INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'idle',
                last_started_at TEXT,
                last_completed_at TEXT,
                last_error TEXT
            );
            CREATE TABLE IF NOT EXISTS albums (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                album_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                artist_id TEXT,
                year INTEGER NOT NULL,
                release_date TEXT,
                date_added TEXT,
                last_played TEXT,
                play_count INTEGER,
                user_rating INTEGER,
                track_count INTEGER NOT NULL,
                duration_seconds INTEGER NOT NULL,
                favorite INTEGER NOT NULL,
                color_seed INTEGER NOT NULL,
                image_item_id TEXT,
                image_tag TEXT,
                image_origin TEXT NOT NULL DEFAULT 'unknown' CHECK (image_origin IN ('unknown', 'source', 'fallback', 'external')),
                release_types_json TEXT NOT NULL DEFAULT '[]',
                is_compilation INTEGER,
                musicbrainz_album_id TEXT,
                musicbrainz_release_group_id TEXT,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, album_id)
            );
            CREATE TABLE IF NOT EXISTS tracks (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                album_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                artist_id TEXT,
                album TEXT NOT NULL,
                year INTEGER NOT NULL,
                release_date TEXT,
                date_added TEXT,
                last_played TEXT,
                play_count INTEGER,
                user_rating INTEGER,
                duration_seconds INTEGER NOT NULL,
                favorite INTEGER NOT NULL,
                disc_number INTEGER NOT NULL,
                track_number INTEGER NOT NULL,
                image_item_id TEXT,
                image_tag TEXT,
                image_origin TEXT NOT NULL DEFAULT 'unknown' CHECK (image_origin IN ('unknown', 'source', 'fallback', 'external')),
                local_path TEXT,
                source_format TEXT,
                comment TEXT,
                skip_count INTEGER,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, track_id)
            );
            CREATE TABLE IF NOT EXISTS artists (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                artist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                album_count INTEGER NOT NULL,
                track_count INTEGER NOT NULL,
                favorite INTEGER NOT NULL,
                last_played TEXT,
                play_count INTEGER,
                user_rating INTEGER,
                image_item_id TEXT,
                image_tag TEXT,
                image_origin TEXT NOT NULL DEFAULT 'unknown' CHECK (image_origin IN ('unknown', 'source', 'fallback', 'external')),
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, artist_id)
            );
            CREATE TABLE IF NOT EXISTS album_artists (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                artist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                album_count INTEGER NOT NULL,
                track_count INTEGER NOT NULL,
                favorite INTEGER NOT NULL,
                last_played TEXT,
                play_count INTEGER,
                user_rating INTEGER,
                image_item_id TEXT,
                image_tag TEXT,
                image_origin TEXT NOT NULL DEFAULT 'unknown' CHECK (image_origin IN ('unknown', 'source', 'fallback', 'external')),
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, artist_id)
            );
            CREATE TABLE IF NOT EXISTS genres (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                genre_id TEXT NOT NULL,
                name TEXT NOT NULL,
                album_count INTEGER NOT NULL,
                track_count INTEGER NOT NULL,
                duration_seconds INTEGER NOT NULL DEFAULT 0,
                image_item_id TEXT,
                image_tag TEXT,
                image_origin TEXT NOT NULL DEFAULT 'unknown' CHECK (image_origin IN ('unknown', 'source', 'fallback', 'external')),
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, genre_id)
            );
            CREATE TABLE IF NOT EXISTS playlists (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                track_count INTEGER NOT NULL,
                duration_seconds INTEGER NOT NULL,
                top_genres_json TEXT NOT NULL DEFAULT '[]',
                image_item_id TEXT,
                image_tag TEXT,
                image_origin TEXT NOT NULL DEFAULT 'unknown' CHECK (image_origin IN ('unknown', 'source', 'fallback', 'external')),
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, playlist_id)
            );
            CREATE TABLE IF NOT EXISTS smart_playlists (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                smart_playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                builtin_key TEXT,
                definition_json TEXT NOT NULL,
                position INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, smart_playlist_id)
            );
            CREATE TABLE IF NOT EXISTS smart_playlist_seed_state (
                server_id TEXT PRIMARY KEY REFERENCES servers(server_id) ON DELETE CASCADE,
                seeded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS album_genres (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                album_id TEXT NOT NULL,
                genre_name TEXT NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, album_id, genre_name)
            );
            CREATE TABLE IF NOT EXISTS track_genres (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                genre_name TEXT NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, track_id, genre_name)
            );
            CREATE TABLE IF NOT EXISTS album_artist_links (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                album_id TEXT NOT NULL,
                artist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, album_id, artist_id)
            );
            CREATE TABLE IF NOT EXISTS track_artist_links (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                album_id TEXT NOT NULL,
                artist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, track_id, artist_id)
            );
            CREATE TABLE IF NOT EXISTS playlist_tracks (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                playlist_id TEXT NOT NULL,
                entry_id TEXT NOT NULL,
                track_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, playlist_id, entry_id)
            );
            CREATE TABLE IF NOT EXISTS collection_cover_refs (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                collection_type TEXT NOT NULL,
                collection_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                image_item_id TEXT NOT NULL,
                image_tag TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, collection_type, collection_id, position)
            );
            CREATE TABLE IF NOT EXISTS home_section_items (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                section_kind TEXT NOT NULL,
                item_type TEXT NOT NULL,
                item_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, section_kind, item_type, item_id)
            );
            CREATE TABLE IF NOT EXISTS home_section_prefetch_items (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                section_kind TEXT NOT NULL,
                item_type TEXT NOT NULL,
                item_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, section_kind, item_type, item_id)
            );
            CREATE TABLE IF NOT EXISTS lyrics_cache (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                track_id TEXT NOT NULL,
                source TEXT NOT NULL,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, track_id)
            );
            CREATE TABLE IF NOT EXISTS cover_cache (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                item_id TEXT NOT NULL,
                image_tag TEXT NOT NULL,
                size INTEGER NOT NULL,
                path TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, item_id, image_tag, size)
            );
            CREATE TABLE IF NOT EXISTS external_image_lookup_misses (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                item_id TEXT NOT NULL,
                image_tag TEXT NOT NULL,
                size INTEGER NOT NULL,
                reason TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, item_id, image_tag, size)
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS library_fts USING fts5(
                server_id UNINDEXED,
                item_type UNINDEXED,
                item_id UNINDEXED,
                title,
                subtitle
            );
            CREATE INDEX IF NOT EXISTS albums_server_title_idx
                ON albums(server_id, title);
            CREATE INDEX IF NOT EXISTS albums_server_title_nocase_idx
                ON albums(server_id, title COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS albums_server_artist_idx
                ON albums(server_id, artist_id, album_id);
            CREATE INDEX IF NOT EXISTS tracks_server_title_idx
                ON tracks(server_id, title);
            CREATE INDEX IF NOT EXISTS tracks_server_title_nocase_idx
                ON tracks(server_id, title COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS artists_server_name_nocase_idx
                ON artists(server_id, name COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS album_artists_server_name_nocase_idx
                ON album_artists(server_id, name COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS genres_server_name_nocase_idx
                ON genres(server_id, name COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS playlists_server_name_nocase_idx
                ON playlists(server_id, name COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS playlist_tracks_order_idx
                ON playlist_tracks(server_id, playlist_id, position, entry_id);
            CREATE INDEX IF NOT EXISTS tracks_server_album_idx
                ON tracks(server_id, album_id, disc_number, track_number);
            CREATE INDEX IF NOT EXISTS tracks_server_artist_idx
                ON tracks(server_id, artist_id, album_id);
            CREATE INDEX IF NOT EXISTS tracks_server_comment_nocase_idx
                ON tracks(server_id, comment COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS track_activity_server_skip_idx
                ON track_activity(server_id, skip_count DESC);
            CREATE INDEX IF NOT EXISTS smart_playlists_server_position_idx
                ON smart_playlists(server_id, position, name COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS home_section_items_order_idx
                ON home_section_items(server_id, section_kind, position);
            CREATE INDEX IF NOT EXISTS home_section_prefetch_items_order_idx
                ON home_section_prefetch_items(server_id, section_kind, position);
            CREATE INDEX IF NOT EXISTS album_genres_server_genre_idx
                ON album_genres(server_id, genre_name, album_id);
            CREATE INDEX IF NOT EXISTS track_genres_server_genre_idx
                ON track_genres(server_id, genre_name, track_id);
            CREATE INDEX IF NOT EXISTS collection_cover_refs_lookup_idx
                ON collection_cover_refs(server_id, collection_type, collection_id, position);
            CREATE INDEX IF NOT EXISTS album_artist_links_server_artist_idx
                ON album_artist_links(server_id, artist_id, album_id);
            CREATE INDEX IF NOT EXISTS track_artist_links_server_artist_idx
                ON track_artist_links(server_id, artist_id, track_id);
            CREATE INDEX IF NOT EXISTS track_music_folders_folder_idx
                ON track_music_folders(server_id, folder_id, track_id);
            CREATE INDEX IF NOT EXISTS track_music_folders_track_idx
                ON track_music_folders(server_id, track_id, folder_id);
            CREATE INDEX IF NOT EXISTS track_local_matches_track_idx
                ON track_local_matches(server_id, track_id);
            ",
        )?;
        self.create_local_manifest_schema()?;
        self.ensure_column("tracks", "source_format", "TEXT")?;
        self.ensure_column("tracks", "comment", "TEXT")?;
        self.ensure_column("tracks", "skip_count", "INTEGER")?;
        self.ensure_column("albums", "release_types_json", "TEXT NOT NULL DEFAULT '[]'")?;
        self.ensure_column("albums", "is_compilation", "INTEGER")?;
        self.ensure_column("albums", "musicbrainz_album_id", "TEXT")?;
        self.ensure_column("albums", "musicbrainz_release_group_id", "TEXT")?;
        self.ensure_column("playlists", "top_genres_json", "TEXT NOT NULL DEFAULT '[]'")?;
        self.ensure_column("genres", "duration_seconds", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_image_origin_columns()?;
        self.create_entity_identity_schema()?;
        self.connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }
    fn create_entity_identity_schema(&self) -> StoreResult<()> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS source_objects (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                source_object_id TEXT NOT NULL,
                entity_kind TEXT,
                entity_id TEXT,
                source_kind TEXT NOT NULL,
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
                    source_kind != 'cue_track'
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
                PRIMARY KEY (server_id, source_object_id)
            );
            CREATE INDEX IF NOT EXISTS source_objects_entity_idx
                ON source_objects(server_id, entity_kind, entity_id);
            CREATE INDEX IF NOT EXISTS source_objects_parent_idx
                ON source_objects(server_id, parent_source_object_id);

            CREATE TABLE IF NOT EXISTS entities (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                source TEXT NOT NULL,
                source_object_id TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, entity_kind, entity_id)
            );

            CREATE TABLE IF NOT EXISTS entity_identity_keys (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                namespace TEXT NOT NULL,
                value TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                source TEXT NOT NULL,
                strength INTEGER NOT NULL DEFAULT 100,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, entity_kind, namespace, value)
            );
            CREATE INDEX IF NOT EXISTS entity_identity_entity_idx
                ON entity_identity_keys(server_id, entity_kind, entity_id);

            CREATE TABLE IF NOT EXISTS entity_grouping_keys (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                namespace TEXT NOT NULL,
                value TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                source TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, entity_kind, namespace, value, entity_id)
            );
            CREATE INDEX IF NOT EXISTS entity_grouping_lookup_idx
                ON entity_grouping_keys(server_id, entity_kind, namespace, value);

            CREATE TABLE IF NOT EXISTS entity_facts (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                fact_key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, entity_kind, entity_id, fact_key, source)
            );

            CREATE TABLE IF NOT EXISTS entity_resolver_state (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                purpose TEXT NOT NULL,
                resolver_namespace TEXT NOT NULL,
                resolver_value TEXT NOT NULL,
                status TEXT NOT NULL,
                reason TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (
                    server_id, entity_kind, purpose, resolver_namespace, resolver_value
                )
            );

            CREATE TABLE IF NOT EXISTS entity_content_refs (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                content_kind TEXT NOT NULL,
                content_key TEXT NOT NULL,
                source TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, entity_kind, entity_id, content_kind, source)
            );
            CREATE INDEX IF NOT EXISTS entity_content_key_idx
                ON entity_content_refs(content_kind, content_key);

            CREATE TABLE IF NOT EXISTS entity_links (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                entity_kind TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                namespace TEXT NOT NULL,
                url TEXT NOT NULL,
                label TEXT,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (server_id, entity_kind, entity_id, namespace, source)
            );
            CREATE INDEX IF NOT EXISTS entity_links_namespace_idx
                ON entity_links(server_id, entity_kind, namespace);

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
        )?;
        self.backfill_entity_identity_schema()?;
        Ok(())
    }
    fn backfill_entity_identity_schema(&self) -> StoreResult<()> {
        self.connection.execute_batch(
            "
            INSERT OR IGNORE INTO source_objects (
                server_id, source_object_id, entity_kind, entity_id, source_kind,
                source_path, metadata_json, sync_generation, updated_at
            )
            SELECT server_id, 'local:file:' || root_path || char(31) || relative_path,
                   NULL, NULL, 'local_file',
                   path, '{}', scan_generation, CURRENT_TIMESTAMP
            FROM local_file_manifest;

            INSERT OR IGNORE INTO entities (
                server_id, entity_kind, entity_id, source, source_object_id
            )
            SELECT server_id, 'track', track_id, 'provider', NULL
            FROM tracks;
            INSERT OR IGNORE INTO entities (
                server_id, entity_kind, entity_id, source, source_object_id
            )
            SELECT server_id, 'album', album_id, 'provider', NULL
            FROM albums;
            INSERT OR IGNORE INTO entities (
                server_id, entity_kind, entity_id, source, source_object_id
            )
            SELECT server_id, 'artist', artist_id, 'provider', NULL
            FROM artists;
            INSERT OR IGNORE INTO entities (
                server_id, entity_kind, entity_id, source, source_object_id
            )
            SELECT server_id, 'album_artist', artist_id, 'provider', NULL
            FROM album_artists;

            UPDATE entities
            SET source = 'local',
                source_object_id = (
                    SELECT 'local:file:' || manifest.root_path || char(31) || manifest.relative_path
                    FROM local_file_manifest manifest
                    WHERE manifest.server_id = entities.server_id
                      AND manifest.track_id = entities.entity_id
                    LIMIT 1
                ),
                updated_at = CURRENT_TIMESTAMP
            WHERE entity_kind = 'track'
              AND EXISTS (
                SELECT 1
                FROM local_file_manifest manifest
                WHERE manifest.server_id = entities.server_id
                  AND manifest.track_id = entities.entity_id
              );

            INSERT OR IGNORE INTO entity_identity_keys (
                server_id, entity_kind, namespace, value, entity_id, source, strength
            )
            SELECT server_id, 'track', 'source:track_id', track_id, track_id, 'provider', 100
            FROM tracks;
            INSERT OR IGNORE INTO entity_identity_keys (
                server_id, entity_kind, namespace, value, entity_id, source, strength
            )
            SELECT server_id, 'track', 'local:path', path, track_id, 'local', 100
            FROM local_file_manifest;
            INSERT OR IGNORE INTO entity_identity_keys (
                server_id, entity_kind, namespace, value, entity_id, source, strength
            )
            SELECT server_id, 'album', 'source:album_id', album_id, album_id, 'provider', 100
            FROM albums;
            INSERT OR IGNORE INTO entity_identity_keys (
                server_id, entity_kind, namespace, value, entity_id, source, strength
            )
            SELECT server_id, 'artist', 'source:artist_id', artist_id, artist_id, 'provider', 100
            FROM artists;
            INSERT OR IGNORE INTO entity_identity_keys (
                server_id, entity_kind, namespace, value, entity_id, source, strength
            )
            SELECT server_id, 'album_artist', 'source:artist_id', artist_id, artist_id, 'provider', 100
            FROM album_artists;
            INSERT OR IGNORE INTO entity_identity_keys (
                server_id, entity_kind, namespace, value, entity_id, source, strength
            )
            SELECT server_id, 'album', 'musicbrainz:release',
                   TRIM(musicbrainz_album_id), album_id, 'provider', 100
            FROM albums
            WHERE TRIM(COALESCE(musicbrainz_album_id, '')) <> '';

            INSERT OR IGNORE INTO entity_grouping_keys (
                server_id, entity_kind, namespace, value, entity_id, source
            )
            SELECT server_id, 'album', 'musicbrainz:release_group',
                   TRIM(musicbrainz_release_group_id), album_id, 'provider'
            FROM albums
            WHERE TRIM(COALESCE(musicbrainz_release_group_id, '')) <> '';

            INSERT OR IGNORE INTO entity_facts (
                server_id, entity_kind, entity_id, fact_key, value_json, source, status
            )
            SELECT server_id, 'album', album_id, 'release_types',
                   release_types_json, 'provider', 'resolved'
            FROM albums
            WHERE release_types_json <> '[]';
            INSERT OR IGNORE INTO entity_facts (
                server_id, entity_kind, entity_id, fact_key, value_json, source, status
            )
            SELECT server_id, 'album', album_id, 'is_compilation',
                   CASE WHEN is_compilation = 1 THEN 'true' ELSE 'false' END,
                   'provider', 'resolved'
            FROM albums
            WHERE is_compilation IS NOT NULL;
            ",
        )?;
        if self.table_exists("album_release_type_lookup_misses")? {
            self.connection.execute_batch(
                "
                INSERT OR REPLACE INTO entity_resolver_state (
                    server_id, entity_kind, purpose, resolver_namespace,
                    resolver_value, status, reason, updated_at
                )
                SELECT server_id, 'album', 'release_metadata', 'musicbrainz',
                       lookup_key, 'missing', reason, updated_at
                FROM album_release_type_lookup_misses;
                DROP TABLE album_release_type_lookup_misses;
                ",
            )?;
        }
        if self.table_exists("album_identity")? {
            self.connection.execute_batch("DROP TABLE album_identity")?;
        }
        Ok(())
    }
    pub(super) fn table_exists(&self, table: &str) -> StoreResult<bool> {
        let count = self.connection.query_row(
            "
            SELECT COUNT(*)
            FROM sqlite_master
            WHERE type = 'table' AND name = ?1
            ",
            params![table],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }
    pub(super) fn table_has_column(&self, table: &str, column: &str) -> StoreResult<bool> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        Ok(collect_rows(columns)?.iter().any(|name| name == column))
    }
    pub(super) fn ensure_column(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> StoreResult<()> {
        if self.table_exists(table)? && !self.table_has_column(table, column)? {
            self.connection.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
        Ok(())
    }
    fn ensure_image_origin_columns(&self) -> StoreResult<()> {
        for table in IMAGE_ORIGIN_TABLES {
            self.ensure_column(
                table,
                "image_origin",
                "TEXT NOT NULL DEFAULT 'unknown' CHECK (image_origin IN ('unknown', 'source', 'fallback', 'external'))",
            )?;
        }
        Ok(())
    }
    pub fn load_queue_snapshot(&self, server_id: &ServerId) -> StoreResult<Option<QueueSnapshot>> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM queue_snapshots WHERE server_id = ?1",
                params![server_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value
            .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
            .transpose()
    }
    pub fn save_queue_snapshot(&self, snapshot: &QueueSnapshot) -> StoreResult<()> {
        let value = serde_json::to_string(snapshot)?;
        self.connection.execute(
            "
            INSERT INTO queue_snapshots (server_id, value, updated_at)
            VALUES (?1, ?2, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at
            ",
            params![snapshot.server_id.as_str(), value],
        )?;
        Ok(())
    }
    pub fn save_queue_progress(
        &self,
        server_id: &ServerId,
        entry_id: &QueueEntryId,
        track_id: &TrackId,
        progress_seconds: u32,
    ) -> StoreResult<bool> {
        let updated = self.connection.execute(
            "
            UPDATE queue_snapshots
            SET
                value = json_set(value, '$.progress_seconds', ?4),
                updated_at = CURRENT_TIMESTAMP
            WHERE server_id = ?1
                AND json_extract(value, '$.current_index') IS NOT NULL
                AND json_extract(
                    value,
                    '$.entries[' || json_extract(value, '$.current_index') || '].id'
                ) = ?2
                AND json_extract(
                    value,
                    '$.entries[' || json_extract(value, '$.current_index') || '].track_id'
                ) = ?3
            ",
            params![
                server_id.as_str(),
                entry_id.as_str(),
                track_id.as_str(),
                i64::from(progress_seconds)
            ],
        )?;
        Ok(updated > 0)
    }
    pub fn save_server(&self, saved: &SavedServer) -> StoreResult<()> {
        save_server_on_connection(&self.connection, saved)
    }
    pub fn save_server_settings_update(
        &self,
        saved: &SavedServer,
        clear_identity_cache: bool,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            save_server_on_connection(connection, saved)?;
            if clear_identity_cache {
                clear_server_cache(connection, &saved.server.id)?;
            }
            Ok(())
        })
    }
    pub fn set_active_server(&self, server_id: &ServerId) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO active_server (singleton, server_id)
            VALUES (1, ?1)
            ON CONFLICT(singleton) DO UPDATE SET server_id = excluded.server_id
            ",
            params![server_id.as_str()],
        )?;
        Ok(())
    }
    pub fn active_server(&self) -> StoreResult<Option<SavedServer>> {
        self.connection
            .query_row(
                "
                SELECT s.server_id, s.provider, s.name, s.base_url, s.user_id,
                       s.username, s.trust_invalid_cert
                FROM active_server a
                JOIN servers s ON s.server_id = a.server_id
                WHERE a.singleton = 1
                ",
                [],
                saved_server_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn saved_server(&self, server_id: &ServerId) -> StoreResult<Option<SavedServer>> {
        self.connection
            .query_row(
                "
                SELECT server_id, provider, name, base_url, user_id, username, trust_invalid_cert
                FROM servers
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
                saved_server_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn list_servers(&self) -> StoreResult<Vec<SavedServer>> {
        let mut statement = self.connection.prepare(
            "
            SELECT server_id, provider, name, base_url, user_id, username, trust_invalid_cert
            FROM servers
            ORDER BY name
            ",
        )?;
        collect_rows(statement.query_map([], saved_server_from_row)?)
    }
    pub fn save_server_local_access(&self, access: &ServerLocalAccess) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO server_local_access (
                server_id, root_path, path_replace_from, path_replace_to, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id) DO UPDATE SET
                root_path = excluded.root_path,
                path_replace_from = excluded.path_replace_from,
                path_replace_to = excluded.path_replace_to,
                updated_at = excluded.updated_at
            ",
            params![
                access.server_id.as_str(),
                access.root_path.as_str(),
                access.path_replace_from.as_deref(),
                access.path_replace_to.as_deref(),
            ],
        )?;
        Ok(())
    }
    pub fn server_local_access(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<Option<ServerLocalAccess>> {
        self.connection
            .query_row(
                "
                SELECT server_id, root_path, path_replace_from, path_replace_to
                FROM server_local_access
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
                |row| {
                    Ok(ServerLocalAccess {
                        server_id: ServerId::new(row.get::<_, String>(0)?),
                        root_path: row.get(1)?,
                        path_replace_from: row.get(2)?,
                        path_replace_to: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn local_access_status_facts(
        &self,
        access: &ServerLocalAccess,
    ) -> StoreResult<LocalAccessStatusFacts> {
        let prefix = access
            .path_replace_from
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let (
            total_track_count,
            direct_match_count,
            prefix_match_count,
            metadata_match_count,
            effective_match_count,
        ) = self.connection.query_row(
            "
            WITH mapped_tracks AS (
                SELECT track_id,
                       CASE
                           WHEN TRIM(COALESCE(local_path, '')) <> ''
                            AND (
                                (?2 IS NOT NULL AND ?2 <> ''
                                 AND substr(local_path, 1, length(?2)) = ?2)
                                OR local_path NOT GLOB '/*'
                            )
                           THEN 1 ELSE 0
                       END AS prefix_match,
                       CASE
                           WHEN TRIM(COALESCE(local_path, '')) <> ''
                            AND local_path GLOB '/*'
                            AND NOT (
                                ?2 IS NOT NULL AND ?2 <> ''
                                AND substr(local_path, 1, length(?2)) = ?2
                            )
                           THEN 1 ELSE 0
                       END AS direct_match
                FROM tracks
                WHERE server_id = ?1
            ),
            effective_matches AS (
                SELECT track_id
                FROM mapped_tracks
                WHERE prefix_match = 1 OR direct_match = 1
                UNION
                SELECT track_id
                FROM track_local_matches
                WHERE server_id = ?1
            )
            SELECT
                (SELECT COUNT(*) FROM tracks WHERE server_id = ?1),
                COALESCE(SUM(direct_match), 0),
                COALESCE(SUM(prefix_match), 0),
                (SELECT COUNT(*) FROM track_local_matches WHERE server_id = ?1),
                (SELECT COUNT(*) FROM effective_matches)
            FROM mapped_tracks
            ",
            params![access.server_id.as_str(), prefix],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        let metadata_sample = self
            .connection
            .query_row(
                "
                SELECT t.local_path, m.local_path
                FROM tracks t
                JOIN track_local_matches m
                  ON m.server_id = t.server_id AND m.track_id = t.track_id
                WHERE t.server_id = ?1
                  AND TRIM(COALESCE(t.local_path, '')) <> ''
                ORDER BY t.album COLLATE NOCASE,
                         t.disc_number,
                         t.track_number,
                         t.title COLLATE NOCASE
                LIMIT 1
                ",
                params![access.server_id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let (sample_server_path, sample_metadata_path) =
            if let Some((server_path, local_path)) = metadata_sample {
                (Some(server_path), Some(local_path))
            } else {
                (
                    self.connection
                        .query_row(
                            "
                            SELECT local_path
                            FROM tracks
                            WHERE server_id = ?1
                              AND TRIM(COALESCE(local_path, '')) <> ''
                            ORDER BY album COLLATE NOCASE,
                                     disc_number,
                                     track_number,
                                     title COLLATE NOCASE
                            LIMIT 1
                            ",
                            params![access.server_id.as_str()],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()?,
                    None,
                )
            };
        let total_track_count = usize_from_count(total_track_count);
        let effective_match_count = usize_from_count(effective_match_count);
        Ok(LocalAccessStatusFacts {
            sample_server_path,
            sample_metadata_path,
            direct_match_count: usize_from_count(direct_match_count),
            prefix_match_count: usize_from_count(prefix_match_count),
            metadata_match_count: usize_from_count(metadata_match_count),
            unmatched_count: total_track_count.saturating_sub(effective_match_count),
            total_track_count,
        })
    }
    pub fn delete_server_local_access(&self, server_id: &ServerId) -> StoreResult<()> {
        self.connection.execute(
            "DELETE FROM server_local_access WHERE server_id = ?1",
            params![server_id.as_str()],
        )?;
        Ok(())
    }
    pub fn upsert_music_folders(
        &self,
        server_id: &ServerId,
        folders: &[MusicFolder],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO server_music_folders (server_id, folder_id, name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(server_id, folder_id) DO UPDATE SET
                    name = excluded.name,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for folder in folders {
                statement.execute(params![
                    server_id.as_str(),
                    folder.id.as_str(),
                    folder.name.as_str(),
                    generation,
                ])?;
            }
            Ok(())
        })
    }
    pub fn list_music_folders(&self, server_id: &ServerId) -> StoreResult<Vec<MusicFolder>> {
        let mut statement = self.connection.prepare(
            "
            SELECT folder_id, name
            FROM server_music_folders
            WHERE server_id = ?1
            ORDER BY name COLLATE NOCASE
            ",
        )?;
        collect_rows(statement.query_map(params![server_id.as_str()], |row| {
            Ok(MusicFolder {
                id: MusicFolderId::new(row.get::<_, String>(0)?),
                name: row.get(1)?,
            })
        })?)
    }
    pub fn selected_music_folder_id(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<Option<MusicFolderId>> {
        self.connection
            .query_row(
                "
                SELECT selected_music_folder_id
                FROM server_library_preferences
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|value| value.flatten().map(MusicFolderId::new))
            .map_err(StoreError::from)
    }
    pub fn set_selected_music_folder_id(
        &self,
        server_id: &ServerId,
        folder_id: Option<&MusicFolderId>,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO server_library_preferences (
                server_id, selected_music_folder_id, updated_at
            )
            VALUES (?1, ?2, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id) DO UPDATE SET
                selected_music_folder_id = excluded.selected_music_folder_id,
                updated_at = excluded.updated_at
            ",
            params![server_id.as_str(), folder_id.map(MusicFolderId::as_str)],
        )?;
        Ok(())
    }
    pub fn upsert_track_music_folder_memberships(
        &self,
        server_id: &ServerId,
        folder_id: &MusicFolderId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO track_music_folders (
                    server_id, track_id, folder_id, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(server_id, track_id, folder_id) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for track in tracks {
                statement.execute(params![
                    server_id.as_str(),
                    track.id.as_str(),
                    folder_id.as_str(),
                    generation,
                ])?;
            }
            Ok(())
        })
    }
    pub fn replace_track_local_matches(
        &self,
        server_id: &ServerId,
        matches: &[(TrackId, String, String)],
    ) -> StoreResult<()> {
        let mut incoming = matches.to_vec();
        incoming.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        if self.track_local_matches_with_kind(server_id)? == incoming {
            return Ok(());
        }
        self.write_batch(|connection| {
            connection.execute(
                "DELETE FROM track_local_matches WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            let mut statement = connection.prepare(
                "
                INSERT INTO track_local_matches (
                    server_id, track_id, local_path, match_kind, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
                ",
            )?;
            for (track_id, local_path, match_kind) in matches {
                statement.execute(params![
                    server_id.as_str(),
                    track_id.as_str(),
                    local_path.as_str(),
                    match_kind.as_str(),
                ])?;
            }
            Ok(())
        })
    }
    fn track_local_matches_with_kind(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<Vec<(TrackId, String, String)>> {
        let mut statement = self.connection.prepare(
            "
            SELECT track_id, local_path, match_kind
            FROM track_local_matches
            WHERE server_id = ?1
            ORDER BY track_id, local_path, match_kind
            ",
        )?;
        collect_rows(statement.query_map(params![server_id.as_str()], |row| {
            Ok((
                TrackId::new(row.get::<_, String>(0)?),
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?)
    }
    pub fn delete_track_local_matches(&self, server_id: &ServerId) -> StoreResult<()> {
        self.connection.execute(
            "DELETE FROM track_local_matches WHERE server_id = ?1",
            params![server_id.as_str()],
        )?;
        Ok(())
    }
    pub fn track_local_match_path(
        &self,
        server_id: &ServerId,
        track_id: &TrackId,
    ) -> StoreResult<Option<String>> {
        self.connection
            .query_row(
                "
                SELECT local_path
                FROM track_local_matches
                WHERE server_id = ?1 AND track_id = ?2
                ",
                params![server_id.as_str(), track_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
    pub fn track_local_match_paths(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<Vec<(TrackId, String)>> {
        let mut statement = self.connection.prepare(
            "
            SELECT track_id, local_path
            FROM track_local_matches
            WHERE server_id = ?1
            ORDER BY track_id
            ",
        )?;
        collect_rows(statement.query_map(params![server_id.as_str()], |row| {
            Ok((TrackId::new(row.get::<_, String>(0)?), row.get(1)?))
        })?)
    }
    pub fn sync_state(&self, server_id: &ServerId) -> StoreResult<SyncState> {
        self.connection
            .query_row(
                "
                SELECT server_id, generation, status, last_started_at, last_completed_at, last_error
                FROM sync_state
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
                |row| {
                    Ok(SyncState {
                        server_id: ServerId::new(row.get::<_, String>(0)?),
                        generation: row.get(1)?,
                        status: row.get(2)?,
                        last_started_at: row.get(3)?,
                        last_completed_at: row.get(4)?,
                        last_error: row.get(5)?,
                    })
                },
            )
            .map_err(StoreError::from)
    }
    pub fn sync_completed_age_seconds(&self, server_id: &ServerId) -> StoreResult<Option<i64>> {
        self.connection
            .query_row(
                "
                SELECT CAST(strftime('%s', 'now') AS INTEGER)
                     - CAST(strftime('%s', last_completed_at) AS INTEGER)
                FROM sync_state
                WHERE server_id = ?1 AND last_completed_at IS NOT NULL
                ",
                params![server_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(StoreError::from)
    }
    pub fn begin_sync(&self, server_id: &ServerId) -> StoreResult<i64> {
        let current = self
            .connection
            .query_row(
                "SELECT generation FROM sync_state WHERE server_id = ?1",
                params![server_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let generation = current + 1;
        self.connection.execute(
            "
            INSERT INTO sync_state (
                server_id, generation, status, last_started_at, last_error
            )
            VALUES (?1, ?2, 'running', CURRENT_TIMESTAMP, NULL)
            ON CONFLICT(server_id) DO UPDATE SET
                generation = excluded.generation,
                status = excluded.status,
                last_started_at = excluded.last_started_at,
                last_error = NULL
            ",
            params![server_id.as_str(), generation],
        )?;
        Ok(generation)
    }
}

fn usize_from_count(value: i64) -> usize {
    value.max(0) as usize
}

pub(super) fn save_server_on_connection(
    connection: &Connection,
    saved: &SavedServer,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO servers (
            server_id, provider, name, base_url, user_id, username,
            trust_invalid_cert, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP)
        ON CONFLICT(server_id) DO UPDATE SET
            provider = excluded.provider,
            name = excluded.name,
            base_url = excluded.base_url,
            user_id = excluded.user_id,
            username = excluded.username,
            trust_invalid_cert = excluded.trust_invalid_cert,
            updated_at = excluded.updated_at
        ",
        params![
            saved.server.id.as_str(),
            saved.server.provider,
            saved.server.name,
            saved.server.base_url,
            saved.user_id,
            saved.username,
            bool_to_i64(saved.trust_invalid_cert),
        ],
    )?;
    connection.execute(
        "
        INSERT OR IGNORE INTO sync_state (server_id)
        VALUES (?1)
        ",
        params![saved.server.id.as_str()],
    )?;
    Ok(())
}

pub(super) fn clear_server_cache(connection: &Connection, server_id: &ServerId) -> StoreResult<()> {
    clear_library_cache_on_connection(connection, server_id)?;
    connection.execute(
        "DELETE FROM queue_snapshots WHERE server_id = ?1",
        params![server_id.as_str()],
    )?;
    connection.execute(
        "DELETE FROM server_library_preferences WHERE server_id = ?1",
        params![server_id.as_str()],
    )?;
    connection.execute(
        "
        UPDATE sync_state
        SET generation = 0,
            status = 'idle',
            last_started_at = NULL,
            last_completed_at = NULL,
            last_error = NULL
        WHERE server_id = ?1
        ",
        params![server_id.as_str()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_op_migration(_store: &Store) -> StoreResult<()> {
        Ok(())
    }

    #[test]
    fn store_require_steps() {
        static MIGRATIONS: &[SchemaMigration] = &[
            SchemaMigration {
                from_version: 1,
                run: no_op_migration,
            },
            SchemaMigration {
                from_version: 2,
                run: no_op_migration,
            },
        ];

        let path = schema_migration_path(1, 3, MIGRATIONS).expect("migration path");
        assert_eq!(
            path.iter()
                .map(|migration| migration.to_version())
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(schema_migration_path(1, 4, MIGRATIONS).is_none());
        assert!(schema_migration_path(4, 3, MIGRATIONS).is_none());
    }
}
