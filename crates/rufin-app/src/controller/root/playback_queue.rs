fn lyrics_from_text(track_id: TrackId, result: &LyricsSearchResult) -> Lyrics {
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

fn lyric_line_from_text(line: &str) -> Option<rufin_provider::LyricLine> {
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

fn parse_lrc_timestamp(line: &str) -> Option<(u64, &str)> {
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

fn fraction_to_millis(fraction: &str) -> Option<u64> {
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

fn lyrics_search_for_settings(settings: &AppSettings) -> JellyfinLyricsSearch {
    if settings.private_mode || !settings.external_lyrics_enabled {
        JellyfinLyricsSearch::ServerOnly
    } else if settings.prefer_server_lyrics {
        JellyfinLyricsSearch::ServerThenRemote
    } else {
        JellyfinLyricsSearch::RemoteThenServer
    }
}

fn cached_lyrics_allowed(lyrics: &Lyrics, search: JellyfinLyricsSearch) -> bool {
    match lyrics.source {
        rufin_provider::LyricsSource::Local => true,
        rufin_provider::LyricsSource::Server => true,
        rufin_provider::LyricsSource::Remote => !matches!(search, JellyfinLyricsSearch::ServerOnly),
    }
}

fn provider_for_saved(
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
        return LocalProvider::from_roots_with_identity(
            local_folder_paths(&settings),
            saved.server.clone(),
        )
        .map(LoadedProvider::Local)
        .map_err(|error| error.to_string());
    }
    if saved.server.provider == LOCAL_PROVIDER_ID {
        let session = SavedProviderSession {
            server: saved.server.clone(),
            user_id: saved.user_id.clone(),
            username: saved.username.clone(),
            trust_invalid_cert: saved.trust_invalid_cert,
            access_token: String::new(),
        };
        return provider_from_saved(session).map_err(|error| error.to_string());
    }
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
    };
    provider_from_saved(session).map_err(|error| error.to_string())
}

fn load_folder_detail(
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

fn sync_playlist_mutation(
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

fn report_playback_async(
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

fn playlist_entries_for_tracks(playlist_id: &PlaylistId, tracks: &[Track]) -> Vec<PlaylistEntry> {
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

fn unique_millis() -> Option<u128> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn emit_snapshot_result(
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

fn emit_playlist_changed_result(
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

fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.config_dir().to_path_buf())
}

fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.cache_dir().to_path_buf())
}

fn app_cache_database_path() -> PathBuf {
    cache_dir()
        .map(|dir| app_cache_database_path_for_cache_dir(&dir))
        .unwrap_or_else(|| PathBuf::from(CACHE_DATABASE_FILE_NAME))
}

fn app_cache_database_path_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir
        .join(STORE_DIR_NAME)
        .join(CACHE_DATABASE_FILE_NAME)
}

fn app_settings_path() -> PathBuf {
    config_dir()
        .map(|dir| app_settings_path_for_config_dir(&dir))
        .unwrap_or_else(|| PathBuf::from(SETTINGS_FILE_NAME))
}

fn app_settings_path_for_config_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE_NAME)
}

fn cover_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|dir| cover_cache_dir_for_cache_dir(&dir))
}

fn cover_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(COVER_CACHE_DIR_NAME)
}

fn lyrics_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(LYRICS_CACHE_DIR_NAME)
}

fn playback_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(PLAYBACK_CACHE_DIR_NAME)
}

fn tmp_cache_dir_for_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(TMP_CACHE_DIR_NAME)
}

fn ensure_app_cache_dirs(cache_dir: &Path) -> Result<(), String> {
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

fn cover_cache_path_for_key(key: &str) -> Option<PathBuf> {
    cover_cache_dir().map(|dir| dir.join(key))
}

fn restrict_settings_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn clear_disk_cover_cache(server_id: &ServerId) -> Result<(), String> {
    let Some(path) = cover_cache_dir().map(|dir| dir.join(encode_key_part(server_id.as_str())))
    else {
        return Ok(());
    };
    remove_dir_if_exists(&path)
}

fn remove_dir_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
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

fn sync_is_running(sync_in_flight: &Arc<Mutex<HashSet<ServerId>>>, server_id: &ServerId) -> bool {
    sync_in_flight
        .lock()
        .map(|running| running.contains(server_id))
        .unwrap_or(true)
}

fn cancel_sync_if_running(
    sync_in_flight: &Arc<Mutex<HashSet<ServerId>>>,
    server_id: &ServerId,
) -> Result<bool, String> {
    sync_in_flight
        .lock()
        .map(|mut running| running.remove(server_id))
        .map_err(|_| "sync guard lock was poisoned".to_string())
}

fn acquire_cover_slot(slots: &Arc<(Mutex<usize>, Condvar)>) -> bool {
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

fn release_cover_slot(slots: &Arc<(Mutex<usize>, Condvar)>) {
    let (lock, ready) = &**slots;
    if let Ok(mut active) = lock.lock() {
        *active = active.saturating_sub(1);
        ready.notify_one();
    }
}
