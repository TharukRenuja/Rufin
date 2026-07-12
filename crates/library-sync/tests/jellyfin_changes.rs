use library::{
    LibrarySync, PlaylistId, SourceEntityKind, SourceId, SourceObjectMapping, Store, SyncCoverage,
    TrackId,
};
use library_sync::{CancellationToken, ChangeSyncOutcome, SyncAttempt, sync_remote_changes};
use sources::jellyfin::{JellyfinConfiguredSession, JellyfinSource, JellyfinSourceConfig};
use sources::{
    CredentialSourceConfig, LibraryChangeResolution, LibraryChangeResolver,
    LibraryObjectObservation, MusicSource, SourceIdentity, SourceObjectChanges,
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn track_change_fetches_its_album_and_commits_the_cache_change() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "track-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "track-one",
                "Name": "First Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "Album": "Blue Rooms",
                "Artists": ["Astral Kin"],
                "ArtistItems": [{ "Id": "artist-one", "Name": "Astral Kin" }],
                "AlbumArtists": [{ "Id": "album-artist-one", "Name": "Astral Kin" }],
                "Genres": ["Ambient"]
            }]
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "album-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "album-one",
                "Name": "Blue Rooms",
                "Type": "MusicAlbum",
                "AlbumArtist": "Astral Kin",
                "AlbumArtists": [{ "Id": "album-artist-one", "Name": "Astral Kin" }],
                "ArtistItems": [{ "Id": "artist-one", "Name": "Astral Kin" }],
                "Genres": ["Ambient"]
            }]
        })))
        .expect(2)
        .mount(&server)
        .await;
    let source = provider(&server, "token-one");

    let resolution = source
        .resolve_changes(
            &SourceObjectChanges::new(["track-one".to_string()]),
            &[mapping("track-one", SourceEntityKind::Track)],
        )
        .await
        .expect("resolve track");
    let observation = exact(resolution);

    assert_eq!(observation.tracks.len(), 1);
    assert_eq!(observation.albums.len(), 1);
    let changes = SourceObjectChanges::new(["track-one".to_string()]);
    let mut initial = (*observation).clone();
    initial.tracks[0].title = "Before change".to_string();
    let (store, source_id) = seed_cached_observation(&source, &initial);

    let updated = run_change(&store, &source_id, &source, &changes).await;
    let ChangeSyncOutcome::Committed(updated) = updated else {
        panic!("expected finite track commit");
    };
    let track_id = TrackId::new("jellyfin:track:track-one");
    assert!(updated.delta.tracks.fields.contains(&track_id));
    assert_eq!(
        store
            .load_track(&source_id, &track_id)
            .expect("load changed track")
            .expect("changed track")
            .title,
        "First Motion"
    );

    let removed_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "track-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 0,
            "Items": []
        })))
        .expect(1)
        .mount(&removed_server)
        .await;
    let removed_source = provider(&removed_server, "token-one");

    let removed = run_change(&store, &source_id, &removed_source, &changes).await;
    let ChangeSyncOutcome::Committed(removed) = removed else {
        panic!("expected typed track tombstone commit");
    };
    assert_eq!(removed.delta.tracks.deleted, vec![track_id.clone()]);
    assert_eq!(
        store
            .load_track(&source_id, &track_id)
            .expect("load removed track"),
        None
    );
    assert_eq!(
        store
            .load_albums(&source_id, 0, 10)
            .expect("load retained album")
            .total,
        1
    );
    assert!(
        store
            .source_object_mappings(&source_id, "track-one")
            .expect("load removed mapping")
            .is_empty()
    );

    let revision = store
        .source_cache_revision(&source_id)
        .expect("cache revision");
    let cached_album = store.load_albums(&source_id, 0, 10).expect("cached album");

    let removed_album_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "album-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 0,
            "Items": []
        })))
        .expect(1)
        .mount(&removed_album_server)
        .await;
    let removed_album_source = provider(&removed_album_server, "token-one");

    let outcome = run_change(
        &store,
        &source_id,
        &removed_album_source,
        &SourceObjectChanges::new(["album-one".to_string()]),
    )
    .await;

    assert_eq!(outcome, ChangeSyncOutcome::NeedsFull);
    assert_eq!(
        store
            .source_cache_revision(&source_id)
            .expect("unchanged revision"),
        revision
    );
    assert_eq!(
        store
            .load_albums(&source_id, 0, 10)
            .expect("unchanged album"),
        cached_album
    );

    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "artist-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 0,
            "Items": []
        })))
        .expect(1)
        .mount(&removed_album_server)
        .await;
    let artist_resolution = removed_album_source
        .resolve_changes(
            &SourceObjectChanges::new(["artist-one".to_string()]),
            &[mapping("artist-one", SourceEntityKind::Artist)],
        )
        .await
        .expect("resolve removed artist");
    assert_eq!(artist_resolution, LibraryChangeResolution::Full);
}

#[tokio::test]
async fn resolves_track_and_playlist_tombstones() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "playlist-one,track-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 0,
            "Items": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server, "token-one");

    let resolution = source
        .resolve_changes(
            &SourceObjectChanges::new(["track-one".to_string(), "playlist-one".to_string()]),
            &[
                mapping("track-one", SourceEntityKind::Track),
                mapping("playlist-one", SourceEntityKind::Playlist),
            ],
        )
        .await
        .expect("resolve tombstones");
    let observation = exact(resolution);

    assert_eq!(
        observation.missing_source_objects,
        ["playlist-one".to_string(), "track-one".to_string()]
            .into_iter()
            .collect()
    );
    assert!(observation.mappings.is_empty());
}

#[tokio::test]
async fn ignores_an_unmapped_nonmusic_item() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "movie-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "movie-one",
                "Name": "Unrelated Movie",
                "Type": "Movie"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server, "token-one");

    let resolution = source
        .resolve_changes(&SourceObjectChanges::new(["movie-one".to_string()]), &[])
        .await
        .expect("resolve nonmusic item");

    assert_eq!(resolution, LibraryChangeResolution::Ignored);
}

#[tokio::test]
async fn playlist_resolution_reads_every_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "playlist-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "playlist-one",
                "Name": "Late Set",
                "Type": "Playlist",
                "ChildCount": 2
            }]
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("StartIndex", "0"))
        .and(query_param("Limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 2,
            "Items": [{
                "Id": "track-one",
                "Name": "First Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "PlaylistItemId": "entry-one"
            }]
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("StartIndex", "1"))
        .and(query_param("Limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 2,
            "Items": [{
                "Id": "track-two",
                "Name": "Second Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "PlaylistItemId": "entry-two"
            }]
        })))
        .expect(2)
        .mount(&server)
        .await;
    let source = provider(&server, "token-one");

    let resolution = source
        .resolve_changes(
            &SourceObjectChanges::new(["playlist-one".to_string()]),
            &[mapping("playlist-one", SourceEntityKind::Playlist)],
        )
        .await
        .expect("resolve playlist");
    let observation = exact(resolution);

    assert_eq!(observation.playlists.len(), 1);
    assert_eq!(observation.playlists[0].entries.len(), 2);
    assert_eq!(observation.playlists[0].entries[0].entry_id, "entry-one");
    assert_eq!(observation.playlists[0].entries[1].entry_id, "entry-two");

    let (store, source_id) = seed_cached_observation(&source, &LibraryObjectObservation::default());
    let outcome = run_change(
        &store,
        &source_id,
        &source,
        &SourceObjectChanges::new(["playlist-one".to_string()]),
    )
    .await;
    let ChangeSyncOutcome::Committed(commit) = outcome else {
        panic!("expected finite playlist commit");
    };
    assert_eq!(
        commit.delta.playlists.added,
        vec![PlaylistId::new("jellyfin:playlist:playlist-one")]
    );
    assert_eq!(
        store
            .playlist_entry_keys(
                &source_id,
                &PlaylistId::new("jellyfin:playlist:playlist-one")
            )
            .expect("playlist entries")
            .len(),
        2
    );
}

fn mapping(raw_id: &str, kind: SourceEntityKind) -> SourceObjectMapping {
    let item_kind = match kind {
        SourceEntityKind::Album => "album",
        SourceEntityKind::Track => "track",
        SourceEntityKind::Artist | SourceEntityKind::AlbumArtist => "artist",
        SourceEntityKind::Genre => "genre",
        SourceEntityKind::Playlist => "playlist",
        SourceEntityKind::MusicFolder => "music-folder",
    };
    SourceObjectMapping {
        source_object_id: raw_id.to_string(),
        entity_kind: kind,
        entity_id: format!("jellyfin:{item_kind}:{raw_id}"),
    }
}

fn provider(server: &MockServer, token: &str) -> JellyfinSource {
    JellyfinSource::from_configured_session(JellyfinConfiguredSession {
        source: SourceIdentity {
            id: SourceId::new("jellyfin:server:test"),
            kind: "jellyfin".to_string(),
            name: "Test".to_string(),
            base_url: server.uri(),
        },
        user_id: "user-one".to_string(),
        trust_invalid_cert: false,
        access_token: token.to_string(),
        device_id: "rufin-install-one".to_string(),
    })
    .expect("provider")
}

fn exact(resolution: LibraryChangeResolution) -> Box<LibraryObjectObservation> {
    match resolution {
        LibraryChangeResolution::Exact(observation) => observation,
        LibraryChangeResolution::Full => panic!("expected exact resolution"),
        LibraryChangeResolution::Ignored => panic!("expected exact resolution"),
    }
}

async fn run_change(
    store: &Store,
    source_id: &SourceId,
    resolver: &JellyfinSource,
    changes: &SourceObjectChanges,
) -> ChangeSyncOutcome {
    let generation = store.begin_sync(source_id).expect("begin change sync");
    let base_cache_revision = store
        .source_cache_revision(source_id)
        .expect("cache revision");
    let cancellation = CancellationToken::new();
    let mut progress = |_| {};
    let mut attempt = SyncAttempt {
        store,
        source_id,
        generation,
        base_cache_revision,
        cancellation: &cancellation,
        progress: &mut progress,
    };
    sync_remote_changes(&mut attempt, resolver, changes)
        .await
        .expect("sync resolved change")
}

fn seed_cached_observation(
    source: &JellyfinSource,
    observation: &LibraryObjectObservation,
) -> (Store, SourceId) {
    let source_id = source.identity().id.clone();
    let store = Store::open_memory().expect("open Store");
    let stored = JellyfinSourceConfig {
        credentials: CredentialSourceConfig {
            source: source.identity().clone(),
            user_id: "user-one".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
        },
        use_instant_mix: false,
    }
    .into_stored();
    store.save_source(&stored).expect("save source");
    let generation = store.begin_sync(&source_id).expect("begin seed sync");
    let base_cache_revision = store
        .source_cache_revision(&source_id)
        .expect("cache revision");
    store
        .commit_library_sync(
            &source_id,
            generation,
            base_cache_revision,
            LibrarySync {
                albums: observation.albums.clone(),
                tracks: observation.tracks.clone(),
                artists: observation.artists.clone(),
                album_artists: observation.album_artists.clone(),
                genres: observation.genres.clone(),
                playlists: observation.playlists.clone(),
                home_sections: observation.home_sections.clone(),
                mappings: observation.mappings.clone(),
                coverage: SyncCoverage::All {
                    music_folders: Vec::new(),
                },
                local_access: None,
            },
        )
        .expect("seed library");
    (store, source_id)
}
