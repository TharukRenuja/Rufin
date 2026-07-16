use super::test_support::*;
use crate::play_context::{
    PlayContext, PlayContextAnchor, PlayContextDescriptor, PlayContextOrder, PlaylistSort,
};
use crate::{MoodId, RandomTrackQuery};

#[test]
fn relation_keep_id() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
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
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert tracks");
    store
        .upsert_playlists(
            &saved.source_id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_entries(&saved.source_id, &playlist.id, &entries, generation)
        .expect("upsert playlist entries");
    let detail = store
        .load_playlist_detail(&saved.source_id, &playlist.id)
        .expect("load playlist detail")
        .expect("playlist detail");
    assert_eq!(
        detail
            .entries
            .iter()
            .map(|entry| (entry.entry_id.as_str(), entry.track.id.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("entry-one", track.id.clone()),
            ("entry-two", track.id.clone()),
        ]
    );
    assert_eq!(
        detail
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![track.id.clone(), track.id]
    );
}

#[test]
fn relation_preserve_order() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
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
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.source_id,
            &[repeated_track.clone(), other_track.clone()],
            generation,
        )
        .expect("upsert tracks");
    store
        .upsert_playlists(
            &saved.source_id,
            std::slice::from_ref(&playlist),
            generation,
        )
        .expect("upsert playlist");
    store
        .upsert_playlist_entries(&saved.source_id, &playlist.id, &entries, generation)
        .expect("upsert playlist entries");

    let context = PlayContext {
        descriptor: PlayContextDescriptor::Playlist {
            playlist_id: playlist.id.clone(),
        },
        order: PlayContextOrder::Playlist {
            query: Some("echo".to_string()),
            sort: PlaylistSort::Title,
            descending: true,
        },
    };
    let materialized = store
        .materialize_play_context(
            &saved.source_id,
            &context,
            &PlayContextAnchor {
                track_id: repeated_track.id.clone(),
                source_rank: 0,
                source_item_id: Some("entry-two".to_string()),
            },
        )
        .expect("materialize filtered playlist");
    assert_eq!(
        materialized
            .items
            .iter()
            .map(|item| item.source_item_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("entry-two"), Some("entry-one")]
    );
    assert_eq!(
        materialized
            .items
            .iter()
            .map(|item| item.source_rank)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );

    let context = PlayContext {
        descriptor: PlayContextDescriptor::Playlist {
            playlist_id: playlist.id,
        },
        order: PlayContextOrder::Playlist {
            query: None,
            sort: PlaylistSort::Position,
            descending: false,
        },
    };
    let materialized = store
        .materialize_play_context(
            &saved.source_id,
            &context,
            &PlayContextAnchor {
                track_id: repeated_track.id,
                source_rank: 1,
                source_item_id: Some("entry-two".to_string()),
            },
        )
        .expect("materialize complete playlist");
    assert_eq!(
        materialized
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
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let payload = r#"{"source":"Remote","lines":[{"text":"hello","start_millis":12000}]}"#;
    store
        .save_lyrics_payload(&saved.source_id, &track.id, "remote", payload)
        .expect("save lyrics");
    assert_eq!(
        store
            .load_lyrics_payload(&saved.source_id, &track.id)
            .expect("load lyrics"),
        Some(payload.to_string())
    );
    assert_eq!(
        store
            .load_lyrics_payload(&SourceId::fake(2), &track.id)
            .expect("load missing lyrics"),
        None
    );
}
#[test]
fn relation_preserve_lyrics() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let server_payload = r#"{"source":"Server","lines":[{"text":"server line"}]}"#;
    store
        .save_lyrics_payload(&saved.source_id, &track.id, "server", server_payload)
        .expect("save lyrics");

    assert!(
        !store
            .delete_lyrics_payload(&saved.source_id, &track.id, "remote")
            .expect("delete remote lyrics")
    );
    assert_eq!(
        store
            .load_lyrics_payload(&saved.source_id, &track.id)
            .expect("load lyrics"),
        Some(server_payload.to_string())
    );

    let remote_payload = r#"{"source":"Remote","lines":[{"text":"remote line"}]}"#;
    store
        .save_lyrics_payload(&saved.source_id, &track.id, "remote", remote_payload)
        .expect("save remote lyrics");
    assert!(
        store
            .delete_lyrics_payload(&saved.source_id, &track.id, "remote")
            .expect("delete remote lyrics")
    );
    assert_eq!(
        store
            .load_lyrics_payload(&saved.source_id, &track.id)
            .expect("load lyrics"),
        None
    );
}
#[test]
fn relation_track_favorite() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let mut album = album(1);
    album.favorite = false;
    let mut track = track(1, &album);
    track.favorite = false;
    let artist = artist(1, None);
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.source_id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .set_album_favorite(&saved.source_id, &album.id, true)
        .expect("favorite album");
    store
        .set_track_favorite(&saved.source_id, &track.id, true)
        .expect("favorite track");
    store
        .set_artist_favorite(&saved.source_id, &artist.id, true)
        .expect("favorite artist");
    assert!(
        store
            .load_albums(&saved.source_id, 0, 1)
            .expect("load albums")
            .items[0]
            .favorite
    );
    assert!(
        store
            .load_tracks(&saved.source_id, 0, 1)
            .expect("load tracks")
            .items[0]
            .favorite
    );
    assert!(
        store
            .load_artists(&saved.source_id, false, 0, 1)
            .expect("load artists")
            .items[0]
            .favorite
    );
    assert_eq!(
        store
            .load_favorite_tracks(&saved.source_id)
            .expect("favorite tracks")
            .len(),
        1
    );
}
#[test]
fn genre_detail_tracks() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let track = track(1, &album);
    let genre = Genre {
        id: GenreId::new("jellyfin:genre:dream-pop"),
        name: "Dream Pop".to_string(),
        album_count: 0,
        track_count: 0,
        duration_seconds: 0,
        image_ref: Some(image_ref("genre-dream-pop", "tag")),
        representative_albums: Vec::new(),
    };
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(&saved.source_id, std::slice::from_ref(&genre), generation)
        .expect("upsert genre");
    let (detail, statements) = trace_read_statements(&store, || {
        store
            .load_genre_detail(&saved.source_id, &genre.id)
            .expect("load genre detail")
            .expect("genre detail")
    });
    assert!(
        statements
            .iter()
            .all(|sql| !sql.contains("SELECT DISTINCT a.album_id"))
    );
    assert_eq!(detail.genre.name, genre.name);
    assert_eq!(detail.genre.album_count, 1);
    assert_eq!(detail.genre.track_count, 1);
    assert_eq!(detail.tracks[0].id, track.id);
}

#[test]
fn cached_random_tracks_filter_wrap_and_stay_bounded() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut ambient_album = album(1);
    ambient_album.genres = vec!["Ambient".to_string()];
    ambient_album.year = 2020;
    let mut rock_album = album(2);
    rock_album.genres = vec!["Rock".to_string()];
    rock_album.year = 2010;
    let mut ambient_genre = genre(1, None);
    ambient_genre.id = GenreId::new("local:genre:ambient");
    ambient_genre.name = "Ambient".to_string();
    let mut rock_genre = genre(2, None);
    rock_genre.id = GenreId::new("local:genre:rock");
    rock_genre.name = "Rock".to_string();
    let mut tracks = (1..=510)
        .map(|number| {
            let mut track = track(number, &ambient_album);
            track.id = TrackId::new(format!("local:track:{number:016x}"));
            track
        })
        .collect::<Vec<_>>();
    tracks.extend((511..=520).map(|number| {
        let mut track = track(number, &rock_album);
        track.id = TrackId::new(format!("local:track:{number:016x}"));
        track
    }));
    case.upsert_albums(&case.id, &[ambient_album, rock_album], generation)
        .expect("upsert albums");
    case.upsert_tracks(&case.id, &tracks, generation)
        .expect("upsert tracks");
    case.upsert_genres(
        &case.id,
        &[ambient_genre.clone(), rock_genre.clone()],
        generation,
    )
    .expect("upsert genres");

    let mut request = RandomTrackQuery {
        limit: 999,
        min_year: Some(2020),
        max_year: Some(2020),
        genre_id: Some(ambient_genre.id.clone()),
        genre_name: Some(ambient_genre.name.clone()),
    };
    let selected = case
        .load_cached_random_tracks(&case.id, "local:track:000000000000012c", &request)
        .expect("load cached random tracks");
    assert_eq!(selected.len(), 500);
    assert_eq!(selected[0].id.as_str(), "local:track:000000000000012c");
    assert_eq!(selected[499].id.as_str(), "local:track:0000000000000121");
    assert!(
        selected
            .iter()
            .all(|track| track.year == 2020 && track.genres == ["Ambient"])
    );

    request.limit = 0;
    assert_eq!(
        case.load_cached_random_tracks(&case.id, "local:track:000000000000012c", &request)
            .expect("clamp minimum")
            .len(),
        1
    );
    request.genre_id = Some(rock_genre.id);
    assert!(
        case.load_cached_random_tracks(&case.id, "local:track:000000000000012c", &request)
            .expect("apply genre id and name together")
            .is_empty()
    );
}

#[test]
fn track_id_batch_read_preserves_occurrences_and_durable_cached_facts() {
    let case = StoreCase::open();
    let generation = case.start_sync("begin sync");
    let mut album = album(1);
    album.genres = vec!["Ambient".to_string()];
    let mut first = track(1, &album);
    first.local_path = Some("/music/first.flac".to_string());
    first.source_format = Some("FLAC".to_string());
    first.comment = Some("opening".to_string());
    first.skip_count = Some(2);
    first.bpm = Some(110);
    first.moods = vec!["Focused".to_string()];
    let mut second = track(2, &album);
    second.local_path = Some("/music/second.flac".to_string());
    second.source_format = Some("FLAC".to_string());
    second.comment = Some("closing".to_string());
    second.skip_count = Some(3);
    second.bpm = Some(120);
    second.moods = vec!["Energetic".to_string()];
    case.upsert_albums(&case.id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    case.upsert_tracks(&case.id, &[first.clone(), second.clone()], generation)
        .expect("upsert tracks");

    let mut loaded = case
        .load_tracks_by_ids(
            &case.id,
            &[
                second.id.clone(),
                TrackId::new("missing"),
                first.id.clone(),
                second.id.clone(),
            ],
        )
        .expect("load tracks by ids");
    for track in &mut loaded {
        track.album_artwork = None;
    }
    assert_eq!(loaded, vec![second.clone(), first, second]);
    assert!(
        case.load_tracks_by_ids(&case.id, &[])
            .expect("empty id batch")
            .is_empty()
    );
}

#[test]
fn relation_return_genre() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Dream Pop".to_string()];
    let mut movie_genre = genre(2, None);
    movie_genre.name = "Science Fiction".to_string();
    let mut music_genre = genre(3, None);
    music_genre.name = "Dream Pop".to_string();
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_genres(
            &saved.source_id,
            &[movie_genre, music_genre.clone()],
            generation,
        )
        .expect("upsert genres");
    let genres = store
        .load_genres(&saved.source_id, 0, 20)
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
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let mut album = album(1);
    album.genres = vec!["Anime".to_string()];
    let track = track(1, &album);
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:anime"),
        name: "Anime".to_string(),
        album_count: 167,
        track_count: 1_561,
        duration_seconds: 0,
        image_ref: None,
        representative_albums: Vec::new(),
    };
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.source_id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");
    let genres = store
        .load_genres(&saved.source_id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.source_id, &provider_genre.id)
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
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    let mut first = track(1, &album);
    first.moods = vec!["Focused".to_string(), "Energetic".to_string()];
    let mut second = track(2, &album);
    second.moods = vec!["Focused".to_string()];
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(
            &saved.source_id,
            &[first.clone(), second.clone()],
            generation,
        )
        .expect("upsert tracks");

    let ((moods, matching, detail), statements) = trace_read_statements(&store, || {
        let moods = store
            .load_moods(&saved.source_id, 0, 20)
            .expect("load moods");
        let matching = store
            .load_moods_matching(&saved.source_id, "focus", 0, 20)
            .expect("search moods");
        let detail = store
            .load_mood_detail(&saved.source_id, &MoodId::new("Focused"))
            .expect("load mood detail")
            .expect("mood detail");
        (moods, matching, detail)
    });
    let aggregate_queries = statements
        .iter()
        .filter(|sql| sql.contains("FROM track_moods tm") && sql.contains("COUNT(*)"))
        .collect::<Vec<_>>();
    assert!(
        statements
            .iter()
            .all(|sql| !sql.contains("SELECT DISTINCT a.album_id"))
    );
    assert_eq!(aggregate_queries.len(), 3);
    for sql in aggregate_queries {
        let plan = explain_query_plan(&store, sql);
        assert!(
            plan.iter()
                .any(|detail| detail.contains("track_moods_source_mood_idx")),
            "{plan:?}"
        );
        assert!(
            plan.iter()
                .all(|detail| !detail.contains("count(DISTINCT)")),
            "{plan:?}"
        );
    }

    assert_eq!(moods.total, 2);
    assert_eq!(
        moods
            .items
            .iter()
            .map(|mood| (mood.name.as_str(), mood.track_count, mood.duration_seconds))
            .collect::<Vec<_>>(),
        vec![
            ("Energetic", 1, first.duration_seconds),
            (
                "Focused",
                2,
                first.duration_seconds + second.duration_seconds
            ),
        ]
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
    assert_eq!(
        detail.mood.duration_seconds,
        first.duration_seconds + second.duration_seconds
    );
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
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.genres = vec!["Instrumental".to_string()];
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:instrumental"),
        name: "Instrumental".to_string(),
        album_count: 12,
        track_count: 99,
        duration_seconds: 0,
        image_ref: None,
        representative_albums: Vec::new(),
    };
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.source_id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");

    let genres = store
        .load_genres(&saved.source_id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.source_id, &provider_genre.id)
        .expect("load genre detail")
        .expect("genre detail");

    assert_eq!(genres.items[0].album_count, 1);
    assert_eq!(genres.items[0].track_count, 1);
    assert_eq!(detail.genre.album_count, 1);
    assert_eq!(detail.genre.track_count, 1);
    assert_eq!(detail.tracks[0].id, track.id);
}

#[test]
fn missing_album_counts() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    let mut track = track(1, &album);
    track.genres = vec!["Instrumental".to_string()];
    let provider_genre = Genre {
        id: GenreId::new("jellyfin:genre:instrumental"),
        name: "Instrumental".to_string(),
        album_count: 12,
        track_count: 99,
        duration_seconds: 0,
        image_ref: None,
        representative_albums: Vec::new(),
    };
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_genres(
            &saved.source_id,
            std::slice::from_ref(&provider_genre),
            generation,
        )
        .expect("upsert genre");

    let genres = store
        .load_genres(&saved.source_id, 0, 20)
        .expect("load genres");
    let detail = store
        .load_genre_detail(&saved.source_id, &provider_genre.id)
        .expect("load genre detail")
        .expect("genre detail");

    assert_eq!(genres.items[0].album_count, 0);
    assert_eq!(genres.items[0].track_count, 1);
    assert_eq!(detail.genre.album_count, 0);
    assert_eq!(detail.genre.track_count, 1);
    assert_eq!(detail.tracks, vec![track]);
}

#[test]
fn relation_track_missing() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(9);
    let tracks = vec![track(1, &album), track(2, &album)];
    store
        .upsert_tracks(&saved.source_id, &tracks, generation)
        .expect("upsert tracks");
    let detail = store
        .load_album_detail(&saved.source_id, &album.id)
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
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
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
        representative_albums: Vec::new(),
    };
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, &tracks, generation)
        .expect("upsert tracks");
    store
        .upsert_artists(
            &saved.source_id,
            std::slice::from_ref(&artist),
            false,
            generation,
        )
        .expect("upsert artist");
    store
        .upsert_artists(
            &saved.source_id,
            std::slice::from_ref(&artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    store
        .refresh_library_counts(&saved.source_id)
        .expect("refresh counts");
    let album = store
        .load_albums(&saved.source_id, 0, 1)
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
        .load_artists(&saved.source_id, false, 0, 1)
        .expect("load artists")
        .items
        .remove(0);
    let album_artist = store
        .load_artists(&saved.source_id, true, 0, 1)
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
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
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
        representative_albums: Vec::new(),
    };
    let mut track = track(1, &album);
    track.artist_id = None;
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    store
        .upsert_artists(
            &saved.source_id,
            std::slice::from_ref(&artist),
            true,
            generation,
        )
        .expect("upsert album artist");
    let detail = store
        .load_artist_detail(&saved.source_id, &artist.id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist.id, artist.id);
    assert_eq!(detail.artist.name, artist.name);
    assert_eq!(detail.artist.image_ref, artist.image_ref);
    assert_eq!(
        detail
            .artist
            .representative_albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>(),
        vec![album.id.clone()]
    );
    assert_eq!(detail.albums, vec![album]);
    assert!(detail.appears_on.is_empty());
    assert_eq!(
        detail
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![track.id]
    );
}
#[test]
fn artist_track_fallback() {
    let store = Store::open_memory().expect("open store");
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    let tracks = vec![track(1, &album), track(2, &album)];
    let artist_id = album.artist_id.clone().expect("artist id");
    store
        .upsert_tracks(&saved.source_id, &tracks, generation)
        .expect("upsert tracks");
    let detail = store
        .load_artist_detail(&saved.source_id, &artist_id)
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
    let saved = stored_source();
    store.save_source(&saved).expect("save server");
    let generation = store.begin_sync(&saved.source_id).expect("begin sync");
    let album = album(1);
    let artist_id = album.artist_id.clone().expect("artist id");
    let mut track = track(1, &album);
    track.artist_id = None;
    store
        .upsert_albums(&saved.source_id, std::slice::from_ref(&album), generation)
        .expect("upsert album");
    store
        .upsert_tracks(&saved.source_id, std::slice::from_ref(&track), generation)
        .expect("upsert track");
    let detail = store
        .load_artist_detail(&saved.source_id, &artist_id)
        .expect("load artist detail")
        .expect("artist detail");
    assert_eq!(detail.artist.id, artist_id);
    assert_eq!(detail.artist.name, album.artist);
    assert_eq!(detail.artist.album_count, 1);
    assert_eq!(detail.artist.track_count, 1);
    assert_eq!(detail.albums[0].id, album.id);
    assert!(detail.appears_on.is_empty());
    assert_eq!(detail.tracks[0].id, track.id);
}
