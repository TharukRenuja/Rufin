use std::mem::size_of;
use std::sync::Arc;

use library::{
    Album, AlbumId, AlbumRelations, Artist, ArtistCredit, ArtistId, CandidateBatch,
    CandidateFinish, CandidateHeader, Genre, GenreCredit, GenreId, HOME_SECTION_ITEM_LIMIT,
    HomeFacts, Library, MoodCredit, MoodId, MusicFolder, MusicFolderId, Playlist, PlaylistEntry,
    PlaylistId, PlaylistSnapshot, SmartPlaylistBuiltin, SmartPlaylistId, SourceId, Track,
    TrackData, TrackRelations, TrackSort,
};

const BATCH_SIZE: usize = 500;
const GENRE_COUNT: usize = 48;
const MOOD_COUNT: usize = 12;
const PLAYLIST_COUNT: usize = 4;
const PLAYLIST_TRACK_LIMIT: usize = 5_000;

#[test]
fn complete_loaded_projections_stay_compact_at_large_library_sizes() {
    assert_eq!(size_of::<Track>(), size_of::<usize>());

    assert_large_library(30_000);
    assert_large_library(50_000);
}

fn assert_large_library(track_count: usize) {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let library = Library::open(directory.path().join("library.db")).expect("open Library");
    let source_id = SourceId::new(format!("subsonic:bounds:{track_count}"));
    let folder_id = MusicFolderId::new("folder:all");
    let album_count = track_count.div_ceil(10);
    let artist_count = track_count.div_ceil(100);

    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id,
            input_version: 1,
            input_digest: [track_count as u8; 32],
        })
        .expect("begin source candidate");

    write_batches(&mut candidate, album_count, |range| {
        CandidateBatch::Albums(range.map(album).collect())
    });
    write_batches(&mut candidate, artist_count, |range| {
        CandidateBatch::Artists(range.map(artist).collect())
    });
    candidate
        .write(CandidateBatch::Genres(
            (0..GENRE_COUNT).map(genre).collect(),
        ))
        .expect("write Genres");
    candidate
        .write(CandidateBatch::MusicFolders(vec![MusicFolder {
            id: folder_id.clone(),
            name: "All music".to_string(),
        }]))
        .expect("write music folder");
    write_batches(&mut candidate, track_count, |range| {
        CandidateBatch::Tracks(range.map(|index| track(index, &folder_id)).collect())
    });
    candidate
        .write(CandidateBatch::Playlists(playlists(track_count)))
        .expect("write Playlists");

    let loaded = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept source candidate")
        .loaded;
    library
        .initialize_smart_playlists(&loaded)
        .expect("initialize smart Playlists");
    let home = library.home(&loaded, None).expect("compose Home");

    assert_eq!(loaded.counts().expect("read counts").tracks, track_count);
    assert_eq!(loaded.counts().expect("read counts").albums, album_count);
    assert!(
        home.sections
            .iter()
            .all(|section| section.items.len() <= HOME_SECTION_ITEM_LIMIT)
    );
    assert!(home.genres.len() <= GENRE_COUNT);
    assert!(home.showcase.is_some());

    drop(library);

    {
        let tracks = loaded
            .track_list(None, TrackSort::Title, false)
            .expect("project Tracks");
        let shared_order = tracks.clone();
        assert_eq!(tracks.len(), track_count);
        assert!(tracks.shares_order(&shared_order));
        let first = tracks
            .track(0)
            .expect("resolve first Track")
            .expect("first Track");
        let selected = loaded
            .track(&first.id)
            .expect("resolve selected Track")
            .expect("selected Track");
        assert!(Track::ptr_eq(&first, &selected));

        assert_eq!(
            loaded
                .favorite_track_list(None, TrackSort::Title, false)
                .expect("project favorite Tracks")
                .len(),
            track_count.div_ceil(17)
        );
        assert_eq!(
            loaded.albums(None).expect("project Albums").len(),
            album_count
        );
        assert_eq!(
            loaded.artists(None).expect("project Artists").len(),
            artist_count
        );
        assert_eq!(
            loaded
                .album_artists(None)
                .expect("project Album Artists")
                .len(),
            artist_count
        );
        assert_eq!(
            loaded.genres(None).expect("project Genres").len(),
            GENRE_COUNT
        );
        assert_eq!(loaded.moods(None).expect("project Moods").len(), MOOD_COUNT);
        assert_eq!(
            loaded.playlists().expect("project Playlists").len(),
            PLAYLIST_COUNT
        );
        assert_eq!(
            loaded
                .smart_playlists(None)
                .expect("project smart Playlists")
                .len(),
            SmartPlaylistBuiltin::all().len()
        );
        assert_eq!(
            loaded
                .track_list(Some(&folder_id), TrackSort::Title, false)
                .expect("project folder Tracks")
                .len(),
            track_count
        );
        assert_eq!(
            loaded
                .albums(Some(&folder_id))
                .expect("project folder Albums")
                .len(),
            album_count
        );
        assert_eq!(
            loaded
                .artists(Some(&folder_id))
                .expect("project folder Artists")
                .len(),
            artist_count
        );

        let album = loaded
            .album_detail(&AlbumId::fake(0), None)
            .expect("project Album detail")
            .expect("Album detail");
        assert_eq!(album.tracks.len(), 10);
        let artist = loaded
            .artist_overview(&ArtistId::fake(0), None)
            .expect("project Artist overview")
            .expect("Artist overview");
        assert_eq!(artist.summary.track_count, 100);
        assert_eq!(artist.albums.len(), 10);
        assert!(
            loaded
                .artist_discography(&ArtistId::fake(0), None)
                .expect("project Artist discography")
                .is_some()
        );
        assert_eq!(
            loaded
                .artist_track_detail(&ArtistId::fake(0), None)
                .expect("project Artist Tracks")
                .expect("Artist Tracks")
                .tracks
                .len(),
            100
        );
        assert_eq!(
            loaded
                .genre_detail(&GenreId::fake(0), None)
                .expect("project Genre detail")
                .expect("Genre detail")
                .tracks
                .len(),
            (0..track_count)
                .filter(|index| { index % GENRE_COUNT == 0 || (index / 10) % GENRE_COUNT == 0 })
                .count()
        );
        assert_eq!(
            loaded
                .mood_detail(&MoodId::fake(0), None)
                .expect("project Mood detail")
                .expect("Mood detail")
                .tracks
                .len(),
            track_count.div_ceil(MOOD_COUNT)
        );
        assert_eq!(
            loaded
                .playlist_detail(&PlaylistId::new("playlist:0"))
                .expect("project Playlist detail")
                .expect("Playlist detail")
                .entries
                .len(),
            PLAYLIST_TRACK_LIMIT
        );
        assert_eq!(
            loaded
                .smart_playlist_detail(&SmartPlaylistId::new("builtin:never_played"), None,)
                .expect("project smart Playlist detail")
                .expect("smart Playlist detail")
                .tracks
                .len(),
            track_count
        );
    }

    assert_eq!(Arc::strong_count(&loaded), 1);
}

fn write_batches(
    candidate: &mut library::SourceCandidate,
    count: usize,
    mut batch: impl FnMut(std::ops::Range<usize>) -> CandidateBatch,
) {
    for start in (0..count).step_by(BATCH_SIZE) {
        candidate
            .write(batch(start..(start + BATCH_SIZE).min(count)))
            .expect("write candidate batch");
    }
}

fn album(index: usize) -> Album {
    let artist = artist_credit(index / 10);
    let genre = genre_credit(index % GENRE_COUNT);
    Album {
        id: AlbumId::fake(index),
        title: format!("Album {index:05}"),
        artist: artist.name.clone(),
        year: 1980 + (index % 47) as u16,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        favorite: false,
        color_seed: 0,
        image_ref: None,
        local_artwork: None,
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
        relations: AlbumRelations {
            album_artists: vec![artist.clone()],
            artists: vec![artist],
            genres: vec![genre],
        },
    }
}

fn artist(index: usize) -> Artist {
    Artist {
        id: ArtistId::fake(index),
        name: format!("Artist {index:05}"),
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
        local_artwork: None,
    }
}

fn genre(index: usize) -> Genre {
    Genre {
        id: GenreId::fake(index),
        name: format!("Genre {index:02}"),
        image_ref: None,
    }
}

fn track(index: usize, folder_id: &MusicFolderId) -> Track {
    let album_index = index / 10;
    let artist = artist_credit(index / 100);
    Track::new(TrackData {
        id: library::TrackId::fake(index),
        album_id: Some(AlbumId::fake(album_index)),
        title: format!("Track {index:06}"),
        artist: artist.name.clone(),
        album: format!("Album {album_index:05}"),
        album_artwork: None,
        year: 1980 + (album_index % 47) as u16,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: 180 + (index % 120) as u32,
        favorite: index % 17 == 0,
        disc_number: 1,
        track_number: (index % 10 + 1) as u16,
        image_ref: None,
        local_artwork: None,
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        source_path: None,
        cue: None,
        source_format: Some("flac".to_string()),
        comment: None,
        skip_count: None,
        bpm: None,
        relations: TrackRelations {
            artists: vec![artist.clone()],
            album_artists: vec![artist],
            genres: vec![genre_credit(index % GENRE_COUNT)],
            moods: vec![MoodCredit {
                id: MoodId::fake(index % MOOD_COUNT),
                name: format!("Mood {:02}", index % MOOD_COUNT),
            }],
            music_folders: vec![folder_id.clone()],
        },
    })
}

fn artist_credit(index: usize) -> ArtistCredit {
    ArtistCredit {
        id: ArtistId::fake(index),
        name: format!("Artist {index:05}"),
        musicbrainz_artist_id: None,
    }
}

fn genre_credit(index: usize) -> GenreCredit {
    GenreCredit {
        id: GenreId::fake(index),
        name: format!("Genre {index:02}"),
    }
}

fn playlists(track_count: usize) -> Vec<PlaylistSnapshot> {
    let entries = track_count.min(PLAYLIST_TRACK_LIMIT);
    (0..PLAYLIST_COUNT)
        .map(|playlist| PlaylistSnapshot {
            playlist: Playlist {
                id: PlaylistId::new(format!("playlist:{playlist}")),
                name: format!("Playlist {}", playlist + 1),
                image_ref: None,
            },
            entries: (0..entries)
                .map(|position| PlaylistEntry {
                    occurrence_id: format!("{playlist}:{position}"),
                    track_id: library::TrackId::fake((position + playlist * entries) % track_count),
                })
                .collect(),
        })
        .collect()
}
