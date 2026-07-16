use directories::ProjectDirs;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) const CACHE_DATABASE_FILE_NAME: &str = "rufin-cache.sqlite";
pub(super) const SETTINGS_FILE_NAME: &str = "settings.json";
const CONFIG_SECRETS_FILE_NAME: &str = "secrets.json";
const STORE_DIR_NAME: &str = "store";
const ARTWORK_CACHE_DIR_NAME: &str = "covers";
const LYRICS_CACHE_DIR_NAME: &str = "lyrics";
const PLAYBACK_CACHE_DIR_NAME: &str = "playback";

fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.config_dir().to_path_buf())
}

pub(super) fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.cache_dir().to_path_buf())
}

pub(super) fn app_cache_database_path() -> PathBuf {
    cache_dir()
        .map(|dir| dir.join(STORE_DIR_NAME).join(CACHE_DATABASE_FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(CACHE_DATABASE_FILE_NAME))
}

pub(super) fn app_settings_path() -> PathBuf {
    config_dir()
        .map(|dir| dir.join(SETTINGS_FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(SETTINGS_FILE_NAME))
}

pub(super) fn config_secrets_path() -> PathBuf {
    config_dir()
        .map(|dir| dir.join(CONFIG_SECRETS_FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(CONFIG_SECRETS_FILE_NAME))
}

pub(super) fn artwork_cache_dir() -> PathBuf {
    cache_dir()
        .map(|dir| dir.join(ARTWORK_CACHE_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(ARTWORK_CACHE_DIR_NAME))
}

pub(super) fn playback_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(PLAYBACK_CACHE_DIR_NAME)
}

pub(super) fn ensure_app_cache_dirs(cache_dir: &Path) -> Result<(), String> {
    for name in [
        STORE_DIR_NAME,
        ARTWORK_CACHE_DIR_NAME,
        LYRICS_CACHE_DIR_NAME,
        PLAYBACK_CACHE_DIR_NAME,
    ] {
        fs::create_dir_all(cache_dir.join(name)).map_err(|error| error.to_string())?;
    }
    Ok(())
}
