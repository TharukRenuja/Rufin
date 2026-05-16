use std::{collections::HashMap, path::Path};

use rufin_core::{
    Album, AlbumId, AppSettings, Artist, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind,
    ImageRef, Playlist, PlaylistId, QueueSnapshot, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_provider::{Lyrics, PagedResponse, PlaylistDetail, PlaylistEntry, SearchResults};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use thiserror::Error;

const SETTINGS_KEY: &str = "default";
const SCHEMA_VERSION: i64 = 4;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedServer {
    pub server: ServerIdentity,
    pub user_id: String,
    pub username: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncState {
    pub server_id: ServerId,
    pub generation: i64,
    pub status: String,
    pub last_started_at: Option<String>,
    pub last_completed_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverCacheEntry {
    pub server_id: ServerId,
    pub item_id: String,
    pub image_tag: String,
    pub size: u32,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedArtistDetail {
    pub artist: Artist,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedGenreDetail {
    pub genre: Genre,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.configure_pragmas(true)?;
        store.migrate()?;
        Ok(store)
    }

    pub fn open_memory() -> StoreResult<Self> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.configure_pragmas(true)?;
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> StoreResult<()> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

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
                track_count INTEGER NOT NULL,
                duration_seconds INTEGER NOT NULL,
                favorite INTEGER NOT NULL,
                color_seed INTEGER NOT NULL,
                image_item_id TEXT,
                image_tag TEXT,
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
                duration_seconds INTEGER NOT NULL,
                favorite INTEGER NOT NULL,
                disc_number INTEGER NOT NULL,
                track_number INTEGER NOT NULL,
                image_item_id TEXT,
                image_tag TEXT,
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
                image_item_id TEXT,
                image_tag TEXT,
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
                image_item_id TEXT,
                image_tag TEXT,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, artist_id)
            );

            CREATE TABLE IF NOT EXISTS genres (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                genre_id TEXT NOT NULL,
                name TEXT NOT NULL,
                album_count INTEGER NOT NULL,
                track_count INTEGER NOT NULL,
                image_item_id TEXT,
                image_tag TEXT,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, genre_id)
            );

            CREATE TABLE IF NOT EXISTS playlists (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                track_count INTEGER NOT NULL,
                duration_seconds INTEGER NOT NULL,
                image_item_id TEXT,
                image_tag TEXT,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, playlist_id)
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

            CREATE TABLE IF NOT EXISTS playlist_tracks (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                playlist_id TEXT NOT NULL,
                entry_id TEXT NOT NULL,
                track_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, playlist_id, entry_id)
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
            CREATE INDEX IF NOT EXISTS tracks_server_album_idx
                ON tracks(server_id, album_id, disc_number, track_number);
            CREATE INDEX IF NOT EXISTS album_genres_server_genre_idx
                ON album_genres(server_id, genre_name, album_id);
            CREATE INDEX IF NOT EXISTS track_genres_server_genre_idx
                ON track_genres(server_id, genre_name, track_id);
            ",
        )?;
        self.ensure_column("albums", "image_item_id", "TEXT")?;
        self.ensure_column("albums", "image_tag", "TEXT")?;
        self.ensure_column("tracks", "image_item_id", "TEXT")?;
        self.ensure_column("tracks", "image_tag", "TEXT")?;
        for table in ["artists", "album_artists", "genres", "playlists"] {
            self.ensure_column(table, "image_item_id", "TEXT")?;
            self.ensure_column(table, "image_tag", "TEXT")?;
        }
        self.ensure_playlist_entries_table()?;
        self.connection.execute(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1)",
            [],
        )?;
        self.connection.execute(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
            params![SCHEMA_VERSION],
        )?;
        Ok(())
    }

    fn ensure_column(&self, table: &str, column: &str, sql_type: &str) -> StoreResult<()> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        let has_column = collect_rows(columns)?.iter().any(|name| name == column);
        if !has_column {
            self.connection.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {sql_type}"),
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_playlist_entries_table(&self) -> StoreResult<()> {
        let mut statement = self
            .connection
            .prepare("PRAGMA table_info(playlist_tracks)")?;
        let columns = collect_rows(statement.query_map([], |row| row.get::<_, String>(1))?)?;
        if columns.iter().any(|name| name == "entry_id") {
            return Ok(());
        }

        self.connection.execute_batch(
            "
            ALTER TABLE playlist_tracks RENAME TO playlist_tracks_v3;
            CREATE TABLE playlist_tracks (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                playlist_id TEXT NOT NULL,
                entry_id TEXT NOT NULL,
                track_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, playlist_id, entry_id)
            );
            INSERT INTO playlist_tracks (
                server_id, playlist_id, entry_id, track_id, position, sync_generation
            )
            SELECT server_id,
                   playlist_id,
                   track_id || ':' || position,
                   track_id,
                   position,
                   sync_generation
            FROM playlist_tracks_v3;
            DROP TABLE playlist_tracks_v3;
            ",
        )?;
        Ok(())
    }

    pub fn load_settings(&self) -> StoreResult<AppSettings> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![SETTINGS_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        value
            .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
            .unwrap_or_else(|| Ok(AppSettings::default()))
    }

    pub fn save_settings(&self, settings: &AppSettings) -> StoreResult<()> {
        let value = serde_json::to_string(settings)?;
        self.connection.execute(
            "
            INSERT INTO app_settings (key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![SETTINGS_KEY, value],
        )?;
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

    pub fn save_server(&self, saved: &SavedServer) -> StoreResult<()> {
        self.connection.execute(
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
        self.connection.execute(
            "
            INSERT OR IGNORE INTO sync_state (server_id)
            VALUES (?1)
            ",
            params![saved.server.id.as_str()],
        )?;
        Ok(())
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

    pub fn complete_sync(&self, server_id: &ServerId, generation: i64) -> StoreResult<()> {
        self.prune_missing_items(server_id, generation)?;
        self.connection.execute(
            "
            UPDATE sync_state
            SET status = 'idle',
                generation = ?2,
                last_completed_at = CURRENT_TIMESTAMP,
                last_error = NULL
            WHERE server_id = ?1
            ",
            params![server_id.as_str(), generation],
        )?;
        Ok(())
    }

    pub fn fail_sync(&self, server_id: &ServerId, error: &str) -> StoreResult<()> {
        self.connection.execute(
            "
            UPDATE sync_state
            SET status = 'error',
                last_error = ?2
            WHERE server_id = ?1
            ",
            params![server_id.as_str(), error],
        )?;
        Ok(())
    }

    pub fn clear_library_cache(&self, server_id: &ServerId) -> StoreResult<()> {
        self.write_batch(|connection| {
            clear_library_cache_on_connection(connection, server_id)?;
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
        })
    }

    pub fn forget_server(&self, server_id: &ServerId) -> StoreResult<()> {
        self.write_batch(|connection| {
            clear_library_cache_on_connection(connection, server_id)?;
            connection.execute(
                "DELETE FROM queue_snapshots WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM active_server WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM sync_state WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM servers WHERE server_id = ?1",
                params![server_id.as_str()],
            )?;
            Ok(())
        })
    }

    pub fn upsert_albums(
        &self,
        server_id: &ServerId,
        albums: &[Album],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO albums (
                    server_id, album_id, title, artist, artist_id, year, track_count,
                    duration_seconds, favorite, color_seed, image_item_id, image_tag,
                    sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(server_id, album_id) DO UPDATE SET
                    title = excluded.title,
                    artist = excluded.artist,
                    artist_id = excluded.artist_id,
                    year = excluded.year,
                    track_count = excluded.track_count,
                    duration_seconds = excluded.duration_seconds,
                    favorite = excluded.favorite,
                    color_seed = excluded.color_seed,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_genres = connection.prepare(
                "DELETE FROM album_genres WHERE server_id = ?1 AND album_id = ?2",
            )?;
            let mut insert_genre = connection.prepare(
                "
                INSERT INTO album_genres (server_id, album_id, genre_name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(server_id, album_id, genre_name) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = 'album' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'album', ?2, ?3, ?4)
                ",
            )?;

            for album in albums {
                let (image_item_id, image_tag) = image_ref_parts(album.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    album.id.as_str(),
                    album.title,
                    album.artist,
                    album.artist_id.as_ref().map(ArtistId::as_str),
                    i64::from(album.year),
                    i64::from(album.track_count),
                    i64::from(album.duration_seconds),
                    bool_to_i64(album.favorite),
                    i64::from(album.color_seed),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
                delete_genres.execute(params![server_id.as_str(), album.id.as_str()])?;
                for genre in &album.genres {
                    if !genre.trim().is_empty() {
                        insert_genre.execute(params![
                            server_id.as_str(),
                            album.id.as_str(),
                            genre.trim(),
                            generation,
                        ])?;
                    }
                }
                delete_fts.execute(params![server_id.as_str(), album.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    album.id.as_str(),
                    album.title,
                    album.artist,
                ])?;
            }
            Ok(())
        })
    }

    pub fn upsert_tracks(
        &self,
        server_id: &ServerId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO tracks (
                    server_id, track_id, album_id, title, artist, artist_id, album,
                    year, duration_seconds, favorite, disc_number, track_number,
                    image_item_id, image_tag, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                ON CONFLICT(server_id, track_id) DO UPDATE SET
                    album_id = excluded.album_id,
                    title = excluded.title,
                    artist = excluded.artist,
                    artist_id = excluded.artist_id,
                    album = excluded.album,
                    year = excluded.year,
                    duration_seconds = excluded.duration_seconds,
                    favorite = excluded.favorite,
                    disc_number = excluded.disc_number,
                    track_number = excluded.track_number,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_genres = connection.prepare(
                "DELETE FROM track_genres WHERE server_id = ?1 AND track_id = ?2",
            )?;
            let mut insert_genre = connection.prepare(
                "
                INSERT INTO track_genres (server_id, track_id, genre_name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(server_id, track_id, genre_name) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = 'track' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'track', ?2, ?3, ?4)
                ",
            )?;

            for track in tracks {
                let (image_item_id, image_tag) = image_ref_parts(track.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    track.id.as_str(),
                    track.album_id.as_str(),
                    track.title,
                    track.artist,
                    track.artist_id.as_ref().map(ArtistId::as_str),
                    track.album,
                    i64::from(track.year),
                    i64::from(track.duration_seconds),
                    bool_to_i64(track.favorite),
                    i64::from(track.disc_number),
                    i64::from(track.track_number),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
                delete_genres.execute(params![server_id.as_str(), track.id.as_str()])?;
                for genre in &track.genres {
                    if !genre.trim().is_empty() {
                        insert_genre.execute(params![
                            server_id.as_str(),
                            track.id.as_str(),
                            genre.trim(),
                            generation,
                        ])?;
                    }
                }
                delete_fts.execute(params![server_id.as_str(), track.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    track.id.as_str(),
                    track.title,
                    format!("{} {}", track.artist, track.album),
                ])?;
            }
            Ok(())
        })
    }

    pub fn refresh_library_counts(&self, server_id: &ServerId) -> StoreResult<()> {
        self.write_batch(|connection| {
            let generation = connection
                .query_row(
                    "SELECT generation FROM sync_state WHERE server_id = ?1",
                    params![server_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            repair_linked_artists(connection, server_id, generation)?;
            repair_linked_genres(connection, server_id, generation)?;
            connection.execute(
                "
                UPDATE albums
                SET track_count = MAX(track_count, (
                    SELECT COUNT(*)
                    FROM tracks
                    WHERE tracks.server_id = albums.server_id
                      AND tracks.album_id = albums.album_id
                )),
                    duration_seconds = MAX(duration_seconds, (
                    SELECT COALESCE(SUM(duration_seconds), 0)
                    FROM tracks
                    WHERE tracks.server_id = albums.server_id
                      AND tracks.album_id = albums.album_id
                ))
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                UPDATE artists
                SET track_count = MAX(track_count, (
                    SELECT COUNT(*)
                    FROM tracks
                    WHERE tracks.server_id = artists.server_id
                      AND tracks.artist_id = artists.artist_id
                )),
                    album_count = MAX(album_count, (
                    SELECT COUNT(DISTINCT album_id)
                    FROM tracks
                    WHERE tracks.server_id = artists.server_id
                      AND tracks.artist_id = artists.artist_id
                ))
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                UPDATE album_artists
                SET track_count = MAX(track_count, (
                    SELECT COALESCE(SUM(track_count), 0)
                    FROM albums
                    WHERE albums.server_id = album_artists.server_id
                      AND albums.artist_id = album_artists.artist_id
                )),
                    album_count = MAX(album_count, (
                    SELECT COUNT(*)
                    FROM albums
                    WHERE albums.server_id = album_artists.server_id
                      AND albums.artist_id = album_artists.artist_id
                ))
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                UPDATE genres
                SET album_count = MAX(album_count, (
                    SELECT COUNT(DISTINCT album_id)
                    FROM album_genres
                    WHERE album_genres.server_id = genres.server_id
                      AND album_genres.genre_name = genres.name
                )),
                    track_count = MAX(track_count, (
                    SELECT COUNT(DISTINCT track_id)
                    FROM track_genres
                    WHERE track_genres.server_id = genres.server_id
                      AND track_genres.genre_name = genres.name
                ))
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
            )?;
            Ok(())
        })
    }

    pub fn upsert_artists(
        &self,
        server_id: &ServerId,
        artists: &[Artist],
        album_artist: bool,
        generation: i64,
    ) -> StoreResult<()> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        self.write_batch(|connection| {
            let sql = format!(
                "
                INSERT INTO {table} (
                    server_id, artist_id, name, album_count, track_count, favorite,
                    image_item_id, image_tag, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(server_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    favorite = excluded.favorite,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                "
            );
            let mut statement = connection.prepare(&sql)?;
            let item_type = if album_artist {
                "album_artist"
            } else {
                "artist"
            };
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = ?2 AND item_id = ?3",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, ?2, ?3, ?4, '')
                ",
            )?;

            for artist in artists {
                let (image_item_id, image_tag) = image_ref_parts(artist.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    artist.id.as_str(),
                    artist.name,
                    i64::from(artist.album_count),
                    i64::from(artist.track_count),
                    bool_to_i64(artist.favorite),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
                delete_fts.execute(params![server_id.as_str(), item_type, artist.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    item_type,
                    artist.id.as_str(),
                    artist.name,
                ])?;
            }
            Ok(())
        })
    }

    pub fn upsert_genres(
        &self,
        server_id: &ServerId,
        genres: &[Genre],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO genres (
                    server_id, genre_id, name, album_count, track_count, image_item_id,
                    image_tag, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(server_id, genre_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for genre in genres {
                let (image_item_id, image_tag) = image_ref_parts(genre.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    genre.id.as_str(),
                    genre.name,
                    i64::from(genre.album_count),
                    i64::from(genre.track_count),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
            }
            Ok(())
        })
    }

    pub fn upsert_playlists(
        &self,
        server_id: &ServerId,
        playlists: &[Playlist],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            let mut statement = connection.prepare(
                "
                INSERT INTO playlists (
                    server_id, playlist_id, name, track_count, duration_seconds,
                    image_item_id, image_tag, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(server_id, playlist_id) DO UPDATE SET
                    name = excluded.name,
                    track_count = excluded.track_count,
                    duration_seconds = excluded.duration_seconds,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = 'playlist' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'playlist', ?2, ?3, '')
                ",
            )?;

            for playlist in playlists {
                let (image_item_id, image_tag) = image_ref_parts(playlist.image_ref.as_ref());
                statement.execute(params![
                    server_id.as_str(),
                    playlist.id.as_str(),
                    playlist.name,
                    i64::from(playlist.track_count),
                    i64::from(playlist.duration_seconds),
                    image_item_id,
                    image_tag,
                    generation,
                ])?;
                delete_fts.execute(params![server_id.as_str(), playlist.id.as_str()])?;
                insert_fts.execute(params![
                    server_id.as_str(),
                    playlist.id.as_str(),
                    playlist.name,
                ])?;
            }
            Ok(())
        })
    }

    pub fn load_home_sections(&self, server_id: &ServerId) -> StoreResult<Vec<HomeSection>> {
        let sections = [
            (HomeSectionKind::Explore, 0_usize),
            (HomeSectionKind::MostPlayed, 6),
            (HomeSectionKind::NewlyAdded, 12),
            (HomeSectionKind::RecentlyPlayed, 18),
            (HomeSectionKind::RecentlyReleased, 24),
        ]
        .into_iter()
        .map(|(kind, offset)| {
            self.load_albums(server_id, offset, 8)
                .map(|response| HomeSection {
                    kind,
                    albums: response.items,
                })
        })
        .collect::<StoreResult<Vec<_>>>()?;

        Ok(sections
            .into_iter()
            .filter(|section| !section.albums.is_empty())
            .collect())
    }

    pub fn load_albums(
        &self,
        server_id: &ServerId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Album>> {
        let total = self.count("albums", server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT album_id, title, artist, artist_id, year, track_count,
                   duration_seconds, favorite, color_seed, image_item_id, image_tag
            FROM albums
            WHERE server_id = ?1
            ORDER BY title COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let mut items = collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            album_from_row,
        )?)?;
        self.attach_album_genres(server_id, &mut items)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_album_detail(
        &self,
        server_id: &ServerId,
        album_id: &AlbumId,
    ) -> StoreResult<Option<(Album, Vec<Track>)>> {
        let album = self
            .connection
            .query_row(
                "
                SELECT album_id, title, artist, artist_id, year, track_count,
                       duration_seconds, favorite, color_seed, image_item_id, image_tag
                FROM albums
                WHERE server_id = ?1 AND album_id = ?2
                ",
                params![server_id.as_str(), album_id.as_str()],
                album_from_row,
            )
            .optional()?;
        let mut statement = self.connection.prepare(
            "
            SELECT track_id, album_id, title, artist, artist_id, album, year,
                   duration_seconds, favorite, disc_number, track_number, image_item_id, image_tag
            FROM tracks
            WHERE server_id = ?1 AND album_id = ?2
            ORDER BY disc_number, track_number, title COLLATE NOCASE
            ",
        )?;
        let mut tracks = collect_rows(statement.query_map(
            params![server_id.as_str(), album_id.as_str()],
            track_from_row,
        )?)?;
        self.attach_track_genres(server_id, &mut tracks)?;
        let mut album = match album {
            Some(album) => album,
            None if tracks.is_empty() => return Ok(None),
            None => synthesize_album_from_tracks(album_id, &tracks),
        };
        self.attach_album_genres(server_id, std::slice::from_mut(&mut album))?;
        Ok(Some((album, tracks)))
    }

    pub fn load_artist_detail(
        &self,
        server_id: &ServerId,
        artist_id: &ArtistId,
    ) -> StoreResult<Option<CachedArtistDetail>> {
        let artist = self
            .connection
            .query_row(
                "
                SELECT artist_id, name, album_count, track_count, favorite, image_item_id, image_tag
                FROM artists
                WHERE server_id = ?1 AND artist_id = ?2
                ",
                params![server_id.as_str(), artist_id.as_str()],
                artist_from_row,
            )
            .optional()?;
        let artist = match artist {
            Some(artist) => Some(artist),
            None => self
                .connection
                .query_row(
                    "
                    SELECT artist_id, name, album_count, track_count, favorite, image_item_id, image_tag
                    FROM album_artists
                    WHERE server_id = ?1 AND artist_id = ?2
                    ",
                    params![server_id.as_str(), artist_id.as_str()],
                    artist_from_row,
                )
                .optional()?,
        };

        let mut albums_statement = self.connection.prepare(
            "
            SELECT album_id, title, artist, artist_id, year, track_count,
                   duration_seconds, favorite, color_seed, image_item_id, image_tag
            FROM albums
            WHERE server_id = ?1 AND artist_id = ?2
            ORDER BY year, title COLLATE NOCASE
            ",
        )?;
        let mut albums = collect_rows(albums_statement.query_map(
            params![server_id.as_str(), artist_id.as_str()],
            album_from_row,
        )?)?;
        self.attach_album_genres(server_id, &mut albums)?;

        let mut tracks_statement = self.connection.prepare(
            "
            SELECT DISTINCT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.duration_seconds, t.favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM tracks t
            LEFT JOIN albums a
                ON a.server_id = t.server_id AND a.album_id = t.album_id
            WHERE t.server_id = ?1
              AND (t.artist_id = ?2 OR a.artist_id = ?2)
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                     t.title COLLATE NOCASE
            ",
        )?;
        let mut tracks = collect_rows(tracks_statement.query_map(
            params![server_id.as_str(), artist_id.as_str()],
            track_from_row,
        )?)?;
        self.attach_track_genres(server_id, &mut tracks)?;

        let artist = match artist {
            Some(artist) => artist,
            None if albums.is_empty() && tracks.is_empty() => return Ok(None),
            None => synthesize_artist_from_links(artist_id, &albums, &tracks),
        };

        Ok(Some(CachedArtistDetail {
            artist,
            albums,
            tracks,
        }))
    }

    pub fn load_tracks(
        &self,
        server_id: &ServerId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let total = self.count("tracks", server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT track_id, album_id, title, artist, artist_id, album, year,
                   duration_seconds, favorite, disc_number, track_number, image_item_id, image_tag
            FROM tracks
            WHERE server_id = ?1
            ORDER BY title COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            track_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_artists(
        &self,
        server_id: &ServerId,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let total = self.count(table, server_id)?;
        let sql = format!(
            "
            SELECT artist_id, name, album_count, track_count, favorite, image_item_id, image_tag
            FROM {table}
            WHERE server_id = ?1
            ORDER BY name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            "
        );
        let mut statement = self.connection.prepare(&sql)?;
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            artist_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn load_genres(
        &self,
        server_id: &ServerId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Genre>> {
        let total = self.count_linked_genres(server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT genre_id, name, album_count, track_count, image_item_id, image_tag
            FROM genres g
            WHERE g.server_id = ?1
              AND (
                  EXISTS (
                      SELECT 1
                      FROM album_genres ag
                      WHERE ag.server_id = g.server_id AND ag.genre_name = g.name
                  )
                  OR EXISTS (
                      SELECT 1
                      FROM track_genres tg
                      WHERE tg.server_id = g.server_id AND tg.genre_name = g.name
                  )
              )
            ORDER BY name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            genre_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total))
    }

    fn count_linked_genres(&self, server_id: &ServerId) -> StoreResult<usize> {
        self.connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM genres g
                WHERE g.server_id = ?1
                  AND (
                      EXISTS (
                          SELECT 1
                          FROM album_genres ag
                          WHERE ag.server_id = g.server_id AND ag.genre_name = g.name
                      )
                      OR EXISTS (
                          SELECT 1
                          FROM track_genres tg
                          WHERE tg.server_id = g.server_id AND tg.genre_name = g.name
                      )
                  )
                ",
                params![server_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map(u32_from_i64)
            .map(|count| count as usize)
            .map_err(StoreError::from)
    }

    pub fn load_playlists(
        &self,
        server_id: &ServerId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Playlist>> {
        let total = self.count("playlists", server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT playlist_id, name, track_count, duration_seconds, image_item_id, image_tag
            FROM playlists
            WHERE server_id = ?1
            ORDER BY name COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            playlist_from_row,
        )?)?;
        Ok(PagedResponse::new(items, total))
    }

    pub fn upsert_playlist_tracks(
        &self,
        server_id: &ServerId,
        playlist_id: &PlaylistId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        let entries = tracks
            .iter()
            .enumerate()
            .map(|(position, track)| PlaylistEntry {
                entry_id: format!("{}:{position}", track.id.as_str()),
                track: track.clone(),
            })
            .collect::<Vec<_>>();
        self.upsert_playlist_entries(server_id, playlist_id, &entries, generation)
    }

    pub fn upsert_playlist_entries(
        &self,
        server_id: &ServerId,
        playlist_id: &PlaylistId,
        entries: &[PlaylistEntry],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "DELETE FROM playlist_tracks WHERE server_id = ?1 AND playlist_id = ?2",
                params![server_id.as_str(), playlist_id.as_str()],
            )?;
            let mut statement = connection.prepare(
                "
                INSERT INTO playlist_tracks (
                    server_id, playlist_id, entry_id, track_id, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(server_id, playlist_id, entry_id) DO UPDATE SET
                    track_id = excluded.track_id,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for (position, entry) in entries.iter().enumerate() {
                statement.execute(params![
                    server_id.as_str(),
                    playlist_id.as_str(),
                    entry.entry_id,
                    entry.track.id.as_str(),
                    position as i64,
                    generation,
                ])?;
            }
            Ok(())
        })
    }

    pub fn load_playlist_detail(
        &self,
        server_id: &ServerId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<Option<PlaylistDetail>> {
        let playlist = self
            .connection
            .query_row(
                "
                SELECT playlist_id, name, track_count, duration_seconds, image_item_id, image_tag
                FROM playlists
                WHERE server_id = ?1 AND playlist_id = ?2
                ",
                params![server_id.as_str(), playlist_id.as_str()],
                playlist_from_row,
            )
            .optional()?;
        let Some(playlist) = playlist else {
            return Ok(None);
        };

        let mut statement = self.connection.prepare(
            "
            SELECT pt.entry_id,
                   t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.duration_seconds, t.favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM playlist_tracks pt
            JOIN tracks t
                ON t.server_id = pt.server_id AND t.track_id = pt.track_id
            WHERE pt.server_id = ?1 AND pt.playlist_id = ?2
            ORDER BY pt.position
            ",
        )?;
        let mut entries = collect_rows(statement.query_map(
            params![server_id.as_str(), playlist_id.as_str()],
            playlist_entry_from_row,
        )?)?;
        let mut tracks = entries
            .iter()
            .map(|entry| entry.track.clone())
            .collect::<Vec<_>>();
        self.attach_track_genres(server_id, &mut tracks)?;
        for (entry, track) in entries.iter_mut().zip(tracks.iter().cloned()) {
            entry.track = track;
        }

        Ok(Some(PlaylistDetail {
            playlist,
            tracks,
            entries,
        }))
    }

    pub fn load_genre_detail(
        &self,
        server_id: &ServerId,
        genre_id: &GenreId,
    ) -> StoreResult<Option<CachedGenreDetail>> {
        let genre = self
            .connection
            .query_row(
                "
                SELECT genre_id, name, album_count, track_count, image_item_id, image_tag
                FROM genres
                WHERE server_id = ?1 AND genre_id = ?2
                ",
                params![server_id.as_str(), genre_id.as_str()],
                genre_from_row,
            )
            .optional()?;
        let Some(genre) = genre else {
            return Ok(None);
        };

        let mut albums_statement = self.connection.prepare(
            "
            SELECT DISTINCT a.album_id, a.title, a.artist, a.artist_id, a.year,
                   a.track_count, a.duration_seconds, a.favorite, a.color_seed,
                   a.image_item_id, a.image_tag
            FROM album_genres ag
            JOIN albums a
                ON a.server_id = ag.server_id AND a.album_id = ag.album_id
            WHERE ag.server_id = ?1 AND ag.genre_name = ?2
            ORDER BY a.title COLLATE NOCASE
            ",
        )?;
        let mut albums = collect_rows(albums_statement.query_map(
            params![server_id.as_str(), genre.name.as_str()],
            album_from_row,
        )?)?;
        self.attach_album_genres(server_id, &mut albums)?;

        let mut tracks_statement = self.connection.prepare(
            "
            SELECT DISTINCT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.duration_seconds, t.favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM track_genres tg
            JOIN tracks t
                ON t.server_id = tg.server_id AND t.track_id = tg.track_id
            WHERE tg.server_id = ?1 AND tg.genre_name = ?2
            ORDER BY t.album COLLATE NOCASE, t.disc_number, t.track_number,
                     t.title COLLATE NOCASE
            ",
        )?;
        let mut tracks = collect_rows(tracks_statement.query_map(
            params![server_id.as_str(), genre.name.as_str()],
            track_from_row,
        )?)?;
        self.attach_track_genres(server_id, &mut tracks)?;

        Ok(Some(CachedGenreDetail {
            genre,
            albums,
            tracks,
        }))
    }

    pub fn load_favorite_tracks(&self, server_id: &ServerId) -> StoreResult<Vec<Track>> {
        let mut statement = self.connection.prepare(
            "
            SELECT track_id, album_id, title, artist, artist_id, album, year,
                   duration_seconds, favorite, disc_number, track_number, image_item_id, image_tag
            FROM tracks
            WHERE server_id = ?1 AND favorite = 1
            ORDER BY title COLLATE NOCASE
            LIMIT 500
            ",
        )?;
        let mut tracks =
            collect_rows(statement.query_map(params![server_id.as_str()], track_from_row)?)?;
        self.attach_track_genres(server_id, &mut tracks)?;
        Ok(tracks)
    }

    pub fn set_album_favorite(
        &self,
        server_id: &ServerId,
        album_id: &AlbumId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.connection.execute(
            "UPDATE albums SET favorite = ?3 WHERE server_id = ?1 AND album_id = ?2",
            params![server_id.as_str(), album_id.as_str(), bool_to_i64(favorite)],
        )?;
        Ok(())
    }

    pub fn set_track_favorite(
        &self,
        server_id: &ServerId,
        track_id: &TrackId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.connection.execute(
            "UPDATE tracks SET favorite = ?3 WHERE server_id = ?1 AND track_id = ?2",
            params![server_id.as_str(), track_id.as_str(), bool_to_i64(favorite)],
        )?;
        Ok(())
    }

    pub fn set_artist_favorite(
        &self,
        server_id: &ServerId,
        artist_id: &ArtistId,
        favorite: bool,
    ) -> StoreResult<()> {
        self.connection.execute(
            "UPDATE artists SET favorite = ?3 WHERE server_id = ?1 AND artist_id = ?2",
            params![
                server_id.as_str(),
                artist_id.as_str(),
                bool_to_i64(favorite)
            ],
        )?;
        self.connection.execute(
            "UPDATE album_artists SET favorite = ?3 WHERE server_id = ?1 AND artist_id = ?2",
            params![
                server_id.as_str(),
                artist_id.as_str(),
                bool_to_i64(favorite)
            ],
        )?;
        Ok(())
    }

    pub fn rename_playlist(
        &self,
        server_id: &ServerId,
        playlist_id: &PlaylistId,
        name: &str,
    ) -> StoreResult<()> {
        self.connection.execute(
            "UPDATE playlists SET name = ?3 WHERE server_id = ?1 AND playlist_id = ?2",
            params![server_id.as_str(), playlist_id.as_str(), name],
        )?;
        self.connection.execute(
            "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = 'playlist' AND item_id = ?2",
            params![server_id.as_str(), playlist_id.as_str()],
        )?;
        self.connection.execute(
            "INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
             VALUES (?1, 'playlist', ?2, ?3, '')",
            params![server_id.as_str(), playlist_id.as_str(), name],
        )?;
        Ok(())
    }

    pub fn save_lyrics(&self, server_id: &ServerId, lyrics: &Lyrics) -> StoreResult<()> {
        let value = serde_json::to_string(lyrics)?;
        let source = match lyrics.source {
            rufin_provider::LyricsSource::Server => "server",
            rufin_provider::LyricsSource::Remote => "remote",
        };
        self.connection.execute(
            "
            INSERT INTO lyrics_cache (server_id, track_id, source, value, updated_at)
            VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id, track_id) DO UPDATE SET
                source = excluded.source,
                value = excluded.value,
                updated_at = excluded.updated_at
            ",
            params![server_id.as_str(), lyrics.track_id.as_str(), source, value],
        )?;
        Ok(())
    }

    pub fn load_lyrics(
        &self,
        server_id: &ServerId,
        track_id: &TrackId,
    ) -> StoreResult<Option<Lyrics>> {
        let value = self
            .connection
            .query_row(
                "
                SELECT value
                FROM lyrics_cache
                WHERE server_id = ?1 AND track_id = ?2
                ",
                params![server_id.as_str(), track_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value
            .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
            .unwrap_or_else(|| Ok(None))
    }

    pub fn search_library(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<SearchResults> {
        let Some(query) = fts_query(query) else {
            return Ok(SearchResults::default());
        };

        Ok(SearchResults {
            albums: self.search_albums(server_id, &query, limit)?,
            tracks: self.search_tracks(server_id, &query, limit)?,
            artists: self.search_artists(server_id, &query, limit)?,
            playlists: self.search_playlists(server_id, &query, limit)?,
        })
    }

    pub fn save_cover_cache_entry(&self, entry: &CoverCacheEntry) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO cover_cache (
                server_id, item_id, image_tag, size, path, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id, item_id, image_tag, size) DO UPDATE SET
                path = excluded.path,
                updated_at = excluded.updated_at
            ",
            params![
                entry.server_id.as_str(),
                entry.item_id,
                entry.image_tag,
                i64::from(entry.size),
                entry.path,
            ],
        )?;
        Ok(())
    }

    pub fn load_cover_cache_entry(
        &self,
        server_id: &ServerId,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<Option<CoverCacheEntry>> {
        self.connection
            .query_row(
                "
                SELECT server_id, item_id, image_tag, size, path
                FROM cover_cache
                WHERE server_id = ?1 AND item_id = ?2 AND image_tag = ?3 AND size = ?4
                ",
                params![server_id.as_str(), item_id, image_tag, i64::from(size)],
                |row| {
                    Ok(CoverCacheEntry {
                        server_id: ServerId::new(row.get::<_, String>(0)?),
                        item_id: row.get(1)?,
                        image_tag: row.get(2)?,
                        size: u32_from_i64(row.get(3)?),
                        path: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn delete_cover_cache_entry(
        &self,
        server_id: &ServerId,
        item_id: &str,
        image_tag: &str,
        size: u32,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            DELETE FROM cover_cache
            WHERE server_id = ?1 AND item_id = ?2 AND image_tag = ?3 AND size = ?4
            ",
            params![server_id.as_str(), item_id, image_tag, i64::from(size)],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> StoreResult<i64> {
        self.connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn foreign_keys_enabled(&self) -> StoreResult<bool> {
        let enabled = self
            .connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?;
        Ok(enabled == 1)
    }

    pub fn journal_mode(&self) -> StoreResult<String> {
        self.connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(StoreError::from)
    }

    pub fn fts5_available(&self) -> StoreResult<bool> {
        let exists = self.connection.query_row(
            "
            SELECT COUNT(*)
            FROM sqlite_master
            WHERE type = 'table' AND name = 'library_fts'
            ",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(exists == 1)
    }

    fn search_albums(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Album>> {
        let mut statement = self.connection.prepare(
            "
            SELECT a.album_id, a.title, a.artist, a.artist_id, a.year,
                   a.track_count, a.duration_seconds, a.favorite, a.color_seed,
                   a.image_item_id, a.image_tag
            FROM library_fts f
            JOIN albums a
                ON a.server_id = f.server_id AND a.album_id = f.item_id
            WHERE f.server_id = ?1
              AND f.item_type = 'album'
              AND library_fts MATCH ?2
            ORDER BY bm25(library_fts)
            LIMIT ?3
            ",
        )?;
        let mut albums = collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64],
            album_from_row,
        )?)?;
        self.attach_album_genres(server_id, &mut albums)?;
        Ok(albums)
    }

    fn search_tracks(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Track>> {
        let mut statement = self.connection.prepare(
            "
            SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id,
                   t.album, t.year, t.duration_seconds, t.favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag
            FROM library_fts f
            JOIN tracks t
                ON t.server_id = f.server_id AND t.track_id = f.item_id
            WHERE f.server_id = ?1
              AND f.item_type = 'track'
              AND library_fts MATCH ?2
            ORDER BY bm25(library_fts)
            LIMIT ?3
            ",
        )?;
        let mut tracks = collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64],
            track_from_row,
        )?)?;
        self.attach_track_genres(server_id, &mut tracks)?;
        Ok(tracks)
    }

    fn search_artists(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Artist>> {
        let mut statement = self.connection.prepare(
            "
            SELECT a.artist_id, a.name, a.album_count, a.track_count, a.favorite,
                   a.image_item_id, a.image_tag
            FROM library_fts f
            JOIN artists a
                ON a.server_id = f.server_id AND a.artist_id = f.item_id
            WHERE f.server_id = ?1
              AND f.item_type = 'artist'
              AND library_fts MATCH ?2
            ORDER BY bm25(library_fts)
            LIMIT ?3
            ",
        )?;
        collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64],
            artist_from_row,
        )?)
    }

    fn search_playlists(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Playlist>> {
        let mut statement = self.connection.prepare(
            "
            SELECT p.playlist_id, p.name, p.track_count, p.duration_seconds,
                   p.image_item_id, p.image_tag
            FROM library_fts f
            JOIN playlists p
                ON p.server_id = f.server_id AND p.playlist_id = f.item_id
            WHERE f.server_id = ?1
              AND f.item_type = 'playlist'
              AND library_fts MATCH ?2
            ORDER BY bm25(library_fts)
            LIMIT ?3
            ",
        )?;
        collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64],
            playlist_from_row,
        )?)
    }

    fn attach_album_genres(&self, server_id: &ServerId, albums: &mut [Album]) -> StoreResult<()> {
        if albums.is_empty() {
            return Ok(());
        }

        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let mut genres = self.load_genre_links(server_id, "album_genres", "album_id", &ids)?;
        for album in albums {
            album.genres = genres.remove(album.id.as_str()).unwrap_or_default();
        }
        Ok(())
    }

    fn attach_track_genres(&self, server_id: &ServerId, tracks: &mut [Track]) -> StoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }

        let ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let mut genres = self.load_genre_links(server_id, "track_genres", "track_id", &ids)?;
        for track in tracks {
            track.genres = genres.remove(track.id.as_str()).unwrap_or_default();
        }
        Ok(())
    }

    fn load_genre_links(
        &self,
        server_id: &ServerId,
        table: &str,
        id_column: &str,
        ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<String>>> {
        let mut by_item = HashMap::<String, Vec<String>>::new();
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT {id_column}, genre_name
                FROM {table}
                WHERE server_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY genre_name COLLATE NOCASE
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (item_id, genre_name) = row?;
                by_item.entry(item_id).or_default().push(genre_name);
            }
        }
        Ok(by_item)
    }

    fn count(&self, table: &str, server_id: &ServerId) -> StoreResult<usize> {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE server_id = ?1");
        let count = self
            .connection
            .query_row(&sql, params![server_id.as_str()], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(count.max(0) as usize)
    }

    fn prune_missing_items(&self, server_id: &ServerId, generation: i64) -> StoreResult<()> {
        self.write_batch(|connection| {
            for table in [
                "albums",
                "tracks",
                "artists",
                "album_artists",
                "genres",
                "album_genres",
                "track_genres",
                "playlists",
                "playlist_tracks",
            ] {
                let sql =
                    format!("DELETE FROM {table} WHERE server_id = ?1 AND sync_generation < ?2");
                connection.execute(&sql, params![server_id.as_str(), generation])?;
            }

            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type = 'album'
                  AND item_id NOT IN (
                    SELECT album_id FROM albums WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type = 'track'
                  AND item_id NOT IN (
                    SELECT track_id FROM tracks WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type IN ('artist', 'album_artist')
                  AND item_id NOT IN (
                    SELECT artist_id FROM artists WHERE server_id = ?1
                    UNION
                    SELECT artist_id FROM album_artists WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            connection.execute(
                "
                DELETE FROM library_fts
                WHERE server_id = ?1
                  AND item_type = 'playlist'
                  AND item_id NOT IN (
                    SELECT playlist_id FROM playlists WHERE server_id = ?1
                  )
                ",
                params![server_id.as_str()],
            )?;
            Ok(())
        })
    }

    fn configure_pragmas(&self, wal: bool) -> StoreResult<()> {
        self.connection.pragma_update(None, "foreign_keys", "ON")?;
        if wal {
            self.connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        Ok(())
    }

    fn write_batch<T>(
        &self,
        operation: impl FnOnce(&Connection) -> StoreResult<T>,
    ) -> StoreResult<T> {
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let result = operation(&self.connection);
        match result {
            Ok(value) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _rollback = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}

pub fn image_cache_key(server_id: &ServerId, item_id: &str, image_tag: &str, size: u32) -> String {
    format!(
        "{}/{}/{}/{}",
        encode_key_part(server_id.as_str()),
        encode_key_part(item_id),
        encode_key_part(image_tag),
        size
    )
}

pub fn lyrics_cache_key(server_id: &ServerId, track_id: &str) -> String {
    format!(
        "{}/{}",
        encode_key_part(server_id.as_str()),
        encode_key_part(track_id)
    )
}

fn saved_server_from_row(row: &Row<'_>) -> rusqlite::Result<SavedServer> {
    Ok(SavedServer {
        server: ServerIdentity {
            id: ServerId::new(row.get::<_, String>(0)?),
            provider: row.get(1)?,
            name: row.get(2)?,
            base_url: row.get(3)?,
        },
        user_id: row.get(4)?,
        username: row.get(5)?,
        trust_invalid_cert: row.get::<_, i64>(6)? == 1,
    })
}

fn album_from_row(row: &Row<'_>) -> rusqlite::Result<Album> {
    let artist_id = row.get::<_, Option<String>>(3)?.map(ArtistId::new);
    Ok(Album {
        id: AlbumId::new(row.get::<_, String>(0)?),
        title: row.get(1)?,
        artist: row.get(2)?,
        artist_id,
        year: u16_from_i64(row.get(4)?),
        track_count: u16_from_i64(row.get(5)?),
        duration_seconds: u32_from_i64(row.get(6)?),
        favorite: row.get::<_, i64>(7)? == 1,
        color_seed: u32_from_i64(row.get(8)?),
        image_ref: image_ref_from_row(row, 9, 10)?,
        genres: Vec::new(),
    })
}

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    track_from_row_at(row, 0)
}

fn playlist_entry_from_row(row: &Row<'_>) -> rusqlite::Result<PlaylistEntry> {
    Ok(PlaylistEntry {
        entry_id: row.get(0)?,
        track: track_from_row_at(row, 1)?,
    })
}

fn track_from_row_at(row: &Row<'_>, offset: usize) -> rusqlite::Result<Track> {
    let artist_id = row.get::<_, Option<String>>(offset + 4)?.map(ArtistId::new);
    Ok(Track {
        id: TrackId::new(row.get::<_, String>(offset)?),
        album_id: AlbumId::new(row.get::<_, String>(offset + 1)?),
        title: row.get(offset + 2)?,
        artist: row.get(offset + 3)?,
        artist_id,
        album: row.get(offset + 5)?,
        year: u16_from_i64(row.get(offset + 6)?),
        duration_seconds: u32_from_i64(row.get(offset + 7)?),
        favorite: row.get::<_, i64>(offset + 8)? == 1,
        disc_number: u16_from_i64(row.get(offset + 9)?),
        track_number: u16_from_i64(row.get(offset + 10)?),
        image_ref: image_ref_from_row(row, offset + 11, offset + 12)?,
        genres: Vec::new(),
    })
}

fn artist_from_row(row: &Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: ArtistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        album_count: u32_from_i64(row.get(2)?),
        track_count: u32_from_i64(row.get(3)?),
        favorite: row.get::<_, i64>(4)? == 1,
        image_ref: image_ref_from_row(row, 5, 6)?,
    })
}

fn image_ref_from_row(
    row: &Row<'_>,
    item_index: usize,
    tag_index: usize,
) -> rusqlite::Result<Option<ImageRef>> {
    let Some(item_id) = row.get::<_, Option<String>>(item_index)? else {
        return Ok(None);
    };
    Ok(Some(ImageRef {
        item_id,
        tag: row.get::<_, Option<String>>(tag_index)?,
    }))
}

fn image_ref_parts(image_ref: Option<&ImageRef>) -> (Option<&str>, Option<&str>) {
    match image_ref {
        Some(image_ref) => (Some(image_ref.item_id.as_str()), image_ref.tag.as_deref()),
        None => (None, None),
    }
}

fn synthesize_album_from_tracks(album_id: &AlbumId, tracks: &[Track]) -> Album {
    let first = tracks
        .first()
        .expect("album fallback requires at least one track");
    Album {
        id: album_id.clone(),
        title: first.album.clone(),
        artist: first.artist.clone(),
        artist_id: first.artist_id.clone(),
        year: first.year,
        track_count: tracks.len().min(usize::from(u16::MAX)) as u16,
        duration_seconds: tracks
            .iter()
            .map(|track| track.duration_seconds)
            .fold(0_u32, u32::saturating_add),
        favorite: tracks.iter().any(|track| track.favorite),
        color_seed: stable_seed(album_id.as_str()),
        image_ref: first.image_ref.clone(),
        genres: first.genres.clone(),
    }
}

fn synthesize_artist_from_links(
    artist_id: &ArtistId,
    albums: &[Album],
    tracks: &[Track],
) -> Artist {
    let name = tracks
        .iter()
        .find(|track| track.artist_id.as_ref() == Some(artist_id))
        .map(|track| track.artist.clone())
        .or_else(|| {
            albums
                .iter()
                .find(|album| album.artist_id.as_ref() == Some(artist_id))
                .map(|album| album.artist.clone())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| artist_id.as_str().to_string());

    let mut album_ids = Vec::new();
    for album in albums {
        if !album_ids.contains(&album.id) {
            album_ids.push(album.id.clone());
        }
    }
    for track in tracks {
        if !album_ids.contains(&track.album_id) {
            album_ids.push(track.album_id.clone());
        }
    }

    Artist {
        id: artist_id.clone(),
        name,
        album_count: album_ids.len().min(u32::MAX as usize) as u32,
        track_count: tracks.len().min(u32::MAX as usize) as u32,
        favorite: false,
        image_ref: albums
            .first()
            .and_then(|album| album.image_ref.clone())
            .or_else(|| tracks.first().and_then(|track| track.image_ref.clone())),
    }
}

fn genre_from_row(row: &Row<'_>) -> rusqlite::Result<Genre> {
    Ok(Genre {
        id: GenreId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        album_count: u32_from_i64(row.get(2)?),
        track_count: u32_from_i64(row.get(3)?),
        image_ref: image_ref_from_row(row, 4, 5)?,
    })
}

fn playlist_from_row(row: &Row<'_>) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: PlaylistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        track_count: u32_from_i64(row.get(2)?),
        duration_seconds: u32_from_i64(row.get(3)?),
        image_ref: image_ref_from_row(row, 4, 5)?,
    })
}

fn stable_seed(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}

fn repair_linked_artists(
    connection: &Connection,
    server_id: &ServerId,
    generation: i64,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO artists (
            server_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT t.server_id,
               t.artist_id,
               MIN(t.artist),
               COUNT(DISTINCT t.album_id),
               COUNT(*),
               MAX(t.favorite),
               ?2
        FROM tracks t
        WHERE t.server_id = ?1
          AND t.artist_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM artists a
              WHERE a.server_id = t.server_id AND a.artist_id = t.artist_id
          )
        GROUP BY t.server_id, t.artist_id
        ",
        params![server_id.as_str(), generation],
    )?;
    connection.execute(
        "
        INSERT INTO album_artists (
            server_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT a.server_id,
               a.artist_id,
               MIN(a.artist),
               COUNT(*),
               COALESCE(SUM(a.track_count), 0),
               MAX(a.favorite),
               ?2
        FROM albums a
        WHERE a.server_id = ?1
          AND a.artist_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM album_artists aa
              WHERE aa.server_id = a.server_id AND aa.artist_id = a.artist_id
          )
        GROUP BY a.server_id, a.artist_id
        ",
        params![server_id.as_str(), generation],
    )?;
    refresh_artist_fts(connection, server_id, "artists", "artist")?;
    refresh_artist_fts(connection, server_id, "album_artists", "album_artist")?;
    Ok(())
}

fn repair_linked_genres(
    connection: &Connection,
    server_id: &ServerId,
    generation: i64,
) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "
        SELECT genre_name
        FROM (
            SELECT genre_name
            FROM album_genres
            WHERE server_id = ?1
            UNION
            SELECT genre_name
            FROM track_genres
            WHERE server_id = ?1
        ) linked
        WHERE TRIM(linked.genre_name) != ''
          AND NOT EXISTS (
              SELECT 1
              FROM genres g
              WHERE g.server_id = ?1 AND g.name = linked.genre_name
          )
        ORDER BY linked.genre_name COLLATE NOCASE
        ",
    )?;
    let genre_names = collect_rows(
        statement.query_map(params![server_id.as_str()], |row| row.get::<_, String>(0))?,
    )?;
    let mut insert = connection.prepare(
        "
        INSERT INTO genres (
            server_id, genre_id, name, album_count, track_count, sync_generation
        )
        VALUES (?1, ?2, ?3, 0, 0, ?4)
        ON CONFLICT(server_id, genre_id) DO UPDATE SET
            name = excluded.name,
            sync_generation = excluded.sync_generation
        ",
    )?;
    for name in genre_names {
        let genre_id = format!("linked:genre:{:08x}", stable_seed(&name));
        insert.execute(params![server_id.as_str(), genre_id, name, generation])?;
    }
    Ok(())
}

fn refresh_artist_fts(
    connection: &Connection,
    server_id: &ServerId,
    table: &str,
    item_type: &str,
) -> StoreResult<()> {
    connection.execute(
        "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = ?2",
        params![server_id.as_str(), item_type],
    )?;
    let sql = format!(
        "
        INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
        SELECT server_id, '{item_type}', artist_id, name, ''
        FROM {table}
        WHERE server_id = ?1
        "
    );
    connection.execute(&sql, params![server_id.as_str()])?;
    Ok(())
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&Row<'_>) -> rusqlite::Result<T>>,
) -> StoreResult<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::from)
}

fn clear_library_cache_on_connection(
    connection: &Connection,
    server_id: &ServerId,
) -> StoreResult<()> {
    for table in [
        "playlist_tracks",
        "playlists",
        "genres",
        "track_genres",
        "album_genres",
        "album_artists",
        "artists",
        "tracks",
        "albums",
        "lyrics_cache",
        "cover_cache",
    ] {
        let sql = format!("DELETE FROM {table} WHERE server_id = ?1");
        connection.execute(&sql, params![server_id.as_str()])?;
    }
    connection.execute(
        "DELETE FROM library_fts WHERE server_id = ?1",
        params![server_id.as_str()],
    )?;
    Ok(())
}

fn fts_query(query: &str) -> Option<String> {
    let tokens = query
        .split_whitespace()
        .filter_map(|token| {
            let token = token
                .chars()
                .filter(|character| character.is_alphanumeric())
                .collect::<String>();
            (!token.is_empty()).then(|| format!("{token}*"))
        })
        .collect::<Vec<_>>();

    (!tokens.is_empty()).then(|| tokens.join(" "))
}

fn bool_to_i64(value: bool) -> i64 {
    i64::from(value)
}

fn u16_from_i64(value: i64) -> u16 {
    value.clamp(0, i64::from(u16::MAX)) as u16
}

fn u32_from_i64(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}

fn encode_key_part(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{CoverCacheEntry, SavedServer, Store, image_cache_key, lyrics_cache_key};
    use rufin_core::{
        Album, AlbumId, AppSettings, Artist, ArtistId, Genre, GenreId, ImageRef, Playlist,
        PlaylistId, QueueEngine, ServerId, ServerIdentity, ThemePreference, Track, TrackId,
    };
    use rufin_provider::{LyricLine, Lyrics, LyricsSource, PlaylistEntry};

    #[test]
    fn migrations_run_from_empty_database() {
        let store = Store::open_memory().expect("open store");

        assert_eq!(store.schema_version().expect("schema version"), 4);
        assert!(store.foreign_keys_enabled().expect("foreign keys"));
        assert!(store.fts5_available().expect("fts5 table"));
    }

    #[test]
    fn migrations_create_library_route_indexes() {
        let store = Store::open_memory().expect("open store");

        for (table, index) in [
            ("albums", "albums_server_title_nocase_idx"),
            ("artists", "artists_server_name_nocase_idx"),
            ("album_artists", "album_artists_server_name_nocase_idx"),
            ("genres", "genres_server_name_nocase_idx"),
            ("playlists", "playlists_server_name_nocase_idx"),
            ("album_genres", "album_genres_server_genre_idx"),
            ("track_genres", "track_genres_server_genre_idx"),
        ] {
            assert!(index_exists(&store, table, index), "{index} should exist");
        }
    }

    #[test]
    fn v2_to_v4_migration_preserves_existing_rows_and_adds_image_columns() {
        let connection = rusqlite::Connection::open_in_memory().expect("open connection");
        connection
            .execute_batch(
                "
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO schema_migrations (version) VALUES (1), (2);

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
                    'jellyfin:server:test', 'jellyfin', 'Test Server',
                    'https://music.example', 'user', 'demo', 0
                );

                CREATE TABLE albums (
                    server_id TEXT NOT NULL,
                    album_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    artist TEXT NOT NULL,
                    artist_id TEXT,
                    year INTEGER NOT NULL,
                    track_count INTEGER NOT NULL,
                    duration_seconds INTEGER NOT NULL,
                    favorite INTEGER NOT NULL,
                    color_seed INTEGER NOT NULL,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, album_id)
                );
                INSERT INTO albums VALUES (
                    'jellyfin:server:test', 'album-1', 'Old Album', 'Old Artist',
                    'artist-1', 2020, 1, 180, 0, 1, 2
                );

                CREATE TABLE tracks (
                    server_id TEXT NOT NULL,
                    track_id TEXT NOT NULL,
                    album_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    artist TEXT NOT NULL,
                    artist_id TEXT,
                    album TEXT NOT NULL,
                    year INTEGER NOT NULL,
                    duration_seconds INTEGER NOT NULL,
                    favorite INTEGER NOT NULL,
                    disc_number INTEGER NOT NULL,
                    track_number INTEGER NOT NULL,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, track_id)
                );
                INSERT INTO tracks VALUES (
                    'jellyfin:server:test', 'track-1', 'album-1', 'Old Track',
                    'Old Artist', 'artist-1', 'Old Album', 2020, 180, 0, 1, 1, 2
                );

                CREATE TABLE artists (
                    server_id TEXT NOT NULL,
                    artist_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    album_count INTEGER NOT NULL,
                    track_count INTEGER NOT NULL,
                    favorite INTEGER NOT NULL,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, artist_id)
                );
                CREATE TABLE album_artists (
                    server_id TEXT NOT NULL,
                    artist_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    album_count INTEGER NOT NULL,
                    track_count INTEGER NOT NULL,
                    favorite INTEGER NOT NULL,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, artist_id)
                );
                CREATE TABLE genres (
                    server_id TEXT NOT NULL,
                    genre_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    album_count INTEGER NOT NULL,
                    track_count INTEGER NOT NULL,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, genre_id)
                );
                CREATE TABLE playlists (
                    server_id TEXT NOT NULL,
                    playlist_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    track_count INTEGER NOT NULL,
                    duration_seconds INTEGER NOT NULL,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, playlist_id)
                );
                ",
            )
            .expect("seed v2 schema");
        let store = Store { connection };

        store.configure_pragmas(true).expect("configure pragmas");
        store.migrate().expect("migrate");

        let server_id = ServerId::new("jellyfin:server:test");
        assert_eq!(store.schema_version().expect("schema version"), 4);
        for table in [
            "albums",
            "tracks",
            "artists",
            "album_artists",
            "genres",
            "playlists",
        ] {
            assert!(table_has_column(&store, table, "image_item_id"));
            assert!(table_has_column(&store, table, "image_tag"));
        }
        assert_eq!(
            store
                .load_albums(&server_id, 0, 10)
                .expect("load albums")
                .items[0]
                .title,
            "Old Album"
        );
        assert_eq!(
            store
                .load_tracks(&server_id, 0, 10)
                .expect("load tracks")
                .items[0]
                .title,
            "Old Track"
        );
    }

    #[test]
    fn v3_to_v4_migration_adds_playlist_entry_ids_and_lyrics_cache() {
        let connection = rusqlite::Connection::open_in_memory().expect("open connection");
        connection
            .execute_batch(
                "
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO schema_migrations (version) VALUES (1), (2), (3);

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
                    'jellyfin:server:test', 'jellyfin', 'Test Server',
                    'https://music.example', 'user', 'demo', 0
                );
                CREATE TABLE albums (
                    server_id TEXT NOT NULL,
                    album_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    artist TEXT NOT NULL,
                    artist_id TEXT,
                    year INTEGER NOT NULL,
                    track_count INTEGER NOT NULL,
                    duration_seconds INTEGER NOT NULL,
                    favorite INTEGER NOT NULL,
                    color_seed INTEGER NOT NULL,
                    image_item_id TEXT,
                    image_tag TEXT,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, album_id)
                );
                INSERT INTO albums VALUES (
                    'jellyfin:server:test', 'album-1', 'Old Album', 'Old Artist',
                    'artist-1', 2020, 1, 180, 0, 1, NULL, NULL, 3
                );
                CREATE TABLE tracks (
                    server_id TEXT NOT NULL,
                    track_id TEXT NOT NULL,
                    album_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    artist TEXT NOT NULL,
                    artist_id TEXT,
                    album TEXT NOT NULL,
                    year INTEGER NOT NULL,
                    duration_seconds INTEGER NOT NULL,
                    favorite INTEGER NOT NULL,
                    disc_number INTEGER NOT NULL,
                    track_number INTEGER NOT NULL,
                    image_item_id TEXT,
                    image_tag TEXT,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, track_id)
                );
                INSERT INTO tracks VALUES (
                    'jellyfin:server:test', 'track-1', 'album-1', 'Old Track',
                    'Old Artist', 'artist-1', 'Old Album', 2020, 180, 0, 1, 1,
                    NULL, NULL, 3
                );
                CREATE TABLE playlists (
                    server_id TEXT NOT NULL,
                    playlist_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    track_count INTEGER NOT NULL,
                    duration_seconds INTEGER NOT NULL,
                    image_item_id TEXT,
                    image_tag TEXT,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, playlist_id)
                );
                INSERT INTO playlists VALUES (
                    'jellyfin:server:test', 'playlist-1', 'Old Playlist', 1, 180,
                    NULL, NULL, 3
                );
                CREATE TABLE playlist_tracks (
                    server_id TEXT NOT NULL,
                    playlist_id TEXT NOT NULL,
                    track_id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    sync_generation INTEGER NOT NULL,
                    PRIMARY KEY (server_id, playlist_id, track_id)
                );
                INSERT INTO playlist_tracks VALUES (
                    'jellyfin:server:test', 'playlist-1', 'track-1', 7, 3
                );
                ",
            )
            .expect("seed v3 schema");
        let store = Store { connection };

        store.configure_pragmas(true).expect("configure pragmas");
        store.migrate().expect("migrate");

        assert_eq!(store.schema_version().expect("schema version"), 4);
        assert!(table_has_column(&store, "playlist_tracks", "entry_id"));
        assert!(table_has_column(&store, "lyrics_cache", "value"));
        let entry_id: String = store
            .connection
            .query_row(
                "SELECT entry_id FROM playlist_tracks WHERE track_id = 'track-1'",
                [],
                |row| row.get(0),
            )
            .expect("playlist entry id");
        assert_eq!(entry_id, "track-1:7");
    }

    #[test]
    fn file_store_uses_wal_journal_mode() {
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
    fn settings_round_trip() {
        let store = Store::open_memory().expect("open store");
        let settings = AppSettings {
            theme_preference: ThemePreference::Dark,
            ..AppSettings::default()
        };

        store.save_settings(&settings).expect("save settings");

        assert_eq!(store.load_settings().expect("load settings"), settings);
    }

    #[test]
    fn missing_settings_return_defaults() {
        let store = Store::open_memory().expect("open store");

        assert_eq!(
            store.load_settings().expect("load settings"),
            AppSettings::default()
        );
    }

    #[test]
    fn queue_snapshot_round_trip_by_server() {
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
    fn active_server_round_trips_without_token() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();

        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active server");

        assert_eq!(store.active_server().expect("active server"), Some(saved));
    }

    #[test]
    fn cached_album_and_track_pages_round_trip() {
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
    fn image_refs_round_trip_for_cached_library_models() {
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
    fn paged_reads_return_items_beyond_previous_snapshot_caps() {
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

    #[test]
    fn playlist_entries_allow_duplicate_tracks_and_keep_entry_ids() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(1);
        let track = track(1, &album);
        let playlist = playlist(1, None);
        let entries = vec![
            PlaylistEntry {
                entry_id: "entry-one".to_string(),
                track: track.clone(),
            },
            PlaylistEntry {
                entry_id: "entry-two".to_string(),
                track: track.clone(),
            },
        ];

        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert tracks");
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

        let detail = store
            .load_playlist_detail(&saved.server.id, &playlist.id)
            .expect("load playlist detail")
            .expect("playlist detail");

        assert_eq!(detail.entries, entries);
        assert_eq!(detail.tracks, vec![track.clone(), track]);
    }

    #[test]
    fn lyrics_cache_round_trips_by_server_and_track() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(1);
        let track = track(1, &album);
        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert track");
        let lyrics = Lyrics {
            track_id: track.id.clone(),
            source: LyricsSource::Remote,
            lines: vec![LyricLine {
                start_millis: Some(12_000),
                text: "hello".to_string(),
            }],
        };

        store
            .save_lyrics(&saved.server.id, &lyrics)
            .expect("save lyrics");

        assert_eq!(
            store
                .load_lyrics(&saved.server.id, &track.id)
                .expect("load lyrics"),
            Some(lyrics)
        );
        assert_eq!(
            store
                .load_lyrics(&ServerId::fake(2), &track.id)
                .expect("load missing lyrics"),
            None
        );
    }

    #[test]
    fn favorite_flag_updates_refresh_cached_models_and_favorite_tracks() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let mut album = album(1);
        album.favorite = false;
        let mut track = track(1, &album);
        track.favorite = false;
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

        store
            .set_album_favorite(&saved.server.id, &album.id, true)
            .expect("favorite album");
        store
            .set_track_favorite(&saved.server.id, &track.id, true)
            .expect("favorite track");
        store
            .set_artist_favorite(&saved.server.id, &artist.id, true)
            .expect("favorite artist");

        assert!(
            store
                .load_albums(&saved.server.id, 0, 1)
                .expect("load albums")
                .items[0]
                .favorite
        );
        assert!(
            store
                .load_tracks(&saved.server.id, 0, 1)
                .expect("load tracks")
                .items[0]
                .favorite
        );
        assert!(
            store
                .load_artists(&saved.server.id, false, 0, 1)
                .expect("load artists")
                .items[0]
                .favorite
        );
        assert_eq!(
            store
                .load_favorite_tracks(&saved.server.id)
                .expect("favorite tracks")
                .len(),
            1
        );
    }

    #[test]
    fn genre_detail_returns_linked_albums_and_tracks() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let mut album = album(1);
        album.genres = vec!["Dream Pop".to_string()];
        let track = track(1, &album);
        let genre = Genre {
            id: GenreId::new("jellyfin:genre:dream-pop"),
            name: "Dream Pop".to_string(),
            album_count: 0,
            track_count: 0,
            image_ref: Some(image_ref("genre-dream-pop", "tag")),
        };

        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert track");
        store
            .upsert_genres(&saved.server.id, std::slice::from_ref(&genre), generation)
            .expect("upsert genre");

        let detail = store
            .load_genre_detail(&saved.server.id, &genre.id)
            .expect("load genre detail")
            .expect("genre detail");

        assert_eq!(detail.genre, genre);
        assert_eq!(detail.albums, vec![album]);
        assert_eq!(detail.tracks, vec![track]);
    }

    #[test]
    fn genre_list_only_returns_music_linked_genres() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let mut album = album(1);
        album.genres = vec!["Dream Pop".to_string()];
        let mut movie_genre = genre(2, None);
        movie_genre.name = "Science Fiction".to_string();
        let mut music_genre = genre(3, None);
        music_genre.name = "Dream Pop".to_string();

        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_genres(
                &saved.server.id,
                &[movie_genre, music_genre.clone()],
                generation,
            )
            .expect("upsert genres");

        let genres = store
            .load_genres(&saved.server.id, 0, 20)
            .expect("load genres");

        assert_eq!(genres.total, 1);
        assert_eq!(genres.items, vec![music_genre]);
    }

    #[test]
    fn refresh_library_counts_repairs_missing_linked_genre_rows() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let mut album = album(1);
        album.genres = vec!["Dream Pop".to_string()];
        let track = track(1, &album);

        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert track");
        store
            .refresh_library_counts(&saved.server.id)
            .expect("refresh counts");

        let genres = store
            .load_genres(&saved.server.id, 0, 20)
            .expect("load genres");

        assert_eq!(genres.total, 1);
        assert_eq!(genres.items[0].name, "Dream Pop");
        assert_eq!(genres.items[0].album_count, 1);
        assert_eq!(genres.items[0].track_count, 1);
    }

    #[test]
    fn album_detail_falls_back_to_tracks_when_album_row_is_missing() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(9);
        let tracks = vec![track(1, &album), track(2, &album)];

        store
            .upsert_tracks(&saved.server.id, &tracks, generation)
            .expect("upsert tracks");

        let detail = store
            .load_album_detail(&saved.server.id, &album.id)
            .expect("load album detail")
            .expect("album detail");

        assert_eq!(detail.0.id, album.id);
        assert_eq!(detail.0.title, album.title);
        assert_eq!(detail.0.artist, album.artist);
        assert_eq!(detail.0.track_count, 2);
        assert_eq!(detail.1, tracks);
    }

    #[test]
    fn refresh_library_counts_uses_cached_tracks() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let mut album = album(1);
        album.track_count = 0;
        album.duration_seconds = 0;
        let tracks = vec![track(1, &album), track(2, &album)];
        let artist = Artist {
            id: ArtistId::fake(1),
            name: "Artist".to_string(),
            album_count: 0,
            track_count: 0,
            favorite: false,
            image_ref: None,
        };

        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, &tracks, generation)
            .expect("upsert tracks");
        store
            .upsert_artists(
                &saved.server.id,
                std::slice::from_ref(&artist),
                false,
                generation,
            )
            .expect("upsert artist");
        store
            .upsert_artists(
                &saved.server.id,
                std::slice::from_ref(&artist),
                true,
                generation,
            )
            .expect("upsert album artist");
        store
            .refresh_library_counts(&saved.server.id)
            .expect("refresh counts");

        let album = store
            .load_albums(&saved.server.id, 0, 1)
            .expect("load albums")
            .items
            .remove(0);

        assert_eq!(album.track_count, 2);
        assert_eq!(
            album.duration_seconds,
            tracks
                .iter()
                .map(|track| track.duration_seconds)
                .sum::<u32>()
        );
        let artist = store
            .load_artists(&saved.server.id, false, 0, 1)
            .expect("load artists")
            .items
            .remove(0);
        let album_artist = store
            .load_artists(&saved.server.id, true, 0, 1)
            .expect("load album artists")
            .items
            .remove(0);

        assert_eq!(artist.album_count, 1);
        assert_eq!(artist.track_count, 2);
        assert_eq!(album_artist.album_count, 1);
        assert_eq!(album_artist.track_count, 2);
    }

    #[test]
    fn refresh_library_counts_repairs_missing_linked_artist_rows() {
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
            .refresh_library_counts(&saved.server.id)
            .expect("refresh counts");

        let artist = store
            .load_artists(&saved.server.id, false, 0, 1)
            .expect("load artists")
            .items
            .remove(0);
        let album_artist = store
            .load_artists(&saved.server.id, true, 0, 1)
            .expect("load album artists")
            .items
            .remove(0);
        let search = store
            .search_library(&saved.server.id, "Artist", 10)
            .expect("search");

        assert_eq!(artist.name, album.artist);
        assert_eq!(artist.album_count, 1);
        assert_eq!(artist.track_count, 2);
        assert_eq!(album_artist.name, album.artist);
        assert_eq!(album_artist.album_count, 1);
        assert_eq!(album_artist.track_count, 2);
        assert_eq!(search.artists, vec![artist]);
    }

    #[test]
    fn refresh_library_counts_preserves_provider_counts_without_relationships() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let artist = Artist {
            id: ArtistId::fake(99),
            name: "Provider Counted".to_string(),
            album_count: 3,
            track_count: 18,
            favorite: false,
            image_ref: None,
        };

        store
            .upsert_artists(
                &saved.server.id,
                std::slice::from_ref(&artist),
                false,
                generation,
            )
            .expect("upsert artist");
        store
            .refresh_library_counts(&saved.server.id)
            .expect("refresh counts");

        let artist = store
            .load_artists(&saved.server.id, false, 0, 1)
            .expect("load artists")
            .items
            .remove(0);

        assert_eq!(artist.album_count, 3);
        assert_eq!(artist.track_count, 18);
    }

    #[test]
    fn artist_detail_uses_album_artist_albums_and_tracks() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(1);
        let artist = Artist {
            id: album.artist_id.clone().expect("album artist id"),
            name: album.artist.clone(),
            album_count: 0,
            track_count: 0,
            favorite: false,
            image_ref: None,
        };
        let mut track = track(1, &album);
        track.artist_id = None;

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
                true,
                generation,
            )
            .expect("upsert album artist");

        let detail = store
            .load_artist_detail(&saved.server.id, &artist.id)
            .expect("load artist detail")
            .expect("artist detail");

        assert_eq!(detail.artist, artist);
        assert_eq!(detail.albums, vec![album]);
        assert_eq!(detail.tracks, vec![track]);
    }

    #[test]
    fn artist_detail_falls_back_to_track_links_when_artist_row_is_missing() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(1);
        let tracks = vec![track(1, &album), track(2, &album)];
        let artist_id = album.artist_id.clone().expect("artist id");

        store
            .upsert_tracks(&saved.server.id, &tracks, generation)
            .expect("upsert tracks");

        let detail = store
            .load_artist_detail(&saved.server.id, &artist_id)
            .expect("load artist detail")
            .expect("artist detail");

        assert_eq!(detail.artist.id, artist_id);
        assert_eq!(detail.artist.name, album.artist);
        assert_eq!(detail.artist.album_count, 1);
        assert_eq!(detail.artist.track_count, 2);
        assert!(detail.albums.is_empty());
        assert_eq!(detail.tracks, tracks);
    }

    #[test]
    fn artist_detail_falls_back_to_album_links_when_artist_row_is_missing() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(1);
        let artist_id = album.artist_id.clone().expect("artist id");
        let mut track = track(1, &album);
        track.artist_id = None;

        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert track");

        let detail = store
            .load_artist_detail(&saved.server.id, &artist_id)
            .expect("load artist detail")
            .expect("artist detail");

        assert_eq!(detail.artist.id, artist_id);
        assert_eq!(detail.artist.name, album.artist);
        assert_eq!(detail.artist.album_count, 1);
        assert_eq!(detail.artist.track_count, 1);
        assert_eq!(detail.albums, vec![album]);
        assert_eq!(detail.tracks, vec![track]);
    }

    #[test]
    fn search_uses_local_fts_rows() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(7);
        let track = track(4, &album);

        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert track");

        let results = store
            .search_library(&saved.server.id, "Album 7", 10)
            .expect("search");

        assert_eq!(results.albums, vec![album]);
        assert_eq!(results.tracks, vec![track]);
    }

    #[test]
    fn sync_generation_prunes_missing_items_after_success() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let album_one = album(1);
        let album_two = album(2);

        let first_generation = store.begin_sync(&saved.server.id).expect("begin first");
        store
            .upsert_albums(
                &saved.server.id,
                &[album_one.clone(), album_two],
                first_generation,
            )
            .expect("upsert first");
        store
            .complete_sync(&saved.server.id, first_generation)
            .expect("complete first");

        let second_generation = store.begin_sync(&saved.server.id).expect("begin second");
        store
            .upsert_albums(&saved.server.id, &[album_one], second_generation)
            .expect("upsert second");
        store
            .complete_sync(&saved.server.id, second_generation)
            .expect("complete second");

        let albums = store
            .load_albums(&saved.server.id, 0, 10)
            .expect("load albums");
        assert_eq!(albums.total, 1);
    }

    #[test]
    fn cover_cache_index_round_trips() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let entry = cover_entry(&saved.server.id);

        store
            .save_cover_cache_entry(&entry)
            .expect("save cover cache");

        assert_eq!(
            store
                .load_cover_cache_entry(&saved.server.id, "album-one", "tag-one", 256)
                .expect("load cover cache"),
            Some(entry)
        );
    }

    #[test]
    fn cover_cache_index_can_delete_missing_entries() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let entry = cover_entry(&saved.server.id);

        store
            .save_cover_cache_entry(&entry)
            .expect("save cover cache");
        store
            .delete_cover_cache_entry(&saved.server.id, "album-one", "tag-one", 256)
            .expect("delete cover cache");

        assert_eq!(
            store
                .load_cover_cache_entry(&saved.server.id, "album-one", "tag-one", 256)
                .expect("load cover cache"),
            None
        );
    }

    #[test]
    fn clear_library_cache_removes_library_search_and_cover_rows_only() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        let settings = AppSettings {
            theme_preference: ThemePreference::Dark,
            ..AppSettings::default()
        };
        let mut queue = QueueEngine::new(saved.server.id.clone());
        queue.append(&track(1, &album(1)));

        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active");
        store.save_settings(&settings).expect("save settings");
        store
            .save_queue_snapshot(&queue.snapshot())
            .expect("save queue");
        seed_cached_library(&store, &saved.server.id);
        store
            .save_cover_cache_entry(&cover_entry(&saved.server.id))
            .expect("save cover cache");

        store
            .clear_library_cache(&saved.server.id)
            .expect("clear cache");

        assert_eq!(store.active_server().expect("active server"), Some(saved));
        assert_eq!(store.load_settings().expect("settings"), settings);
        assert_eq!(
            store
                .load_queue_snapshot(&queue.snapshot().server_id)
                .expect("queue"),
            Some(queue.snapshot())
        );
        assert_eq!(
            store
                .load_albums(&queue.snapshot().server_id, 0, 10)
                .expect("albums")
                .total,
            0
        );
        assert!(
            store
                .search_library(&queue.snapshot().server_id, "Album", 10)
                .expect("search")
                .albums
                .is_empty()
        );
        assert_eq!(
            store
                .load_cover_cache_entry(&queue.snapshot().server_id, "album-one", "tag-one", 256)
                .expect("cover cache"),
            None
        );
        let sync_state = store
            .sync_state(&queue.snapshot().server_id)
            .expect("sync state");
        assert_eq!(sync_state.generation, 0);
        assert_eq!(sync_state.status, "idle");
        assert_eq!(sync_state.last_error, None);
    }

    #[test]
    fn forget_server_removes_server_local_state_but_keeps_app_settings() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        let settings = AppSettings {
            theme_preference: ThemePreference::Dark,
            ..AppSettings::default()
        };
        let mut queue = QueueEngine::new(saved.server.id.clone());
        queue.append(&track(1, &album(1)));

        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active");
        store.save_settings(&settings).expect("save settings");
        store
            .save_queue_snapshot(&queue.snapshot())
            .expect("save queue");
        seed_cached_library(&store, &saved.server.id);

        store
            .forget_server(&saved.server.id)
            .expect("forget server");

        assert_eq!(store.active_server().expect("active server"), None);
        assert!(store.list_servers().expect("servers").is_empty());
        assert_eq!(
            store
                .load_queue_snapshot(&saved.server.id)
                .expect("queue snapshot"),
            None
        );
        assert_eq!(store.load_settings().expect("settings"), settings);
        assert_eq!(
            store
                .load_tracks(&saved.server.id, 0, 10)
                .expect("tracks")
                .total,
            0
        );
        assert!(
            store.sync_state(&saved.server.id).is_err(),
            "forgotten server should not have sync state"
        );
    }

    #[test]
    fn cache_keys_are_stable_and_path_safe() {
        let server_id = ServerId::new("server:one");

        assert_eq!(
            image_cache_key(&server_id, "album/one", "tag:two", 256),
            "server_one/album_one/tag_two/256"
        );
        assert_eq!(
            lyrics_cache_key(&server_id, "track/one"),
            "server_one/track_one"
        );
    }

    fn saved_server() -> SavedServer {
        SavedServer {
            server: ServerIdentity {
                id: ServerId::new("jellyfin:server:test"),
                provider: "jellyfin".to_string(),
                name: "Test Server".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        }
    }

    fn album(number: u32) -> Album {
        Album {
            id: AlbumId::fake(number),
            title: format!("Album {number}"),
            artist: "Artist".to_string(),
            artist_id: Some(ArtistId::fake(1)),
            year: 2026,
            track_count: 2,
            duration_seconds: 360,
            favorite: number == 2,
            color_seed: number,
            image_ref: None,
            genres: Vec::new(),
        }
    }

    fn album_with_image(number: u32) -> Album {
        Album {
            image_ref: Some(image_ref(
                format!("album-{number}"),
                format!("album-tag-{number}"),
            )),
            genres: vec!["Dream Pop".to_string()],
            ..album(number)
        }
    }

    fn artist(number: u32, image_ref: Option<ImageRef>) -> Artist {
        Artist {
            id: ArtistId::fake(number),
            name: format!("Artist {number}"),
            album_count: 1,
            track_count: 2,
            favorite: false,
            image_ref,
        }
    }

    fn genre(number: u32, image_ref: Option<ImageRef>) -> Genre {
        Genre {
            id: GenreId::fake(number),
            name: format!("Genre {number}"),
            album_count: 1,
            track_count: 2,
            image_ref,
        }
    }

    fn playlist(number: u32, image_ref: Option<ImageRef>) -> Playlist {
        Playlist {
            id: PlaylistId::fake(number),
            name: format!("Playlist {number}"),
            track_count: 2,
            duration_seconds: 360,
            image_ref,
        }
    }

    fn image_ref(item_id: impl Into<String>, tag: impl Into<String>) -> ImageRef {
        ImageRef::new(item_id, Some(tag.into()))
    }

    fn table_has_column(store: &Store, table: &str, column: &str) -> bool {
        let mut statement = store
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("table info");
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query columns");
        columns
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect columns")
            .iter()
            .any(|name| name == column)
    }

    fn index_exists(store: &Store, table: &str, index: &str) -> bool {
        let mut statement = store
            .connection
            .prepare(&format!("PRAGMA index_list({table})"))
            .expect("index list");
        let indexes = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query indexes");
        indexes
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect indexes")
            .iter()
            .any(|name| name == index)
    }

    fn seed_cached_library(store: &Store, server_id: &ServerId) {
        let generation = store.begin_sync(server_id).expect("begin sync");
        let album = album(1);
        let track = track(1, &album);
        store
            .upsert_albums(server_id, std::slice::from_ref(&album), generation)
            .expect("upsert albums");
        store
            .upsert_tracks(server_id, std::slice::from_ref(&track), generation)
            .expect("upsert tracks");
        store
            .complete_sync(server_id, generation)
            .expect("complete sync");
    }

    fn cover_entry(server_id: &ServerId) -> CoverCacheEntry {
        CoverCacheEntry {
            server_id: server_id.clone(),
            item_id: "album-one".to_string(),
            image_tag: "tag-one".to_string(),
            size: 256,
            path: "/tmp/rufin-cover.jpg".to_string(),
        }
    }

    fn track(number: u32, album: &Album) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id: album.id.clone(),
            title: format!("Track {number}"),
            artist: album.artist.clone(),
            artist_id: album.artist_id.clone(),
            album: album.title.clone(),
            year: album.year,
            duration_seconds: 180,
            favorite: number == 1,
            disc_number: 1,
            track_number: number as u16,
            image_ref: album.image_ref.clone(),
            genres: album.genres.clone(),
        }
    }
}
