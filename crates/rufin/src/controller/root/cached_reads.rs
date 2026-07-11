use super::*;
use std::time::Instant;

pub(in crate::controller) fn promote_prefetched_home_section(
    store: &StoreHandle,
    source_id: &SourceId,
    section: &HomeSection,
) -> Result<(), String> {
    let (generation, base_cache_revision) = store.with_store(|store| {
        let state = store.sync_state(source_id)?;
        Ok((state.generation, state.cache_revision))
    })?;
    let commit = store.with_store(|store| {
        store.promote_home_section(source_id, generation, base_cache_revision, section)
    })?;
    prune_successful_sync_image_cache(store, source_id, commit.pruned_cover_entries);
    Ok(())
}
pub(in crate::controller) fn cache_home_section(
    store: &StoreHandle,
    source_id: &SourceId,
    section: &HomeSection,
    generation: i64,
    base_cache_revision: i64,
) -> Result<SyncCommit, String> {
    store.with_store(|store| {
        store.replace_home_section(source_id, generation, base_cache_revision, section)
    })
}
pub(in crate::controller) fn sync_page_finished(
    item_count: usize,
    total: usize,
    offset: usize,
) -> bool {
    item_count == 0 || (total > 0 && offset >= total) || (total == 0 && item_count < PAGE_SIZE)
}
pub(in crate::controller) fn normalize_artist_detail_image_refs(
    detail: &mut CachedArtistDetail,
    settings: &AppSettings,
) {
    cover_art_policy::bind_artist(&mut detail.artist, settings);
    cover_art_policy::bind_albums(&mut detail.albums, settings);
    cover_art_policy::bind_albums(&mut detail.appears_on, settings);
    cover_art_policy::bind_tracks(&mut detail.tracks, settings);
    cover_art_policy::bind_artist(&mut detail.artist, settings);
}
pub(in crate::controller) fn load_library_counts(
    store: &StoreHandle,
    source_id: &SourceId,
) -> Result<LibraryCounts, String> {
    store.with_store(|store| {
        store.read_snapshot(|store| {
            Ok(LibraryCounts {
                albums: store.load_albums(source_id, 0, 0)?.total,
                tracks: store.load_tracks(source_id, 0, 0)?.total,
                artists: store.load_artists(source_id, false, 0, 0)?.total,
                album_artists: store.load_artists(source_id, true, 0, 0)?.total,
                genres: store.load_genres(source_id, 0, 0)?.total,
                playlists: store.load_playlists(source_id, 0, 0)?.total,
            })
        })
    })
}
pub(in crate::controller) fn load_home_update(
    store: &StoreHandle,
    saved: &SavedSource,
) -> Result<LibraryHomeUpdate, String> {
    let settings = load_settings_from_store(store);
    store.with_store(|store| {
        store.read_snapshot(|store| {
            let mut sections = store.load_home_sections(&saved.source.id)?;
            let mut prefetched_explore =
                store.load_home_section_prefetch(&saved.source.id, HomeSectionKind::Explore)?;
            for section in &mut sections {
                home_image_refs_from_store(store, saved, &settings, section)?;
            }
            if let Some(section) = &mut prefetched_explore {
                home_image_refs_from_store(store, saved, &settings, section)?;
            }
            Ok(LibraryHomeUpdate {
                sections,
                prefetched_explore,
            })
        })
    })
}
pub(in crate::controller) fn restore_queue(
    store: &StoreHandle,
    server: Option<&SourceIdentity>,
) -> Option<QueueEngine> {
    let server = server?;
    let saved = SavedSource {
        source: server.clone(),
        user_id: String::new(),
        username: String::new(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    };
    let settings = load_settings_from_store(store);
    match store.with_store(|store| store.load_queue_snapshot(&server.id)) {
        Ok(Some(mut snapshot)) => {
            match queue_track_refs(store, &saved, &settings, &mut snapshot.entries) {
                Ok(true) => {
                    if let Err(error) =
                        store.with_store(|store| store.save_queue_snapshot(&snapshot))
                    {
                        warn!(%error, "failed to persist refreshed queue image refs");
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    warn!(%error, "failed to refresh queue image refs");
                }
            }
            Some(QueueEngine::restore(snapshot))
        }
        Ok(None) => Some(QueueEngine::new(server.id.clone())),
        Err(error) => {
            warn!(%error, "failed to restore queue snapshot");
            Some(QueueEngine::new(server.id.clone()))
        }
    }
}

pub(in crate::controller) struct QueueActivationContext<'a> {
    pub(in crate::controller) store: &'a StoreHandle,
    pub(in crate::controller) queue: &'a Arc<Mutex<Option<QueueEngine>>>,
    pub(in crate::controller) playback_request_generation: &'a Arc<AtomicU64>,
    pub(in crate::controller) next_preload: &'a Arc<Mutex<NextPreloadState>>,
    pub(in crate::controller) playback: &'a Arc<Mutex<Box<dyn PlaybackBackend>>>,
    pub(in crate::controller) playback_snapshot: &'a Arc<Mutex<PlaybackSnapshot>>,
    pub(in crate::controller) auto_dj_enabled: &'a Arc<Mutex<bool>>,
    pub(in crate::controller) events: &'a Sender<ControllerEvent>,
}

pub(in crate::controller) struct PreparedQueueActivation {
    source_id: SourceId,
    queue: QueueEngine,
    queue_snapshot: QueueSnapshot,
    playback_snapshot: PlaybackSnapshot,
}
pub(in crate::controller) fn prepare_saved_queue_activation(
    context: &QueueActivationContext<'_>,
    saved: &SavedSource,
) -> Result<Option<PreparedQueueActivation>, String> {
    let current_source_id = context
        .queue
        .lock()
        .map_err(|_| "queue lock was poisoned".to_string())?
        .as_ref()
        .map(|queue| queue.source_id().clone());
    if current_source_id.as_ref() == Some(&saved.source.id) {
        return Ok(None);
    }

    let restored = restore_queue(context.store, Some(&saved.source))
        .unwrap_or_else(|| QueueEngine::new(saved.source.id.clone()));
    let queue_snapshot = restored.snapshot();
    let auto_dj_enabled = context
        .auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default();
    let playback_snapshot = playback_snapshot_from_queue(
        Some(&restored),
        auto_dj_enabled,
        &load_settings_from_store(context.store).playback,
    );

    Ok(Some(PreparedQueueActivation {
        source_id: saved.source.id.clone(),
        queue: restored,
        queue_snapshot,
        playback_snapshot,
    }))
}
pub(in crate::controller) fn apply_prepared_queue_activation(
    context: &QueueActivationContext<'_>,
    activation: PreparedQueueActivation,
) -> Result<(), String> {
    let PreparedQueueActivation {
        source_id,
        queue: restored,
        queue_snapshot,
        playback_snapshot: player,
    } = activation;

    let mut queue = context
        .queue
        .lock()
        .map_err(|_| "queue lock was poisoned".to_string())?;
    let current_source_id = queue.as_ref().map(|queue| queue.source_id().clone());
    if current_source_id.as_ref() == Some(&source_id) {
        return Ok(());
    }
    *queue = Some(restored);
    drop(queue);

    if let Ok(mut snapshot) = context.playback_snapshot.lock() {
        *snapshot = player.clone();
    }

    invalidate_playback_requests(context.playback_request_generation);
    stop_playback_backend(context.playback, context.next_preload, context.events);
    let _sent = context
        .events
        .send(ControllerEvent::Queue(Box::new(Some(queue_snapshot))));
    let _sent = context
        .events
        .send(ControllerEvent::Playback(Box::new(player)));
    Ok(())
}

pub(in crate::controller) struct PreparedActiveSourceQueueReset {
    queue: QueueEngine,
    queue_snapshot: QueueSnapshot,
    playback_snapshot: PlaybackSnapshot,
}

pub(in crate::controller) fn prepare_active_source_queue_reset(
    context: &QueueActivationContext<'_>,
    saved: &SavedSource,
) -> PreparedActiveSourceQueueReset {
    let restored = QueueEngine::new(saved.source.id.clone());
    let queue_snapshot = restored.snapshot();
    let auto_dj_enabled = context
        .auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default();
    let player = playback_snapshot_from_queue(
        Some(&restored),
        auto_dj_enabled,
        &load_settings_from_store(context.store).playback,
    );
    PreparedActiveSourceQueueReset {
        queue: restored,
        queue_snapshot,
        playback_snapshot: player,
    }
}

pub(in crate::controller) fn apply_active_source_queue_reset(
    context: &QueueActivationContext<'_>,
    mut queue: std::sync::MutexGuard<'_, Option<QueueEngine>>,
    reset: PreparedActiveSourceQueueReset,
) {
    let PreparedActiveSourceQueueReset {
        queue: restored,
        queue_snapshot,
        playback_snapshot: player,
    } = reset;
    *queue = Some(restored);
    drop(queue);
    if let Ok(mut snapshot) = context.playback_snapshot.lock() {
        *snapshot = player.clone();
    }
    invalidate_playback_requests(context.playback_request_generation);
    stop_playback_backend(context.playback, context.next_preload, context.events);
    let _sent = context
        .events
        .send(ControllerEvent::Queue(Box::new(Some(queue_snapshot))));
    let _sent = context
        .events
        .send(ControllerEvent::Playback(Box::new(player)));
}
pub(in crate::controller) fn stop_playback_backend(
    playback: &Arc<Mutex<Box<dyn PlaybackBackend>>>,
    next_preload: &Arc<Mutex<NextPreloadState>>,
    events: &Sender<ControllerEvent>,
) {
    clear_next_preload(next_preload);
    if let Err(error) = playback
        .lock()
        .map_err(|_| "playback lock was poisoned".to_string())
        .and_then(|mut playback| {
            playback
                .send(PlaybackCommand::Stop)
                .map_err(|error| error.to_string())
        })
    {
        let _sent = events.send(ControllerEvent::Error(error));
    }
}
pub(in crate::controller) fn invalidate_playback_requests(
    playback_request_generation: &Arc<AtomicU64>,
) {
    playback_request_generation.fetch_add(1, Ordering::AcqRel);
}
pub(in crate::controller) fn next_playback_request_generation(
    playback_request_generation: &Arc<AtomicU64>,
) -> u64 {
    playback_request_generation.fetch_add(1, Ordering::AcqRel) + 1
}
pub(in crate::controller) fn playback_request_generation_matches(
    playback_request_generation: &Arc<AtomicU64>,
    request_generation: u64,
) -> bool {
    playback_request_generation.load(Ordering::Acquire) == request_generation
}
pub(in crate::controller) fn clear_queue_and_stop_playback(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_request_generation: &Arc<AtomicU64>,
    next_preload: &Arc<Mutex<NextPreloadState>>,
    playback: &Arc<Mutex<Box<dyn PlaybackBackend>>>,
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    auto_dj_enabled: &Arc<Mutex<bool>>,
    events: &Sender<ControllerEvent>,
) {
    invalidate_playback_requests(playback_request_generation);
    if let Ok(mut queue) = queue.lock() {
        *queue = None;
    }
    stop_playback_backend(playback, next_preload, events);
    let player = PlaybackSnapshot {
        auto_dj_enabled: auto_dj_enabled
            .lock()
            .map(|enabled| *enabled)
            .unwrap_or_default(),
        ..PlaybackSnapshot::default()
    };
    if let Ok(mut snapshot) = playback_snapshot.lock() {
        *snapshot = player.clone();
    }
    let _sent = events.send(ControllerEvent::Queue(Box::new(None)));
    let _sent = events.send(ControllerEvent::Playback(Box::new(player)));
}
pub(in crate::controller) fn emit_snapshot(store: &StoreHandle, events: &Sender<ControllerEvent>) {
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}
pub(in crate::controller) fn trimmed_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
pub(crate) fn load_settings_from_store(store: &StoreHandle) -> AppSettings {
    let mut settings = store.load_settings();
    settings.migrate_defaults();
    settings
}
pub(in crate::controller) fn prune_successful_sync_image_cache(
    store: &StoreHandle,
    source_id: &SourceId,
    mut pruned_entries: Vec<CoverCacheEntry>,
) {
    match stale_external_images(store, source_id) {
        Ok(mut entries) => pruned_entries.append(&mut entries),
        Err(error) => warn!(%error, "failed to prune generated external image cache entries"),
    }
    prune_disk_cover_cache_entries(&pruned_entries);
}

fn stale_external_images(
    store: &StoreHandle,
    source_id: &SourceId,
) -> Result<Vec<CoverCacheEntry>, String> {
    let settings = load_settings_from_store(store);
    let prune_all_external = !external_metadata::cached_refs_enabled(&settings);
    let live_refs = if prune_all_external {
        Vec::new()
    } else {
        generated_external_image_refs(store, source_id, &settings)?
    };
    store.with_store(|store| store.prune_external_images(source_id, &live_refs, prune_all_external))
}

fn generated_external_image_refs(
    store: &StoreHandle,
    source_id: &SourceId,
    settings: &AppSettings,
) -> Result<Vec<ImageRef>, String> {
    let mut albums = external_prune_albums(store, source_id)?;
    let mut tracks = external_prune_tracks(store, source_id)?;
    cover_art_policy::bind_albums(&mut albums, settings);
    cover_art_policy::bind_tracks(&mut tracks, settings);
    let mut seen = HashSet::<(String, Option<String>)>::new();
    let mut refs = Vec::new();
    for image_ref in albums
        .into_iter()
        .filter_map(|album| album.image_ref)
        .chain(tracks.into_iter().filter_map(|track| track.image_ref))
    {
        if !external_metadata::is_external_image_ref(&image_ref) {
            continue;
        }
        if seen.insert((image_ref.item_id.clone(), image_ref.tag.clone())) {
            refs.push(image_ref);
        }
    }
    Ok(refs)
}

fn external_prune_albums(store: &StoreHandle, source_id: &SourceId) -> Result<Vec<Album>, String> {
    let mut albums = Vec::new();
    let mut offset = 0;
    loop {
        let page = store.with_store(|store| store.load_albums(source_id, offset, PAGE_SIZE))?;
        let item_count = page.items.len();
        albums.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(albums);
        }
    }
}

fn external_prune_tracks(store: &StoreHandle, source_id: &SourceId) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0;
    loop {
        let page = store.with_store(|store| store.load_tracks(source_id, offset, PAGE_SIZE))?;
        let item_count = page.items.len();
        tracks.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(tracks);
        }
    }
}

pub(in crate::controller) fn playback_snapshot_from_queue(
    queue: Option<&QueueEngine>,
    auto_dj_enabled: bool,
    playback_settings: &PlaybackSettings,
) -> PlaybackSnapshot {
    queue
        .map(|queue| {
            let duration_seconds = queue
                .current()
                .map(|entry| entry.duration_seconds)
                .unwrap_or_default();
            let waveform_cache_key = waveform_cache_key_for_queue(Some(queue));
            let waveform_peaks = waveform_cache_key
                .as_deref()
                .and_then(|key| cached_waveform_peaks(key, duration_seconds));

            PlaybackSnapshot {
                current_source_id: Some(queue.source_id().clone()),
                current: queue.current().cloned(),
                state: PlaybackState::Stopped,
                position_seconds: queue.progress_seconds(),
                position_millis: u64::from(queue.progress_seconds()) * 1_000,
                duration_seconds,
                volume: playback_settings.volume,
                muted: playback_settings.muted,
                repeat_mode: queue.repeat_mode(),
                shuffle_enabled: queue.shuffle().enabled,
                auto_dj_enabled,
                buffering_percent: None,
                last_error: None,
                waveform_cache_key,
                waveform_peaks,
            }
        })
        .unwrap_or_else(|| PlaybackSnapshot {
            auto_dj_enabled,
            volume: playback_settings.volume,
            muted: playback_settings.muted,
            ..PlaybackSnapshot::default()
        })
}
pub(in crate::controller) fn next_queue_entry_after_current(
    queue: &QueueEngine,
) -> Option<QueueEntry> {
    queue.next_after_end_of_stream().cloned()
}

pub(in crate::controller) fn current_request_match(
    queue: Option<&QueueEngine>,
    source_id: &SourceId,
    entry: &QueueEntry,
) -> bool {
    let Some(queue) = queue else {
        return false;
    };
    if queue.source_id() != source_id {
        return false;
    }
    queue
        .current()
        .is_some_and(|current| current.id == entry.id && current.track_id == entry.track_id)
}
pub(in crate::controller) fn current_request_valid(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    source_id: &SourceId,
    entry: &QueueEntry,
) -> bool {
    queue
        .lock()
        .ok()
        .is_some_and(|queue| current_request_match(queue.as_ref(), source_id, entry))
}
pub(in crate::controller) fn request_generation_match(
    playback_request_generation: &Arc<AtomicU64>,
    request_generation: u64,
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    source_id: &SourceId,
    entry: &QueueEntry,
) -> bool {
    playback_request_generation_matches(playback_request_generation, request_generation)
        && current_request_valid(queue, source_id, entry)
}

#[derive(Clone, Debug, Default)]
pub(in crate::controller) struct NextPreloadState {
    generation: u64,
    request: Option<NextPreloadRequest>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::controller) struct NextPreloadRequest {
    pub(in crate::controller) source_id: SourceId,
    pub(in crate::controller) current_entry_id: QueueEntryId,
    pub(in crate::controller) next_entry_id: QueueEntryId,
    pub(in crate::controller) next_entry: QueueEntry,
    pub(in crate::controller) stream_quality: StreamQuality,
}

#[derive(Clone, Debug)]
struct NextPreloadTicket {
    generation: u64,
    request: NextPreloadRequest,
}

fn preload_request_match(queue: Option<&QueueEngine>, request: &NextPreloadRequest) -> bool {
    let Some(queue) = queue else {
        return false;
    };
    if queue.source_id() != &request.source_id {
        return false;
    }
    let Some(current) = queue.current() else {
        return false;
    };
    if current.id != request.current_entry_id {
        return false;
    }
    next_queue_entry_after_current(queue).is_some_and(|entry| entry.id == request.next_entry_id)
}
fn begin_next_preload(
    next_preload: &Arc<Mutex<NextPreloadState>>,
    request: NextPreloadRequest,
) -> Option<NextPreloadTicket> {
    let mut state = next_preload.lock().ok()?;
    if state.request.as_ref() == Some(&request) {
        return None;
    }
    state.generation = state.generation.wrapping_add(1);
    state.request = Some(request.clone());
    Some(NextPreloadTicket {
        generation: state.generation,
        request,
    })
}

pub(in crate::controller) fn clear_next_preload(next_preload: &Arc<Mutex<NextPreloadState>>) {
    if let Ok(mut state) = next_preload.lock() {
        state.generation = state.generation.wrapping_add(1);
        state.request = None;
    }
}

fn next_preload_ticket_valid(
    next_preload: &Arc<Mutex<NextPreloadState>>,
    ticket: &NextPreloadTicket,
) -> bool {
    next_preload.lock().ok().is_some_and(|state| {
        state.generation == ticket.generation && state.request.as_ref() == Some(&ticket.request)
    })
}

fn clear_matching_next_preload(
    next_preload: &Arc<Mutex<NextPreloadState>>,
    ticket: &NextPreloadTicket,
) {
    if let Ok(mut state) = next_preload.lock()
        && state.generation == ticket.generation
        && state.request.as_ref() == Some(&ticket.request)
    {
        state.generation = state.generation.wrapping_add(1);
        state.request = None;
    }
}
pub(in crate::controller) fn shuffle_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(1)
}
pub(in crate::controller) fn platform_secret_store(settings: &AppSettings) -> Arc<dyn SecretStore> {
    match settings.secret_storage_mode {
        SecretStorageMode::ConfigFile => Arc::new(CachedSecretStore::new(Arc::new(
            ConfigSecretStore::with_scope(config_secrets_path(), settings.secret_scope_id.clone()),
        ))),
        SecretStorageMode::SystemKeyring => system_keyring_secret_store(&settings.secret_scope_id),
    }
}

#[cfg(unix)]
fn system_keyring_secret_store(scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(CachedSecretStore::new(Arc::new(SecretServiceStore::new(
        scope_id.to_string(),
    ))))
}

#[cfg(not(unix))]
fn system_keyring_secret_store(_scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(UnavailableSecretStore::new(
        "system keyring is unavailable on this platform",
    ))
}
pub(in crate::controller) fn saved_server_needs_auth(
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedSource,
) -> bool {
    match crate::sources::configured_source_needs_auth(secrets, saved) {
        Ok(needs_auth) => needs_auth,
        Err(error) => {
            warn!(
                %error,
                source_id = %saved.source.id,
                source_kind = %saved.source.kind,
                "failed to resolve source authentication state"
            );
            true
        }
    }
}
pub(in crate::controller) fn emit_runtime_snapshot(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
) {
    match load_runtime_snapshot(store, secrets) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}
pub(in crate::controller) fn playback_track_from_entry(entry: &QueueEntry) -> PlaybackTrack {
    PlaybackTrack {
        id: entry.track_id.clone(),
        album_id: entry.album_id.clone(),
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        album: entry.album.clone(),
        duration_seconds: entry.duration_seconds,
    }
}
pub(in crate::controller) fn prepared_item_from_entry(
    entry: &QueueEntry,
    stream: StreamDescriptor,
) -> PreparedPlaybackItem {
    PreparedPlaybackItem::new(playback_track_from_entry(entry), stream)
}
pub(in crate::controller) fn resolve_prepared_item(
    store: &StoreHandle,
    runtime: &Runtime,
    active_source: &ActiveSourceSlot,
    source_id: &SourceId,
    entry: &QueueEntry,
    playback_settings: &PlaybackSettings,
) -> Result<PreparedPlaybackItem, String> {
    let stream = resolve_stream(
        store,
        runtime,
        active_source,
        source_id,
        &entry.track_id,
        playback_settings,
    )?;
    Ok(prepared_item_from_entry(entry, stream))
}
pub(in crate::controller) fn send_prepared_next(
    playback: &Arc<Mutex<Box<dyn PlaybackBackend>>>,
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    events: &Sender<ControllerEvent>,
    request: &NextPreloadRequest,
    prepared: PreparedPlaybackItem,
) -> bool {
    let Ok(queue) = queue.lock() else {
        return false;
    };
    if !preload_request_match(queue.as_ref(), request) {
        return false;
    }
    let track_id = prepared.track.id.clone();
    if let Err(error) = playback
        .lock()
        .map_err(|_| "playback lock was poisoned".to_string())
        .and_then(|mut playback| {
            playback
                .send(PlaybackCommand::PrepareNext(Some(prepared)))
                .map_err(|error| error.to_string())
        })
    {
        let _sent = events.send(ControllerEvent::Error(error));
        return false;
    }
    info!(track_id = %track_id.as_str(), "sent next playback stream");
    true
}
pub(in crate::controller) fn prepare_next_stream_from_handles(
    store: StoreHandle,
    runtime: Arc<Runtime>,
    active_source: ActiveSourceSlot,
    playback: Arc<Mutex<Box<dyn PlaybackBackend>>>,
    queue: Arc<Mutex<Option<QueueEngine>>>,
    next_preload: Arc<Mutex<NextPreloadState>>,
    events: Sender<ControllerEvent>,
) {
    let playback_settings = load_settings_from_store(&store).playback;
    let Some(request) = next_preload_request_from_queue(&queue, &playback_settings) else {
        clear_next_preload(&next_preload);
        if let Err(error) = playback
            .lock()
            .map_err(|_| "playback lock was poisoned".to_string())
            .and_then(|mut playback| {
                playback
                    .send(PlaybackCommand::PrepareNext(None))
                    .map_err(|error| error.to_string())
            })
        {
            let _sent = events.send(ControllerEvent::Error(error));
        }
        return;
    };
    let Some(ticket) = begin_next_preload(&next_preload, request) else {
        return;
    };

    thread::spawn(move || {
        let preload_started_at = Instant::now();
        let prepared = match resolve_prepared_item(
            &store,
            &runtime,
            &active_source,
            &ticket.request.source_id,
            &ticket.request.next_entry,
            &playback_settings,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                if !next_preload_ticket_valid(&next_preload, &ticket) {
                    debug!(
                        track_id = %ticket.request.next_entry.track_id.as_str(),
                        elapsed_ms = preload_started_at.elapsed().as_millis(),
                        "discarded stale next playback stream error"
                    );
                    return;
                }
                clear_matching_next_preload(&next_preload, &ticket);
                if preload_error_is_transient(&error) {
                    debug!(%error, "skipped next playback preload while store is busy");
                    return;
                }
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
        };
        let elapsed_ms = preload_started_at.elapsed().as_millis();
        if !next_preload_ticket_valid(&next_preload, &ticket) {
            debug!(
                track_id = %ticket.request.next_entry.track_id.as_str(),
                elapsed_ms,
                "discarded stale next playback stream"
            );
            return;
        }
        info!(
            track_id = %ticket.request.next_entry.track_id.as_str(),
            elapsed_ms,
            "resolved next playback stream"
        );
        if !send_prepared_next(&playback, &queue, &events, &ticket.request, prepared) {
            clear_matching_next_preload(&next_preload, &ticket);
        }
    });
}
pub(in crate::controller) fn next_preload_request_from_queue(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_settings: &PlaybackSettings,
) -> Option<NextPreloadRequest> {
    queue.lock().ok().and_then(|queue| {
        let queue = queue.as_ref()?;
        let source_id = queue.source_id().clone();
        let current_entry_id = queue.current()?.id.clone();
        let next_entry = next_queue_entry_after_current(queue)?;
        let next_entry_id = next_entry.id.clone();
        if next_entry_id == current_entry_id {
            return None;
        }
        Some(NextPreloadRequest {
            source_id,
            current_entry_id,
            next_entry_id,
            next_entry,
            stream_quality: playback_settings.stream_quality,
        })
    })
}

fn preload_error_is_transient(error: &str) -> bool {
    error.contains("database is locked") || error.contains("database table is locked")
}
