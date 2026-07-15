//! Compares one music source with Rufin's stored library and applies its changes.
//!
//! `sources` fetches provider data; `library` owns the stored items and queries.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use library::{
    LibrarySync, LocalAccessUpdate, MusicFolderSnapshot, PagedResponse, SourceEntityKind, SourceId,
    SourceObjectMapping, Store, StoreError, SyncCommit, SyncCoverage, TrackFolderMembership,
};
use sources::{
    LibraryChangeResolution, LibraryChangeResolver, LibraryObjectObservation, MusicFolderProvider,
    MusicSource, PageState, PagedRequest, PlaylistReader, SourceError, SourceObjectChanges,
    SourceObjectKeyProvider,
};
use thiserror::Error;

mod coordinator;
mod freshness;
mod local;
mod local_access;

pub use coordinator::{
    CancellationToken, CancelledRun, Finish, RequestKind, SourceSyncChanged, Start,
    SyncCoordinator, SyncPhase,
};
pub use freshness::Freshness;
pub use library::LibraryCommitted;
pub use local::{LocalObservation, acquire_local, commit_local};
pub use local_access::LocalAccessObservation;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LibrarySyncEvent {
    Committed(LibraryCommitted),
    SyncChanged(SourceSyncChanged),
}

const PAGE_SIZE: usize = 500;
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Collection {
    Albums,
    Tracks,
    MusicFolders,
    Artists,
    AlbumArtists,
    Genres,
    Playlists,
    HomeSections,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalScanStage {
    Walking,
    ReadingTags,
    BuildingLibrary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalScanProgress {
    pub stage: LocalScanStage,
    pub roots_walked: u64,
    pub directory_entries_visited: u64,
    pub audio_candidates: u64,
    pub processed_tracks: usize,
    pub total_tracks: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    LocalScan(LocalScanProgress),
    CollectionStarted(Collection),
    PageFetching {
        collection: Collection,
        fetched: usize,
        total: Option<usize>,
    },
    PageStaged {
        collection: Collection,
        fetched: usize,
    },
    Finalizing,
    Finished,
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("library sync was cancelled")]
    Cancelled,
    #[error("source changed while the library was being read")]
    Unstable,
    #[error("library sync state is unavailable: {0}")]
    Unavailable(&'static str),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Store(StoreError),
}

impl From<StoreError> for SyncError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub type SyncResult<T> = Result<T, SyncError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncOutcome {
    Committed(Box<SyncCommit>),
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChangeSyncOutcome {
    Committed(Box<SyncCommit>),
    NeedsFull,
    Ignored,
}

pub struct SyncAttempt<'a> {
    pub store: &'a Store,
    pub source_id: &'a SourceId,
    pub generation: i64,
    pub base_cache_revision: i64,
    pub cancellation: &'a CancellationToken,
    pub progress: &'a mut dyn FnMut(Progress),
}

/// Requests combine as a set, and All includes every object request
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ReconcileScope {
    #[default]
    None,
    Objects(SourceObjectChanges),
    All,
}

impl ReconcileScope {
    pub fn objects(changes: SourceObjectChanges) -> Self {
        if changes.is_empty() {
            Self::None
        } else {
            Self::Objects(changes)
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    pub fn merge(&mut self, other: Self) {
        match (&mut *self, other) {
            (Self::All, _) | (_, Self::None) => {}
            (scope, Self::All) => *scope = Self::All,
            (Self::None, scope) => *self = scope,
            (Self::Objects(current), Self::Objects(other)) => current.merge(other),
        }
    }
}

pub struct RemoteLibrary<'a> {
    pub core: &'a (dyn MusicSource + Send + Sync),
    pub music_folders: &'a (dyn MusicFolderProvider + Send + Sync),
    pub playlists: &'a (dyn PlaylistReader + Send + Sync),
    pub keys: &'a (dyn SourceObjectKeyProvider + Send + Sync),
}

pub async fn sync_remote_changes(
    attempt: &mut SyncAttempt<'_>,
    resolver: &(dyn LibraryChangeResolver + Send + Sync),
    changes: &SourceObjectChanges,
) -> SyncResult<ChangeSyncOutcome> {
    let cancellation = attempt.cancellation;
    let cancelled = &|| cancellation.is_cancelled();
    check_cancelled(cancelled)?;
    let mut known = Vec::new();
    for source_object_id in changes.iter() {
        known.extend(
            attempt
                .store
                .source_object_mappings(attempt.source_id, source_object_id)?,
        );
    }
    let resolution = await_source(cancelled, resolver.resolve_changes(changes, &known)).await?;
    let observation = match resolution {
        LibraryChangeResolution::Exact(observation) => observation,
        LibraryChangeResolution::Full => return Ok(ChangeSyncOutcome::NeedsFull),
        LibraryChangeResolution::Ignored => return Ok(ChangeSyncOutcome::Ignored),
    };
    let observation = *observation;
    if !bounded_observation_is_complete(changes, &known, &observation) {
        return Ok(ChangeSyncOutcome::NeedsFull);
    }

    let tombstones = known
        .into_iter()
        .filter(|mapping| {
            observation
                .missing_source_objects
                .contains(&mapping.source_object_id)
        })
        .collect::<Vec<_>>();
    let track_folders = observation
        .track_music_folders
        .into_iter()
        .map(|(track_id, folder_ids)| TrackFolderMembership {
            track_id,
            folder_ids,
        })
        .collect();
    let sync = LibrarySync {
        albums: observation.albums,
        tracks: observation.tracks,
        artists: observation.artists,
        album_artists: observation.album_artists,
        genres: observation.genres,
        playlists: observation.playlists,
        home_sections: observation.home_sections,
        mappings: observation.mappings,
        coverage: SyncCoverage::Finite {
            tombstones,
            track_folders,
        },
        local_access: None,
    };

    check_cancelled(cancelled)?;
    (attempt.progress)(Progress::Finalizing);
    if !cancellation.can_commit() {
        return Err(SyncError::Cancelled);
    }
    let commit = match attempt.store.commit_library_sync(
        attempt.source_id,
        attempt.generation,
        attempt.base_cache_revision,
        sync,
    ) {
        Ok(commit) => commit,
        Err(library::StoreError::NeedsFullSync) => return Ok(ChangeSyncOutcome::NeedsFull),
        Err(error) => return Err(error.into()),
    };
    (attempt.progress)(Progress::Finished);
    Ok(ChangeSyncOutcome::Committed(Box::new(commit)))
}

fn bounded_observation_is_complete(
    changes: &SourceObjectChanges,
    known: &[SourceObjectMapping],
    observation: &LibraryObjectObservation,
) -> bool {
    if observation.ignored_source_objects.iter().any(|ignored| {
        known
            .iter()
            .any(|mapping| mapping.source_object_id == *ignored)
    }) {
        return false;
    }
    let observed = observation
        .mappings
        .iter()
        .map(|mapping| mapping.source_object_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    changes.iter().all(|source_object_id| {
        observed.contains(source_object_id.as_str())
            || observation
                .missing_source_objects
                .contains(source_object_id)
                && known.iter().any(|mapping| {
                    mapping.source_object_id == *source_object_id
                        && matches!(
                            mapping.entity_kind,
                            SourceEntityKind::Track | SourceEntityKind::Playlist
                        )
                })
            || observation
                .ignored_source_objects
                .contains(source_object_id)
    })
}

pub async fn sync_remote(
    attempt: &mut SyncAttempt<'_>,
    source: RemoteLibrary<'_>,
    local_access: Option<&LocalAccessObservation>,
) -> SyncResult<SyncCommit> {
    let cancellation = attempt.cancellation;
    let cancelled = &|| cancellation.is_cancelled();
    check_cancelled(cancelled)?;
    let mut sync = LibrarySync {
        albums: Vec::new(),
        tracks: Vec::new(),
        artists: Vec::new(),
        album_artists: Vec::new(),
        genres: Vec::new(),
        playlists: Vec::new(),
        home_sections: Vec::new(),
        mappings: Vec::new(),
        coverage: SyncCoverage::All {
            music_folders: Vec::new(),
        },
        local_access: local_access.map(|local_access| LocalAccessUpdate {
            manifest: local_access.manifest().clone(),
            matches: Vec::new(),
        }),
    };
    enumerate_remote(
        &source,
        &mut sync,
        local_access,
        cancelled,
        &mut *attempt.progress,
    )
    .await?;

    (attempt.progress)(Progress::CollectionStarted(Collection::HomeSections));
    sync.home_sections = await_source(cancelled, source.core.home_sections()).await?;

    check_cancelled(cancelled)?;
    (attempt.progress)(Progress::Finalizing);
    if !cancellation.can_commit() {
        return Err(SyncError::Cancelled);
    }
    let commit = attempt.store.commit_library_sync(
        attempt.source_id,
        attempt.generation,
        attempt.base_cache_revision,
        sync,
    )?;
    (attempt.progress)(Progress::Finished);
    Ok(commit)
}

async fn enumerate_remote(
    source: &RemoteLibrary<'_>,
    sync: &mut LibrarySync,
    local_access: Option<&LocalAccessObservation>,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> SyncResult<()> {
    let mut seen = HashSet::new();
    let mut seen_folder_tracks = HashSet::new();
    progress(Progress::CollectionStarted(Collection::Albums));
    read_pages(
        Collection::Albums,
        cancelled,
        progress,
        true,
        |request| source.core.albums(request),
        |items| {
            record_entities(
                sync,
                &mut seen,
                source.keys,
                SourceEntityKind::Album,
                items.iter().map(|album| album.id.as_str()),
                |sync| sync.albums.extend_from_slice(items),
            )
        },
    )
    .await?;

    progress(Progress::CollectionStarted(Collection::Tracks));
    read_pages(
        Collection::Tracks,
        cancelled,
        progress,
        true,
        |request| source.core.tracks(request),
        |items| {
            record_entities(
                sync,
                &mut seen,
                source.keys,
                SourceEntityKind::Track,
                items.iter().map(|track| track.id.as_str()),
                |sync| {
                    sync.tracks.extend_from_slice(items);
                    if let Some(local_access) = local_access
                        && let Some(update) = sync.local_access.as_mut()
                    {
                        update.matches.extend(local_access.matches(items));
                    }
                },
            )
        },
    )
    .await?;

    progress(Progress::CollectionStarted(Collection::MusicFolders));
    let folders = await_source(cancelled, source.music_folders.music_folders()).await?;
    let mut folder_snapshots = Vec::new();
    record_entities(
        sync,
        &mut seen,
        source.keys,
        SourceEntityKind::MusicFolder,
        folders.iter().map(|folder| folder.id.as_str()),
        |_| {
            folder_snapshots.extend(folders.iter().cloned().map(|folder| MusicFolderSnapshot {
                folder,
                track_ids: Vec::new(),
            }));
        },
    )?;
    for (folder, snapshot) in folders.iter().zip(&mut folder_snapshots) {
        required_relation(
            read_pages(
                Collection::MusicFolders,
                cancelled,
                progress,
                false,
                |request| {
                    source
                        .music_folders
                        .tracks_in_music_folder(&folder.id, request)
                },
                |items| {
                    if items.iter().any(|track| {
                        !seen_folder_tracks.insert((folder.id.clone(), track.id.clone()))
                    }) {
                        return Err(SyncError::Unstable);
                    }
                    snapshot
                        .track_ids
                        .extend(items.iter().map(|track| track.id.clone()));
                    Ok(())
                },
            )
            .await,
        )?;
    }
    sync.coverage = SyncCoverage::All {
        music_folders: folder_snapshots,
    };

    for (collection, kind, album_artist) in [
        (Collection::Artists, SourceEntityKind::Artist, false),
        (
            Collection::AlbumArtists,
            SourceEntityKind::AlbumArtist,
            true,
        ),
    ] {
        progress(Progress::CollectionStarted(collection));
        read_pages(
            collection,
            cancelled,
            progress,
            false,
            |request| {
                if album_artist {
                    source.core.album_artists(request)
                } else {
                    source.core.artists(request)
                }
            },
            |items| {
                record_entities(
                    sync,
                    &mut seen,
                    source.keys,
                    kind,
                    items.iter().map(|artist| artist.id.as_str()),
                    |sync| {
                        if album_artist {
                            sync.album_artists.extend_from_slice(items);
                        } else {
                            sync.artists.extend_from_slice(items);
                        }
                    },
                )
            },
        )
        .await?;
    }

    progress(Progress::CollectionStarted(Collection::Genres));
    read_pages(
        Collection::Genres,
        cancelled,
        progress,
        false,
        |request| source.core.genres(request),
        |items| {
            record_entities(
                sync,
                &mut seen,
                source.keys,
                SourceEntityKind::Genre,
                items.iter().map(|genre| genre.id.as_str()),
                |sync| sync.genres.extend_from_slice(items),
            )
        },
    )
    .await?;

    progress(Progress::CollectionStarted(Collection::Playlists));
    let mut pages = PageState::default();
    loop {
        let page = await_source(
            cancelled,
            source.playlists.playlists(pages.request(PAGE_SIZE)),
        )
        .await?;
        let finished = pages
            .add(page.items.len(), (page.total > 0).then_some(page.total))
            .ok_or(SyncError::Unstable)?;
        record_entities(
            sync,
            &mut seen,
            source.keys,
            SourceEntityKind::Playlist,
            page.items.iter().map(|playlist| playlist.id.as_str()),
            |_| {},
        )?;
        for playlist in &page.items {
            let detail = required_relation(
                await_source(cancelled, source.playlists.playlist_detail(&playlist.id)).await,
            )?;
            sync.playlists.push(detail);
        }
        if finished {
            break;
        }
    }
    Ok(())
}

fn record_entities<'a>(
    sync: &mut LibrarySync,
    seen: &mut HashSet<(SourceEntityKind, String)>,
    keys: &(dyn SourceObjectKeyProvider + Send + Sync),
    kind: SourceEntityKind,
    ids: impl IntoIterator<Item = &'a str>,
    record_items: impl FnOnce(&mut LibrarySync),
) -> SyncResult<()> {
    let mappings = source_mappings(keys, kind, ids)?;
    if mappings
        .iter()
        .any(|mapping| !seen.insert((kind, mapping.source_object_id.clone())))
    {
        return Err(SyncError::Unstable);
    }
    record_items(sync);
    sync.mappings.extend(mappings);
    Ok(())
}

fn source_mappings<'a>(
    keys: &(dyn SourceObjectKeyProvider + Send + Sync),
    kind: SourceEntityKind,
    ids: impl IntoIterator<Item = &'a str>,
) -> SyncResult<Vec<SourceObjectMapping>> {
    ids.into_iter()
        .map(|entity_id| {
            Ok(SourceObjectMapping {
                source_object_id: keys.source_object_key(kind, entity_id)?,
                entity_kind: kind,
                entity_id: entity_id.to_string(),
            })
        })
        .collect()
}

async fn read_pages<T, Fetch, PageFuture, Observe>(
    collection: Collection,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
    report_progress: bool,
    mut fetch: Fetch,
    mut observe: Observe,
) -> SyncResult<()>
where
    Fetch: FnMut(PagedRequest) -> PageFuture,
    PageFuture: Future<Output = sources::SourceResult<PagedResponse<T>>>,
    Observe: FnMut(&[T]) -> SyncResult<()>,
{
    let mut pages = PageState::default();
    loop {
        let page = if report_progress {
            progress(Progress::PageFetching {
                collection,
                fetched: pages.fetched(),
                total: pages.total(),
            });
            await_source(cancelled, fetch(pages.request(PAGE_SIZE))).await?
        } else {
            await_source(cancelled, fetch(pages.request(PAGE_SIZE))).await?
        };
        let finished = pages
            .add(page.items.len(), (page.total > 0).then_some(page.total))
            .ok_or(SyncError::Unstable)?;
        observe(&page.items)?;
        if report_progress {
            progress(Progress::PageStaged {
                collection,
                fetched: pages.fetched(),
            });
        }
        if finished {
            return Ok(());
        }
    }
}

async fn await_source<T, F>(cancelled: &dyn Fn() -> bool, operation: F) -> SyncResult<T>
where
    F: Future<Output = sources::SourceResult<T>>,
{
    tokio::pin!(operation);
    loop {
        check_cancelled(cancelled)?;
        tokio::select! {
            result = &mut operation => return result.map_err(SyncError::from),
            _ = tokio::time::sleep(CANCELLATION_POLL_INTERVAL) => {}
        }
    }
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> SyncResult<()> {
    if cancelled() {
        Err(SyncError::Cancelled)
    } else {
        Ok(())
    }
}

fn required_relation<T>(result: SyncResult<T>) -> SyncResult<T> {
    result.map_err(|error| match error {
        SyncError::Source(SourceError::NotFound) => SyncError::Unstable,
        error => error,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use library::{
        Album, AlbumDetail, AlbumId, Artist, Genre, GenreDetail, GenreId, HomeSection,
        LocalFileFacts, LocalManifestEntry, MusicFolder, MusicFolderId, Playlist, PlaylistDetail,
        PlaylistId, SearchResults, Track, TrackId,
    };
    use library::{SourceLocalAccess, StoredSource};
    use sources::SourceIdentity;
    use sources::local::LocalManifestScan;

    use super::*;

    struct FixedChangeResolver {
        resolution: LibraryChangeResolution,
    }

    #[async_trait(?Send)]
    impl LibraryChangeResolver for FixedChangeResolver {
        async fn resolve_changes(
            &self,
            _changes: &SourceObjectChanges,
            _known: &[SourceObjectMapping],
        ) -> sources::SourceResult<LibraryChangeResolution> {
            Ok(self.resolution.clone())
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum AlbumPages {
        KnownTotal,
        UnknownTotal,
        UnknownTotalCapped,
        UnknownTotalRepeats,
        InconsistentTotal,
        LaterPageFailure,
        StopsEarly,
        ExceedsTotal,
    }

    struct AlbumSource {
        identity: SourceIdentity,
        albums: Vec<Album>,
        tracks: Vec<Track>,
        album_pages: AlbumPages,
        album_reads: AtomicUsize,
        track_reads: AtomicUsize,
        artist_reads: AtomicUsize,
        album_artist_reads: AtomicUsize,
        genre_reads: AtomicUsize,
        music_folder_reads: AtomicUsize,
        folder_track_reads: AtomicUsize,
        playlist_reads: AtomicUsize,
        playlist_detail_reads: AtomicUsize,
    }

    impl AlbumSource {
        fn new(source_id: &SourceId, album_pages: AlbumPages) -> Self {
            let albums = (0..502).map(album).collect::<Vec<_>>();
            Self {
                identity: SourceIdentity {
                    id: source_id.clone(),
                    kind: "test".to_string(),
                    name: "Test".to_string(),
                    base_url: "https://music.example".to_string(),
                },
                tracks: vec![track(&albums[0])],
                albums,
                album_pages,
                album_reads: AtomicUsize::new(0),
                track_reads: AtomicUsize::new(0),
                artist_reads: AtomicUsize::new(0),
                album_artist_reads: AtomicUsize::new(0),
                genre_reads: AtomicUsize::new(0),
                music_folder_reads: AtomicUsize::new(0),
                folder_track_reads: AtomicUsize::new(0),
                playlist_reads: AtomicUsize::new(0),
                playlist_detail_reads: AtomicUsize::new(0),
            }
        }

        fn page<T: Clone>(
            &self,
            items: &[T],
            request: PagedRequest,
            total: usize,
        ) -> PagedResponse<T> {
            PagedResponse::new(
                items
                    .iter()
                    .skip(request.offset)
                    .take(request.limit)
                    .cloned()
                    .collect(),
                total,
            )
        }

        fn read_count(counter: &AtomicUsize) -> usize {
            counter.load(Ordering::Relaxed)
        }
    }

    fn album(number: u32) -> Album {
        Album {
            id: AlbumId::new(format!("album-{number:03}")),
            title: format!("Album {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 0,
            duration_seconds: 0,
            favorite: false,
            color_seed: number,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        }
    }

    fn music_folder() -> MusicFolder {
        MusicFolder {
            id: MusicFolderId::new("folder-one"),
            name: "Folder One".to_string(),
        }
    }

    fn track(album: &Album) -> Track {
        Track {
            id: TrackId::new("track-one"),
            album_id: album.id.clone(),
            title: "First Motion".to_string(),
            artist: "Astral Kin".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: album.title.clone(),
            year: album.year,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 210,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            album_artwork: None,
            genres: Vec::new(),
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

    fn playlist() -> Playlist {
        Playlist {
            id: PlaylistId::new("playlist-one"),
            name: "Playlist One".to_string(),
            owner: None,
            track_count: 0,
            duration_seconds: 0,
            top_genres: Vec::new(),
            image_ref: None,
            representative_albums: Vec::new(),
        }
    }

    #[async_trait(?Send)]
    impl MusicSource for AlbumSource {
        fn identity(&self) -> &SourceIdentity {
            &self.identity
        }

        async fn home_sections(&self) -> sources::SourceResult<Vec<HomeSection>> {
            Ok(Vec::new())
        }

        async fn albums(
            &self,
            request: PagedRequest,
        ) -> sources::SourceResult<PagedResponse<Album>> {
            let read = self.album_reads.fetch_add(1, Ordering::Relaxed);
            let page = match self.album_pages {
                AlbumPages::KnownTotal => self.page(&self.albums, request, self.albums.len()),
                AlbumPages::UnknownTotal => self.page(&self.albums, request, 0),
                AlbumPages::UnknownTotalCapped => {
                    let capped = PagedRequest::new(request.offset, request.limit.min(200));
                    self.page(&self.albums, capped, 0)
                }
                AlbumPages::UnknownTotalRepeats => {
                    let repeated = PagedRequest::new(0, request.limit.min(200));
                    self.page(&self.albums, repeated, 0)
                }
                AlbumPages::InconsistentTotal => self.page(
                    &self.albums,
                    request,
                    self.albums.len().saturating_sub(usize::from(read > 0)),
                ),
                AlbumPages::LaterPageFailure if read > 0 => {
                    return Err(SourceError::Network("later page failed".to_string()));
                }
                AlbumPages::LaterPageFailure => self.page(&self.albums, request, self.albums.len()),
                AlbumPages::StopsEarly if read > 0 => {
                    PagedResponse::new(Vec::new(), self.albums.len())
                }
                AlbumPages::StopsEarly => self.page(&self.albums, request, self.albums.len()),
                AlbumPages::ExceedsTotal => self.page(&self.albums, request, self.albums.len() - 1),
            };
            Ok(page)
        }

        async fn album_detail(&self, _album_id: &AlbumId) -> sources::SourceResult<AlbumDetail> {
            Err(SourceError::NotFound)
        }

        async fn tracks(
            &self,
            request: PagedRequest,
        ) -> sources::SourceResult<PagedResponse<Track>> {
            self.track_reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.page(&self.tracks, request, self.tracks.len()))
        }

        async fn artists(
            &self,
            request: PagedRequest,
        ) -> sources::SourceResult<PagedResponse<Artist>> {
            self.artist_reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.page(&[], request, 0))
        }

        async fn album_artists(
            &self,
            request: PagedRequest,
        ) -> sources::SourceResult<PagedResponse<Artist>> {
            self.album_artist_reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.page(&[], request, 0))
        }

        async fn genres(
            &self,
            request: PagedRequest,
        ) -> sources::SourceResult<PagedResponse<Genre>> {
            self.genre_reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.page(&[], request, 0))
        }

        async fn genre_detail(&self, _genre_id: &GenreId) -> sources::SourceResult<GenreDetail> {
            Err(SourceError::NotFound)
        }

        async fn track(&self, _track_id: &TrackId) -> sources::SourceResult<Track> {
            Err(SourceError::NotFound)
        }

        async fn search(&self, _query: &str) -> sources::SourceResult<SearchResults> {
            Ok(SearchResults::default())
        }
    }

    #[async_trait(?Send)]
    impl MusicFolderProvider for AlbumSource {
        async fn music_folders(&self) -> sources::SourceResult<Vec<MusicFolder>> {
            self.music_folder_reads.fetch_add(1, Ordering::Relaxed);
            Ok(vec![music_folder()])
        }

        async fn tracks_in_music_folder(
            &self,
            _folder_id: &MusicFolderId,
            request: PagedRequest,
        ) -> sources::SourceResult<PagedResponse<Track>> {
            self.folder_track_reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.page(&[], request, 0))
        }
    }

    #[async_trait(?Send)]
    impl PlaylistReader for AlbumSource {
        async fn playlists(
            &self,
            request: PagedRequest,
        ) -> sources::SourceResult<PagedResponse<Playlist>> {
            self.playlist_reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.page(&[playlist()], request, 1))
        }

        async fn playlist_detail(
            &self,
            playlist_id: &PlaylistId,
        ) -> sources::SourceResult<PlaylistDetail> {
            self.playlist_detail_reads.fetch_add(1, Ordering::Relaxed);
            Ok(PlaylistDetail {
                playlist: Playlist {
                    id: playlist_id.clone(),
                    ..playlist()
                },
                tracks: Vec::new(),
                entries: Vec::new(),
            })
        }
    }

    impl SourceObjectKeyProvider for AlbumSource {
        fn source_object_key(
            &self,
            _entity_kind: SourceEntityKind,
            entity_id: &str,
        ) -> sources::SourceResult<String> {
            Ok(entity_id.to_string())
        }
    }

    fn save_source(store: &Store, source: &AlbumSource) {
        store
            .save_source(&StoredSource {
                source_id: source.identity.id.clone(),
                kind: source.identity.kind.clone(),
                name: source.identity.name.clone(),
                provider_payload: "{}".to_string(),
            })
            .expect("save source");
    }

    fn save_empty_source(store: &Store, source_id: &SourceId) {
        store
            .save_source(&StoredSource {
                source_id: source_id.clone(),
                kind: "test".to_string(),
                name: "Test".to_string(),
                provider_payload: "{}".to_string(),
            })
            .expect("save source");
    }

    async fn run_sync(store: &Store, source_id: &SourceId, source: &AlbumSource) -> SyncResult<()> {
        run_sync_with_local_access(store, source_id, source, None)
            .await
            .map(|_| ())
    }

    async fn run_sync_with_local_access(
        store: &Store,
        source_id: &SourceId,
        source: &AlbumSource,
        local_access: Option<&LocalAccessObservation>,
    ) -> SyncResult<SyncCommit> {
        let cancellation = CancellationToken::new();
        let mut progress = |_| {};
        run_sync_attempt(
            store,
            source_id,
            source,
            local_access,
            &cancellation,
            &mut progress,
        )
        .await
    }

    async fn run_sync_attempt(
        store: &Store,
        source_id: &SourceId,
        source: &AlbumSource,
        local_access: Option<&LocalAccessObservation>,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(Progress),
    ) -> SyncResult<SyncCommit> {
        let generation = store.begin_sync(source_id)?;
        let base_cache_revision = store.source_cache_revision(source_id)?;
        let mut attempt = SyncAttempt {
            store,
            source_id,
            generation,
            base_cache_revision,
            cancellation,
            progress,
        };
        sync_remote(
            &mut attempt,
            RemoteLibrary {
                core: source,
                music_folders: source,
                playlists: source,
                keys: source,
            },
            local_access,
        )
        .await
    }

    #[tokio::test]
    async fn ignored_change_does_not_open_a_cache_commit() {
        let source_id = SourceId::new("test:ignored-change");
        let store = Store::open_memory().expect("open Store");
        save_empty_source(&store, &source_id);
        let generation = store.begin_sync(&source_id).expect("begin sync");
        let revision = store
            .source_cache_revision(&source_id)
            .expect("cache revision");
        let cancellation = CancellationToken::new();
        let mut progress = Vec::new();
        let outcome = {
            let mut report = |event| progress.push(event);
            let mut attempt = SyncAttempt {
                store: &store,
                source_id: &source_id,
                generation,
                base_cache_revision: revision,
                cancellation: &cancellation,
                progress: &mut report,
            };
            sync_remote_changes(
                &mut attempt,
                &FixedChangeResolver {
                    resolution: LibraryChangeResolution::Ignored,
                },
                &SourceObjectChanges::new(["movie-one".to_string()]),
            )
            .await
            .expect("ignore unrelated change")
        };

        assert_eq!(outcome, ChangeSyncOutcome::Ignored);
        assert!(progress.is_empty());
        assert_eq!(
            store
                .source_cache_revision(&source_id)
                .expect("unchanged cache revision"),
            revision
        );
        store
            .finish_sync_without_commit(&source_id, generation)
            .expect("finish test sync");
    }

    #[tokio::test]
    async fn full_change_resolution_keeps_the_full_fallback() {
        let source_id = SourceId::new("test:full-change-fallback");
        let store = Store::open_memory().expect("open Store");
        save_empty_source(&store, &source_id);
        let generation = store.begin_sync(&source_id).expect("begin sync");
        let revision = store
            .source_cache_revision(&source_id)
            .expect("cache revision");
        let cancellation = CancellationToken::new();
        let mut progress = |_| {};
        let mut attempt = SyncAttempt {
            store: &store,
            source_id: &source_id,
            generation,
            base_cache_revision: revision,
            cancellation: &cancellation,
            progress: &mut progress,
        };

        let outcome = sync_remote_changes(
            &mut attempt,
            &FixedChangeResolver {
                resolution: LibraryChangeResolution::Full,
            },
            &SourceObjectChanges::new(["album-one".to_string()]),
        )
        .await
        .expect("request full fallback");

        assert_eq!(outcome, ChangeSyncOutcome::NeedsFull);
        assert_eq!(
            store
                .source_cache_revision(&source_id)
                .expect("unchanged cache revision"),
            revision
        );
        store
            .finish_sync_without_commit(&source_id, generation)
            .expect("finish test sync");
    }

    #[tokio::test]
    async fn remote_collections_are_enumerated_once() {
        for (pages, expected_album_reads) in
            [(AlbumPages::KnownTotal, 2), (AlbumPages::UnknownTotal, 3)]
        {
            let source_id = SourceId::new(format!("test:single-pass:{pages:?}"));
            let store = Store::open_memory().expect("open Store");
            let source = AlbumSource::new(&source_id, pages);
            save_source(&store, &source);

            run_sync(&store, &source_id, &source)
                .await
                .expect("sync stable source");

            assert_eq!(
                AlbumSource::read_count(&source.album_reads),
                expected_album_reads,
                "{pages:?}"
            );
            assert_eq!(AlbumSource::read_count(&source.track_reads), 1, "{pages:?}");
            assert_eq!(
                AlbumSource::read_count(&source.artist_reads),
                1,
                "{pages:?}"
            );
            assert_eq!(
                AlbumSource::read_count(&source.album_artist_reads),
                1,
                "{pages:?}"
            );
            assert_eq!(AlbumSource::read_count(&source.genre_reads), 1, "{pages:?}");
            assert_eq!(
                AlbumSource::read_count(&source.music_folder_reads),
                1,
                "{pages:?}"
            );
            assert_eq!(
                AlbumSource::read_count(&source.folder_track_reads),
                1,
                "{pages:?}"
            );
            assert_eq!(
                AlbumSource::read_count(&source.playlist_reads),
                1,
                "{pages:?}"
            );
            assert_eq!(
                AlbumSource::read_count(&source.playlist_detail_reads),
                1,
                "{pages:?}"
            );
        }
    }

    #[tokio::test]
    async fn capped_unknown_total_reads_until_empty_and_reports_pagination() {
        let source_id = SourceId::new("test:capped-unknown-total");
        let store = Store::open_memory().expect("open Store");
        let source = AlbumSource::new(&source_id, AlbumPages::UnknownTotalCapped);
        save_source(&store, &source);
        let cancellation = CancellationToken::new();
        let mut progress = Vec::new();
        let mut report = |event| progress.push(event);

        run_sync_attempt(
            &store,
            &source_id,
            &source,
            None,
            &cancellation,
            &mut report,
        )
        .await
        .expect("sync capped source");

        assert_eq!(AlbumSource::read_count(&source.album_reads), 4);
        assert_eq!(
            store
                .load_albums(&source_id, 0, 600)
                .expect("complete albums")
                .items
                .len(),
            502
        );
        let album_pages = progress
            .iter()
            .filter_map(|event| match event {
                Progress::PageStaged {
                    collection: Collection::Albums,
                    fetched,
                } => Some(*fetched),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(album_pages, vec![200, 400, 502, 502]);
    }

    #[tokio::test]
    async fn pagination_anomalies_keep_the_last_cache() {
        let source_id = SourceId::new("test:page-invariants");
        let store = Store::open_memory().expect("open Store");
        let stable = AlbumSource::new(&source_id, AlbumPages::KnownTotal);
        save_source(&store, &stable);
        run_sync(&store, &source_id, &stable)
            .await
            .expect("initial sync");
        let revision = store
            .source_cache_revision(&source_id)
            .expect("initial revision");
        let cached = store
            .load_albums(&source_id, 0, 600)
            .expect("initial albums");

        for pages in [
            AlbumPages::InconsistentTotal,
            AlbumPages::UnknownTotalRepeats,
            AlbumPages::StopsEarly,
            AlbumPages::ExceedsTotal,
        ] {
            let source = AlbumSource::new(&source_id, pages);
            let result = run_sync(&store, &source_id, &source).await;

            assert!(matches!(result, Err(SyncError::Unstable)), "{pages:?}");
            assert_eq!(
                store
                    .source_cache_revision(&source_id)
                    .expect("revision after rejected sync"),
                revision,
                "{pages:?}"
            );
            assert_eq!(
                store
                    .load_albums(&source_id, 0, 600)
                    .expect("albums after rejected sync"),
                cached,
                "{pages:?}"
            );
        }
    }

    #[tokio::test]
    async fn later_page_failure_keeps_the_last_cache() {
        let source_id = SourceId::new("test:later-page-failure");
        let store = Store::open_memory().expect("open Store");
        let stable = AlbumSource::new(&source_id, AlbumPages::KnownTotal);
        save_source(&store, &stable);
        run_sync(&store, &source_id, &stable)
            .await
            .expect("initial sync");
        let revision = store
            .source_cache_revision(&source_id)
            .expect("initial revision");
        let cached = store
            .load_albums(&source_id, 0, 600)
            .expect("initial albums");
        let failing = AlbumSource::new(&source_id, AlbumPages::LaterPageFailure);

        let result = run_sync(&store, &source_id, &failing).await;

        assert!(matches!(
            result,
            Err(SyncError::Source(SourceError::Network(_)))
        ));
        assert_eq!(AlbumSource::read_count(&failing.album_reads), 2);
        assert_eq!(
            store
                .source_cache_revision(&source_id)
                .expect("revision after failed sync"),
            revision
        );
        assert_eq!(
            store
                .load_albums(&source_id, 0, 600)
                .expect("albums after failed sync"),
            cached
        );
    }

    #[tokio::test]
    async fn cancellation_at_finalization_keeps_the_last_cache() {
        let source_id = SourceId::new("test:cancel-finalization");
        let store = Store::open_memory().expect("open Store");
        let stable = AlbumSource::new(&source_id, AlbumPages::KnownTotal);
        save_source(&store, &stable);
        run_sync(&store, &source_id, &stable)
            .await
            .expect("initial sync");
        let revision = store
            .source_cache_revision(&source_id)
            .expect("initial revision");
        let cached = store
            .load_albums(&source_id, 0, 600)
            .expect("initial albums");
        let mut changed = AlbumSource::new(&source_id, AlbumPages::KnownTotal);
        changed.albums[0].title = "Uncommitted Album".to_string();
        let cancellation = CancellationToken::new();
        let mut progress = |event| {
            if matches!(event, Progress::Finalizing) {
                cancellation.cancel();
            }
        };

        let result = run_sync_attempt(
            &store,
            &source_id,
            &changed,
            None,
            &cancellation,
            &mut progress,
        )
        .await;

        assert!(matches!(result, Err(SyncError::Cancelled)));
        assert_eq!(
            store
                .source_cache_revision(&source_id)
                .expect("revision after cancellation"),
            revision
        );
        assert_eq!(
            store
                .load_albums(&source_id, 0, 600)
                .expect("albums after cancellation"),
            cached
        );
    }

    #[tokio::test]
    async fn missing_local_access_observation_does_not_veto_remote_sync() {
        let source_id = SourceId::new("test:local-access-failure");
        let stable = AlbumSource::new(&source_id, AlbumPages::KnownTotal);
        let store = Store::open_memory().expect("open Store");
        save_source(&store, &stable);
        let missing_root =
            std::env::temp_dir().join(format!("rufin-missing-local-access-{}", std::process::id()));
        store
            .save_source_local_access(&SourceLocalAccess {
                source_id: source_id.clone(),
                root_path: missing_root.to_string_lossy().into_owned(),
                path_replace_from: None,
                path_replace_to: None,
            })
            .expect("save local access");
        let local_path = PathBuf::from("/music/first.flac");
        let mut local_track = stable.tracks[0].clone();
        local_track.id = TrackId::new("local-track-one");
        local_track.local_path = Some(local_path.to_string_lossy().into_owned());
        let manifest_entry = LocalManifestEntry {
            facts: LocalFileFacts {
                path: local_path.clone(),
                root_path: PathBuf::from("/music"),
                relative_path: "first.flac".to_string(),
                file_size: 1,
                mtime_seconds: 2,
                mtime_nanos: 3,
                inode: None,
                device: None,
            },
            track: local_track,
            album_artist: "Astral Kin".to_string(),
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
            cover: None,
            metadata_hash: "metadata".to_string(),
            search_hash: "search".to_string(),
        };
        let observation = LocalAccessObservation::from_manifest_scan(LocalManifestScan {
            entries: vec![manifest_entry.clone()],
            changed_manifest_paths: vec![local_path],
            ..LocalManifestScan::default()
        });
        run_sync_with_local_access(&store, &source_id, &stable, Some(&observation))
            .await
            .expect("initial sync");
        let revision = store
            .source_cache_revision(&source_id)
            .expect("initial revision");
        let prior_match = store
            .track_local_match_path(&source_id, &stable.tracks[0].id)
            .expect("load initial match");
        assert_eq!(prior_match.as_deref(), Some("/music/first.flac"));
        let mut changed = AlbumSource::new(&source_id, AlbumPages::KnownTotal);
        changed.albums[0].title = "Changed Album".to_string();

        let commit = run_sync_with_local_access(&store, &source_id, &changed, None)
            .await
            .expect("sync remote library without local observation");

        assert_eq!(commit.cache_revision, revision + 1);
        let changed_album = store
            .load_albums(&source_id, 0, 600)
            .expect("albums after sync")
            .items
            .into_iter()
            .find(|album| album.id == changed.albums[0].id)
            .expect("changed album");
        assert_eq!(changed_album.title, "Changed Album");
        assert_eq!(
            store
                .track_local_match_path(&source_id, &stable.tracks[0].id)
                .expect("load retained match"),
            prior_match
        );
        assert_eq!(
            store
                .load_local_manifest(&source_id)
                .expect("load retained manifest"),
            vec![manifest_entry]
        );
    }
}
