use super::test_support::*;
use rufin_core::{PlaySourceDescriptor, PlaySourceKey, PlaylistEntrySortDescriptor, SourceOrder};

#[test]
fn playlist_entries_allow_duplicate_tracks_and_keep_entry_ids() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    let playlist = playlist(1, None);
    let entries = vec![
        PlaylistEntry {
            entry_id: "entry-one".to_string(),
            track: track.clone(),
        },
        PlaylistEntry {
            entry_id: "entry-two".to_string(),
            track: track.clone(),
        },
    ];
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert tracks");
    store
        .upsert_playlists(
            &saved.server.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_entries(&saved.server.id, &playlist.id, &entries, generation)
        .expect("upsert playlist entries");
    let detail = store
        .load_playlist_detail(&saved.server.id, &playlist.id)
        .expect("load playlist detail")
        .expect("playlist detail");
    assert_eq!(detail.entries, entries);
    assert_eq!(detail.tracks, vec![track.clone(), track]);
}

#[test]
fn playlist_source_window_preserves_duplicate_occurrences_in_display_order() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let mut repeated_track = track(1, &album);
    repeated_track.title = "Echo".to_string();
    let mut other_track = track(2, &album);
    other_track.title = "Outside".to_string();
    let mut playlist = playlist(1, None);
    playlist.track_count = 3;
    playlist.duration_seconds = 540;
    let entries = vec![
        PlaylistEntry {
            entry_id: "entry-one".to_string(),
            track: repeated_track.clone(),
        },
        PlaylistEntry {
            entry_id: "entry-two".to_string(),
            track: repeated_track.clone(),
        },
        PlaylistEntry {
            entry_id: "entry-three".to_string(),
            track: other_track.clone(),
        },
    ];
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.server.id,
            &[repeated_track.clone(), other_track.clone()],
            generation,
        )
        .expect("upsert tracks");
    store
        .upsert_playlists(
            &saved.server.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_entries(&saved.server.id, &playlist.id, &entries, generation)
        .expect("upsert playlist entries");

    let source = PlaySourceKey {
        descriptor: PlaySourceDescriptor::Playlist {
            playlist_id: playlist.id.clone(),
        },
        order: SourceOrder::PlaylistDisplayed {
            query: Some("echo".to_string()),
            sort: PlaylistEntrySortDescriptor::Title,
            descending: true,
        },
    };

    assert_eq!(
        store
            .count_tracks_for_source(&saved.server.id, &source)
            .expect("count source tracks"),
        2
    );
    assert_eq!(
        store
            .track_rank_for_source(
                &saved.server.id,
                &source,
                &repeated_track.id,
                Some("entry-two")
            )
            .expect("rank second duplicate"),
        Some(0)
    );
    assert_eq!(
        store
            .track_rank_for_source(
                &saved.server.id,
                &source,
                &repeated_track.id,
                Some("entry-one")
            )
            .expect("rank first duplicate"),
        Some(1)
    );
    assert_eq!(
        store
            .track_rank_for_source(&saved.server.id, &source, &other_track.id, None)
            .expect("rank filtered track"),
        None
    );

    let window = store
        .tracks_window_for_source(&saved.server.id, &source, 0, 0, 1)
        .expect("source window");
    assert_eq!(window.start_rank, 0);
    assert_eq!(window.total_source_items, 2);
    assert_eq!(
        window
            .items
            .iter()
            .map(|item| item.source_item_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("entry-two"), Some("entry-one")]
    );
    assert_eq!(
        window
            .items
            .iter()
            .map(|item| item.source_index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );

    let source = PlaySourceKey {
        descriptor: PlaySourceDescriptor::Playlist {
            playlist_id: playlist.id,
        },
        order: SourceOrder::PlaylistDisplayed {
            query: None,
            sort: PlaylistEntrySortDescriptor::Position,
            descending: false,
        },
    };
    let anchor_rank = store
        .track_rank_for_source(
            &saved.server.id,
            &source,
            &repeated_track.id,
            Some("entry-two"),
        )
        .expect("rank source occurrence")
        .expect("source occurrence rank");
    let window = store
        .tracks_window_for_source(&saved.server.id, &source, anchor_rank, 1, 1)
        .expect("source window");
    assert_eq!(window.start_rank, 0);
    assert_eq!(window.total_source_items, 3);
    assert_eq!(
        window
            .items
            .iter()
            .map(|item| item.source_item_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("entry-one"), Some("entry-two"), Some("entry-three")]
    );
}
#[test]
fn lyrics_cache_round_trips_by_server_and_track() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        lines: vec![LyricLine {
            start_millis: Some(12_000),
            text: "hello".to_string(),
        }],
    };
    store
        .save_lyrics(&saved.server.id, &lyrics)
        .expect("save lyrics");
    assert_eq!(
        store
            .load_lyrics(&saved.server.id, &track.id)
            .expect("load lyrics"),
        Some(lyrics)
    );
    assert_eq!(
        store
            .load_lyrics(&ServerId::fake(2), &track.id)
            .expect("load missing lyrics"),
        None
    );
}
#[test]
fn delete_remote_lyrics_preserves_server_lyrics() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let server_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Server,
        lines: vec![LyricLine {
            start_millis: None,
            text: "server line".to_string(),
        }],
    };
    store
        .save_lyrics(&saved.server.id, &server_lyrics)
        .expect("save lyrics");

    assert!(
        !store
            .delete_remote_lyrics(&saved.server.id, &track.id)
            .expect("delete remote lyrics")
    );
    assert_eq!(
        store
            .load_lyrics(&saved.server.id, &track.id)
            .expect("load lyrics"),
        Some(server_lyrics)
    );

    let remote_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        lines: vec![LyricLine {
            start_millis: None,
            text: "remote line".to_string(),
        }],
    };
    store
        .save_lyrics(&saved.server.id, &remote_lyrics)
        .expect("save remote lyrics");
    assert!(
        store
            .delete_remote_lyrics(&saved.server.id, &track.id)
            .expect("delete remote lyrics")
    );
    assert_eq!(
        store
            .load_lyrics(&saved.server.id, &track.id)
            .expect("load lyrics"),
        None
    );
}
#[test]
fn favorite_flag_updates_refresh_cached_models_and_favorite_tracks() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(1);
    album.favorite = false;
    let mut track = track(1, &album);
    track.favorite = false;
    let artist = artist(1, None);
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .set_album_favorite(&saved.server.id, &album.id, true)
        .expect("favorite album");
    store
        .set_track_favorite(&saved.server.id, &track.id, true)
        .expect("favorite track");
    store
        .set_artist_favorite(&saved.server.id, &artist.id, true)
        .expect("favorite artist");
    assert!(
        store
            .load_albums(&saved.server.id, 0, 1)
            .expect("load albums")
            .items[0]
            .favorite
    );
    assert!(
        store
            .load_tracks(&saved.server.id, 0, 1)
            .expect("load tracks")
            .items[0]
            .favorite
    );
    assert!(
        store
            .load_artists(&saved.server.id, false, 0, 1)
            .expect("load artists")
            .items[0]
            .favorite
    );
    assert_eq!(
        store
            .load_favorite_tracks(&saved.server.id)
            .expect("favorite tracks")
            .len(),
        1
    );
}
#[test]
fn genre_detail_returns_linked_albums_and_tracks() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let track = track(1, &album);
    let genre = Genre {
        id: GenreId::new("jellyfin:genre:dream-pop"),
        name: "Dream Pop".to_string(),
        album_count: 0,
        track_count: 0,
        image_refs: Vec::new(),
        image_ref: Some(image_ref("genre-dream-pop", "tag")),
    };
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(&saved.server.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    let detail = store
        .load_genre_detail(&saved.server.id, &genre.id)
        .expect("load genre detail")
        .expect("genre detail");
    assert_eq!(detail.genre.name, genre.name);
    assert_eq!(detail.genre.album_count, 1);
    assert_eq!(detail.genre.track_count, 1);
    assert_eq!(detail.albums, vec![album]);
    assert_eq!(detail.tracks, vec![track]);
}
#[test]
fn genre_list_only_returns_music_linked_genres() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let mut movie_genre = genre(2, None);
    movie_genre.name = "Science Fiction".to_string();
    let mut music_genre = genre(3, None);
    music_genre.name = "Dream Pop".to_string();
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_genres(
            &saved.server.id,
            &[movie_genre, music_genre.clone()],
            generation,
        )
        .expect("upsert genres");
    let genres = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load genres");
    assert_eq!(genres.total, 1);
    assert_eq!(genres.items[0].id, music_genre.id);
    assert_eq!(genres.items[0].name, music_genre.name);
    assert_eq!(genres.items[0].album_count, 1);
    assert_eq!(genres.items[0].track_count, 0);
}
#[test]
fn genre_counts_use_linked_music_items_instead_of_provider_counts() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Anime".to_string()];
    let track = track(1, &album);
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:anime"),
        name: "Anime".to_string(),
        album_count: 167,
        track_count: 1_561,
        image_refs: Vec::new(),
        image_ref: None,
    };
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.server.id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");
    let genres = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.server.id, &provider_genre.id)
        .expect("load genre detail")
        .expect("genre detail");
    assert_eq!(genres.items[0].album_count, 1);
    assert_eq!(genres.items[0].track_count, 1);
    assert_eq!(detail.genre.album_count, 1);
    assert_eq!(detail.genre.track_count, 1);
}

#[test]
fn genre_counts_derive_albums_from_track_only_links() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.genres = vec!["Instrumental".to_string()];
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:instrumental"),
        name: "Instrumental".to_string(),
        album_count: 12,
        track_count: 99,
        image_refs: Vec::new(),
        image_ref: None,
    };
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.server.id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");

    let genres = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.server.id, &provider_genre.id)
        .expect("load genre detail")
        .expect("genre detail");

    assert_eq!(genres.items[0].album_count, 1);
    assert_eq!(genres.items[0].track_count, 1);
    assert_eq!(detail.genre.album_count, 1);
    assert_eq!(detail.genre.track_count, 1);
    assert_eq!(detail.albums, vec![album]);
    assert_eq!(detail.tracks, vec![track]);
}

#[test]
fn genre_counts_exclude_missing_album_rows_for_track_only_links() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.genres = vec!["Instrumental".to_string()];
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:instrumental"),
        name: "Instrumental".to_string(),
        album_count: 12,
        track_count: 99,
        image_refs: Vec::new(),
        image_ref: None,
    };
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.server.id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");

    let genres = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.server.id, &provider_genre.id)
        .expect("load genre detail")
        .expect("genre detail");

    assert_eq!(genres.items[0].album_count, 0);
    assert_eq!(genres.items[0].track_count, 1);
    assert_eq!(detail.genre.album_count, 0);
    assert_eq!(detail.genre.track_count, 1);
    assert!(detail.albums.is_empty());
    assert_eq!(detail.tracks, vec![track]);
}

#[test]
fn refresh_library_counts_repairs_missing_linked_genre_rows() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let track = track(1, &album);
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    let genres = store
        .load_genres(&saved.server.id, 0, 20)
        .expect("load genres");
    assert_eq!(genres.total, 1);
    assert_eq!(genres.items[0].name, "Dream Pop");
    assert_eq!(genres.items[0].album_count, 1);
    assert_eq!(genres.items[0].track_count, 1);
}
#[test]
fn album_detail_falls_back_to_tracks_when_album_row_is_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(9);
    let tracks = vec![track(1, &album), track(2, &album)];
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    let detail = store
        .load_album_detail(&saved.server.id, &album.id)
        .expect("load album detail")
        .expect("album detail");
    assert_eq!(detail.0.id, album.id);
    assert_eq!(detail.0.title, album.title);
    assert_eq!(detail.0.artist, album.artist);
    assert_eq!(detail.0.track_count, 2);
    assert_eq!(detail.1, tracks);
}
#[test]
fn refresh_library_counts_uses_cached_tracks() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(1);
    album.track_count = 0;
    album.duration_seconds = 0;
    let tracks = vec![track(1, &album), track(2, &album)];
    let artist = Artist {
        id: ArtistId::fake(1),
        name: "Artist".to_string(),
        album_count: 0,
        track_count: 0,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    };
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    let album = store
        .load_albums(&saved.server.id, 0, 1)
        .expect("load albums")
        .items
        .remove(0);
    assert_eq!(album.track_count, 2);
    assert_eq!(
        album.duration_seconds,
        tracks
            .iter()
            .map(|track| track.duration_seconds)
            .sum::<u32>()
    );
    let artist = store
        .load_artists(&saved.server.id, false, 0, 1)
        .expect("load artists")
        .items
        .remove(0);
    let album_artist = store
        .load_artists(&saved.server.id, true, 0, 1)
        .expect("load album artists")
        .items
        .remove(0);
    assert_eq!(artist.album_count, 1);
    assert_eq!(artist.track_count, 2);
    assert_eq!(album_artist.album_count, 1);
    assert_eq!(album_artist.track_count, 2);
}
#[test]
fn refresh_library_counts_repairs_missing_linked_artist_rows() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    let artist = store
        .load_artists(&saved.server.id, false, 0, 1)
        .expect("load artists")
        .items
        .remove(0);
    let album_artist = store
        .load_artists(&saved.server.id, true, 0, 1)
        .expect("load album artists")
        .items
        .remove(0);
    let search = store
        .search_library(&saved.server.id, "Artist", 10)
        .expect("search");
    assert_eq!(artist.name, album.artist);
    assert_eq!(artist.album_count, 1);
    assert_eq!(artist.track_count, 2);
    assert_eq!(album_artist.name, album.artist);
    assert_eq!(album_artist.album_count, 1);
    assert_eq!(album_artist.track_count, 2);
    assert_eq!(search.artists, vec![artist]);
}
#[test]
fn refresh_library_counts_preserves_provider_counts_without_relationships() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let artist = Artist {
        id: ArtistId::fake(99),
        name: "Provider Counted".to_string(),
        album_count: 3,
        track_count: 18,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    };
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    let artist = store
        .load_artists(&saved.server.id, false, 0, 1)
        .expect("load artists")
        .items
        .remove(0);
    assert_eq!(artist.album_count, 3);
    assert_eq!(artist.track_count, 18);
}
#[test]
fn artist_detail_uses_album_artist_albums_and_tracks() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let artist = Artist {
        id: album.artist_id.clone().expect("album artist id"),
        name: album.artist.clone(),
        album_count: 0,
        track_count: 0,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    };
    let mut track = track(1, &album);
    track.artist_id = None;
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    let detail = store
        .load_artist_detail(&saved.server.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist, artist);
    assert_eq!(detail.albums, vec![album]);
    assert!(detail.appears_on.is_empty());
    assert_eq!(detail.tracks, vec![track]);
}
#[test]
fn artist_detail_falls_back_to_track_links_when_artist_row_is_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    let artist_id = album.artist_id.clone().expect("artist id");
    store
        .upsert_tracks(&saved.server.id, &tracks, generation)
        .expect("upsert tracks");
    let detail = store
        .load_artist_detail(&saved.server.id, &artist_id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist.id, artist_id);
    assert_eq!(detail.artist.name, album.artist);
    assert_eq!(detail.artist.album_count, 1);
    assert_eq!(detail.artist.track_count, 2);
    assert!(detail.albums.is_empty());
    assert_eq!(
        detail.appears_on,
        vec![synthesize_album_from_tracks(&album.id, &tracks)]
    );
    assert_eq!(detail.tracks, tracks);
}
#[test]
fn artist_detail_falls_back_to_album_links_when_artist_row_is_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album = album(1);
    let artist_id = album.artist_id.clone().expect("artist id");
    let mut track = track(1, &album);
    track.artist_id = None;
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let detail = store
        .load_artist_detail(&saved.server.id, &artist_id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist.id, artist_id);
    assert_eq!(detail.artist.name, album.artist);
    assert_eq!(detail.artist.album_count, 1);
    assert_eq!(detail.artist.track_count, 1);
    assert_eq!(detail.albums, vec![album]);
    assert!(detail.appears_on.is_empty());
    assert_eq!(detail.tracks, vec![track]);
}
#[test]
fn artist_detail_groups_non_primary_track_albums_as_appears_on() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(3);
    album.artist = "Other Artist".to_string();
    album.artist_id = Some(ArtistId::fake(99));
    let artist = Artist {
        id: ArtistId::fake(1),
        name: "Artist".to_string(),
        album_count: 0,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    };
    let mut track = track(1, &album);
    track.artist = artist.name.clone();
    track.artist_id = Some(artist.id.clone());
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    let detail = store
        .load_artist_detail(&saved.server.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist, artist);
    assert!(detail.albums.is_empty());
    assert_eq!(detail.appears_on, vec![album]);
    assert_eq!(detail.tracks, vec![track]);
}
#[test]
fn artist_detail_uses_album_name_when_artist_ids_are_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(4);
    album.artist_id = None;
    let track = track(1, &album);
    let artist = Artist {
        id: ArtistId::fake(1),
        name: album.artist.clone(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    };
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    let detail = store
        .load_artist_detail(&saved.server.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.albums, vec![album]);
    assert!(detail.appears_on.is_empty());
    assert_eq!(detail.tracks, vec![track]);
}
#[test]
fn artist_detail_groups_name_matched_track_albums_as_appears_on() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(5);
    album.artist = "Other Artist".to_string();
    album.artist_id = Some(ArtistId::fake(99));
    let artist = Artist {
        id: ArtistId::fake(1),
        name: "Artist".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    };
    let mut track = track(1, &album);
    track.artist = artist.name.clone();
    track.artist_id = None;
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    let detail = store
        .load_artist_detail(&saved.server.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert!(detail.albums.is_empty());
    assert_eq!(detail.appears_on, vec![album]);
    assert_eq!(detail.tracks, vec![track]);
}
#[test]
fn artist_detail_uses_track_artist_links_as_appears_on() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let mut album = album(6);
    album.artist = "Primary Artist".to_string();
    album.artist_id = Some(ArtistId::fake(99));
    let credited_artist = ArtistId::fake(7);
    let mut track = track(1, &album);
    track.artist = "Primary Artist".to_string();
    track.artist_id = Some(ArtistId::fake(99));
    track.artist_credits = vec![credit(credited_artist.clone(), "Featured Artist")];
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    let detail = store
        .load_artist_detail(&saved.server.id, &credited_artist)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist.name, "Featured Artist");
    assert_eq!(detail.artist.album_count, 1);
    assert_eq!(detail.artist.track_count, 1);
    assert!(detail.albums.is_empty());
    assert_eq!(detail.appears_on.len(), 1);
    assert_eq!(detail.appears_on[0].id, album.id);
    assert_eq!(detail.tracks.len(), 1);
    assert_eq!(detail.tracks[0].id, track.id);
}
#[test]
fn artist_detail_uses_album_artist_links_as_primary_albums() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album_artist_id = ArtistId::fake(8);
    let mut album = album(7);
    album.artist = "Various Artists".to_string();
    album.artist_id = Some(ArtistId::fake(99));
    album.album_artist_credits = vec![credit(album_artist_id.clone(), "Linked Album Artist")];
    let mut track = track(1, &album);
    track.artist = "Different Track Artist".to_string();
    track.artist_id = Some(ArtistId::fake(10));
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    let album_artist = store
        .load_artists(&saved.server.id, true, 0, 10)
        .expect("load album artists")
        .items
        .into_iter()
        .find(|artist| artist.id == album_artist_id)
        .expect("linked album artist");
    assert_eq!(album_artist.name, "Linked Album Artist");
    assert_eq!(album_artist.album_count, 1);
    assert_eq!(album_artist.track_count, u32::from(album.track_count));
    let detail = store
        .load_artist_detail(&saved.server.id, &album_artist_id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist, album_artist);
    assert_eq!(detail.albums.len(), 1);
    assert_eq!(detail.albums[0].id, album.id);
    assert!(detail.appears_on.is_empty());
    assert_eq!(detail.tracks.len(), 1);
    assert_eq!(detail.tracks[0].id, track.id);
}
#[test]
fn artist_detail_matches_album_artist_name_when_link_is_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_server();
    store.save_server(&saved).expect("save server");
    let generation = store.begin_sync(&saved.server.id).expect("begin sync");
    let album_artist = Artist {
        id: ArtistId::fake(8),
        name: "Linked Album Artist".to_string(),
        album_count: 1,
        track_count: 0,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    };
    let mut album = album(7);
    album.artist = album_artist.name.clone();
    album.artist_id = Some(ArtistId::fake(99));
    album.album_artist_credits = Vec::new();
    let mut track = track(1, &album);
    track.artist = "Different Track Artist".to_string();
    track.artist_id = Some(ArtistId::fake(10));
    track.artist_credits = vec![credit(
        track.artist_id.clone().expect("track artist id"),
        &track.artist,
    )];
    store
        .upsert_albums(&saved.server.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.server.id,
            std::slice::from_ref(&album_artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    store
        .refresh_library_counts(&saved.server.id)
        .expect("refresh counts");
    let loaded_artist = store
        .load_artists(&saved.server.id, true, 0, 10)
        .expect("load album artists")
        .items
        .into_iter()
        .find(|artist| artist.id == album_artist.id)
        .expect("album artist");
    assert_eq!(loaded_artist.album_count, 1);
    assert_eq!(loaded_artist.track_count, u32::from(album.track_count));
    let detail = store
        .load_artist_detail(&saved.server.id, &album_artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist, loaded_artist);
    assert_eq!(detail.albums.len(), 1);
    assert_eq!(detail.albums[0].id, album.id);
    assert!(detail.appears_on.is_empty());
    assert_eq!(detail.tracks.len(), 1);
    assert_eq!(detail.tracks[0].id, track.id);
}
