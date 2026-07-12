use super::*;

const TEST_WAIT: Duration = Duration::from_secs(1);

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
        album_artwork: None,
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
    let (events, receiver) = channel();
    let runtime = Runtime::new()
        .map(Arc::new)
        .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
    let secret_switch = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
    let active_saved = store
        .with_store(|store| store.active_source())
        .expect("load active source");
    let active_source = active_saved
        .as_ref()
        .and_then(|saved| active_source_for_saved_test(&store, saved).ok());
    let playback_source_id = active_source
        .as_ref()
        .map(|active| active.identity.id.clone());
    let artwork = crate::controller::artwork::open_for_test(Arc::clone(&runtime), events.clone());
    let controller = AppController {
        settings: super::settings_controller::SettingsController::new(
            store.clone(),
            Arc::<dyn SecretStore>::clone(&secrets),
        ),
        store,
        runtime,
        active_source: Arc::new(std::sync::RwLock::new(active_source)),
        secrets,
        secret_switch,
        playback_product: Arc::new(std::sync::RwLock::new(None)),
        source_transitions: Arc::new(SourceTransitions::new()),
        lyrics_request_generation: Arc::new(AtomicU64::new(0)),
        waveform_request_key: Arc::new(Mutex::new(None)),
        waveform_warm_generation: Arc::new(AtomicU64::new(0)),
        sync_coordinator: Arc::new(Mutex::new(library_sync::SyncCoordinator::new())),
        artwork,
        events,
        home_refresh_in_flight: InFlightGuards::new("Home refresh"),
        explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
    };
    if let Some(source_id) = playback_source_id {
        controller
            .activate_playback_source(&source_id)
            .expect("activate playback source");
    }
    (controller, receiver)
}

fn active_source_for_saved_test(
    store: &StoreHandle,
    saved: &StoredSource,
) -> Result<Arc<ActiveSource>, String> {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    secrets
        .save_token(saved.source_id.as_str(), "test-salt:test-token")
        .map_err(|error| error.to_string())?;
    crate::source_setup::activate_configured_source(store, &secrets, saved)
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
        album_artwork: None,
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

pub(in crate::controller) fn saved_source() -> StoredSource {
    sources::jellyfin::JellyfinSourceConfig {
        credentials: sources::CredentialSourceConfig {
            source: SourceIdentity {
                id: SourceId::new("jellyfin:server:test"),
                kind: sources::jellyfin::JELLYFIN_SOURCE_ID.to_string(),
                name: "Test Server".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        },
        use_instant_mix: false,
    }
    .into_stored()
}

pub(in crate::controller) fn begin_active_sync(store: &StoreHandle, saved: &StoredSource) -> i64 {
    store
        .with_store(|store| {
            store.save_source(saved)?;
            store.set_active_source(&saved.source_id)?;
            store.begin_sync(&saved.source_id)
        })
        .expect("begin sync")
}

pub(in crate::controller) fn begin_sync_with_access(
    store: &StoreHandle,
    saved: &StoredSource,
    access: &SourceLocalAccess,
) -> i64 {
    store
        .with_store(|store| {
            store.save_source(saved)?;
            store.set_active_source(&saved.source_id)?;
            store.save_source_local_access(access)?;
            store.begin_sync(&saved.source_id)
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
        settings: Arc::new(Mutex::new(StoredSettings::default())),
        write_gate: library::StoreWriteGate::default(),
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

#[derive(Default)]
pub(in crate::controller) struct CachedLibraryObservation {
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub album_artists: Vec<Artist>,
    pub genres: Vec<Genre>,
    pub music_folders: Vec<(MusicFolder, Vec<Track>)>,
    pub playlists: Vec<PlaylistDetail>,
    pub home_sections: Vec<HomeSection>,
    pub local_matches: Vec<(TrackId, String, String)>,
}

pub(in crate::controller) fn commit_cached_library(
    store: &Store,
    source_id: &SourceId,
    generation: i64,
    mut observation: CachedLibraryObservation,
) -> StoreResult<SyncCommit> {
    for track in &observation.tracks {
        if observation
            .albums
            .iter()
            .any(|album| album.id == track.album_id)
        {
            continue;
        }
        observation.albums.push(Album {
            id: track.album_id.clone(),
            title: track.album.clone(),
            artist: track.artist.clone(),
            artist_id: track.artist_id.clone(),
            album_artist_credits: track.album_artist_credits.clone(),
            artist_credits: track.artist_credits.clone(),
            year: track.year,
            release_date: track.release_date.clone(),
            date_added: track.date_added.clone(),
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 0,
            duration_seconds: 0,
            favorite: false,
            color_seed: 0,
            image_ref: track.image_ref.clone(),
            genres: track.genres.clone(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        });
    }

    let mut artists = observation
        .artists
        .drain(..)
        .map(|artist| (artist.id.clone(), artist))
        .collect::<HashMap<_, _>>();
    let mut album_artists = observation
        .album_artists
        .drain(..)
        .map(|artist| (artist.id.clone(), artist))
        .collect::<HashMap<_, _>>();
    for album in &observation.albums {
        if let Some(artist_id) = &album.artist_id {
            album_artists
                .entry(artist_id.clone())
                .or_insert_with(|| test_artist(artist_id.clone(), album.artist.clone()));
        }
        add_test_credits(&mut artists, &album.artist_credits);
        add_test_credits(&mut album_artists, &album.album_artist_credits);
    }
    for track in &observation.tracks {
        if let Some(artist_id) = &track.artist_id {
            artists
                .entry(artist_id.clone())
                .or_insert_with(|| test_artist(artist_id.clone(), track.artist.clone()));
        }
        add_test_credits(&mut artists, &track.artist_credits);
        add_test_credits(&mut album_artists, &track.album_artist_credits);
    }
    observation.artists = artists.into_values().collect();
    observation
        .artists
        .sort_by(|left, right| left.id.cmp(&right.id));
    observation.album_artists = album_artists.into_values().collect();
    observation
        .album_artists
        .sort_by(|left, right| left.id.cmp(&right.id));

    let mut genres = observation
        .genres
        .drain(..)
        .map(|genre| (genre.name.clone(), genre))
        .collect::<HashMap<_, _>>();
    for name in observation
        .albums
        .iter()
        .flat_map(|album| &album.genres)
        .chain(observation.tracks.iter().flat_map(|track| &track.genres))
    {
        genres.entry(name.clone()).or_insert_with(|| Genre {
            id: GenreId::new(name.clone()),
            name: name.clone(),
            album_count: 0,
            track_count: 0,
            duration_seconds: 0,
            image_ref: None,
            representative_albums: Vec::new(),
        });
    }
    observation.genres = genres.into_values().collect();
    observation
        .genres
        .sort_by(|left, right| left.id.cmp(&right.id));

    let folders = observation
        .music_folders
        .iter()
        .map(|(folder, tracks)| library::MusicFolderSnapshot {
            folder: folder.clone(),
            track_ids: tracks.iter().map(|track| track.id.clone()).collect(),
        })
        .collect::<Vec<_>>();
    let mappings = [
        (
            SourceEntityKind::Album,
            observation
                .albums
                .iter()
                .map(|album| album.id.as_str())
                .collect(),
        ),
        (
            SourceEntityKind::Track,
            observation
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect(),
        ),
        (
            SourceEntityKind::Artist,
            observation
                .artists
                .iter()
                .map(|artist| artist.id.as_str())
                .collect(),
        ),
        (
            SourceEntityKind::AlbumArtist,
            observation
                .album_artists
                .iter()
                .map(|artist| artist.id.as_str())
                .collect(),
        ),
        (
            SourceEntityKind::Genre,
            observation
                .genres
                .iter()
                .map(|genre| genre.id.as_str())
                .collect(),
        ),
        (
            SourceEntityKind::Playlist,
            observation
                .playlists
                .iter()
                .map(|detail| detail.playlist.id.as_str())
                .collect(),
        ),
        (
            SourceEntityKind::MusicFolder,
            folders
                .iter()
                .map(|folder| folder.folder.id.as_str())
                .collect(),
        ),
    ]
    .into_iter()
    .flat_map(|(entity_kind, ids): (SourceEntityKind, Vec<&str>)| {
        ids.into_iter().map(move |entity_id| SourceObjectMapping {
            source_object_id: entity_id.to_string(),
            entity_kind,
            entity_id: entity_id.to_string(),
        })
    })
    .collect();
    let local_access =
        (!observation.local_matches.is_empty()).then_some(library::LocalAccessUpdate {
            manifest: library::LocalManifestDelta::default(),
            matches: observation.local_matches,
        });
    let base_cache_revision = store.source_cache_revision(source_id)?;
    store.commit_library_sync(
        source_id,
        generation,
        base_cache_revision,
        library::LibrarySync {
            albums: observation.albums,
            tracks: observation.tracks,
            artists: observation.artists,
            album_artists: observation.album_artists,
            genres: observation.genres,
            playlists: observation.playlists,
            home_sections: observation.home_sections,
            mappings,
            coverage: library::SyncCoverage::All {
                music_folders: folders,
            },
            local_access,
        },
    )
}

fn add_test_credits(artists: &mut HashMap<ArtistId, Artist>, credits: &[library::ArtistCredit]) {
    for credit in credits {
        artists
            .entry(credit.id.clone())
            .or_insert_with(|| test_artist(credit.id.clone(), credit.name.clone()));
    }
}

fn test_artist(id: ArtistId, name: String) -> Artist {
    Artist {
        id,
        name,
        album_count: 0,
        track_count: 0,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
        representative_albums: Vec::new(),
    }
}

pub(in crate::controller) fn seed_cached_library(
    store: &StoreHandle,
    saved: &StoredSource,
    albums: &[Album],
    tracks: &[Track],
    home_sections: &[HomeSection],
) {
    store
        .with_store(|store| {
            store.save_source(saved)?;
            store.set_active_source(&saved.source_id)?;
            let generation = store.begin_sync(&saved.source_id)?;
            commit_cached_library(
                store,
                &saved.source_id,
                generation,
                CachedLibraryObservation {
                    albums: albums.to_vec(),
                    tracks: tracks.to_vec(),
                    home_sections: home_sections.to_vec(),
                    ..CachedLibraryObservation::default()
                },
            )
            .map(|_| ())
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
pub(in crate::controller) fn wait_for_playback_projection(
    events: &Receiver<ControllerEvent>,
) -> PlaybackProjection {
    wait_for_event(events, "playback projection", |event| match event {
        ControllerEvent::PlaybackProduct(projection) => Some(*projection),
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
pub(in crate::controller) fn wait_for_notice(events: &Receiver<ControllerEvent>) -> SourceNotice {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::SourceNotice(notice) => Some(notice),
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
pub(in crate::controller) fn wait_for_lyrics(
    events: &Receiver<ControllerEvent>,
) -> Option<metadata::Lyrics> {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Lyrics { lyrics, .. } => Some(*lyrics),
        _ => None,
    })
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
