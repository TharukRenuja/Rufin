use super::*;

#[test]
pub(in crate::controller) fn explicit_favorite_updates_can_unfavorite_persistent_controls() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    let album = snapshot
        .albums
        .iter()
        .find(|album| !album.favorite)
        .expect("non-favorite album")
        .clone();
    controller.set_album_favorite(album.id.clone(), true);
    let (item_id, favorite, snapshot) = wait_for_favorite_changed(&events);
    assert_eq!(item_id, FavoriteItemId::Album(album.id.clone()));
    assert!(favorite);
    assert!(
        snapshot
            .albums
            .iter()
            .find(|candidate| candidate.id == album.id)
            .expect("cached album")
            .favorite
    );
    controller.set_album_favorite(album.id.clone(), false);
    let (item_id, favorite, snapshot) = wait_for_favorite_changed(&events);
    assert_eq!(item_id, FavoriteItemId::Album(album.id.clone()));
    assert!(!favorite);
    assert!(
        !snapshot
            .albums
            .iter()
            .find(|candidate| candidate.id == album.id)
            .expect("cached album")
            .favorite
    );
}
#[test]
pub(in crate::controller) fn fake_playlist_mutations_create_move_and_remove_entries() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    let first = snapshot.tracks[0].clone();
    let second = snapshot.tracks[1].clone();
    let third = snapshot.tracks[2].clone();
    controller.create_playlist(
        "Controller Playlist".to_string(),
        vec![first.clone(), second.clone()],
    );
    let snapshot = wait_for_snapshot(&events);
    let playlist = snapshot
        .playlists
        .iter()
        .find(|playlist| playlist.name == "Controller Playlist")
        .expect("created playlist")
        .clone();
    assert_playlist_order(
        &controller,
        &playlist.id,
        &[first.id.as_str(), second.id.as_str()],
    );
    let detail = controller
        .cached_playlist_detail(&playlist.id)
        .expect("playlist detail")
        .expect("playlist detail");
    controller.move_playlist_entry(playlist.id.clone(), detail.entries[1].entry_id.clone(), 0);
    let (changed_id, _snapshot) = wait_for_playlist_changed(&events);
    assert_eq!(changed_id, playlist.id);
    assert_playlist_order(
        &controller,
        &playlist.id,
        &[second.id.as_str(), first.id.as_str()],
    );
    controller.add_tracks_to_playlist(playlist.id.clone(), vec![third.clone()]);
    let (changed_id, _snapshot) = wait_for_playlist_changed(&events);
    assert_eq!(changed_id, playlist.id);
    assert_playlist_order(
        &controller,
        &playlist.id,
        &[second.id.as_str(), first.id.as_str(), third.id.as_str()],
    );
    let detail = controller
        .cached_playlist_detail(&playlist.id)
        .expect("playlist detail")
        .expect("playlist detail");
    controller.remove_playlist_entry(playlist.id.clone(), detail.entries[0].entry_id.clone());
    let (changed_id, _snapshot) = wait_for_playlist_changed(&events);
    assert_eq!(changed_id, playlist.id);
    assert_playlist_order(
        &controller,
        &playlist.id,
        &[first.id.as_str(), third.id.as_str()],
    );
}
#[test]
pub(in crate::controller) fn fake_lyrics_request_emits_empty_lyrics_event() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    controller.play_now(snapshot.tracks[0].clone());
    let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    controller.request_lyrics_for_current();
    assert!(wait_for_lyrics(&events).is_none());
}
#[test]
pub(in crate::controller) fn local_lyrics_request_skips_unsupported_provider_lookup() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let mut track = restored_track();
    track.id = TrackId::new("local:track:lyrics");
    track.local_path = None;
    let mut queue = QueueEngine::new(saved.server.id.clone());
    queue.play_now(&track);
    store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_queue_snapshot(&queue.snapshot())?;
            Ok(())
        })
        .expect("seed local queue");
    let (controller, events) = controller_from_store_for_test(store);

    controller.request_lyrics_for_current();

    assert!(wait_for_lyrics(&events).is_none());
}

#[test]
pub(in crate::controller) fn local_loaded_provider_lyrics_respects_capability() {
    let root = self::unique_test_dir("local-provider-lyrics-capability");
    fs::create_dir_all(&root).expect("create local root");
    let provider = LoadedProvider::Local(
        LocalProvider::from_roots_with_identity(vec![root.clone()], local_source_saved().server)
            .expect("local provider"),
    );
    let runtime = Runtime::new().expect("runtime");

    let lyrics = runtime
        .block_on(provider.lyrics_with_search(
            &TrackId::new("local:track:no-lyrics"),
            JellyfinLyricsSearch::ServerThenRemote,
        ))
        .expect("lyrics result");

    assert!(lyrics.is_none());
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn server_lyrics_request_ignores_cached_remote_lyrics() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    let track = snapshot.tracks[0].clone();
    controller.play_now(track.clone());
    let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    let server_id = controller
        .store
        .with_store(|store| store.active_server())
        .expect("load active server")
        .expect("active server")
        .server
        .id;
    let remote_lyrics = Lyrics {
        track_id: track.id,
        source: LyricsSource::Remote,
        lines: vec![LyricLine {
            text: "cached remote line".to_string(),
            start_millis: None,
        }],
    };
    controller
        .store
        .with_store(|store| store.save_lyrics(&server_id, &remote_lyrics))
        .expect("save remote lyrics");
    controller.request_server_lyrics_for_current();
    assert!(wait_for_lyrics(&events).is_none());
}
#[test]
pub(in crate::controller) fn clearing_remote_lyrics_emits_empty_event_and_removes_cache() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    let track = snapshot.tracks[0].clone();
    controller.play_now(track.clone());
    let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    let server_id = controller
        .store
        .with_store(|store| store.active_server())
        .expect("load active server")
        .expect("active server")
        .server
        .id;
    let remote_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        lines: vec![LyricLine {
            text: "cached remote line".to_string(),
            start_millis: None,
        }],
    };
    controller
        .store
        .with_store(|store| store.save_lyrics(&server_id, &remote_lyrics))
        .expect("save remote lyrics");

    controller.request_lyrics_for_current();
    assert_eq!(wait_for_lyrics(&events), Some(remote_lyrics));
    controller.clear_remote_lyrics_for_current();

    assert!(wait_for_lyrics(&events).is_none());
    assert_eq!(
        controller
            .store
            .with_store(|store| store.load_lyrics(&server_id, &track.id))
            .expect("load lyrics"),
        None
    );
}
#[test]
pub(in crate::controller) fn clearing_remote_lyrics_preserves_server_cache() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    let track = snapshot.tracks[0].clone();
    controller.play_now(track.clone());
    let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    let server_id = controller
        .store
        .with_store(|store| store.active_server())
        .expect("load active server")
        .expect("active server")
        .server
        .id;
    let server_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Server,
        lines: vec![LyricLine {
            text: "server line".to_string(),
            start_millis: None,
        }],
    };
    controller
        .store
        .with_store(|store| store.save_lyrics(&server_id, &server_lyrics))
        .expect("save server lyrics");

    controller.clear_remote_lyrics_for_current();
    controller.request_lyrics_for_current();

    assert_eq!(wait_for_lyrics(&events), Some(server_lyrics));
}
#[test]
pub(in crate::controller) fn restored_queue_request_lyrics_emits_cached_current_lyrics() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = SavedServer {
        server: ServerIdentity {
            id: ServerId::new("jellyfin:server:lyrics"),
            provider: "jellyfin".to_string(),
            name: "Lyrics Server".to_string(),
            base_url: "https://music.example".to_string(),
        },
        user_id: "user".to_string(),
        username: "demo".to_string(),
        trust_invalid_cert: false,
    };
    let track = restored_track();
    let mut queue = QueueEngine::new(saved.server.id.clone());
    queue.play_now(&track);
    queue.set_progress_seconds(12);
    let lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Server,
        lines: vec![LyricLine {
            text: "first line".to_string(),
            start_millis: Some(1_000),
        }],
    };
    store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_queue_snapshot(&queue.snapshot())?;
            store.save_lyrics(&saved.server.id, &lyrics)?;
            Ok(())
        })
        .expect("seed restored state");
    let (controller, events) = controller_from_store_for_test(store);
    controller.request_lyrics_for_current();
    assert_eq!(wait_for_lyrics(&events), Some(lyrics));
}
#[test]
pub(in crate::controller) fn lyrics_search_respects_private_mode_and_preference() {
    let mut settings = AppSettings {
        external_lyrics_enabled: true,
        ..AppSettings::default()
    };
    assert_eq!(
        super::lyrics_search_for_settings(&settings),
        JellyfinLyricsSearch::ServerThenRemote
    );
    settings.prefer_server_lyrics = false;
    assert_eq!(
        super::lyrics_search_for_settings(&settings),
        JellyfinLyricsSearch::RemoteThenServer
    );
    settings.private_mode = true;
    assert_eq!(
        super::lyrics_search_for_settings(&settings),
        JellyfinLyricsSearch::ServerOnly
    );
    settings.private_mode = false;
    settings.external_lyrics_enabled = false;
    assert_eq!(
        super::lyrics_search_for_settings(&settings),
        JellyfinLyricsSearch::ServerOnly
    );
}
#[test]
pub(in crate::controller) fn saved_lrclib_result_uses_explicit_output_path() {
    let dir = self::unique_test_dir("lyrics-portal-save");
    fs::create_dir_all(&dir).expect("create dir");
    let sidecar = dir.join("Track.lrc");
    let output = dir.join("Chosen Lyrics.lrc");
    let entry = rufin_core::QueueEntry {
        id: rufin_core::QueueEntryId::new("queue-entry:lyrics"),
        track_id: TrackId::new("jellyfin:track:lyrics-save"),
        album_id: None,
        title: "Track".to_string(),
        artist: "Artist".to_string(),
        artist_id: None,
        album: "Album".to_string(),
        year: 0,
        duration_seconds: 180,
        favorite: false,
        image_ref: None,
        local_path: Some(dir.join("Track.flac").to_string_lossy().into_owned()),
        source_format: None,
        origin: None,
    };
    let result = super::LyricsSearchResult {
        id: 1,
        track_name: "Track".to_string(),
        artist_name: "Artist".to_string(),
        album_name: "Album".to_string(),
        duration_seconds: 180,
        synced_lyrics: Some("[00:01.00]line one".to_string()),
        plain_lyrics: None,
    };
    let (saved_path, lyrics) = super::save_lrclib_result(
        &ServerId::new("jellyfin:server:lyrics"),
        &entry,
        &result,
        output.clone(),
    )
    .expect("save lyrics");
    assert_eq!(saved_path, output);
    assert_eq!(
        fs::read_to_string(&saved_path).expect("saved lyrics"),
        "[00:01.00]line one"
    );
    assert!(!sidecar.exists());
    assert!(!dir.join("Chosen Lyrics.lrc.tmp").exists());
    assert_eq!(lyrics.track_id, entry.track_id);
    let _cleanup = fs::remove_dir_all(dir);
}
#[test]
pub(in crate::controller) fn local_sidecar_lyrics_use_same_stem_as_audio_file() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let dir = self::unique_test_dir("local-sidecar");
    fs::create_dir_all(&dir).expect("create dir");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: dir.to_string_lossy().into_owned(),
                path_replace_from: None,
                path_replace_to: Some(dir.to_string_lossy().into_owned()),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let audio = dir.join("07 I'm feeling lucky.flac");
    let lrc = dir.join("07 I'm feeling lucky.lrc");
    fs::write(&audio, []).expect("audio");
    fs::write(&lrc, "[00:01.00]line one").expect("lrc");
    let mut track = restored_track();
    track.local_path = Some(audio.to_string_lossy().into_owned());
    store
        .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
        .expect("upsert track");
    let lyrics =
        super::local_sidecar_lyrics(&store, &saved.server.id, &track.id).expect("sidecar lyrics");
    assert_eq!(lyrics.source, LyricsSource::Local);
    assert_eq!(lyrics.lines[0].text, "line one");
    assert_eq!(lyrics.lines[0].start_millis, Some(1_000));
    let _cleanup = fs::remove_dir_all(dir);
}
#[test]
pub(in crate::controller) fn local_sidecar_lyrics_ignore_oversized_files() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let dir = self::unique_test_dir("local-sidecar-large");
    fs::create_dir_all(&dir).expect("create dir");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: dir.to_string_lossy().into_owned(),
                path_replace_from: None,
                path_replace_to: Some(dir.to_string_lossy().into_owned()),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let audio = dir.join("Track.flac");
    let lrc = dir.join("Track.lrc");
    fs::write(&audio, []).expect("audio");
    let file = fs::File::create(&lrc).expect("lrc");
    file.set_len((LOCAL_LYRICS_MAX_BYTES + 1) as u64)
        .expect("lrc length");
    let mut track = restored_track();
    track.local_path = Some(audio.to_string_lossy().into_owned());
    store
        .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
        .expect("upsert track");

    let lyrics = super::local_sidecar_lyrics(&store, &saved.server.id, &track.id);

    assert_eq!(lyrics, None);
    let _cleanup = fs::remove_dir_all(dir);
}
#[test]
pub(in crate::controller) fn mapped_local_audio_path_uses_server_prefix_replacement() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: "/unused".to_string(),
                path_replace_from: Some("/server/music".to_string()),
                path_replace_to: Some(
                    self::unique_test_dir("mapped-audio")
                        .to_string_lossy()
                        .into_owned(),
                ),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let root = store
        .with_store(|store| store.server_local_access(&saved.server.id))
        .expect("access")
        .expect("access")
        .path_replace_to
        .expect("replace to");
    let root = PathBuf::from(root);
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let mut track = restored_track();
    track.local_path = Some("/server/music/Album/Track.flac".to_string());
    store
        .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
        .expect("upsert track");
    let mapped = super::local_audio_path_for_track(&store, &saved.server.id, &track.id)
        .expect("mapped path");
    assert_eq!(mapped, audio);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn remote_local_audio_path_requires_configured_access() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let dir = self::unique_test_dir("remote-no-local-access");
    fs::create_dir_all(&dir).expect("create dir");
    let audio = dir.join("Track.flac");
    fs::write(&audio, []).expect("audio");
    let mut track = restored_track();
    track.local_path = Some(audio.to_string_lossy().into_owned());
    store
        .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
        .expect("upsert track");
    let mapped = super::local_audio_path_for_track(&store, &saved.server.id, &track.id);
    assert_eq!(mapped, None);
    let _cleanup = fs::remove_dir_all(dir);
}
#[test]
pub(in crate::controller) fn resolve_stream_prefers_local_file_for_remote_server_with_access() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let root = self::unique_test_dir("local-playback-stream");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: root.to_string_lossy().into_owned(),
                path_replace_from: Some("/server/music".to_string()),
                path_replace_to: Some(root.to_string_lossy().into_owned()),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let mut track = restored_track();
    track.local_path = Some("/server/music/Album/Track.flac".to_string());
    store
        .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
        .expect("upsert track");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let stream = super::resolve_stream(
        &store,
        &runtime,
        &secrets,
        &saved.server.id,
        &track.id,
        &PlaybackSettings::default(),
    )
    .expect("stream");
    assert!(stream.uri().starts_with("file://"));
    assert!(stream.uri().contains("Track.flac"));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn resolve_stream_uses_requested_saved_server_when_active_source_changes()
{
    let store = StoreHandle::open_memory().expect("memory store");
    let playback_server = SavedServer {
        server: ServerIdentity {
            id: ServerId::new("fake:server:playback"),
            provider: "fake".to_string(),
            name: "Playback Server".to_string(),
            base_url: "https://playback.example.test".to_string(),
        },
        user_id: "listener".to_string(),
        username: "listener".to_string(),
        trust_invalid_cert: false,
    };
    let local = local_source_saved();
    store
        .with_store(|store| {
            store.save_server(&playback_server)?;
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)
        })
        .expect("seed servers");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let track_id = TrackId::new("fake:track:queued");

    let stream = super::resolve_stream(
        &store,
        &runtime,
        &secrets,
        &playback_server.server.id,
        &track_id,
        &PlaybackSettings::default(),
    )
    .expect("stream");

    assert_eq!(stream.uri(), "fake://local/stream/fake:track:queued");
}
#[test]
pub(in crate::controller) fn resolve_stream_uses_cached_file_for_local_source() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let root = self::unique_test_dir("local-source-stream");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let mut track = restored_track();
    track.id = TrackId::new("local:track:stream");
    track.local_path = Some(audio.to_string_lossy().into_owned());
    store
        .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
        .expect("upsert track");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let stream = super::resolve_stream(
        &store,
        &runtime,
        &secrets,
        &saved.server.id,
        &track.id,
        &PlaybackSettings::default(),
    )
    .expect("stream");

    assert!(stream.uri().starts_with("file://"));
    assert!(stream.uri().contains("Track.flac"));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn resolve_stream_uses_cached_local_match_without_server_path() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let root = self::unique_test_dir("cached-local-match-stream");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: root.to_string_lossy().into_owned(),
                path_replace_from: None,
                path_replace_to: Some(root.to_string_lossy().into_owned()),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let track = restored_track();
    store
        .with_store(|store| {
            store.upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)?;
            store.replace_track_local_matches(
                &saved.server.id,
                &[(
                    track.id.clone(),
                    audio.to_string_lossy().into_owned(),
                    "metadata".to_string(),
                )],
            )
        })
        .expect("seed track");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let stream = super::resolve_stream(
        &store,
        &runtime,
        &secrets,
        &saved.server.id,
        &track.id,
        &PlaybackSettings::default(),
    )
    .expect("stream");
    assert!(stream.uri().starts_with("file://"));
    assert!(stream.uri().contains("Track.flac"));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn relative_local_audio_path_uses_configured_local_prefix() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let scan_root = self::unique_test_dir("relative-scan-root");
    let local_root = self::unique_test_dir("relative-local-prefix");
    let audio = local_root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: scan_root.to_string_lossy().into_owned(),
                path_replace_from: None,
                path_replace_to: Some(local_root.to_string_lossy().into_owned()),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let mut track = restored_track();
    track.local_path = Some("Album/Track.flac".to_string());
    store
        .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
        .expect("upsert track");
    let mapped = super::local_audio_path_for_track(&store, &saved.server.id, &track.id)
        .expect("mapped path");
    assert_eq!(mapped, audio);
    let _cleanup = fs::remove_dir_all(scan_root);
    let _cleanup = fs::remove_dir_all(local_root);
}
#[test]
pub(in crate::controller) fn local_access_matching_uses_manifest_cached_track_data() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let root = self::unique_test_dir("local-access-manifest");
    let audio = root.join("Album/Filename Fallback.mp3");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: root.to_string_lossy().into_owned(),
                path_replace_from: None,
                path_replace_to: Some(root.to_string_lossy().into_owned()),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let mut remote = restored_track();
    remote.title = "Manifest Title".to_string();
    remote.album = "Manifest Album".to_string();
    remote.artist = "Manifest Artist".to_string();
    remote.duration_seconds = 0;
    let mut local = remote.clone();
    local.id = TrackId::new("local:track:manifest");
    local.album_id = AlbumId::new("local:album:manifest");
    local.local_path = Some(audio.to_string_lossy().into_owned());
    let manifest = LocalManifestEntry {
        facts: local_manifest_file_facts(&root, &audio),
        track: local,
        album_artist: "Manifest Artist".to_string(),
        cover: None,
        metadata_hash: "metadata".to_string(),
        search_hash: "search".to_string(),
    };
    store
        .with_store(|store| {
            store.upsert_tracks(&saved.server.id, &[remote.clone()], generation)?;
            store.replace_local_manifest(&saved.server.id, generation, &[manifest])
        })
        .expect("seed tracks and manifest");
    let runtime = Runtime::new().expect("runtime");

    let count = runtime
        .block_on(super::refresh_local_track_matches(
            &store,
            &saved.server.id,
            Some(generation),
        ))
        .expect("refresh local matches");

    assert_eq!(count, 1);
    assert_eq!(
        store
            .with_store(|store| store.track_local_match_path(&saved.server.id, &remote.id))
            .expect("match path"),
        Some(audio.to_string_lossy().into_owned())
    );
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn snapshot_local_access_status_counts_cached_mapping_candidates() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let root = self::unique_test_dir("local-access-status");
    let local_prefix = root.join("mapped");
    let direct_audio = root.join("Direct.flac");
    let prefix_audio = local_prefix.join("Album/Mapped.flac");
    let metadata_audio = root.join("Metadata.flac");
    fs::create_dir_all(prefix_audio.parent().expect("parent")).expect("create mapped dir");
    fs::write(&direct_audio, []).expect("direct audio");
    fs::write(&prefix_audio, []).expect("prefix audio");
    fs::write(&metadata_audio, []).expect("metadata audio");
    let generation = store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)?;
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: root.to_string_lossy().into_owned(),
                path_replace_from: Some("/server/music".to_string()),
                path_replace_to: Some(local_prefix.to_string_lossy().into_owned()),
            })?;
            store.begin_sync(&saved.server.id)
        })
        .expect("begin sync");
    let mut direct = restored_track();
    direct.id = TrackId::new("jellyfin:track:direct");
    direct.title = "Direct".to_string();
    direct.local_path = Some(direct_audio.to_string_lossy().into_owned());
    let mut prefix = restored_track();
    prefix.id = TrackId::new("jellyfin:track:prefix");
    prefix.title = "Prefix".to_string();
    prefix.local_path = Some("/server/music/Album/Mapped.flac".to_string());
    let mut metadata = restored_track();
    metadata.id = TrackId::new("jellyfin:track:metadata");
    metadata.title = "Metadata".to_string();
    let mut unmatched = restored_track();
    unmatched.id = TrackId::new("jellyfin:track:unmatched");
    unmatched.title = "Unmatched".to_string();
    unmatched.local_path = Some("/server/music/Album/Missing.flac".to_string());
    store
        .with_store(|store| {
            store.upsert_tracks(
                &saved.server.id,
                &[direct, prefix, metadata.clone(), unmatched],
                generation,
            )?;
            store.replace_track_local_matches(
                &saved.server.id,
                &[(
                    metadata.id.clone(),
                    metadata_audio.to_string_lossy().into_owned(),
                    "metadata".to_string(),
                )],
            )
        })
        .expect("seed tracks");
    let snapshot = super::load_snapshot(&store).expect("load snapshot");
    assert_eq!(snapshot.local_access_status.total_track_count, 4);
    assert_eq!(snapshot.local_access_status.direct_match_count, 1);
    assert_eq!(snapshot.local_access_status.prefix_match_count, 2);
    assert_eq!(snapshot.local_access_status.metadata_match_count, 1);
    assert_eq!(snapshot.local_access_status.unmatched_count, 0);
    assert!(snapshot.local_access_status.sample_server_path.is_some());
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn conservative_local_matches_only_accept_unique_duration_matches() {
    let album = AlbumId::fake(1);
    let mut remote = restored_track();
    remote.album_id = album.clone();
    remote.title = "First Motion".to_string();
    remote.album = "Blue Rooms".to_string();
    remote.artist = "Astral Kin".to_string();
    remote.duration_seconds = 210;
    remote.disc_number = 1;
    remote.track_number = 7;
    let mut local = remote.clone();
    local.id = TrackId::new("local:track:one");
    local.local_path = Some("/home/me/Music/Blue Rooms/07 First Motion.flac".to_string());
    local.duration_seconds = 212;
    let matches = super::conservative_local_matches(&[remote.clone()], &[local.clone()]);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].0, remote.id);
    assert_eq!(
        matches[0].1,
        "/home/me/Music/Blue Rooms/07 First Motion.flac"
    );
    let local_one = local.clone();
    let mut duplicate = local;
    duplicate.id = TrackId::new("local:track:two");
    duplicate.local_path = Some("/home/me/Music/Other/07 First Motion.flac".to_string());
    assert!(super::conservative_local_matches(&[remote], &[local_one, duplicate]).is_empty());
}
#[test]
pub(in crate::controller) fn snapshot_includes_active_server_local_access() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_server();
    let access = ServerLocalAccess {
        server_id: saved.server.id.clone(),
        root_path: "/home/demo/Music".to_string(),
        path_replace_from: Some("/server/music".to_string()),
        path_replace_to: Some("/home/demo/Music".to_string()),
    };
    store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.save_server_local_access(&access)?;
            store.set_active_server(&saved.server.id)
        })
        .expect("save server");
    let snapshot = super::load_snapshot(&store).expect("load snapshot");
    assert_eq!(snapshot.local_access, Some(access));
}
#[test]
pub(in crate::controller) fn lrclib_result_text_becomes_timed_lyrics() {
    let result = super::LyricsSearchResult {
        id: 7,
        track_name: "Song".to_string(),
        artist_name: "Artist".to_string(),
        album_name: "Album".to_string(),
        duration_seconds: 180,
        synced_lyrics: Some(
            "[00:12.34]first line\n[ar:Artist]\n[00:13.005]second line".to_string(),
        ),
        plain_lyrics: None,
    };
    let lyrics = super::lyrics_from_text(TrackId::new("track-one"), &result);
    assert_eq!(lyrics.lines.len(), 2);
    assert_eq!(lyrics.lines[0].text, "first line");
    assert_eq!(lyrics.lines[0].start_millis, Some(12_340));
    assert_eq!(lyrics.lines[1].text, "second line");
    assert_eq!(lyrics.lines[1].start_millis, Some(13_005));
}
#[test]
pub(in crate::controller) fn selected_lrclib_result_becomes_current_track_lyrics() {
    let result = super::LyricsSearchResult {
        id: 12,
        track_name: "Example Track".to_string(),
        artist_name: "Example Artist".to_string(),
        album_name: "Example Album".to_string(),
        duration_seconds: 95,
        synced_lyrics: Some("[00:01.00]line one".to_string()),
        plain_lyrics: Some("line one".to_string()),
    };

    let lyrics = super::lyrics_from_lrclib_search_result(TrackId::new("track-preview"), &result)
        .expect("lyrics");

    assert_eq!(lyrics.track_id, TrackId::new("track-preview"));
    assert_eq!(lyrics.source, LyricsSource::Remote);
    assert_eq!(lyrics.lines[0].text, "line one");
    assert_eq!(lyrics.lines[0].start_millis, Some(1_000));
}
#[test]
pub(in crate::controller) fn lrclib_duration_accepts_fractional_seconds() {
    let json = r#"{
            "id": 7,
            "trackName": "Imagine",
            "artistName": "John Lennon",
            "albumName": "Imagine",
            "duration": 185.0,
            "plainLyrics": "line",
            "syncedLyrics": null
        }"#;
    let dto = serde_json::from_str::<super::LrcLibLyricsDto>(json).expect("deserialize lrclib dto");
    let result = super::LyricsSearchResult::from(dto);
    assert_eq!(result.duration_seconds, 185);
    assert_eq!(result.track_name, "Imagine");
    assert_eq!(result.artist_name, "John Lennon");
}
#[test]
pub(in crate::controller) fn lrclib_manual_search_uses_combined_query_first() {
    let urls = super::lrclib_search_urls("joy", "feel my soul").expect("lrclib search urls");
    let query_pairs = urls[0]
        .query_pairs()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(
        query_pairs,
        vec![("q".to_string(), "feel my soul joy".to_string())]
    );
}
#[test]
pub(in crate::controller) fn lrclib_manual_search_keeps_single_field_fallbacks() {
    let urls =
        super::lrclib_search_urls("Example Artist", "Opening Theme").expect("lrclib search urls");
    let query_sets = urls
        .iter()
        .map(|url| {
            url.query_pairs()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        query_sets,
        vec![
            vec![("q".to_string(), "Opening Theme Example Artist".to_string())],
            vec![
                ("track_name".to_string(), "Opening Theme".to_string()),
                ("artist_name".to_string(), "Example Artist".to_string()),
            ],
            vec![("q".to_string(), "Opening Theme".to_string())],
            vec![("track_name".to_string(), "Opening Theme".to_string())],
            vec![("q".to_string(), "Example Artist".to_string())],
        ]
    );
}
#[test]
pub(in crate::controller) fn lrclib_automatic_search_requires_track_and_artist() {
    let urls = super::lrclib_automatic_search_urls("Example Artist", "Opening Theme")
        .expect("lrclib automatic search urls");
    let query_sets = urls
        .iter()
        .map(|url| {
            url.query_pairs()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        query_sets,
        vec![
            vec![("q".to_string(), "Opening Theme Example Artist".to_string())],
            vec![
                ("track_name".to_string(), "Opening Theme".to_string()),
                ("artist_name".to_string(), "Example Artist".to_string()),
            ],
        ]
    );
    assert!(
        super::lrclib_automatic_search_urls("", "Opening Theme")
            .expect("missing artist urls")
            .is_empty()
    );
    assert!(
        super::lrclib_automatic_search_urls("Example Artist", "")
            .expect("missing track urls")
            .is_empty()
    );
}
#[test]
pub(in crate::controller) fn lrclib_exact_lookup_url_uses_track_artist_and_duration() {
    let url = super::lrclib_get_url("The Cure", "Lovesong", 210)
        .expect("lrclib get url")
        .expect("url");

    assert_eq!(
        url.as_str(),
        "https://lrclib.net/api/get?track_name=Lovesong&artist_name=The+Cure&duration=210"
    );
    let url = super::lrclib_get_url("The Cure", "Lovesong", 0)
        .expect("lrclib get url")
        .expect("url");
    assert_eq!(
        url.as_str(),
        "https://lrclib.net/api/get?track_name=Lovesong&artist_name=The+Cure"
    );
}
#[test]
pub(in crate::controller) fn automatic_lrclib_fallback_skips_empty_hits() {
    let entry = QueueEntry {
        id: QueueEntryId::new("queue-entry:lyrics-fallback"),
        track_id: TrackId::new("jellyfin:track:lovesong"),
        album_id: Some(AlbumId::fake(1)),
        title: "Lovesong".to_string(),
        artist: "The Cure".to_string(),
        artist_id: Some(ArtistId::fake(1)),
        album: "Disintegration".to_string(),
        year: 1989,
        duration_seconds: 210,
        favorite: false,
        image_ref: None,
        local_path: None,
        source_format: None,
        origin: None,
    };
    let results = vec![
        super::LyricsSearchResult {
            id: 1,
            track_name: "Lovesong".to_string(),
            artist_name: "The Cure".to_string(),
            album_name: "Disintegration".to_string(),
            duration_seconds: 210,
            synced_lyrics: None,
            plain_lyrics: None,
        },
        super::LyricsSearchResult {
            id: 2,
            track_name: "Lovesong".to_string(),
            artist_name: "The Cure".to_string(),
            album_name: "Disintegration".to_string(),
            duration_seconds: 210,
            synced_lyrics: Some("[00:01.00]first line".to_string()),
            plain_lyrics: Some("first line".to_string()),
        },
    ];

    let lyrics = super::lyrics_from_lrclib_results(&entry, results).expect("lyrics");

    assert_eq!(lyrics.track_id, entry.track_id);
    assert_eq!(lyrics.source, LyricsSource::Remote);
    assert_eq!(lyrics.lines[0].text, "first line");
    assert_eq!(lyrics.lines[0].start_millis, Some(1_000));
}
#[test]
pub(in crate::controller) fn lrclib_search_body_decodes_feel_my_soul_result() {
    let json = r#"[{
            "id": 9386114,
            "name": "feel my soul",
            "artistName": "joy",
            "albumName": "feel my soul",
            "duration": 223.0,
            "plainLyrics": "plain line",
            "syncedLyrics": "[00:01.00]synced line",
            "lyricsfile": null
        }]"#;
    let results = super::parse_lrclib_search_body(json).expect("parse lrclib response");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, 9_386_114);
    assert_eq!(results[0].track_name, "feel my soul");
    assert_eq!(results[0].artist_name, "joy");
    assert_eq!(results[0].duration_seconds, 223);
    assert!(results[0].synced_lyrics.is_some());
    assert!(results[0].plain_lyrics.is_some());
}
#[test]
pub(in crate::controller) fn lrclib_search_body_accepts_name_and_track_name_fields() {
    let json = r#"[{
            "id": 12,
            "name": "Legacy Name",
            "trackName": "Current Name",
            "artistName": "Example Artist",
            "albumName": "Example Album",
            "duration": 95.0,
            "plainLyrics": "line",
            "syncedLyrics": null
        }]"#;

    let results = super::parse_lrclib_search_body(json).expect("parse lrclib response");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].track_name, "Current Name");
    assert_eq!(results[0].artist_name, "Example Artist");
}
#[test]
pub(in crate::controller) fn lrclib_results_prefer_matching_title_over_album_hit() {
    let mut results = vec![
        super::LyricsSearchResult {
            id: 1,
            track_name: "Crippled Inside".to_string(),
            artist_name: "John Lennon".to_string(),
            album_name: "Imagine".to_string(),
            duration_seconds: 233,
            synced_lyrics: Some("[00:01.00]line".to_string()),
            plain_lyrics: Some("line".to_string()),
        },
        super::LyricsSearchResult {
            id: 2,
            track_name: "Imagine".to_string(),
            artist_name: "John Lennon".to_string(),
            album_name: "Lennon".to_string(),
            duration_seconds: 185,
            synced_lyrics: None,
            plain_lyrics: Some("line".to_string()),
        },
    ];
    super::order_lrclib_results(&mut results, "John Lennon", "Imagine");
    assert_eq!(results[0].track_name, "Imagine");
}
#[test]
pub(in crate::controller) fn lrclib_results_prefer_compact_title_token_matches() {
    let mut results = vec![
        super::LyricsSearchResult {
            id: 1,
            track_name: "Long Title With Part Token".to_string(),
            artist_name: "Example Artist".to_string(),
            album_name: "Example Album".to_string(),
            duration_seconds: 240,
            synced_lyrics: Some("[00:01.00]line".to_string()),
            plain_lyrics: Some("line".to_string()),
        },
        super::LyricsSearchResult {
            id: 2,
            track_name: "Part Two".to_string(),
            artist_name: "Example Artist".to_string(),
            album_name: "Example Album".to_string(),
            duration_seconds: 120,
            synced_lyrics: Some("[00:01.00]line".to_string()),
            plain_lyrics: Some("line".to_string()),
        },
    ];

    super::order_lrclib_results(&mut results, "", "part");

    assert_eq!(results[0].track_name, "Part Two");
}
#[test]
pub(in crate::controller) fn controller_events_are_sendable() {
    pub(in crate::controller) fn assert_send<T: Send>() {}
    assert_send::<ControllerEvent>();
}
#[test]
pub(in crate::controller) fn provider_not_found_cover_errors_are_classified() {
    assert!(super::covers::is_provider_not_found_error(
        "provider item was not found"
    ));
    assert!(!super::covers::is_provider_not_found_error(
        "provider network failed: offline"
    ));
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
    let queue = restore_queue(&store, snapshot.server.as_ref());
    let playback_snapshot =
        playback_snapshot_from_queue(queue.as_ref(), settings.auto_dj_enabled, &settings.playback);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let controller = AppController {
        settings: super::settings_controller::SettingsController::new(
            store.clone(),
            secrets.clone(),
        ),
        store,
        runtime,
        secrets,
        queue: Arc::new(Mutex::new(queue)),
        play_activation_generation: Arc::new(AtomicU64::new(0)),
        queue_persist_generation: Arc::new(AtomicU64::new(0)),
        playback_request_generation: Arc::new(AtomicU64::new(0)),
        playback: Arc::new(Mutex::new(Box::new(
            rufin_playback::FakePlaybackBackend::new(),
        ))),
        playback_snapshot: Arc::new(Mutex::new(playback_snapshot)),
        playback_activity: Arc::new(Mutex::new(PlaybackActivityState::default())),
        playback_start_probe: Arc::new(Mutex::new(None)),
        auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
        last_progress_snapshot: Arc::new(Mutex::new(None)),
        last_report_snapshot: Arc::new(Mutex::new(None)),
        external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
        external_cover_retry_generation: Arc::new(AtomicU64::new(0)),
        events,
        sync_in_flight: InFlightGuards::new("Sync"),
        home_refresh_in_flight: InFlightGuards::new("Home refresh"),
        playlist_refresh_in_flight: InFlightGuards::new("Playlist refresh"),
        explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
        cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
        external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
        cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        _test_permit: test_permit,
    };
    (controller, receiver)
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
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
    }
}
pub(in crate::controller) fn saved_server() -> SavedServer {
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
#[test]
pub(in crate::controller) fn grouped_cover_refs_keep_one_unique_cover_full_size() {
    let cover = test_image_ref(1);
    let albums = vec![library_album(
        1,
        "Example Artist",
        "Example Album",
        Some(cover.clone()),
    )];
    let refs = super::grouped_cover_refs_for_items(&albums, &[]);
    assert_eq!(refs, vec![cover]);
}
#[test]
pub(in crate::controller) fn grouped_cover_refs_deduplicate_and_limit_to_four() {
    let first = test_image_ref(1);
    let second = test_image_ref(2);
    let third = test_image_ref(3);
    let fourth = test_image_ref(4);
    let fifth = test_image_ref(5);
    let albums = vec![
        library_album(1, "Example Artist", "First", Some(first.clone())),
        library_album(2, "Example Artist", "Duplicate", Some(first.clone())),
        library_album(3, "Example Artist", "Second", Some(second.clone())),
    ];
    let mut tracks = vec![
        library_track(1, None, AlbumId::fake(1), "Example Artist", &[]),
        library_track(2, None, AlbumId::fake(2), "Example Artist", &[]),
        library_track(3, None, AlbumId::fake(3), "Example Artist", &[]),
    ];
    tracks[0].image_ref = Some(third.clone());
    tracks[1].image_ref = Some(fourth.clone());
    tracks[2].image_ref = Some(fifth);
    let refs = super::grouped_cover_refs_for_items(&albums, &tracks);
    assert_eq!(refs, vec![first, second, third, fourth]);
}
#[test]
pub(in crate::controller) fn artist_detail_fallback_uses_external_album_image_after_normalization()
{
    let mut detail = CachedArtistDetail {
        artist: rufin_core::Artist {
            id: ArtistId::fake(1),
            name: "Example Artist".to_string(),
            album_count: 1,
            track_count: 0,
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
            image_ref: None,
        },
        albums: vec![library_album(1, "Example Artist", "Example Album", None)],
        appears_on: Vec::new(),
        tracks: Vec::new(),
    };
    let settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };
    super::normalize_artist_detail_image_refs(&mut detail, &settings);
    let image_ref = detail.artist.image_ref.expect("artist fallback image ref");
    assert!(image_ref.item_id.starts_with("external:album:"));
    assert!(
        image_ref
            .item_id
            .contains("Example%20Artist:Example%20Album")
    );
}
#[test]
pub(in crate::controller) fn artist_collection_fallback_uses_external_album_image_after_normalization()
 {
    let artist_id = ArtistId::fake(1);
    let mut artists = vec![rufin_core::Artist {
        id: artist_id.clone(),
        name: "Example Artist".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    }];
    let fallback_albums = std::collections::HashMap::from([(
        artist_id,
        library_album(1, "Example Artist", "Example Album", None),
    )]);
    let settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };

    super::apply_artist_album_fallback_image_refs(&mut artists, fallback_albums, &settings);

    let image_ref = artists[0]
        .image_ref
        .as_ref()
        .expect("artist fallback image ref");
    assert!(image_ref.item_id.starts_with("external:album:"));
    assert!(
        image_ref
            .item_id
            .contains("Example%20Artist:Example%20Album")
    );
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
) -> rufin_core::LocalFileFacts {
    let metadata = fs::metadata(path).expect("metadata");
    let modified = metadata.modified().expect("modified time");
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .expect("modified after epoch");
    rufin_core::LocalFileFacts {
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
    }
}
