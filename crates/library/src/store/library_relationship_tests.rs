use super::test_support::*;
use domain::{
    MoodId, PlaySourceDescriptor, PlaySourceKey, PlaylistEntrySortDescriptor, SourceOrder,
};

#[test]
fn relation_keep_id() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
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
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert tracks");
    store
        .upsert_playlists(
            &saved.source.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_entries(&saved.source.id, &playlist.id, &entries, generation)
        .expect("upsert playlist entries");
    let detail = store
        .load_playlist_detail(&saved.source.id, &playlist.id)
        .expect("load playlist detail")
        .expect("playlist detail");
    assert_eq!(detail.entries, entries);
    assert_eq!(detail.tracks, vec![track.clone(), track]);
}

#[test]
fn relation_preserve_order() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
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
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.source.id,
            &[repeated_track.clone(), other_track.clone()],
            generation,
        )
        .expect("upsert tracks");
    store
        .upsert_playlists(
            &saved.source.id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_entries(&saved.source.id, &playlist.id, &entries, generation)
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
            .count_tracks_for_source(&saved.source.id, &source)
            .expect("count source tracks"),
        2
    );
    assert_eq!(
        store
            .track_rank_for_source(
                &saved.source.id,
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
                &saved.source.id,
                &source,
                &repeated_track.id,
                Some("entry-one")
            )
            .expect("rank first duplicate"),
        Some(1)
    );
    assert_eq!(
        store
            .track_rank_for_source(&saved.source.id, &source, &other_track.id, None)
            .expect("rank filtered track"),
        None
    );

    let window = store
        .tracks_window_for_source(&saved.source.id, &source, 0, 0, 1)
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
            &saved.source.id,
            &source,
            &repeated_track.id,
            Some("entry-two"),
        )
        .expect("rank source occurrence")
        .expect("source occurrence rank");
    let window = store
        .tracks_window_for_source(&saved.source.id, &source, anchor_rank, 1, 1)
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

    let window = store
        .tracks_window_for_source(&saved.source.id, &source, 2, 1, 1)
        .expect("source window near end");
    assert_eq!(window.start_rank, 0);
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
fn relation_track_server() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        external_provider: None,
        lines: vec![LyricLine {
            start_millis: Some(12_000),
            text: "hello".to_string(),
        }],
    };
    store
        .save_lyrics(&saved.source.id, &lyrics)
        .expect("save lyrics");
    assert_eq!(
        store
            .load_lyrics(&saved.source.id, &track.id)
            .expect("load lyrics"),
        Some(lyrics)
    );
    assert_eq!(
        store
            .load_lyrics(&SourceId::fake(2), &track.id)
            .expect("load missing lyrics"),
        None
    );
}
#[test]
fn relation_preserve_lyrics() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let server_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Server,
        external_provider: None,
        lines: vec![LyricLine {
            start_millis: None,
            text: "server line".to_string(),
        }],
    };
    store
        .save_lyrics(&saved.source.id, &server_lyrics)
        .expect("save lyrics");

    assert!(
        !store
            .delete_remote_lyrics(&saved.source.id, &track.id)
            .expect("delete remote lyrics")
    );
    assert_eq!(
        store
            .load_lyrics(&saved.source.id, &track.id)
            .expect("load lyrics"),
        Some(server_lyrics)
    );

    let remote_lyrics = Lyrics {
        track_id: track.id.clone(),
        source: LyricsSource::Remote,
        external_provider: None,
        lines: vec![LyricLine {
            start_millis: None,
            text: "remote line".to_string(),
        }],
    };
    store
        .save_lyrics(&saved.source.id, &remote_lyrics)
        .expect("save remote lyrics");
    assert!(
        store
            .delete_remote_lyrics(&saved.source.id, &track.id)
            .expect("delete remote lyrics")
    );
    assert_eq!(
        store
            .load_lyrics(&saved.source.id, &track.id)
            .expect("load lyrics"),
        None
    );
}
#[test]
fn relation_track_favorite() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let mut album = album(1);
    album.favorite = false;
    let mut track = track(1, &album);
    track.favorite = false;
    let artist = artist(1, None);
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.source.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .set_album_favorite(&saved.source.id, &album.id, true)
        .expect("favorite album");
    store
        .set_track_favorite(&saved.source.id, &track.id, true)
        .expect("favorite track");
    store
        .set_artist_favorite(&saved.source.id, &artist.id, true)
        .expect("favorite artist");
    assert!(
        store
            .load_albums(&saved.source.id, 0, 1)
            .expect("load albums")
            .items[0]
            .favorite
    );
    assert!(
        store
            .load_tracks(&saved.source.id, 0, 1)
            .expect("load tracks")
            .items[0]
            .favorite
    );
    assert!(
        store
            .load_artists(&saved.source.id, false, 0, 1)
            .expect("load artists")
            .items[0]
            .favorite
    );
    assert_eq!(
        store
            .load_favorite_tracks(&saved.source.id)
            .expect("favorite tracks")
            .len(),
        1
    );
}
#[test]
fn genre_detail_tracks() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let track = track(1, &album);
    let genre = Genre {
        id: GenreId::new("jellyfin:genre:dream-pop"),
        name: "Dream Pop".to_string(),
        album_count: 0,
        track_count: 0,
        duration_seconds: 0,
        image_refs: Vec::new(),
        image_ref: Some(image_ref("genre-dream-pop", "tag")),
    };
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(&saved.source.id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    let detail = store
        .load_genre_detail(&saved.source.id, &genre.id)
        .expect("load genre detail")
        .expect("genre detail");
    assert_eq!(detail.genre.name, genre.name);
    assert_eq!(detail.genre.album_count, 1);
    assert_eq!(detail.genre.track_count, 1);
    assert_eq!(detail.albums, vec![album]);
    assert_eq!(detail.tracks, vec![track]);
}

#[test]
fn relation_return_genre() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let mut movie_genre = genre(2, None);
    movie_genre.name = "Science Fiction".to_string();
    let mut music_genre = genre(3, None);
    music_genre.name = "Dream Pop".to_string();
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_genres(
            &saved.source.id,
            &[movie_genre, music_genre.clone()],
            generation,
        )
        .expect("upsert genres");
    let genres = store
        .load_genres(&saved.source.id, 0, 20)
        .expect("load genres");
    assert_eq!(genres.total, 1);
    assert_eq!(genres.items[0].id, music_genre.id);
    assert_eq!(genres.items[0].name, music_genre.name);
    assert_eq!(genres.items[0].album_count, 1);
    assert_eq!(genres.items[0].track_count, 0);
}
#[test]
fn relation_use_counts() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Anime".to_string()];
    let track = track(1, &album);
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:anime"),
        name: "Anime".to_string(),
        album_count: 167,
        track_count: 1_561,
        duration_seconds: 0,
        image_refs: Vec::new(),
        image_ref: None,
    };
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.source.id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");
    let genres = store
        .load_genres(&saved.source.id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.source.id, &provider_genre.id)
        .expect("load genre detail")
        .expect("genre detail");
    assert_eq!(genres.items[0].album_count, 1);
    assert_eq!(genres.items[0].track_count, 1);
    assert_eq!(genres.items[0].duration_seconds, track.duration_seconds);
    assert_eq!(detail.genre.album_count, 1);
    assert_eq!(detail.genre.track_count, 1);
    assert_eq!(detail.genre.duration_seconds, track.duration_seconds);
}

#[test]
fn mood_projection_uses_track_metadata() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    let mut first = track(1, &album);
    first.moods = vec!["Focused".to_string(), "Energetic".to_string()];
    let mut second = track(2, &album);
    second.moods = vec!["Focused".to_string()];
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.source.id,
            &[first.clone(), second.clone()],
            generation,
        )
        .expect("upsert tracks");

    let moods = store
        .load_moods(&saved.source.id, 0, 20)
        .expect("load moods");
    let matching = store
        .load_moods_matching(&saved.source.id, "focus", 0, 20)
        .expect("search moods");
    let detail = store
        .load_mood_detail(&saved.source.id, &MoodId::new("Focused"))
        .expect("load mood detail")
        .expect("mood detail");

    assert_eq!(moods.total, 2);
    assert_eq!(
        moods
            .items
            .iter()
            .map(|mood| mood.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Energetic", "Focused"]
    );
    assert_eq!(matching.total, 1);
    assert_eq!(matching.items[0].name, "Focused");
    assert_eq!(matching.items[0].track_count, 2);
    assert_eq!(
        matching.items[0].duration_seconds,
        first.duration_seconds + second.duration_seconds
    );
    assert_eq!(detail.mood.name, "Focused");
    assert_eq!(detail.mood.track_count, 2);
    assert_eq!(detail.albums, vec![album]);
    assert_eq!(
        detail
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    assert_eq!(
        detail.tracks[0].moods,
        vec!["Energetic".to_string(), "Focused".to_string()]
    );
}

#[test]
fn track_only_counts() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.genres = vec!["Instrumental".to_string()];
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:instrumental"),
        name: "Instrumental".to_string(),
        album_count: 12,
        track_count: 99,
        duration_seconds: 0,
        image_refs: Vec::new(),
        image_ref: None,
    };
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.source.id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");

    let genres = store
        .load_genres(&saved.source.id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.source.id, &provider_genre.id)
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
fn missing_album_counts() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.genres = vec!["Instrumental".to_string()];
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:instrumental"),
        name: "Instrumental".to_string(),
        album_count: 12,
        track_count: 99,
        duration_seconds: 0,
        image_refs: Vec::new(),
        image_ref: None,
    };
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.source.id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");

    let genres = store
        .load_genres(&saved.source.id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.source.id, &provider_genre.id)
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
fn relation_track_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(9);
    let tracks = vec![track(1, &album), track(2, &album)];
    store
        .upsert_tracks(&saved.source.id, &tracks, generation)
        .expect("upsert tracks");
    let detail = store
        .load_album_detail(&saved.source.id, &album.id)
        .expect("load album detail")
        .expect("album detail");
    assert_eq!(detail.0.id, album.id);
    assert_eq!(detail.0.title, album.title);
    assert_eq!(detail.0.artist, album.artist);
    assert_eq!(detail.0.track_count, 2);
    assert_eq!(detail.1, tracks);
}
#[test]
fn relation_track_cache() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
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
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_artists(
            &saved.source.id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .upsert_artists(
            &saved.source.id,
            std::slice::from_ref(&artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    store
        .refresh_library_counts(&saved.source.id)
        .expect("refresh counts");
    let album = store
        .load_albums(&saved.source.id, 0, 1)
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
        .load_artists(&saved.source.id, false, 0, 1)
        .expect("load artists")
        .items
        .remove(0);
    let album_artist = store
        .load_artists(&saved.source.id, true, 0, 1)
        .expect("load album artists")
        .items
        .remove(0);
    assert_eq!(artist.album_count, 1);
    assert_eq!(artist.track_count, 2);
    assert_eq!(album_artist.album_count, 1);
    assert_eq!(album_artist.track_count, 2);
}
#[test]
fn artist_detail_primary() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
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
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    let mut track = track(1, &album);
    track.artist_id = None;
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.source.id,
            std::slice::from_ref(&artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    let detail = store
        .load_artist_detail(&saved.source.id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist, artist);
    assert_eq!(detail.albums, vec![album]);
    assert!(detail.appears_on.is_empty());
    assert_eq!(detail.tracks, vec![track]);
}
#[test]
fn artist_track_fallback() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    let artist_id = album.artist_id.clone().expect("artist id");
    store
        .upsert_tracks(&saved.source.id, &tracks, generation)
        .expect("upsert tracks");
    let detail = store
        .load_artist_detail(&saved.source.id, &artist_id)
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
fn relation_fall_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = saved_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source.id).expect("begin sync");
    let album = album(1);
    let artist_id = album.artist_id.clone().expect("artist id");
    let mut track = track(1, &album);
    track.artist_id = None;
    store
        .upsert_albums(&saved.source.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source.id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let detail = store
        .load_artist_detail(&saved.source.id, &artist_id)
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
