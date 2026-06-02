use super::*;
use std::fmt::Write as _;

const JELLYFIN_DEVICE_ID_RANDOM_BYTES: usize = 16;

pub(in crate::controller) fn lyrics_from_text(
    track_id: TrackId,
    result: &LyricsSearchResult,
) -> Lyrics {
    let content = lyrics_result_content(result).unwrap_or_default();
    Lyrics {
        track_id,
        source: rufin_provider::LyricsSource::Remote,
        lines: content
            .lines()
            .filter_map(lyric_line_from_text)
            .collect::<Vec<_>>(),
    }
}

pub(in crate::controller) fn lyric_line_from_text(line: &str) -> Option<rufin_provider::LyricLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((start_millis, text)) = parse_lrc_timestamp(trimmed) {
        return Some(rufin_provider::LyricLine {
            text: text.to_string(),
            start_millis: Some(start_millis),
        });
    }
    if trimmed.starts_with('[') && trimmed.contains(']') {
        return None;
    }
    Some(rufin_provider::LyricLine {
        text: trimmed.to_string(),
        start_millis: None,
    })
}

pub(in crate::controller) fn parse_lrc_timestamp(line: &str) -> Option<(u64, &str)> {
    let timestamp_end = line.find(']')?;
    let timestamp = line.get(1..timestamp_end)?;
    let (minutes, seconds) = timestamp.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let (seconds, fraction) = seconds
        .split_once('.')
        .map(|(seconds, fraction)| (seconds, Some(fraction)))
        .unwrap_or((seconds, None));
    let seconds = seconds.parse::<u64>().ok()?;
    let fraction_millis = match fraction {
        Some(fraction) => fraction_to_millis(fraction)?,
        None => 0,
    };
    Some((
        (minutes * 60 + seconds) * 1_000 + fraction_millis,
        line.get(timestamp_end + 1..)?.trim(),
    ))
}

pub(in crate::controller) fn fraction_to_millis(fraction: &str) -> Option<u64> {
    let mut millis = 0_u64;
    for (index, character) in fraction.chars().take(3).enumerate() {
        let digit = character.to_digit(10)? as u64;
        millis += digit
            * match index {
                0 => 100,
                1 => 10,
                _ => 1,
            };
    }
    Some(millis)
}

pub(in crate::controller) fn lyrics_search_for_settings(
    settings: &AppSettings,
) -> JellyfinLyricsSearch {
    if settings.private_mode || !settings.external_lyrics_enabled {
        JellyfinLyricsSearch::ServerOnly
    } else if settings.prefer_server_lyrics {
        JellyfinLyricsSearch::ServerThenRemote
    } else {
        JellyfinLyricsSearch::RemoteThenServer
    }
}

pub(in crate::controller) fn cached_lyrics_allowed(
    lyrics: &Lyrics,
    search: JellyfinLyricsSearch,
) -> bool {
    match lyrics.source {
        rufin_provider::LyricsSource::Local => true,
        rufin_provider::LyricsSource::Server => true,
        rufin_provider::LyricsSource::Remote => !matches!(search, JellyfinLyricsSearch::ServerOnly),
    }
}

pub(in crate::controller) fn provider_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<LoadedProvider, String> {
    let _unused = runtime;
    if saved.server.provider == LOCAL_PROVIDER_ID
        && saved.server.id.as_str() == LOCAL_SOURCE_SERVER_ID
    {
        let settings = load_settings_from_store(store);
        let manifest_cache = load_local_manifest_cache(store, &saved.server.id)?;
        return LocalProvider::from_roots_with_manifest_cache(
            local_folder_paths(&settings),
            saved.server.clone(),
            manifest_cache,
        )
        .map(LoadedProvider::Local)
        .map_err(|error| error.to_string());
    }
    if saved.server.provider == LOCAL_PROVIDER_ID {
        let manifest_cache = load_local_manifest_cache(store, &saved.server.id)?;
        return LocalProvider::from_roots_with_manifest_cache(
            vec![PathBuf::from(&saved.server.base_url)],
            saved.server.clone(),
            manifest_cache,
        )
        .map(LoadedProvider::Local)
        .map_err(|error| error.to_string());
    }
    let device_id = if saved.server.provider == "jellyfin" {
        Some(ensure_jellyfin_device_id(store)?)
    } else {
        None
    };
    let token = secrets
        .load_token(&saved.server.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No saved token found for the active server.".to_string())?;
    let session = SavedProviderSession {
        server: saved.server.clone(),
        user_id: saved.user_id.clone(),
        username: saved.username.clone(),
        trust_invalid_cert: saved.trust_invalid_cert,
        access_token: token,
        device_id,
    };
    provider_from_saved(session).map_err(|error| error.to_string())
}
fn load_local_manifest_cache(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<Vec<LocalManifestEntry>, String> {
    store.with_store(|store| store.load_local_manifest(server_id))
}

pub(in crate::controller) fn ensure_jellyfin_device_id(
    store: &StoreHandle,
) -> Result<String, String> {
    ensure_jellyfin_device_id_with_generator(store, generate_jellyfin_device_id)
}

pub(in crate::controller) fn ensure_jellyfin_device_id_with_generator(
    store: &StoreHandle,
    generate: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    let mut settings = load_settings_from_store(store);
    if let Some(device_id) = normalized_jellyfin_device_id(&settings.jellyfin_device_id) {
        return Ok(device_id);
    }

    let device_id = normalized_jellyfin_device_id(&generate()?)
        .ok_or_else(|| "generated Jellyfin device id was empty".to_string())?;
    settings.jellyfin_device_id = device_id.clone();
    settings.migrate_defaults();
    store.save_settings(&settings)?;
    Ok(device_id)
}

fn normalized_jellyfin_device_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn generate_jellyfin_device_id() -> Result<String, String> {
    let mut bytes = [0_u8; JELLYFIN_DEVICE_ID_RANDOM_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("failed to generate Jellyfin device id: {error}"))?;
    let mut value = String::with_capacity("rufin-".len() + bytes.len() * 2);
    value.push_str("rufin-");
    for byte in bytes {
        write!(&mut value, "{byte:02x}")
            .map_err(|error| format!("failed to format Jellyfin device id: {error}"))?;
    }
    Ok(value)
}

pub(in crate::controller) fn load_folder_detail(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    path: &[FolderPathItem],
) -> Result<FolderDetail, String> {
    let saved = store
        .with_store(|store| store.active_server())?
        .ok_or_else(|| "No active server.".to_string())?;
    let selected_music_folder_id =
        store.with_store(|store| store.selected_music_folder_id(&saved.server.id))?;
    let settings = load_settings_for_saved(store, &saved);
    let provider = provider_for_saved(store, runtime, secrets, &saved)?;
    let music_provider = provider.as_music_provider();
    if !music_provider.capabilities().folder_browsing {
        return Err("folder browsing is not supported by the active provider.".to_string());
    }
    let folder_id = path.last().map(|entry| &entry.id);
    let mut detail = runtime
        .block_on(music_provider.folder(folder_id, selected_music_folder_id.as_ref()))
        .map_err(|error| error.to_string())?;
    external_metadata::normalize_tracks(&mut detail.tracks, &settings);
    Ok(detail)
}

pub(in crate::controller) fn sync_playlist_mutation(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    before: &rufin_provider::PlaylistDetail,
    after: &rufin_provider::PlaylistDetail,
) -> Result<Option<rufin_provider::PlaylistDetail>, String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    let before_ids = before
        .entries
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<HashSet<_>>();
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
                provider
                    .as_music_provider()
                    .remove_playlist_entries(&before.playlist.id, &removed),
            )
            .map_err(|error| error.to_string())?;
    }

    let added = after
        .entries
        .iter()
        .filter(|entry| !before_ids.contains(entry.entry_id.as_str()))
        .map(|entry| entry.track.id.clone())
        .collect::<Vec<_>>();
    if !added.is_empty() {
        runtime
            .block_on(
                provider
                    .as_music_provider()
                    .add_playlist_tracks(&before.playlist.id, &added),
            )
            .map_err(|error| error.to_string())?;
    }

    for (new_index, entry) in after.entries.iter().enumerate() {
        let Some(old_index) = before
            .entries
            .iter()
            .position(|candidate| candidate.entry_id == entry.entry_id)
        else {
            continue;
        };
        if old_index != new_index && before_ids.contains(entry.entry_id.as_str()) {
            runtime
                .block_on(provider.as_music_provider().move_playlist_entry(
                    &before.playlist.id,
                    &entry.entry_id,
                    new_index,
                ))
                .map_err(|error| error.to_string())?;
        }
    }

    runtime
        .block_on(
            provider
                .as_music_provider()
                .playlist_detail(&before.playlist.id),
        )
        .map(Some)
        .map_err(|error| error.to_string())
}

pub(in crate::controller) fn report_playback_async(
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    _events: Sender<ControllerEvent>,
    server_id: ServerId,
    report: PlaybackReport,
) {
    thread::spawn(move || {
        let Some(saved) = store
            .with_store(|store| store.active_server())
            .unwrap_or(None)
            .filter(|saved| saved.server.id == server_id)
        else {
            return;
        };
        if saved.server.provider == "fake" || saved.server.provider == "local" {
            return;
        }
        let result = provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
            runtime
                .block_on(provider.as_music_provider().report_playback(report))
                .map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            warn!(%error, "failed to report playback to provider");
        }
    });
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

pub(in crate::controller) fn emit_snapshot_result(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        let _sent = events.send(ControllerEvent::Error(error));
        return;
    }
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}

pub(in crate::controller) fn emit_playlist_changed_result(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    playlist_id: PlaylistId,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        let _sent = events.send(ControllerEvent::Error(error));
        return;
    }
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::PlaylistChanged {
                playlist_id,
                snapshot: Box::new(snapshot),
            });
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}

pub(in crate::controller) fn emit_smart_playlist_changed_result(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    smart_playlist_id: SmartPlaylistId,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        let _sent = events.send(ControllerEvent::Error(error));
        return;
    }
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::SmartPlaylistChanged {
                smart_playlist_id,
                snapshot: Box::new(snapshot),
            });
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}

pub(in crate::controller) fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.config_dir().to_path_buf())
}

pub(in crate::controller) fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.cache_dir().to_path_buf())
}

pub(in crate::controller) fn app_cache_database_path() -> PathBuf {
    cache_dir()
        .map(|dir| app_cache_database_path_for_cache_dir(&dir))
        .unwrap_or_else(|| PathBuf::from(CACHE_DATABASE_FILE_NAME))
}

pub(in crate::controller) fn app_cache_database_path_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir
        .join(STORE_DIR_NAME)
        .join(CACHE_DATABASE_FILE_NAME)
}

pub(in crate::controller) fn app_settings_path() -> PathBuf {
    config_dir()
        .map(|dir| app_settings_path_for_config_dir(&dir))
        .unwrap_or_else(|| PathBuf::from(SETTINGS_FILE_NAME))
}

pub(in crate::controller) fn app_settings_path_for_config_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE_NAME)
}

pub(in crate::controller) fn cover_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|dir| cover_cache_dir_for_cache_dir(&dir))
}

pub(in crate::controller) fn cover_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(COVER_CACHE_DIR_NAME)
}

pub(in crate::controller) fn lyrics_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(LYRICS_CACHE_DIR_NAME)
}

pub(in crate::controller) fn playback_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(PLAYBACK_CACHE_DIR_NAME)
}

pub(in crate::controller) fn waveform_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    playback_cache_dir_for_cache_dir(cache_dir).join(WAVEFORM_CACHE_DIR_NAME)
}

pub(in crate::controller) fn tmp_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(TMP_CACHE_DIR_NAME)
}

pub(in crate::controller) fn ensure_app_cache_dirs(cache_dir: &Path) -> Result<(), String> {
    for dir in [
        app_cache_database_path_for_cache_dir(cache_dir)
            .parent()
            .map(Path::to_path_buf),
        Some(cover_cache_dir_for_cache_dir(cache_dir)),
        Some(lyrics_cache_dir_for_cache_dir(cache_dir)),
        Some(playback_cache_dir_for_cache_dir(cache_dir)),
        Some(tmp_cache_dir_for_cache_dir(cache_dir)),
    ]
    .into_iter()
    .flatten()
    {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(in crate::controller) fn cover_cache_path_for_key(key: &str) -> Option<PathBuf> {
    cover_cache_dir().map(|dir| dir.join(key))
}

pub(in crate::controller) fn waveform_cache_path_for_key(key: &str) -> Option<PathBuf> {
    cache_dir().map(|dir| waveform_cache_dir_for_cache_dir(&dir).join(key))
}

pub(in crate::controller) fn restrict_settings_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(in crate::controller) fn clear_disk_cover_cache(server_id: &ServerId) -> Result<(), String> {
    let Some(path) = cover_cache_dir().map(|dir| dir.join(encode_key_part(server_id.as_str())))
    else {
        return Ok(());
    };
    remove_dir_if_exists(&path)
}

pub(in crate::controller) fn prune_disk_cover_cache_entries(entries: &[CoverCacheEntry]) {
    if entries.is_empty() {
        return;
    }
    let Some(root) = cover_cache_dir() else {
        return;
    };
    prune_disk_cover_cache_entries_in_dir(entries, &root);
}

fn prune_disk_cover_cache_entries_in_dir(entries: &[CoverCacheEntry], root: &Path) {
    let Ok(root) = root.canonicalize() else {
        return;
    };
    let mut removed = 0_usize;
    let mut skipped = 0_usize;
    let mut failed = 0_usize;
    for entry in entries {
        match stale_cover_cache_file_path(entry, &root) {
            Some(path) => match remove_safe_cover_cache_file(&path, &root) {
                Ok(true) => removed += 1,
                Ok(false) => skipped += 1,
                Err(_) => failed += 1,
            },
            None => skipped += 1,
        }
    }
    if failed > 0 {
        warn!(
            removed,
            skipped, failed, "failed to remove some stale cover cache files"
        );
    } else {
        debug!(removed, skipped, "pruned stale cover cache files");
    }
}

fn stale_cover_cache_file_path(entry: &CoverCacheEntry, root: &Path) -> Option<PathBuf> {
    let key = rufin_store::image_cache_key(
        &entry.server_id,
        &entry.item_id,
        &entry.image_tag,
        entry.size,
    );
    let expected_path = root.join(key);
    let stored_path = PathBuf::from(&entry.path);
    (stored_path == expected_path).then_some(stored_path)
}

fn remove_safe_cover_cache_file(path: &Path, root: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let canonical_path = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical_path.starts_with(root) {
        return Ok(false);
    }
    match fs::remove_file(&canonical_path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

pub(in crate::controller) fn clear_disk_waveform_cache(server_id: &ServerId) -> Result<(), String> {
    let Some(path) = cache_dir().map(|dir| {
        waveform_cache_dir_for_cache_dir(&dir).join(encode_key_part(server_id.as_str()))
    }) else {
        return Ok(());
    };
    remove_dir_if_exists(&path)
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

pub(in crate::controller) fn sync_is_running(
    sync_in_flight: &InFlightGuards<ServerId>,
    server_id: &ServerId,
) -> bool {
    sync_in_flight.contains_or_blocked(server_id)
}

pub(in crate::controller) fn cancel_sync_if_running(
    sync_in_flight: &InFlightGuards<ServerId>,
    server_id: &ServerId,
) -> Result<bool, String> {
    sync_in_flight.remove(server_id)
}

pub(in crate::controller) fn acquire_cover_slot(slots: &Arc<(Mutex<usize>, Condvar)>) -> bool {
    let (lock, ready) = &**slots;
    let Ok(mut active) = lock.lock() else {
        return false;
    };
    while *active >= 2 {
        let Ok(waiting) = ready.wait(active) else {
            return false;
        };
        active = waiting;
    }
    *active += 1;
    true
}

pub(in crate::controller) fn release_cover_slot(slots: &Arc<(Mutex<usize>, Condvar)>) {
    let (lock, ready) = &**slots;
    if let Ok(mut active) = lock.lock() {
        *active = active.saturating_sub(1);
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_disk_cover_cache_entries_removes_only_safe_expected_files() {
        let root = unique_test_dir("cover-prune-root");
        let outside = unique_test_dir("cover-prune-outside").join("stale-cover");
        fs::create_dir_all(&root).expect("cover root");
        fs::create_dir_all(outside.parent().expect("outside parent")).expect("outside root");

        let server_id = ServerId::new("local-source".to_string());
        let expected_key = rufin_store::image_cache_key(&server_id, "album-one", "old-tag", 256);
        let expected_path = root.join(&expected_key);
        fs::create_dir_all(expected_path.parent().expect("expected parent"))
            .expect("expected parent dir");
        fs::write(&expected_path, b"stale").expect("write expected stale file");

        let mismatched_path = root.join("local-source").join("unexpected");
        fs::create_dir_all(mismatched_path.parent().expect("mismatched parent"))
            .expect("mismatched parent dir");
        fs::write(&mismatched_path, b"keep").expect("write mismatched file");
        fs::write(&outside, b"keep").expect("write outside file");

        let entries = vec![
            CoverCacheEntry {
                server_id: server_id.clone(),
                item_id: "album-one".to_string(),
                image_tag: "old-tag".to_string(),
                size: 256,
                path: expected_path.to_string_lossy().into_owned(),
            },
            CoverCacheEntry {
                server_id: server_id.clone(),
                item_id: "album-two".to_string(),
                image_tag: "old-tag".to_string(),
                size: 256,
                path: mismatched_path.to_string_lossy().into_owned(),
            },
            CoverCacheEntry {
                server_id,
                item_id: "album-three".to_string(),
                image_tag: "old-tag".to_string(),
                size: 256,
                path: outside.to_string_lossy().into_owned(),
            },
        ];

        prune_disk_cover_cache_entries_in_dir(&entries, &root);

        assert!(!expected_path.exists());
        assert!(mismatched_path.exists());
        assert!(outside.exists());
        let _cleanup_root = fs::remove_dir_all(root);
        let _cleanup_outside = fs::remove_dir_all(outside.parent().expect("outside parent"));
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("rufin-{label}-{}-{nanos}", std::process::id()))
    }
}
