use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use library::{
    AcceptedPlay, AcceptedSkip, CandidateBatch, CandidateFinish, CandidateHeader, HomeFacts,
    HomeSectionKind, MetadataChange, MetadataEdit, MetadataItemId, MusicFolder, MusicFolderId,
    PlaybackLoad, Track, TrackData, TrackRelations, TrackSort,
};
use secrets::{MemorySecretStore, SecretStorageMode, SwitchableSecretStore};
use sources::{LocalFilesystemChange, LocalFolderHostInput, SourceConfiguration, SourceSetupInput};

use super::*;

#[test]
fn playback_activity_does_not_rebuild_download_policy() {
    let change = AcceptedLibraryChange {
        tracks: vec![library::AcceptedTrackReplacement {
            id: library::TrackId::new("track"),
            track: None,
        }],
        ..AcceptedLibraryChange::default()
    };
    let rules = ui::DownloadRules {
        entire_library: true,
        ..ui::DownloadRules::default()
    };

    assert!(should_reconcile_downloads(&change, &NextHome::Keep, rules));
    assert!(!should_reconcile_downloads(
        &change,
        &NextHome::ActivityKeep,
        rules
    ));
    assert!(!should_reconcile_downloads(
        &change,
        &NextHome::AcceptedPlay(library::TrackId::new("track")),
        rules
    ));
}

#[test]
fn upgraded_navidrome_reader_selects_the_generic_cache_before_refreshing() {
    let directory = tempfile::tempdir().expect("temporary Navidrome cache");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let source_id = SourceId::new("navidrome:server:reader-upgrade");
    let configuration = |library_version| SourceConfiguration {
        source_id: source_id.clone(),
        kind: "navidrome".to_string(),
        name: "Navidrome".to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "base_url": "http://127.0.0.1:9",
            "username": "Listener",
            "trust_invalid_cert": false,
            "navidrome_library_version": library_version,
        })
        .to_string(),
    };
    let generic_configuration = configuration(0);
    let full_configuration = configuration(1);
    let generic = generic_configuration
        .input_identity()
        .expect("generic Navidrome identity");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let accepted = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: generic.version,
            input_digest: generic.digest,
        })
        .and_then(|candidate| {
            candidate.finish(
                CandidateFinish {
                    freshness: None,
                    home: HomeFacts::Source {
                        sections: Vec::new(),
                    },
                    accepted_at: 1,
                },
                None,
            )
        })
        .and_then(|candidate| candidate.accept())
        .expect("accept generic Navidrome cache");
    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open test Settings");
    let credential_ref = CredentialRef::new("navidrome-reader-upgrade");
    let configured = ConfiguredSource {
        configuration: full_configuration,
        credential_ref: Some(credential_ref.clone()),
        music_folder_id: None,
        local_access: None,
    };
    settings
        .update(|stored| {
            stored.sources.configured = vec![configured.clone()];
            Ok(())
        })
        .expect("save inactive Navidrome");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, _event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    save_provider_secret(
        &secrets,
        &credential_ref,
        serde_json::json!({
            "version": 1,
            "salt": "salt",
            "token": "token",
            "navidrome_password": "password"
        })
        .to_string(),
    )
    .expect("save Navidrome password");
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library.clone(),
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        scrobbler,
        runtime.handle().clone(),
        SourceOutputs { events, discovery },
    );
    let prepared = runtime
        .block_on(prepare_select(
            Arc::clone(&bootstrap.owner.shared),
            configured,
            Arc::new(|_| {}),
            Arc::new(AtomicBool::new(false)),
        ))
        .expect("prepare cached Navidrome selection");

    let PreparedReplacement {
        reason:
            ReplacementReason::Select {
                cached: true,
                refresh_after_select: true,
            },
        source: Some(_),
        library: ReplacementLibrary::Cached(cached),
        ..
    } = prepared
    else {
        panic!("the reader upgrade must select its cache and schedule a full refresh");
    };
    assert_eq!(cached.library_id(), accepted.loaded.library_id());
}

#[test]
fn selected_local_lifecycle_keeps_cached_state_and_finishes_user_work_before_switch() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let music_root = directory.path().join("music");
    std::fs::create_dir(&music_root).expect("create Local music folder");
    let source_id = SourceId::new("local:server:cached-folder");
    let track_id = library::TrackId::new("local:track:cached");
    let configuration = SourceConfiguration {
        source_id: source_id.clone(),
        kind: "local".to_string(),
        name: "Local".to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "roots": [music_root],
        })
        .to_string(),
    };
    let identity = configuration.input_identity().expect("source identity");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let available_folder = MusicFolderId::new("folder:available");
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: identity.version,
            input_digest: identity.digest,
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(vec![Track::new(TrackData {
            id: track_id.clone(),
            album_id: None,
            title: "Cached Track".to_string(),
            artist: "Artist".to_string(),
            album: String::new(),
            album_artwork: None,
            year: 2024,
            release_date: None,
            date_added: Some("2024-01-01".to_string()),
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            local_artwork: None,
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            source_path: Some(
                music_root
                    .join("Cached.flac")
                    .to_string_lossy()
                    .into_owned(),
            ),
            cue: None,
            source_format: Some("flac".to_string()),
            comment: None,
            skip_count: None,
            bpm: None,
            relations: TrackRelations {
                music_folders: vec![available_folder.clone()],
                ..TrackRelations::default()
            },
        })]))
        .expect("write cached Track");
    candidate
        .write(CandidateBatch::MusicFolders(vec![MusicFolder {
            id: available_folder,
            name: "Available".to_string(),
            image_ref: None,
        }]))
        .expect("write available music folder");
    let accepted = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept cached source");
    let activity = library
        .record_play(
            &accepted.loaded,
            AcceptedPlay {
                play_id: "cached-play".to_string(),
                track_id: track_id.clone(),
                played_at: 1_700_000_000,
                month: "2023-11".to_string(),
            },
        )
        .expect("record cached play")
        .expect("new cached play");
    library
        .apply_recorded_activity(&accepted.loaded, &activity)
        .expect("apply cached play");
    let cached_track = accepted
        .loaded
        .track(&track_id)
        .expect("read cached Track")
        .expect("cached Track");
    let mut queue = playback::Sequence::new(source_id.clone());
    queue
        .apply_batch(
            playback::Batch::new(vec![playback::BatchItem::new(
                cached_track,
                playback::Provenance::Manual,
            )]),
            playback::Placement::End,
        )
        .expect("prepare cached queue");
    let mut checkpoint = playback::build_checkpoint(&queue);
    checkpoint.state.selected = None;
    library
        .replace_playback(checkpoint)
        .expect("save cached queue");

    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration,
                credential_ref: None,
                music_folder_id: Some(MusicFolderId::new("folder:removed")),
                local_access: None,
            }];
            stored.sources.selected_source_id = Some(source_id.clone());
            stored.ui.secret_storage_mode = SecretStorageMode::SystemKeyring;
            Ok(())
        })
        .expect("save selected source");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library.clone(),
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        Arc::clone(&scrobbler),
        runtime.handle().clone(),
        SourceOutputs {
            events: events.clone(),
            discovery,
        },
    );

    assert_eq!(
        bootstrap.operation,
        SourceOperation::Switching {
            target: source_id.clone(),
            progress: initial_progress(),
        }
    );
    assert_eq!(
        bootstrap.configured.selected_source_id.as_ref(),
        Some(&source_id)
    );
    assert!(bootstrap.owner.shared.selected().is_none());

    let _playback = attach_test_playback(
        &bootstrap,
        library,
        settings.clone(),
        runtime.handle().clone(),
        events,
        scrobbler,
        directory.path(),
    );
    bootstrap.owner.start().expect("start source owner");
    let (operations, selected, playback) = runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                let mut operations = Vec::new();
                let mut selected = None;
                let mut playback = None;
                loop {
                    match event_receiver.recv().await.expect("source startup event") {
                        SourceEvent::Operation(operation) => {
                            let idle = operation == SourceOperation::Idle;
                            operations.push(operation);
                            if idle && selected.is_some() {
                                break;
                            }
                        }
                        SourceEvent::Selected {
                            selected: next,
                            playback: next_playback,
                            ..
                        } => {
                            selected = Some(next);
                            playback = Some(next_playback);
                        }
                        _ => {}
                    }
                }
                (operations, selected.unwrap(), playback.unwrap())
            })
            .await
        })
        .expect("cached source startup completes");

    assert_eq!(
        operations,
        [
            SourceOperation::Switching {
                target: source_id.clone(),
                progress: initial_progress(),
            },
            SourceOperation::Idle,
        ]
    );
    assert_eq!(selected.music_folder_id, None);
    assert_eq!(
        selected
            .loaded
            .track_list(None, TrackSort::Title, false)
            .expect("read all cached Tracks")
            .len(),
        1
    );
    assert_eq!(
        selected
            .home
            .section(HomeSectionKind::MostPlayed)
            .expect("cached Home section")
            .items
            .len(),
        1
    );
    assert_eq!(playback.view.queue.total, 1);
    assert_eq!(playback.queue_page.as_ref().map(|page| page.total), Some(1));
    assert_eq!(settings.load().sources.configured[0].music_folder_id, None);

    runtime
        .block_on(
            bootstrap
                .owner
                .change_secret_storage(SecretStorageMode::ConfigFile)
                .recv(),
        )
        .expect("secret-storage response")
        .expect("change secret storage");
    assert_eq!(
        bootstrap
            .owner
            .shared
            .selected()
            .expect("Local source remains installed")
            .source_id(),
        &source_id
    );
    assert_eq!(
        settings.load().sources.selected_source_id.as_ref(),
        Some(&source_id)
    );
    while let Ok(event) = event_receiver.try_recv() {
        assert!(
            !matches!(
                event,
                SourceEvent::Operation(SourceOperation::Switching { .. })
                    | SourceEvent::ReleaseSelected { .. }
            ),
            "Local secret-storage change must not publish a source transition"
        );
    }

    bootstrap.owner.configure_source(SourceSetup::Local {
        roots: vec![directory.path().join("missing-music-root")],
    });
    let add_events = runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                let mut events = Vec::new();
                loop {
                    let event = event_receiver.recv().await.expect("failed add event");
                    if let SourceEvent::ReleaseSelected { acknowledged } = &event {
                        acknowledged
                            .try_send(())
                            .expect("acknowledge selected-source release");
                    }
                    let finished = matches!(
                        event,
                        SourceEvent::Operation(SourceOperation::Failed { add_form: true, .. })
                    );
                    events.push(event);
                    if finished {
                        return events;
                    }
                }
            })
            .await
        })
        .expect("failed add completes");
    assert!(
        add_events
            .iter()
            .any(|event| matches!(event, SourceEvent::ReleaseSelected { .. })),
        "a source add must stop and release the previous source before acquisition"
    );
    let adding = add_events
        .iter()
        .position(|event| {
            matches!(
                event,
                SourceEvent::Operation(SourceOperation::Adding { .. })
            )
        })
        .expect("source add announces its full-page operation");
    let release = add_events
        .iter()
        .position(|event| matches!(event, SourceEvent::ReleaseSelected { .. }))
        .expect("source add releases the selected source");
    assert!(adding < release);
    assert_eq!(
        bootstrap
            .owner
            .shared
            .selected()
            .expect("selected Local source after failed add")
            .source_id(),
        &source_id
    );

    let failed_remote_id = SourceId::new("jellyfin:server:missing-credential");
    settings
        .update(|stored| {
            stored.sources.configured.push(ConfiguredSource {
                configuration: SourceConfiguration {
                    source_id: failed_remote_id.clone(),
                    kind: "jellyfin".to_string(),
                    name: "Unavailable Jellyfin".to_string(),
                    provider_payload: "{".to_string(),
                },
                credential_ref: None,
                music_folder_id: None,
                local_access: None,
            });
            Ok(())
        })
        .expect("save unavailable remote source");
    bootstrap
        .owner
        .set_favorite(FavoriteItemId::Track(track_id.clone()), true);
    bootstrap.owner.edit_playlist(PlaylistEdit::Create {
        name: "Before Switch".to_string(),
        track_ids: vec![track_id.clone()],
    });
    bootstrap.owner.select_source(failed_remote_id.clone());

    let switch_events = runtime.block_on(async {
        let mut events = Vec::new();
        let mut event_names = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(10));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                event = event_receiver.recv() => {
                    let event = event.expect("failed switch event");
                    if let SourceEvent::ReleaseSelected { acknowledged } = &event {
                        acknowledged
                            .try_send(())
                            .expect("acknowledge selected-source release");
                    }
                    event_names.push(match &event {
                        SourceEvent::Operation(SourceOperation::Switching { .. }) => "switching",
                        SourceEvent::Operation(SourceOperation::Failed { .. }) => "failed",
                        SourceEvent::Operation(_) => "operation",
                        SourceEvent::LibraryUpdate(_) => "library update",
                        SourceEvent::ReleaseSelected { .. } => "release",
                        SourceEvent::Selected { .. } => "selected",
                        _ => "other",
                    });
                    let finished = matches!(
                        &event,
                        SourceEvent::Operation(SourceOperation::Failed {
                            source_id: Some(failed),
                            add_form: false,
                            ..
                        }) if failed == &failed_remote_id
                    );
                    events.push(event);
                    if finished {
                        break events;
                    }
                }
                _ = &mut deadline => {
                    panic!("failed switch did not finish; events: {event_names:?}");
                }
            }
        }
    });
    let release = switch_events
        .iter()
        .position(|event| matches!(event, SourceEvent::ReleaseSelected { .. }))
        .expect("failed switch releases the previous runtime before restoring it");
    assert_eq!(
        switch_events[..release]
            .iter()
            .filter(|event| matches!(event, SourceEvent::LibraryUpdate(_)))
            .count(),
        2,
        "favorite and playlist work must publish before the source is released"
    );
    let restored = bootstrap
        .owner
        .shared
        .selected()
        .expect("failed switch restores selected Local source");
    assert_eq!(restored.source_id(), &source_id);
    assert!(
        restored
            .loaded
            .track(&track_id)
            .expect("read favorited Track")
            .expect("favorited Track")
            .favorite
    );
    assert!(
        restored
            .loaded
            .playlists()
            .expect("read restored playlists")
            .iter()
            .any(|playlist| playlist.playlist.name == "Before Switch")
    );
    assert_eq!(
        settings.load().sources.selected_source_id.as_ref(),
        Some(&source_id)
    );
}

#[test]
fn selected_same_account_update_keeps_playback_and_source_epoch() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let source_id = SourceId::new("jellyfin:server:reauth");
    let track_id = library::TrackId::new("jellyfin:track:reauth");
    let configuration = |base_url: &str| SourceConfiguration {
        source_id: source_id.clone(),
        kind: "jellyfin".to_string(),
        name: "Jellyfin".to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "base_url": base_url,
            "server_id": "stable-server",
            "user_id": "listener-id",
            "username": "listener",
            "trust_invalid_cert": false,
            "use_jellyfin_instant_mix": false,
        })
        .to_string(),
    };
    let original_configuration = configuration("https://old.invalid");
    let replacement_configuration = configuration("https://new.invalid");
    let identity = original_configuration
        .input_identity()
        .expect("source identity");
    assert_eq!(
        identity,
        replacement_configuration
            .input_identity()
            .expect("replacement source identity")
    );

    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: identity.version,
            input_digest: identity.digest,
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(vec![test_track(
            track_id.clone(),
            "Selected Track",
            directory.path().join("Selected.flac"),
        )]))
        .expect("write selected Track");
    let accepted = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::Source {
                    sections: Vec::new(),
                },
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept source");
    let track = accepted
        .loaded
        .track_list(None, TrackSort::Title, false)
        .expect("read selected Track")
        .track(0)
        .expect("resolve selected Track")
        .expect("selected Track");
    let mut sequence = playback::Sequence::new(source_id.clone());
    sequence
        .apply_batch(
            playback::Batch::new(vec![
                playback::BatchItem::new(track.clone(), playback::Provenance::Manual),
                playback::BatchItem::new(track, playback::Provenance::Radio),
            ]),
            playback::Placement::Replace { anchor_index: 1 },
        )
        .expect("prepare duplicate queue");
    sequence.set_repeat_mode(playback::RepeatMode::All);
    sequence.set_shuffle_seed(true, 7);
    sequence.set_progress_millis(42_000);
    let checkpoint = playback::build_checkpoint(&sequence);
    library
        .replace_playback(checkpoint.clone())
        .expect("save Playback checkpoint");

    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    let configured = ConfiguredSource {
        configuration: original_configuration.clone(),
        credential_ref: None,
        music_folder_id: None,
        local_access: None,
    };
    let mut before = configured.clone();
    before.configuration.source_id = SourceId::new("jellyfin:server:before");
    before.configuration.name = "Before".to_string();
    let mut after = configured.clone();
    after.configuration.source_id = SourceId::new("jellyfin:server:after");
    after.configuration.name = "After".to_string();
    settings
        .update(|stored| {
            stored.sources.configured = vec![before.clone(), configured.clone(), after.clone()];
            stored.sources.selected_source_id = Some(source_id.clone());
            stored.ui.repeat_mode = playback::RepeatMode::All;
            stored.ui.shuffle_enabled = true;
            Ok(())
        })
        .expect("save selected source");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library.clone(),
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        Arc::clone(&scrobbler),
        runtime.handle().clone(),
        SourceOutputs {
            events: events.clone(),
            discovery,
        },
    );
    let playback = attach_test_playback(
        &bootstrap,
        library.clone(),
        settings,
        runtime.handle().clone(),
        events,
        scrobbler,
        directory.path(),
    );
    let mut actor = actor_for_test(&bootstrap.owner);
    let original_source = Arc::new(
        Source::open(
            original_configuration.clone(),
            Some("old-token".to_string()),
            Some("test-device".to_string()),
        )
        .expect("open original source"),
    );
    let initial = runtime
        .block_on(actor.prepare_runtime(
            original_configuration,
            Some(original_source),
            Arc::clone(&accepted.loaded),
            None,
            None,
        ))
        .and_then(|selected| runtime.block_on(actor.install_runtime(selected)))
        .expect("install original source session");
    let initial_epoch = initial.0.snapshot().source_session_epoch;
    runtime.block_on(
        bootstrap
            .owner
            .shared
            .publish_selected(initial.0, initial.1),
    );
    let initial_event = runtime
        .block_on(event_receiver.recv())
        .expect("initial selected event");
    let SourceEvent::Selected {
        playback: initial_playback,
        ..
    } = initial_event
    else {
        panic!("initial source publication was not Selected");
    };
    assert_eq!(
        initial_playback.view.controls.repeat_mode,
        playback::RepeatMode::All
    );
    assert!(initial_playback.view.controls.shuffle_enabled);
    assert_eq!(initial_playback.view.transport.position_millis, 42_000);

    let replacement_source = Arc::new(
        Source::open(
            replacement_configuration.clone(),
            Some("new-token".to_string()),
            Some("test-device".to_string()),
        )
        .expect("open replacement source"),
    );
    let mut replacement_candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: identity.version,
            input_digest: identity.digest,
        })
        .expect("begin same-source candidate");
    replacement_candidate
        .write(CandidateBatch::Tracks(vec![test_track(
            track_id.clone(),
            "Updated Track",
            directory.path().join("Selected.flac"),
        )]))
        .expect("write updated Track");
    let replacement_candidate = replacement_candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::Source {
                    sections: Vec::new(),
                },
                accepted_at: 2,
            },
            Some(&accepted.loaded),
        )
        .expect("prepare same-source candidate");
    runtime
        .block_on(actor.commit_selected_update(
            PreparedSelectedUpdate {
                configured,
                configuration: replacement_configuration.clone(),
                source: replacement_source,
                credential: None,
                candidate: Some(Box::new(replacement_candidate)),
            },
            Vec::new(),
        ))
        .expect("update selected source connection");

    let update_events = std::iter::from_fn(|| event_receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(!update_events.iter().any(|event| matches!(
        event,
        SourceEvent::ReleaseSelected { .. } | SourceEvent::Selected { .. }
    )));
    let selected = actor
        .shared
        .selected()
        .expect("selected source remains installed");
    assert_eq!(selected.source_id(), &source_id);
    assert_eq!(selected.source_session_epoch, initial_epoch);
    assert_eq!(selected.configuration, replacement_configuration);

    let progress = SourceReadProgress {
        stage: SourceReadStage::Artwork,
        completed: 2,
        total: Some(3),
    };
    actor.active = Some(ActiveWork {
        token: 99,
        purpose: WorkPurpose::Update {
            selected: true,
            progress_source: None,
        },
        activity_updates: Vec::new(),
        cancelled: Arc::new(AtomicBool::new(false)),
        handle: runtime
            .spawn(async { Err::<PreparedWork, String>("unused progress test work".to_string()) }),
    });
    runtime.block_on(actor.publish_progress(99, progress));
    assert!(
        event_receiver.try_recv().is_err(),
        "a remote settings update must not publish foreground artwork progress"
    );

    actor.active.as_mut().expect("active update").purpose = WorkPurpose::Update {
        selected: true,
        progress_source: Some(source_id.clone()),
    };
    runtime.block_on(actor.publish_progress(99, progress));
    assert!(matches!(
        event_receiver.try_recv(),
        Ok(SourceEvent::Operation(SourceOperation::Refreshing {
            source_id: visible_source,
            progress: SourceProgress {
                stage: SourceProgressStage::Artwork,
                completed: 2,
                total: Some(3),
            },
        })) if visible_source == source_id
    ));

    actor.active.as_mut().expect("active update").purpose = WorkPurpose::Refresh {
        qualifier: selected.qualifier(),
        visible: true,
    };
    runtime.block_on(actor.publish_progress(99, progress));
    assert!(matches!(
        event_receiver.try_recv(),
        Ok(SourceEvent::Operation(SourceOperation::Refreshing {
            source_id: visible_source,
            progress: SourceProgress {
                stage: SourceProgressStage::Artwork,
                completed: 2,
                total: Some(3),
            },
        })) if visible_source == source_id
    ));
    if let Some(active) = actor.active.take() {
        active.handle.abort();
    }

    assert_eq!(
        selected
            .loaded
            .track(&track_id)
            .expect("read updated Track")
            .expect("updated Track")
            .title,
        "Updated Track"
    );
    let rebound = update_events
        .iter()
        .find_map(|event| {
            let SourceEvent::Playback { projection, .. } = event else {
                return None;
            };
            Some(projection)
        })
        .expect("same-source update refreshes queued Tracks");
    assert_eq!(
        rebound.view.queue.current_occurrence,
        initial_playback.view.queue.current_occurrence
    );
    assert_eq!(
        rebound.view.queue.current_index,
        initial_playback.view.queue.current_index
    );
    assert_eq!(rebound.view.queue.total, initial_playback.view.queue.total);
    let initial_page = initial_playback
        .queue_page
        .as_ref()
        .expect("initial queue page");
    let rebound_page = rebound.queue_page.as_ref().expect("refreshed queue page");
    assert_eq!(rebound_page.total, initial_page.total);
    assert_eq!(
        rebound_page
            .rows
            .iter()
            .map(|row| &row.entry.occurrence)
            .collect::<Vec<_>>(),
        initial_page
            .rows
            .iter()
            .map(|row| &row.entry.occurrence)
            .collect::<Vec<_>>()
    );
    assert!(
        rebound_page
            .rows
            .iter()
            .all(|row| row.entry.track.title == "Updated Track")
    );
    assert_eq!(rebound.view.controls.repeat_mode, playback::RepeatMode::All);
    assert!(rebound.view.controls.shuffle_enabled);
    assert_eq!(rebound.view.transport.position_millis, 42_000);
    playback
        .prepare_track_refresh(initial_epoch)
        .expect("Playback remains attached to the same source epoch");
    assert_eq!(
        actor
            .shared
            .settings
            .load()
            .sources
            .configured
            .iter()
            .map(|configured| configured.configuration.name.as_str())
            .collect::<Vec<_>>(),
        ["Before", "Jellyfin", "After"]
    );
    let replacement_id = SourceId::new("jellyfin:server:replacement");
    let mut replacement = actor.shared.settings.load().sources.configured[1].clone();
    replacement.configuration.source_id = replacement_id.clone();
    replacement.configuration.name = "Replacement".to_string();
    replace_source_account(&actor.shared.settings, &source_id, replacement, true)
        .expect("replace selected account in place");
    let replaced = actor.shared.settings.load().sources;
    assert_eq!(
        replaced
            .configured
            .iter()
            .map(|configured| configured.configuration.name.as_str())
            .collect::<Vec<_>>(),
        ["Before", "Replacement", "After"]
    );
    assert_eq!(replaced.selected_source_id.as_ref(), Some(&replacement_id));

    playback
        .stop_for_source_switch()
        .expect("stop rebound Playback");
    let PlaybackLoad::Ready(reopened) = library
        .load_playback(&source_id)
        .expect("reopen Playback checkpoint")
    else {
        panic!("rebound Playback checkpoint was not preserved");
    };
    assert!(reopened.revision > checkpoint.revision);
    assert_eq!(reopened.source_id, checkpoint.source_id);
    assert_eq!(reopened.queue.occurrences, checkpoint.queue.occurrences);
    assert_eq!(reopened.queue.traversal, checkpoint.queue.traversal);
    assert_eq!(reopened.state, checkpoint.state);
    assert!(
        reopened
            .queue
            .fallback_tracks
            .iter()
            .all(|track| track.title == "Updated Track")
    );
}

#[test]
fn source_transition_releases_the_previous_library_before_publishing_the_target() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let previous_root = directory.path().join("previous");
    let target_root = directory.path().join("target");
    std::fs::create_dir(&previous_root).expect("create previous Local folder");
    std::fs::create_dir(&target_root).expect("create target Local folder");
    let previous_id = SourceId::new("local:server:previous");
    let target_id = SourceId::new("local:server:target");
    let configuration = |source_id: SourceId, name: &str, root: &Path| SourceConfiguration {
        source_id,
        kind: "local".to_string(),
        name: name.to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "roots": [root],
        })
        .to_string(),
    };
    let previous_configuration = configuration(previous_id.clone(), "Previous", &previous_root);
    let target_configuration = configuration(target_id.clone(), "Target", &target_root);
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let accept_library = |configuration: &SourceConfiguration, title: &str, path: PathBuf| {
        let identity = configuration.input_identity().expect("source identity");
        let mut candidate = library
            .begin_source_candidate(CandidateHeader {
                source_id: configuration.source_id.clone(),
                input_version: identity.version,
                input_digest: identity.digest,
            })
            .expect("begin source candidate");
        candidate
            .write(CandidateBatch::Tracks(vec![test_track(
                library::TrackId::new(format!("{}:track", configuration.source_id)),
                title,
                path,
            )]))
            .expect("write source Track");
        candidate
            .finish(
                CandidateFinish {
                    freshness: None,
                    home: HomeFacts::RufinDefined,
                    accepted_at: 1,
                },
                None,
            )
            .and_then(|prepared| prepared.accept())
            .expect("accept source")
            .loaded
    };
    let previous_loaded = accept_library(
        &previous_configuration,
        "Previous Track",
        previous_root.join("Previous.flac"),
    );
    let target_loaded = accept_library(
        &target_configuration,
        "Target Track",
        target_root.join("Target.flac"),
    );

    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration: previous_configuration.clone(),
                credential_ref: None,
                music_folder_id: None,
                local_access: None,
            }];
            stored.sources.selected_source_id = Some(previous_id.clone());
            Ok(())
        })
        .expect("save previous source");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library.clone(),
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        Arc::clone(&scrobbler),
        runtime.handle().clone(),
        SourceOutputs {
            events: events.clone(),
            discovery,
        },
    );
    let playback = attach_test_playback(
        &bootstrap,
        library,
        settings.clone(),
        runtime.handle().clone(),
        events,
        scrobbler,
        directory.path(),
    );
    let mut actor = actor_for_test(&bootstrap.owner);
    let initial = runtime
        .block_on(actor.prepare_runtime(
            previous_configuration,
            None,
            Arc::clone(&previous_loaded),
            None,
            None,
        ))
        .and_then(|selected| runtime.block_on(actor.install_runtime(selected)))
        .expect("install previous source session");
    runtime.block_on(
        bootstrap
            .owner
            .shared
            .publish_selected(initial.0, initial.1),
    );
    let SourceEvent::Selected {
        playback: initial_playback,
        ..
    } = runtime
        .block_on(event_receiver.recv())
        .expect("previous selected event")
    else {
        panic!("previous source publication was not Selected");
    };
    assert!(!initial_playback.view.controls.auto_dj_enabled);
    assert!(!initial_playback.view.controls.shuffle_enabled);
    assert_eq!(
        initial_playback.view.controls.repeat_mode,
        playback::RepeatMode::Off
    );

    ::playback::TransportCommandPort::set_shuffle(playback.as_ref(), true);
    ::playback::TransportCommandPort::set_repeat(playback.as_ref(), playback::RepeatMode::All);
    ::playback::TransportCommandPort::toggle_auto_dj(playback.as_ref());
    let mut applied_modes = None;
    for _ in 0..8 {
        let event = runtime
            .block_on(event_receiver.recv())
            .expect("playback mode event");
        let SourceEvent::Playback { projection, .. } = event else {
            continue;
        };
        if projection.view.controls.auto_dj_enabled
            && projection.view.controls.shuffle_enabled
            && projection.view.controls.repeat_mode == playback::RepeatMode::All
        {
            applied_modes = Some(projection);
            break;
        }
    }
    assert!(
        applied_modes.is_some(),
        "app-wide Playback modes were not applied"
    );
    let stored_modes = SettingsFile::open(directory.path().join("settings.json"))
        .expect("reopen Playback settings")
        .load()
        .ui;
    assert!(stored_modes.auto_dj_enabled);
    assert!(stored_modes.shuffle_enabled);
    assert_eq!(stored_modes.repeat_mode, playback::RepeatMode::All);

    let previous_library = Arc::downgrade(&previous_loaded);
    drop(previous_loaded);
    runtime.block_on(async {
        {
            let transition = actor.begin_transition();
            tokio::pin!(transition);
            let acknowledged = loop {
                tokio::select! {
                    () = transition.as_mut() => {
                        panic!("source transition finished before requesting route release");
                    }
                    event = event_receiver.recv() => {
                        match event.expect("source transition event") {
                            SourceEvent::ReleaseSelected { acknowledged } => break acknowledged,
                            SourceEvent::Selected { .. } => {
                                panic!("target source was published before releasing the previous source");
                            }
                            _ => {}
                        }
                    }
                }
            };
            assert!(
                previous_library.upgrade().is_some(),
                "the selected source owns its Library until the route acknowledges release"
            );
            acknowledged
                .send(())
                .await
                .expect("acknowledge selected source release");
            transition.as_mut().await;
        }
        assert!(
            previous_library.upgrade().is_none(),
            "the source transition must release the previous Library before acquisition"
        );
        actor
            .commit_replacement(PreparedReplacement {
                reason: ReplacementReason::Add,
                previous: None,
                configuration: target_configuration,
                source: None,
                credential: None,
                library: ReplacementLibrary::Cached(target_loaded),
            })
            .await
            .expect("commit target source");
        loop {
            if let SourceEvent::Selected {
                selected,
                playback,
                ..
            } =
                event_receiver.recv().await.expect("target source event")
            {
                assert_eq!(selected.source_id, target_id);
                assert!(playback.view.controls.auto_dj_enabled);
                assert!(playback.view.controls.shuffle_enabled);
                assert_eq!(
                    playback.view.controls.repeat_mode,
                    ::playback::RepeatMode::All
                );
                break;
            }
        }
    });
    assert!(
        previous_library.upgrade().is_none(),
        "the completed transition must not retain the previous Library"
    );
}

#[test]
fn secret_storage_change_preserves_a_selected_remote_without_cached_facts() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let source_id = SourceId::new("jellyfin:server:configured");
    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration: SourceConfiguration {
                    source_id: source_id.clone(),
                    kind: "jellyfin".to_string(),
                    name: "Configured Jellyfin".to_string(),
                    provider_payload: serde_json::json!({
                        "version": 1,
                        "base_url": "https://jellyfin.invalid",
                        "user_id": "listener-id",
                        "username": "listener",
                        "trust_invalid_cert": false,
                        "use_jellyfin_instant_mix": false,
                    })
                    .to_string(),
                },
                credential_ref: Some(CredentialRef::new("missing-credential")),
                music_folder_id: None,
                local_access: None,
            }];
            stored.sources.selected_source_id = Some(source_id.clone());
            stored.ui.secret_storage_mode = SecretStorageMode::SystemKeyring;
            Ok(())
        })
        .expect("save configured source");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library,
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        scrobbler,
        runtime.handle().clone(),
        SourceOutputs { events, discovery },
    );
    assert_eq!(
        bootstrap.operation,
        SourceOperation::Switching {
            target: source_id.clone(),
            progress: initial_progress(),
        }
    );
    assert_eq!(
        bootstrap.configured.selected_source_id.as_ref(),
        Some(&source_id)
    );
    assert!(bootstrap.owner.shared.selected().is_none());

    let mut actor = actor_for_test(&bootstrap.owner);
    runtime
        .block_on(actor.change_secret_storage(SecretStorageMode::ConfigFile))
        .expect("change secret storage");

    assert_eq!(
        settings.load().sources.selected_source_id.as_ref(),
        Some(&source_id)
    );
    assert!(bootstrap.owner.shared.selected().is_none());
    assert!(
        event_receiver.try_recv().is_err(),
        "an unavailable source does not need a synthetic source assignment"
    );
}

#[test]
fn activity_mailbox_applies_and_publishes_recorded_updates_in_order() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let music_root = directory.path().join("music");
    std::fs::create_dir(&music_root).expect("create Local music folder");
    let source_id = SourceId::new("local:server:activity-mailbox");
    let track_id = library::TrackId::new("local:track:activity-mailbox");
    let second_track_id = library::TrackId::new("local:track:activity-mailbox-two");
    let configuration = SourceConfiguration {
        source_id: source_id.clone(),
        kind: "local".to_string(),
        name: "Local".to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "roots": [music_root],
        })
        .to_string(),
    };
    let identity = configuration.input_identity().expect("source identity");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: identity.version,
            input_digest: identity.digest,
        })
        .expect("begin source candidate");
    let first_track = Track::new(TrackData {
        id: track_id.clone(),
        album_id: None,
        title: "Activity Track".to_string(),
        artist: "Artist".to_string(),
        album: String::new(),
        album_artwork: None,
        year: 2024,
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
        local_artwork: None,
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        source_path: Some(
            music_root
                .join("Activity.flac")
                .to_string_lossy()
                .into_owned(),
        ),
        cue: None,
        source_format: Some("flac".to_string()),
        comment: None,
        skip_count: None,
        bpm: None,
        relations: TrackRelations::default(),
    });
    let mut second_track = first_track.clone();
    second_track.id = second_track_id.clone();
    second_track.title = "Second Activity Track".to_string();
    second_track.source_path = Some(
        music_root
            .join("Second Activity.flac")
            .to_string_lossy()
            .into_owned(),
    );
    candidate
        .write(CandidateBatch::Tracks(vec![first_track, second_track]))
        .expect("write cached Track");
    let accepted = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept cached source");
    library
        .initialize_smart_playlists(&accepted.loaded)
        .expect("seed Smart Playlists");

    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration: configuration.clone(),
                credential_ref: None,
                music_folder_id: None,
                local_access: None,
            }];
            stored.sources.selected_source_id = Some(source_id.clone());
            Ok(())
        })
        .expect("save selected source");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library.clone(),
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings,
        secrets,
        scrobbler,
        runtime.handle().clone(),
        SourceOutputs { events, discovery },
    );
    let selected = SelectedSourceRuntime {
        configuration,
        source: None,
        source_session_epoch: SourceSessionEpoch::new(1),
        home: library
            .home(&accepted.loaded, None)
            .expect("prepare activity Home"),
        loaded: Arc::clone(&accepted.loaded),
        music_folder_id: None,
    };
    *bootstrap
        .owner
        .shared
        .selected
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(SelectedSourceSession::new(selected.clone()));

    let play = library
        .record_play(
            &selected.loaded,
            AcceptedPlay {
                play_id: "mailbox-play".to_string(),
                track_id: track_id.clone(),
                played_at: 1_700_000_000,
                month: "2023-11".to_string(),
            },
        )
        .expect("record play")
        .expect("new play");
    let second_play = library
        .record_play(
            &selected.loaded,
            AcceptedPlay {
                play_id: "mailbox-play-two".to_string(),
                track_id: second_track_id.clone(),
                played_at: 1_700_000_001,
                month: "2023-11".to_string(),
            },
        )
        .expect("record second play")
        .expect("new second play");
    let skip = library
        .record_skip(
            &selected.loaded,
            AcceptedSkip {
                track_id: track_id.clone(),
            },
        )
        .expect("record skip");
    let receiver = bootstrap
        .owner
        .receiver
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .expect("take source mailbox");
    let actor = actor_for_test(&bootstrap.owner);
    let actor_task = runtime.spawn(actor.run(receiver));
    let acceptance = bootstrap.owner.acceptance_sender();
    acceptance.publish_activity(
        source_id.clone(),
        selected.source_session_epoch,
        play,
        Some(track_id.clone()),
    );
    acceptance.publish_activity(
        source_id.clone(),
        selected.source_session_epoch,
        second_play,
        Some(second_track_id.clone()),
    );
    acceptance.publish_activity(source_id.clone(), selected.source_session_epoch, skip, None);
    let updates = runtime.block_on(async {
        let mut updates = Vec::new();
        while updates.len() < 3 {
            if let SourceEvent::LibraryUpdate(update) =
                event_receiver.recv().await.expect("source publication")
            {
                updates.push(update);
            }
        }
        updates
    });
    actor_task.abort();
    runtime.block_on(async {
        let _ = actor_task.await;
    });

    assert_eq!(updates[0].source_id, source_id);
    assert_eq!(
        updates[0].source_session_epoch,
        selected.source_session_epoch
    );
    let first = updates[0].change.tracks[0]
        .track
        .as_ref()
        .expect("played Track");
    assert_eq!(first.play_count, Some(1));
    assert_eq!(first.skip_count, Some(0));
    assert!(!updates[0].change.smart_playlists.is_empty());
    let first_home = updates[0].home.as_ref().expect("first next Home");
    assert_eq!(
        first_home
            .section(HomeSectionKind::RecentlyPlayed)
            .expect("first Recently Played")
            .items
            .len(),
        1
    );

    let second = updates[1].change.tracks[0]
        .track
        .as_ref()
        .expect("second played Track");
    assert_eq!(second.play_count, Some(1));
    assert_eq!(second.skip_count, Some(0));
    assert!(!updates[1].change.smart_playlists.is_empty());
    let second_home = updates[1].home.as_ref().expect("second next Home");
    assert_eq!(
        second_home
            .section(HomeSectionKind::MostPlayed)
            .expect("accumulated Most Played")
            .items
            .len(),
        2
    );
    assert_eq!(
        second_home
            .section(HomeSectionKind::RecentlyPlayed)
            .expect("accumulated Recently Played")
            .items
            .len(),
        2
    );

    let skipped = updates[2].change.tracks[0]
        .track
        .as_ref()
        .expect("skipped Track");
    assert_eq!(skipped.play_count, Some(1));
    assert_eq!(skipped.skip_count, Some(1));
    assert!(!updates[2].change.smart_playlists.is_empty());
    assert!(updates[2].home.is_none());
    let final_track = selected
        .loaded
        .track(&track_id)
        .expect("read final Track")
        .expect("final Track");
    assert_eq!(final_track.play_count, Some(1));
    assert_eq!(final_track.skip_count, Some(1));
    let second_track = selected
        .loaded
        .track(&second_track_id)
        .expect("read second final Track")
        .expect("second final Track");
    assert_eq!(second_track.play_count, Some(1));
    assert_eq!(second_track.skip_count, Some(0));
}

#[test]
fn activity_accepted_during_refresh_is_replayed_into_the_replacement_library() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let path = directory.path().join("library.db");
    let source_id = SourceId::new("local:server:activity-refresh");
    let track_id = library::TrackId::new("local:track:activity-refresh");
    let library = Library::open(&path).expect("open test Library");
    let mut initial = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: 1,
            input_digest: [1; 32],
        })
        .expect("begin initial candidate");
    initial
        .write(CandidateBatch::Tracks(vec![test_track(
            track_id.clone(),
            "Before Refresh",
            directory.path().join("Track.flac"),
        )]))
        .expect("write initial Track");
    let initial = initial
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept initial source");

    let mut replacement = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: 1,
            input_digest: [2; 32],
        })
        .expect("begin replacement candidate");
    replacement
        .write(CandidateBatch::Tracks(vec![test_track(
            track_id.clone(),
            "After Refresh",
            directory.path().join("Track.flac"),
        )]))
        .expect("write replacement Track");
    let replacement = replacement
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 2,
            },
            Some(&initial.loaded),
        )
        .expect("prepare replacement source");

    let activity = library
        .record_play(
            &initial.loaded,
            AcceptedPlay {
                play_id: "refresh-play".to_string(),
                track_id: track_id.clone(),
                played_at: 1_700_000_000,
                month: "2023-11".to_string(),
            },
        )
        .expect("record play during refresh")
        .expect("new play during refresh");
    library
        .apply_recorded_activity(&initial.loaded, &activity)
        .expect("apply play to current source");
    let replacement = replacement.accept().expect("accept replacement source");
    assert_eq!(
        replacement
            .loaded
            .track(&track_id)
            .expect("read replacement Track")
            .expect("replacement Track")
            .play_count,
        None,
        "the candidate was prepared before the accepted activity"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime");
    runtime
        .block_on(replay_activity_updates(
            library.clone(),
            Arc::clone(&replacement.loaded),
            vec![activity],
        ))
        .expect("replay activity into replacement");
    assert_eq!(
        replacement
            .loaded
            .track(&track_id)
            .expect("read replayed Track")
            .expect("replayed Track")
            .play_count,
        Some(1)
    );

    drop(replacement);
    drop(initial);
    drop(library);
    let reopened = Library::open(path)
        .expect("reopen Library")
        .load_source(&source_id)
        .expect("load replacement source")
        .expect("replacement source");
    assert_eq!(
        reopened
            .track(&track_id)
            .expect("read reopened Track")
            .expect("reopened Track")
            .play_count,
        Some(1)
    );
}

#[test]
fn local_file_change_updates_only_the_changed_component() {
    let directory = tempfile::tempdir().expect("temporary Local source");
    let music_root = directory.path().join("music");
    std::fs::create_dir(&music_root).expect("create Local music folder");
    let music_root = std::fs::canonicalize(music_root).expect("canonical Local music folder");
    let first_path = music_root.join("First.mp3");
    std::fs::write(&first_path, []).expect("write first Local Track");
    let other_directory = music_root.join("Other");
    std::fs::create_dir(&other_directory).expect("create unrelated Local directory");
    std::fs::write(other_directory.join("Outside.mp3"), []).expect("write unrelated Local Track");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime");
    let connected = runtime
        .block_on(Source::connect(SourceSetupInput::Local(
            LocalFolderHostInput {
                roots: vec![music_root.clone()],
            },
        )))
        .expect("open Local source");
    let (configuration, source, credential) = connected.into_parts();
    assert_eq!(credential, None);
    let identity = configuration.input_identity().expect("source identity");
    let source = Arc::new(source);
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let prepared = runtime
        .block_on(acquisition::read_source(
            library.clone(),
            identity,
            Arc::clone(&source),
            None,
            Arc::new(|_: SourceReadProgress| {}),
            Arc::new(AtomicBool::new(false)),
        ))
        .expect("read initial Local source");
    let accepted = prepared.accept().expect("accept initial Local source");
    assert_eq!(
        accepted
            .loaded
            .track_list(None, TrackSort::Title, false)
            .expect("read initial Tracks")
            .len(),
        2
    );
    let selected = SelectedSourceRuntime {
        configuration,
        source: Some(Arc::clone(&source)),
        source_session_epoch: SourceSessionEpoch::new(1),
        home: library
            .home(&accepted.loaded, None)
            .expect("prepare Local Home"),
        loaded: Arc::clone(&accepted.loaded),
        music_folder_id: None,
    };

    let second_path = music_root.join("Second.mp3");
    std::fs::write(&second_path, []).expect("write changed Local Track");
    let replacement = runtime
        .block_on(prepare_local_change(
            Arc::clone(&source),
            Arc::clone(&selected.loaded),
            LocalFilesystemChange::Paths(BTreeSet::from([second_path])),
            Arc::new(AtomicBool::new(false)),
        ))
        .expect("read changed Local component")
        .expect("changed Local path produced an exact component");
    assert_eq!(replacement.tracks.len(), 1);
    assert!(
        replacement
            .tracks
            .iter()
            .any(|track| track.title == "Second")
    );
    assert!(
        replacement
            .tracks
            .iter()
            .all(|track| track.title != "Outside")
    );
    let changed = library
        .accept_local_component(&accepted.loaded, replacement)
        .expect("accept changed Local component")
        .expect("changed Local component");
    assert!(changed.tracks.iter().any(|replacement| {
        replacement
            .track
            .as_ref()
            .is_some_and(|track| track.title == "Second")
    }));
    assert_eq!(
        accepted
            .loaded
            .track_list(None, TrackSort::Title, false)
            .expect("read changed Tracks")
            .len(),
        3
    );

    let unchanged = runtime
        .block_on(prepare_local_change(
            source,
            Arc::clone(&selected.loaded),
            LocalFilesystemChange::Rescan,
            Arc::new(AtomicBool::new(false)),
        ))
        .expect("verify unchanged Local source");
    assert!(unchanged.is_none());
}

#[test]
fn metadata_reply_reports_stale_and_interrupted_work() {
    let (sender, receiver) = async_channel::bounded(1);
    drop(MetadataReply::new(sender));
    assert_eq!(receiver.try_recv(), Ok(Err(MetadataError::Unavailable)));

    let (sender, receiver) = async_channel::bounded(1);
    let mut reply = MetadataReply::new(sender);
    reply.mark_write_started();
    drop(reply);
    assert_eq!(
        receiver.try_recv(),
        Ok(Err(MetadataError::SavedRefreshFailed(
            "Metadata editing was interrupted before the written metadata was accepted."
                .to_string()
        )))
    );

    let (sender, receiver) = async_channel::bounded(1);
    MetadataReply::new(sender).finish(Ok(()));
    assert_eq!(receiver.try_recv(), Ok(Ok(())));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn selected_work_collects_activity_only_from_the_same_source_session() {
    let qualifier = SourceQualifier {
        source_id: SourceId::new("selected"),
        epoch: SourceSessionEpoch::new(4),
    };
    let purpose = WorkPurpose::Selected {
        qualifier: qualifier.clone(),
        automatic: false,
    };
    assert!(work_accepts_activity(&purpose, &qualifier));
    assert!(!work_accepts_activity(
        &purpose,
        &SourceQualifier {
            source_id: qualifier.source_id,
            epoch: SourceSessionEpoch::new(5),
        }
    ));
}

#[test]
fn local_metadata_edit_prepares_the_written_file_for_library_acceptance() {
    let directory = tempfile::tempdir().expect("temporary Local source");
    let music_root = directory.path().join("music");
    std::fs::create_dir(&music_root).expect("create Local music folder");
    let path = music_root.join("Before.wav");
    write_silent_wav(&path).expect("write WAV");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime");
    let connected = runtime
        .block_on(Source::connect(SourceSetupInput::Local(
            LocalFolderHostInput {
                roots: vec![music_root],
            },
        )))
        .expect("open Local source");
    let (configuration, source, _) = connected.into_parts();
    let identity = configuration.input_identity().expect("source identity");
    let source = Arc::new(source);
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let prepared = runtime
        .block_on(acquisition::read_source(
            library.clone(),
            identity,
            Arc::clone(&source),
            None,
            Arc::new(|_: SourceReadProgress| {}),
            Arc::new(AtomicBool::new(false)),
        ))
        .expect("read initial Local source");
    let accepted = prepared.accept().expect("accept initial Local source");
    let edited_track = accepted
        .loaded
        .track_list(None, TrackSort::Title, false)
        .expect("read initial Tracks")
        .track(0)
        .expect("resolve initial Track")
        .expect("initial Track");
    let draft = runtime
        .block_on(source.read_metadata(library::MetadataSubject::track(edited_track.clone()), None))
        .expect("read metadata draft");
    let refresh = runtime
        .block_on(source.write_metadata(
            library::MetadataSubject::track(edited_track.clone()),
            MetadataEdit {
                item_id: MetadataItemId::Track(edited_track.id.clone()),
                revision: draft.revision,
                changes: vec![MetadataChange::Title("After".to_string())],
            },
            None,
        ))
        .expect("write Local metadata");
    let MetadataRefresh::Local(change) = refresh else {
        panic!("Local metadata write did not request a Local refresh");
    };
    let check = source
        .check_local(change, &|| false)
        .expect("check written Local metadata");
    let accepted_files = accepted
        .loaded
        .local_file_baseline(check.file_seeds())
        .expect("read accepted Local file baseline");
    let change = source
        .confirm_local_change(check, accepted_files, &|_| {}, &|| false)
        .expect("confirm written Local metadata")
        .expect("written Local metadata changed");
    let baseline = accepted
        .loaded
        .local_component_baseline(change.component_seeds())
        .expect("read accepted Local component baseline");
    let replacement = source
        .complete_local_change(change, baseline, 1, &|| false)
        .expect("prepare Local metadata replacement");
    let change = library
        .accept_local_component(&accepted.loaded, replacement)
        .expect("accept metadata component")
        .expect("changed metadata component");

    assert!(change.tracks.iter().any(|replacement| {
        replacement
            .track
            .as_ref()
            .is_some_and(|track| track.id == edited_track.id && track.title == "After")
    }));
    assert_eq!(
        accepted
            .loaded
            .track(&edited_track.id)
            .expect("read accepted Track")
            .expect("accepted Track")
            .title,
        "After"
    );
}

#[test]
fn private_mode_still_uses_source_metadata_search() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime");
    let editing = MetadataEditing::new(vec![library::MetadataField::Title]);
    let current = library::MetadataValues {
        title: "Current".to_string(),
        ..library::MetadataValues::default()
    };
    let source_candidate = library::MetadataValues {
        title: "Source candidate".to_string(),
        ..library::MetadataValues::default()
    };

    let identified = runtime
        .block_on(resolve_identification(
            false,
            true,
            &editing,
            &current,
            async { panic!("private mode must not poll direct MusicBrainz lookup") },
            async { Ok(Some(source_candidate)) },
        ))
        .expect("source metadata search")
        .expect("source metadata search candidate");
    assert_eq!(identified.title, "Source candidate");
}

#[test]
fn direct_metadata_candidate_short_circuits_source_search() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime");
    let editing = MetadataEditing::new(vec![library::MetadataField::Title]);
    let current = library::MetadataValues {
        title: "Current".to_string(),
        ..library::MetadataValues::default()
    };
    let direct = library::MetadataValues {
        title: "Direct".to_string(),
        ..library::MetadataValues::default()
    };

    let identified = runtime
        .block_on(resolve_identification(
            true,
            true,
            &editing,
            &current,
            async { Ok(Some(direct)) },
            async { panic!("an applicable direct candidate must not poll the source fallback") },
        ))
        .expect("direct identification")
        .expect("direct candidate");
    assert_eq!(identified.title, "Direct");
}

#[test]
fn direct_miss_or_unchanged_candidate_falls_back_once() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime");
    let editing = MetadataEditing::new(vec![library::MetadataField::Title]);
    let current = library::MetadataValues {
        title: "Current".to_string(),
        ..library::MetadataValues::default()
    };
    let source_candidate = || library::MetadataValues {
        title: "Source candidate".to_string(),
        ..library::MetadataValues::default()
    };

    let identified = runtime
        .block_on(resolve_identification(
            true,
            true,
            &editing,
            &current,
            async { Ok(None) },
            async { Ok(Some(source_candidate())) },
        ))
        .expect("source fallback after direct miss")
        .expect("source fallback candidate");
    assert_eq!(identified.title, "Source candidate");

    let identified = runtime
        .block_on(resolve_identification(
            true,
            true,
            &editing,
            &current,
            async { Ok(Some(current.clone())) },
            async { Ok(Some(source_candidate())) },
        ))
        .expect("source fallback after unchanged direct candidate")
        .expect("source fallback candidate");
    assert_eq!(identified.title, "Source candidate");
}

#[test]
fn metadata_identification_failure_arbitration_uses_the_applicable_request() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime");
    let editing = MetadataEditing::new(vec![library::MetadataField::Title]);
    let current = library::MetadataValues {
        title: "Current".to_string(),
        ..library::MetadataValues::default()
    };

    let identified = runtime
        .block_on(resolve_identification(
            true,
            true,
            &editing,
            &current,
            async { Err("MusicBrainz request failed".to_string()) },
            async { Ok(None) },
        ))
        .expect("successful source miss suppresses a direct failure");
    assert_eq!(identified, None);

    let error = runtime
        .block_on(resolve_identification(
            true,
            true,
            &editing,
            &current,
            async { Err("MusicBrainz request failed".to_string()) },
            async { Err("Jellyfin request failed".to_string()) },
        ))
        .expect_err("native failure wins");
    assert_eq!(error, "Jellyfin request failed");

    let error = runtime
        .block_on(resolve_identification(
            true,
            false,
            &editing,
            &current,
            async { Err("MusicBrainz request failed".to_string()) },
            async { panic!("unsupported native search must not be polled") },
        ))
        .expect_err("direct-only failure remains visible");
    assert_eq!(error, "MusicBrainz request failed");

    let identified = runtime
        .block_on(resolve_identification(
            false,
            false,
            &editing,
            &current,
            async { panic!("inapplicable direct search must not be polled") },
            async { panic!("inapplicable native search must not be polled") },
        ))
        .expect("no applicable lookup is silent");
    assert_eq!(identified, None);
}

#[test]
fn removing_the_selected_source_chooses_the_first_survivor() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let configured = |id: &str, title: &str| {
        let fixture_name = title.to_ascii_lowercase();
        let root = directory.path().join(&fixture_name);
        std::fs::create_dir(&root).expect("create Local root");
        let configuration = SourceConfiguration {
            source_id: SourceId::new(id),
            kind: "local".to_string(),
            name: title.to_string(),
            provider_payload: serde_json::json!({
                "version": 1,
                "roots": [root],
            })
            .to_string(),
        };
        let identity = configuration.input_identity().expect("source identity");
        let mut candidate = library
            .begin_source_candidate(CandidateHeader {
                source_id: configuration.source_id.clone(),
                input_version: identity.version,
                input_digest: identity.digest,
            })
            .expect("begin Local candidate");
        candidate
            .write(CandidateBatch::Tracks(vec![test_track(
                library::TrackId::new(format!("{id}:track")),
                title,
                directory.path().join(format!("{fixture_name}.flac")),
            )]))
            .expect("write Local Track");
        candidate
            .finish(
                CandidateFinish {
                    freshness: None,
                    home: HomeFacts::RufinDefined,
                    accepted_at: 1,
                },
                None,
            )
            .and_then(|candidate| candidate.accept())
            .expect("accept Local source");
        ConfiguredSource {
            configuration,
            credential_ref: None,
            music_folder_id: None,
            local_access: None,
        }
    };
    let survivor = configured("local:server:survivor", "Survivor");
    let removed_source = configured("local:server:removed", "Removed");
    let survivor_id = survivor.configuration.source_id.clone();
    let removed = removed_source.configuration.source_id.clone();
    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    settings
        .update(|stored| {
            stored.sources.configured = vec![survivor, removed_source];
            stored.sources.selected_source_id = Some(removed.clone());
            Ok(())
        })
        .expect("save configured sources");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library.clone(),
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        Arc::clone(&scrobbler),
        runtime.handle().clone(),
        SourceOutputs {
            events: events.clone(),
            discovery,
        },
    );
    let playback = attach_test_playback(
        &bootstrap,
        library.clone(),
        settings.clone(),
        runtime.handle().clone(),
        events,
        scrobbler,
        directory.path(),
    );
    bootstrap.owner.start().expect("start source owner");
    runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                let mut selected = false;
                loop {
                    match event_receiver.recv().await.expect("initial source event") {
                        SourceEvent::Selected { selected: next, .. } => {
                            selected = next.source_id == removed;
                        }
                        SourceEvent::Operation(SourceOperation::Idle) if selected => break,
                        SourceEvent::ReleaseSelected { acknowledged } => {
                            let _ = acknowledged.try_send(());
                        }
                        _ => {}
                    }
                }
            })
            .await
        })
        .expect("initial source opens");

    bootstrap.owner.forget_source(removed.clone());
    let removal_events = runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                let mut events = Vec::new();
                let mut selected = false;
                loop {
                    let event = event_receiver.recv().await.expect("source removal event");
                    if let SourceEvent::ReleaseSelected { acknowledged } = &event {
                        let _ = acknowledged.try_send(());
                    }
                    if let SourceEvent::Selected { selected: next, .. } = &event {
                        selected = next.source_id == survivor_id;
                    }
                    let finished =
                        selected && matches!(event, SourceEvent::Operation(SourceOperation::Idle));
                    events.push(event);
                    if finished {
                        break events;
                    }
                }
            })
            .await
        })
        .expect("selected source removal completes");
    assert!(removal_events.iter().any(|event| matches!(
        event,
        SourceEvent::Operation(SourceOperation::Switching { target, .. })
            if target == &survivor_id
    )));
    assert!(
        removal_events
            .iter()
            .any(|event| matches!(event, SourceEvent::ReleaseSelected { .. }))
    );
    assert!(
        !removal_events.iter().any(
            |event| matches!(event, SourceEvent::Configured(configured) if configured.first_run)
        )
    );
    let stored = settings.load().sources;
    assert_eq!(stored.selected_source_id.as_ref(), Some(&survivor_id));
    assert_eq!(stored.configured.len(), 1);
    assert_eq!(
        bootstrap
            .owner
            .selected()
            .expect("surviving source is selected")
            .source_id(),
        &survivor_id
    );
    assert!(
        library
            .load_source(&removed)
            .expect("read removed source")
            .is_none()
    );
    playback
        .stop_for_source_switch()
        .expect("stop surviving Playback");
}

#[test]
fn first_local_folder_enters_the_source_add_transition() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library,
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings,
        secrets,
        scrobbler,
        runtime.handle().clone(),
        SourceOutputs { events, discovery },
    );
    let mut actor = actor_for_test(&bootstrap.owner);
    runtime.block_on(actor.queue_work(WorkRequest::AddLocalFolder(directory.path().join("music"))));
    assert!(matches!(
        event_receiver.try_recv(),
        Ok(SourceEvent::Operation(SourceOperation::Adding { .. }))
    ));
    assert!(matches!(
        actor.active.as_ref().map(|active| &active.purpose),
        Some(WorkPurpose::Add)
    ));
    runtime.block_on(actor.cancel_all_work());
}

#[test]
fn failed_metadata_access_setting_save_restores_the_accepted_mapping() {
    let directory = tempfile::tempdir().expect("temporary Local access transaction");
    let store_path = directory.path().join("library.db");
    let library = Library::open(&store_path).expect("open test Library");
    let source_id = SourceId::new("navidrome:server:local-access-transaction");
    let track_id = library::TrackId::new("navidrome:track:local-access-transaction");
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: 1,
            input_digest: [11; 32],
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(vec![test_track(
            track_id.clone(),
            "Track",
            PathBuf::from("/server/music/Artist/Track.wav"),
        )]))
        .expect("write mapped Track");
    let accepted = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept mapped source");
    let previous_root = directory.path().join("previous");
    let previous_path = previous_root.join("Artist/Track.wav");
    let previous_access = ConfiguredLocalAccess {
        root_path: previous_root.clone(),
        server_prefix: Some("/server/music".to_string()),
        local_prefix: Some(previous_root.to_string_lossy().into_owned()),
    };
    let previous_files = vec![library::LocalAccessFile {
        path: previous_path.to_string_lossy().into_owned(),
        root: previous_root.to_string_lossy().into_owned(),
        relative_path: "Artist/Track.wav".to_string(),
        size_bytes: 1,
        mtime_ns: 1,
        device_id: None,
        inode: None,
        parser_version: 1,
        title: "Track".to_string(),
        album: String::new(),
        artist: "Artist".to_string(),
        disc_number: 1,
        track_number: 1,
        duration_seconds: 180,
    }];
    library
        .replace_local_access(
            &accepted.loaded,
            configured_local_access_mapping(&previous_access),
            previous_files.clone(),
        )
        .expect("accept previous Local access");

    let proposed_root = directory.path().join("proposed");
    let error = accept_metadata_local_access_mapping(
        &library,
        &accepted.loaded,
        library::LocalAccessMapping {
            root_path: proposed_root.clone(),
            server_prefix: Some("/server/music".to_string()),
            local_prefix: Some(proposed_root.to_string_lossy().into_owned()),
        },
        Some(previous_access),
        || Err("settings write failed".to_string()),
    )
    .expect_err("failed Settings save rolls back Local access");

    assert_eq!(error, "settings write failed");
    assert_eq!(
        accepted
            .loaded
            .local_access_files()
            .expect("read restored Local access"),
        previous_files
    );
    let (_, targets) = accepted
        .loaded
        .metadata_subject_with_local_access(&MetadataItemId::Track(track_id.clone()), None)
        .expect("resolve restored Local access")
        .expect("restored metadata Track");
    assert_eq!(
        targets
            .first()
            .expect("previous Local access remains accepted")
            .path(),
        previous_path
    );

    drop(accepted);
    drop(library);
    let reopened = Library::open(store_path)
        .expect("reopen Library")
        .load_source(&source_id)
        .expect("load restored source")
        .expect("restored source");
    assert_eq!(
        reopened
            .local_access_files()
            .expect("read durable restored Local access"),
        previous_files
    );
}

#[test]
fn metadata_mapping_is_session_scoped_and_normal_save_does_not_wait_for_scan() {
    let directory = tempfile::tempdir().expect("temporary stale mapping");
    let music_root = directory.path().join("music");
    std::fs::create_dir(&music_root).expect("create Local music folder");
    let source_id = SourceId::new("local:server:stale-mapping");
    let track_id = library::TrackId::new("local:track:stale-mapping");
    let configuration = SourceConfiguration {
        source_id: source_id.clone(),
        kind: "local".to_string(),
        name: "Local".to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "roots": [music_root],
        })
        .to_string(),
    };
    let identity = configuration.input_identity().expect("source identity");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: identity.version,
            input_digest: identity.digest,
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(vec![test_track(
            track_id.clone(),
            "Track",
            music_root.join("Track.flac"),
        )]))
        .expect("write cached Track");
    let accepted = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept cached source");

    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration: configuration.clone(),
                credential_ref: None,
                music_folder_id: None,
                local_access: None,
            }];
            stored.sources.selected_source_id = Some(source_id.clone());
            Ok(())
        })
        .expect("save selected source");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, _event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library.clone(),
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        scrobbler,
        runtime.handle().clone(),
        SourceOutputs { events, discovery },
    );
    let selected = SelectedSourceRuntime {
        configuration,
        source: None,
        source_session_epoch: SourceSessionEpoch::new(1),
        home: library
            .home(&accepted.loaded, None)
            .expect("prepare cached Home"),
        loaded: Arc::clone(&accepted.loaded),
        music_folder_id: None,
    };
    *bootstrap
        .owner
        .shared
        .selected
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(SelectedSourceSession::new(selected));
    let mut actor = actor_for_test(&bootstrap.owner);
    let (completion, result) = async_channel::bounded(1);
    runtime.block_on(actor.save_local_access(
        SourceLocalAccess {
            source_id: source_id.clone(),
            root_path: music_root.clone(),
            server_prefix: None,
            local_prefix: Some(music_root.to_string_lossy().into_owned()),
        },
        Some(MetadataRequest {
            source_id: source_id.clone(),
            source_session_epoch: SourceSessionEpoch::new(2),
            item_id: MetadataItemId::Track(track_id),
        }),
        completion,
    ));

    assert!(
        runtime
            .block_on(result.recv())
            .expect("stale mapping reply")
            .expect_err("stale mapping rejected")
            .contains("inactive source session")
    );
    assert!(settings.load().sources.configured[0].local_access.is_none());

    let access = SourceLocalAccess {
        source_id,
        root_path: music_root.clone(),
        server_prefix: None,
        local_prefix: Some(music_root.to_string_lossy().into_owned()),
    };
    let (completion, result) = async_channel::bounded(1);
    runtime.block_on(actor.save_local_access(access.clone(), None, completion));
    assert_eq!(result.try_recv(), Ok(Ok(())));
    assert_eq!(
        settings.load().sources.configured[0].local_access,
        Some(ConfiguredLocalAccess {
            root_path: access.root_path,
            server_prefix: access.server_prefix,
            local_prefix: access.local_prefix,
        })
    );
    assert!(
        actor.local_access.is_some(),
        "the accepted setting replies before its background folder scan finishes"
    );
    runtime.block_on(actor.cancel_all_work());
}

#[test]
fn failed_metadata_mapping_preflight_preserves_the_accepted_mapping() {
    let directory = tempfile::tempdir().expect("temporary failed mapping recovery");
    let previous_root = directory.path().join("previous");
    let proposed_root = directory.path().join("proposed");
    let previous_path = previous_root.join("Artist/Track.flac");
    std::fs::create_dir_all(
        previous_path
            .parent()
            .expect("previous Local access parent"),
    )
    .expect("create previous Local access folder");
    std::fs::create_dir(&proposed_root).expect("create proposed Local access folder");
    std::fs::write(&previous_path, []).expect("write previous Local access file");
    let source_id = SourceId::new("navidrome:server:failed-mapping-recovery");
    let track_id = library::TrackId::new("navidrome:track:failed-mapping-recovery");
    let configuration = SourceConfiguration {
        source_id: source_id.clone(),
        kind: "navidrome".to_string(),
        name: "Navidrome".to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "base_url": "https://navidrome.example",
            "username": "listener",
            "trust_invalid_cert": false,
        })
        .to_string(),
    };
    let identity = configuration.input_identity().expect("source identity");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: identity.version,
            input_digest: identity.digest,
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(vec![test_track(
            track_id.clone(),
            "Track",
            PathBuf::from("/server/music/Artist/Track.flac"),
        )]))
        .expect("write cached Track");
    let accepted = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept cached source");
    let previous_access = ConfiguredLocalAccess {
        root_path: previous_root.clone(),
        server_prefix: Some("/server/music".to_string()),
        local_prefix: Some(previous_root.to_string_lossy().into_owned()),
    };
    let previous_files = vec![library::LocalAccessFile {
        path: previous_path.to_string_lossy().into_owned(),
        root: previous_root.to_string_lossy().into_owned(),
        relative_path: "Artist/Track.flac".to_string(),
        size_bytes: 0,
        mtime_ns: 1,
        device_id: None,
        inode: None,
        parser_version: 1,
        title: "Track".to_string(),
        album: String::new(),
        artist: "Artist".to_string(),
        disc_number: 1,
        track_number: 1,
        duration_seconds: 180,
    }];
    library
        .replace_local_access(
            &accepted.loaded,
            configured_local_access_mapping(&previous_access),
            previous_files.clone(),
        )
        .expect("accept previous Local access");

    let settings =
        SettingsFile::open(directory.path().join("settings.json")).expect("open Settings");
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration: configuration.clone(),
                credential_ref: None,
                music_folder_id: None,
                local_access: Some(previous_access.clone()),
            }];
            stored.sources.selected_source_id = Some(source_id.clone());
            Ok(())
        })
        .expect("save selected source");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let artwork = artwork::Artwork::new(directory.path().join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, _event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(library.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        library,
        test_downloads(directory.path().join("downloads"), runtime.handle().clone()),
        settings.clone(),
        secrets,
        scrobbler,
        runtime.handle().clone(),
        SourceOutputs { events, discovery },
    );
    let selected = SelectedSourceRuntime {
        configuration,
        source: None,
        source_session_epoch: SourceSessionEpoch::new(1),
        home: bootstrap
            .owner
            .shared
            .library
            .home(&accepted.loaded, None)
            .expect("prepare cached Home"),
        loaded: Arc::clone(&accepted.loaded),
        music_folder_id: None,
    };
    *bootstrap
        .owner
        .shared
        .selected
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(SelectedSourceSession::new(selected));
    let mut actor = actor_for_test(&bootstrap.owner);
    let (completion, result) = async_channel::bounded(1);
    runtime.block_on(actor.save_local_access(
        SourceLocalAccess {
            source_id: source_id.clone(),
            root_path: proposed_root.clone(),
            server_prefix: Some("/server/music".to_string()),
            local_prefix: Some(proposed_root.to_string_lossy().into_owned()),
        },
        Some(MetadataRequest {
            source_id,
            source_session_epoch: SourceSessionEpoch::new(1),
            item_id: MetadataItemId::Track(track_id.clone()),
        }),
        completion,
    ));

    assert!(
        runtime
            .block_on(result.recv())
            .expect("failed mapping preflight reply")
            .expect_err("an unavailable source rejects the mapping")
            .contains("source is unavailable")
    );
    assert!(
        actor.local_access.is_none(),
        "a metadata preflight must not start a whole-folder scan"
    );
    assert_eq!(
        settings.load().sources.configured[0].local_access,
        Some(previous_access)
    );
    assert_eq!(
        accepted
            .loaded
            .local_access_files()
            .expect("read accepted Local access"),
        previous_files
    );
    let (_, targets) = accepted
        .loaded
        .metadata_subject_with_local_access(&MetadataItemId::Track(track_id.clone()), None)
        .expect("resolve previous Local access")
        .expect("previous metadata Track");
    assert_eq!(
        targets
            .first()
            .expect("previous Local access remains accepted")
            .path(),
        previous_path
    );
}

#[test]
fn standardized_results_reuse_accepted_track_facts_without_a_source_mirror() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let library = Library::open(directory.path().join("library.db")).expect("open test Library");
    let track_id = library::TrackId::new("navidrome:track:known");
    let accepted_track = test_track(
        track_id.clone(),
        "Accepted",
        PathBuf::from("/music/Artist/Accepted.flac"),
    );
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: SourceId::new("navidrome:server:test"),
            input_version: 1,
            input_digest: [1; 32],
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(vec![accepted_track]))
        .expect("write accepted Track");
    let accepted = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept source");
    let reported = test_track(
        track_id,
        "Reported",
        PathBuf::from("generated/Reported.flac"),
    );
    let unknown = test_track(
        library::TrackId::new("navidrome:track:unknown"),
        "Unknown",
        PathBuf::from("generated/Unknown.flac"),
    );

    let search = reconcile_search_results(
        &accepted.loaded,
        library::SearchResults {
            tracks: vec![reported.clone(), unknown.clone()],
            ..library::SearchResults::default()
        },
    )
    .expect("reconcile search");
    assert_eq!(search.tracks[0].title, "Accepted");
    assert_eq!(
        search.tracks[0].source_path.as_deref(),
        Some("/music/Artist/Accepted.flac")
    );
    assert_eq!(search.tracks[1], unknown);

    let folder = reconcile_folder_contents(
        &accepted.loaded,
        FolderContents {
            folders: Arc::from([]),
            tracks: vec![reported].into(),
        },
    )
    .expect("reconcile folder");
    assert_eq!(folder.tracks[0].title, "Accepted");
}

fn actor_for_test(owner: &SourceOwner) -> Actor {
    Actor {
        shared: Arc::clone(&owner.shared),
        sender: owner.messages.clone(),
        active: None,
        observer: None,
        local_access: None,
        pending: VecDeque::new(),
        next_freshness_check: tokio::time::Instant::now(),
        fallback: None,
        selected_revealed: false,
        active_album_release: None,
    }
}

fn test_track(id: library::TrackId, title: &str, path: PathBuf) -> Track {
    Track::new(TrackData {
        id,
        album_id: None,
        title: title.to_string(),
        artist: "Artist".to_string(),
        album: String::new(),
        album_artwork: None,
        year: 2024,
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
        local_artwork: None,
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        source_path: Some(path.to_string_lossy().into_owned()),
        cue: None,
        source_format: Some("flac".to_string()),
        comment: None,
        skip_count: None,
        bpm: None,
        relations: TrackRelations::default(),
    })
}

fn test_downloads(root: PathBuf, runtime: tokio::runtime::Handle) -> downloads::Downloads {
    let (events, _receiver) = async_channel::unbounded();
    downloads::Downloads::new(root, runtime, events)
}

#[derive(Default)]
struct AcceptingPlaybackBackend;

impl playback::PlaybackBackend for AcceptingPlaybackBackend {
    fn send(&mut self, _command: playback::BackendCommand) -> Result<(), playback::BackendError> {
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<playback::BackendEvent> {
        Vec::new()
    }
}

fn attach_test_playback(
    bootstrap: &SourceBootstrap,
    library: Library,
    settings: SettingsFile,
    runtime: tokio::runtime::Handle,
    source_events: async_channel::Sender<SourceEvent>,
    scrobbler: Arc<Scrobbler>,
    data_directory: &Path,
) -> Arc<PlaybackOwner> {
    let (waveform_events, _waveform_receiver) = async_channel::unbounded();
    let waveform = crate::waveform::WaveformOwner::new(
        runtime.clone(),
        waveform_events,
        data_directory.join("playback"),
        false,
    );
    let (lyrics_events, _lyrics_receiver) = async_channel::unbounded();
    let lyrics = lyrics::LyricsService::new(
        library.clone(),
        runtime.clone(),
        lyrics::Settings {
            external_lyrics_enabled: false,
            ..lyrics::Settings::default()
        },
        true,
        lyrics_events,
    );
    let playback = PlaybackOwner::new(
        library,
        settings,
        runtime,
        source_events,
        bootstrap.owner.acceptance_sender(),
        waveform,
        lyrics,
        Arc::new(desktop_integration::Discord::new()),
        scrobbler,
        || Ok(Box::<AcceptingPlaybackBackend>::default() as Box<dyn playback::PlaybackBackend>),
    );
    bootstrap.owner.attach_playback(&playback);
    playback
}

fn write_silent_wav(path: &Path) -> std::io::Result<()> {
    let sample_rate = 8_000_u32;
    let bits_per_sample = 16_u16;
    let channels = 1_u16;
    let data_len = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let block_align = channels * (bits_per_sample / 8);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(bytes.len() + data_len as usize, 0);
    std::fs::write(path, bytes)
}
