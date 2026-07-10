use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use domain::{
    AppSettings, GeneratedTrackSeed, GeneratedTrackSeedKind, HomeSection, HomeSectionKind,
    PlayedFilter, RandomTrackRequest, SourceFeatureOwner, SourceId, SourceIdentity,
    SourcePlaylistOperation, Track, TrackId,
};
use library::{SavedSource, SourceLocalAccess, Store, StoreResult};
use source::{
    FavoriteMutator, FolderBrowser, GeneratedTrackProvider, ImageProvider, LyricsProvider,
    MusicFolderProvider, MusicSource, PlaybackReporter, PlaylistCreator, PlaylistDeleter,
    PlaylistEntryMover, PlaylistEntryRemover, PlaylistReader, PlaylistRenamer, PlaylistTrackAdder,
    RandomTrackProvider, RecentAlbumProvider, RecentTrackProvider, StreamResolver, TrackChangeFeed,
};
use tokio::runtime::Runtime;
use tracing::info;

use crate::controller::{
    CancellationToken, SourceSyncReadiness, StoreHandle, SyncContext, SyncJobOutcome,
    SyncProgressReporter,
};

pub(crate) type LibraryCore = Arc<dyn MusicSource + Send + Sync>;
pub(crate) type Streams = Arc<dyn StreamResolver + Send + Sync>;
pub(crate) type Images = Arc<dyn ImageProvider + Send + Sync>;
pub(crate) type Favorites = Arc<dyn FavoriteMutator + Send + Sync>;
pub(crate) type PlaylistCreation = Arc<dyn PlaylistCreator + Send + Sync>;
pub(crate) type PlaylistReads = Arc<dyn PlaylistReader + Send + Sync>;
pub(crate) type PlaylistRenames = Arc<dyn PlaylistRenamer + Send + Sync>;
pub(crate) type PlaylistDeletes = Arc<dyn PlaylistDeleter + Send + Sync>;
pub(crate) type PlaylistTrackAdds = Arc<dyn PlaylistTrackAdder + Send + Sync>;
pub(crate) type PlaylistEntryRemovals = Arc<dyn PlaylistEntryRemover + Send + Sync>;
pub(crate) type PlaylistEntryMoves = Arc<dyn PlaylistEntryMover + Send + Sync>;
pub(crate) type RandomTracks = Arc<dyn RandomTrackProvider + Send + Sync>;
pub(crate) type GeneratedTracks = Arc<dyn GeneratedTrackProvider + Send + Sync>;
pub(crate) type MusicFolders = Arc<dyn MusicFolderProvider + Send + Sync>;
pub(crate) type Folders = Arc<dyn FolderBrowser + Send + Sync>;
pub(crate) type NativeLyrics = Arc<dyn LyricsProvider + Send + Sync>;
pub(crate) type NativePlaybackReporting = Arc<dyn PlaybackReporter + Send + Sync>;
pub(crate) type RecentTracks = Arc<dyn RecentTrackProvider + Send + Sync>;
pub(crate) type RecentAlbums = Arc<dyn RecentAlbumProvider + Send + Sync>;
pub(crate) type TrackChanges = Arc<dyn TrackChangeFeed + Send + Sync>;
pub(crate) type GeneratedTrackExecutor = Arc<
    dyn Fn(
            &StoreHandle,
            &Runtime,
            &SavedSource,
            &AppSettings,
            GeneratedTrackSeed,
            usize,
        ) -> Result<Vec<Track>, String>
        + Send
        + Sync,
>;
pub(crate) type FullIngest = Arc<
    dyn Fn(
            &SyncContext,
            i64,
            SyncProgressReporter,
            &CancellationToken,
        ) -> Result<SyncJobOutcome, String>
        + Send
        + Sync,
>;
pub(crate) type HomeSectionLoader = Arc<
    dyn Fn(&StoreHandle, &Runtime, HomeSectionKind) -> Result<HomeSection, String> + Send + Sync,
>;
pub(crate) type ReadinessEvaluator =
    Arc<dyn Fn(&StoreHandle, bool) -> Result<SourceSyncReadiness, String> + Send + Sync>;
pub(crate) type InitialCoverCacheRequirement = Arc<dyn Fn(&StoreHandle) -> bool + Send + Sync>;
pub(crate) type CachedReconciliation =
    Arc<dyn Fn(SyncContext, SavedSource, Arc<ActiveSource>) + Send + Sync>;
pub(crate) type FreshnessWatcherFactory = Arc<
    dyn Fn(SyncContext, SavedSource, Arc<ActiveSource>) -> Option<Box<dyn FreshnessWatcher>>
        + Send
        + Sync,
>;
pub(crate) type AudioFileLookup =
    Arc<dyn Fn(&Store, &SourceId, &TrackId) -> StoreResult<Option<PathBuf>> + Send + Sync>;
pub(crate) type ActiveSourceSlot = Arc<RwLock<Option<Arc<ActiveSource>>>>;
#[derive(Clone)]
pub(crate) enum OperationOwner<T> {
    Native(T),
    Store,
}

impl<T> OperationOwner<T> {
    pub(crate) const fn owner(&self) -> SourceFeatureOwner {
        match self {
            Self::Native(_) => SourceFeatureOwner::Native,
            Self::Store => SourceFeatureOwner::Store,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct PlaylistRowOperations {
    pub(crate) rename: Option<PlaylistRenames>,
    pub(crate) delete: Option<PlaylistDeletes>,
    pub(crate) add_tracks: Option<PlaylistMutationOperation<PlaylistTrackAdds>>,
    pub(crate) remove_entries: Option<PlaylistMutationOperation<PlaylistEntryRemovals>>,
    pub(crate) move_entry: Option<PlaylistMutationOperation<PlaylistEntryMoves>>,
}

/// Each playlist edit also knows how to reload its final state from the source
#[derive(Clone)]
pub(crate) struct PlaylistMutationOperation<T> {
    pub(crate) executor: T,
    pub(crate) readback: PlaylistReads,
}

impl PlaylistRowOperations {
    pub(crate) fn supports(
        &self,
        operation: SourcePlaylistOperation,
        owner: SourceFeatureOwner,
    ) -> bool {
        match owner {
            SourceFeatureOwner::Store => true,
            SourceFeatureOwner::Native => match operation {
                SourcePlaylistOperation::Rename => self.rename.is_some(),
                SourcePlaylistOperation::Delete => self.delete.is_some(),
                SourcePlaylistOperation::AddTracks => self.add_tracks.is_some(),
                SourcePlaylistOperation::RemoveEntries => self.remove_entries.is_some(),
                SourcePlaylistOperation::ReorderEntries => self.move_entry.is_some(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RandomTrackDomain {
    played_filters: &'static [PlayedFilter],
    year_range: bool,
    genre: bool,
}

impl RandomTrackDomain {
    pub(crate) const fn new(
        played_filters: &'static [PlayedFilter],
        year_range: bool,
        genre: bool,
    ) -> Self {
        Self {
            played_filters,
            year_range,
            genre,
        }
    }

    pub(crate) const fn played_filters(self) -> &'static [PlayedFilter] {
        self.played_filters
    }

    pub(crate) const fn allows_year_range(self) -> bool {
        self.year_range
    }

    pub(crate) const fn allows_genre(self) -> bool {
        self.genre
    }

    pub(crate) fn validate(self, request: &RandomTrackRequest) -> Result<(), &'static str> {
        if !self.played_filters.contains(&request.played_filter) {
            return Err("the selected play-history filter is not available for this source");
        }
        if !self.year_range && (request.min_year.is_some() || request.max_year.is_some()) {
            return Err("year filtering is not available for this source");
        }
        if !self.genre && (request.genre_id.is_some() || request.genre_name.is_some()) {
            return Err("genre filtering is not available for this source");
        }
        if request
            .min_year
            .zip(request.max_year)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            return Err("minimum year cannot be greater than maximum year");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct RandomTrackOperation {
    pub(crate) domain: RandomTrackDomain,
    pub(crate) executor: RandomTracks,
}

#[derive(Clone)]
pub(crate) struct ManualRadioOperation {
    pub(crate) seed_domain: &'static [GeneratedTrackSeedKind],
    pub(crate) executor: GeneratedTrackExecutor,
}

impl ManualRadioOperation {
    pub(crate) fn accepts(&self, seed: &GeneratedTrackSeed) -> bool {
        self.seed_domain.contains(&seed.kind())
    }
}

pub(crate) type AutoDjFallbackExecutor = Arc<
    dyn Fn(
            &StoreHandle,
            &Runtime,
            &SavedSource,
            &AppSettings,
            Option<String>,
            usize,
            &TrackId,
        ) -> Result<Vec<Track>, String>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub(crate) struct AutoDjCandidateOperation {
    pub(crate) generated: GeneratedTrackExecutor,
    pub(crate) fallback: AutoDjFallbackExecutor,
}

#[derive(Clone)]
pub(crate) struct FreshnessOperations {
    pub(crate) available: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) reconcile_cached: CachedReconciliation,
    pub(crate) start_watcher: FreshnessWatcherFactory,
}

pub(crate) trait FreshnessWatcher: Send {
    fn active(&self) -> &Arc<ActiveSource>;
}

/// Everything the selected source can do, including which inputs each action accepts
pub(crate) struct ActiveSource {
    pub(crate) identity: SourceIdentity,
    pub(crate) ingest: FullIngest,
    pub(crate) readiness: ReadinessEvaluator,
    pub(crate) initial_cover_cache_required: InitialCoverCacheRequirement,
    pub(crate) freshness: Option<FreshnessOperations>,
    pub(crate) home_section: HomeSectionLoader,
    pub(crate) playback_file: AudioFileLookup,
    pub(crate) sidecar_file: AudioFileLookup,
    pub(crate) streams: Streams,
    pub(crate) images: Images,
    pub(crate) favorites: OperationOwner<Favorites>,
    pub(crate) playlist_creation: OperationOwner<PlaylistCreation>,
    pub(crate) playlist_rows: PlaylistRowOperations,
    pub(crate) random_tracks: RandomTrackOperation,
    pub(crate) manual_radio: ManualRadioOperation,
    pub(crate) auto_dj: AutoDjCandidateOperation,
    pub(crate) folders: Option<Folders>,
    pub(crate) lyrics: Option<NativeLyrics>,
    pub(crate) reporter: Option<NativePlaybackReporting>,
}

impl ActiveSource {
    pub(crate) fn supports_playlist_operation(
        &self,
        operation: SourcePlaylistOperation,
        owner: SourceFeatureOwner,
    ) -> bool {
        self.playlist_rows.supports(operation, owner)
    }
}

pub(crate) fn selected_active_source(
    slot: &ActiveSourceSlot,
    source_id: &domain::SourceId,
) -> Result<Arc<ActiveSource>, String> {
    slot.read()
        .map_err(|_| "active source lock was poisoned".to_string())?
        .as_ref()
        .filter(|active| active.identity.id == *source_id)
        .cloned()
        .ok_or_else(|| "The selected source is not active.".to_string())
}

pub(crate) fn current_active_source(slot: &ActiveSourceSlot) -> Option<Arc<ActiveSource>> {
    slot.read().ok().and_then(|active| active.as_ref().cloned())
}

pub(crate) fn active_source_instance_is_current(
    slot: &ActiveSourceSlot,
    expected: &Arc<ActiveSource>,
) -> bool {
    current_active_source(slot).is_some_and(|active| Arc::ptr_eq(&active, expected))
}

pub(crate) fn with_active_source_instance<T>(
    slot: &ActiveSourceSlot,
    expected: &Arc<ActiveSource>,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let active = slot
        .read()
        .map_err(|_| "active source lock was poisoned".to_string())?;
    if !active
        .as_ref()
        .is_some_and(|active| Arc::ptr_eq(active, expected))
    {
        return Err("The selected source changed during reconciliation.".to_string());
    }
    operation()
}

pub(crate) fn map_server_path_to_local(raw: &str, access: &SourceLocalAccess) -> Option<PathBuf> {
    let replace_to = access
        .path_replace_to
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&access.root_path);
    if let Some(prefix) = access
        .path_replace_from
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && raw.starts_with(prefix)
    {
        let suffix = raw.get(prefix.len()..)?.trim_start_matches(['/', '\\']);
        return Some(PathBuf::from(replace_to).join(path_from_server_suffix(suffix)));
    }
    let raw_path = std::path::Path::new(raw);
    raw_path
        .is_relative()
        .then(|| PathBuf::from(replace_to).join(raw_path))
}

fn path_from_server_suffix(suffix: &str) -> PathBuf {
    suffix
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect()
}

pub(crate) fn cached_local_audio_path(
    store: &Store,
    source_id: &SourceId,
    track_id: &TrackId,
) -> StoreResult<Option<PathBuf>> {
    Ok(store
        .track_local_path(source_id, track_id)?
        .map(PathBuf::from)
        .filter(|path| timed_is_file(path, "direct")))
}

pub(crate) fn matched_remote_audio_path(
    store: &Store,
    source_id: &SourceId,
    track_id: &TrackId,
) -> StoreResult<Option<PathBuf>> {
    Ok(store
        .track_local_match_path(source_id, track_id)?
        .map(PathBuf::from)
        .filter(|path| timed_is_file(path, "matched")))
}

pub(crate) fn accessible_remote_audio_path(
    store: &Store,
    source_id: &SourceId,
    track_id: &TrackId,
) -> StoreResult<Option<PathBuf>> {
    let Some(access) = store.source_local_access(source_id)? else {
        return Ok(None);
    };
    if let Some(matched) = store
        .track_local_match_path(source_id, track_id)?
        .map(PathBuf::from)
        .filter(|path| timed_is_file(path, "matched"))
    {
        return Ok(Some(matched));
    }
    let Some(raw) = store.track_local_path(source_id, track_id)? else {
        return Ok(None);
    };
    let direct = PathBuf::from(&raw);
    if timed_is_file(&direct, "raw") {
        return Ok(Some(direct));
    }
    Ok(map_server_path_to_local(&raw, &access).filter(|path| timed_is_file(path, "mapped")))
}

fn timed_is_file(path: &Path, kind: &str) -> bool {
    let started = std::time::Instant::now();
    let exists = path.is_file();
    let elapsed_ms = started.elapsed().as_millis();
    if elapsed_ms > 250 {
        info!(kind, elapsed_ms, exists, "slow local audio file check");
    }
    exists
}

impl fmt::Debug for ActiveSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveSource")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

pub(crate) fn cached_home_section_loader(source_id: SourceId) -> HomeSectionLoader {
    Arc::new(move |store, _runtime, kind| {
        store.with_store(|store| {
            Ok(store
                .load_home_sections(&source_id)?
                .into_iter()
                .find(|section| section.kind == kind)
                .unwrap_or(HomeSection {
                    kind,
                    albums: Vec::new(),
                    tracks: Vec::new(),
                }))
        })
    })
}

pub(crate) fn native_home_section_loader(source: LibraryCore) -> HomeSectionLoader {
    Arc::new(move |_store, runtime, kind| {
        runtime
            .block_on(source.home_section(kind))
            .map_err(|error| error.to_string())
    })
}
