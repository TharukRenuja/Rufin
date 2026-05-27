    #[test]
    fn artist_lists_hide_appears_on_only_artists() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let primary_artist = Artist {
            id: ArtistId::fake(1),
            name: "Primary Artist".to_string(),
            album_count: 1,
            track_count: 1,
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
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
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert track");
        store
            .upsert_artists(
                &saved.server.id,
                &[primary_artist.clone(), linked_artist.clone()],
                false,
                generation,
            )
            .expect("upsert artists");
        store
            .refresh_library_counts(&saved.server.id)
            .expect("refresh counts");
        let artist_page = store
            .load_artists(&saved.server.id, false, 0, 10)
            .expect("load artists");
        assert_eq!(artist_page.items, vec![primary_artist]);
        let linked_search = store
            .load_artists_matching(&saved.server.id, false, "Featured Artist", 0, 10)
            .expect("search artists");
        assert!(linked_search.items.is_empty());
        let detail = store
            .load_artist_detail(&saved.server.id, &linked_artist.id)
            .expect("load artist detail")
            .expect("artist detail");
        assert_eq!(detail.appears_on.len(), 1);
        assert_eq!(detail.appears_on[0].id, album.id);
    }
    #[test]
    fn cached_pages_rehydrate_metadata_credits_and_genres() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
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
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(
                &saved.server.id,
                &[track_one.clone(), track_two.clone()],
                generation,
            )
            .expect("upsert tracks");
        let mut loaded_albums = store
            .load_albums(&saved.server.id, 0, 10)
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
            .load_tracks(&saved.server.id, 0, 10)
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
            .load_tracks_for_albums(&saved.server.id, std::slice::from_ref(&album.id))
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
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(7);
        let track = track(4, &album);
        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
            .expect("upsert track");
        let results = store
            .search_library(&saved.server.id, "Album 7", 10)
            .expect("search");
        assert_eq!(results.albums, vec![album]);
        assert_eq!(results.tracks, vec![track]);
    }
    #[test]
    fn sync_generation_prunes_missing_items_after_success() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let album_one = album(1);
        let album_two = album(2);
        let first_generation = store.begin_sync(&saved.server.id).expect("begin first");
        store
            .upsert_albums(
                &saved.server.id,
                &[album_one.clone(), album_two],
                first_generation,
            )
            .expect("upsert first");
        store
            .complete_sync(&saved.server.id, first_generation)
            .expect("complete first");
        let second_generation = store.begin_sync(&saved.server.id).expect("begin second");
        store
            .upsert_albums(&saved.server.id, &[album_one], second_generation)
            .expect("upsert second");
        store
            .complete_sync(&saved.server.id, second_generation)
            .expect("complete second");
        let albums = store
            .load_albums(&saved.server.id, 0, 10)
            .expect("load albums");
        assert_eq!(albums.total, 1);
    }
    #[test]
    fn home_sections_preserve_synced_album_and_track_order() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album_one = album(1);
        let album_two = album(2);
        let track_one = track(1, &album_one);
        let track_two = track(2, &album_two);
        store
            .upsert_albums(
                &saved.server.id,
                &[album_one.clone(), album_two.clone()],
                generation,
            )
            .expect("upsert albums");
        store
            .upsert_tracks(
                &saved.server.id,
                &[track_one.clone(), track_two.clone()],
                generation,
            )
            .expect("upsert tracks");
        store
            .upsert_home_sections(
                &saved.server.id,
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
            .load_home_sections(&saved.server.id)
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
    fn home_sections_without_cached_rows_are_empty() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let album = album(1);
        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
            .expect("upsert album");
        let sections = store
            .load_home_sections(&saved.server.id)
            .expect("load home sections");
        assert!(sections.is_empty());
    }
    #[test]
    fn home_section_prefetch_does_not_replace_visible_section() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let generation = store.begin_sync(&saved.server.id).expect("begin sync");
        let visible_album = album(1);
        let prefetched_album = album(2);
        store
            .upsert_albums(
                &saved.server.id,
                &[visible_album.clone(), prefetched_album.clone()],
                generation,
            )
            .expect("upsert albums");
        store
            .upsert_home_section(
                &saved.server.id,
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
                &saved.server.id,
                &HomeSection {
                    kind: HomeSectionKind::Explore,
                    albums: vec![prefetched_album.clone()],
                    tracks: Vec::new(),
                },
                generation,
            )
            .expect("upsert prefetched Explore");
        let visible = store
            .load_home_sections(&saved.server.id)
            .expect("load visible sections");
        let prefetched = store
            .load_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
            .expect("load prefetched Explore")
            .expect("prefetched Explore");
        assert_eq!(visible[0].albums[0].id, visible_album.id);
        assert_eq!(prefetched.albums[0].id, prefetched_album.id);
        store
            .clear_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
            .expect("clear prefetched Explore");
        assert!(
            store
                .load_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
                .expect("load cleared prefetched Explore")
                .is_none()
        );
    }
    #[test]
    fn cover_cache_index_round_trips() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let entry = cover_entry(&saved.server.id);
        store
            .save_cover_cache_entry(&entry)
            .expect("save cover cache");
        assert_eq!(
            store
                .load_cover_cache_entry(&saved.server.id, "album-one", "tag-one", 256)
                .expect("load cover cache"),
            Some(entry)
        );
    }
    #[test]
    fn cover_cache_index_can_delete_missing_entries() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let entry = cover_entry(&saved.server.id);
        store
            .save_cover_cache_entry(&entry)
            .expect("save cover cache");
        store
            .delete_cover_cache_entry(&saved.server.id, "album-one", "tag-one", 256)
            .expect("delete cover cache");
        assert_eq!(
            store
                .load_cover_cache_entry(&saved.server.id, "album-one", "tag-one", 256)
                .expect("load cover cache"),
            None
        );
    }
    #[test]
    fn external_image_lookup_miss_index_round_trips() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        store
            .save_external_image_lookup_miss(
                &saved.server.id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
                "not found",
            )
            .expect("save lookup miss");
        assert!(
            store
                .load_external_image_lookup_miss(
                    &saved.server.id,
                    "external:album:artist:album",
                    "external-v1-tag",
                    256,
                )
                .expect("load lookup miss")
        );
        store
            .delete_external_image_lookup_miss(
                &saved.server.id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
            )
            .expect("delete lookup miss");
        assert!(
            !store
                .load_external_image_lookup_miss(
                    &saved.server.id,
                    "external:album:artist:album",
                    "external-v1-tag",
                    256,
                )
                .expect("load deleted lookup miss")
        );
    }
    #[test]
    fn external_image_lookup_misses_can_be_cleared_for_server() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        let other = saved_server_with_id("other-server");
        store.save_server(&saved).expect("save server");
        store.save_server(&other).expect("save other server");
        for server_id in [&saved.server.id, &other.server.id] {
            store
                .save_external_image_lookup_miss(
                    server_id,
                    "external:album:artist:album",
                    "external-v1-tag",
                    256,
                    "not found",
                )
                .expect("save lookup miss");
        }

        store
            .clear_external_image_lookup_misses(&saved.server.id)
            .expect("clear lookup misses");

        assert!(
            !store
                .load_external_image_lookup_miss(
                    &saved.server.id,
                    "external:album:artist:album",
                    "external-v1-tag",
                    256,
                )
                .expect("load cleared lookup miss")
        );
        assert!(
            store
                .load_external_image_lookup_miss(
                    &other.server.id,
                    "external:album:artist:album",
                    "external-v1-tag",
                    256,
                )
                .expect("load other lookup miss")
        );
    }
    #[test]
    fn cover_cache_success_clears_external_image_lookup_miss() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let entry = cover_entry(&saved.server.id);
        store
            .save_external_image_lookup_miss(
                &saved.server.id,
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
                    &saved.server.id,
                    &entry.item_id,
                    &entry.image_tag,
                    entry.size,
                )
                .expect("load cleared lookup miss")
        );
    }
    #[test]
    fn clear_library_cache_removes_library_search_and_cover_rows_only() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        let mut queue = QueueEngine::new(saved.server.id.clone());
        queue.append(&track(1, &album(1)));
        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active");
        store
            .save_queue_snapshot(&queue.snapshot())
            .expect("save queue");
        seed_cached_library(&store, &saved.server.id);
        store
            .save_cover_cache_entry(&cover_entry(&saved.server.id))
            .expect("save cover cache");
        store
            .save_external_image_lookup_miss(
                &saved.server.id,
                "external:album:artist:album",
                "external-v1-tag",
                256,
                "not found",
            )
            .expect("save lookup miss");
        store
            .clear_library_cache(&saved.server.id)
            .expect("clear cache");
        assert_eq!(store.active_server().expect("active server"), Some(saved));
        assert_eq!(
            store
                .load_queue_snapshot(&queue.snapshot().server_id)
                .expect("queue"),
            Some(queue.snapshot())
        );
        assert_eq!(
            store
                .load_albums(&queue.snapshot().server_id, 0, 10)
                .expect("albums")
                .total,
            0
        );
        assert!(
            store
                .search_library(&queue.snapshot().server_id, "Album", 10)
                .expect("search")
                .albums
                .is_empty()
        );
        assert_eq!(
            store
                .load_cover_cache_entry(&queue.snapshot().server_id, "album-one", "tag-one", 256)
                .expect("cover cache"),
            None
        );
        assert!(
            !store
                .load_external_image_lookup_miss(
                    &queue.snapshot().server_id,
                    "external:album:artist:album",
                    "external-v1-tag",
                    256,
                )
                .expect("lookup miss")
        );
        let sync_state = store
            .sync_state(&queue.snapshot().server_id)
            .expect("sync state");
        assert_eq!(sync_state.generation, 0);
        assert_eq!(sync_state.status, "idle");
        assert_eq!(sync_state.last_error, None);
    }
    #[test]
    fn forget_server_removes_server_local_state() {
        let store = Store::open_memory().expect("open store");
        let saved = saved_server();
        let mut queue = QueueEngine::new(saved.server.id.clone());
        queue.append(&track(1, &album(1)));
        store.save_server(&saved).expect("save server");
        store
            .set_active_server(&saved.server.id)
            .expect("set active");
        store
            .save_queue_snapshot(&queue.snapshot())
            .expect("save queue");
        seed_cached_library(&store, &saved.server.id);
        store
            .forget_server(&saved.server.id)
            .expect("forget server");
        assert_eq!(store.active_server().expect("active server"), None);
        assert!(store.list_servers().expect("servers").is_empty());
        assert_eq!(
            store
                .load_queue_snapshot(&saved.server.id)
                .expect("queue snapshot"),
            None
        );
        assert_eq!(
            store
                .load_tracks(&saved.server.id, 0, 10)
                .expect("tracks")
                .total,
            0
        );
        assert!(
            store.sync_state(&saved.server.id).is_err(),
            "forgotten server should not have sync state"
        );
    }
    #[test]
    fn cache_keys_are_stable_and_path_safe() {
        let server_id = ServerId::new("server:one");
        assert_eq!(
            image_cache_key(&server_id, "album/one", "tag:two", 256),
            "server_one/album_one/tag_two/256"
        );
        assert_eq!(
            lyrics_cache_key(&server_id, "track/one"),
            "server_one/track_one"
        );
    }
    #[test]
    fn cache_key_parts_are_bounded_for_long_local_cover_ids() {
        let server_id = ServerId::new("local:server:test");
        let long_item_id = format!("local:cover:embedded:{}", "nested-folder-".repeat(40));
        let key = image_cache_key(&server_id, &long_item_id, "untagged", 256);
        let parts = key.split('/').collect::<Vec<_>>();
        assert_eq!(parts.len(), 4);
        assert!(parts.iter().all(|part| part.len() <= 180));
        assert!(parts[1].starts_with("local_cover_embedded_nested-folder-"));
        assert!(parts[1].len() < long_item_id.len());
        assert_eq!(
            image_cache_key(&server_id, &long_item_id, "untagged", 256),
            key
        );
        let other_item_id = format!("local:cover:embedded:{}", "nested-folder-".repeat(39));
        assert_ne!(
            image_cache_key(&server_id, &other_item_id, "untagged", 256),
            key
        );
    }
    fn saved_server() -> SavedServer {
        saved_server_with_id("jellyfin:server:test")
    }
    fn saved_server_with_id(server_id: &str) -> SavedServer {
        SavedServer {
            server: ServerIdentity {
                id: ServerId::new(server_id),
                provider: "jellyfin".to_string(),
                name: "Test Server".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        }
    }
    fn album(number: u32) -> Album {
        Album {
            id: AlbumId::fake(number),
            title: format!("Album {number}"),
            artist: "Artist".to_string(),
            artist_id: Some(ArtistId::fake(1)),
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 2,
            duration_seconds: 360,
            favorite: number == 2,
            color_seed: number,
            image_ref: None,
            genres: Vec::new(),
        }
    }
    fn album_with_image(number: u32) -> Album {
        Album {
            image_ref: Some(image_ref(
                format!("album-{number}"),
                format!("album-tag-{number}"),
            )),
            genres: vec!["Dream Pop".to_string()],
            ..album(number)
        }
    }
    fn credit(id: ArtistId, name: &str) -> ArtistCredit {
        ArtistCredit {
            id,
            name: name.to_string(),
        }
    }
    fn artist(number: u32, image_ref: Option<ImageRef>) -> Artist {
        Artist {
            id: ArtistId::fake(number),
            name: format!("Artist {number}"),
            album_count: 1,
            track_count: 2,
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
            image_ref,
        }
    }
    fn genre(number: u32, image_ref: Option<ImageRef>) -> Genre {
        Genre {
            id: GenreId::fake(number),
            name: format!("Genre {number}"),
            album_count: 1,
            track_count: 2,
            image_ref,
        }
    }
    fn playlist(number: u32, image_ref: Option<ImageRef>) -> Playlist {
        Playlist {
            id: PlaylistId::fake(number),
            name: format!("Playlist {number}"),
            track_count: 2,
            duration_seconds: 360,
            image_ref,
        }
    }
    fn image_ref(item_id: impl Into<String>, tag: impl Into<String>) -> ImageRef {
        ImageRef::new(item_id, Some(tag.into()))
    }
    fn index_exists(store: &Store, table: &str, index: &str) -> bool {
        let mut statement = store
            .connection
            .prepare(&format!("PRAGMA index_list({table})"))
            .expect("index list");
        let indexes = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query indexes");
        indexes
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect indexes")
            .iter()
            .any(|name| name == index)
    }
    fn seed_cached_library(store: &Store, server_id: &ServerId) {
        let generation = store.begin_sync(server_id).expect("begin sync");
        let album = album(1);
        let track = track(1, &album);
        store
            .upsert_albums(server_id, std::slice::from_ref(&album), generation)
            .expect("upsert albums");
        store
            .upsert_tracks(server_id, std::slice::from_ref(&track), generation)
            .expect("upsert tracks");
        store
            .complete_sync(server_id, generation)
            .expect("complete sync");
    }
    fn cover_entry(server_id: &ServerId) -> CoverCacheEntry {
        CoverCacheEntry {
            server_id: server_id.clone(),
            item_id: "album-one".to_string(),
            image_tag: "tag-one".to_string(),
            size: 256,
            path: "/tmp/rufin-cover.jpg".to_string(),
        }
    }
    fn track(number: u32, album: &Album) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id: album.id.clone(),
            title: format!("Track {number}"),
            artist: album.artist.clone(),
            artist_id: album.artist_id.clone(),
            artist_credits: album
                .artist_id
                .clone()
                .map(|artist_id| vec![credit(artist_id, &album.artist)])
                .unwrap_or_default(),
            album_artist_credits: Vec::new(),
            album: album.title.clone(),
            year: album.year,
            release_date: album.release_date.clone(),
            date_added: album.date_added.clone(),
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: number == 1,
            disc_number: 1,
            track_number: number as u16,
            image_ref: album.image_ref.clone(),
            genres: album.genres.clone(),
            local_path: None,
            source_format: None,
        }
    }
