use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use library::{
    AcceptedPlay, CandidateBatch, CandidateFinish, CandidateHeader, FolderContents, HomeFacts,
    MetadataChange, MetadataEdit, MetadataEditing, MetadataItemId, MusicFolder, MusicFolderId,
    Track, TrackData, TrackRelations, TrackSort,
};
use secrets::{MemorySecretStore, SwitchableSecretStore};
use sources::{
    LocalFilesystemChange, LocalFolderHostInput, MetadataRefresh, NativeSourceResult,
    SourceConfiguration, SourceError, SourceSetupInput,
};

use super::*;

#[test]
fn active_source_resolves_replacement_and_rejects_retired_session() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let runtime = test_runtime();
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let source_id = SourceId::new("local:server:session-fence");
    let library = accept_library(
        &libraries,
        source_id.clone(),
        vec![test_track(
            library::TrackId::new("local:track:session-fence"),
            "Session",
            PathBuf::from("Session.flac"),
            None,
        )],
        Vec::new(),
        1,
    );
    let settings = SettingsFile::memory();
    let (bootstrap, _events) = test_owner(directory.path(), &runtime, libraries, settings);
    let mut configuration = test_configuration(source_id.clone(), "First");
    let session = install_selected_for_test(
        &bootstrap.owner,
        configuration.clone(),
        None,
        Arc::clone(&library),
        SourceSessionEpoch::new(1),
    );

    configuration.name = "Replacement".to_string();
    let mut replacement = (*session.resolve().expect("selected session")).clone();
    replacement.configuration = configuration.clone();
    assert!(bootstrap.owner.shared.replace_selected(replacement));
    assert_eq!(
        session
            .resolve()
            .expect("same session replacement")
            .configuration
            .name,
        "Replacement"
    );

    let next = install_selected_for_test(
        &bootstrap.owner,
        configuration,
        None,
        library,
        SourceSessionEpoch::new(2),
    );
    assert!(session.resolve().is_none());
    assert_eq!(
        next.resolve().expect("new session").source_session_epoch,
        SourceSessionEpoch::new(2)
    );
}

#[test]
fn same_session_executor_change_retires_previous_access_tasks() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let root_a = directory.path().join("A");
    let root_b = directory.path().join("B");
    std::fs::create_dir(&root_a).expect("create first Local root");
    std::fs::create_dir(&root_b).expect("create replacement Local root");
    let runtime = test_runtime();
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let connected = runtime
        .block_on(Source::connect(SourceSetupInput::Local(
            LocalFolderHostInput {
                roots: vec![root_a],
            },
        )))
        .expect("connect first Local source");
    let (configuration, source, credential) = connected.into_parts();
    assert_eq!(credential, None);
    let source = Arc::new(source);
    let candidate = runtime
        .block_on(
            Arc::clone(&source).prepare_library_candidate(
                libraries.clone(),
                configuration
                    .input_identity()
                    .expect("first Local input identity"),
                None,
                Arc::new(|_| {}),
                Arc::new(AtomicBool::new(false)),
            ),
        )
        .expect("prepare first Local library");
    let library = candidate
        .accept()
        .expect("accept first Local library")
        .library;
    let settings = SettingsFile::memory();
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration: configuration.clone(),
                credential_ref: None,
                music_folder_id: None,
                local_access: None,
            }];
            stored.sources.selected_source_id = Some(configuration.source_id.clone());
            Ok(())
        })
        .expect("save first Local source");
    let (bootstrap, _events) = test_owner(directory.path(), &runtime, libraries, settings);
    let playback = attach_test_playback(&bootstrap.owner, &runtime, directory.path());
    let session = install_selected_for_test(
        &bootstrap.owner,
        configuration.clone(),
        Some(Arc::clone(&source)),
        library,
        SourceSessionEpoch::new(1),
    );
    let prepared = playback
        .prepare_selected(
            Arc::clone(&session),
            session.resolve().expect("selected source for Playback"),
        )
        .expect("prepare selected Playback");
    let cutover = playback.stop_for_source_switch();
    let _projection = playback.install_prepared(prepared, cutover);
    let qualifier = session.resolve().expect("selected source").qualifier();
    let observer_cancelled = Arc::new(AtomicBool::new(false));
    let local_cancelled = Arc::new(AtomicBool::new(false));
    let queued_observer_work = {
        let session = Arc::clone(&session);
        let cancelled = Arc::clone(&observer_cancelled);
        move || resolve_observer_session(&cancelled, &session)
    };
    let observer_handle = runtime.spawn(std::future::pending::<()>());
    let local_handle = runtime.spawn(std::future::pending::<()>());
    {
        let mut state = bootstrap
            .owner
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.observer = Some(ActiveObserver {
            qualifier: qualifier.clone(),
            cancelled: Arc::clone(&observer_cancelled),
            handle: observer_handle,
        });
        state.local_access = Some(ActiveLocalAccess {
            token: 1,
            qualifier,
            cancelled: Arc::clone(&local_cancelled),
            handle: local_handle.abort_handle(),
        });
    }

    runtime.block_on(async {
        bootstrap
            .owner
            .as_ref()
            .clone()
            .apply_source_update(
                configuration.source_id.clone(),
                SourceSettingsInput::Local {
                    roots: vec![root_b.clone()],
                },
                false,
                Arc::new(AtomicBool::new(false)),
            )
            .await;
    });

    assert!(observer_cancelled.load(Ordering::Acquire));
    assert!(local_cancelled.load(Ordering::Acquire));
    assert!(
        queued_observer_work().is_none(),
        "work queued by the retired observer must not resolve the retained session"
    );
    let selected = session.resolve().expect("same selected session");
    assert_eq!(selected.source_session_epoch, SourceSessionEpoch::new(1));
    assert!(!Arc::ptr_eq(
        &source,
        selected
            .source
            .as_ref()
            .expect("replacement source executor")
    ));
    let saved = configured_source(
        &bootstrap.owner.shared.settings.load().sources,
        &configuration.source_id,
    )
    .expect("saved replacement Local source");
    assert_eq!(
        local_roots(&saved.configuration).expect("saved replacement Local roots"),
        vec![root_b.canonicalize().expect("canonical replacement root")]
    );
}

#[test]
fn cached_folder_and_search_work_without_source_access() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let runtime = test_runtime();
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let source_id = SourceId::new("navidrome:server:offline");
    let folder_a = MusicFolderId::new("music-folder:a");
    let folder_b = MusicFolderId::new("music-folder:b");
    let track_a = library::TrackId::new("navidrome:track:alpha");
    let track_b = library::TrackId::new("navidrome:track:beta");
    let library = accept_library(
        &libraries,
        source_id.clone(),
        vec![
            test_track(
                track_a.clone(),
                "Alpha",
                PathBuf::from("Alpha.flac"),
                Some(folder_a.clone()),
            ),
            test_track(
                track_b,
                "Beta",
                PathBuf::from("Beta.flac"),
                Some(folder_b.clone()),
            ),
        ],
        vec![
            MusicFolder {
                id: folder_a.clone(),
                name: "A".to_string(),
                image_ref: None,
            },
            MusicFolder {
                id: folder_b,
                name: "B".to_string(),
                image_ref: None,
            },
        ],
        1,
    );
    let (bootstrap, _events) = test_owner(
        directory.path(),
        &runtime,
        libraries,
        SettingsFile::memory(),
    );
    let session = install_selected_for_test(
        &bootstrap.owner,
        test_configuration(source_id, "Offline"),
        None,
        library,
        SourceSessionEpoch::new(1),
    );

    let scoped = runtime
        .block_on(session.folder(None, Some(folder_a.clone())).recv())
        .expect("folder reply")
        .expect("cached folder");
    assert_eq!(
        scoped
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![track_a.clone()]
    );

    let stale = runtime
        .block_on(
            session
                .folder(
                    Some(library::FolderId::new("remote:folder:stale")),
                    Some(folder_a),
                )
                .recv(),
        )
        .expect("stale folder reply")
        .expect("stale folder falls back to scoped cache");
    assert_eq!(stale.tracks.len(), 1);
    assert_eq!(stale.tracks[0].id, track_a);

    let search = runtime
        .block_on(session.search(library::SearchRequest::new("Alpha")).recv())
        .expect("search reply")
        .expect("cached search");
    assert_eq!(search.tracks.len(), 1);
    assert_eq!(search.tracks[0].title, "Alpha");
}

#[test]
fn folder_and_search_fallback_only_for_outages() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let runtime = test_runtime();
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let library = accept_library(
        &libraries,
        SourceId::new("navidrome:server:outage-policy"),
        vec![test_track(
            library::TrackId::new("navidrome:track:outage-policy"),
            "Cached",
            PathBuf::from("Cached.flac"),
            None,
        )],
        Vec::new(),
        1,
    );

    let network = route_folder_result(
        Arc::clone(&library),
        None,
        None,
        Some(Err(SourceError::Network("offline".to_string()))),
    )
    .expect("network folder fallback");
    assert_eq!(network.tracks.len(), 1);

    let auth = route_folder_result(
        Arc::clone(&library),
        None,
        None,
        Some(Err(SourceError::Auth("expired".to_string()))),
    )
    .expect_err("authentication errors remain visible");
    assert!(auth.contains("authentication"));

    let unavailable = route_folder_result(
        Arc::clone(&library),
        None,
        None,
        Some(Ok(NativeSourceResult::Unavailable)),
    )
    .expect("provider-unavailable folder fallback");
    assert_eq!(unavailable.tracks.len(), 1);

    let server = runtime
        .block_on(route_search_result(
            Arc::clone(&library),
            library::SearchRequest::new("Cached"),
            Some(Err(SourceError::Server {
                status: 503,
                message: "maintenance".to_string(),
            })),
        ))
        .expect("server outage search fallback");
    assert_eq!(server.tracks.len(), 1);

    let protocol = runtime
        .block_on(route_search_result(
            library,
            library::SearchRequest::new("Cached"),
            Some(Err(SourceError::Other("malformed response".to_string()))),
        ))
        .expect_err("protocol errors remain visible");
    assert!(protocol.contains("malformed response"));

    assert!(source_error_allows_cache(&SourceError::Network(
        "offline".to_string()
    )));
    assert!(source_error_allows_cache(&SourceError::Server {
        status: 500,
        message: String::new(),
    }));
    assert!(source_error_allows_cache(&SourceError::Server {
        status: 599,
        message: String::new(),
    }));
    assert!(!source_error_allows_cache(&SourceError::Server {
        status: 404,
        message: String::new(),
    }));
    assert!(!source_error_allows_cache(
        &SourceError::Auth(String::new())
    ));
}

#[test]
fn retired_session_discards_delayed_metadata_results() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let runtime = test_runtime();
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let source_id = SourceId::new("local:server:metadata-fence");
    let library = accept_library(
        &libraries,
        source_id.clone(),
        vec![test_track(
            library::TrackId::new("local:track:metadata-fence"),
            "Metadata",
            PathBuf::from("Metadata.flac"),
            None,
        )],
        Vec::new(),
        1,
    );
    let (bootstrap, _events) = test_owner(
        directory.path(),
        &runtime,
        libraries,
        SettingsFile::memory(),
    );
    let configuration = test_configuration(source_id, "Metadata");
    let session = install_selected_for_test(
        &bootstrap.owner,
        configuration.clone(),
        None,
        Arc::clone(&library),
        SourceSessionEpoch::new(1),
    );
    let qualifier = session
        .resolve()
        .expect("selected metadata session")
        .qualifier();

    let allowed = runtime.block_on(async {
        let (started, start) = async_channel::bounded(1);
        let (release, released) = async_channel::bounded(1);
        let shared = Arc::clone(&bootstrap.owner.shared);
        let qualifier = qualifier.clone();
        let task = tokio::spawn(async move {
            fence_selected_completion(
                &shared,
                &qualifier,
                async move {
                    started.send(()).await.expect("signal delayed read");
                    released.recv().await.expect("release delayed read");
                    Ok::<u8, MetadataError>(7)
                },
                Err(MetadataError::Unavailable),
            )
            .await
        });
        start.recv().await.expect("delayed read started");
        let mut replacement = (*session.resolve().expect("same session")).clone();
        replacement.configuration.name = "Same epoch".to_string();
        assert!(bootstrap.owner.shared.replace_selected(replacement));
        release.send(()).await.expect("release delayed read");
        task.await.expect("join delayed read")
    });
    assert_eq!(allowed, Ok(7));

    let retired_read = runtime.block_on(async {
        let (started, start) = async_channel::bounded(1);
        let (release, released) = async_channel::bounded(1);
        let shared = Arc::clone(&bootstrap.owner.shared);
        let qualifier = qualifier.clone();
        let task = tokio::spawn(async move {
            fence_selected_completion(
                &shared,
                &qualifier,
                async move {
                    started.send(()).await.expect("signal delayed read");
                    released.recv().await.expect("release delayed read");
                    Ok::<u8, MetadataError>(9)
                },
                Err(MetadataError::Unavailable),
            )
            .await
        });
        start.recv().await.expect("delayed read started");
        install_selected_for_test(
            &bootstrap.owner,
            configuration.clone(),
            None,
            Arc::clone(&library),
            SourceSessionEpoch::new(2),
        );
        release.send(()).await.expect("release delayed read");
        task.await.expect("join delayed read")
    });
    assert_eq!(retired_read, Err(MetadataError::Unavailable));

    let epoch_two = bootstrap
        .owner
        .shared
        .selected()
        .expect("second selected session")
        .qualifier();
    let retired_identification = runtime.block_on(async {
        let (started, start) = async_channel::bounded(1);
        let (release, released) = async_channel::bounded(1);
        let shared = Arc::clone(&bootstrap.owner.shared);
        let task = tokio::spawn(async move {
            fence_selected_completion(
                &shared,
                &epoch_two,
                async move {
                    started
                        .send(())
                        .await
                        .expect("signal delayed identification");
                    released
                        .recv()
                        .await
                        .expect("release delayed identification");
                    Ok::<Option<u8>, String>(Some(3))
                },
                Ok(None),
            )
            .await
        });
        start.recv().await.expect("delayed identification started");
        install_selected_for_test(
            &bootstrap.owner,
            configuration,
            None,
            library,
            SourceSessionEpoch::new(3),
        );
        release
            .send(())
            .await
            .expect("release delayed identification");
        task.await.expect("join delayed identification")
    });
    assert_eq!(retired_identification, Ok(None));
}

#[test]
fn failed_target_prepare_keeps_the_selected_session() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let runtime = test_runtime();
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let selected_id = SourceId::new("local:server:selected-before-failure");
    let library = accept_library(
        &libraries,
        selected_id.clone(),
        vec![test_track(
            library::TrackId::new("local:track:selected-before-failure"),
            "Selected",
            PathBuf::from("Selected.flac"),
            None,
        )],
        Vec::new(),
        1,
    );
    let target_id = SourceId::new("local:server:missing-target");
    let target = SourceConfiguration {
        source_id: target_id.clone(),
        kind: "local".to_string(),
        name: "Missing".to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "roots": [directory.path().join("does-not-exist")],
        })
        .to_string(),
    };
    let settings = SettingsFile::memory();
    settings
        .update(|stored| {
            stored.sources.configured = vec![ConfiguredSource {
                configuration: target,
                credential_ref: None,
                music_folder_id: None,
                local_access: None,
            }];
            Ok(())
        })
        .expect("save target source");
    let (bootstrap, events) = test_owner(directory.path(), &runtime, libraries, settings);
    let selected = install_selected_for_test(
        &bootstrap.owner,
        test_configuration(selected_id, "Selected"),
        None,
        library,
        SourceSessionEpoch::new(1),
    );

    bootstrap.owner.select_source(target_id.clone());
    let (failed_source, released) = runtime.block_on(async {
        let mut released = false;
        loop {
            match events.recv().await.expect("source transition event") {
                SourceEvent::ReleaseSelected { acknowledged } => {
                    released = true;
                    acknowledged.send(()).await.expect("acknowledge release");
                }
                SourceEvent::Operation(SourceOperation::Failed { source_id, .. }) => {
                    break (source_id, released);
                }
                _ => {}
            }
        }
    });
    assert_eq!(failed_source, Some(target_id));
    assert!(!released, "failed preparation must not enter cutover");
    assert!(selected.resolve().is_some());
}

#[test]
fn preparing_a_replacement_keeps_downloads_on_the_current_library() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let runtime = test_runtime();
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let current_id = SourceId::new("local:server:current-downloads");
    let current_track_id = library::TrackId::new("local:track:current-downloads");
    let current = accept_library(
        &libraries,
        current_id.clone(),
        vec![test_track(
            current_track_id.clone(),
            "Current",
            PathBuf::from("Current.flac"),
            None,
        )],
        Vec::new(),
        1,
    );
    let replacement_id = SourceId::new("local:server:replacement-downloads");
    let replacement = accept_library(
        &libraries,
        replacement_id.clone(),
        vec![test_track(
            library::TrackId::new("local:track:replacement-downloads"),
            "Replacement",
            PathBuf::from("Replacement.flac"),
            None,
        )],
        Vec::new(),
        2,
    );
    let settings = SettingsFile::memory();
    let (bootstrap, _events, download_events) =
        test_owner_with_download_events(directory.path(), &runtime, libraries, settings);
    let _current_session = install_selected_for_test(
        &bootstrap.owner,
        test_configuration(current_id, "Current"),
        None,
        Arc::clone(&current),
        SourceSessionEpoch::new(1),
    );
    let playback = attach_test_playback(&bootstrap.owner, &runtime, directory.path());

    runtime.block_on(async {
        bootstrap
            .owner
            .shared
            .downloads
            .attach(None, &current, None)
            .await
            .expect("attach current downloads");
        let selected = Arc::new(SelectedSourceState {
            configuration: test_configuration(replacement_id, "Replacement"),
            source: None,
            source_session_epoch: SourceSessionEpoch::new(2),
            home: replacement.home(None).expect("prepare replacement Home"),
            library: replacement,
            music_folder_id: None,
        });
        let session = ActiveSource::new(&bootstrap.owner.shared, &selected);
        let prepared = playback
            .prepare_selected(session, selected)
            .expect("prepare replacement Playback");

        let tracks: library::TrackSelection = current
            .track_list(None, TrackSort::Title, false)
            .expect("current Track selection")
            .into();
        bootstrap.owner.shared.downloads.download(
            Arc::clone(&current),
            downloads::DownloadSubject::Track(current_track_id.clone()),
            tracks,
        );
        let feedback = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let downloads::DownloadEvent::Feedback(feedback) =
                    download_events.recv().await.expect("download publication")
                {
                    break feedback;
                }
            }
        })
        .await
        .expect("current Library must remain the Downloads target during preparation");
        assert_eq!(
            feedback.subject,
            downloads::DownloadSubject::Track(current_track_id)
        );
        drop(prepared);
    });
}

#[test]
fn activity_publishes_while_candidate_acquisition_is_blocked_and_rebases_once() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let path = directory.path().join("library.db");
    let source_id = SourceId::new("local:server:activity-refresh");
    let track_id = library::TrackId::new("local:track:activity-refresh");
    let libraries = Libraries::open(&path).expect("open Library");
    let initial = accept_library(
        &libraries,
        source_id.clone(),
        vec![test_track(
            track_id.clone(),
            "Before Refresh",
            directory.path().join("Track.flac"),
            None,
        )],
        Vec::new(),
        1,
    );
    let smart_playlist_id = initial
        .create_smart_playlist(
            "Played".to_string(),
            library::SmartPlaylistDefinition {
                match_all: vec![library::SmartPlaylistRule {
                    field: library::SmartPlaylistRuleField::PlayCount,
                    operator: library::SmartPlaylistRuleOperator::Above,
                    value: Some(library::SmartPlaylistRuleValue::Number(0)),
                }],
                match_any: Vec::new(),
                sort_field: library::SmartPlaylistSortField::PlayCount,
                descending: true,
                limit: None,
            },
        )
        .expect("create activity smart playlist")
        .expect("new activity smart playlist")
        .smart_playlists
        .into_iter()
        .next()
        .expect("created activity smart playlist ID");
    let mut replacement = libraries
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
            None,
        )]))
        .expect("write replacement Track");
    let replacement = replacement
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 2,
            },
            Some(&initial),
        )
        .expect("prepare replacement source");

    let runtime = test_runtime();
    let (bootstrap, events) = test_owner(
        directory.path(),
        &runtime,
        libraries.clone(),
        SettingsFile::memory(),
    );
    let epoch = SourceSessionEpoch::new(1);
    let session = install_selected_for_test(
        &bootstrap.owner,
        test_configuration(source_id.clone(), "Activity refresh"),
        None,
        Arc::clone(&initial),
        epoch,
    );
    let (started, candidate_started) = async_channel::bounded(1);
    let (resume, candidate_resume) = async_channel::bounded(1);
    let (accepted, candidate_accepted) = async_channel::bounded(1);
    bootstrap
        .owner
        .spawn_serialized(false, move |operations, _| async move {
            started
                .send(())
                .await
                .expect("signal candidate acquisition");
            candidate_resume
                .recv()
                .await
                .expect("finish candidate acquisition");
            let acceptance_owner = Arc::clone(&operations.shared);
            let _acceptance = acceptance_owner.acceptance_lane.lock().await;
            let result = replacement
                .accept()
                .map_err(string_error)
                .and_then(|commit| {
                    let current = operations
                        .shared
                        .selected()
                        .ok_or_else(|| "the selected source was retired".to_string())?;
                    let home = commit.library.home(None).map_err(string_error)?;
                    let library = Arc::clone(&commit.library);
                    let mut next = (*current).clone();
                    next.library = commit.library;
                    next.home = home;
                    operations
                        .shared
                        .replace_selected(next)
                        .then_some(library)
                        .ok_or_else(|| "the selected source changed".to_string())
                });
            accepted
                .send(result)
                .await
                .expect("report candidate acceptance");
        });
    let replacement = runtime.block_on(async {
        candidate_started
            .recv()
            .await
            .expect("candidate acquisition started");
        let activity = initial
            .record_play(AcceptedPlay {
                play_id: "refresh-play".to_string(),
                track_id: track_id.clone(),
                played_at: 1_700_000_000,
                month: "2023-11".to_string(),
            })
            .expect("record play during refresh")
            .expect("new play during refresh");
        bootstrap
            .owner
            .publish_activity(source_id.clone(), epoch, activity);
        let publication = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let SourceEvent::LibraryUpdate(update) =
                    events.recv().await.expect("activity publication")
                {
                    break update;
                }
            }
        })
        .await
        .expect("activity must publish while candidate acquisition is blocked");
        assert_eq!(
            publication.change.smart_playlists.as_slice(),
            std::slice::from_ref(&smart_playlist_id)
        );
        assert!(
            publication.home.is_some(),
            "accepted play must publish Home"
        );
        let current = session.resolve().expect("current selected source");
        assert!(Arc::ptr_eq(&current.library, &initial));
        assert_eq!(
            current
                .library
                .track(&track_id)
                .expect("read current Track")
                .expect("current Track")
                .play_count,
            Some(1)
        );
        assert_eq!(
            current
                .library
                .smart_playlist_detail(&smart_playlist_id, None)
                .expect("read current activity smart playlist")
                .expect("current activity smart playlist")
                .tracks
                .len(),
            1
        );
        resume.send(()).await.expect("finish candidate acquisition");
        let replacement = candidate_accepted
            .recv()
            .await
            .expect("candidate acceptance result")
            .expect("accept replacement source");
        let lane = Arc::clone(&bootstrap.owner.shared);
        let _finished = lane.lane.lock().await;
        replacement
    });
    assert_eq!(
        replacement
            .track(&track_id)
            .expect("read replacement Track")
            .expect("replacement Track")
            .play_count,
        Some(1)
    );
    assert_eq!(
        replacement
            .history_track_list(None)
            .expect("read replacement History")
            .len(),
        1
    );
    assert_eq!(
        replacement
            .smart_playlist_detail(&smart_playlist_id, None)
            .expect("read replacement activity smart playlist")
            .expect("replacement activity smart playlist")
            .tracks
            .len(),
        1
    );

    drop(replacement);
    drop(session);
    drop(bootstrap);
    drop(initial);
    drop(libraries);
    let reopened = Libraries::open(path)
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
    assert_eq!(
        reopened
            .history_track_list(None)
            .expect("read reopened History")
            .len(),
        1
    );
    assert_eq!(
        reopened
            .smart_playlist_detail(&smart_playlist_id, None)
            .expect("read reopened activity smart playlist")
            .expect("reopened activity smart playlist")
            .tracks
            .len(),
        1
    );
}

#[test]
fn local_file_change_updates_only_the_changed_component() {
    let directory = tempfile::tempdir().expect("temporary Local source");
    let music_root = directory.path().join("music");
    std::fs::create_dir(&music_root).expect("create Local music folder");
    let music_root = std::fs::canonicalize(music_root).expect("canonical Local music folder");
    std::fs::write(music_root.join("First.mp3"), []).expect("write first Local Track");
    let other_directory = music_root.join("Other");
    std::fs::create_dir(&other_directory).expect("create unrelated Local directory");
    std::fs::write(other_directory.join("Outside.mp3"), []).expect("write unrelated Local Track");
    let runtime = test_runtime();
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
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let accepted = runtime
        .block_on(Arc::clone(&source).prepare_library_candidate(
            libraries,
            identity,
            None,
            Arc::new(|_: SourceReadProgress| {}),
            Arc::new(AtomicBool::new(false)),
        ))
        .map_err(string_error)
        .and_then(|candidate| candidate.accept().map_err(string_error))
        .expect("accept initial Local source")
        .library;
    assert_eq!(
        accepted
            .track_list(None, TrackSort::Title, false)
            .expect("read initial Tracks")
            .len(),
        2
    );

    let second_path = music_root.join("Second.mp3");
    std::fs::write(&second_path, []).expect("write changed Local Track");
    let replacement = runtime
        .block_on(prepare_local_change(
            Arc::clone(&source),
            Arc::clone(&accepted),
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
    let changed = accepted
        .accept_local_component(replacement)
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
            .track_list(None, TrackSort::Title, false)
            .expect("read changed Tracks")
            .len(),
        3
    );

    let unchanged = runtime
        .block_on(prepare_local_change(
            source,
            accepted,
            LocalFilesystemChange::Rescan,
            Arc::new(AtomicBool::new(false)),
        ))
        .expect("verify unchanged Local source");
    assert!(unchanged.is_none());
}

#[test]
fn local_metadata_edit_prepares_the_written_file_for_library_acceptance() {
    let directory = tempfile::tempdir().expect("temporary Local source");
    let music_root = directory.path().join("music");
    std::fs::create_dir(&music_root).expect("create Local music folder");
    let path = music_root.join("Before.wav");
    write_silent_wav(&path).expect("write WAV");
    let runtime = test_runtime();
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
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let accepted = runtime
        .block_on(Arc::clone(&source).prepare_library_candidate(
            libraries,
            identity,
            None,
            Arc::new(|_: SourceReadProgress| {}),
            Arc::new(AtomicBool::new(false)),
        ))
        .map_err(string_error)
        .and_then(|candidate| candidate.accept().map_err(string_error))
        .expect("accept initial Local source")
        .library;
    let edited_track = accepted
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
    let replacement = source
        .prepare_local_change(&accepted, change, 1, &|_| {}, &|| false)
        .expect("prepare Local metadata replacement")
        .expect("written Local metadata changed");
    let change = accepted
        .accept_local_component(replacement)
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
            .track(&edited_track.id)
            .expect("read accepted Track")
            .expect("accepted Track")
            .title,
        "After"
    );
}

#[test]
fn private_mode_still_uses_source_metadata_search() {
    let runtime = test_runtime();
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
    let runtime = test_runtime();
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
    let runtime = test_runtime();
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
    let runtime = test_runtime();
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
fn failed_metadata_access_setting_save_restores_the_accepted_mapping() {
    let directory = tempfile::tempdir().expect("temporary Local access transaction");
    let store_path = directory.path().join("library.db");
    let libraries = Libraries::open(&store_path).expect("open Library");
    let source_id = SourceId::new("navidrome:server:local-access-transaction");
    let track_id = library::TrackId::new("navidrome:track:local-access-transaction");
    let library = accept_library(
        &libraries,
        source_id.clone(),
        vec![test_track(
            track_id.clone(),
            "Track",
            PathBuf::from("/server/music/Artist/Track.wav"),
            None,
        )],
        Vec::new(),
        1,
    );
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
            configured_local_access_mapping(&previous_access),
            previous_files.clone(),
        )
        .expect("accept previous Local access");

    let proposed_root = directory.path().join("proposed");
    let error = accept_metadata_local_access_mapping(
        &library,
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
        library
            .local_access_files()
            .expect("read restored Local access"),
        previous_files
    );
    let (_, targets) = library
        .metadata_subject_with_local_access(&MetadataItemId::Track(track_id), None)
        .expect("resolve restored Local access")
        .expect("restored metadata Track");
    assert_eq!(
        targets
            .first()
            .expect("previous Local access remains accepted")
            .path(),
        previous_path
    );

    drop(library);
    drop(libraries);
    let reopened = Libraries::open(store_path)
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
fn standardized_results_reuse_accepted_track_facts_without_a_source_mirror() {
    let directory = tempfile::tempdir().expect("temporary Rufin data directory");
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let track_id = library::TrackId::new("navidrome:track:known");
    let accepted_track = test_track(
        track_id.clone(),
        "Accepted",
        PathBuf::from("/music/Artist/Accepted.flac"),
        None,
    );
    let library = accept_library(
        &libraries,
        SourceId::new("navidrome:server:test"),
        vec![accepted_track],
        Vec::new(),
        1,
    );
    let reported = test_track(
        track_id,
        "Reported",
        PathBuf::from("generated/Reported.flac"),
        None,
    );
    let unknown = test_track(
        library::TrackId::new("navidrome:track:unknown"),
        "Unknown",
        PathBuf::from("generated/Unknown.flac"),
        None,
    );

    let search = reconcile_search_results(
        &library,
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
        &library,
        FolderContents {
            folders: Arc::from([]),
            tracks: vec![reported].into(),
        },
    )
    .expect("reconcile folder");
    assert_eq!(folder.tracks[0].title, "Accepted");
}

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime")
}

fn test_owner(
    directory: &Path,
    runtime: &tokio::runtime::Runtime,
    libraries: Libraries,
    settings: SettingsFile,
) -> (SourceBootstrap, async_channel::Receiver<SourceEvent>) {
    let (bootstrap, events, _download_events) =
        test_owner_with_download_events(directory, runtime, libraries, settings);
    (bootstrap, events)
}

fn test_owner_with_download_events(
    directory: &Path,
    runtime: &tokio::runtime::Runtime,
    libraries: Libraries,
    settings: SettingsFile,
) -> (
    SourceBootstrap,
    async_channel::Receiver<SourceEvent>,
    async_channel::Receiver<downloads::DownloadEvent>,
) {
    let artwork = artwork::Artwork::new(directory.join("artwork"), runtime.handle().clone())
        .expect("open Artwork");
    let (events, event_receiver) = async_channel::unbounded();
    let (discovery, _discovery_receiver) = async_channel::unbounded();
    let (download_events, download_event_receiver) = async_channel::unbounded();
    let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let scrobbler = Arc::new(
        Scrobbler::new(libraries.clone(), ::scrobbling::Settings::default(), false)
            .expect("open Scrobbler"),
    );
    let bootstrap = SourceOwner::open_dormant(
        artwork,
        libraries,
        downloads::Downloads::new(
            directory.join("downloads"),
            runtime.handle().clone(),
            download_events,
            Vec::new(),
        ),
        settings,
        secrets,
        scrobbler,
        runtime.handle().clone(),
        SourceOutputs { events, discovery },
    );
    (bootstrap, event_receiver, download_event_receiver)
}

#[derive(Default)]
struct AcceptingPlaybackBackend;

impl ::playback::PlaybackBackend for AcceptingPlaybackBackend {
    fn send(
        &mut self,
        _command: ::playback::BackendCommand,
    ) -> Result<(), ::playback::BackendError> {
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<::playback::BackendEvent> {
        Vec::new()
    }
}

fn attach_test_playback(
    owner: &Arc<SourceOwner>,
    runtime: &tokio::runtime::Runtime,
    directory: &Path,
) -> Arc<PlaybackOwner> {
    let (playback_events, _playback_event_receiver) = async_channel::unbounded();
    let (waveform_events, _waveform_event_receiver) = async_channel::unbounded();
    let waveform = crate::waveform::WaveformOwner::new(
        runtime.handle().clone(),
        waveform_events,
        directory.join("waveforms"),
        false,
    );
    let (lyrics_events, _lyrics_event_receiver) = async_channel::unbounded();
    let stored = owner.shared.settings.load();
    let lyrics = ::lyrics::LyricsService::new(
        owner.shared.library.clone(),
        runtime.handle().clone(),
        stored.ui.lyrics,
        stored.ui.private_mode,
        lyrics_events,
    );
    let playback = PlaybackOwner::new(
        owner.shared.library.clone(),
        owner.shared.settings.clone(),
        runtime.handle().clone(),
        playback_events,
        owner.acceptance_sender(),
        waveform,
        lyrics,
        Arc::new(desktop_integration::Discord::new()),
        Arc::clone(&owner.shared.scrobbler),
        || Ok(Box::<AcceptingPlaybackBackend>::default()),
    );
    owner.attach_playback(&playback);
    playback
}

fn install_selected_for_test(
    owner: &Arc<SourceOwner>,
    configuration: SourceConfiguration,
    source: Option<Arc<Source>>,
    library: Arc<Library>,
    epoch: SourceSessionEpoch,
) -> Arc<ActiveSource> {
    let home = library.home(None).expect("prepare selected Home");
    let selected = Arc::new(SelectedSourceState {
        configuration,
        source,
        source_session_epoch: epoch,
        library,
        home,
        music_folder_id: None,
    });
    let session = ActiveSource::new(&owner.shared, &selected);
    owner
        .shared
        .install_selected_slot(Arc::clone(&session), selected);
    session
}

fn accept_library(
    libraries: &Libraries,
    source_id: SourceId,
    tracks: Vec<Track>,
    music_folders: Vec<MusicFolder>,
    digest: u8,
) -> Arc<Library> {
    let mut candidate = libraries
        .begin_source_candidate(CandidateHeader {
            source_id,
            input_version: 1,
            input_digest: [digest; 32],
        })
        .expect("begin source candidate");
    if !tracks.is_empty() {
        candidate
            .write(CandidateBatch::Tracks(tracks))
            .expect("write candidate Tracks");
    }
    if !music_folders.is_empty() {
        candidate
            .write(CandidateBatch::MusicFolders(music_folders))
            .expect("write candidate music folders");
    }
    candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: i64::from(digest),
            },
            None,
        )
        .and_then(|candidate| candidate.accept())
        .expect("accept source candidate")
        .library
}

fn test_configuration(source_id: SourceId, name: &str) -> SourceConfiguration {
    SourceConfiguration {
        source_id,
        kind: "local".to_string(),
        name: name.to_string(),
        provider_payload: serde_json::json!({
            "version": 1,
            "roots": [],
        })
        .to_string(),
    }
}

fn test_track(
    id: library::TrackId,
    title: &str,
    path: PathBuf,
    music_folder: Option<MusicFolderId>,
) -> Track {
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
        relations: TrackRelations {
            music_folders: music_folder.into_iter().collect(),
            ..TrackRelations::default()
        },
    })
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
