use std::collections::HashMap;
use std::path::{Path, PathBuf};

use library::{
    CandidateBatch, CandidateFinish, CandidateHeader, HomeFacts, Libraries, LocalAccessFile,
    LocalAccessMapping, MetadataError, MetadataItemId, PlayableFile, SourceId, Track, TrackData,
    TrackId, TrackRelations, project_local_access_path, reported_path_is_absolute,
};

#[test]
fn local_access_projection_uses_the_reported_server_path() {
    let root = PathBuf::from("/portal/library");
    let absolute = LocalAccessMapping {
        root_path: root.clone(),
        server_prefix: Some("/srv/navidrome/audio".to_string()),
        local_prefix: None,
    };
    assert_eq!(
        project_local_access_path("/srv/navidrome/audio/Artist/Album/Track.flac", &absolute,),
        Some(root.join("Artist/Album/Track.flac"))
    );

    let relative = LocalAccessMapping {
        root_path: root.clone(),
        server_prefix: None,
        local_prefix: None,
    };
    assert_eq!(
        project_local_access_path("Artist/Album/Track.flac", &relative),
        Some(root.join("Artist/Album/Track.flac"))
    );
    assert_eq!(
        project_local_access_path("/music/Artist/Album/Track.flac", &absolute),
        None
    );
    let server_root = LocalAccessMapping {
        root_path: root.clone(),
        server_prefix: Some("/".to_string()),
        local_prefix: None,
    };
    assert_eq!(
        project_local_access_path("/Artist/Album/Track.flac", &server_root),
        Some(root.join("Artist/Album/Track.flac"))
    );
    assert!(reported_path_is_absolute(r"D:\Music\Artist\Track.flac"));
    assert!(reported_path_is_absolute(
        r"\\server\Music\Artist\Track.flac"
    ));
    assert!(reported_path_is_absolute("/srv/music/Artist/Track.flac"));
}

#[test]
fn local_access_preserves_full_filesystem_identities() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let root = tempfile::tempdir().expect("local access root");
    let store_path = directory.path().join("library.db");
    let source_id = SourceId::new("opensubsonic:server:filesystem-identities");
    let library = Libraries::open(&store_path).expect("open Library");
    let loaded = accept_tracks(&library, source_id.clone(), Vec::new());
    let identity_values = [0, i64::MAX as u64, (i64::MAX as u64) + 1, u64::MAX];
    let files = identity_values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let path = root.path().join(format!("Track-{index}.flac"));
            std::fs::write(&path, []).expect("write local access file");
            let mut file = access_file(root.path(), &path, "Track", "Album", "Artist");
            file.device_id = Some(value);
            file.inode = Some(value);
            file
        })
        .collect::<Vec<_>>();
    loaded
        .replace_local_access(
            LocalAccessMapping {
                root_path: root.path().to_path_buf(),
                server_prefix: None,
                local_prefix: None,
            },
            files.clone(),
        )
        .expect("accept full-range filesystem identities");
    assert_eq!(
        loaded
            .local_access_files()
            .expect("read local access files"),
        files
    );

    drop(loaded);
    drop(library);
    let reopened = Libraries::open(store_path)
        .expect("reopen Library")
        .load_source(&source_id)
        .expect("load source")
        .expect("source");
    assert_eq!(
        reopened
            .local_access_files()
            .expect("read reopened local access files"),
        files
    );
}

#[test]
fn accepted_local_access_maps_tracks_and_reopens() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let root = tempfile::tempdir().expect("local access root");
    let store_path = directory.path().join("library.db");
    let source_id = SourceId::new("opensubsonic:server:local-access");
    let direct_path = root.path().join("Direct.flac");
    let prefix_path = root.path().join("Artist/Album/Prefix.flac");
    let metadata_path = root.path().join("Elsewhere/Metadata.flac");
    std::fs::create_dir_all(prefix_path.parent().expect("prefix parent"))
        .expect("create prefix directory");
    std::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
        .expect("create metadata directory");
    std::fs::write(&direct_path, []).expect("write direct file");
    std::fs::write(&prefix_path, []).expect("write prefix file");
    std::fs::write(&metadata_path, []).expect("write metadata file");
    let tracks = vec![
        track(
            "direct",
            "Direct",
            "Album",
            "Artist",
            Some(direct_path.to_string_lossy().into_owned()),
        ),
        track(
            "prefix",
            "Prefix",
            "Album",
            "Artist",
            Some("/server/music/Artist/Album/Prefix.flac".to_string()),
        ),
        track(
            "metadata",
            "Metadata",
            "Other Album",
            "Other Artist",
            Some("/server/unknown/Metadata.flac".to_string()),
        ),
    ];
    let files = vec![
        access_file(root.path(), &direct_path, "Direct", "Album", "Artist"),
        access_file(root.path(), &prefix_path, "Prefix", "Album", "Artist"),
        access_file(
            root.path(),
            &metadata_path,
            "Metadata",
            "Other Album",
            "Other Artist",
        ),
    ];
    let mapping = LocalAccessMapping {
        root_path: root.path().to_path_buf(),
        server_prefix: Some("/server/music".to_string()),
        local_prefix: None,
    };

    {
        let library = Libraries::open(&store_path).expect("open Library");
        let loaded = accept_tracks(&library, source_id.clone(), tracks);
        let before_scan = loaded
            .configure_local_access(mapping.clone())
            .expect("configure mapping before its scan");
        assert_eq!(before_scan.unmatched_count, 3);
        assert_playable(&loaded, "direct", &direct_path);
        assert_playable(&loaded, "prefix", &prefix_path);
        let status = loaded
            .replace_local_access(mapping.clone(), files.clone())
            .expect("accept local access files");
        assert_eq!(status.direct_match_count, 1);
        assert_eq!(status.prefix_match_count, 1);
        assert_eq!(status.metadata_match_count, 1);
        assert_eq!(status.unmatched_count, 0);
        assert_playable(&loaded, "direct", &direct_path);
        assert_playable(&loaded, "prefix", &prefix_path);
        assert_playable(&loaded, "metadata", &metadata_path);
        assert_eq!(
            metadata_target(&loaded, "direct").expect("accepted direct metadata target"),
            direct_path
        );
        assert_eq!(
            metadata_target(&loaded, "prefix").expect("accepted prefix metadata target"),
            prefix_path
        );
        assert_eq!(
            metadata_target(&loaded, "metadata")
                .expect_err("metadata-only playback matching cannot select a mutation target"),
            MetadataError::LocalAccessRequired {
                source_path: "/server/unknown/Metadata.flac".to_string(),
            }
        );
    }

    let library = Libraries::open(&store_path).expect("reopen Library");
    let loaded = library
        .load_source(&source_id)
        .expect("load accepted source")
        .expect("accepted source");
    assert!(
        loaded
            .playable_file(&TrackId::new("track:direct"))
            .expect("read unconfigured playback")
            .is_none(),
        "persisted files are inert until their current mapping is configured"
    );
    let status = loaded
        .configure_local_access(mapping)
        .expect("configure reopened local access");
    assert_eq!(status.direct_match_count, 1);
    assert_eq!(status.prefix_match_count, 1);
    assert_eq!(status.metadata_match_count, 1);
    assert_playable(&loaded, "direct", &direct_path);
    assert_playable(&loaded, "prefix", &prefix_path);
    assert_playable(&loaded, "metadata", &metadata_path);
    assert_eq!(
        metadata_target(&loaded, "prefix").expect("configured reopened prefix target"),
        prefix_path
    );
    assert_eq!(
        metadata_target(&loaded, "metadata")
            .expect_err("reopened metadata-only match cannot select a mutation target"),
        MetadataError::LocalAccessRequired {
            source_path: "/server/unknown/Metadata.flac".to_string(),
        }
    );

    let status = loaded.clear_local_access().expect("clear local access");
    assert_eq!(status.unmatched_count, 3);
    assert!(loaded.local_access_files().expect("read files").is_empty());
}

#[test]
fn proposed_metadata_access_projects_the_exact_file_without_scanning_the_root() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let root = tempfile::tempdir().expect("proposed local access root");
    let local_path = root.path().join("Artist/Track.flac");
    std::fs::create_dir_all(local_path.parent().expect("Track parent"))
        .expect("create Track parent");
    std::fs::write(&local_path, []).expect("write proposed Track");
    let reported_path = "/server/music/Artist/Track.flac";
    let mut cue = track(
        "cue",
        "Cue",
        "Album",
        "Artist",
        Some(reported_path.to_string()),
    );
    cue.make_mut().cue = Some(library::CueSegment {
        cue_path: "/server/music/Album.cue".to_string(),
        start_millis: 0,
        end_millis: 1,
    });
    let library =
        Libraries::open(directory.path().join("library.db")).expect("open temporary Library");
    let loaded = accept_tracks(
        &library,
        SourceId::new("opensubsonic:server:metadata-proposal"),
        vec![
            track(
                "proposed",
                "Track",
                "Album",
                "Artist",
                Some(reported_path.to_string()),
            ),
            track("absent", "Absent", "Album", "Artist", None),
            cue,
        ],
    );
    let mapping = LocalAccessMapping {
        root_path: root.path().to_path_buf(),
        server_prefix: Some("/server/music".to_string()),
        local_prefix: None,
    };
    assert_eq!(
        metadata_target(&loaded, "proposed").expect_err("accepted mapping is still absent"),
        MetadataError::LocalAccessRequired {
            source_path: reported_path.to_string(),
        }
    );
    let (_, targets) = loaded
        .metadata_subject_with_local_access(
            &MetadataItemId::Track(TrackId::new("track:proposed")),
            Some(&mapping),
        )
        .expect("resolve proposed mapping")
        .expect("proposed metadata Track");
    assert_eq!(targets[0].path(), local_path);
    assert!(
        loaded
            .local_access_files()
            .expect("read accepted mapping")
            .is_empty(),
        "validating a proposal must not accept it"
    );

    for (id, source_path) in [("absent", ""), ("cue", reported_path)] {
        assert_eq!(
            loaded
                .metadata_subject_with_local_access(
                    &MetadataItemId::Track(TrackId::new(format!("track:{id}"))),
                    Some(&mapping),
                )
                .expect_err("non-exact metadata target rejected"),
            MetadataError::LocalAccessRequired {
                source_path: source_path.to_string(),
            }
        );
    }
    let wrong_prefix = LocalAccessMapping {
        server_prefix: Some("/another/library".to_string()),
        ..mapping
    };
    assert_eq!(
        loaded
            .metadata_subject_with_local_access(
                &MetadataItemId::Track(TrackId::new("track:proposed")),
                Some(&wrong_prefix),
            )
            .expect_err("a source path outside the configured prefix is rejected"),
        MetadataError::LocalAccessRequired {
            source_path: reported_path.to_string(),
        }
    );
}

#[test]
fn metadata_mapping_rejects_ambiguity_and_files_from_an_old_root() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let configured_root = tempfile::tempdir().expect("configured root");
    let old_root = tempfile::tempdir().expect("old root");
    let library =
        Libraries::open(directory.path().join("library.db")).expect("open temporary Library");
    let loaded = accept_tracks(
        &library,
        SourceId::new("jellyfin:server:ambiguous"),
        vec![track("ambiguous", "Same", "Album", "Artist", None)],
    );
    let first = configured_root.path().join("First.flac");
    let second = configured_root.path().join("Second.flac");
    let old = old_root.path().join("Old.flac");
    let files = vec![
        access_file(configured_root.path(), &first, "Same", "Album", "Artist"),
        access_file(configured_root.path(), &second, "Same", "Album", "Artist"),
        access_file(old_root.path(), &old, "Same", "Album", "Artist"),
    ];
    let status = loaded
        .replace_local_access(
            LocalAccessMapping {
                root_path: configured_root.path().to_path_buf(),
                server_prefix: None,
                local_prefix: None,
            },
            files,
        )
        .expect("accept ambiguous local files");

    assert_eq!(status.unmatched_count, 1);
    assert_eq!(status.metadata_match_count, 0);
    assert!(
        loaded
            .playable_file(&TrackId::new("track:ambiguous"))
            .expect("read playback mapping")
            .is_none()
    );
}

#[test]
fn server_prefix_matches_a_path_component_not_a_text_prefix() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let root = tempfile::tempdir().expect("local access root");
    let library =
        Libraries::open(directory.path().join("library.db")).expect("open temporary Library");
    let loaded = accept_tracks(
        &library,
        SourceId::new("opensubsonic:server:path-boundary"),
        vec![track(
            "path-boundary",
            "Remote title",
            "Remote album",
            "Remote artist",
            Some("/server/music-old/Track.flac".to_string()),
        )],
    );
    let local_path = root.path().join("-old/Track.flac");
    let status = loaded
        .replace_local_access(
            LocalAccessMapping {
                root_path: root.path().to_path_buf(),
                server_prefix: Some("/server/music".to_string()),
                local_prefix: None,
            },
            vec![access_file(
                root.path(),
                &local_path,
                "Different title",
                "Different album",
                "Different artist",
            )],
        )
        .expect("accept local access file");

    assert_eq!(status.prefix_match_count, 0);
    assert_eq!(status.unmatched_count, 1);
}

#[test]
fn an_exact_download_wins_without_replacing_local_access() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let local = tempfile::tempdir().expect("local access root");
    let library =
        Libraries::open(directory.path().join("library.db")).expect("open temporary Library");
    let track_id = TrackId::new("track:downloaded");
    let loaded = accept_tracks(
        &library,
        SourceId::new("opensubsonic:server:download"),
        vec![track(
            "downloaded",
            "Downloaded",
            "Album",
            "Artist",
            Some("/server/music/Downloaded.flac".to_string()),
        )],
    );
    let mapped = local.path().join("Downloaded.flac");
    let downloaded = directory.path().join("downloaded.audio");
    std::fs::write(&mapped, b"mapped").expect("mapped audio");
    std::fs::write(&downloaded, b"downloaded").expect("downloaded audio");
    loaded
        .configure_local_access(LocalAccessMapping {
            root_path: local.path().to_path_buf(),
            server_prefix: Some("/server/music".to_string()),
            local_prefix: None,
        })
        .expect("configure mapping");

    let removed = loaded
        .replace_downloaded_files(HashMap::from([(track_id.clone(), downloaded.clone())]))
        .expect("attach download");
    assert!(removed.is_empty());

    assert!(loaded.is_downloaded(&track_id).expect("download state"));
    assert_eq!(
        loaded
            .playable_file(&track_id)
            .expect("downloaded playback"),
        Some(PlayableFile::File {
            path: downloaded.clone()
        })
    );
    assert_eq!(
        loaded
            .remove_downloaded_file(&track_id)
            .expect("remove download"),
        Some(downloaded)
    );
    assert_eq!(
        loaded.playable_file(&track_id).expect("mapped playback"),
        Some(PlayableFile::File { path: mapped })
    );
}

fn accept_tracks(
    library: &Libraries,
    source_id: SourceId,
    tracks: Vec<Track>,
) -> std::sync::Arc<library::Library> {
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id,
            input_version: 1,
            input_digest: [7; 32],
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Tracks(tracks))
        .expect("write Tracks");
    candidate
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
        .library
}

fn track(id: &str, title: &str, album: &str, artist: &str, source_path: Option<String>) -> Track {
    Track::new(TrackData {
        id: TrackId::new(format!("track:{id}")),
        album_id: None,
        title: title.to_string(),
        artist: artist.to_string(),
        album: album.to_string(),
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
        track_number: 1,
        image_ref: None,
        local_artwork: None,
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        source_path,
        cue: None,
        source_format: Some("flac".to_string()),
        comment: None,
        skip_count: None,
        bpm: None,
        relations: TrackRelations::default(),
    })
}

fn access_file(
    root: &Path,
    path: &Path,
    title: &str,
    album: &str,
    artist: &str,
) -> LocalAccessFile {
    LocalAccessFile {
        path: path.to_string_lossy().into_owned(),
        root: root.to_string_lossy().into_owned(),
        relative_path: path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned(),
        size_bytes: 1,
        mtime_ns: 1,
        device_id: Some(1),
        inode: Some(1),
        parser_version: 1,
        title: title.to_string(),
        album: album.to_string(),
        artist: artist.to_string(),
        disc_number: 1,
        track_number: 1,
        duration_seconds: 180,
    }
}

fn assert_playable(loaded: &library::Library, id: &str, expected: &PathBuf) {
    let playable = loaded
        .playable_file(&TrackId::new(format!("track:{id}")))
        .expect("read playback mapping")
        .expect("mapped playback file");
    assert_eq!(
        playable,
        PlayableFile::File {
            path: expected.clone()
        }
    );
}

fn metadata_target(
    loaded: &std::sync::Arc<library::Library>,
    id: &str,
) -> Result<PathBuf, MetadataError> {
    let (_, targets) = loaded
        .metadata_subject_with_local_access(
            &MetadataItemId::Track(TrackId::new(format!("track:{id}"))),
            None,
        )?
        .ok_or(MetadataError::Unavailable)?;
    targets
        .into_iter()
        .next()
        .map(|target| target.path().to_path_buf())
        .ok_or(MetadataError::Unavailable)
}
