use library::{
    AcceptedPlay, Album, AlbumId, AlbumRelations, Artist, ArtistCredit, ArtistId, CandidateBatch,
    CandidateFinish, CandidateHeader, Genre, GenreCredit, GenreId, HomeFacts, Libraries,
    MoodCredit, MoodId, MusicFolder, MusicFolderId, Playlist, PlaylistEntry, PlaylistId,
    PlaylistSnapshot, SmartPlaylistBuiltin, SourceId, Track, TrackData, TrackId, TrackRelations,
};

#[test]
fn collection_download_status_distinguishes_any_from_all_tracks() {
    let directory = tempfile::tempdir().expect("temporary Library");
    let library = Libraries::open(directory.path().join("library.db")).expect("open Library");
    let source_id = SourceId::new("subsonic:download-status");
    let album_id = AlbumId::fake(1);
    let artist_id = ArtistId::fake(1);
    let genre_id = GenreId::fake(1);
    let mood_id = MoodId::fake(1);
    let folder_id = MusicFolderId::fake(1);
    let playlist_id = PlaylistId::fake(1);
    let tracks = vec![
        track(1, &album_id, &artist_id, &genre_id, &mood_id, &folder_id),
        track(2, &album_id, &artist_id, &genre_id, &mood_id, &folder_id),
    ];
    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id,
            input_digest: [6; 32],
        })
        .expect("begin source candidate");
    candidate
        .write(CandidateBatch::Albums(vec![album(
            album_id.clone(),
            &artist_id,
            &genre_id,
        )]))
        .expect("write album");
    candidate
        .write(CandidateBatch::Artists(vec![artist(artist_id.clone())]))
        .expect("write artist");
    candidate
        .write(CandidateBatch::Genres(vec![Genre {
            id: genre_id.clone(),
            name: "Genre".to_string(),
            image_ref: None,
        }]))
        .expect("write genre");
    candidate
        .write(CandidateBatch::MusicFolders(vec![MusicFolder {
            id: folder_id.clone(),
            name: "Music".to_string(),
            image_ref: None,
        }]))
        .expect("write music folder");
    candidate
        .write(CandidateBatch::Tracks(tracks.clone()))
        .expect("write tracks");
    candidate
        .write(CandidateBatch::Playlists(vec![PlaylistSnapshot {
            playlist: Playlist {
                id: playlist_id.clone(),
                name: "Playlist".to_string(),
                image_ref: None,
            },
            entries: tracks
                .iter()
                .enumerate()
                .map(|(position, track)| PlaylistEntry {
                    occurrence_id: position.to_string(),
                    track_id: track.id.clone(),
                })
                .collect(),
        }]))
        .expect("write playlist");
    let loaded = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(library::PreparedSourceCandidate::accept)
        .expect("accept source")
        .library;
    loaded
        .initialize_smart_playlists()
        .expect("initialize smart playlists");
    let smart_playlist_id = loaded
        .smart_playlists(Some(&folder_id))
        .expect("load smart playlists")
        .into_iter()
        .find(|playlist| playlist.smart_playlist.builtin == Some(SmartPlaylistBuiltin::NeverPlayed))
        .expect("Never Played playlist")
        .smart_playlist
        .id
        .clone();

    assert!(!all_collection_statuses(
        &loaded,
        &album_id,
        &artist_id,
        &genre_id,
        &mood_id,
        &folder_id,
        &playlist_id,
        &smart_playlist_id,
    ));
    assert_eq!(
        loaded
            .album_track_selection(&album_id, Some(&folder_id))
            .download_status()
            .expect("album download status"),
        library::DownloadStatus::default()
    );
    loaded
        .set_downloaded_file(tracks[0].id.clone(), directory.path().join("one.audio"))
        .expect("mark first track downloaded");
    assert!(!all_collection_statuses(
        &loaded,
        &album_id,
        &artist_id,
        &genre_id,
        &mood_id,
        &folder_id,
        &playlist_id,
        &smart_playlist_id,
    ));
    let partial = library::DownloadStatus {
        any: true,
        complete: false,
    };
    assert_eq!(
        loaded
            .album_track_selection(&album_id, Some(&folder_id))
            .download_status()
            .expect("album download status"),
        partial
    );
    assert_eq!(
        loaded
            .playlist_track_selection(&playlist_id)
            .download_status()
            .expect("playlist download status"),
        partial
    );
    let recorded = loaded
        .record_play(AcceptedPlay {
            play_id: "download-status:play".to_string(),
            track_id: tracks[1].id.clone(),
            played_at: 1_700_000_000,
            month: "2023-11".to_string(),
        })
        .expect("record play")
        .expect("new play");
    let activity = loaded
        .apply_recorded_activity(&recorded)
        .expect("apply play activity")
        .expect("play changes smart playlist membership");
    assert!(activity.smart_playlists.contains(&smart_playlist_id));
    assert!(
        loaded
            .is_smart_playlist_downloaded(&smart_playlist_id, Some(&folder_id))
            .expect("Never Played download status after membership change")
    );
    loaded
        .set_downloaded_file(tracks[1].id.clone(), directory.path().join("two.audio"))
        .expect("mark second track downloaded");

    assert!(all_collection_statuses(
        &loaded,
        &album_id,
        &artist_id,
        &genre_id,
        &mood_id,
        &folder_id,
        &playlist_id,
        &smart_playlist_id,
    ));
    let complete = library::DownloadStatus {
        any: true,
        complete: true,
    };
    assert_eq!(
        loaded
            .album_track_selection(&album_id, Some(&folder_id))
            .download_status()
            .expect("album download status"),
        complete
    );
    assert_eq!(
        loaded
            .playlist_track_selection(&playlist_id)
            .download_status()
            .expect("playlist download status"),
        complete
    );
}

#[allow(clippy::too_many_arguments)]
fn all_collection_statuses(
    loaded: &std::sync::Arc<library::Library>,
    album_id: &AlbumId,
    artist_id: &ArtistId,
    genre_id: &GenreId,
    mood_id: &MoodId,
    folder_id: &MusicFolderId,
    playlist_id: &PlaylistId,
    smart_playlist_id: &library::SmartPlaylistId,
) -> bool {
    loaded
        .is_album_downloaded(album_id, Some(folder_id))
        .expect("album status")
        && loaded
            .is_artist_downloaded(artist_id, Some(folder_id))
            .expect("artist status")
        && loaded
            .is_genre_downloaded(genre_id, Some(folder_id))
            .expect("genre status")
        && loaded
            .is_mood_downloaded(mood_id, Some(folder_id))
            .expect("mood status")
        && loaded
            .is_playlist_downloaded(playlist_id)
            .expect("playlist status")
        && loaded
            .is_smart_playlist_downloaded(smart_playlist_id, Some(folder_id))
            .expect("smart playlist status")
}

fn album(id: AlbumId, artist_id: &ArtistId, genre_id: &GenreId) -> Album {
    let artist = artist_credit(artist_id);
    Album {
        id,
        title: "Album".to_string(),
        artist: artist.name.clone(),
        year: 2026,
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
            genres: vec![genre_credit(genre_id)],
        },
    }
}

fn artist(id: ArtistId) -> Artist {
    Artist {
        id,
        name: "Artist".to_string(),
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
        local_artwork: None,
    }
}

fn track(
    index: usize,
    album_id: &AlbumId,
    artist_id: &ArtistId,
    genre_id: &GenreId,
    mood_id: &MoodId,
    folder_id: &MusicFolderId,
) -> Track {
    let artist = artist_credit(artist_id);
    Track::new(TrackData {
        id: TrackId::fake(index),
        album_id: Some(album_id.clone()),
        title: format!("Track {index}"),
        artist: artist.name.clone(),
        album: "Album".to_string(),
        album_artwork: None,
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: 180,
        favorite: false,
        disc_number: 1,
        track_number: index as u16,
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
            genres: vec![genre_credit(genre_id)],
            moods: vec![MoodCredit {
                id: mood_id.clone(),
                name: "Mood".to_string(),
            }],
            music_folders: vec![folder_id.clone()],
        },
    })
}

fn artist_credit(id: &ArtistId) -> ArtistCredit {
    ArtistCredit {
        id: id.clone(),
        name: "Artist".to_string(),
        musicbrainz_artist_id: None,
    }
}

fn genre_credit(id: &GenreId) -> GenreCredit {
    GenreCredit {
        id: id.clone(),
        name: "Genre".to_string(),
    }
}
