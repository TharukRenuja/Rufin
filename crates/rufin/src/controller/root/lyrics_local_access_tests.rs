use super::*;
use metadata::ExternalLyricsProvider;
use sources::MusicSource;
use std::io::{Read, Write};
use std::net::TcpListener;

fn active_source_for_test(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> ActiveSourceSlot {
    let active = crate::source_setup::activate_configured_source(store, secrets, saved)
        .expect("activate saved source");
    Arc::new(std::sync::RwLock::new(Some(active)))
}

fn controller_with_current_track(
    saved: &StoredSource,
    track: &Track,
) -> (ProductOwners, ProductReceivers) {
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(&store, saved, &[], std::slice::from_ref(track), &[]);
    seed_playback_checkpoint(&store, &saved.source_id, track, 0);
    owners_from_store_for_test(store)
}

fn media_key(source_id: &SourceId, track_id: &TrackId) -> playback::MediaKey {
    playback::MediaKey {
        source_id: source_id.clone(),
        track_id: track_id.clone(),
    }
}

fn seed_playback_checkpoint(
    store: &StoreHandle,
    source_id: &SourceId,
    track: &Track,
    progress_millis: u64,
) {
    let mut sequence = playback::Sequence::new(source_id.clone());
    sequence
        .apply_batch(
            playback::Batch::new(vec![playback::BatchItem::new(
                track.clone(),
                playback::Provenance::Manual,
            )]),
            playback::Placement::Replace { anchor_index: 0 },
        )
        .expect("seed playback sequence");
    sequence.set_progress_millis(progress_millis);
    let checkpoint = playback::encode_checkpoint(&sequence).expect("encode playback checkpoint");
    store
        .with_store(|store| {
            store.save_playback_checkpoint(&library::PlaybackCheckpointRecord {
                source_id: checkpoint.header.source_id,
                revision: checkpoint.header.revision,
                selected_occurrence_id: checkpoint
                    .header
                    .selected_occurrence
                    .map(|occurrence| occurrence.to_string()),
                progress_millis: checkpoint.header.progress_millis,
                repeat_mode: "Off".to_string(),
                shuffle_enabled: checkpoint.header.shuffle_enabled,
                payload: checkpoint.payload,
            })
        })
        .expect("seed playback checkpoint");
}

#[test]
pub(in crate::controller) fn store_album_favorite_updates_projection() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let album = local_album_with_image_ref(ImageRef::new("local:album:favorite", None));
    seed_cached_library(&store, &saved, std::slice::from_ref(&album), &[], &[]);
    let (owners, events) = owners_from_store_for_test(store);

    owners.library.set_album_favorite(album.id.clone(), true);
    let (item_id, favorite) = wait_for_favorite_changed(&events);
    assert_eq!(item_id, FavoriteItemId::Album(album.id.clone()));
    assert!(favorite);
    assert!(
        owners
            .library
            .library_query(saved.source_id.clone())
            .album_detail(&album.id)
            .expect("album detail")
            .map(|(album, _)| album)
            .expect("cached album")
            .favorite
    );
    owners.library.set_album_favorite(album.id.clone(), false);
    let (item_id, favorite) = wait_for_favorite_changed(&events);
    assert_eq!(item_id, FavoriteItemId::Album(album.id.clone()));
    assert!(!favorite);
    assert!(
        !owners
            .library
            .library_query(saved.source_id.clone())
            .album_detail(&album.id)
            .expect("album detail")
            .map(|(album, _)| album)
            .expect("cached album")
            .favorite
    );
}
#[test]
pub(in crate::controller) fn store_empty_playlist_has_empty_detail() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    seed_cached_library(&store, &saved, &[], &[], &[]);
    let root = self::unique_test_dir("empty-local-playlist");
    fs::create_dir_all(&root).expect("create empty local root");
    let mut settings = store.load_settings();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save local settings");
    let (owners, events) = owners_from_store_for_test(store);

    owners
        .library
        .create_playlist("Empty Playlist".to_string(), Vec::new());
    let playlist_id = wait_for_playlist_changed(&events);
    let playlist = owners
        .library
        .library_query(saved.source_id.clone())
        .playlists_page(0, 10)
        .expect("playlists")
        .items
        .into_iter()
        .find(|playlist| playlist.id == playlist_id)
        .expect("created playlist");

    assert_eq!(playlist.track_count, 0);
    assert_eq!(playlist.duration_seconds, 0);
    assert!(
        owners
            .library
            .library_query(saved.source_id.clone())
            .playlist_detail(&playlist.id)
            .expect("playlist detail")
            .expect("playlist detail")
            .entries
            .is_empty()
    );
    assert!(
        events.source_presentation.try_recv().is_err(),
        "playlist creation must not publish source presentation"
    );
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn store_playlist_commands_preserve_exact_order() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let mut first = restored_track();
    first.id = TrackId::new("local:track:playlist-one");
    let mut second = restored_track();
    second.id = TrackId::new("local:track:playlist-two");
    let mut third = restored_track();
    third.id = TrackId::new("local:track:playlist-three");
    seed_cached_library(
        &store,
        &saved,
        &[],
        &[first.clone(), second.clone(), third.clone()],
        &[],
    );
    let (owners, events) = owners_from_store_for_test(store.clone());

    owners.library.create_playlist(
        "Local Playlist".to_string(),
        vec![first.clone(), second.clone()],
    );
    let created_id = wait_for_playlist_changed(&events);
    let playlist = owners
        .library
        .library_query(saved.source_id.clone())
        .playlists_page(0, 10)
        .expect("playlists")
        .items
        .into_iter()
        .find(|playlist| playlist.id == created_id)
        .expect("created local playlist");
    assert_eq!(
        store
            .with_store(|store| store.playlist_owner(&saved.source_id, &playlist.id))
            .expect("playlist owner"),
        Some(SourceFeatureOwner::Store)
    );
    assert_playlist_order(
        &owners.library,
        &saved.source_id,
        &playlist.id,
        &[first.id.as_str(), second.id.as_str()],
    );

    let detail = owners
        .library
        .library_query(saved.source_id.clone())
        .playlist_detail(&playlist.id)
        .expect("playlist detail")
        .expect("playlist detail");
    owners
        .library
        .move_playlist_entry(playlist.id.clone(), detail.entries[1].entry_id.clone(), 0);
    let changed_id = wait_for_playlist_changed(&events);
    assert_eq!(changed_id, playlist.id);
    assert_playlist_order(
        &owners.library,
        &saved.source_id,
        &playlist.id,
        &[second.id.as_str(), first.id.as_str()],
    );

    owners.library.add_context_to_playlist(
        playlist.id.clone(),
        library::play_context::PlayContextDescriptor::Global {
            music_folder_id: None,
        },
        true,
    );
    let changed_id = wait_for_playlist_changed(&events);
    assert_eq!(changed_id, playlist.id);
    assert_playlist_order(
        &owners.library,
        &saved.source_id,
        &playlist.id,
        &[second.id.as_str(), first.id.as_str(), third.id.as_str()],
    );

    let detail = owners
        .library
        .library_query(saved.source_id.clone())
        .playlist_detail(&playlist.id)
        .expect("playlist detail")
        .expect("playlist detail");
    owners
        .library
        .remove_playlist_entry(playlist.id.clone(), detail.entries[0].entry_id.clone());
    let changed_id = wait_for_playlist_changed(&events);
    assert_eq!(changed_id, playlist.id);
    assert_playlist_order(
        &owners.library,
        &saved.source_id,
        &playlist.id,
        &[first.id.as_str(), third.id.as_str()],
    );

    owners
        .library
        .rename_playlist(playlist.id.clone(), "Renamed Playlist".to_string());
    assert_eq!(wait_for_playlist_changed(&events), playlist.id);
    assert_eq!(
        owners
            .library
            .library_query(saved.source_id.clone())
            .playlist_detail(&playlist.id)
            .expect("renamed playlist detail")
            .expect("renamed playlist")
            .playlist
            .name,
        "Renamed Playlist"
    );

    owners.library.delete_playlist(playlist.id.clone());
    assert_eq!(wait_for_playlist_changed(&events), playlist.id);
    assert!(
        owners
            .library
            .library_query(saved.source_id.clone())
            .playlist_detail(&playlist.id)
            .expect("deleted playlist detail")
            .is_none()
    );
    assert!(
        events.source_presentation.try_recv().is_err(),
        "playlist mutations must not publish source presentation"
    );
}

#[test]
pub(in crate::controller) fn smart_playlist_commands_publish_library_facts_only() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    seed_cached_library(&store, &saved, &[], &[], &[]);
    let (owners, events) = owners_from_store_for_test(store);
    let query = owners.library.library_query(saved.source_id.clone());
    let definition =
        library::smart_playlists::builtin_definition(SmartPlaylistBuiltin::NeverPlayed);

    owners
        .library
        .save_smart_playlist("Custom Smart".to_string(), definition.clone());
    let custom_id = wait_for_smart_playlist_changed(&events);
    assert_eq!(
        query
            .smart_playlist_detail(&custom_id)
            .expect("created smart playlist detail")
            .expect("created smart playlist")
            .smart_playlist
            .name,
        "Custom Smart"
    );

    owners.library.update_smart_playlist(
        custom_id.clone(),
        "Updated Smart".to_string(),
        definition,
    );
    assert_eq!(wait_for_smart_playlist_changed(&events), custom_id);
    assert_eq!(
        query
            .smart_playlist_detail(&custom_id)
            .expect("updated smart playlist detail")
            .expect("updated smart playlist")
            .smart_playlist
            .name,
        "Updated Smart"
    );

    let builtins = query
        .smart_playlists_page(0, 100)
        .expect("smart playlist index");
    let most_played_id = builtins
        .items
        .iter()
        .find(|playlist| playlist.builtin == Some(SmartPlaylistBuiltin::MostPlayed))
        .expect("Most Played smart playlist")
        .id
        .clone();
    owners
        .library
        .move_smart_playlist(custom_id.clone(), most_played_id.clone(), false);
    assert_eq!(wait_for_smart_playlist_changed(&events), custom_id);

    owners.library.delete_smart_playlist(custom_id.clone());
    assert_eq!(wait_for_smart_playlist_changed(&events), custom_id);
    assert!(
        query
            .smart_playlist_detail(&custom_id)
            .expect("deleted smart playlist detail")
            .is_none()
    );

    owners.library.delete_smart_playlist(most_played_id.clone());
    assert_eq!(wait_for_smart_playlist_changed(&events), most_played_id);
    owners
        .library
        .restore_builtin_smart_playlist(SmartPlaylistBuiltin::MostPlayed);
    assert_eq!(wait_for_smart_playlist_changed(&events), most_played_id);
    assert!(
        query
            .smart_playlist_detail(&most_played_id)
            .expect("restored smart playlist detail")
            .is_some()
    );
    assert!(
        events.source_presentation.try_recv().is_err(),
        "smart playlist mutations must not publish source presentation"
    );
}

#[test]
pub(in crate::controller) fn lyrics_emit_event() {
    let root = self::unique_test_dir("local-track-without-sidecar");
    fs::create_dir_all(&root).expect("create local root");
    let media_path = root.join("Track.flac");
    fs::write(&media_path, []).expect("create local media file");
    let saved = local_source_saved();
    let mut track = restored_track();
    track.id = TrackId::new("local:track:without-sidecar");
    track.album_id = AlbumId::new("local:album:without-sidecar");
    track.local_path = Some(media_path.to_string_lossy().into_owned());
    let (owners, events) = controller_with_current_track(&saved, &track);

    owners.lyrics.request_lyrics_for_media(
        media_key(&saved.source_id, &track.id),
        metadata::LyricsRequestKind::ServerOnly,
    );

    assert!(wait_for_lyrics(&events).is_none());
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn server_only_ignores_cached_remote_and_calls_native() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind lyrics server");
    let address = listener.local_addr().expect("lyrics server address");
    let (request_sender, request_receiver) = channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept lyrics request");
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).expect("read lyrics request");
        request_sender
            .send(String::from_utf8_lossy(&buffer[..read]).into_owned())
            .expect("record lyrics request");
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("write lyrics response");
    });
    let mut config = sources::jellyfin::JellyfinSourceConfig::from_stored(&saved_source())
        .expect("Jellyfin source config");
    config.credentials.source.base_url = format!("http://{address}");
    let saved = config.into_stored();
    let track = restored_track();
    let source_id = saved.source_id.clone();
    let (owners, events) = controller_with_current_track(&saved, &track);
    let remote_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        external_provider: None,
        lines: vec![LyricLine {
            text: "cached remote line".to_string(),
            start_millis: None,
        }],
    };
    save_cached_lyrics(&owners.lyrics.store, &source_id, &remote_lyrics)
        .expect("save remote lyrics");
    owners
        .source
        .secrets
        .save_token(source_id.as_str(), "lyrics-token")
        .expect("save lyrics token");

    owners.lyrics.request_lyrics_for_media(
        media_key(&source_id, &track.id),
        metadata::LyricsRequestKind::ServerOnly,
    );

    assert!(wait_for_lyrics(&events).is_none());
    let request = request_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("native lyrics request");
    assert!(request.starts_with("GET /Audio/lyrics/Lyrics HTTP/1.1"));
    assert_eq!(
        load_cached_lyrics(&owners.lyrics.store, &source_id, &track.id)
            .expect("load ignored cache"),
        Some(remote_lyrics)
    );
    server.join().expect("lyrics server");
}
#[test]
pub(in crate::controller) fn lyrics_remove_cache() {
    let saved = saved_source();
    let track = restored_track();
    let source_id = saved.source_id.clone();
    let (owners, events) = controller_with_current_track(&saved, &track);
    let remote_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        external_provider: None,
        lines: vec![LyricLine {
            text: "cached remote line".to_string(),
            start_millis: None,
        }],
    };
    save_cached_lyrics(&owners.lyrics.store, &source_id, &remote_lyrics)
        .expect("save remote lyrics");

    owners.lyrics.request_lyrics_for_media(
        media_key(&source_id, &track.id),
        metadata::LyricsRequestKind::Configured,
    );
    assert_eq!(wait_for_lyrics(&events), Some(remote_lyrics));
    owners.lyrics.clear_remote_lyrics_for_current();

    assert!(wait_for_lyrics(&events).is_none());
    assert_eq!(
        load_cached_lyrics(&owners.lyrics.store, &source_id, &track.id).expect("load lyrics"),
        None
    );
}
#[test]
pub(in crate::controller) fn lyrics_auto_uses_cached_remote() {
    let saved = saved_source();
    let track = restored_track();
    let source_id = saved.source_id.clone();
    let (owners, events) = controller_with_current_track(&saved, &track);
    let remote_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        external_provider: None,
        lines: vec![LyricLine {
            text: "cached remote line".to_string(),
            start_millis: None,
        }],
    };
    save_cached_lyrics(&owners.lyrics.store, &source_id, &remote_lyrics)
        .expect("save remote lyrics");

    owners.lyrics.request_lyrics_for_media(
        media_key(&source_id, &track.id),
        metadata::LyricsRequestKind::Configured,
    );

    assert_eq!(wait_for_lyrics(&events), Some(remote_lyrics));
}
#[test]
pub(in crate::controller) fn invalid_cached_netease_placeholder_is_deleted() {
    let saved = local_source_saved();
    let mut track = restored_track();
    track.id = TrackId::new("local:track:invalid-cached-lyrics");
    track.album_id = AlbumId::new("local:album:invalid-cached-lyrics");
    track.title.clear();
    track.artist.clear();
    track.artist_id = None;
    let source_id = saved.source_id.clone();
    let (owners, events) = controller_with_current_track(&saved, &track);
    let mut settings = owners.settings.load_settings();
    settings.ui.metadata.external_lyrics_enabled = true;
    settings.ui.metadata.external_lyrics_providers = vec![ExternalLyricsProvider::Netease];
    owners
        .settings
        .save_settings(&settings)
        .expect("save Netease cache policy");
    let remote_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        external_provider: Some(ExternalLyricsProvider::Netease),
        lines: vec![
            LyricLine {
                text: "作曲 : Example Composer".to_string(),
                start_millis: Some(0),
            },
            LyricLine {
                text: "Sorry，此歌曲暂无文本歌词。".to_string(),
                start_millis: Some(5_000),
            },
        ],
    };
    save_cached_lyrics(&owners.lyrics.store, &source_id, &remote_lyrics)
        .expect("seed invalid cached lyrics");

    owners.lyrics.request_lyrics_for_media(
        media_key(&source_id, &track.id),
        metadata::LyricsRequestKind::Configured,
    );

    assert!(wait_for_lyrics(&events).is_none());
    assert_eq!(
        load_cached_lyrics(&owners.lyrics.store, &source_id, &track.id).expect("load lyrics"),
        None
    );
}
#[test]
pub(in crate::controller) fn lyrics_preserve_cache() {
    let saved = saved_source();
    let track = restored_track();
    let source_id = saved.source_id.clone();
    let (owners, events) = controller_with_current_track(&saved, &track);
    let server_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Server,
        external_provider: None,
        lines: vec![LyricLine {
            text: "server line".to_string(),
            start_millis: None,
        }],
    };
    save_cached_lyrics(&owners.lyrics.store, &source_id, &server_lyrics)
        .expect("save server lyrics");

    owners.lyrics.clear_remote_lyrics_for_current();
    owners.lyrics.request_lyrics_for_media(
        media_key(&source_id, &track.id),
        metadata::LyricsRequestKind::Configured,
    );

    assert_eq!(wait_for_lyrics(&events), Some(server_lyrics));
}
#[test]
pub(in crate::controller) fn lyrics_emit_current() {
    let store = StoreHandle::open_memory().expect("memory store");
    let mut saved = saved_source();
    saved.source_id = SourceId::new("jellyfin:server:lyrics");
    saved.name = "Lyrics Server".to_string();
    let track = restored_track();
    let lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Server,
        external_provider: None,
        lines: vec![LyricLine {
            text: "first line".to_string(),
            start_millis: Some(1_000),
        }],
    };
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)?;
            Ok(())
        })
        .expect("seed restored state");
    save_cached_lyrics(&store, &saved.source_id, &lyrics).expect("seed restored lyrics");
    seed_playback_checkpoint(&store, &saved.source_id, &track, 12_000);
    let (owners, events) = owners_from_store_for_test(store);
    owners.lyrics.request_lyrics_for_media(
        media_key(&saved.source_id, &track.id),
        metadata::LyricsRequestKind::Configured,
    );
    assert_eq!(wait_for_lyrics(&events), Some(lyrics));
}
#[test]
pub(in crate::controller) fn lyrics_skip_stale_track_request() {
    let store = StoreHandle::open_memory().expect("memory store");
    let mut saved = saved_source();
    saved.source_id = SourceId::new("jellyfin:server:lyrics");
    saved.name = "Lyrics Server".to_string();
    let track = restored_track();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)?;
            Ok(())
        })
        .expect("seed restored state");
    seed_playback_checkpoint(&store, &saved.source_id, &track, 0);
    let (owners, events) = owners_from_store_for_test(store);

    owners.lyrics.request_lyrics_for_media(
        media_key(
            &saved.source_id,
            &TrackId::new("jellyfin:track:stale-lyrics"),
        ),
        metadata::LyricsRequestKind::Configured,
    );

    let _error = recv_typed_event_timeout(
        &events.metadata_lyrics,
        std::time::Duration::from_millis(100),
    )
    .expect_err("lyrics event should not be emitted");
}

#[test]
pub(in crate::controller) fn lyrics_preview_rejects_a_disabled_provider() {
    let saved = saved_source();
    let track = restored_track();
    let (owners, events) = controller_with_current_track(&saved, &track);
    let mut settings = owners.settings.load_settings();
    settings.ui.metadata.external_lyrics_enabled = true;
    settings.ui.metadata.external_lyrics_providers = vec![ExternalLyricsProvider::Genius];
    owners
        .settings
        .save_settings(&settings)
        .expect("save lyrics settings");

    owners.lyrics.preview_lyrics_search_result(
        media_key(&saved.source_id, &track.id),
        LyricsSearchResult {
            provider: ExternalLyricsProvider::Lrclib,
            id: "disabled-result".to_string(),
            track_name: track.title.clone(),
            artist_name: track.artist.clone(),
            album_name: String::new(),
            duration_seconds: 0,
            synced_lyrics: None,
            plain_lyrics: Some("should not load".to_string()),
        },
    );

    recv_typed_event_timeout(&events.metadata_lyrics, Duration::from_millis(100))
        .expect_err("disabled provider result should not be applied");
    assert_eq!(
        load_cached_lyrics(&owners.lyrics.store, &saved.source_id, &track.id)
            .expect("load lyrics cache"),
        None
    );
}

#[test]
pub(in crate::controller) fn lyrics_settings_invalidate_a_queued_result() {
    let saved = saved_source();
    let track = restored_track();
    let (owners, events) = controller_with_current_track(&saved, &track);
    let lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        external_provider: Some(ExternalLyricsProvider::Lrclib),
        lines: vec![LyricLine {
            text: "queued line".to_string(),
            start_millis: None,
        }],
    };
    save_cached_lyrics(&owners.lyrics.store, &saved.source_id, &lyrics)
        .expect("save cached lyrics");

    owners.lyrics.request_lyrics_for_media(
        media_key(&saved.source_id, &track.id),
        metadata::LyricsRequestKind::Configured,
    );
    let generation = loop {
        match wait_for_typed_event(
            &events.metadata_lyrics,
            Duration::from_secs(5),
            "lyrics event",
        ) {
            metadata::LyricsEvent::Loaded { generation, .. } => break generation,
            _ => {}
        }
    };
    assert!(owners.lyrics.lyrics_result_is_current(generation));

    let mut settings = owners.settings.load_settings();
    settings.ui.metadata.external_lyrics_providers = vec![ExternalLyricsProvider::Genius];
    owners
        .settings
        .save_settings(&settings)
        .expect("disable lyrics provider");

    assert!(!owners.lyrics.lyrics_result_is_current(generation));
}

#[test]
pub(in crate::controller) fn playback_skips_uncached_prefix_access() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_source();
    let root = self::unique_test_dir("local-playback-stream");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = begin_sync_with_access(
        &store,
        &saved,
        &SourceLocalAccess {
            source_id: saved.source_id.clone(),
            root_path: root.to_string_lossy().into_owned(),
            path_replace_from: Some("/server/music".to_string()),
            path_replace_to: Some(root.to_string_lossy().into_owned()),
        },
    );
    let mut track = restored_track();
    track.local_path = Some("/server/music/Album/Track.flac".to_string());
    store
        .with_store(|store| store.upsert_tracks(&saved.source_id, &[track.clone()], generation))
        .expect("upsert track");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets = Arc::new(MemorySecretStore::new());
    secrets
        .save_token(saved.source_id.as_str(), "test-token")
        .expect("save token");
    let secrets: Arc<dyn SecretStore> = secrets;
    let active_source = active_source_for_test(&store, &secrets, &saved);
    let stream = super::resolve_stream_request(
        &store,
        &runtime,
        &active_source,
        &saved.source_id,
        &StreamRequest::original(track.id.clone()),
    )
    .expect("stream");
    assert!(stream.uri().starts_with("https://music.example/Audio/"));
    assert!(stream.uri().contains("/stream?"));
    assert!(stream.uri().contains("Static=true"));
    assert!(stream.uri().contains("api_key=test-token"));
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn local_playback_uses_cached_file() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let root = self::unique_test_dir("local-source-stream");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = begin_active_sync(&store, &saved);
    let mut track = restored_track();
    track.id = TrackId::new("local:track:stream");
    track.local_path = Some(audio.to_string_lossy().into_owned());
    store
        .with_store(|store| store.upsert_tracks(&saved.source_id, &[track.clone()], generation))
        .expect("upsert track");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let active_source = active_source_for_test(&store, &secrets, &saved);

    let stream = super::resolve_stream_request(
        &store,
        &runtime,
        &active_source,
        &saved.source_id,
        &StreamRequest::original(track.id.clone()),
    )
    .expect("stream");

    assert!(stream.uri().starts_with("file://"));
    assert!(stream.uri().contains("Track.flac"));
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn local_stream_resolution_rejects_stale_cached_path() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let root = self::unique_test_dir("local-source-stale-stream");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = begin_active_sync(&store, &saved);
    let mut track = restored_track();
    track.id = TrackId::new("local:track:stale-stream");
    track.local_path = Some(audio.to_string_lossy().into_owned());
    store
        .with_store(|store| store.upsert_tracks(&saved.source_id, &[track.clone()], generation))
        .expect("upsert track");
    fs::remove_file(&audio).expect("remove audio");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let active_source = active_source_for_test(&store, &secrets, &saved);

    let error = super::resolve_stream_request(
        &store,
        &runtime,
        &active_source,
        &saved.source_id,
        &StreamRequest::original(track.id.clone()),
    )
    .expect_err("stale cached path");

    assert!(error.contains("Cached local source is missing"));
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn local_stream_resolution_does_not_scan_on_cache_miss() {
    let store = StoreHandle::open_memory().expect("memory store");
    let root = self::unique_test_dir("local-stream-no-rescan");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let identity = SourceIdentity {
        id: SourceId::new("local:server:no-rescan"),
        kind: LOCAL_SOURCE_ID.to_string(),
        name: "Local".to_string(),
        base_url: root.to_string_lossy().into_owned(),
    };
    let saved = sources::local::LocalSourceConfig {
        source: identity.clone(),
    }
    .into_stored();
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let provider = LocalSource::from_roots_with_identity(vec![root.clone()], identity)
        .expect("local provider");
    let mut track = runtime
        .block_on(provider.tracks(sources::PagedRequest::new(0, 1)))
        .expect("tracks")
        .items
        .into_iter()
        .next()
        .expect("track");
    track.local_path = None;
    let generation = begin_active_sync(&store, &saved);
    store
        .with_store(|store| store.upsert_tracks(&saved.source_id, &[track.clone()], generation))
        .expect("upsert track");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let active_source = active_source_for_test(&store, &secrets, &saved);

    let error = super::resolve_stream_request(
        &store,
        &runtime,
        &active_source,
        &saved.source_id,
        &StreamRequest::original(track.id.clone()),
    )
    .expect_err("missing cached path");

    assert!(error.contains("Cached local source is missing"));
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn remote_playback_uses_cached_local_match() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_source();
    let root = self::unique_test_dir("cached-local-match-stream");
    let audio = root.join("Album/Track.flac");
    fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
    fs::write(&audio, []).expect("audio");
    let generation = begin_sync_with_access(
        &store,
        &saved,
        &SourceLocalAccess {
            source_id: saved.source_id.clone(),
            root_path: root.to_string_lossy().into_owned(),
            path_replace_from: None,
            path_replace_to: Some(root.to_string_lossy().into_owned()),
        },
    );
    let track = restored_track();
    store
        .with_store(|store| {
            commit_cached_library(
                store,
                &saved.source_id,
                generation,
                CachedLibraryObservation {
                    tracks: vec![track.clone()],
                    local_matches: vec![(
                        track.id.clone(),
                        audio.to_string_lossy().into_owned(),
                        "metadata".to_string(),
                    )],
                    ..CachedLibraryObservation::default()
                },
            )
            .map(|_| ())
        })
        .expect("seed track");
    let runtime = Arc::new(Runtime::new().expect("runtime"));
    let secrets = Arc::new(MemorySecretStore::new());
    secrets
        .save_token(saved.source_id.as_str(), "test-token")
        .expect("save token");
    let secrets: Arc<dyn SecretStore> = secrets;
    let active_source = active_source_for_test(&store, &secrets, &saved);
    let stream = super::resolve_stream_request(
        &store,
        &runtime,
        &active_source,
        &saved.source_id,
        &StreamRequest::original(track.id.clone()),
    )
    .expect("stream");
    assert!(stream.uri().starts_with("file://"));
    assert!(stream.uri().contains("Track.flac"));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn lyrics_local_access_status() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_source();
    let root = self::unique_test_dir("local-access-status");
    let local_prefix = root.join("mapped");
    let direct_audio = root.join("Direct.flac");
    let prefix_audio = local_prefix.join("Album/Mapped.flac");
    let metadata_audio = root.join("Metadata.flac");
    fs::create_dir_all(prefix_audio.parent().expect("parent")).expect("create mapped dir");
    fs::write(&direct_audio, []).expect("direct audio");
    fs::write(&prefix_audio, []).expect("prefix audio");
    fs::write(&metadata_audio, []).expect("metadata audio");
    let generation = begin_sync_with_access(
        &store,
        &saved,
        &SourceLocalAccess {
            source_id: saved.source_id.clone(),
            root_path: root.to_string_lossy().into_owned(),
            path_replace_from: Some("/server/music".to_string()),
            path_replace_to: Some(local_prefix.to_string_lossy().into_owned()),
        },
    );
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
            commit_cached_library(
                store,
                &saved.source_id,
                generation,
                CachedLibraryObservation {
                    tracks: vec![direct, prefix, metadata.clone(), unmatched],
                    local_matches: vec![(
                        metadata.id.clone(),
                        metadata_audio.to_string_lossy().into_owned(),
                        "metadata".to_string(),
                    )],
                    ..CachedLibraryObservation::default()
                },
            )
            .map(|_| ())
        })
        .expect("seed tracks");
    let snapshot = super::load_source_presentation(&store).expect("load snapshot");
    assert_eq!(snapshot.local_access_status.total_track_count, 4);
    assert_eq!(snapshot.local_access_status.direct_match_count, 1);
    assert_eq!(snapshot.local_access_status.prefix_match_count, 2);
    assert_eq!(snapshot.local_access_status.metadata_match_count, 1);
    assert_eq!(snapshot.local_access_status.unmatched_count, 0);
    assert!(snapshot.local_access_status.sample_source_path.is_some());
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn lyrics_include_access() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_source();
    let access = SourceLocalAccess {
        source_id: saved.source_id.clone(),
        root_path: "/home/demo/Music".to_string(),
        path_replace_from: Some("/server/music".to_string()),
        path_replace_to: Some("/home/demo/Music".to_string()),
    };
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.save_source_local_access(&access)?;
            store.set_active_source(&saved.source_id)
        })
        .expect("save server");
    let snapshot = super::load_source_presentation(&store).expect("load snapshot");
    assert_eq!(snapshot.local_access, Some(access));
}

#[test]
pub(in crate::controller) fn local_access_commands_emit_only_local_access_projection() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = self::saved_source();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)
        })
        .expect("save server");
    let (owners, events) = owners_from_store_for_test(store);

    owners.source.save_source_local_access(
        saved.source_id.clone(),
        PathBuf::from("/home/demo/Music"),
        Some("/server/music".to_string()),
        Some("/home/demo/Music".to_string()),
    );
    let saved_access = wait_for_source_local_access(&events);
    assert_eq!(saved_access.source_id, saved.source_id);
    assert_eq!(
        saved_access
            .access
            .as_ref()
            .map(|access| access.root_path.as_str()),
        Some("/home/demo/Music")
    );
    assert!(events.source_presentation.try_recv().is_err());

    owners
        .source
        .clear_source_local_access(saved.source_id.clone());
    let cleared_access = wait_for_source_local_access(&events);
    assert_eq!(cleared_access.source_id, saved.source_id);
    assert!(cleared_access.access.is_none());
    assert!(events.source_presentation.try_recv().is_err());
}

#[test]
pub(in crate::controller) fn product_events_are_sendable() {
    pub(in crate::controller) fn assert_send<T: Send>() {}
    assert_send::<SourcePresentationState>();
    assert_send::<sources::SourceLocalAccessPresentation>();
    assert_send::<sources::SourceSelectionChanged>();
    assert_send::<sources::ServerDiscoveryUpdate>();
    assert_send::<sources::SourceNotice>();
    assert_send::<sources::SourceTransitionFailed>();
    assert_send::<library_sync::LibrarySyncEvent>();
    assert_send::<library::LibraryEvent>();
    assert_send::<playback::PlaybackProjection>();
    assert_send::<playback::WaveformProjection>();
    assert_send::<metadata::LyricsEvent>();
    assert_send::<artwork::ArtworkEvent>();
}
