use super::*;
#[test]
fn local_identity_is_stable_for_root() {
    let dir = tempfile::tempdir().expect("tempdir");

    let first = LocalProvider::identity_for_root(dir.path()).expect("identity");
    let second = LocalProvider::identity_for_root(dir.path()).expect("identity");

    assert_eq!(first, second);
    assert_eq!(first.provider, LOCAL_PROVIDER_ID);
}
#[tokio::test]
async fn local_stream_uses_file_uri() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("track.mp3");
    fs::write(&path, []).expect("audio file");
    let provider = LocalProvider::from_root(dir.path().to_path_buf()).expect("provider");
    let track = provider
        .tracks(PagedRequest::new(0, 1))
        .await
        .expect("tracks")
        .items
        .into_iter()
        .next()
        .expect("track");

    let stream = provider.stream(&track.id).await.expect("stream");

    assert!(stream.uri().starts_with("file://"));
}
#[tokio::test]
async fn local_provider_scans_multiple_roots() {
    let first = tempfile::tempdir().expect("first root");
    let second = tempfile::tempdir().expect("second root");
    fs::write(first.path().join("first.mp3"), []).expect("first track");
    fs::write(second.path().join("second.mp3"), []).expect("second track");

    let provider = LocalProvider::from_roots(vec![
        first.path().to_path_buf(),
        second.path().to_path_buf(),
    ])
    .expect("provider");

    let tracks = provider
        .tracks(PagedRequest::new(0, 10))
        .await
        .expect("tracks");

    assert_eq!(tracks.total, 2);
    assert_eq!(tracks.items.len(), 2);
}
#[tokio::test]
async fn local_provider_dedupes_overlapping_roots() {
    let root = tempfile::tempdir().expect("root");
    let nested = root.path().join("nested");
    fs::create_dir_all(&nested).expect("nested root");
    fs::write(nested.join("track.mp3"), []).expect("track");

    let provider =
        LocalProvider::from_roots(vec![root.path().to_path_buf(), nested]).expect("provider");

    let tracks = provider
        .tracks(PagedRequest::new(0, 10))
        .await
        .expect("tracks");

    assert_eq!(tracks.total, 1);
    assert_eq!(provider.manifest_scan().entries.len(), 1);
}
#[tokio::test]
async fn manifest_scan_reuses_unchanged_audio_without_tag_reads() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("track.mp3"), []).expect("track file");
    let server = LocalProvider::identity_for_root(dir.path()).expect("identity");
    let cold =
        LocalProvider::from_roots_with_identity(vec![dir.path().to_path_buf()], server.clone())
            .expect("cold provider");

    let warm = LocalProvider::from_roots_with_manifest_cache(
        vec![dir.path().to_path_buf()],
        server,
        cold.manifest_scan().entries.clone(),
    )
    .expect("warm provider");

    assert_eq!(cold.manifest_scan().counters.tag_reads, 1);
    assert_eq!(warm.manifest_scan().counters.tag_reads, 0);
    assert_eq!(warm.manifest_scan().counters.unchanged_reused, 1);
    assert!(!warm.manifest_scan().library_changed);
    assert_eq!(
        warm.tracks(PagedRequest::new(0, 10))
            .await
            .expect("tracks")
            .total,
        1
    );
}
#[tokio::test]
async fn manifest_scan_updates_folder_art_revision_without_tag_reads() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("track.mp3"), []).expect("track file");
    let cover = dir.path().join("cover.jpg");
    fs::write(&cover, [1_u8]).expect("cover file");
    let server = LocalProvider::identity_for_root(dir.path()).expect("identity");
    let cold =
        LocalProvider::from_roots_with_identity(vec![dir.path().to_path_buf()], server.clone())
            .expect("cold provider");
    let cold_tag = cold
        .albums(PagedRequest::new(0, 10))
        .await
        .expect("albums")
        .items
        .into_iter()
        .next()
        .and_then(|album| album.image_ref.and_then(|image_ref| image_ref.tag))
        .expect("cold cover tag");
    fs::write(&cover, [1_u8, 2]).expect("replace cover file");

    let warm = LocalProvider::from_roots_with_manifest_cache(
        vec![dir.path().to_path_buf()],
        server,
        cold.manifest_scan().entries.clone(),
    )
    .expect("warm provider");
    let warm_tag = warm
        .albums(PagedRequest::new(0, 10))
        .await
        .expect("albums")
        .items
        .into_iter()
        .next()
        .and_then(|album| album.image_ref.and_then(|image_ref| image_ref.tag))
        .expect("warm cover tag");

    assert_eq!(warm.manifest_scan().counters.tag_reads, 0);
    assert_eq!(warm.manifest_scan().counters.artwork_changed, 1);
    assert!(warm.manifest_scan().library_changed);
    assert_ne!(cold_tag, warm_tag);
}

#[test]
fn reparsed_manifest_entry_classifies_cover_only_change_as_artwork_track() {
    let facts = test_file_facts("/tmp/rufin-track-cover.flac");
    let stale = manifest_entry_for_scanned(
        &facts,
        &scanned_test_track(
            1,
            AlbumId::new("local:album:one"),
            Some(LocalCover::File {
                path: PathBuf::from("/tmp/cover.jpg"),
                revision: Some("cover-one".to_string()),
            }),
        ),
    );
    let current = manifest_entry_for_scanned(
        &facts,
        &scanned_test_track(
            1,
            AlbumId::new("local:album:one"),
            Some(LocalCover::File {
                path: PathBuf::from("/tmp/cover.jpg"),
                revision: Some("cover-two".to_string()),
            }),
        ),
    );
    let mut changed_track_ids = Vec::new();
    let mut metadata_track_ids = Vec::new();
    let mut artwork_track_ids = Vec::new();
    let mut retained_track_ids = Vec::new();
    let mut counters = LocalScanCounters::default();

    assert!(classify_reparsed_track(
        Some(&stale),
        &current,
        &mut changed_track_ids,
        &mut metadata_track_ids,
        &mut artwork_track_ids,
        &mut retained_track_ids,
        &mut counters,
    ));

    assert!(changed_track_ids.is_empty());
    assert!(metadata_track_ids.is_empty());
    assert_eq!(artwork_track_ids, vec![TrackId::fake(1)]);
    assert!(retained_track_ids.is_empty());
    assert_eq!(counters.artwork_changed, 1);
}

#[test]
fn reparsed_manifest_entry_classifies_non_search_metadata_change_as_metadata_track() {
    let facts = test_file_facts("/tmp/rufin-track-duration.flac");
    let stale_scanned = scanned_test_track(1, AlbumId::new("local:album:one"), None);
    let stale = manifest_entry_for_scanned(&facts, &stale_scanned);
    let mut current_scanned = stale_scanned;
    current_scanned.track.duration_seconds += 1;
    let current = manifest_entry_for_scanned(&facts, &current_scanned);
    let mut changed_track_ids = Vec::new();
    let mut metadata_track_ids = Vec::new();
    let mut artwork_track_ids = Vec::new();
    let mut retained_track_ids = Vec::new();
    let mut counters = LocalScanCounters::default();

    assert!(classify_reparsed_track(
        Some(&stale),
        &current,
        &mut changed_track_ids,
        &mut metadata_track_ids,
        &mut artwork_track_ids,
        &mut retained_track_ids,
        &mut counters,
    ));

    assert!(changed_track_ids.is_empty());
    assert_eq!(metadata_track_ids, vec![TrackId::fake(1)]);
    assert!(artwork_track_ids.is_empty());
    assert!(retained_track_ids.is_empty());
    assert_eq!(counters.artwork_changed, 0);
}

#[test]
fn reparsed_manifest_entry_classifies_album_id_change_as_changed_track() {
    let facts = test_file_facts("/tmp/rufin-track-album-id.flac");
    let stale_scanned = scanned_test_track(1, AlbumId::new("local:album:one"), None);
    let stale = manifest_entry_for_scanned(&facts, &stale_scanned);
    let mut current_scanned = stale_scanned;
    current_scanned.track.album_id = AlbumId::new("local:album:two");
    let current = manifest_entry_for_scanned(&facts, &current_scanned);
    let mut changed_track_ids = Vec::new();
    let mut metadata_track_ids = Vec::new();
    let mut artwork_track_ids = Vec::new();
    let mut retained_track_ids = Vec::new();
    let mut counters = LocalScanCounters::default();

    assert!(classify_reparsed_track(
        Some(&stale),
        &current,
        &mut changed_track_ids,
        &mut metadata_track_ids,
        &mut artwork_track_ids,
        &mut retained_track_ids,
        &mut counters,
    ));

    assert_eq!(changed_track_ids, vec![TrackId::fake(1)]);
    assert!(metadata_track_ids.is_empty());
    assert!(artwork_track_ids.is_empty());
    assert!(retained_track_ids.is_empty());
    assert_eq!(counters.artwork_changed, 0);
}

#[test]
fn reparsed_manifest_entry_classifies_comment_change_as_metadata_track() {
    let facts = test_file_facts("/tmp/rufin-track-comment.flac");
    let stale_scanned = scanned_test_track(1, AlbumId::new("local:album:one"), None);
    let stale = manifest_entry_for_scanned(&facts, &stale_scanned);
    let mut current_scanned = stale_scanned;
    current_scanned.track.comment = Some("alternate edition".to_string());
    let current = manifest_entry_for_scanned(&facts, &current_scanned);
    let mut changed_track_ids = Vec::new();
    let mut metadata_track_ids = Vec::new();
    let mut artwork_track_ids = Vec::new();
    let mut retained_track_ids = Vec::new();
    let mut counters = LocalScanCounters::default();

    assert!(classify_reparsed_track(
        Some(&stale),
        &current,
        &mut changed_track_ids,
        &mut metadata_track_ids,
        &mut artwork_track_ids,
        &mut retained_track_ids,
        &mut counters,
    ));

    assert!(changed_track_ids.is_empty());
    assert_eq!(metadata_track_ids, vec![TrackId::fake(1)]);
    assert!(artwork_track_ids.is_empty());
    assert!(retained_track_ids.is_empty());
}

#[test]
fn local_tracks_share_album_cover_ref() {
    let dir = tempfile::tempdir().expect("tempdir");
    let album_id = AlbumId::new("local:album:test");
    let first_cover = LocalCover::Embedded {
        path: dir.path().join("first.flac"),
        bytes: Arc::from([1_u8, 2, 3]),
        content_type: Some("image/jpeg".to_string()),
        revision: Some("embedded:first".to_string()),
    };
    let second_cover = LocalCover::Embedded {
        path: dir.path().join("second.flac"),
        bytes: Arc::from([4_u8, 5, 6]),
        content_type: Some("image/jpeg".to_string()),
        revision: Some("embedded:second".to_string()),
    };

    let library = build_library(
        vec![
            scanned_test_track(1, album_id.clone(), None),
            scanned_test_track(2, album_id.clone(), Some(first_cover)),
            scanned_test_track(3, album_id, Some(second_cover)),
        ],
        Vec::new(),
        HashMap::new(),
    );

    assert_eq!(library.albums.len(), 1);
    assert_eq!(library.covers.len(), 1);
    let album_cover = library.albums[0].image_ref.clone().expect("album cover");
    assert!(
        library
            .tracks
            .iter()
            .all(|track| track.image_ref.as_ref() == Some(&album_cover))
    );
}
#[tokio::test]
async fn local_embedded_cover_uses_scanned_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut covers = HashMap::new();
    covers.insert(
        "cover-one".to_string(),
        LocalCover::Embedded {
            path: dir.path().join("track.flac"),
            bytes: Arc::from([1_u8, 2, 3]),
            content_type: Some("image/png".to_string()),
            revision: Some("embedded:test".to_string()),
        },
    );
    let provider = LocalProvider {
        identity: ProviderIdentity {
            server: identity_for_root(dir.path()),
        },
        capabilities: local_capabilities(),
        library: LocalLibrary {
            covers,
            ..LocalLibrary::default()
        },
        manifest_scan: LocalManifestScan::default(),
    };

    let image = provider
        .image_bytes(ImageRequest {
            item_id: "cover-one".to_string(),
            kind: ImageKind::Primary,
            tag: None,
            size: 512,
        })
        .await
        .expect("embedded local cover");

    assert_eq!(image.bytes, vec![1, 2, 3]);
    assert_eq!(image.content_type.as_deref(), Some("image/png"));
}
#[tokio::test]
async fn local_file_cover_rejects_oversized_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cover_path = dir.path().join("folder.jpg");
    let file = fs::File::create(&cover_path).expect("cover file");
    file.set_len((LOCAL_COVER_MAX_BYTES + 1) as u64)
        .expect("cover length");
    let mut covers = HashMap::new();
    covers.insert(
        "cover-one".to_string(),
        LocalCover::File {
            path: cover_path,
            revision: None,
        },
    );
    let provider = LocalProvider {
        identity: ProviderIdentity {
            server: identity_for_root(dir.path()),
        },
        capabilities: local_capabilities(),
        library: LocalLibrary {
            covers,
            ..LocalLibrary::default()
        },
        manifest_scan: LocalManifestScan::default(),
    };

    let error = provider
        .image_bytes(ImageRequest {
            item_id: "cover-one".to_string(),
            kind: ImageKind::Primary,
            tag: None,
            size: 512,
        })
        .await
        .expect_err("oversized local cover");

    assert!(error.to_string().contains("local cover exceeded"));
}
#[test]
fn local_cover_item_id_reads_file_cover_from_configured_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cover_path = dir.path().join("folder.jpg");
    fs::write(&cover_path, [1_u8, 2, 3]).expect("cover file");
    let item_id = cover_id(&LocalCover::File {
        path: cover_path,
        revision: None,
    });

    let image =
        LocalProvider::image_bytes_for_cover_item_id(&item_id, vec![dir.path().to_path_buf()])
            .expect("local cover");

    assert_eq!(image.bytes, vec![1, 2, 3]);
    assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
}
#[test]
fn local_cover_item_id_rejects_paths_outside_configured_roots() {
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::tempdir().expect("outside");
    let cover_path = outside.path().join("folder.jpg");
    fs::write(&cover_path, [1_u8, 2, 3]).expect("cover file");
    let item_id = cover_id(&LocalCover::File {
        path: cover_path,
        revision: None,
    });

    let error =
        LocalProvider::image_bytes_for_cover_item_id(&item_id, vec![root.path().to_path_buf()])
            .expect_err("outside-root cover");

    assert_eq!(error.to_string(), "provider item was not found");
}
#[test]
fn embedded_cover_rejects_oversized_picture_data() {
    let picture = Picture::unchecked(vec![0_u8; LOCAL_COVER_MAX_BYTES + 1]).build();

    let error = picture_data_bounded(&picture).expect_err("oversized embedded cover");

    assert!(error.to_string().contains("embedded cover exceeded"));
}
#[test]
fn folder_cover_uses_single_supported_image_fallback() {
    let dir = tempfile::tempdir().expect("tempdir");
    let image = dir.path().join("artwork.png");
    fs::write(&image, [1_u8]).expect("image file");

    assert_eq!(folder_cover(dir.path()).as_deref(), Some(image.as_path()));
}
#[test]
fn folder_cover_prefers_explicit_name_over_other_images() {
    let dir = tempfile::tempdir().expect("tempdir");
    let booklet = dir.path().join("booklet.png");
    let cover = dir.path().join("Cover.JPG");
    fs::write(&booklet, [1_u8]).expect("booklet image");
    fs::write(&cover, [2_u8]).expect("cover image");

    assert_eq!(folder_cover(dir.path()).as_deref(), Some(cover.as_path()));
}
#[test]
fn folder_cover_skips_ambiguous_unnamed_images() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("back.jpg"), [1_u8]).expect("back image");
    fs::write(dir.path().join("booklet.png"), [2_u8]).expect("booklet image");

    assert!(folder_cover(dir.path()).is_none());
}
#[test]
fn local_artist_image_prefers_artist_folder_artwork_over_album_cover() {
    let dir = tempfile::tempdir().expect("tempdir");
    let album_id = AlbumId::new("local:album:test");
    let artist_dir = dir.path().join("Example Artist");
    let album_dir = artist_dir.join("Example Album");
    fs::create_dir_all(&album_dir).expect("album dir");
    let artist_image = artist_dir.join("artist.jpg");
    let album_image = album_dir.join("cover.jpg");
    let track_path = album_dir.join("track.flac");
    fs::write(&artist_image, [1_u8]).expect("artist image");
    fs::write(&album_image, [2_u8]).expect("album image");
    fs::write(&track_path, []).expect("track file");

    let library = build_library(
        vec![scanned_test_track_at(
            1,
            album_id,
            Some(LocalCover::File {
                path: album_image,
                revision: Some("file:album".to_string()),
            }),
            &track_path,
        )],
        Vec::new(),
        HashMap::new(),
    );

    let artist_ref = ImageRef::new(
        cover_id(&LocalCover::File {
            path: artist_image,
            revision: None,
        }),
        file_revision(&artist_dir.join("artist.jpg")),
    );
    assert_eq!(library.covers.len(), 2);
    assert_eq!(library.artists[0].image_ref.as_ref(), Some(&artist_ref));
    assert_eq!(
        library.album_artists[0].image_ref.as_ref(),
        Some(&artist_ref)
    );
}
#[test]
fn local_artist_image_falls_back_to_album_cover() {
    let dir = tempfile::tempdir().expect("tempdir");
    let album_id = AlbumId::new("local:album:test");
    let album_dir = dir.path().join("Example Artist").join("Example Album");
    fs::create_dir_all(&album_dir).expect("album dir");
    let album_image = album_dir.join("cover.jpg");
    let track_path = album_dir.join("track.flac");
    fs::write(&album_image, [1_u8]).expect("album image");
    fs::write(&track_path, []).expect("track file");

    let library = build_library(
        vec![scanned_test_track_at(
            1,
            album_id,
            Some(LocalCover::File {
                path: album_image,
                revision: Some("file:album".to_string()),
            }),
            &track_path,
        )],
        Vec::new(),
        HashMap::new(),
    );

    let album_ref = library.albums[0].image_ref.clone().expect("album cover");
    assert_eq!(library.artists[0].image_ref.as_ref(), Some(&album_ref));
    assert_eq!(
        library.album_artists[0].image_ref.as_ref(),
        Some(&album_ref)
    );
}
#[tokio::test]
async fn local_folder_root_lists_configured_roots() {
    let first = tempfile::tempdir().expect("first root");
    let second = tempfile::tempdir().expect("second root");

    let provider = LocalProvider::from_roots(vec![
        first.path().to_path_buf(),
        second.path().to_path_buf(),
    ])
    .expect("provider");

    let detail = provider.folder(None, None).await.expect("root folder");
    let folder_names = detail
        .folders
        .iter()
        .map(|folder| folder.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(detail.tracks.len(), 0);
    let first_name = first
        .path()
        .file_name()
        .expect("first root has file name")
        .to_str()
        .expect("first root file name is utf-8");
    let second_name = second
        .path()
        .file_name()
        .expect("second root has file name")
        .to_str()
        .expect("second root file name is utf-8");
    assert!(folder_names.contains(&first_name));
    assert!(folder_names.contains(&second_name));
}
#[tokio::test]
async fn local_folder_nested_view_lists_child_folders_and_direct_tracks() {
    let root = tempfile::tempdir().expect("root");
    let artist = root.path().join("Artist");
    let album = artist.join("Album");
    fs::create_dir_all(&album).expect("album dir");
    fs::write(artist.join("single.mp3"), []).expect("single track");
    fs::write(album.join("album-track.mp3"), []).expect("album track");
    let provider = LocalProvider::from_root(root.path().to_path_buf()).expect("provider");

    let root_path = root.path().canonicalize().expect("canonical root");
    let artist_id = folder_for_path(&root_path.join("Artist")).id;
    let artist_detail = provider
        .folder(Some(&artist_id), None)
        .await
        .expect("artist folder");

    assert_eq!(artist_detail.folders.len(), 1);
    assert_eq!(artist_detail.folders[0].name, "Album");
    assert_eq!(artist_detail.tracks.len(), 1);
    assert_eq!(artist_detail.tracks[0].title, "single");

    let album_id = folder_for_path(&root_path.join("Artist").join("Album")).id;
    let album_detail = provider
        .folder(Some(&album_id), None)
        .await
        .expect("album folder");

    assert_eq!(album_detail.folders.len(), 0);
    assert_eq!(album_detail.tracks.len(), 1);
    assert_eq!(album_detail.tracks[0].title, "album-track");
}
#[tokio::test]
async fn local_folder_rejects_unknown_folder_ids() {
    let root = tempfile::tempdir().expect("root");
    let provider = LocalProvider::from_root(root.path().to_path_buf()).expect("provider");
    let outside = FolderId::new("local:folder:%2Fetc%2Fmusic");

    let result = provider.folder(Some(&outside), None).await;

    assert!(matches!(result, Err(ProviderError::NotFound)));
}

fn scanned_test_track(number: u32, album_id: AlbumId, cover: Option<LocalCover>) -> ScannedTrack {
    let artist = ArtistCredit {
        id: ArtistId::new("local:artist:example"),
        name: "Example Artist".to_string(),
    };
    ScannedTrack {
        track: Track {
            id: TrackId::fake(number),
            album_id,
            title: format!("Track {number}"),
            artist: artist.name.clone(),
            artist_id: Some(artist.id.clone()),
            artist_credits: vec![artist.clone()],
            album_artist_credits: vec![artist],
            album: "Example Album".to_string(),
            year: 2024,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 60,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
            image_ref: None,
            genres: Vec::new(),
            local_path: Some(format!("/tmp/rufin-track-{number}.flac")),
            source_format: Some("flac".to_string()),
            comment: None,
            skip_count: None,
        },
        album_artist: "Example Artist".to_string(),
        cover,
    }
}

fn scanned_test_track_at(
    number: u32,
    album_id: AlbumId,
    cover: Option<LocalCover>,
    path: &Path,
) -> ScannedTrack {
    let mut scanned = scanned_test_track(number, album_id, cover);
    scanned.track.local_path = Some(path.to_string_lossy().into_owned());
    scanned
}

fn test_file_facts(path: &str) -> LocalFileFacts {
    let path = PathBuf::from(path);
    LocalFileFacts {
        root_path: path.parent().unwrap_or(Path::new("/")).to_path_buf(),
        relative_path: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("track.flac")
            .to_string(),
        path,
        file_size: 128,
        mtime_seconds: 1,
        mtime_nanos: 2,
        inode: Some(3),
        device: Some(4),
    }
}
