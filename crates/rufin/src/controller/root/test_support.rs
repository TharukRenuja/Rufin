use super::*;

const TEST_WAIT: Duration = Duration::from_secs(1);
const TEST_POLL: Duration = Duration::from_millis(10);

pub(in crate::controller) fn library_track(
    number: u32,
    artist_id: Option<ArtistId>,
    album_id: AlbumId,
    artist: &str,
    genres: &[&str],
) -> Track {
    Track {
        id: TrackId::fake(number),
        album_id,
        title: format!("Track {number}"),
        artist: artist.to_string(),
        artist_id,
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: "Album".to_string(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: 180,
        favorite: false,
        disc_number: 1,
        track_number: number as u16,
        image_ref: None,
        genres: genres.iter().map(|genre| genre.to_string()).collect(),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
        bpm: None,
        moods: Vec::new(),
    }
}

pub(in crate::controller) fn controller_from_store_for_test(
    store: StoreHandle,
) -> (AppController, Receiver<ControllerEvent>) {
    let test_permit = Some(super::controller_test_permit());
    let (events, receiver) = channel();
    let runtime = Runtime::new()
        .map(Arc::new)
        .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
    let snapshot = load_snapshot(&store).expect("load snapshot");
    let settings = load_settings_from_store(&store);
    let queue = restore_queue(&store, snapshot.source.as_ref());
    let playback_snapshot =
        playback_snapshot_from_queue(queue.as_ref(), settings.auto_dj_enabled, &settings.playback);
    let secret_switch = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
    let controller = AppController {
        settings: super::settings_controller::SettingsController::new(
            store.clone(),
            Arc::<dyn SecretStore>::clone(&secrets),
        ),
        store,
        runtime,
        secrets,
        secret_switch,
        queue: Arc::new(Mutex::new(queue)),
        source_selection_generation: Arc::new(AtomicU64::new(0)),
        play_activation_generation: Arc::new(AtomicU64::new(0)),
        queue_persist_generation: Arc::new(AtomicU64::new(0)),
        playback_request_generation: Arc::new(AtomicU64::new(0)),
        next_preload: Arc::new(Mutex::new(NextPreloadState::default())),
        waveform_warm_generation: Arc::new(AtomicU64::new(0)),
        playback: Arc::new(Mutex::new(Box::new(playback::FakePlaybackBackend::new()))),
        playback_snapshot: Arc::new(Mutex::new(playback_snapshot)),
        playback_activity: Arc::new(Mutex::new(PlaybackActivityState::default())),
        auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
        last_progress_snapshot: Arc::new(Mutex::new(None)),
        last_report_snapshot: Arc::new(Mutex::new(None)),
        external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
        local_library_watcher: Arc::new(Mutex::new(None)),
        remote_library_watcher: Arc::new(Mutex::new(None)),
        external_cover_retry_generation: Arc::new(AtomicU64::new(0)),
        events,
        sync_in_flight: InFlightGuards::new("Sync"),
        home_refresh_in_flight: InFlightGuards::new("Home refresh"),
        explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
        cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
        external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
        cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        _test_permit: test_permit,
    };
    (controller, receiver)
}

pub(in crate::controller) struct RecordingPlaybackBackend {
    commands: Arc<Mutex<Vec<PlaybackCommand>>>,
    events: Vec<PlaybackEvent>,
}

impl RecordingPlaybackBackend {
    pub(in crate::controller) fn new(commands: Arc<Mutex<Vec<PlaybackCommand>>>) -> Self {
        Self {
            commands,
            events: Vec::new(),
        }
    }
}

impl PlaybackBackend for RecordingPlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), playback::PlaybackError> {
        self.commands
            .lock()
            .expect("commands")
            .push(command.clone());
        match command {
            PlaybackCommand::Play { .. } | PlaybackCommand::PlayPrepared { .. } => {
                self.events
                    .push(PlaybackEvent::StateChanged(PlaybackState::Playing));
            }
            PlaybackCommand::PrepareNext(_) => {}
            PlaybackCommand::SetVolume(volume) => {
                self.events.push(PlaybackEvent::VolumeChanged {
                    volume,
                    muted: false,
                });
            }
            PlaybackCommand::SetMuted(muted) => {
                self.events
                    .push(PlaybackEvent::VolumeChanged { volume: 1.0, muted });
            }
            _ => {}
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        std::mem::take(&mut self.events)
    }
}

pub(in crate::controller) struct QueuedPlaybackEvents {
    events: Vec<PlaybackEvent>,
}

impl QueuedPlaybackEvents {
    pub(in crate::controller) fn new(events: Vec<PlaybackEvent>) -> Self {
        Self { events }
    }
}

impl PlaybackBackend for QueuedPlaybackEvents {
    fn send(&mut self, _command: PlaybackCommand) -> Result<(), playback::PlaybackError> {
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        std::mem::take(&mut self.events)
    }
}

pub(in crate::controller) struct RejectingPlaybackBackend {
    commands: Arc<Mutex<Vec<PlaybackCommand>>>,
}

impl RejectingPlaybackBackend {
    pub(in crate::controller) fn new(commands: Arc<Mutex<Vec<PlaybackCommand>>>) -> Self {
        Self { commands }
    }
}

impl PlaybackBackend for RejectingPlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), playback::PlaybackError> {
        self.commands
            .lock()
            .expect("commands")
            .push(command.clone());
        match command {
            PlaybackCommand::Play { .. } | PlaybackCommand::PlayPrepared { .. } => Err(
                playback::PlaybackError::Backend("start rejected".to_string()),
            ),
            _ => Ok(()),
        }
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        Vec::new()
    }
}

pub(in crate::controller) struct BlockingPlaybackBackend {
    commands: Arc<Mutex<Vec<PlaybackCommand>>>,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    events: Vec<PlaybackEvent>,
}

impl BlockingPlaybackBackend {
    pub(in crate::controller) fn new(
        commands: Arc<Mutex<Vec<PlaybackCommand>>>,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> Self {
        Self {
            commands,
            entered,
            release,
            events: Vec::new(),
        }
    }
}

impl PlaybackBackend for BlockingPlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), playback::PlaybackError> {
        if matches!(
            command,
            PlaybackCommand::Play { .. } | PlaybackCommand::PlayPrepared { .. }
        ) {
            let _sent = self.entered.send(());
            self.release
                .recv_timeout(Duration::from_secs(1))
                .map_err(|_| playback::PlaybackError::Backend("start gate timed out".into()))?;
            self.events
                .push(PlaybackEvent::StateChanged(PlaybackState::Playing));
        }
        self.commands.lock().expect("commands").push(command);
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        std::mem::take(&mut self.events)
    }
}

pub(in crate::controller) fn restored_track() -> Track {
    Track {
        id: TrackId::new("jellyfin:track:lyrics"),
        album_id: AlbumId::fake(1),
        title: "Restored Track".to_string(),
        artist: "Artist".to_string(),
        artist_id: Some(ArtistId::fake(1)),
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: "Album".to_string(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: 180,
        favorite: false,
        disc_number: 1,
        track_number: 1,
        image_ref: None,
        genres: Vec::new(),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
        bpm: None,
        moods: Vec::new(),
    }
}

pub(in crate::controller) fn saved_source() -> SavedSource {
    SavedSource {
        source: SourceIdentity {
            id: SourceId::new("jellyfin:server:test"),
            kind: "jellyfin".to_string(),
            name: "Test Server".to_string(),
            base_url: "https://music.example".to_string(),
        },
        user_id: "user".to_string(),
        username: "demo".to_string(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    }
}

pub(in crate::controller) fn begin_active_sync(store: &StoreHandle, saved: &SavedSource) -> i64 {
    store
        .with_store(|store| {
            store.save_source(saved)?;
            store.set_active_source(&saved.source.id)?;
            store.begin_sync(&saved.source.id)
        })
        .expect("begin sync")
}

pub(in crate::controller) fn begin_sync_with_access(
    store: &StoreHandle,
    saved: &SavedSource,
    access: &SourceLocalAccess,
) -> i64 {
    store
        .with_store(|store| {
            store.save_source(saved)?;
            store.set_active_source(&saved.source.id)?;
            store.save_source_local_access(access)?;
            store.begin_sync(&saved.source.id)
        })
        .expect("begin sync")
}

pub(in crate::controller) fn unique_test_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rufin-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

pub(in crate::controller) fn local_manifest_file_facts(
    root: &std::path::Path,
    path: &std::path::Path,
) -> domain::LocalFileFacts {
    let metadata = fs::metadata(path).expect("metadata");
    let modified = metadata.modified().expect("modified time");
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .expect("modified after epoch");
    domain::LocalFileFacts {
        path: path.to_path_buf(),
        root_path: root.to_path_buf(),
        relative_path: path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned(),
        file_size: metadata.len(),
        mtime_seconds: duration.as_secs().min(i64::MAX as u64) as i64,
        mtime_nanos: duration.subsec_nanos(),
        inode: local_manifest_inode(&metadata),
        device: local_manifest_device(&metadata),
    }
}

#[cfg(unix)]
fn local_manifest_inode(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(not(unix))]
fn local_manifest_inode(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

#[cfg(unix)]
fn local_manifest_device(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}

#[cfg(not(unix))]
fn local_manifest_device(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

pub(in crate::controller) fn test_image_ref(number: u32) -> ImageRef {
    ImageRef::new(
        format!("jellyfin:album:{number}"),
        Some(format!("tag-{number}")),
    )
}

pub(in crate::controller) fn library_album(
    number: u32,
    artist: &str,
    title: &str,
    image_ref: Option<ImageRef>,
) -> Album {
    Album {
        id: AlbumId::fake(number),
        title: title.to_string(),
        artist: artist.to_string(),
        artist_id: Some(ArtistId::fake(number)),
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        track_count: 1,
        duration_seconds: 180,
        favorite: false,
        color_seed: number,
        image_ref,
        genres: Vec::new(),
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    }
}

pub(in crate::controller) fn disk_store_for_test(label: &str) -> (StoreHandle, PathBuf) {
    let root = unique_test_dir(label);
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create store root");
    let store = StoreHandle::Path {
        cache_database_path: root.join(CACHE_DATABASE_FILE_NAME),
        settings_path: root.join("config").join(SETTINGS_FILE_NAME),
    };
    store.with_store(|_| Ok(())).expect("open disk store");
    (store, root)
}

pub(in crate::controller) fn disk_store_database_path(store: &StoreHandle) -> PathBuf {
    match store {
        StoreHandle::Path {
            cache_database_path,
            ..
        } => cache_database_path.clone(),
        StoreHandle::Memory { .. } => panic!("expected disk store"),
    }
}

pub(in crate::controller) fn seed_cached_library(
    store: &StoreHandle,
    saved: &SavedSource,
    albums: &[Album],
    tracks: &[Track],
    home_sections: &[HomeSection],
) {
    store
        .with_store(|store| {
            store.save_source(saved)?;
            store.set_active_source(&saved.source.id)?;
            let generation = store.begin_sync(&saved.source.id)?;
            if !albums.is_empty() {
                store.upsert_albums(&saved.source.id, albums, generation)?;
            }
            if !tracks.is_empty() {
                store.upsert_tracks(&saved.source.id, tracks, generation)?;
            }
            if !home_sections.is_empty() {
                store.upsert_home_sections(&saved.source.id, home_sections, generation)?;
            }
            store.complete_sync(&saved.source.id, generation)
        })
        .expect("seed library cache");
}

pub(in crate::controller) fn local_album_with_image_ref(image_ref: ImageRef) -> Album {
    Album {
        id: AlbumId::new("local:album:one"),
        title: "Example Album".to_string(),
        artist: "Example Artist".to_string(),
        artist_id: Some(ArtistId::new("local:artist:one")),
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        track_count: 1,
        duration_seconds: 180,
        favorite: false,
        color_seed: 1,
        image_ref: Some(image_ref),
        genres: Vec::new(),
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    }
}

pub(in crate::controller) fn local_track_with_image_ref(
    number: u32,
    album: &Album,
    image_ref: ImageRef,
) -> Track {
    let mut track = library_track(
        number,
        Some(ArtistId::new("local:artist:one")),
        album.id.clone(),
        "Example Artist",
        &[],
    );
    track.id = TrackId::new(format!("local:track:{number}"));
    track.album = album.title.clone();
    track.image_ref = Some(image_ref);
    track
}

pub(in crate::controller) fn remote_album_with_image_ref(image_ref: ImageRef) -> Album {
    let mut album = local_album_with_image_ref(image_ref);
    album.id = AlbumId::new("jellyfin:album:one");
    album.artist_id = Some(ArtistId::new("jellyfin:artist:one"));
    album
}

pub(in crate::controller) fn provider_cover_ref() -> ImageRef {
    ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()))
}

pub(in crate::controller) fn external_cover_ref() -> ImageRef {
    ImageRef::new(
        "external:album:Example%20Artist:Example%20Album",
        Some("external-v1-test".to_string()),
    )
}

pub(in crate::controller) fn test_cover_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rufin-cover-test-{}-{label}.jpg",
        std::process::id()
    ))
}

pub(in crate::controller) fn seed_cover_cache(
    controller: &AppController,
    image_ref: &ImageRef,
    size: u32,
    path: &std::path::Path,
) -> SourceId {
    let saved = saved_source();
    let source_id = saved.source.id.clone();
    let image_tag = image_ref
        .tag
        .as_deref()
        .unwrap_or(IMAGE_TAG_UNTAGGED)
        .to_string();
    controller
        .store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&source_id)?;
            store.save_cover_cache_entry(&CoverCacheEntry {
                source_id: source_id.clone(),
                item_id: image_ref.item_id.clone(),
                image_tag,
                size,
                path: path.to_string_lossy().to_string(),
            })
        })
        .expect("seed cover cache");
    source_id
}

pub(in crate::controller) fn seed_external_cover_miss(
    controller: &AppController,
    image_ref: &ImageRef,
    size: u32,
) -> SourceId {
    let saved = saved_source();
    let source_id = saved.source.id.clone();
    let image_tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    controller
        .store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&source_id)?;
            store.save_external_image_lookup_miss(
                &source_id,
                &image_ref.item_id,
                image_tag,
                size,
                "external cover lookup found no usable image",
            )
        })
        .expect("seed external miss");
    source_id
}

pub(in crate::controller) fn wait_for_snapshot(
    events: &Receiver<ControllerEvent>,
) -> LibrarySnapshot {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Snapshot(snapshot)
        | ControllerEvent::HomeSectionsUpdated { snapshot, .. }
        | ControllerEvent::PlaylistChanged { snapshot, .. }
        | ControllerEvent::SmartPlaylistChanged { snapshot, .. } => Some(*snapshot),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_favorite_changed(
    events: &Receiver<ControllerEvent>,
) -> (FavoriteItemId, bool, LibrarySnapshot) {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::FavoriteChanged {
            item_id,
            favorite,
            snapshot,
        } => Some((item_id, favorite, *snapshot)),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playlist_changed(
    events: &Receiver<ControllerEvent>,
) -> (PlaylistId, LibrarySnapshot) {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::PlaylistChanged {
            playlist_id,
            snapshot,
        } => Some((playlist_id, *snapshot)),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_status(events: &Receiver<ControllerEvent>) -> String {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::LoginStatus(status) => Some(status),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_source_selection(
    events: &Receiver<ControllerEvent>,
) -> LibrarySourceSelection {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::SourceSelectionChanged { selected_source } => Some(selected_source),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_queue(
    events: &Receiver<ControllerEvent>,
) -> Option<domain::QueueSnapshot> {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Queue(queue) => Some(*queue),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_queue_matching(
    events: &Receiver<ControllerEvent>,
    mut matches: impl FnMut(&QueueSnapshot) -> bool,
) -> QueueSnapshot {
    for _ in 0..8 {
        let queue = wait_for_queue(events).expect("queue");
        if matches(&queue) {
            return queue;
        }
    }
    panic!("matching queue event was not emitted");
}
fn wait_for_event<T>(
    events: &Receiver<ControllerEvent>,
    context: &str,
    mut select: impl FnMut(ControllerEvent) -> Option<T>,
) -> T {
    loop {
        let event = events.recv_timeout(TEST_WAIT).expect(context);
        match event {
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            ControllerEvent::FavoriteChangeFailed { error, .. } => {
                panic!("favorite change failed: {error}");
            }
            event => {
                if let Some(value) = select(event) {
                    return value;
                }
            }
        }
    }
}
pub(in crate::controller) fn random_request(
    action: RandomPlayAction,
    limit: usize,
) -> RandomPlayRequest {
    RandomPlayRequest {
        action,
        limit,
        min_year: None,
        max_year: None,
        genre_id: None,
        genre_name: None,
        played_filter: PlayedFilter::All,
    }
}
pub(in crate::controller) fn random_track_ids(tracks: &[Track], limit: usize) -> Vec<TrackId> {
    let mut ids = tracks
        .iter()
        .map(|track| track.id.clone())
        .collect::<Vec<_>>();
    ids.sort_by_key(|id| id.as_str().to_string());
    ids.truncate(limit);
    ids
}
pub(in crate::controller) fn wait_for_cover_ready(
    events: &Receiver<ControllerEvent>,
    expected_key: &str,
) -> PathBuf {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::CoverReady { key, path } if key == expected_key => Some(path),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_lyrics(
    events: &Receiver<ControllerEvent>,
) -> Option<source::Lyrics> {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Lyrics { lyrics, .. } => Some(*lyrics),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_recorded_command(
    commands: &Arc<Mutex<Vec<PlaybackCommand>>>,
    predicate: impl Fn(&PlaybackCommand) -> bool,
) -> PlaybackCommand {
    for _ in 0..100 {
        if let Some(command) = commands
            .lock()
            .expect("commands")
            .iter()
            .find(|command| predicate(command))
            .cloned()
        {
            return command;
        }
        std::thread::sleep(TEST_POLL);
    }
    panic!("timed out waiting for playback command");
}
pub(in crate::controller) fn wait_for_playback_state(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    state: PlaybackState,
) -> super::PlaybackSnapshot {
    wait_for_polled_event(controller, events, "playback state", |event| match event {
        ControllerEvent::Playback(playback) if playback.state == state => Some(*playback),
        ControllerEvent::Error(error) => panic!("controller error: {error}"),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playback_matching(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    mut matches: impl FnMut(&PlaybackSnapshot) -> bool,
) -> PlaybackSnapshot {
    wait_for_polled_event(controller, events, "playback", |event| match event {
        ControllerEvent::Playback(playback) if matches(&playback) => Some(*playback),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_repeat_without_queue(
    events: &Receiver<ControllerEvent>,
    repeat_mode: RepeatMode,
) -> PlaybackSnapshot {
    loop {
        match events
            .recv_timeout(Duration::from_secs(5))
            .expect("repeat event")
        {
            ControllerEvent::Playback(playback) if playback.repeat_mode == repeat_mode => {
                return *playback;
            }
            ControllerEvent::Playback(_) => {}
            ControllerEvent::Queue(_) => panic!("repeat mode emitted a queue event"),
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => {}
        }
    }
}
pub(in crate::controller) fn wait_for_shuffle_without_queue(
    events: &Receiver<ControllerEvent>,
    enabled: bool,
) -> PlaybackSnapshot {
    loop {
        match events
            .recv_timeout(Duration::from_secs(5))
            .expect("shuffle event")
        {
            ControllerEvent::Playback(playback) if playback.shuffle_enabled == enabled => {
                return *playback;
            }
            ControllerEvent::Playback(_) => {}
            ControllerEvent::Queue(_) => panic!("shuffle mode emitted a queue event"),
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => {}
        }
    }
}
pub(in crate::controller) fn wait_for_playback_track_position(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    track_id: &TrackId,
    position_millis: u64,
) -> super::PlaybackSnapshot {
    wait_for_polled_event(
        controller,
        events,
        "playback track position",
        |event| match event {
            ControllerEvent::Playback(playback)
                if playback.position_millis == position_millis
                    && playback
                        .current
                        .as_ref()
                        .is_some_and(|entry| &entry.track_id == track_id) =>
            {
                Some(*playback)
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => None,
        },
    )
}
pub(in crate::controller) fn wait_for_playback_auto_dj(
    events: &Receiver<ControllerEvent>,
    enabled: bool,
) -> super::PlaybackSnapshot {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Playback(playback) if playback.auto_dj_enabled == enabled => {
            Some(*playback)
        }
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playback_repeat(
    events: &Receiver<ControllerEvent>,
    repeat_mode: RepeatMode,
) -> super::PlaybackSnapshot {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Playback(playback) if playback.repeat_mode == repeat_mode => {
            Some(*playback)
        }
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playback_current_favorite(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    favorite: bool,
) -> super::PlaybackSnapshot {
    wait_for_polled_event(
        controller,
        events,
        "playback favorite",
        |event| match event {
            ControllerEvent::Playback(playback)
                if playback
                    .current
                    .as_ref()
                    .is_some_and(|entry| entry.favorite == favorite) =>
            {
                Some(*playback)
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => None,
        },
    )
}
pub(in crate::controller) fn wait_for_token_deleted(
    secrets: &Arc<dyn SecretStore>,
    source_id: &SourceId,
) {
    for _ in 0..100 {
        if secrets.load_token(source_id).expect("load token").is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(secrets.load_token(source_id).expect("load token"), None);
}
pub(in crate::controller) fn wait_for_polled_event<T>(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    context: &str,
    mut select: impl FnMut(ControllerEvent) -> Option<T>,
) -> T {
    let deadline = std::time::Instant::now() + TEST_WAIT;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {context}"
        );
        controller.poll_playback_events();
        match events.recv_timeout(TEST_POLL) {
            Ok(event) => {
                if let Some(value) = select(event) {
                    return value;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("controller event channel closed")
            }
        }
    }
}
pub(in crate::controller) fn assert_playlist_order(
    controller: &AppController,
    playlist_id: &PlaylistId,
    ids: &[&str],
) {
    let detail = controller
        .cached_playlist_detail(playlist_id)
        .expect("playlist detail")
        .expect("playlist detail");
    assert_eq!(
        detail
            .entries
            .iter()
            .map(|entry| entry.track.id.as_str())
            .collect::<Vec<_>>(),
        ids
    );
}
