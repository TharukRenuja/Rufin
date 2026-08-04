use std::sync::Arc;

use library::{
    Album, AlbumArtwork, AlbumId, AlbumRelations, AlbumSummary, Artist, CandidateBatch,
    CandidateFinish, CandidateHeader, HomeFacts, Libraries, Library, Playlist, PlaylistEntry,
    PlaylistId, PlaylistSnapshot, PlaylistSummary, STORE_ROW_BATCH_LIMIT, SmartPlaylist,
    SmartPlaylistDefinition, SmartPlaylistId, SmartPlaylistSortField, SmartPlaylistSummary,
    SourceId, Track, TrackData, TrackId, TrackRelations,
};

pub(crate) struct SourceFixture {
    pub(crate) library: Arc<Library>,
    _directory: tempfile::TempDir,
}

pub(crate) fn track(id: impl std::fmt::Display, title: impl Into<String>) -> Track {
    Track::new(TrackData {
        id: TrackId::fake(id),
        album_id: None,
        title: title.into(),
        artist: "Artist".to_string(),
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
        track_number: 1,
        image_ref: None,
        local_artwork: None,
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        source_path: None,
        cue: None,
        source_format: None,
        comment: None,
        skip_count: None,
        bpm: None,
        relations: TrackRelations::default(),
    })
}

pub(crate) fn album(id: impl std::fmt::Display, title: impl Into<String>) -> Album {
    Album {
        id: AlbumId::fake(id),
        title: title.into(),
        artist: "Artist".to_string(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        favorite: false,
        color_seed: 1,
        image_ref: None,
        local_artwork: None,
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
        relations: AlbumRelations::default(),
    }
}

pub(crate) fn album_summary(album: Album, track_count: u32, duration_seconds: u32) -> AlbumSummary {
    let album = Arc::new(album);
    AlbumSummary {
        artwork: AlbumArtwork {
            album: Arc::clone(&album),
            representative_track: None,
        },
        album,
        track_count,
        duration_seconds,
    }
}

pub(crate) fn playlist(id: impl std::fmt::Display, name: impl Into<String>) -> Playlist {
    Playlist {
        id: PlaylistId::fake(id),
        name: name.into(),
        image_ref: None,
    }
}

pub(crate) fn playlist_summary(
    playlist: Playlist,
    track_count: u32,
    duration_seconds: u32,
) -> PlaylistSummary {
    PlaylistSummary {
        playlist: Arc::new(playlist),
        genres: Arc::from([]),
        representative_albums: Arc::from([]),
        track_count,
        duration_seconds,
    }
}

pub(crate) fn smart_playlist(id: impl std::fmt::Display, name: impl Into<String>) -> SmartPlaylist {
    SmartPlaylist {
        id: SmartPlaylistId::fake(id),
        name: name.into(),
        position: 0,
        builtin: None,
        definition: SmartPlaylistDefinition {
            match_all: Vec::new(),
            match_any: Vec::new(),
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        },
    }
}

pub(crate) fn smart_playlist_summary(
    smart_playlist: SmartPlaylist,
    track_count: u32,
    duration_seconds: u32,
) -> SmartPlaylistSummary {
    SmartPlaylistSummary {
        smart_playlist: Arc::new(smart_playlist),
        representative_albums: Arc::from([]),
        track_count,
        duration_seconds,
    }
}

pub(crate) fn loaded_source(
    source_id: SourceId,
    albums: Vec<Album>,
    tracks: Vec<Track>,
    playlists: Vec<PlaylistSnapshot>,
) -> Arc<Library> {
    source_fixture(source_id, albums, tracks, playlists).library
}

pub(crate) fn source_fixture(
    source_id: SourceId,
    albums: Vec<Album>,
    tracks: Vec<Track>,
    playlists: Vec<PlaylistSnapshot>,
) -> SourceFixture {
    source_fixture_with_artists(source_id, albums, tracks, Vec::new(), playlists)
}

pub(crate) fn source_fixture_with_artists(
    source_id: SourceId,
    albums: Vec<Album>,
    tracks: Vec<Track>,
    artists: Vec<Artist>,
    playlists: Vec<PlaylistSnapshot>,
) -> SourceFixture {
    let directory = tempfile::tempdir().expect("temporary UI Store");
    let libraries = Libraries::open(directory.path().join("library.db")).expect("open UI Store");
    let mut candidate = libraries
        .begin_source_candidate(CandidateHeader {
            source_id: source_id.clone(),
            input_version: 1,
            input_digest: [1; 32],
        })
        .expect("begin UI source candidate");
    for albums in albums.chunks(STORE_ROW_BATCH_LIMIT) {
        candidate
            .write(CandidateBatch::Albums(albums.to_vec()))
            .expect("write UI Albums");
    }
    for tracks in tracks.chunks(STORE_ROW_BATCH_LIMIT) {
        candidate
            .write(CandidateBatch::Tracks(tracks.to_vec()))
            .expect("write UI Tracks");
    }
    for artists in artists.chunks(STORE_ROW_BATCH_LIMIT) {
        candidate
            .write(CandidateBatch::Artists(artists.to_vec()))
            .expect("write UI Artists");
    }
    for playlists in playlists.chunks(STORE_ROW_BATCH_LIMIT) {
        candidate
            .write(CandidateBatch::Playlists(playlists.to_vec()))
            .expect("write UI Playlists");
    }
    let library = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept UI source candidate")
        .library;
    SourceFixture {
        library,
        _directory: directory,
    }
}

pub(crate) fn playlist_snapshot(
    playlist: Playlist,
    entries: impl IntoIterator<Item = (impl Into<String>, TrackId)>,
) -> PlaylistSnapshot {
    PlaylistSnapshot {
        playlist,
        entries: entries
            .into_iter()
            .map(|(occurrence_id, track_id)| PlaylistEntry {
                occurrence_id: occurrence_id.into(),
                track_id,
            })
            .collect(),
    }
}
