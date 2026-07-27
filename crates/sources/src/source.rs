use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::Sender;
use library::{
    AlbumId, ArtistId, CandidateBatch, FavoriteAcceptance, FavoriteItemId, FolderContents,
    FolderId, GenreId, HomeFacts, HomeSectionKind, ImageRef, LocalArtworkRef,
    LocalComponentBaseline, LocalComponentReplacement, MusicFolderId, PlaylistAcceptance,
    PlaylistEdit, PlaylistId, PlaylistSnapshot, ProviderFreshness, SourceHomeSection,
    SourceHomeSectionKind, SourceId, TrackId,
};

use crate::{
    GeneratedTracksRequest, ImageBytes, LyricsSearch, NativeLyrics, PlaybackReport,
    RandomTrackRequest, SourceConfiguration, SourceError, SourceResult, SourceSettingsInput,
    SourceSetupInput, StreamDescriptor, StreamRequest,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceReadStage {
    Albums,
    Tracks,
    Artists,
    Genres,
    Playlists,
    Home,
    Artwork,
    Files,
    Finalizing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceReadProgress {
    pub stage: SourceReadStage,
    pub completed: usize,
    pub total: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SourceReadSummary {
    pub albums: usize,
    pub tracks: usize,
    pub artists: usize,
    pub genres: usize,
    pub music_folders: usize,
    pub playlists: usize,
    pub local_files: usize,
    pub metadata_fallbacks: usize,
    pub unreadable_files: usize,
    pub invalid_cues: usize,
    pub skipped_playlists: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeSourceResult<T> {
    Available(T),
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceImageRequest {
    Native { image_ref: ImageRef, size: u32 },
    Local(LocalArtworkRef),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFreshness {
    Unavailable,
    Unchanged,
    Busy,
    Changed(ProviderFreshness),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLibraryChange {
    inner: SourceLibraryChangeKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SourceLibraryChangeKind {
    Full,
    Jellyfin(BTreeSet<String>),
}

impl SourceLibraryChange {
    pub fn merge(&mut self, other: Self) {
        match (&mut self.inner, other.inner) {
            (SourceLibraryChangeKind::Full, _) | (_, SourceLibraryChangeKind::Full) => {
                self.inner = SourceLibraryChangeKind::Full
            }
            (
                SourceLibraryChangeKind::Jellyfin(current),
                SourceLibraryChangeKind::Jellyfin(incoming),
            ) => current.extend(incoming),
        }
    }

    pub(crate) fn jellyfin_items(items: impl IntoIterator<Item = String>) -> Self {
        Self {
            inner: SourceLibraryChangeKind::Jellyfin(items.into_iter().collect()),
        }
    }

    pub(crate) fn full() -> Self {
        Self {
            inner: SourceLibraryChangeKind::Full,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceLibraryItemId {
    Album(AlbumId),
    Track(TrackId),
    Artist(ArtistId),
    Genre(GenreId),
    Playlist(PlaylistId),
    MusicFolder(MusicFolderId),
}

#[derive(Clone, Debug)]
pub enum SourceLibraryChangeRead {
    Exact(library::SourceLibraryUpdate),
    Full,
    Ignored,
}

pub struct ConnectedSource {
    configuration: SourceConfiguration,
    source: Source,
    credential: Option<String>,
}

pub enum SourceEditResult {
    Unchanged,
    ConfigurationOnly(SourceConfiguration),
    SameAccount(ConnectedSource),
    DifferentAccount(ConnectedSource),
}

pub(crate) trait RemotePlaylistSource {
    async fn create_playlist(&self, name: &str, track_ids: &[TrackId]) -> SourceResult<PlaylistId>;
    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> SourceResult<()>;
    async fn delete_playlist(&self, playlist_id: &PlaylistId) -> SourceResult<()>;
    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> SourceResult<()>;
    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        occurrence_ids: &[String],
    ) -> SourceResult<()>;
    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        occurrence_id: &str,
        new_index: usize,
    ) -> SourceResult<()>;
    async fn read_playlist_snapshot(
        &self,
        playlist_id: &PlaylistId,
    ) -> SourceResult<PlaylistSnapshot>;
}

impl ConnectedSource {
    pub fn into_parts(self) -> (SourceConfiguration, Source, Option<String>) {
        (self.configuration, self.source, self.credential)
    }

    pub(crate) fn local(
        configuration: SourceConfiguration,
        source: crate::local::LocalSource,
    ) -> Self {
        Self {
            source: Source::new(
                configuration.source_id.clone(),
                Implementation::Local(source),
            ),
            configuration,
            credential: None,
        }
    }

    pub(crate) fn jellyfin(
        configuration: SourceConfiguration,
        source: crate::jellyfin::JellyfinSource,
        credential: Option<String>,
    ) -> Self {
        Self {
            source: Source::new(
                configuration.source_id.clone(),
                Implementation::Jellyfin(source),
            ),
            configuration,
            credential,
        }
    }

    pub(crate) fn subsonic(
        configuration: SourceConfiguration,
        source: crate::subsonic::SubsonicSource,
        credential: Option<String>,
    ) -> Self {
        Self {
            source: Source::new(
                configuration.source_id.clone(),
                Implementation::OpenSubsonic(source),
            ),
            configuration,
            credential,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalFilesystemChange {
    Paths(BTreeSet<PathBuf>),
    Rescan,
}

impl LocalFilesystemChange {
    pub fn merge(&mut self, other: Self) {
        match (&mut *self, other) {
            (Self::Rescan, _) => {}
            (current, Self::Rescan) => *current = Self::Rescan,
            (Self::Paths(current), Self::Paths(other)) => current.extend(other),
        }
    }
}

pub struct SourceLocalCheck {
    inner: crate::local::LocalCheck,
}

impl SourceLocalCheck {
    pub fn file_seeds(&self) -> &[library::LocalFileSeed] {
        self.inner.file_seeds()
    }
}

pub struct SourceLocalChange {
    inner: crate::local::LocalChange,
}

impl SourceLocalChange {
    pub fn component_seeds(&self) -> &[library::LocalComponentSeed] {
        self.inner.component_seeds()
    }
}

/// Inputs that determine the canonical facts emitted by one source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInputIdentity {
    pub source_id: SourceId,
    pub version: u32,
    pub digest: [u8; 32],
}

/// Terminal facts from one bounded source read.
///
/// Canonical batches cross the callback while the concrete source reads them.
/// Library owns candidate creation, persistence, comparison, and acceptance.
pub struct SourceFacts {
    freshness: Option<ProviderFreshness>,
    home: HomeFacts,
    summary: SourceReadSummary,
}

impl SourceFacts {
    pub fn freshness(&self) -> Option<&ProviderFreshness> {
        self.freshness.as_ref()
    }

    pub fn home(&self) -> &HomeFacts {
        &self.home
    }

    pub fn summary(&self) -> SourceReadSummary {
        self.summary
    }

    pub(crate) fn new(
        freshness: Option<ProviderFreshness>,
        home: HomeFacts,
        summary: SourceReadSummary,
    ) -> Self {
        Self {
            freshness,
            home,
            summary,
        }
    }
}

/// Direct bounded edge from a provider response to Library's candidate.
///
/// `false` means the caller could not accept the batch. Rufin retains the
/// actual downstream error and cancels the source operation; Sources does not
/// need a Library error or handle.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "provider fixtures use the direct callback sink")
)]
enum BatchTarget<'a> {
    Callback(&'a mut (dyn FnMut(CandidateBatch) -> bool + Send)),
    Channel(Sender<CandidateBatch>),
}

pub(crate) struct BatchEmitter<'a> {
    target: BatchTarget<'a>,
    summary: SourceReadSummary,
}

impl<'a> BatchEmitter<'a> {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "provider fixtures use the direct callback sink")
    )]
    pub(crate) fn new(accept: &'a mut (dyn FnMut(CandidateBatch) -> bool + Send)) -> Self {
        Self {
            target: BatchTarget::Callback(accept),
            summary: SourceReadSummary::default(),
        }
    }

    fn channel(sender: Sender<CandidateBatch>) -> Self {
        Self {
            target: BatchTarget::Channel(sender),
            summary: SourceReadSummary::default(),
        }
    }

    pub(crate) fn emit(&mut self, batch: CandidateBatch) -> SourceResult<()> {
        if !self.record(&batch) {
            return Ok(());
        }
        let accepted = match &mut self.target {
            BatchTarget::Callback(accept) => accept(batch),
            BatchTarget::Channel(sender) => sender.send_blocking(batch).is_ok(),
        };
        if !accepted {
            return Err(SourceError::Cancelled);
        }
        Ok(())
    }

    pub(crate) async fn emit_async(&mut self, batch: CandidateBatch) -> SourceResult<()> {
        if !self.record(&batch) {
            return Ok(());
        }
        let accepted = match &mut self.target {
            BatchTarget::Callback(accept) => accept(batch),
            BatchTarget::Channel(sender) => sender.send(batch).await.is_ok(),
        };
        if !accepted {
            return Err(SourceError::Cancelled);
        }
        Ok(())
    }

    fn record(&mut self, batch: &CandidateBatch) -> bool {
        if batch.is_empty() {
            return false;
        }
        match batch {
            CandidateBatch::Albums(values) => self.summary.albums += values.len(),
            CandidateBatch::Tracks(values) => self.summary.tracks += values.len(),
            CandidateBatch::Artists(values) => self.summary.artists += values.len(),
            CandidateBatch::Genres(values) => self.summary.genres += values.len(),
            CandidateBatch::MusicFolders(values) => self.summary.music_folders += values.len(),
            CandidateBatch::Playlists(values) => self.summary.playlists += values.len(),
            CandidateBatch::LocalFiles(values) => self.summary.local_files += values.len(),
        }
        true
    }

    pub(crate) fn summary(&self) -> SourceReadSummary {
        self.summary
    }

    pub(crate) fn metadata_fallback(&mut self) {
        self.summary.metadata_fallbacks += 1;
    }

    pub(crate) fn unreadable_file(&mut self) {
        self.summary.unreadable_files += 1;
    }

    pub(crate) fn invalid_cue(&mut self) {
        self.summary.invalid_cues += 1;
    }

    pub(crate) fn skipped_playlist(&mut self) {
        self.summary.skipped_playlists += 1;
    }
}

/// One opened concrete source. Its provider enum remains private.
pub struct Source {
    source_id: SourceId,
    implementation: Implementation,
}

enum Implementation {
    Local(crate::local::LocalSource),
    Jellyfin(crate::jellyfin::JellyfinSource),
    OpenSubsonic(crate::subsonic::SubsonicSource),
}

impl Source {
    fn new(source_id: SourceId, implementation: Implementation) -> Self {
        Self {
            source_id,
            implementation,
        }
    }

    pub async fn connect(input: SourceSetupInput) -> SourceResult<ConnectedSource> {
        match input {
            SourceSetupInput::Local(input) => crate::local::connect(input),
            SourceSetupInput::Jellyfin(input) => crate::jellyfin::connect(input).await,
            SourceSetupInput::Subsonic {
                flavor,
                credentials,
            } => crate::subsonic::connect(flavor, credentials).await,
        }
    }

    /// Apply an edit to one configured source without exposing provider payloads
    /// outside Sources.
    ///
    /// The configured source identity is stable. Authentication may reveal a
    /// different provider account, but Rufin still owns the same configured
    /// source slot and decides when its accepted facts are replaced.
    pub async fn edit(
        current: SourceConfiguration,
        current_credential: Option<String>,
        input: SourceSettingsInput,
        jellyfin_device_id: Option<String>,
    ) -> SourceResult<SourceEditResult> {
        match input {
            SourceSettingsInput::Local { roots } => crate::local::edit(current, roots),
            SourceSettingsInput::Jellyfin(input) => {
                crate::jellyfin::edit(current, current_credential, input, jellyfin_device_id).await
            }
            SourceSettingsInput::Subsonic(credentials) => {
                crate::subsonic::edit(current, current_credential, credentials).await
            }
        }
    }

    /// Open a configured source without contacting it.
    ///
    /// Cache usability and source access are decided later. In particular, a
    /// temporarily unavailable Local root does not erase or invalidate its
    /// saved source.
    pub fn open(
        configuration: SourceConfiguration,
        credential: Option<String>,
        jellyfin_device_id: Option<String>,
    ) -> SourceResult<Self> {
        let implementation =
            match configuration.kind.as_str() {
                crate::local::LOCAL_SOURCE_ID => Implementation::Local(
                    crate::local::LocalSource::from_configuration(&configuration)?,
                ),
                crate::jellyfin::JELLYFIN_SOURCE_ID => Implementation::Jellyfin(
                    crate::jellyfin::open(&configuration, credential, jellyfin_device_id)?,
                ),
                "navidrome" | "subsonic" => {
                    Implementation::OpenSubsonic(crate::subsonic::open(&configuration, credential)?)
                }
                kind => {
                    return Err(SourceError::InvalidConfig(format!(
                        "unknown source kind {kind}"
                    )));
                }
            };
        Ok(Self::new(configuration.source_id, implementation))
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub async fn listen_selected_changes(
        self: Arc<Self>,
        catch_up: bool,
        on_items: impl FnMut(SourceLibraryChange) -> bool + Send + 'static,
        on_local: impl FnMut(LocalFilesystemChange) -> bool + Send + 'static,
        should_stop: impl Fn() -> bool + Send + Sync + 'static,
    ) -> SourceResult<NativeSourceResult<()>> {
        match &self.implementation {
            Implementation::Jellyfin(source) => {
                let on_items = std::sync::Mutex::new(on_items);
                let mut on_ready = |reconnecting| {
                    let request = feed_needs_catch_up(catch_up, reconnecting);
                    !request
                        || on_items
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())(
                            SourceLibraryChange::full(),
                        )
                };
                let mut on_change = |change| {
                    on_items
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())(change)
                };
                source
                    .listen_library_changes(&mut on_ready, &mut on_change, &should_stop)
                    .await
                    .map(NativeSourceResult::Available)
            }
            Implementation::Local(_) => tokio::task::spawn_blocking(move || {
                let Implementation::Local(source) = &self.implementation else {
                    unreachable!("selected source implementation changed")
                };
                let on_local = std::sync::Mutex::new(on_local);
                let mut on_ready = |reconnecting| {
                    let request = feed_needs_catch_up(catch_up, reconnecting);
                    !request
                        || on_local
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())(
                            LocalFilesystemChange::Rescan,
                        )
                };
                let mut on_change = |change| {
                    on_local
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())(change)
                };
                source.watch(&mut on_ready, &mut on_change, &should_stop)
            })
            .await
            .map_err(|error| SourceError::Other(error.to_string()))?
            .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(_) => Ok(NativeSourceResult::Unavailable),
        }
    }

    pub async fn read_library_change(
        &self,
        change: SourceLibraryChange,
        contains: &(dyn Fn(&SourceLibraryItemId) -> bool + Send + Sync),
    ) -> SourceResult<SourceLibraryChangeRead> {
        match change.inner {
            SourceLibraryChangeKind::Full => Ok(SourceLibraryChangeRead::Full),
            SourceLibraryChangeKind::Jellyfin(items) => {
                let Implementation::Jellyfin(source) = &self.implementation else {
                    return Err(SourceError::InvalidRequest(
                        "the library change belongs to another source",
                    ));
                };
                source.read_library_change(items, contains).await
            }
        }
    }

    pub async fn check_freshness(
        &self,
        accepted: Option<&ProviderFreshness>,
    ) -> SourceResult<SourceFreshness> {
        match &self.implementation {
            Implementation::OpenSubsonic(source) => source.check_freshness(accepted).await,
            Implementation::Local(_) | Implementation::Jellyfin(_) => {
                Ok(SourceFreshness::Unavailable)
            }
        }
    }

    pub async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> SourceResult<NativeSourceResult<FolderContents>> {
        match &self.implementation {
            Implementation::Local(_) => Ok(NativeSourceResult::Unavailable),
            Implementation::Jellyfin(source) => source
                .read_folder(folder_id, music_folder_id)
                .await
                .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(source) => source
                .read_folder(folder_id, music_folder_id)
                .await
                .map(NativeSourceResult::Available),
        }
    }

    /// Reads one provider-owned Home section.
    ///
    /// Local and Explore are composed from the accepted Library instead.
    pub async fn home_section(
        &self,
        kind: HomeSectionKind,
    ) -> SourceResult<NativeSourceResult<SourceHomeSection>> {
        let kind = match kind {
            HomeSectionKind::Explore => return Ok(NativeSourceResult::Unavailable),
            HomeSectionKind::MostPlayed => SourceHomeSectionKind::MostPlayed,
            HomeSectionKind::NewlyAdded => SourceHomeSectionKind::NewlyAdded,
            HomeSectionKind::RecentlyPlayed => SourceHomeSectionKind::RecentlyPlayed,
            HomeSectionKind::RecentlyReleased => SourceHomeSectionKind::RecentlyReleased,
        };
        match &self.implementation {
            Implementation::Local(_) => Ok(NativeSourceResult::Unavailable),
            Implementation::Jellyfin(source) => source
                .read_home_section(kind)
                .await
                .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(source) => source
                .read_home_section(kind)
                .await
                .map(NativeSourceResult::Available),
        }
    }

    pub async fn random_tracks(
        &self,
        request: RandomTrackRequest,
    ) -> SourceResult<NativeSourceResult<Vec<library::Track>>> {
        match &self.implementation {
            Implementation::Local(_) => Ok(NativeSourceResult::Unavailable),
            Implementation::Jellyfin(source) => source
                .random_tracks(request)
                .await
                .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(_)
                if request.played_filter != crate::PlayedFilter::All =>
            {
                Ok(NativeSourceResult::Unavailable)
            }
            Implementation::OpenSubsonic(source) => source
                .random_tracks(request)
                .await
                .map(NativeSourceResult::Available),
        }
    }

    pub async fn generated_tracks(
        &self,
        request: GeneratedTracksRequest,
    ) -> SourceResult<NativeSourceResult<Vec<library::Track>>> {
        match &self.implementation {
            Implementation::Local(_) => Ok(NativeSourceResult::Unavailable),
            Implementation::Jellyfin(source) => source
                .generated_tracks(request)
                .await
                .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(source) => source
                .generated_tracks(request)
                .await
                .map(NativeSourceResult::Available),
        }
    }

    pub async fn stream(
        &self,
        request: &StreamRequest,
    ) -> SourceResult<NativeSourceResult<StreamDescriptor>> {
        match &self.implementation {
            Implementation::Local(_) => Ok(NativeSourceResult::Unavailable),
            Implementation::Jellyfin(source) => source
                .resolve_stream(request)
                .await
                .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(source) => source
                .resolve_stream(request)
                .await
                .map(NativeSourceResult::Available),
        }
    }

    pub async fn image(&self, request: SourceImageRequest) -> SourceResult<ImageBytes> {
        match (&self.implementation, request) {
            (Implementation::Local(source), SourceImageRequest::Local(reference)) => {
                source.image_bytes(&reference)
            }
            (Implementation::Jellyfin(source), SourceImageRequest::Native { image_ref, size }) => {
                source.image_bytes(&image_ref, size).await
            }
            (
                Implementation::OpenSubsonic(source),
                SourceImageRequest::Native { image_ref, size },
            ) => source.image_bytes(&image_ref, size).await,
            _ => Err(SourceError::InvalidRequest(
                "artwork does not belong to the selected source",
            )),
        }
    }

    pub async fn set_favorite(
        &self,
        item: FavoriteItemId,
        favorite: bool,
    ) -> SourceResult<FavoriteAcceptance> {
        match &self.implementation {
            Implementation::Local(_) => Ok(FavoriteAcceptance::RufinOwned { item, favorite }),
            Implementation::Jellyfin(source) => {
                source.set_favorite(item.clone(), favorite).await?;
                Ok(FavoriteAcceptance::SourceAcknowledged { item, favorite })
            }
            Implementation::OpenSubsonic(source) => {
                source.set_favorite(item.clone(), favorite).await?;
                Ok(FavoriteAcceptance::SourceAcknowledged { item, favorite })
            }
        }
    }

    pub async fn edit_playlist(&self, edit: PlaylistEdit) -> SourceResult<PlaylistAcceptance> {
        match &self.implementation {
            Implementation::Local(_) => Ok(PlaylistAcceptance::RufinOwned(edit)),
            Implementation::Jellyfin(source) => edit_remote_playlist(source, edit).await,
            Implementation::OpenSubsonic(source) => edit_remote_playlist(source, edit).await,
        }
    }

    pub async fn lyrics(
        &self,
        track_id: &TrackId,
        search: LyricsSearch,
    ) -> SourceResult<NativeSourceResult<Option<NativeLyrics>>> {
        match &self.implementation {
            Implementation::Local(_) => Ok(NativeSourceResult::Unavailable),
            Implementation::Jellyfin(source) => source
                .lyrics(track_id, search)
                .await
                .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(source) => source
                .lyrics(track_id, search)
                .await
                .map(NativeSourceResult::Available),
        }
    }

    pub async fn report_playback(
        &self,
        report: PlaybackReport,
    ) -> SourceResult<NativeSourceResult<()>> {
        match &self.implementation {
            Implementation::Local(_) => Ok(NativeSourceResult::Unavailable),
            Implementation::Jellyfin(source) => source
                .report_playback(report)
                .await
                .map(NativeSourceResult::Available),
            Implementation::OpenSubsonic(source) => source
                .report_playback(report)
                .await
                .map(NativeSourceResult::Available),
        }
    }

    pub fn check_local(
        &self,
        change: LocalFilesystemChange,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<SourceLocalCheck> {
        let Implementation::Local(source) = &self.implementation else {
            return Err(SourceError::InvalidRequest(
                "filesystem verification requires a Local source",
            ));
        };
        source
            .check(change, cancelled)
            .map(|inner| SourceLocalCheck { inner })
    }

    pub fn confirm_local_change(
        &self,
        check: SourceLocalCheck,
        baseline: library::LocalFileBaseline,
        progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<Option<SourceLocalChange>> {
        let Implementation::Local(source) = &self.implementation else {
            return Err(SourceError::InvalidRequest(
                "filesystem verification requires a Local source",
            ));
        };
        source
            .confirm_change(check.inner, baseline, progress, cancelled)
            .map(|change| change.map(|inner| SourceLocalChange { inner }))
    }

    pub fn complete_local_change(
        &self,
        change: SourceLocalChange,
        baseline: LocalComponentBaseline,
        observed_at: i64,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<LocalComponentReplacement> {
        let Implementation::Local(source) = &self.implementation else {
            return Err(SourceError::InvalidRequest(
                "filesystem replacement requires a Local source",
            ));
        };
        source.complete_change(change.inner, baseline, observed_at, cancelled)
    }

    pub async fn read_source_facts(
        self: Arc<Self>,
        batches: Sender<CandidateBatch>,
        progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
        cancelled: Arc<AtomicBool>,
    ) -> SourceResult<SourceFacts> {
        let (freshness, home, summary) = match &self.implementation {
            Implementation::Local(_) => {
                let source = Arc::clone(&self);
                let is_cancelled = move || cancelled.load(Ordering::Acquire);
                let completed = tokio::task::spawn_blocking(move || {
                    let Implementation::Local(local) = &source.implementation else {
                        unreachable!("source kind changed while reading Local facts");
                    };
                    let mut emitter = BatchEmitter::channel(batches);
                    let result = local.read_facts(&mut emitter, &*progress, &is_cancelled);
                    result.map(|(freshness, home)| (freshness, home, emitter.summary()))
                })
                .await
                .map_err(|error| SourceError::Other(error.to_string()))??;
                completed
            }
            Implementation::Jellyfin(source) => {
                let mut emitter = BatchEmitter::channel(batches);
                let is_cancelled = || cancelled.load(Ordering::Acquire);
                let (freshness, home) = source
                    .read_facts(&mut emitter, &*progress, &is_cancelled)
                    .await?;
                (freshness, home, emitter.summary())
            }
            Implementation::OpenSubsonic(source) => {
                let mut emitter = BatchEmitter::channel(batches);
                let is_cancelled = || cancelled.load(Ordering::Acquire);
                let (freshness, home) = source
                    .read_facts(&mut emitter, &*progress, &is_cancelled)
                    .await?;
                (freshness, home, emitter.summary())
            }
        };
        Ok(SourceFacts::new(freshness, home, summary))
    }
}

pub(crate) fn configured_source_name(configured: Option<String>, provider: String) -> String {
    if let Some(name) = configured.filter(|name| !name.trim().is_empty()) {
        name.trim().to_string()
    } else {
        provider
    }
}

pub(crate) fn require_source_edit(current: &SourceConfiguration, kind: &str) -> SourceResult<()> {
    if current.kind != kind {
        return Err(SourceError::InvalidRequest(
            "the source edit belongs to another provider",
        ));
    }
    Ok(())
}

pub(crate) fn comparable_address(value: &str) -> &str {
    value.trim().trim_end_matches('/')
}

fn feed_needs_catch_up(catch_up: bool, reconnecting: bool) -> bool {
    catch_up || reconnecting
}

pub(crate) fn edited_source_name(requested: &str, current: &str) -> String {
    let requested = requested.trim();
    if requested.is_empty() {
        current.to_string()
    } else {
        requested.to_string()
    }
}

async fn edit_remote_playlist(
    source: &impl RemotePlaylistSource,
    edit: PlaylistEdit,
) -> SourceResult<PlaylistAcceptance> {
    let playlist_id = match edit {
        PlaylistEdit::Create { name, track_ids } => {
            source.create_playlist(&name, &track_ids).await?
        }
        PlaylistEdit::Rename { playlist_id, name } => {
            source.rename_playlist(&playlist_id, &name).await?;
            playlist_id
        }
        PlaylistEdit::Delete { playlist_id } => {
            source.delete_playlist(&playlist_id).await?;
            return Ok(PlaylistAcceptance::SourceDeleted(playlist_id));
        }
        PlaylistEdit::AddTracks {
            playlist_id,
            track_ids,
        } => {
            source.add_playlist_tracks(&playlist_id, &track_ids).await?;
            playlist_id
        }
        PlaylistEdit::RemoveEntries {
            playlist_id,
            occurrence_ids,
        } => {
            source
                .remove_playlist_entries(&playlist_id, &occurrence_ids)
                .await?;
            playlist_id
        }
        PlaylistEdit::MoveEntry {
            playlist_id,
            occurrence_id,
            new_index,
        } => {
            source
                .move_playlist_entry(&playlist_id, &occurrence_id, new_index)
                .await?;
            playlist_id
        }
    };
    source
        .read_playlist_snapshot(&playlist_id)
        .await
        .map(PlaylistAcceptance::SourceSnapshot)
}

#[cfg(test)]
mod input_identity_tests {
    use super::*;

    #[test]
    fn selected_item_changes_coalesce_without_losing_a_full_refresh() {
        let mut changed =
            SourceLibraryChange::jellyfin_items(["track:one".to_string(), "track:two".to_string()]);
        changed.merge(SourceLibraryChange::jellyfin_items([
            "track:two".to_string(),
            "album:one".to_string(),
        ]));
        assert_eq!(
            changed,
            SourceLibraryChange {
                inner: SourceLibraryChangeKind::Jellyfin(BTreeSet::from([
                    "album:one".to_string(),
                    "track:one".to_string(),
                    "track:two".to_string(),
                ])),
            }
        );

        changed.merge(SourceLibraryChange::full());
        assert_eq!(changed, SourceLibraryChange::full());
    }

    #[test]
    fn local_file_changes_coalesce_without_losing_a_rescan() {
        let mut changed =
            LocalFilesystemChange::Paths(BTreeSet::from([PathBuf::from("/music/one.flac")]));
        changed.merge(LocalFilesystemChange::Paths(BTreeSet::from([
            PathBuf::from("/music/one.flac"),
            PathBuf::from("/music/two.flac"),
        ])));
        assert_eq!(
            changed,
            LocalFilesystemChange::Paths(BTreeSet::from([
                PathBuf::from("/music/one.flac"),
                PathBuf::from("/music/two.flac"),
            ]))
        );

        changed.merge(LocalFilesystemChange::Rescan);
        assert_eq!(changed, LocalFilesystemChange::Rescan);
    }

    fn jellyfin(
        base_url: &str,
        user_id: &str,
        trust_invalid_cert: bool,
        instant_mix: bool,
        name: &str,
    ) -> SourceConfiguration {
        crate::config::encode_provider_payload(
            SourceId::new("configured:jellyfin"),
            crate::jellyfin::JELLYFIN_SOURCE_ID,
            name,
            crate::jellyfin::JellyfinSourceConfig {
                base_url: base_url.to_string(),
                server_id: None,
                user_id: user_id.to_string(),
                username: "listener".to_string(),
                trust_invalid_cert,
                use_instant_mix: instant_mix,
            }
            .into_payload(),
        )
    }

    fn subsonic(base_url: &str, username: &str, trust_invalid_cert: bool) -> SourceConfiguration {
        crate::config::encode_provider_payload(
            SourceId::new("configured:subsonic"),
            "navidrome",
            "Server",
            crate::subsonic::SubsonicSourceConfig {
                base_url: base_url.to_string(),
                username: username.to_string(),
                trust_invalid_cert,
            }
            .into_payload(),
        )
    }

    fn input_digest(configuration: &SourceConfiguration) -> [u8; 32] {
        configuration
            .input_identity()
            .expect("valid source input identity")
            .digest
    }

    #[test]
    fn remote_account_qualifies_cached_source_facts() {
        let jellyfin_a = jellyfin("https://one.example", "account", false, false, "One");
        let jellyfin_b = jellyfin("https://two.example", "account", false, false, "One");
        let jellyfin_other_user =
            jellyfin("https://one.example", "other-account", false, false, "One");
        assert_eq!(input_digest(&jellyfin_a), input_digest(&jellyfin_b));
        assert_ne!(
            input_digest(&jellyfin_a),
            input_digest(&jellyfin_other_user)
        );

        let subsonic_a = subsonic("https://one.example", "listener", false);
        let subsonic_b = subsonic("https://two.example", "listener", false);
        let subsonic_other_user = subsonic("https://one.example", "other", false);
        assert_eq!(input_digest(&subsonic_a), input_digest(&subsonic_b));
        assert_ne!(
            input_digest(&subsonic_a),
            input_digest(&subsonic_other_user)
        );
    }

    #[test]
    fn presentation_and_transport_policy_do_not_invalidate_cached_facts() {
        let baseline = jellyfin("https://music.example", "account", false, false, "Music");
        let presentation = jellyfin("https://music.example/", "account", true, true, "Renamed");
        assert_eq!(input_digest(&baseline), input_digest(&presentation));

        let subsonic_baseline = subsonic("https://music.example", "listener", false);
        let subsonic_trust = subsonic("https://music.example/", "listener", true);
        assert_eq!(
            input_digest(&subsonic_baseline),
            input_digest(&subsonic_trust)
        );
    }

    #[test]
    fn local_roots_qualify_cached_source_facts() {
        let first = SourceConfiguration::local(
            SourceId::new(crate::local::LOCAL_LIBRARY_SOURCE_ID),
            "Local",
            vec![PathBuf::from("/music/one")],
        )
        .expect("first Local configuration");
        let second = SourceConfiguration::local(
            SourceId::new(crate::local::LOCAL_LIBRARY_SOURCE_ID),
            "Local",
            vec![PathBuf::from("/music/two")],
        )
        .expect("second Local configuration");
        assert_ne!(input_digest(&first), input_digest(&second));
    }

    #[tokio::test]
    async fn local_home_refresh_stays_at_the_library_boundary() {
        let source = Source::open(
            crate::config::encode_provider_payload(
                SourceId::new(crate::local::LOCAL_LIBRARY_SOURCE_ID),
                crate::local::LOCAL_SOURCE_ID,
                "Local",
                crate::local::LocalSourceConfig {
                    roots: vec![PathBuf::from("/unavailable/local-home-fixture")],
                }
                .into_payload(),
            ),
            None,
            None,
        )
        .expect("open Local fixture");

        for kind in [
            HomeSectionKind::Explore,
            HomeSectionKind::MostPlayed,
            HomeSectionKind::NewlyAdded,
            HomeSectionKind::RecentlyPlayed,
            HomeSectionKind::RecentlyReleased,
        ] {
            assert!(matches!(
                source
                    .home_section(kind)
                    .await
                    .expect("Local Home boundary"),
                NativeSourceResult::Unavailable
            ));
        }
    }
}
