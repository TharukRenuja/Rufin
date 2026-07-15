use super::*;
pub(in crate::controller) fn load_folder_detail(
    store: &StoreHandle,
    runtime: &Runtime,
    active_source: &ActiveSourceSlot,
    path: &[FolderId],
) -> Result<FolderDetail, String> {
    let saved = store
        .with_store(|store| store.active_source())?
        .ok_or_else(|| "No active server.".to_string())?;
    let selected_music_folder_id =
        store.with_store(|store| store.selected_music_folder_id(&saved.source_id))?;
    let active = selected_active_source(active_source, &saved.source_id)?;
    let browser = active
        .folders
        .as_ref()
        .ok_or_else(|| "Folder browsing is not supported by the active source.".to_string())?;
    let folder_id = path.last();
    runtime
        .block_on(browser.folder(folder_id, selected_music_folder_id.as_ref()))
        .map_err(|error| error.to_string())
}

pub(in crate::controller) fn sync_playlist_mutation(
    runtime: &Runtime,
    active: &ActiveSource,
    operation: SourcePlaylistOperation,
    before: &library::PlaylistDetail,
    after: &library::PlaylistDetail,
) -> Result<library::PlaylistDetail, String> {
    let reader = match operation {
        SourcePlaylistOperation::AddTracks => {
            let operation = active.playlist_rows.add_tracks.as_ref().ok_or_else(|| {
                "Adding tracks is not supported for native playlists by the active source."
                    .to_string()
            })?;
            let before_ids = before
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<HashSet<_>>();
            let added = after
                .entries
                .iter()
                .filter(|entry| !before_ids.contains(entry.entry_id.as_str()))
                .map(|entry| entry.track.id.clone())
                .collect::<Vec<_>>();
            if !added.is_empty() {
                runtime
                    .block_on(
                        operation
                            .executor
                            .add_playlist_tracks(&before.playlist.id, &added),
                    )
                    .map_err(|error| error.to_string())?;
            }
            &operation.readback
        }
        SourcePlaylistOperation::RemoveEntries => {
            let operation = active
                .playlist_rows
                .remove_entries
                .as_ref()
                .ok_or_else(|| {
                    "Removing entries is not supported for native playlists by the active source."
                        .to_string()
                })?;
            let after_ids = after
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<HashSet<_>>();
            let removed = before
                .entries
                .iter()
                .filter(|entry| !after_ids.contains(entry.entry_id.as_str()))
                .map(|entry| entry.entry_id.clone())
                .collect::<Vec<_>>();
            if !removed.is_empty() {
                runtime
                    .block_on(
                        operation
                            .executor
                            .remove_playlist_entries(&before.playlist.id, &removed),
                    )
                    .map_err(|error| error.to_string())?;
            }
            &operation.readback
        }
        SourcePlaylistOperation::ReorderEntries => {
            let operation = active.playlist_rows.move_entry.as_ref().ok_or_else(|| {
                "Reordering entries is not supported for native playlists by the active source."
                    .to_string()
            })?;
            for (new_index, entry) in after.entries.iter().enumerate() {
                let Some(old_index) = before
                    .entries
                    .iter()
                    .position(|candidate| candidate.entry_id == entry.entry_id)
                else {
                    continue;
                };
                if old_index != new_index {
                    runtime
                        .block_on(operation.executor.move_playlist_entry(
                            &before.playlist.id,
                            &entry.entry_id,
                            new_index,
                        ))
                        .map_err(|error| error.to_string())?;
                }
            }
            &operation.readback
        }
        SourcePlaylistOperation::Rename | SourcePlaylistOperation::Delete => {
            return Err("The requested operation does not mutate playlist entries.".to_string());
        }
    };
    runtime
        .block_on(reader.playlist_detail(&before.playlist.id))
        .map_err(|error| error.to_string())
}

pub(in crate::controller) fn playlist_entries_for_tracks(
    playlist_id: &PlaylistId,
    tracks: &[Track],
) -> Vec<PlaylistEntry> {
    let prefix = unique_millis().unwrap_or(0);
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| PlaylistEntry {
            entry_id: format!("{}:{prefix}:{index}", playlist_id.as_str()),
            track: track.clone(),
        })
        .collect()
}

pub(in crate::controller) fn unique_millis() -> Option<u128> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

pub(in crate::controller) fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.config_dir().to_path_buf())
}

pub(in crate::controller) fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.cache_dir().to_path_buf())
}

pub(in crate::controller) fn app_cache_database_path() -> PathBuf {
    cache_dir()
        .map(|dir| cache_db_path(&dir))
        .unwrap_or_else(|| PathBuf::from(CACHE_DATABASE_FILE_NAME))
}

pub(in crate::controller) fn cache_db_path(cache_dir: &Path) -> PathBuf {
    cache_dir
        .join(STORE_DIR_NAME)
        .join(CACHE_DATABASE_FILE_NAME)
}

pub(in crate::controller) fn app_settings_path() -> PathBuf {
    config_dir()
        .map(|dir| settings_file_path(&dir))
        .unwrap_or_else(|| PathBuf::from(SETTINGS_FILE_NAME))
}

pub(in crate::controller) fn settings_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE_NAME)
}

pub(in crate::controller) fn config_secrets_path() -> PathBuf {
    config_dir()
        .map(|dir| config_secret_path(&dir))
        .unwrap_or_else(|| PathBuf::from(CONFIG_SECRETS_FILE_NAME))
}

pub(in crate::controller) fn config_secret_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CONFIG_SECRETS_FILE_NAME)
}

pub(in crate::controller) fn artwork_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|dir| artwork_cache_path(&dir))
}

pub(in crate::controller) fn artwork_cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(ARTWORK_CACHE_DIR_NAME)
}

pub(in crate::controller) fn lyrics_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(LYRICS_CACHE_DIR_NAME)
}

pub(in crate::controller) fn playback_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(PLAYBACK_CACHE_DIR_NAME)
}

pub(in crate::controller) fn waveform_cache_dir(cache_dir: &Path) -> PathBuf {
    playback_cache_dir(cache_dir).join(WAVEFORM_CACHE_DIR_NAME)
}

pub(in crate::controller) fn ensure_app_cache_dirs(cache_dir: &Path) -> Result<(), String> {
    for dir in [
        cache_db_path(cache_dir).parent().map(Path::to_path_buf),
        Some(artwork_cache_path(cache_dir)),
        Some(lyrics_cache_dir(cache_dir)),
        Some(playback_cache_dir(cache_dir)),
    ]
    .into_iter()
    .flatten()
    {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(in crate::controller) fn remove_waveform_tmp(cache_dir: &Path) -> Result<(), String> {
    remove_dir_if_exists(
        &cache_dir
            .join(TMP_CACHE_DIR_NAME)
            .join(WAVEFORM_CACHE_DIR_NAME),
    )?;
    match fs::remove_dir(cache_dir.join(TMP_CACHE_DIR_NAME)) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(in crate::controller) fn waveform_cache_path_for_key(key: &str) -> Option<PathBuf> {
    cache_dir().map(|dir| waveform_cache_dir(&dir).join(key))
}

pub(in crate::controller) fn restrict_settings_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[track_caller]
pub(in crate::controller) fn clear_disk_waveform_cache(source_id: &SourceId) -> Result<(), String> {
    let Some(path) =
        cache_dir().map(|dir| waveform_cache_dir(&dir).join(encode_key_part(source_id.as_str())))
    else {
        return Ok(());
    };
    let caller = std::panic::Location::caller();
    info!(
        source_id = %source_id,
        path = %path.display(),
        caller_file = caller.file(),
        caller_line = caller.line(),
        "clearing disk waveform cache"
    );
    remove_dir_if_exists(&path)
}

#[track_caller]
pub(in crate::controller) fn clear_store_disk_waveform_cache(
    store: &StoreHandle,
    source_id: &SourceId,
) -> Result<(), String> {
    if !store.uses_disk_storage() {
        return Ok(());
    }
    clear_disk_waveform_cache(source_id)
}

pub(in crate::controller) fn remove_dir_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(in crate::controller) fn encode_key_part(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '_',
        })
        .collect()
}
