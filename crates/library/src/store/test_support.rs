use std::cell::RefCell;

pub(super) use crate::{
    Album, AlbumId, Artist, ArtistCredit, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind,
    ImageRef, MusicFolder, MusicFolderId, Playlist, PlaylistDetail, PlaylistEntry,
    PlaylistEntryKey, PlaylistId, PlaylistSnapshot, SourceEntityKind, SourceFeatureOwner, SourceId,
    Track, TrackId, TrackSort,
};

pub(super) use super::{
    LibrarySync, MusicFolderSnapshot, SourceLocalAccess, SourceObjectMapping, Store, StoreResult,
    StoredSource, SyncCommit, SyncCoverage,
};

thread_local! {
    static READ_STATEMENTS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

fn record_read_statement(event: rusqlite::trace::TraceEvent<'_>) {
    let rusqlite::trace::TraceEvent::Stmt(statement, sql) = event else {
        return;
    };
    let sql = statement.expanded_sql().unwrap_or_else(|| sql.to_string());
    let trimmed = sql.trim_start();
    if !trimmed.starts_with("SELECT") && !trimmed.starts_with("WITH") {
        return;
    }
    READ_STATEMENTS.with(|statements| {
        if let Some(statements) = statements.borrow_mut().as_mut() {
            statements.push(sql);
        }
    });
}

pub(super) fn trace_read_statements<T>(
    store: &Store,
    read: impl FnOnce() -> T,
) -> (T, Vec<String>) {
    READ_STATEMENTS.with(|statements| statements.replace(Some(Vec::new())));
    store.connection.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(record_read_statement),
    );
    let result = read();
    store
        .connection
        .trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, None);
    let statements = READ_STATEMENTS.with(|statements| {
        statements
            .replace(None)
            .expect("read statements should be traced")
    });
    (result, statements)
}

pub(super) fn explain_query_plan(store: &Store, sql: &str) -> Vec<String> {
    let mut statement = store
        .connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare query plan");
    statement
        .query_map([], |row| row.get::<_, String>(3))
        .expect("query plan")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect query plan")
}

pub(super) fn sqlite_sidecar_path(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    super::sources::sqlite_sidecar_path(path, suffix)
}

pub(super) fn synthesize_album_from_tracks(album_id: &AlbumId, tracks: &[Track]) -> Album {
    super::sources::synthesize_album_from_tracks(album_id, tracks)
}

pub(super) fn stored_source() -> StoredSource {
    stored_source_with_id("jellyfin:server:test")
}

pub(super) fn stored_source_with_id(source_id: &str) -> StoredSource {
    StoredSource {
        source_id: SourceId::new(source_id),
        kind: "jellyfin".to_string(),
        name: "Test Server".to_string(),
        provider_payload: r#"{"version":1,"base_url":"https://music.example","user_id":"user","username":"demo","trust_invalid_cert":false,"use_jellyfin_instant_mix":false}"#
            .to_string(),
    }
}

pub(super) struct StoreCase {
    pub(super) store: Store,
    pub(super) id: SourceId,
}

impl StoreCase {
    pub(super) fn open() -> Self {
        Self::with_source(stored_source())
    }

    pub(super) fn with_source_id(source_id: &str) -> Self {
        Self::with_source(stored_source_with_id(source_id))
    }

    pub(super) fn start_sync(&self, label: &str) -> i64 {
        self.store.begin_sync(&self.id).expect(label)
    }

    pub(super) fn commit_library(
        &self,
        generation: i64,
        observation: LibraryObservation,
        label: &str,
    ) -> SyncCommit {
        observation
            .commit(&self.store, &self.id, generation)
            .expect(label)
    }

    fn with_source(source: StoredSource) -> Self {
        let store = Store::open_memory().expect("open store");
        store.save_source(&source).expect("save source");
        Self {
            store,
            id: source.source_id,
        }
    }
}

#[derive(Default)]
pub(super) struct LibraryObservation {
    pub(super) albums: Vec<Album>,
    pub(super) tracks: Vec<Track>,
    pub(super) artists: Vec<Artist>,
    pub(super) album_artists: Vec<Artist>,
    pub(super) genres: Vec<Genre>,
    pub(super) music_folders: Vec<(MusicFolder, Vec<Track>)>,
    pub(super) playlists: Vec<PlaylistDetail>,
    pub(super) home_sections: Vec<HomeSection>,
}

impl LibraryObservation {
    pub(super) fn commit(
        self,
        store: &Store,
        source_id: &SourceId,
        generation: i64,
    ) -> StoreResult<SyncCommit> {
        let folders = self
            .music_folders
            .iter()
            .map(|(folder, _)| folder.clone())
            .collect::<Vec<_>>();
        let playlists = self
            .playlists
            .iter()
            .map(|detail| detail.playlist.clone())
            .collect::<Vec<_>>();
        let mappings = self.source_object_mappings(&folders, &playlists);
        let playlist_snapshots = self
            .playlists
            .into_iter()
            .map(|detail| PlaylistSnapshot {
                playlist: detail.playlist,
                entries: detail
                    .entries
                    .into_iter()
                    .map(|entry| PlaylistEntryKey {
                        entry_id: entry.entry_id,
                        track_id: entry.track.id,
                    })
                    .collect(),
            })
            .collect();
        let base_sync_input_revision = store.source_sync_input_revision(source_id)?;
        store.commit_library_sync(
            source_id,
            generation,
            base_sync_input_revision,
            LibrarySync {
                albums: self.albums,
                tracks: self.tracks,
                artists: self.artists,
                album_artists: self.album_artists,
                genres: self.genres,
                playlists: playlist_snapshots,
                home_sections: self.home_sections,
                mappings,
                coverage: SyncCoverage::All {
                    music_folders: self
                        .music_folders
                        .into_iter()
                        .map(|(folder, tracks)| MusicFolderSnapshot {
                            folder,
                            track_ids: tracks.into_iter().map(|track| track.id).collect(),
                        })
                        .collect(),
                },
                local_access: None,
            },
        )
    }

    fn source_object_mappings(
        &self,
        folders: &[MusicFolder],
        playlists: &[Playlist],
    ) -> Vec<SourceObjectMapping> {
        let mut mappings = Vec::new();
        for (kind, ids) in [
            (
                SourceEntityKind::Album,
                self.albums
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (
                SourceEntityKind::Track,
                self.tracks
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (
                SourceEntityKind::Artist,
                self.artists
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (
                SourceEntityKind::AlbumArtist,
                self.album_artists
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (
                SourceEntityKind::Genre,
                self.genres
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (
                SourceEntityKind::Playlist,
                playlists
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (
                SourceEntityKind::MusicFolder,
                folders
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
            ),
        ] {
            mappings.extend(ids.into_iter().map(|entity_id| SourceObjectMapping {
                source_object_id: entity_id.to_string(),
                entity_kind: kind,
                entity_id: entity_id.to_string(),
            }));
        }
        mappings
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
        representative_albums: Vec::new(),
    }
}

pub(super) fn genre(number: u32, image_ref: Option<ImageRef>) -> Genre {
    Genre {
        id: GenreId::fake(number),
        name: format!("Genre {number}"),
        album_count: 1,
        track_count: 2,
        duration_seconds: 360,
        image_ref,
        representative_albums: Vec::new(),
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
        image_ref,
        representative_albums: Vec::new(),
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
    LibraryObservation {
        albums: vec![album],
        tracks: vec![track],
        ..LibraryObservation::default()
    }
    .commit(store, source_id, generation)
    .expect("commit library");
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
        album_artwork: None,
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
