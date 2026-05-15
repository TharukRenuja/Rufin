use std::path::Path;

use rufin_core::{AppSettings, QueueSnapshot, ServerId};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

const SETTINGS_KEY: &str = "default";
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub type StoreResult<T> = Result<T, StoreError>;

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
            ",
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

    fn configure_pragmas(&self, wal: bool) -> StoreResult<()> {
        self.connection.pragma_update(None, "foreign_keys", "ON")?;
        if wal {
            self.connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        Ok(())
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

    use super::{Store, image_cache_key, lyrics_cache_key};
    use rufin_core::{
        AlbumId, AppSettings, QueueEngine, ServerId, ThemePreference, Track, TrackId,
    };

    #[test]
    fn migrations_run_from_empty_database() {
        let store = Store::open_memory().expect("open store");

        assert_eq!(store.schema_version().expect("schema version"), 1);
        assert!(store.foreign_keys_enabled().expect("foreign keys"));
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
        queue.append(&track(1));

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

    fn track(number: u32) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id: AlbumId::fake(1),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            album: "Album".to_string(),
            year: 2026,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
        }
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
}
