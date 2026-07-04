use super::test_support::*;

#[test]
fn sync_hide_artist() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
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
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.source.id,
            &[primary_artist.clone(), linked_artist.clone()],
            false,
            generation,
        )
        .expect("upsert artists");
    store
        .refresh_library_counts(&saved.source.id)
        .expect("refresh counts");
    let artist_page = store
        .load_artists(&saved.source.id, false, 0, 10)
        .expect("load artists");
    assert_eq!(artist_page.items, vec![primary_artist]);
    let linked_search = store
        .load_artists_matching(&saved.source.id, false, "Featured Artist", 0, 10)
        .expect("search artists");
    assert!(linked_search.items.is_empty());
    let detail = store
        .load_artist_detail(&saved.source.id, &linked_artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.appears_on.len(), 1);
    assert_eq!(detail.appears_on[0].id, album.id);
}
#[test]
fn sync_cache_genre() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
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
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.source.id,
            &[track_one.clone(), track_two.clone()],
            generation,
        )
        .expect("upsert tracks");
    let mut loaded_albums = store
        .load_albums(&saved.source.id, 0, 10)
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
        .load_tracks(&saved.source.id, 0, 10)
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
        .load_tracks_for_albums(&saved.source.id, std::slice::from_ref(&album.id))
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
fn search_uses_local_fts_rows() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(7);
    let track = track(4, &album);
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let results = store
        .search_library(&saved.source.id, "Album 7", 10)
        .expect("search");
    assert_eq!(results.albums, vec![album]);
    assert_eq!(results.tracks, vec![track]);
}
#[test]
fn sync_prune_success() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let album_one = album(1);
    let album_two = album(2);
    let first_generation = store.begin_sync(&saved.source.id).expect("begin first");
    store
        .upsert_albums(
            &saved.source.id,
            &[album_one.clone(), album_two],
            first_generation,
        )
        .expect("upsert first");
    store
        .complete_sync(&saved.source.id, first_generation)
        .expect("complete first");
    let second_generation = store.begin_sync(&saved.source.id).expect("begin second");
    store
        .upsert_albums(&saved.source.id, &[album_one], second_generation)
        .expect("upsert second");
    store
        .complete_sync(&saved.source.id, second_generation)
        .expect("complete second");
    let albums = store
        .load_albums(&saved.source.id, 0, 10)
        .expect("load albums");
    assert_eq!(albums.total, 1);
}
#[test]
fn sync_track_order() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album_one = album(1);
    let album_two = album(2);
    let track_one = track(1, &album_one);
    let track_two = track(2, &album_two);
    store
        .upsert_albums(
            &saved.source.id,
            &[album_one.clone(), album_two.clone()],
            generation,
        )
        .expect("upsert albums");
    store
        .upsert_tracks(
            &saved.source.id,
            &[track_one.clone(), track_two.clone()],
            generation,
        )
        .expect("upsert tracks");
    store
        .upsert_home_sections(
            &saved.source.id,
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
        .load_home_sections(&saved.source.id)
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
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    let sections = store
        .load_home_sections(&saved.source.id)
        .expect("load home sections");
    assert!(sections.is_empty());
}
#[test]
fn sync_replace_section() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let visible_album = album(1);
    let prefetched_album = album(2);
    store
        .upsert_albums(
            &saved.source.id,
            &[visible_album.clone(), prefetched_album.clone()],
            generation,
        )
        .expect("upsert albums");
    store
        .upsert_home_section(
            &saved.source.id,
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
            &saved.source.id,
            &HomeSection {
                kind: HomeSectionKind::Explore,
                albums: vec![prefetched_album.clone()],
                tracks: Vec::new(),
            },
            generation,
        )
        .expect("upsert prefetched Explore");
    let visible = store
        .load_home_sections(&saved.source.id)
        .expect("load visible sections");
    let prefetched = store
        .load_home_section_prefetch(&saved.source.id, HomeSectionKind::Explore)
        .expect("load prefetched Explore")
        .expect("prefetched Explore");
    assert_eq!(visible[0].albums[0].id, visible_album.id);
    assert_eq!(prefetched.albums[0].id, prefetched_album.id);
    store
        .clear_home_section_prefetch(&saved.source.id, HomeSectionKind::Explore)
        .expect("clear prefetched Explore");
    assert!(
        store
            .load_home_section_prefetch(&saved.source.id, HomeSectionKind::Explore)
            .expect("load cleared prefetched Explore")
            .is_none()
    );
}
#[test]
fn cover_cache_index_round_trips() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let entry = cover_entry(&saved.source.id);
    store
        .save_cover_cache_entry(&entry)
        .expect("save cover cache");
    assert_eq!(
        store
            .load_cover_cache_entry(&saved.source.id, "album-one", "tag-one", 256)
            .expect("load cover cache"),
        Some(entry)
    );
}

#[test]
fn external_cover_cache_reuses_content_identity() {
    let store = Store::open_memory().expect("open store");
    let first = saved_source();
    let mut second = saved_source();
    second.source.id = SourceId::new("jellyfin:server:second");
    store.save_source(&first).expect("save first server");
    store.save_source(&second).expect("save second server");
    let entry = CoverCacheEntry {
        source_id: first.source.id.clone(),
        item_id: "external:mb-release-group:group-one".to_string(),
        image_tag: "external-v2-tag".to_string(),
        size: 256,
        path: "/tmp/rufin-external-shared-cover.jpg".to_string(),
    };
    store
        .save_cover_cache_entry(&entry)
        .expect("save external cover");

    assert_eq!(
        store
            .load_cover_cache_entry(&second.source.id, &entry.item_id, &entry.image_tag, 256)
            .expect("same-server lookup"),
        None
    );
    assert_eq!(
        store
            .load_external_cover_cache_entry_by_content(&entry.item_id, &entry.image_tag, 256)
            .expect("content lookup"),
        Some(entry)
    );
}

#[test]
fn external_cover_content_cache_records_path_and_miss() {
    let store = Store::open_memory().expect("open store");
    let item_id = "external:mb-release-group:group-one";
    let image_tag = "external-v2-tag";

    assert_eq!(
        store
            .load_external_cover_content_path(item_id, image_tag, 256)
            .expect("initial content path"),
        None
    );

    store
        .save_external_cover_content_miss(item_id, image_tag, 256, "not found")
        .expect("save content miss");
    assert!(
        store
            .load_external_cover_content_miss(item_id, image_tag, 256)
            .expect("load content miss")
    );

    store
        .save_external_cover_content_path(
            item_id,
            image_tag,
            256,
            "/tmp/rufin-external-shared-cover.jpg",
        )
        .expect("save content path");
    assert_eq!(
        store
            .load_external_cover_content_path(item_id, image_tag, 256)
            .expect("load content path"),
        Some("/tmp/rufin-external-shared-cover.jpg".to_string())
    );
    assert!(
        !store
            .load_external_cover_content_miss(item_id, image_tag, 256)
            .expect("miss cleared by content path")
    );
}

#[test]
fn external_lookup_miss_reuses_content_identity() {
    let store = Store::open_memory().expect("open store");
    let first = saved_source();
    let mut second = saved_source();
    second.source.id = SourceId::new("jellyfin:server:second");
    store.save_source(&first).expect("save first server");
    store.save_source(&second).expect("save second server");
    let item_id = "external:mb-release-group:group-one";
    let image_tag = "external-v2-tag";

    store
        .save_external_image_lookup_miss(&first.source.id, item_id, image_tag, 256, "not found")
        .expect("save external miss");

    assert!(
        !store
            .load_external_image_lookup_miss(&second.source.id, item_id, image_tag, 256)
            .expect("same-server miss")
    );
    assert!(
        store
            .load_external_image_lookup_miss_by_content(item_id, image_tag, 256)
            .expect("content miss")
    );
}

#[test]
fn sync_delete_entry() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let entry = cover_entry(&saved.source.id);
    store
        .save_cover_cache_entry(&entry)
        .expect("save cover cache");
    store
        .delete_cover_cache_entry(&saved.source.id, "album-one", "tag-one", 256)
        .expect("delete cover cache");
    assert_eq!(
        store
            .load_cover_cache_entry(&saved.source.id, "album-one", "tag-one", 256)
            .expect("load cover cache"),
        None
    );
}
#[test]
fn sync_prune_misses() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let mut album = album(1);
    album.image_ref = Some(ImageRef::new("album-one", Some("tag-one".to_string())));
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    for entry in [
        cover_entry(&saved.source.id),
        CoverCacheEntry {
            source_id: saved.source.id.clone(),
            item_id: "album-one".to_string(),
            image_tag: "old-tag".to_string(),
            size: 256,
            path: "/tmp/rufin-old-cover.jpg".to_string(),
        },
        CoverCacheEntry {
            source_id: saved.source.id.clone(),
            item_id: "external:album:artist:album".to_string(),
            image_tag: "external-tag".to_string(),
            size: 256,
            path: "/tmp/rufin-external-cover.jpg".to_string(),
        },
    ] {
        store
            .save_cover_cache_entry(&entry)
            .expect("save cover cache");
    }
    store
        .save_external_image_lookup_miss(&saved.source.id, "album-one", "old-tag", 256, "old")
        .expect("save old miss");
    store
        .save_external_image_lookup_miss(
            &saved.source.id,
            "external:album:artist:album",
            "external-tag",
            256,
            "external",
        )
        .expect("save external miss");

    store
        .complete_sync(&saved.source.id, generation)
        .expect("complete sync");

    assert!(
        store
            .load_cover_cache_entry(&saved.source.id, "album-one", "tag-one", 256)
            .expect("live cover")
            .is_some()
    );
    assert!(
        store
            .load_cover_cache_entry(&saved.source.id, "album-one", "old-tag", 256)
            .expect("stale cover")
            .is_none()
    );
    assert!(
        !store
            .load_external_image_lookup_miss(&saved.source.id, "album-one", "old-tag", 256)
            .expect("stale miss")
    );
    assert!(
        store
            .load_cover_cache_entry(
                &saved.source.id,
                "external:album:artist:album",
                "external-tag",
                256,
            )
            .expect("external cover")
            .is_some()
    );
    assert!(
        store
            .load_external_image_lookup_miss(
                &saved.source.id,
                "external:album:artist:album",
                "external-tag",
                256,
            )
            .expect("external miss")
    );
}

#[test]
fn sync_disable_lookup() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let entry = CoverCacheEntry {
        source_id: saved.source.id.clone(),
        item_id: "external:album:artist:album".to_string(),
        image_tag: "external-tag".to_string(),
        size: 256,
        path: "/tmp/rufin-external-cover.jpg".to_string(),
    };
    store
        .save_cover_cache_entry(&entry)
        .expect("save external cover");
    store
        .save_external_image_lookup_miss(
            &saved.source.id,
            &entry.item_id,
            &entry.image_tag,
            256,
            "external",
        )
        .expect("save external miss");

    let pruned = store
        .prune_external_images(&saved.source.id, &[], true)
        .expect("prune external");

    assert_eq!(pruned, vec![entry.clone()]);
    assert!(
        store
            .load_cover_cache_entry(&saved.source.id, &entry.item_id, &entry.image_tag, 256)
            .expect("external cover")
            .is_none()
    );
    assert!(
        !store
            .load_external_image_lookup_miss(
                &saved.source.id,
                &entry.item_id,
                &entry.image_tag,
                256
            )
            .expect("external miss")
    );
}

#[test]
fn sync_keep_misses() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let live = ImageRef::new(
        "external:album:artist:album",
        Some("external-tag".to_string()),
    );
    let live_cover = CoverCacheEntry {
        source_id: saved.source.id.clone(),
        item_id: live.item_id.clone(),
        image_tag: live.tag.clone().expect("live tag"),
        size: 256,
        path: "/tmp/rufin-external-live.jpg".to_string(),
    };
    let stale_cover = CoverCacheEntry {
        source_id: saved.source.id.clone(),
        item_id: "external:album:old:album".to_string(),
        image_tag: "external-old".to_string(),
        size: 256,
        path: "/tmp/rufin-external-stale.jpg".to_string(),
    };
    for entry in [&live_cover, &stale_cover] {
        store
            .save_cover_cache_entry(entry)
            .expect("save external cover");
    }
    for (item_id, image_tag, reason) in [
        (
            live_cover.item_id.as_str(),
            live_cover.image_tag.as_str(),
            "live",
        ),
        (
            stale_cover.item_id.as_str(),
            stale_cover.image_tag.as_str(),
            "old",
        ),
        ("external:album:recent:album", "external-recent", "recent"),
    ] {
        store
            .save_external_image_lookup_miss(&saved.source.id, item_id, image_tag, 256, reason)
            .expect("save external miss");
    }
    store
        .connection
        .execute(
            "
            UPDATE external_image_lookup_misses
            SET updated_at = datetime('now', '-31 days')
            WHERE source_id = ?1 AND item_id = ?2 AND image_tag = ?3
            ",
            rusqlite::params![
                saved.source.id.as_str(),
                stale_cover.item_id.as_str(),
                stale_cover.image_tag.as_str()
            ],
        )
        .expect("age stale miss");

    let pruned = store
        .prune_external_images(&saved.source.id, std::slice::from_ref(&live), false)
        .expect("prune external");

    assert_eq!(pruned, vec![stale_cover.clone()]);
    assert!(
        store
            .load_cover_cache_entry(
                &saved.source.id,
                &live_cover.item_id,
                &live_cover.image_tag,
                256,
            )
            .expect("live cover")
            .is_some()
    );
    assert!(
        store
            .load_cover_cache_entry(
                &saved.source.id,
                &stale_cover.item_id,
                &stale_cover.image_tag,
                256,
            )
            .expect("stale cover")
            .is_none()
    );
    assert!(
        store
            .load_external_image_lookup_miss(
                &saved.source.id,
                &live_cover.item_id,
                &live_cover.image_tag,
                256,
            )
            .expect("live miss")
    );
    assert!(
        !store
            .load_external_image_lookup_miss(
                &saved.source.id,
                &stale_cover.item_id,
                &stale_cover.image_tag,
                256,
            )
            .expect("old stale miss")
    );
    assert!(
        store
            .load_external_image_lookup_miss(
                &saved.source.id,
                "external:album:recent:album",
                "external-recent",
                256,
            )
            .expect("recent stale miss")
    );
}

#[test]
fn sync_keep_cache() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let mut album = album(1);
    album.image_ref = Some(ImageRef::new("album-one", None));
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .save_cover_cache_entry(&CoverCacheEntry {
            source_id: saved.source.id.clone(),
            item_id: "album-one".to_string(),
            image_tag: "untagged".to_string(),
            size: 256,
            path: "/tmp/rufin-untagged-cover.jpg".to_string(),
        })
        .expect("save untagged cover");

    store
        .complete_sync(&saved.source.id, generation)
        .expect("complete sync");

    assert!(
        store
            .load_cover_cache_entry(&saved.source.id, "album-one", "untagged", 256)
            .expect("untagged cover")
            .is_some()
    );
}
#[test]
fn sync_trip_index() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    store
        .save_external_image_lookup_miss(
            &saved.source.id,
            "external:album:artist:album",
            "external-v1-tag",
            256,
            "not found",
        )
        .expect("save lookup miss");
    assert!(
        store
            .load_external_image_lookup_miss(
                &saved.source.id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
            )
            .expect("load lookup miss")
    );
    store
        .delete_external_image_lookup_miss(
            &saved.source.id,
            "external:album:artist:album",
            "external-v1-tag",
            256,
        )
        .expect("delete lookup miss");
    assert!(
        !store
            .load_external_image_lookup_miss(
                &saved.source.id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
            )
            .expect("load deleted lookup miss")
    );
}
#[test]
fn sync_external_server() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    let other = saved_source_with_id("other-server");
    store.save_source(&saved).expect("save server");
    store.save_source(&other).expect("save other server");
    for source_id in [&saved.source.id, &other.source.id] {
        store
            .save_external_image_lookup_miss(
                source_id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
                "not found",
            )
            .expect("save lookup miss");
    }

    store
        .clear_external_image_lookup_misses(&saved.source.id)
        .expect("clear lookup misses");

    assert!(
        !store
            .load_external_image_lookup_miss(
                &saved.source.id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
            )
            .expect("load cleared lookup miss")
    );
    assert!(
        store
            .load_external_image_lookup_miss(
                &other.source.id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
            )
            .expect("load other lookup miss")
    );
}
#[test]
fn sync_clear_miss() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let entry = cover_entry(&saved.source.id);
    store
        .save_external_image_lookup_miss(
            &saved.source.id,
            &entry.item_id,
            &entry.image_tag,
            entry.size,
            "not found",
        )
        .expect("save lookup miss");
    store
        .save_cover_cache_entry(&entry)
        .expect("save cover cache");
    assert!(
        !store
            .load_external_image_lookup_miss(
                &saved.source.id,
                &entry.item_id,
                &entry.image_tag,
                entry.size,
            )
            .expect("load cleared lookup miss")
    );
}
#[test]
fn sync_remove_cover() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    let mut queue = QueueEngine::new(saved.source.id.clone());
    queue.append(&track(1, &album(1)));
    store.save_source(&saved).expect("save server");
    store
        .set_active_source(&saved.source.id)
        .expect("set active");
    store
        .save_queue_snapshot(&queue.snapshot())
        .expect("save queue");
    seed_cached_library(&store, &saved.source.id);
    store
        .save_cover_cache_entry(&cover_entry(&saved.source.id))
        .expect("save cover cache");
    store
        .save_external_image_lookup_miss(
            &saved.source.id,
            "external:album:artist:album",
            "external-v1-tag",
            256,
            "not found",
        )
        .expect("save lookup miss");
    store
        .clear_library_cache(&saved.source.id)
        .expect("clear cache");
    assert_eq!(store.active_source().expect("active server"), Some(saved));
    assert_eq!(
        store
            .load_queue_snapshot(&queue.snapshot().source_id)
            .expect("queue"),
        Some(queue.snapshot())
    );
    assert_eq!(
        store
            .load_albums(&queue.snapshot().source_id, 0, 10)
            .expect("albums")
            .total,
        0
    );
    assert!(
        store
            .search_library(&queue.snapshot().source_id, "Album", 10)
            .expect("search")
            .albums
            .is_empty()
    );
    assert_eq!(
        store
            .load_cover_cache_entry(&queue.snapshot().source_id, "album-one", "tag-one", 256)
            .expect("cover cache"),
        None
    );
    assert!(
        !store
            .load_external_image_lookup_miss(
                &queue.snapshot().source_id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
            )
            .expect("lookup miss")
    );
    let sync_state = store
        .sync_state(&queue.snapshot().source_id)
        .expect("sync state");
    assert_eq!(sync_state.generation, 0);
    assert_eq!(sync_state.status, "idle");
    assert_eq!(sync_state.last_error, None);
}
#[test]
fn sync_remove_state() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    let mut queue = QueueEngine::new(saved.source.id.clone());
    queue.append(&track(1, &album(1)));
    store.save_source(&saved).expect("save server");
    store
        .set_active_source(&saved.source.id)
        .expect("set active");
    store
        .save_queue_snapshot(&queue.snapshot())
        .expect("save queue");
    seed_cached_library(&store, &saved.source.id);
    store
        .forget_source(&saved.source.id)
        .expect("forget server");
    assert_eq!(store.active_source().expect("active server"), None);
    assert!(store.list_sources().expect("sources").is_empty());
    assert_eq!(
        store
            .load_queue_snapshot(&saved.source.id)
            .expect("queue snapshot"),
        None
    );
    assert_eq!(
        store
            .load_tracks(&saved.source.id, 0, 10)
            .expect("tracks")
            .total,
        0
    );
    assert!(
        store.sync_state(&saved.source.id).is_err(),
        "forgotten server should not have sync state"
    );
}
#[test]
fn sync_cache_safe() {
    let source_id = SourceId::new("server:one");
    assert_eq!(
        image_cache_key(&source_id, "album/one", "tag:two", 256),
        "server_one/album_one/tag_two/256"
    );
    assert_eq!(
        lyrics_cache_key(&source_id, "track/one"),
        "server_one/track_one"
    );
}
#[test]
fn sync_cache_id() {
    let source_id = SourceId::new("local:server:test");
    let long_item_id = format!("local:cover:embedded:{}", "nested-folder-".repeat(40));
    let key = image_cache_key(&source_id, &long_item_id, "untagged", 256);
    let parts = key.split('/').collect::<Vec<_>>();
    assert_eq!(parts.len(), 4);
    assert!(parts.iter().all(|part| part.len() <= 180));
    assert!(parts[1].starts_with("local_cover_embedded_nested-folder-"));
    assert!(parts[1].len() < long_item_id.len());
    assert_eq!(
        image_cache_key(&source_id, &long_item_id, "untagged", 256),
        key
    );
    let other_item_id = format!("local:cover:embedded:{}", "nested-folder-".repeat(39));
    assert_ne!(
        image_cache_key(&source_id, &other_item_id, "untagged", 256),
        key
    );
}
