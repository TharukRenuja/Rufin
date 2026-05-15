use std::path::Path;

use rufin_core::{
    Album, AlbumId, AppSettings, Artist, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind,
    Playlist, PlaylistId, QueueSnapshot, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_provider::{PagedResponse, SearchResults};
use rusqlite::{Connection, OptionalExtension, Row, params};
use thiserror::Error;

const SETTINGS_KEY: &str = "default";
const SCHEMA_VERSION: i64 = 2;

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
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, artist_id)
            );

            CREATE TABLE IF NOT EXISTS genres (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                genre_id TEXT NOT NULL,
                name TEXT NOT NULL,
                album_count INTEGER NOT NULL,
                track_count INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, genre_id)
            );

            CREATE TABLE IF NOT EXISTS playlists (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                track_count INTEGER NOT NULL,
                duration_seconds INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, playlist_id)
            );

            CREATE TABLE IF NOT EXISTS playlist_tracks (
                server_id TEXT NOT NULL REFERENCES servers(server_id) ON DELETE CASCADE,
                playlist_id TEXT NOT NULL,
                track_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                sync_generation INTEGER NOT NULL,
                PRIMARY KEY (server_id, playlist_id, track_id)
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
            CREATE INDEX IF NOT EXISTS tracks_server_title_idx
                ON tracks(server_id, title);
            CREATE INDEX IF NOT EXISTS tracks_server_album_idx
                ON tracks(server_id, album_id, disc_number, track_number);
            ",
        )?;
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
                SELECT server_id, generation, status, last_error
                FROM sync_state
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
                |row| {
                    Ok(SyncState {
                        server_id: ServerId::new(row.get::<_, String>(0)?),
                        generation: row.get(1)?,
                        status: row.get(2)?,
                        last_error: row.get(3)?,
                    })
                },
            )
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
                    duration_seconds, favorite, color_seed, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(server_id, album_id) DO UPDATE SET
                    title = excluded.title,
                    artist = excluded.artist,
                    artist_id = excluded.artist_id,
                    year = excluded.year,
                    track_count = excluded.track_count,
                    duration_seconds = excluded.duration_seconds,
                    favorite = excluded.favorite,
                    color_seed = excluded.color_seed,
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
                    generation,
                ])?;
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
                    sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
                    generation,
                ])?;
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
                    sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(server_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    favorite = excluded.favorite,
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
                statement.execute(params![
                    server_id.as_str(),
                    artist.id.as_str(),
                    artist.name,
                    i64::from(artist.album_count),
                    i64::from(artist.track_count),
                    bool_to_i64(artist.favorite),
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
                    server_id, genre_id, name, album_count, track_count, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(server_id, genre_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for genre in genres {
                statement.execute(params![
                    server_id.as_str(),
                    genre.id.as_str(),
                    genre.name,
                    i64::from(genre.album_count),
                    i64::from(genre.track_count),
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
                    sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(server_id, playlist_id) DO UPDATE SET
                    name = excluded.name,
                    track_count = excluded.track_count,
                    duration_seconds = excluded.duration_seconds,
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
                statement.execute(params![
                    server_id.as_str(),
                    playlist.id.as_str(),
                    playlist.name,
                    i64::from(playlist.track_count),
                    i64::from(playlist.duration_seconds),
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
                   duration_seconds, favorite, color_seed
            FROM albums
            WHERE server_id = ?1
            ORDER BY title COLLATE NOCASE
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let items = collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            album_from_row,
        )?)?;
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
                       duration_seconds, favorite, color_seed
                FROM albums
                WHERE server_id = ?1 AND album_id = ?2
                ",
                params![server_id.as_str(), album_id.as_str()],
                album_from_row,
            )
            .optional()?;
        let Some(album) = album else {
            return Ok(None);
        };
        let mut statement = self.connection.prepare(
            "
            SELECT track_id, album_id, title, artist, artist_id, album, year,
                   duration_seconds, favorite, disc_number, track_number
            FROM tracks
            WHERE server_id = ?1 AND album_id = ?2
            ORDER BY disc_number, track_number, title COLLATE NOCASE
            ",
        )?;
        let tracks = collect_rows(statement.query_map(
            params![server_id.as_str(), album_id.as_str()],
            track_from_row,
        )?)?;
        Ok(Some((album, tracks)))
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
                   duration_seconds, favorite, disc_number, track_number
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
            SELECT artist_id, name, album_count, track_count, favorite
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
        let total = self.count("genres", server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT genre_id, name, album_count, track_count
            FROM genres
            WHERE server_id = ?1
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

    pub fn load_playlists(
        &self,
        server_id: &ServerId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Playlist>> {
        let total = self.count("playlists", server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT playlist_id, name, track_count, duration_seconds
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

    pub fn load_favorite_tracks(&self, server_id: &ServerId) -> StoreResult<Vec<Track>> {
        let mut statement = self.connection.prepare(
            "
            SELECT track_id, album_id, title, artist, artist_id, album, year,
                   duration_seconds, favorite, disc_number, track_number
            FROM tracks
            WHERE server_id = ?1 AND favorite = 1
            ORDER BY title COLLATE NOCASE
            LIMIT 500
            ",
        )?;
        collect_rows(statement.query_map(params![server_id.as_str()], track_from_row)?)
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
                   a.track_count, a.duration_seconds, a.favorite, a.color_seed
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
        collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64],
            album_from_row,
        )?)
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
                   t.disc_number, t.track_number
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
        collect_rows(statement.query_map(
            params![server_id.as_str(), query, limit as i64],
            track_from_row,
        )?)
    }

    fn search_artists(
        &self,
        server_id: &ServerId,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<Artist>> {
        let mut statement = self.connection.prepare(
            "
            SELECT a.artist_id, a.name, a.album_count, a.track_count, a.favorite
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
            SELECT p.playlist_id, p.name, p.track_count, p.duration_seconds
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
    })
}

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    let artist_id = row.get::<_, Option<String>>(4)?.map(ArtistId::new);
    Ok(Track {
        id: TrackId::new(row.get::<_, String>(0)?),
        album_id: AlbumId::new(row.get::<_, String>(1)?),
        title: row.get(2)?,
        artist: row.get(3)?,
        artist_id,
        album: row.get(5)?,
        year: u16_from_i64(row.get(6)?),
        duration_seconds: u32_from_i64(row.get(7)?),
        favorite: row.get::<_, i64>(8)? == 1,
        disc_number: u16_from_i64(row.get(9)?),
        track_number: u16_from_i64(row.get(10)?),
    })
}

fn artist_from_row(row: &Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: ArtistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        album_count: u32_from_i64(row.get(2)?),
        track_count: u32_from_i64(row.get(3)?),
        favorite: row.get::<_, i64>(4)? == 1,
    })
}

fn genre_from_row(row: &Row<'_>) -> rusqlite::Result<Genre> {
    Ok(Genre {
        id: GenreId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        album_count: u32_from_i64(row.get(2)?),
        track_count: u32_from_i64(row.get(3)?),
    })
}

fn playlist_from_row(row: &Row<'_>) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: PlaylistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        track_count: u32_from_i64(row.get(2)?),
        duration_seconds: u32_from_i64(row.get(3)?),
    })
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
        "album_artists",
        "artists",
        "tracks",
        "albums",
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
        Album, AlbumId, AppSettings, ArtistId, QueueEngine, ServerId, ServerIdentity,
        ThemePreference, Track, TrackId,
    };

    #[test]
    fn migrations_run_from_empty_database() {
        let store = Store::open_memory().expect("open store");

        assert_eq!(store.schema_version().expect("schema version"), 2);
        assert!(store.foreign_keys_enabled().expect("foreign keys"));
        assert!(store.fts5_available().expect("fts5 table"));
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
        }
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
        }
    }
}
