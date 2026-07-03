use super::*;
use lofty::picture::MimeType;
use lofty::tag::{ItemValue, TagItem, TagType};
#[test]
fn local_root_identity() {
    let dir = tempfile::tempdir().expect("tempdir");

    let first = LocalSource::identity_for_root(dir.path()).expect("identity");
    let second = LocalSource::identity_for_root(dir.path()).expect("identity");

    assert_eq!(first, second);
    assert_eq!(first.provider, LOCAL_SOURCE_ID);
}
#[tokio::test]
async fn local_stream_uses_file_uri() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("track.mp3");
    fs::write(&path, []).expect("audio file");
    let provider = LocalSource::from_root(dir.path().to_path_buf()).expect("provider");
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

    let provider = LocalSource::from_roots(vec![
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
async fn local_provider_projects_single_file_cue_tracks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let audio = dir.path().join("album.wav");
    write_silent_wav(&audio, 8).expect("write wav");
    fs::write(
        dir.path().join("album.cue"),
        r#"
PERFORMER "Cue Artist"
TITLE "Cue Album"
FILE "album.wav" WAVE
  TRACK 01 AUDIO
    TITLE "First Cue Track"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Second Cue Track"
    INDEX 01 00:04:00
"#,
    )
    .expect("write cue");

    let provider = LocalSource::from_root(dir.path().to_path_buf()).expect("provider");
    let tracks = provider
        .tracks(PagedRequest::new(0, 10))
        .await
        .expect("tracks");

    assert_eq!(tracks.total, 2);
    assert!(
        tracks
            .items
            .iter()
            .any(|track| track.title == "First Cue Track")
    );
    assert!(
        tracks
            .items
            .iter()
            .any(|track| track.title == "Second Cue Track")
    );
    assert!(tracks.items.iter().all(|track| track.album == "Cue Album"));
    assert_eq!(provider.manifest_scan().cue_track_sources.len(), 2);
    assert_eq!(provider.manifest_scan().entries.len(), 2);
    assert_eq!(provider.manifest_scan().counters.cue_sheets, 1);
    assert_eq!(provider.manifest_scan().counters.cue_tracks, 2);
    assert_eq!(provider.manifest_scan().counters.cue_backing_reads, 1);
}

#[tokio::test]
async fn local_provider_reuses_unchanged_cue_tracks_without_backing_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let audio = dir.path().join("album.wav");
    write_silent_wav(&audio, 8).expect("write wav");
    fs::write(
        dir.path().join("album.cue"),
        r#"
PERFORMER "Cue Artist"
TITLE "Cue Album"
FILE "album.wav" WAVE
  TRACK 01 AUDIO
    TITLE "First Cue Track"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Second Cue Track"
    INDEX 01 00:04:00
"#,
    )
    .expect("write cue");
    let server = identity_for_root(dir.path());
    let cold =
        LocalSource::from_roots_with_identity(vec![dir.path().to_path_buf()], server.clone())
            .expect("cold provider");

    let warm = LocalSource::from_roots_with_manifest_cache(
        vec![dir.path().to_path_buf()],
        server,
        cold.manifest_scan().entries.clone(),
    )
    .expect("warm provider");

    assert_eq!(cold.manifest_scan().counters.cue_backing_reads, 1);
    assert_eq!(warm.manifest_scan().counters.cue_sheets, 1);
    assert_eq!(warm.manifest_scan().counters.cue_tracks, 2);
    assert_eq!(warm.manifest_scan().counters.cue_backing_reads, 0);
    assert_eq!(warm.manifest_scan().counters.cue_reused_tracks, 2);
    assert_eq!(warm.manifest_scan().counters.tag_reads, 0);
    assert_eq!(warm.manifest_scan().cue_track_sources.len(), 2);
    assert!(!warm.manifest_scan().library_changed);
    assert_eq!(
        warm.tracks(PagedRequest::new(0, 10))
            .await
            .expect("tracks")
            .total,
        2
    );
}

#[tokio::test]
async fn local_provider_replaces_cached_file_track_with_cue_tracks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let audio = dir.path().join("album.wav");
    write_silent_wav(&audio, 8).expect("write wav");
    let server = identity_for_root(dir.path());
    let cold =
        LocalSource::from_roots_with_identity(vec![dir.path().to_path_buf()], server.clone())
            .expect("cold provider");
    let old_track_id = cold.manifest_scan().entries[0].track.id.clone();
    fs::write(
        dir.path().join("album.cue"),
        r#"
PERFORMER "Cue Artist"
TITLE "Cue Album"
FILE "album.wav" WAVE
  TRACK 01 AUDIO
    TITLE "First Cue Track"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Second Cue Track"
    INDEX 01 00:04:00
"#,
    )
    .expect("write cue");

    let warm = LocalSource::from_roots_with_manifest_cache(
        vec![dir.path().to_path_buf()],
        server,
        cold.manifest_scan().entries.clone(),
    )
    .expect("warm provider");
    let tracks = warm.tracks(PagedRequest::new(0, 10)).await.expect("tracks");

    assert_eq!(tracks.total, 2);
    assert_eq!(warm.manifest_scan().cue_track_sources.len(), 2);
    assert_eq!(warm.manifest_scan().deleted_track_ids, vec![old_track_id]);
    assert!(warm.manifest_scan().library_changed);
}
#[tokio::test]
async fn local_provider_skips_oversized_cue_sheet() {
    let dir = tempfile::tempdir().expect("tempdir");
    let audio = dir.path().join("album.wav");
    write_silent_wav(&audio, 8).expect("write wav");
    let cue = dir.path().join("album.cue");
    fs::write(
        &cue,
        r#"
PERFORMER "Cue Artist"
TITLE "Cue Album"
FILE "album.wav" WAVE
  TRACK 01 AUDIO
    TITLE "First Cue Track"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Second Cue Track"
    INDEX 01 00:04:00
"#,
    )
    .expect("write cue");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&cue)
        .expect("cue file");
    file.set_len((LOCAL_CUE_MAX_BYTES + 1) as u64)
        .expect("cue length");

    let provider = LocalSource::from_root(dir.path().to_path_buf()).expect("provider");
    let tracks = provider
        .tracks(PagedRequest::new(0, 10))
        .await
        .expect("tracks");

    assert_eq!(tracks.total, 1);
    assert_eq!(provider.manifest_scan().cue_track_sources.len(), 0);
    assert_eq!(provider.manifest_scan().entries.len(), 1);
}
#[tokio::test]
async fn local_provider_dedupes_overlapping_roots() {
    let root = tempfile::tempdir().expect("root");
    let nested = root.path().join("nested");
    fs::create_dir_all(&nested).expect("nested root");
    fs::write(nested.join("track.mp3"), []).expect("track");

    let provider =
        LocalSource::from_roots(vec![root.path().to_path_buf(), nested]).expect("provider");

    let tracks = provider
        .tracks(PagedRequest::new(0, 10))
        .await
        .expect("tracks");

    assert_eq!(tracks.total, 1);
    assert_eq!(provider.manifest_scan().entries.len(), 1);
}

fn write_silent_wav(path: &Path, seconds: u32) -> std::io::Result<()> {
    let sample_rate = 8_000_u32;
    let bits_per_sample = 16_u16;
    let channels = 1_u16;
    let sample_count = sample_rate.saturating_mul(seconds);
    let data_len = sample_count * u32::from(channels) * u32::from(bits_per_sample / 8);
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
    fs::write(path, bytes)
}
#[tokio::test]
async fn manifest_scan_reuse() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("track.mp3"), []).expect("track file");
    let server = LocalSource::identity_for_root(dir.path()).expect("identity");
    let cold =
        LocalSource::from_roots_with_identity(vec![dir.path().to_path_buf()], server.clone())
            .expect("cold provider");

    let warm = LocalSource::from_roots_with_manifest_cache(
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

#[test]
fn manifest_reuse_keeps_embedded_cover() {
    let path = "/tmp/rufin-embedded-cover.flac";
    let mut tag = Tag::new(TagType::Id3v2);
    tag.push_picture(
        Picture::unchecked(vec![1_u8, 2, 3])
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    );
    let cover = embedded_cover(Path::new(path), None, Some(&tag)).expect("embedded cover");
    let mut scanned = scanned_test_track(1, AlbumId::new("local:album:embedded"), Some(cover));
    scanned.track.local_path = Some(path.to_string());
    let entry = manifest_entry_for_scanned(&test_file_facts(path), &scanned);
    let saved_cover = entry.cover.as_ref().expect("manifest cover");

    assert_eq!(saved_cover.kind, LocalManifestCoverKind::Embedded);
    assert_eq!(saved_cover.source_path, PathBuf::from(path));

    let (reused, _entry, artwork_changed) = reuse_manifest_track(test_file_facts(path), entry);
    let library = build_library(vec![reused], Vec::new(), HashMap::new());

    assert!(!artwork_changed);
    let album_ref = library.albums[0].image_ref.clone().expect("album cover");
    assert_eq!(library.tracks[0].image_ref.as_ref(), Some(&album_ref));
    assert_eq!(library.artists[0].image_ref.as_ref(), Some(&album_ref));
    assert_eq!(
        library.album_artists[0].image_ref.as_ref(),
        Some(&album_ref)
    );
}

#[test]
fn manifest_sync_stores_projected_embedded_cover() {
    let path = PathBuf::from("/tmp/rufin-projected-embedded.flac");
    let cover = LocalCover::Embedded {
        path: path.clone(),
        bytes: Arc::from([1_u8, 2, 3]),
        content_type: Some("image/png".to_string()),
        revision: Some("embedded:projected".to_string()),
    };
    let image_ref = ImageRef::new(cover_id(&cover), cover_revision(&cover));
    let mut scanned = scanned_test_track(1, AlbumId::new("local:album:projected"), None);
    scanned.track.image_ref = Some(image_ref);
    let mut entry = manifest_entry_for_scanned(
        &test_file_facts(path.to_str().expect("test path")),
        &scanned,
    );
    entry.cover = None;
    let library = LocalLibrary {
        tracks: vec![scanned.track],
        ..LocalLibrary::default()
    };

    sync_manifest_covers_from_library(&library, std::slice::from_mut(&mut entry));

    let saved_cover = entry.cover.expect("manifest cover");
    assert_eq!(saved_cover.kind, LocalManifestCoverKind::Embedded);
    assert_eq!(saved_cover.source_path, path);
    assert_eq!(saved_cover.revision, "embedded:projected");
}

#[test]
fn local_scan_reports_progress() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("track.mp3"), []).expect("track file");
    let server = LocalSource::identity_for_root(dir.path()).expect("identity");
    let mut reports = Vec::new();

    let provider = LocalSource::from_roots_with_manifest_cache_and_progress(
        vec![dir.path().to_path_buf()],
        server,
        Vec::new(),
        |progress| reports.push(progress),
    )
    .expect("provider");

    assert_eq!(provider.manifest_scan().counters.audio_candidates, 1);
    assert!(reports.iter().any(
        |progress| progress.stage == LocalScanStage::Walking && progress.audio_candidates == 1
    ));
    assert!(
        reports
            .iter()
            .any(|progress| progress.stage == LocalScanStage::ReadingTags
                && progress.total_tracks == Some(1))
    );
    assert!(
        reports
            .iter()
            .any(|progress| progress.stage == LocalScanStage::BuildingLibrary
                && progress.processed_tracks == 1)
    );
}

#[test]
fn artist_identity_preserves_visible_case_for_hidden_case_only_tag() {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.insert_text(ItemKey::TrackArtists, "FEEDER".to_string());

    let names = artist_names(Some(&tag), "Feeder");
    let visible = artist_credit("Feeder", None);
    let hidden_case = artist_credit("FEEDER", None);

    assert_eq!(names, vec!["Feeder".to_string()]);
    assert_eq!(visible.id, hidden_case.id);
    assert_eq!(visible.name, "Feeder");
}

#[test]
fn musicbrainz_ids_become_optional_identity_data() {
    let recording_id = "b3b3c0bb-1111-4222-8333-123456789abc";
    let release_track_id = "c4c4d1cc-2222-4333-9444-123456789abc";
    let artist_id = "d5d5e2dd-3333-4444-a555-123456789abc";
    let mut tag = Tag::new(TagType::Id3v2);
    tag.push_unchecked(TagItem::new(
        ItemKey::MusicBrainzRecordingId,
        ItemValue::Text(recording_id.to_string()),
    ));
    tag.push_unchecked(TagItem::new(
        ItemKey::MusicBrainzTrackId,
        ItemValue::Text(release_track_id.to_string()),
    ));
    tag.push_unchecked(TagItem::new(
        ItemKey::MusicBrainzArtistId,
        ItemValue::Text(artist_id.to_string()),
    ));

    let credit = artist_credit("Example Artist", Some(artist_id));

    assert_eq!(
        tag_mbid(&tag, ItemKey::MusicBrainzRecordingId).as_deref(),
        Some(recording_id)
    );
    assert_eq!(
        tag_mbid(&tag, ItemKey::MusicBrainzTrackId).as_deref(),
        Some(release_track_id)
    );
    assert_eq!(
        tag_mbids(Some(&tag), ItemKey::MusicBrainzArtistId),
        vec![artist_id.to_string()]
    );
    assert_eq!(credit.musicbrainz_artist_id.as_deref(), Some(artist_id));
    assert!(credit.id.as_str().contains(artist_id));
}

#[test]
fn local_track_tags_include_mood_and_bpm() {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.push_unchecked(TagItem::new(
        ItemKey::Mood,
        ItemValue::Text("Energetic; Focus".to_string()),
    ));
    tag.push_unchecked(TagItem::new(
        ItemKey::IntegerBpm,
        ItemValue::Text("128".to_string()),
    ));

    assert_eq!(
        tag_moods(Some(&tag)),
        vec!["Energetic".to_string(), "Focus".to_string()]
    );
    assert_eq!(tag_bpm(Some(&tag)), Some(128));
}

#[test]
fn local_track_bpm_accepts_decimal_tag_values() {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.push_unchecked(TagItem::new(
        ItemKey::Bpm,
        ItemValue::Text("127.6".to_string()),
    ));

    assert_eq!(tag_bpm(Some(&tag)), Some(128));
}

#[tokio::test]
async fn manifest_scan_update() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("track.mp3"), []).expect("track file");
    let cover = dir.path().join("cover.jpg");
    fs::write(&cover, [1_u8]).expect("cover file");
    let server = LocalSource::identity_for_root(dir.path()).expect("identity");
    let cold =
        LocalSource::from_roots_with_identity(vec![dir.path().to_path_buf()], server.clone())
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

    let warm = LocalSource::from_roots_with_manifest_cache(
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
fn reparsed_track_artwork() {
    let stale = scanned_test_track(
        1,
        AlbumId::new("local:album:one"),
        Some(LocalCover::File {
            path: PathBuf::from("/tmp/cover.jpg"),
            revision: Some("cover-one".to_string()),
        }),
    );
    let current = scanned_test_track(
        1,
        AlbumId::new("local:album:one"),
        Some(LocalCover::File {
            path: PathBuf::from("/tmp/cover.jpg"),
            revision: Some("cover-two".to_string()),
        }),
    );
    let classification =
        classify_reparsed_manifest_entry("/tmp/rufin-track-cover.flac", &stale, &current);

    assert!(classification.changed_track_ids.is_empty());
    assert!(classification.metadata_track_ids.is_empty());
    assert_eq!(classification.artwork_track_ids, vec![TrackId::fake(1)]);
    assert!(classification.retained_track_ids.is_empty());
    assert_eq!(classification.counters.artwork_changed, 1);
}

#[test]
fn metadata_track_reparse() {
    let stale_scanned = scanned_test_track(1, AlbumId::new("local:album:one"), None);
    let mut current_scanned = stale_scanned.clone();
    current_scanned.track.duration_seconds += 1;
    let classification = classify_reparsed_manifest_entry(
        "/tmp/rufin-track-duration.flac",
        &stale_scanned,
        &current_scanned,
    );

    assert!(classification.changed_track_ids.is_empty());
    assert_eq!(classification.metadata_track_ids, vec![TrackId::fake(1)]);
    assert!(classification.artwork_track_ids.is_empty());
    assert!(classification.retained_track_ids.is_empty());
    assert_eq!(classification.counters.artwork_changed, 0);
}

#[test]
fn reparsed_track_changed() {
    let stale_scanned = scanned_test_track(1, AlbumId::new("local:album:one"), None);
    let mut current_scanned = stale_scanned.clone();
    current_scanned.track.album_id = AlbumId::new("local:album:two");
    let classification = classify_reparsed_manifest_entry(
        "/tmp/rufin-track-album-id.flac",
        &stale_scanned,
        &current_scanned,
    );

    assert_eq!(classification.changed_track_ids, vec![TrackId::fake(1)]);
    assert!(classification.metadata_track_ids.is_empty());
    assert!(classification.artwork_track_ids.is_empty());
    assert!(classification.retained_track_ids.is_empty());
    assert_eq!(classification.counters.artwork_changed, 0);
}

#[test]
fn comment_track_reparse() {
    let stale_scanned = scanned_test_track(1, AlbumId::new("local:album:one"), None);
    let mut current_scanned = stale_scanned.clone();
    current_scanned.track.comment = Some("alternate edition".to_string());
    let classification = classify_reparsed_manifest_entry(
        "/tmp/rufin-track-comment.flac",
        &stale_scanned,
        &current_scanned,
    );

    assert!(classification.changed_track_ids.is_empty());
    assert_eq!(classification.metadata_track_ids, vec![TrackId::fake(1)]);
    assert!(classification.artwork_track_ids.is_empty());
    assert!(classification.retained_track_ids.is_empty());
}

#[test]
fn local_share_ref() {
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

#[test]
fn local_repairs_album_ref_from_retained_track_ref() {
    let album_id = AlbumId::new("local:album:test");
    let retained_ref = ImageRef::new("local:cover:retained", Some("retained-tag".to_string()));
    let mut scanned = scanned_test_track(1, album_id, None);
    scanned.track.image_ref = Some(retained_ref.clone());

    let library = build_library(vec![scanned], Vec::new(), HashMap::new());

    assert_eq!(library.albums.len(), 1);
    assert_eq!(library.albums[0].image_ref.as_ref(), Some(&retained_ref));
    assert_eq!(library.tracks[0].image_ref.as_ref(), Some(&retained_ref));
    assert_eq!(library.artists[0].image_ref.as_ref(), Some(&retained_ref));
    assert_eq!(
        library.album_artists[0].image_ref.as_ref(),
        Some(&retained_ref)
    );
}

#[tokio::test]
async fn local_use_bytes() {
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
    let provider = LocalSource {
        identity: SourceIdentity {
            server: identity_for_root(dir.path()),
        },
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
async fn local_reject_file() {
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
    let provider = LocalSource {
        identity: SourceIdentity {
            server: identity_for_root(dir.path()),
        },
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
fn local_read_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cover_path = dir.path().join("folder.jpg");
    fs::write(&cover_path, [1_u8, 2, 3]).expect("cover file");
    let item_id = cover_id(&LocalCover::File {
        path: cover_path,
        revision: None,
    });

    let image = LocalSource::cover_item_bytes(&item_id, vec![dir.path().to_path_buf()])
        .expect("local cover");

    assert_eq!(image.bytes, vec![1, 2, 3]);
    assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
}
#[test]
fn local_reject_root() {
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::tempdir().expect("outside");
    let cover_path = outside.path().join("folder.jpg");
    fs::write(&cover_path, [1_u8, 2, 3]).expect("cover file");
    let item_id = cover_id(&LocalCover::File {
        path: cover_path,
        revision: None,
    });

    let error = LocalSource::cover_item_bytes(&item_id, vec![root.path().to_path_buf()])
        .expect_err("outside-root cover");

    assert_eq!(error.to_string(), "source item was not found");
}
#[test]
fn embedded_reject_picture() {
    let picture = Picture::unchecked(vec![0_u8; LOCAL_COVER_MAX_BYTES + 1]).build();

    let error = picture_data_bounded(&picture).expect_err("oversized embedded cover");

    assert!(error.to_string().contains("embedded cover exceeded"));
}
#[test]
fn folder_use_fallback() {
    let dir = tempfile::tempdir().expect("tempdir");
    let image = dir.path().join("artwork.png");
    fs::write(&image, [1_u8]).expect("image file");

    assert_eq!(folder_cover(dir.path()).as_deref(), Some(image.as_path()));
}
#[test]
fn folder_cover_prefers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let booklet = dir.path().join("booklet.png");
    let cover = dir.path().join("Cover.JPG");
    fs::write(&booklet, [1_u8]).expect("booklet image");
    fs::write(&cover, [2_u8]).expect("cover image");

    assert_eq!(folder_cover(dir.path()).as_deref(), Some(cover.as_path()));
}
#[test]
fn folder_cover_skips() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("back.jpg"), [1_u8]).expect("back image");
    fs::write(dir.path().join("booklet.png"), [2_u8]).expect("booklet image");

    assert!(folder_cover(dir.path()).is_none());
}
#[test]
fn local_cover_artist() {
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
fn local_fall_cover() {
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
async fn local_root_folder() {
    let first = tempfile::tempdir().expect("first root");
    let second = tempfile::tempdir().expect("second root");

    let provider = LocalSource::from_roots(vec![
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
async fn local_track_folders() {
    let root = tempfile::tempdir().expect("root");
    let artist = root.path().join("Artist");
    let album = artist.join("Album");
    fs::create_dir_all(&album).expect("album dir");
    fs::write(artist.join("single.mp3"), []).expect("single track");
    fs::write(album.join("album-track.mp3"), []).expect("album track");
    let provider = LocalSource::from_root(root.path().to_path_buf()).expect("provider");

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
async fn local_reject_id() {
    let root = tempfile::tempdir().expect("root");
    let provider = LocalSource::from_root(root.path().to_path_buf()).expect("provider");
    let outside = FolderId::new("local:folder:%2Fetc%2Fmusic");

    let result = provider.folder(Some(&outside), None).await;

    assert!(matches!(result, Err(SourceError::NotFound)));
}

struct ReparsedClassification {
    changed_track_ids: Vec<TrackId>,
    metadata_track_ids: Vec<TrackId>,
    artwork_track_ids: Vec<TrackId>,
    retained_track_ids: Vec<TrackId>,
    counters: LocalScanCounters,
}

fn classify_reparsed_manifest_entry(
    facts_path: &str,
    stale_scanned: &ScannedTrack,
    current_scanned: &ScannedTrack,
) -> ReparsedClassification {
    let facts = test_file_facts(facts_path);
    let stale = manifest_entry_for_scanned(&facts, stale_scanned);
    let current = manifest_entry_for_scanned(&facts, current_scanned);
    let mut result = ReparsedClassification {
        changed_track_ids: Vec::new(),
        metadata_track_ids: Vec::new(),
        artwork_track_ids: Vec::new(),
        retained_track_ids: Vec::new(),
        counters: LocalScanCounters::default(),
    };

    assert!(classify_reparsed_track(
        Some(&stale),
        &current,
        &mut result.changed_track_ids,
        &mut result.metadata_track_ids,
        &mut result.artwork_track_ids,
        &mut result.retained_track_ids,
        &mut result.counters,
    ));
    result
}

fn scanned_test_track(number: u32, album_id: AlbumId, cover: Option<LocalCover>) -> ScannedTrack {
    let artist = ArtistCredit {
        id: ArtistId::new("local:artist:example"),
        name: "Example Artist".to_string(),
        musicbrainz_artist_id: None,
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
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: Some(format!("/tmp/rufin-track-{number}.flac")),
            source_format: Some("flac".to_string()),
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        },
        album_artist: "Example Artist".to_string(),
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
        cue_source: None,
        cover,
        embedded_cover_path: None,
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
