use super::test_support::*;
use crate::{ActivityOutcome, PlaybackCheckpointRecord, StoreError};

#[test]
fn sync_hide_artist() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let primary_artist = Artist {
        id: ArtistId::fake(1),
        name: "Primary Artist".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
        representative_albums: Vec::new(),
    };
    let linked_artist = Artist {
        id: ArtistId::fake(2),
        name: "Featured Artist".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
        representative_albums: Vec::new(),
    };
    let mut album = album(1);
    album.artist = primary_artist.name.clone();
    album.artist_id = Some(primary_artist.id.clone());
    let mut track = track(1, &album);
    track.artist = primary_artist.name.clone();
    track.artist_id = Some(primary_artist.id.clone());
    track.artist_credits = vec![
        credit(primary_artist.id.clone(), &primary_artist.name),
        credit(linked_artist.id.clone(), &linked_artist.name),
    ];
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.source_id,
            &[primary_artist.clone(), linked_artist.clone()],
            false,
            generation,
        )
        .expect("upsert artists");
    store
        .refresh_library_counts(&saved.source_id)
        .expect("refresh counts");
    let artist_page = store
        .load_artists(&saved.source_id, false, 0, 10)
        .expect("load artists");
    assert_eq!(artist_page.items.len(), 1);
    assert_eq!(artist_page.items[0].id, primary_artist.id);
    assert_eq!(artist_page.items[0].name, primary_artist.name);
    assert_eq!(
        artist_page.items[0]
            .representative_albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>(),
        vec![album.id.clone()]
    );
    let linked_search = store
        .load_artists_matching(&saved.source_id, false, "Featured Artist", 0, 10)
        .expect("search artists");
    assert!(linked_search.items.is_empty());
    let detail = store
        .load_artist_detail(&saved.source_id, &linked_artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.appears_on.len(), 1);
    assert_eq!(detail.appears_on[0].id, album.id);
}
#[test]
fn sync_cache_genre() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album_artist_id = ArtistId::fake(18);
    let track_artist_id = ArtistId::fake(19);
    let mut album = album(8);
    album.album_artist_credits = vec![credit(album_artist_id.clone(), "Linked Album Artist")];
    album.release_date = Some("2024-03-01".to_string());
    album.date_added = Some("2024-03-02T09:10:11Z".to_string());
    album.last_played = Some("2024-04-02T09:10:11Z".to_string());
    album.play_count = Some(17);
    album.user_rating = Some(5);
    album.genres = vec!["Dream Pop".to_string(), "Shoegaze".to_string()];
    let mut track_one = track(2, &album);
    track_one.track_number = 2;
    track_one.artist_credits = vec![credit(track_artist_id.clone(), "Track Artist")];
    track_one.release_date = Some("2024-03-01".to_string());
    track_one.date_added = Some("2024-03-03T09:10:11Z".to_string());
    track_one.last_played = Some("2024-04-03T09:10:11Z".to_string());
    track_one.play_count = Some(11);
    track_one.user_rating = Some(4);
    track_one.genres = vec!["Dream Pop".to_string()];
    let mut track_two = track(1, &album);
    track_two.track_number = 1;
    track_two.artist_credits = vec![credit(track_artist_id.clone(), "Track Artist")];
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.source_id,
            &[track_one.clone(), track_two.clone()],
            generation,
        )
        .expect("upsert tracks");
    let mut loaded_albums = store
        .load_albums(&saved.source_id, 0, 10)
        .expect("load albums")
        .items;
    let loaded_album = loaded_albums.pop().expect("album");
    assert_eq!(loaded_album.release_date.as_deref(), Some("2024-03-01"));
    assert_eq!(
        loaded_album.date_added.as_deref(),
        Some("2024-03-02T09:10:11Z")
    );
    assert_eq!(
        loaded_album.last_played.as_deref(),
        Some("2024-04-02T09:10:11Z")
    );
    assert_eq!(loaded_album.play_count, Some(17));
    assert_eq!(loaded_album.user_rating, Some(5));
    assert_eq!(
        loaded_album.genres,
        vec!["Dream Pop".to_string(), "Shoegaze".to_string()]
    );
    assert_eq!(
        loaded_album.album_artist_credits,
        vec![credit(album_artist_id.clone(), "Linked Album Artist")]
    );
    let tracks = store
        .load_tracks(&saved.source_id, 0, 10)
        .expect("load tracks")
        .items;
    let loaded_track = tracks
        .iter()
        .find(|track| track.id == track_one.id)
        .expect("track");
    assert_eq!(loaded_track.release_date.as_deref(), Some("2024-03-01"));
    assert_eq!(
        loaded_track.date_added.as_deref(),
        Some("2024-03-03T09:10:11Z")
    );
    assert_eq!(
        loaded_track.last_played.as_deref(),
        Some("2024-04-03T09:10:11Z")
    );
    assert_eq!(loaded_track.play_count, Some(11));
    assert_eq!(loaded_track.user_rating, Some(4));
    assert_eq!(loaded_track.genres, vec!["Dream Pop".to_string()]);
    assert_eq!(
        loaded_track.artist_credits,
        vec![credit(track_artist_id, "Track Artist")]
    );
    assert_eq!(
        loaded_track.album_artist_credits,
        vec![credit(album_artist_id, "Linked Album Artist")]
    );
    let by_album = store
        .load_tracks_for_albums(&saved.source_id, std::slice::from_ref(&album.id))
        .expect("load album tracks");
    let album_tracks = by_album.get(&album.id).expect("album tracks");
    assert_eq!(
        album_tracks
            .iter()
            .map(|track| track.track_number)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(album_tracks[1].artist_credits[0].name, "Track Artist");
    assert_eq!(
        album_tracks[1].album_artist_credits[0].name,
        "Linked Album Artist"
    );
}
#[test]
fn sync_prune_success() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let album_one = album(1);
    let album_two = album(2);
    let first_generation = store.begin_sync(&saved.source_id).expect("begin first");
    LibraryObservation {
        albums: vec![album_one.clone(), album_two],
        ..LibraryObservation::default()
    }
    .commit(&store, &saved.source_id, first_generation)
    .expect("commit first");
    let second_generation = store.begin_sync(&saved.source_id).expect("begin second");
    LibraryObservation {
        albums: vec![album_one],
        ..LibraryObservation::default()
    }
    .commit(&store, &saved.source_id, second_generation)
    .expect("commit second");
    let albums = store
        .load_albums(&saved.source_id, 0, 10)
        .expect("load albums");
    assert_eq!(albums.total, 1);
}

#[test]
fn stale_sync_generation_cannot_write_or_complete() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let stale = store
        .begin_sync(&saved.source_id)
        .expect("begin stale sync");
    let current = store
        .begin_sync(&saved.source_id)
        .expect("begin replacement sync");

    let write_error = store
        .upsert_albums(&saved.source_id, &[album(1)], stale)
        .expect_err("stale generation write");
    assert!(matches!(
        write_error,
        StoreError::StaleSyncGeneration { generation, .. } if generation == stale
    ));
    assert_eq!(
        store
            .load_albums(&saved.source_id, 0, 10)
            .expect("load albums")
            .total,
        0
    );

    let complete_error = store
        .commit_library_sync(
            &saved.source_id,
            stale,
            0,
            LibrarySync {
                albums: Vec::new(),
                tracks: Vec::new(),
                artists: Vec::new(),
                album_artists: Vec::new(),
                genres: Vec::new(),
                playlists: Vec::new(),
                home_sections: Vec::new(),
                mappings: Vec::new(),
                coverage: SyncCoverage::All {
                    music_folders: Vec::new(),
                },
                local_access: None,
            },
        )
        .expect_err("stale generation completion");
    assert!(matches!(
        complete_error,
        StoreError::StaleSyncGeneration { generation, .. } if generation == stale
    ));
    let state = store.sync_state(&saved.source_id).expect("sync state");
    assert_eq!(state.generation, current);
    assert_eq!(state.status, "running");

    store
        .fail_sync(&saved.source_id, stale, "stale failure")
        .expect("ignore stale failure");
    let state = store.sync_state(&saved.source_id).expect("sync state");
    assert_eq!(state.generation, current);
    assert_eq!(state.status, "running");
    assert!(state.last_error.is_none());
}
#[test]
fn sync_track_order() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album_one = album(1);
    let album_two = album(2);
    let track_one = track(1, &album_one);
    let track_two = track(2, &album_two);
    store
        .upsert_albums(
            &saved.source_id,
            &[album_one.clone(), album_two.clone()],
            generation,
        )
        .expect("upsert albums");
    store
        .upsert_tracks(
            &saved.source_id,
            &[track_one.clone(), track_two.clone()],
            generation,
        )
        .expect("upsert tracks");
    store
        .upsert_home_sections(
            &saved.source_id,
            &[
                HomeSection {
                    kind: HomeSectionKind::Explore,
                    albums: vec![album_two.clone(), album_one.clone()],
                    tracks: Vec::new(),
                },
                HomeSection {
                    kind: HomeSectionKind::MostPlayed,
                    albums: Vec::new(),
                    tracks: vec![track_two.clone(), track_one.clone()],
                },
            ],
            generation,
        )
        .expect("upsert home sections");
    let sections = store
        .load_home_sections(&saved.source_id)
        .expect("load home sections");
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].kind, HomeSectionKind::Explore);
    assert_eq!(sections[0].albums[0].id, album_two.id);
    assert_eq!(sections[0].albums[1].id, album_one.id);
    assert_eq!(sections[1].kind, HomeSectionKind::MostPlayed);
    assert_eq!(sections[1].tracks[0].id, track_two.id);
    assert_eq!(sections[1].tracks[1].id, track_one.id);
}
#[test]
fn sync_home_empty() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    let sections = store
        .load_home_sections(&saved.source_id)
        .expect("load home sections");
    assert!(sections.is_empty());
}
#[test]
fn sync_replace_section() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let visible_album = album(1);
    let prefetched_album = album(2);
    store
        .upsert_albums(
            &saved.source_id,
            &[visible_album.clone(), prefetched_album.clone()],
            generation,
        )
        .expect("upsert albums");
    store
        .upsert_home_section(
            &saved.source_id,
            &HomeSection {
                kind: HomeSectionKind::Explore,
                albums: vec![visible_album.clone()],
                tracks: Vec::new(),
            },
            generation,
        )
        .expect("upsert visible Explore");
    store
        .upsert_home_section_prefetch(
            &saved.source_id,
            &HomeSection {
                kind: HomeSectionKind::Explore,
                albums: vec![prefetched_album.clone()],
                tracks: Vec::new(),
            },
            generation,
        )
        .expect("upsert prefetched Explore");
    let visible = store
        .load_home_sections(&saved.source_id)
        .expect("load visible sections");
    let prefetched = store
        .load_home_section_prefetch(&saved.source_id, HomeSectionKind::Explore)
        .expect("load prefetched Explore")
        .expect("prefetched Explore");
    assert_eq!(visible[0].albums[0].id, visible_album.id);
    assert_eq!(prefetched.albums[0].id, prefetched_album.id);
    store
        .clear_home_section_prefetch(&saved.source_id, HomeSectionKind::Explore)
        .expect("clear prefetched Explore");
    assert!(
        store
            .load_home_section_prefetch(&saved.source_id, HomeSectionKind::Explore)
            .expect("load cleared prefetched Explore")
            .is_none()
    );
}

fn page_shape<T>(page: crate::PagedResponse<T>) -> (usize, usize) {
    (page.total, page.items.len())
}

fn collection_shapes(
    store: &Store,
    source_id: &SourceId,
    offset: usize,
    limit: usize,
) -> StoreResult<[(usize, usize); 5]> {
    Ok([
        page_shape(store.load_artists(source_id, false, offset, limit)?),
        page_shape(store.load_artists(source_id, true, offset, limit)?),
        page_shape(store.load_moods(source_id, offset, limit)?),
        page_shape(store.load_playlists(source_id, offset, limit)?),
        page_shape(store.load_smart_playlists(source_id, offset, limit)?),
    ])
}

#[test]
fn complete_collection_reads_skip_only_exact_terminal_counts() {
    let store = Store::open_memory().expect("open store");
    let source_id = SourceId::new("test:complete-collections");
    let limit = i64::MAX as usize;

    let (pages, statements) = trace_read_statements(&store, || {
        collection_shapes(&store, &source_id, 0, limit).expect("load empty collections")
    });
    assert_eq!(pages, [(0, 0); 5]);
    assert_eq!(statements.len(), 5);

    let (pages, statements) = trace_read_statements(&store, || {
        collection_shapes(&store, &source_id, 1, limit).expect("load beyond empty collections")
    });
    assert_eq!(pages, [(0, 0); 5]);
    assert_eq!(statements.len(), 10);
}

#[test]
fn complete_collection_totals_preserve_paged_contract() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let album = album(1);
    let mut tracks = vec![track(1, &album), track(2, &album)];
    tracks[0].moods = vec!["Calm".to_string()];
    tracks[1].moods = vec!["Focused".to_string()];
    let mut artists = vec![artist(1, None), artist(2, None)];
    let mut album_artists = vec![artist(11, None), artist(12, None)];
    for artist in artists.iter_mut().chain(album_artists.iter_mut()) {
        artist.track_count = 1;
    }
    let playlists = vec![playlist(1, None), playlist(2, None)];
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_artists(&case.id, &artists, false, generation)
        .expect("upsert artists");
    case.upsert_artists(&case.id, &album_artists, true, generation)
        .expect("upsert album artists");
    case.upsert_playlists(&case.id, &playlists, generation)
        .expect("upsert playlists");

    let limit = i64::MAX as usize;
    assert_eq!(
        collection_shapes(&case, &case.id, 0, limit).expect("load complete collections"),
        [(2, 2), (2, 2), (2, 2), (2, 2), (3, 3)]
    );
    assert_eq!(
        collection_shapes(&case, &case.id, 0, 1).expect("load first pages"),
        [(2, 1), (2, 1), (2, 1), (2, 1), (3, 1)]
    );
    assert_eq!(
        collection_shapes(&case, &case.id, 10, limit).expect("load beyond collection ends"),
        [(2, 0), (2, 0), (2, 0), (2, 0), (3, 0)]
    );
}

#[test]
fn home_overview_reads_all_sections_once() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let genre = genre(1, None);
    let mut album = album_with_image(1);
    album.genres = vec![genre.name.clone()];
    let mut track = track(1, &album);
    track.genres = album.genres.clone();
    store
        .upsert_genres(&saved.source_id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let sections = [
        (HomeSectionKind::Explore, true),
        (HomeSectionKind::MostPlayed, false),
        (HomeSectionKind::NewlyAdded, true),
        (HomeSectionKind::RecentlyPlayed, false),
        (HomeSectionKind::RecentlyReleased, true),
    ]
    .into_iter()
    .map(|(kind, albums)| HomeSection {
        kind,
        albums: albums.then(|| album.clone()).into_iter().collect(),
        tracks: (!albums).then(|| track.clone()).into_iter().collect(),
    })
    .collect::<Vec<_>>();
    store
        .upsert_home_sections(&saved.source_id, &sections, generation)
        .expect("upsert Home sections");

    let (overview, statements) = trace_read_statements(&store, || {
        store
            .load_home_overview_projection(&saved.source_id, 12)
            .expect("load Home overview")
    });

    assert_eq!(overview.sections.len(), 5);
    assert_eq!(overview.genres.len(), 1);
    assert_eq!(statements.len(), 11);

    let mut album_without_direct_art = album;
    album_without_direct_art.image_ref = None;
    store
        .upsert_albums(
            &saved.source_id,
            std::slice::from_ref(&album_without_direct_art),
            generation,
        )
        .expect("remove direct album art");
    let (overview, statements) = trace_read_statements(&store, || {
        store
            .load_home_overview_projection(&saved.source_id, 12)
            .expect("load Home overview with album art fallback")
    });

    assert_eq!(overview.sections.len(), 5);
    assert_eq!(statements.len(), 12);
}

#[test]
fn track_only_home_uses_its_album_as_showcase_fallback() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album_with_image(1);
    let track = track(1, &album);
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_home_sections(
            &saved.source_id,
            &[HomeSection {
                kind: HomeSectionKind::MostPlayed,
                albums: Vec::new(),
                tracks: vec![track],
            }],
            generation,
        )
        .expect("upsert track-only Home");

    let overview = store
        .load_home_overview_projection(&saved.source_id, 12)
        .expect("load track-only Home");

    assert_eq!(overview.sections.len(), 1);
    assert_eq!(
        overview.showcase_fallback.expect("showcase fallback").id,
        album.id
    );
}

#[test]
fn home_projection_survives_a_running_source_sync() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let visible_album = album_with_image(1);
    let hidden_album = album_with_image(2);
    LibraryObservation {
        albums: vec![visible_album.clone(), hidden_album.clone()],
        ..LibraryObservation::default()
    }
    .commit(&store, &saved.source_id, generation)
    .expect("seed mapped albums");
    let visible = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![visible_album.clone()],
        tracks: Vec::new(),
    };
    let hidden = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![hidden_album.clone()],
        tracks: Vec::new(),
    };
    let sync_cache_revision = store
        .source_cache_revision(&saved.source_id)
        .expect("sync cache revision");
    let sync_input_revision = store
        .source_sync_input_revision(&saved.source_id)
        .expect("sync input revision");

    let sync_generation = store
        .begin_sync(&saved.source_id)
        .expect("begin source sync");
    let visible_commit = store
        .replace_home_section(&saved.source_id, &visible)
        .expect("commit visible Explore during sync");
    let hidden_commit = store
        .save_home_section_prefetch(&saved.source_id, &hidden)
        .expect("commit hidden Explore during sync");

    let stale = store
        .require_source_cache_revision(&saved.source_id, sync_cache_revision)
        .expect_err("the old shared cache revision is stale");
    assert!(matches!(
        stale,
        StoreError::StaleCacheRevision {
            revision,
            current,
            ..
        } if revision == sync_cache_revision && current == hidden_commit.commit.cache_revision
    ));
    assert_eq!(
        store
            .source_sync_input_revision(&saved.source_id)
            .expect("unchanged sync input revision"),
        sync_input_revision
    );

    let source_explore = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![hidden_album.clone()],
        tracks: Vec::new(),
    };
    let sync_commit = LibraryObservation {
        albums: vec![visible_album, hidden_album],
        home_sections: vec![source_explore],
        ..LibraryObservation::default()
    }
    .commit(&store, &saved.source_id, sync_generation)
    .expect("commit source sync without overwriting Home");

    assert_eq!(
        visible_commit.commit.cache_revision,
        sync_cache_revision + 1
    );
    assert_eq!(
        hidden_commit.commit.cache_revision,
        visible_commit.commit.cache_revision + 1
    );
    assert_eq!(
        sync_commit.cache_revision,
        hidden_commit.commit.cache_revision + 1
    );
    let visible_after_sync = store
        .load_home_sections(&saved.source_id)
        .expect("load visible Explore after sync");
    assert_eq!(visible_after_sync[0].albums[0].id, visible.albums[0].id);
    let committed_hidden = hidden_commit.section.expect("hidden Explore projection");
    assert_eq!(committed_hidden.kind, hidden.kind);
    assert_eq!(committed_hidden.albums[0].id, hidden.albums[0].id);
}
#[test]
fn sync_remove_state() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    let checkpoint = PlaybackCheckpointRecord {
        source_id: saved.source_id.clone(),
        revision: 1,
        selected_occurrence_id: Some("occurrence-1".to_string()),
        progress_millis: 0,
        repeat_mode: "Off".to_string(),
        shuffle_enabled: false,
        payload: "opaque queue".to_string(),
    };
    store.save_source(&saved).expect("save server");
    store
        .set_active_source(&saved.source_id)
        .expect("set active");
    store
        .save_playback_checkpoint(&checkpoint)
        .expect("save playback checkpoint");
    store
        .record_activity_outcome(&ActivityOutcome {
            source_id: saved.source_id.clone(),
            period: "2026-07".to_string(),
            track_id: TrackId::fake(1),
            qualified_plays: 1,
            skips: 0,
            last_played_at: Some(1_783_850_400),
        })
        .expect("record activity");
    seed_cached_library(&store, &saved.source_id);
    store
        .forget_source(&saved.source_id)
        .expect("forget server");
    assert_eq!(store.active_source().expect("active server"), None);
    assert!(store.list_sources().expect("sources").is_empty());
    assert_eq!(
        store
            .load_playback_checkpoint(&saved.source_id)
            .expect("playback checkpoint"),
        None
    );
    assert_eq!(
        store
            .track_activity_summary(&saved.source_id, &TrackId::fake(1))
            .expect("track activity"),
        Default::default()
    );
    assert_eq!(
        store
            .load_tracks(&saved.source_id, 0, 10)
            .expect("tracks")
            .total,
        0
    );
    assert!(
        store.sync_state(&saved.source_id).is_err(),
        "forgotten server should not have sync state"
    );
}
