use super::*;
use library::{
    CandidateBatch, CandidateFinish, CandidateHeader, HomeFacts, Libraries, Track, TrackData,
    TrackRelations,
};
use proptest::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::mpsc;

fn test_actor(root: &Path) -> Actor {
    Actor {
        root: Arc::new(root.to_path_buf()),
        events: async_channel::unbounded().0,
        transfers: Arc::new(TransferClients::default()),
        prepared_rules: async_channel::unbounded().0,
        attached: HashMap::new(),
        selected: None,
        settings: HashMap::new(),
        running_rules: None,
        pending_rules: None,
        jobs: HashMap::new(),
        paused: false,
        next_job: 0,
    }
}

fn accepted_track(root: &Path, source_id: SourceId, track_id: TrackId) -> (Arc<Library>, Track) {
    let (loaded, mut tracks) = accepted_tracks(root, source_id, vec![track_id]);
    (loaded, tracks.remove(0))
}

fn accepted_tracks(
    root: &Path,
    source_id: SourceId,
    track_ids: Vec<TrackId>,
) -> (Arc<Library>, Vec<Track>) {
    let library = Libraries::open(root.join("library.db")).expect("open test Library");
    let tracks = track_ids
        .into_iter()
        .enumerate()
        .map(|(index, track_id)| {
            Track::new(TrackData {
                id: track_id,
                album_id: None,
                title: format!("Offline track {index}"),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                album_artwork: None,
                year: 0,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                duration_seconds: 180,
                favorite: false,
                disc_number: 1,
                track_number: index as u16 + 1,
                image_ref: None,
                local_artwork: None,
                musicbrainz_recording_id: None,
                musicbrainz_release_track_id: None,
                source_path: None,
                cue: None,
                source_format: Some("flac".to_string()),
                comment: None,
                skip_count: None,
                bpm: None,
                relations: TrackRelations::default(),
            })
        })
        .collect::<Vec<_>>();
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id,
            input_digest: [4; 32],
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(tracks.clone()))
        .expect("write track");
    let loaded = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(library::PreparedSourceCandidate::accept)
        .expect("accept source")
        .library;
    (loaded, tracks)
}

#[test]
fn rule_reconciliation_follows_the_changed_collection_facts() {
    let rules = DownloadRules {
        entire_library: true,
        favorites: true,
        all_playlists: true,
        latest_five_albums: true,
    };
    let tracks = AcceptedLibraryChange {
        tracks: vec![library::AcceptedTrackReplacement {
            id: TrackId::fake(1),
            track: None,
            activity_only: false,
        }],
        ..AcceptedLibraryChange::default()
    };
    assert_eq!(affected_rules(rules, &tracks), rules);

    let albums = AcceptedLibraryChange {
        albums: vec![library::AlbumId::fake(1)],
        ..AcceptedLibraryChange::default()
    };
    assert_eq!(
        affected_rules(rules, &albums),
        DownloadRules {
            favorites: true,
            latest_five_albums: true,
            ..DownloadRules::default()
        }
    );

    let playlists = AcceptedLibraryChange {
        playlists: vec![library::PlaylistId::fake(1)],
        ..AcceptedLibraryChange::default()
    };
    assert_eq!(
        affected_rules(rules, &playlists),
        DownloadRules {
            all_playlists: true,
            ..DownloadRules::default()
        }
    );
}

#[tokio::test]
async fn remove_all_disables_rules_before_clearing_downloads() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let library_root = tempfile::tempdir().expect("temporary Library");
    let source_id = SourceId::fake(1);
    let track_id = TrackId::fake(1);
    let (loaded, _) = accepted_track(library_root.path(), source_id.clone(), track_id.clone());
    let rules = DownloadRules {
        favorites: true,
        ..DownloadRules::default()
    };
    let mut actor = test_actor(directory.path());
    actor.settings.insert(
        source_id.clone(),
        SourceDownloadSettings {
            source_id: source_id.clone(),
            rules,
            quality: StreamQuality::Original,
            directory: None,
        },
    );
    actor.attached.insert(
        source_id.clone(),
        AttachedSource {
            source: None,
            loaded: Arc::downgrade(&loaded),
            music_folder_id: None,
            directory: None,
        },
    );
    actor.selected = Some(Arc::downgrade(&loaded));
    actor.running_rules = Some(RuleIntent {
        loaded: Arc::clone(&loaded),
        music_folder_id: None,
        rules,
    });
    let mut active = Vec::new();

    actor
        .apply(
            Command::Clear {
                source_id: source_id.clone(),
                loaded: Some(Arc::downgrade(&loaded)),
                notify: false,
            },
            &mut active,
        )
        .await;

    assert!(actor.settings_for(&source_id).rules.is_empty());
    assert!(
        actor
            .pending_rules
            .as_ref()
            .is_some_and(|intent| intent.rules.is_empty())
    );
    actor
        .apply_prepared_rules(
            Ok(vec![(DownloadRule::Favorites, vec![track_id])]),
            &mut active,
        )
        .await;
    assert!(actor.jobs.get(&source_id).is_none_or(Vec::is_empty));
    assert!(actor.running_rules.is_none());
    assert!(actor.pending_rules.is_none());
}

#[test]
fn selected_commands_require_the_exact_attached_library() {
    let actor_root = tempfile::tempdir().expect("actor root");
    let previous_root = tempfile::tempdir().expect("previous Library");
    let current_root = tempfile::tempdir().expect("current Library");
    let source_id = SourceId::fake(1);
    let (previous, _) = accepted_track(previous_root.path(), source_id.clone(), TrackId::fake(1));
    let (current, _) = accepted_track(current_root.path(), source_id.clone(), TrackId::fake(1));
    let mut actor = test_actor(actor_root.path());
    actor.attached.insert(
        source_id.clone(),
        AttachedSource {
            source: None,
            loaded: Arc::downgrade(&current),
            music_folder_id: None,
            directory: None,
        },
    );
    actor.selected = Some(Arc::downgrade(&current));

    assert_eq!(
        actor.attached_source_id(&Arc::downgrade(&current)),
        Some(source_id)
    );
    assert_eq!(actor.attached_source_id(&Arc::downgrade(&previous)), None);
}

#[tokio::test]
async fn failed_attachment_still_retires_the_previous_library() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let blocked_root = directory.path().join("not-a-directory");
    std::fs::write(&blocked_root, b"file").expect("create unusable downloads root");
    let previous_root = tempfile::tempdir().expect("previous Library");
    let target_root = tempfile::tempdir().expect("target Library");
    let previous_id = SourceId::fake(1);
    let target_id = SourceId::fake(2);
    let (previous, _) = accepted_track(previous_root.path(), previous_id.clone(), TrackId::fake(1));
    let (target, _) = accepted_track(target_root.path(), target_id.clone(), TrackId::fake(2));
    let mut actor = test_actor(&blocked_root);
    actor.attached.insert(
        previous_id,
        AttachedSource {
            source: None,
            loaded: Arc::downgrade(&previous),
            music_folder_id: None,
            directory: None,
        },
    );
    actor.selected = Some(Arc::downgrade(&previous));
    let (response, result) = async_channel::bounded(1);
    let mut active = Vec::new();

    actor
        .apply(
            Command::Attach {
                source: None,
                loaded: Arc::downgrade(&target),
                music_folder_id: None,
                response,
            },
            &mut active,
        )
        .await;

    assert!(result.recv().await.expect("attachment result").is_err());
    assert_eq!(
        actor.attached_source_id(&Arc::downgrade(&target)),
        Some(target_id)
    );
    assert_eq!(actor.attached_source_id(&Arc::downgrade(&previous)), None);
}

#[tokio::test]
async fn pause_and_source_replacement_are_admitted_during_selection_preparation() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let replacement_directory = tempfile::tempdir().expect("replacement Library");
    let source_id = SourceId::fake(1);
    let (loaded, _) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(1));
    let (replacement, _) = accepted_track(
        replacement_directory.path(),
        SourceId::fake(2),
        TrackId::fake(2),
    );
    let selection: TrackSelection = loaded
        .track_list(None, TrackSort::Title, false)
        .expect("selected tracks")
        .into();
    let (commands, prepared) = async_channel::unbounded();
    let (started, preparation_started) = async_channel::bounded(1);
    let (release_preparation, release) = async_channel::bounded(1);
    let mut actor = test_actor(directory.path());
    actor.attached.insert(
        source_id.clone(),
        AttachedSource {
            source: None,
            loaded: Arc::downgrade(&loaded),
            music_folder_id: None,
            directory: None,
        },
    );
    actor.selected = Some(Arc::downgrade(&loaded));
    let mut active = Vec::new();
    let selected_library = Arc::downgrade(&loaded);
    prepare_command(
        tokio::runtime::Handle::current(),
        commands,
        move || {
            started.send_blocking(()).expect("report blocked work");
            release.recv_blocking().expect("release blocked work");
            selection
                .prepare()
                .and_then(|tracks| tracks.track_ids())
                .map(|track_ids| track_ids.to_vec())
        },
        move |track_ids| Command::Download {
            loaded: selected_library,
            subject: DownloadSubject::Track(TrackId::fake(1)),
            track_ids,
        },
    );
    preparation_started
        .recv()
        .await
        .expect("selection preparation started");

    actor.apply(Command::SetPaused(true), &mut active).await;
    assert!(actor.paused);
    let (response, attached) = async_channel::bounded(1);
    actor
        .apply(
            Command::Attach {
                source: None,
                loaded: Arc::downgrade(&replacement),
                music_folder_id: None,
                response,
            },
            &mut active,
        )
        .await;
    assert!(attached.recv().await.expect("attachment result").is_ok());

    release_preparation
        .send(())
        .await
        .expect("release selection preparation");
    let prepared = prepared.recv().await.expect("prepared selection command");
    actor.apply(prepared, &mut active).await;
    assert!(!actor.jobs.contains_key(&source_id));
}

fn retained_record(source_id: SourceId, track_id: TrackId) -> DownloadRecord {
    DownloadRecord {
        version: RECORD_VERSION,
        source_id,
        track_id,
        owners: HashSet::from([DownloadOwner::Retained]),
        audio_root: None,
        audio_path: None,
    }
}

fn download_job(
    id: &str,
    subject: DownloadSubject,
    remaining: Vec<TrackId>,
    state: DownloadQueueState,
) -> DownloadJob {
    DownloadJob {
        id: id.to_string(),
        subject,
        quality: StreamQuality::Original,
        total_tracks: remaining.len(),
        completed: Vec::new(),
        remaining,
        state,
    }
}

fn remote_source(base_url: &str) -> Arc<Source> {
    Arc::new(
        Source::open(
            sources::SourceConfiguration {
                source_id: SourceId::new("configured:jellyfin"),
                kind: "jellyfin".to_string(),
                name: "Server".to_string(),
                provider_payload: serde_json::json!({
                    "version": 1,
                    "base_url": base_url,
                    "server_id": null,
                    "user_id": "account",
                    "username": "listener",
                    "trust_invalid_cert": false,
                    "use_jellyfin_instant_mix": false,
                })
                .to_string(),
            },
            Some("secret-token".to_string()),
            Some("device-one".to_string()),
        )
        .expect("open remote source"),
    )
}

fn pending_download(
    root: &Path,
    source_id: SourceId,
    job_id: &str,
    track_id: TrackId,
) -> ActiveDownload {
    let paths = download_paths(root, &source_id, &track_id);
    let (cancellation, cancelled) = tokio::sync::oneshot::channel();
    ActiveDownload {
        source_id,
        job_id: job_id.to_string(),
        subject: DownloadSubject::Track(track_id.clone()),
        track_id,
        paths,
        cancellation: Some(cancellation),
        task: tokio::spawn(async move {
            let _ = cancelled.await;
            Err(DownloadFailure::Retry("cancelled".to_string()))
        }),
    }
}

proptest! {
    #[test]
    fn reordering_keeps_every_job_once_and_places_the_moved_job_next_to_its_target(
        count in 2usize..32,
        source_seed in 0usize..32,
        target_seed in 0usize..32,
        after in any::<bool>(),
    ) {
        let mut jobs = (0..count)
            .map(|index| {
                download_job(
                    &format!("job-{index}"),
                    DownloadSubject::Track(TrackId::fake(index)),
                    vec![TrackId::fake(index)],
                    DownloadQueueState::Queued,
                )
            })
            .collect::<Vec<_>>();
        let source_index = source_seed % count;
        let mut target_index = target_seed % (count - 1);
        if target_index >= source_index {
            target_index += 1;
        }
        let job_id = jobs[source_index].id.clone();
        let target_job_id = jobs[target_index].id.clone();
        let expected = jobs.iter().map(|job| job.id.clone()).collect::<HashSet<_>>();

        prop_assert!(reorder_jobs(&mut jobs, &job_id, &target_job_id, after));

        prop_assert_eq!(
            jobs.iter().map(|job| job.id.clone()).collect::<HashSet<_>>(),
            expected
        );
        prop_assert_eq!(jobs.len(), count);
        let moved = jobs
            .iter()
            .position(|job| job.id == job_id)
            .expect("moved job");
        let target = jobs
            .iter()
            .position(|job| job.id == target_job_id)
            .expect("target job");
        if after {
            prop_assert_eq!(moved, target + 1);
        } else {
            prop_assert_eq!(moved + 1, target);
        }
    }

}

#[tokio::test]
async fn one_playlist_opens_three_response_bodies() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind download server");
    let address = listener.local_addr().expect("download server address");
    let (requests, received) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut streams = Vec::new();
        for _ in 0..MAX_ACTIVE_DOWNLOADS {
            let (mut stream, _) = listener.accept().expect("accept download request");
            let mut request = [0; 4096];
            let received = stream.read(&mut request).expect("read download request");
            assert!(received > 0);
            stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\n")
                    .expect("write download headers");
            stream.flush().expect("flush download headers");
            requests.send(()).expect("record download request");
            streams.push(stream);
        }
        released.recv().expect("release download bodies");
        for mut stream in streams {
            let _ = stream.write_all(b"x");
        }
    });
    let source_id = SourceId::fake(1);
    let track_ids = (0..4).map(TrackId::fake).collect::<Vec<_>>();
    let (loaded, _) = accepted_tracks(directory.path(), source_id.clone(), track_ids.clone());
    let source = remote_source(&format!("http://{address}"));
    let mut actor = test_actor(directory.path());
    actor.attached.insert(
        source_id.clone(),
        AttachedSource {
            source: Some(Arc::downgrade(&source)),
            loaded: Arc::downgrade(&loaded),
            music_folder_id: None,
            directory: None,
        },
    );
    actor.jobs.insert(
        source_id.clone(),
        vec![download_job(
            "playlist",
            DownloadSubject::Playlist(library::PlaylistId::fake(1)),
            track_ids,
            DownloadQueueState::Queued,
        )],
    );
    let mut active = Vec::new();

    actor.fill_slots(&mut active).await;

    assert_eq!(active.len(), MAX_ACTIVE_DOWNLOADS);
    tokio::task::spawn_blocking(move || {
        for _ in 0..MAX_ACTIVE_DOWNLOADS {
            received
                .recv_timeout(Duration::from_secs(5))
                .expect("parallel download request");
        }
    })
    .await
    .expect("wait for parallel download requests");
    release.send(()).expect("release download bodies");
    actor.abort_matching(&mut active, false, |_| true).await;
    server.join().expect("download server");
}

#[tokio::test]
async fn attach_removes_downloads_absent_from_the_accepted_source() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let track_id = TrackId::fake(2);
    let paths = download_paths(directory.path(), &source_id, &track_id);
    std::fs::create_dir_all(&paths.directory).expect("download source directory");
    std::fs::write(&paths.audio, b"audio").expect("download audio");
    let record = retained_record(source_id.clone(), track_id);
    std::fs::write(
        &paths.record,
        serde_json::to_vec(&record).expect("encode record"),
    )
    .expect("download record");
    let library = Libraries::open(directory.path().join("library.db")).expect("open test Library");
    let loaded = library
        .begin_source_candidate(CandidateHeader {
            source_id,
            input_digest: [1; 32],
        })
        .expect("begin source candidate")
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept empty source")
        .library;

    for stale in attach_downloaded_files(directory.path(), &loaded).expect("attach downloads") {
        remove_download_files(&stale)
            .await
            .expect("remove download");
    }

    assert!(!paths.audio.exists());
    assert!(!paths.record.exists());
}

#[tokio::test]
async fn an_unreadable_initial_queue_keeps_staging() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let track_id = TrackId::fake(2);
    let (loaded, _) = accepted_track(directory.path(), source_id.clone(), track_id.clone());
    let paths = staging_paths(directory.path(), &source_id, &track_id, None);
    let mut checkpoint = paths.audio_part.as_os_str().to_os_string();
    checkpoint.push(".resume");
    let checkpoint = PathBuf::from(checkpoint);
    std::fs::create_dir_all(&paths.directory).expect("download source directory");
    std::fs::write(&paths.audio_part, b"partial").expect("partial download");
    std::fs::write(
        &checkpoint,
        br#"{"representation":"same","validator":"\"v1\"","length":7}"#,
    )
    .expect("resume checkpoint");
    let queue = source_directory(directory.path(), &source_id).join(QUEUE_FILE);
    std::fs::write(&queue, b"not a queue").expect("corrupt queue");
    let mut actor = test_actor(directory.path());

    assert!(
        actor
            .attach(source_id.clone(), None, Arc::clone(&loaded), None, None)
            .await
            .is_err()
    );

    assert!(
        actor
            .attached
            .get(&source_id)
            .and_then(|attached| attached.loaded.upgrade())
            .is_some()
    );
    assert!(!actor.jobs.contains_key(&source_id));
    assert!(paths.audio_part.is_file());
    assert!(checkpoint.is_file());
    assert_eq!(std::fs::read(queue).expect("saved queue"), b"not a queue");

    assert!(
        actor
            .attach(source_id.clone(), None, Arc::clone(&loaded), None, None)
            .await
            .is_err()
    );
    assert!(!actor.jobs.contains_key(&source_id));
    assert!(paths.audio_part.is_file());
    assert!(checkpoint.is_file());
}

#[tokio::test]
async fn reattaching_keeps_the_live_queue_when_disk_is_empty() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let track_id = TrackId::fake(2);
    let (loaded, _) = accepted_track(directory.path(), source_id.clone(), track_id.clone());
    let mut actor = test_actor(directory.path());
    actor.jobs.insert(
        source_id.clone(),
        vec![download_job(
            "live",
            DownloadSubject::Track(track_id.clone()),
            vec![track_id],
            DownloadQueueState::WaitingForConnection,
        )],
    );
    actor
        .attach(source_id.clone(), None, Arc::clone(&loaded), None, None)
        .await
        .expect("reattach downloads");

    assert_eq!(actor.jobs[&source_id][0].id, "live");
    assert_eq!(
        load_queue(directory.path(), &source_id).expect("saved live queue")[0].id,
        "live"
    );
}

#[tokio::test]
async fn replacing_a_source_does_not_commit_its_old_transfer() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::new("configured:jellyfin");
    let track_id = TrackId::fake(1);
    let (loaded, _) = accepted_track(directory.path(), source_id.clone(), track_id.clone());
    let old_source = remote_source("http://old.invalid");
    let new_source = remote_source("http://new.invalid");
    let paths = download_paths(directory.path(), &source_id, &track_id);
    std::fs::create_dir_all(&paths.directory).expect("download source directory");
    std::fs::write(&paths.audio_part, b"old source audio").expect("completed transfer");

    let mut actor = test_actor(directory.path());
    actor.attached.insert(
        source_id.clone(),
        AttachedSource {
            source: Some(Arc::downgrade(&old_source)),
            loaded: Arc::downgrade(&loaded),
            music_folder_id: None,
            directory: None,
        },
    );
    actor.jobs.insert(
        source_id.clone(),
        vec![download_job(
            "active",
            DownloadSubject::Track(track_id.clone()),
            vec![track_id.clone()],
            DownloadQueueState::Downloading,
        )],
    );
    let (cancellation, _cancelled) = tokio::sync::oneshot::channel();
    let mut active = vec![ActiveDownload {
        source_id: source_id.clone(),
        job_id: "active".to_string(),
        track_id: track_id.clone(),
        subject: DownloadSubject::Track(track_id),
        paths: paths.clone(),
        cancellation: Some(cancellation),
        task: tokio::spawn(async { Ok(()) }),
    }];
    let (response, _result) = async_channel::bounded(1);

    actor
        .apply(
            Command::Attach {
                source: Some(Arc::downgrade(&new_source)),
                loaded: Arc::downgrade(&loaded),
                music_folder_id: None,
                response,
            },
            &mut active,
        )
        .await;

    assert!(active.is_empty());
    assert!(!paths.audio.exists());
    assert!(!paths.record.exists());
    assert!(paths.audio_part.exists());
    assert_eq!(actor.jobs[&source_id][0].state, DownloadQueueState::Queued);
    assert!(Arc::ptr_eq(
        &actor.attached[&source_id]
            .source
            .as_ref()
            .and_then(Weak::upgrade)
            .expect("new source remains attached"),
        &new_source,
    ));
}

#[test]
fn attached_downloads_do_not_own_the_library_lifecycle() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let (loaded, _) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(2));
    let loaded_weak = Arc::downgrade(&loaded);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");
    let (events, receiver) = async_channel::unbounded();
    let downloads = Downloads::new(
        directory.path().join("downloads"),
        runtime.handle().clone(),
        events,
        Vec::new(),
    );

    runtime
        .block_on(downloads.attach(None, &loaded, None))
        .expect("attach downloaded files");
    let event = runtime.block_on(receiver.recv()).expect("download queue");
    assert!(matches!(
        event,
        DownloadEvent::Queue {
            source_id: event_source,
            ..
        } if event_source == source_id
    ));

    drop(loaded);
    assert!(
        loaded_weak.upgrade().is_none(),
        "Downloads must not retain an inactive Library"
    );
}

#[tokio::test]
async fn pause_preserves_the_queue_and_partial_transfer_until_continue() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::new("configured:jellyfin");
    let track_id = TrackId::fake(2);
    let queued_track_id = TrackId::fake(3);
    let (loaded, mut tracks) = accepted_tracks(
        directory.path(),
        source_id.clone(),
        vec![track_id.clone(), queued_track_id],
    );
    let queued_track = tracks.remove(1);
    let source = remote_source("http://127.0.0.1:9");
    let paths = download_paths(directory.path(), &source_id, &track_id);
    std::fs::create_dir_all(&paths.directory).expect("download source directory");
    std::fs::write(&paths.audio_part, b"partial").expect("partial download");
    let mut actor = test_actor(directory.path());
    actor.jobs.insert(
        source_id.clone(),
        vec![download_job(
            "active",
            DownloadSubject::Track(track_id.clone()),
            vec![track_id.clone()],
            DownloadQueueState::Downloading,
        )],
    );
    actor.attached.insert(
        source_id.clone(),
        AttachedSource {
            source: Some(Arc::downgrade(&source)),
            loaded: Arc::downgrade(&loaded),
            music_folder_id: None,
            directory: None,
        },
    );
    let mut active = vec![pending_download(
        directory.path(),
        source_id.clone(),
        "active",
        track_id,
    )];

    actor.apply(Command::SetPaused(true), &mut active).await;

    assert!(actor.paused);
    assert!(active.is_empty());
    assert!(paths.audio_part.exists());
    assert_eq!(actor.jobs[&source_id][0].state, DownloadQueueState::Queued);
    actor.fill_slots(&mut active).await;
    assert!(active.is_empty());
    actor
        .enqueue(
            source_id.clone(),
            DownloadSubject::Playlist(library::PlaylistId::fake(1)),
            StreamQuality::Original,
            vec![queued_track.id.clone()],
        )
        .await;
    assert_eq!(
        actor.jobs[&source_id]
            .iter()
            .find(|job| { job.subject == DownloadSubject::Playlist(library::PlaylistId::fake(1)) })
            .expect("connected paused queue")
            .state,
        DownloadQueueState::Queued
    );

    actor.apply(Command::SetPaused(false), &mut active).await;
    assert!(!actor.paused);
}

#[tokio::test]
async fn cancel_interrupts_only_its_transfer_and_keeps_staging_still_in_demand() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let track_id = TrackId::fake(2);
    let other_track_id = TrackId::fake(3);
    let job_id = "active".to_string();
    let other_job_id = "other".to_string();
    let overlap_job_id = "overlap".to_string();
    let paths = download_paths(directory.path(), &source_id, &track_id);
    let other_paths = download_paths(directory.path(), &source_id, &other_track_id);
    std::fs::create_dir_all(&paths.directory).expect("download source directory");
    std::fs::write(&paths.audio_part, b"partial").expect("partial download");
    std::fs::write(
        &paths.checkpoint,
        br#"{"representation":"same","validator":"\"v1\"","length":7}"#,
    )
    .expect("resume checkpoint");
    std::fs::write(&other_paths.audio_part, b"other partial").expect("other partial download");
    let mut actor = test_actor(directory.path());
    actor.jobs.insert(
        source_id.clone(),
        vec![
            download_job(
                &job_id,
                DownloadSubject::Track(track_id.clone()),
                vec![track_id.clone()],
                DownloadQueueState::Downloading,
            ),
            download_job(
                &other_job_id,
                DownloadSubject::Track(other_track_id.clone()),
                vec![other_track_id.clone()],
                DownloadQueueState::Downloading,
            ),
            download_job(
                &overlap_job_id,
                DownloadSubject::Rule(DownloadRule::Favorites),
                vec![track_id.clone()],
                DownloadQueueState::Queued,
            ),
        ],
    );
    let mut active = vec![
        pending_download(directory.path(), source_id.clone(), &job_id, track_id),
        pending_download(
            directory.path(),
            source_id.clone(),
            &other_job_id,
            other_track_id,
        ),
    ];
    actor
        .apply(
            Command::Cancel {
                source_id: source_id.clone(),
                job_id,
            },
            &mut active,
        )
        .await;

    assert_eq!(active.len(), 1);
    assert_eq!(active[0].job_id, other_job_id);
    assert_eq!(
        actor
            .jobs
            .get(&source_id)
            .expect("remaining source queue")
            .iter()
            .map(|job| job.id.as_str())
            .collect::<Vec<_>>(),
        ["other", "overlap"]
    );
    assert!(paths.audio_part.exists());
    assert!(paths.checkpoint.exists());
    assert!(other_paths.audio_part.exists());

    actor
        .cancel(&source_id, &overlap_job_id, &mut Vec::new())
        .await;

    assert!(!paths.audio_part.exists());
    assert!(!paths.checkpoint.exists());
    actor.abort_matching(&mut active, false, |_| true).await;
}

#[tokio::test]
async fn cancel_keeps_a_completion_while_clear_removes_it() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let first_track = TrackId::fake(2);
    let second_track = TrackId::fake(3);
    let subject = DownloadSubject::Playlist(library::PlaylistId::fake(1));
    let job_id = "playlist";
    let second_paths = download_paths(directory.path(), &source_id, &second_track);
    std::fs::create_dir_all(&second_paths.directory).expect("download source directory");
    std::fs::write(&second_paths.audio_part, b"second audio").expect("completed transfer");
    let mut actor = test_actor(directory.path());
    actor.jobs.insert(
        source_id.clone(),
        vec![download_job(
            job_id,
            subject.clone(),
            vec![first_track.clone(), second_track.clone()],
            DownloadQueueState::Downloading,
        )],
    );
    let mut remaining_active = vec![pending_download(
        directory.path(),
        source_id.clone(),
        job_id,
        first_track.clone(),
    )];
    let (cancellation, _cancelled) = tokio::sync::oneshot::channel();
    let completed = ActiveDownload {
        source_id: source_id.clone(),
        job_id: job_id.to_string(),
        track_id: second_track.clone(),
        subject: subject.clone(),
        paths: second_paths.clone(),
        cancellation: Some(cancellation),
        task: tokio::spawn(async { Ok(()) }),
    };

    actor
        .finish(completed, Ok(Ok(())), &mut remaining_active)
        .await;

    let job = &actor.jobs.get(&source_id).expect("source queue")[0];
    assert_eq!(job.remaining, [first_track]);
    assert!(second_paths.audio.is_file());
    assert!(second_paths.record.is_file());
    assert_eq!(remaining_active.len(), 1);
    actor
        .apply(
            Command::Cancel {
                source_id: source_id.clone(),
                job_id: job_id.to_string(),
            },
            &mut remaining_active,
        )
        .await;

    assert!(remaining_active.is_empty());
    assert!(actor.jobs.get(&source_id).is_none_or(Vec::is_empty));
    assert!(second_paths.audio.exists());
    assert!(second_paths.record.exists());

    let mut clear_job = download_job(
        "clear",
        subject,
        vec![TrackId::fake(4)],
        DownloadQueueState::Queued,
    );
    clear_job.total_tracks = 2;
    clear_job.completed.push(second_track);
    actor.jobs.insert(source_id.clone(), vec![clear_job]);
    actor.clear_job(&source_id, "clear", &mut Vec::new()).await;

    assert!(actor.jobs[&source_id].is_empty());
    assert!(!second_paths.audio.exists());
    assert!(!second_paths.record.exists());
}

#[tokio::test]
async fn reconciling_a_rule_replaces_its_queue_and_releases_stale_owners() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let desired_id = TrackId::fake(2);
    let stale_id = TrackId::fake(3);
    let shared_id = TrackId::fake(4);
    let completed_id = TrackId::fake(5);
    let (loaded, desired_tracks) = accepted_tracks(
        directory.path(),
        source_id.clone(),
        vec![desired_id.clone(), completed_id.clone()],
    );
    for (track_id, owners) in [
        (
            stale_id.clone(),
            HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::Favorites,
            ))]),
        ),
        (
            shared_id.clone(),
            HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::Favorites,
            ))]),
        ),
        (
            completed_id.clone(),
            HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::Favorites,
            ))]),
        ),
    ] {
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio, b"audio").expect("download audio");
        write_record(
            &paths,
            &DownloadRecord {
                version: RECORD_VERSION,
                source_id: source_id.clone(),
                track_id,
                owners,
                audio_root: None,
                audio_path: None,
            },
        )
        .await
        .expect("download record");
    }
    let existing_id = "favorites".to_string();
    let mut actor = test_actor(directory.path());
    actor.attached.insert(
        source_id.clone(),
        AttachedSource {
            source: None,
            loaded: Arc::downgrade(&loaded),
            music_folder_id: None,
            directory: None,
        },
    );
    let mut favorites_job = download_job(
        &existing_id,
        DownloadSubject::Rule(DownloadRule::Favorites),
        vec![stale_id.clone(), shared_id.clone()],
        DownloadQueueState::Queued,
    );
    favorites_job.total_tracks += 1;
    favorites_job.completed.push(completed_id.clone());
    actor.jobs.insert(
        source_id.clone(),
        vec![
            favorites_job,
            download_job(
                "all-playlists",
                DownloadSubject::Rule(DownloadRule::AllPlaylists),
                vec![shared_id.clone()],
                DownloadQueueState::Queued,
            ),
        ],
    );
    let mut active = Vec::new();

    actor
        .reconcile_rule(
            source_id.clone(),
            DownloadRule::Favorites,
            StreamQuality::Original,
            desired_tracks
                .into_iter()
                .map(|track| track.id.clone())
                .collect(),
            &mut active,
        )
        .await;

    let stale_paths = download_paths(directory.path(), &source_id, &stale_id);
    assert!(!stale_paths.audio.exists());
    assert!(!stale_paths.record.exists());
    let shared = load_download_records(directory.path(), &source_id)
        .expect("load records")
        .remove(&shared_id)
        .expect("shared record");
    assert!(
        shared
            .owners
            .contains(&DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::AllPlaylists
            )))
    );
    assert!(
        !shared
            .owners
            .contains(&DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::Favorites
            )))
    );
    let jobs = actor.jobs.get(&source_id).expect("source queue");
    assert_eq!(jobs.len(), 2);
    let favorites = jobs
        .iter()
        .find(|job| job.id == existing_id)
        .expect("favorites queue");
    assert_eq!(favorites.total_tracks, 2);
    assert_eq!(favorites.completed, vec![completed_id]);
    assert_eq!(favorites.remaining, vec![desired_id]);
}

#[tokio::test]
async fn deleting_a_custom_folder_download_leaves_neighboring_music() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let custom = tempfile::tempdir().expect("custom download folder");
    let source_id = SourceId::fake(1);
    let (loaded, track) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(2));
    let paths = new_download_paths(
        directory.path(),
        &source_id,
        &track,
        Some(custom.path()),
        None,
    );
    let other_source_paths = new_download_paths(
        directory.path(),
        &SourceId::fake(2),
        &track,
        Some(custom.path()),
        None,
    );
    assert_ne!(paths.audio, other_source_paths.audio);
    assert_eq!(paths.audio.parent(), other_source_paths.audio.parent());
    std::fs::create_dir_all(&paths.directory).expect("create metadata directory");
    std::fs::create_dir_all(paths.audio.parent().expect("album directory"))
        .expect("create album directory");
    std::fs::write(&paths.audio, b"managed audio").expect("download audio");
    let neighboring = paths
        .audio
        .parent()
        .expect("album directory")
        .join("Already Here.flac");
    std::fs::write(&neighboring, b"user audio").expect("neighboring audio");
    let record = DownloadRecord {
        version: RECORD_VERSION,
        source_id: source_id.clone(),
        track_id: track.id.clone(),
        owners: HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
            DownloadRule::Favorites,
        ))]),
        audio_root: paths.audio_root.clone(),
        audio_path: Some(paths.audio.clone()),
    };
    write_record(&paths, &record).await.expect("record owner");
    loaded
        .set_downloaded_file(track.id.clone(), paths.audio.clone())
        .expect("attach managed audio");
    let mut actor = test_actor(directory.path());
    let loaded_weak = Arc::downgrade(&loaded);

    actor
        .remove_rule(
            &source_id,
            Some(&loaded_weak),
            DownloadRule::Favorites,
            true,
        )
        .await;

    assert!(!paths.audio.exists());
    assert!(!paths.record.exists());
    assert_eq!(
        std::fs::read(&neighboring).expect("neighboring audio remains"),
        b"user audio"
    );
    assert!(
        !loaded
            .is_downloaded(&track.id)
            .expect("read download status")
    );
}

#[test]
fn download_paths_use_the_resolved_transcoded_extension() {
    let directory = tempfile::tempdir().expect("temporary downloads");
    let source_id = SourceId::fake(1);
    let (_loaded, track) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(2));

    let original = new_download_paths(directory.path(), &source_id, &track, None, None);
    let jellyfin_opus = new_download_paths(directory.path(), &source_id, &track, None, Some("ogg"));
    let navidrome_opus =
        new_download_paths(directory.path(), &source_id, &track, None, Some("opus"));

    assert_eq!(
        original.audio.extension().and_then(|value| value.to_str()),
        Some("flac")
    );
    assert_eq!(
        jellyfin_opus
            .audio
            .extension()
            .and_then(|value| value.to_str()),
        Some("ogg")
    );
    assert_eq!(
        navidrome_opus
            .audio
            .extension()
            .and_then(|value| value.to_str()),
        Some("opus")
    );
    assert_eq!(original.audio_part, jellyfin_opus.audio_part);
    assert_eq!(original.audio_part, navidrome_opus.audio_part);
}
