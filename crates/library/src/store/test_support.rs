pub(super) use domain::{
    Album, AlbumId, Artist, ArtistCredit, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind,
    ImageRef, LibraryField, LyricLine, Lyrics, LyricsSource, MusicFolder, MusicFolderId, Playlist,
    PlaylistEntry, PlaylistId, QueueEngine, SourceFeatureOwner, SourceId, SourceIdentity, Track,
    TrackId,
};

pub(super) use super::{
    CoverCacheEntry, SavedSource, SourceLocalAccess, Store, image_cache_key, lyrics_cache_key,
};

pub(super) fn sqlite_sidecar_path(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    super::sources::sqlite_sidecar_path(path, suffix)
}

pub(super) fn synthesize_album_from_tracks(album_id: &AlbumId, tracks: &[Track]) -> Album {
    super::sources::synthesize_album_from_tracks(album_id, tracks)
}

pub(super) fn saved_source() -> SavedSource {
    saved_source_with_id("jellyfin:server:test")
}

pub(super) fn saved_source_with_id(source_id: &str) -> SavedSource {
    SavedSource {
        source: SourceIdentity {
            id: SourceId::new(source_id),
            kind: "jellyfin".to_string(),
            name: "Test Server".to_string(),
            base_url: "https://music.example".to_string(),
        },
        user_id: "user".to_string(),
        username: "demo".to_string(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    }
}

pub(super) struct StoreCase {
    pub(super) store: Store,
    pub(super) id: SourceId,
}

impl StoreCase {
    pub(super) fn open() -> Self {
        Self::with_server(saved_source())
    }

    pub(super) fn with_source_id(source_id: &str) -> Self {
        Self::with_server(saved_source_with_id(source_id))
    }

    pub(super) fn start_sync(&self, label: &str) -> i64 {
        self.store.begin_sync(&self.id).expect(label)
    }

    pub(super) fn finish_sync(&self, generation: i64, label: &str) {
        self.store.complete_sync(&self.id, generation).expect(label);
    }

    fn with_server(saved: SavedSource) -> Self {
        let store = Store::open_memory().expect("open store");
        store.save_source(&saved).expect("save server");
        Self {
            store,
            id: saved.source.id,
        }
    }
}

impl std::ops::Deref for StoreCase {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

pub(super) fn album(number: u32) -> Album {
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
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    }
}

pub(super) fn album_with_image(number: u32) -> Album {
    Album {
        image_ref: Some(image_ref(
            format!("album-{number}"),
            format!("album-tag-{number}"),
        )),
        genres: vec!["Dream Pop".to_string()],
        ..album(number)
    }
}

pub(super) fn credit(id: ArtistId, name: &str) -> ArtistCredit {
    ArtistCredit {
        id,
        name: name.to_string(),
        musicbrainz_artist_id: None,
    }
}

pub(super) fn artist(number: u32, image_ref: Option<ImageRef>) -> Artist {
    Artist {
        id: ArtistId::fake(number),
        name: format!("Artist {number}"),
        album_count: 1,
        track_count: 2,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref,
    }
}

pub(super) fn genre(number: u32, image_ref: Option<ImageRef>) -> Genre {
    Genre {
        id: GenreId::fake(number),
        name: format!("Genre {number}"),
        album_count: 1,
        track_count: 2,
        duration_seconds: 360,
        image_refs: Vec::new(),
        image_ref,
    }
}

pub(super) fn playlist(number: u32, image_ref: Option<ImageRef>) -> Playlist {
    Playlist {
        id: PlaylistId::fake(number),
        name: format!("Playlist {number}"),
        owner: Some(SourceFeatureOwner::Native),
        track_count: 2,
        duration_seconds: 360,
        top_genres: Vec::new(),
        image_refs: Vec::new(),
        image_ref,
    }
}

pub(super) fn image_ref(item_id: impl Into<String>, tag: impl Into<String>) -> ImageRef {
    ImageRef::new(item_id, Some(tag.into()))
}

pub(super) fn index_exists(store: &Store, table: &str, index: &str) -> bool {
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

pub(super) fn seed_cached_library(store: &Store, source_id: &SourceId) {
    let generation = store.begin_sync(source_id).expect("begin sync");
    let album = album(1);
    let track = track(1, &album);
    store
        .upsert_albums(source_id, std::slice::from_ref(&album), generation)
        .expect("upsert albums");
    store
        .upsert_tracks(source_id, std::slice::from_ref(&track), generation)
        .expect("upsert tracks");
    store
        .complete_sync(source_id, generation)
        .expect("complete sync");
}

pub(super) fn cover_entry(source_id: &SourceId) -> CoverCacheEntry {
    CoverCacheEntry {
        source_id: source_id.clone(),
        item_id: "album-one".to_string(),
        image_tag: "tag-one".to_string(),
        size: 256,
        path: "/tmp/rufin-cover.jpg".to_string(),
    }
}

pub(super) fn track(number: u32, album: &Album) -> Track {
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
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
        bpm: None,
        moods: Vec::new(),
    }
}
