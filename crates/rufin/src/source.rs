//! The configured sources and the one selected source session.
//!
//! Rufin owns selection and operation ordering here. Concrete sources acquire
//! facts and perform provider operations; Library accepts and queries them.

mod acquisition;

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artwork::{Artwork, SourceImages};
use async_channel::{Receiver, Sender};
use library::{
    AcceptedLibraryChange, CandidateChange, FavoriteAcceptance, FavoriteItemId, FolderContents,
    HomeSectionKind, HomeSnapshot, Library, LoadedLibrary, LocalAccessTarget, MetadataDraft,
    MetadataEdit, MetadataEditing, MetadataError, MetadataItemId, MusicFolderId,
    PlaylistAcceptance, PlaylistEdit, PlaylistTrackAdd, PreparedSourceCandidate, RecordedActivity,
    SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistId, SourceHomeSection, SourceId,
    Track, TrackSort,
};
use playback::{PlaybackProjection, SourceSessionEpoch};
use scrobbling::Scrobbler;
use secrets::{SecretStorageMode, SwitchableSecretStore};
use sources::{
    CredentialHostInput, CredentialSettingsInput, JellyfinSettingsInput, JellyfinSetupInput,
    LocalFilesystemChange, LocalFolderHostInput, MetadataRefresh, NativeSourceResult, Source,
    SourceCacheMatch, SourceConfiguration, SourceEditResult, SourceFreshness, SourceInputIdentity,
    SourceLibraryChange, SourceLibraryChangeRead, SourceLibraryItemId, SourceReadProgress,
    SourceReadStage, SourceSettingsInput, SourceSetupInput, SubsonicFlavor,
};
use tokio::task::JoinHandle;
use tracing::warn;
use ui::runtime::source::{
    ConfiguredSources, CredentialInput, CredentialPreset, DiscoveredServer, DiscoveryStatus,
    DiscoveryUpdate, DownloadRequest, EditableSource, FolderRequest, LocalAccessStatus,
    LocalFolder, MetadataEditRequest, MetadataIdentificationRequest, MetadataRequest,
    OpenSubsonicKind, RemoveDownloadRequest, SearchRequest, SourceLocalAccess,
    SourceLocalAccessSummary, SourceOperation, SourcePort, SourceProgress, SourceProgressStage,
    SourceSettingsChange, SourceSetup, SourceSummary,
};
use ui::runtime::{
    FavoriteFailure, HomePublication, SelectedLibrary, SelectedLibraryUpdate, SourceEvent,
};

use crate::album_release::run_selected_album_release_lookup;
use crate::playback::PlaybackOwner;
use crate::settings::{
    ConfiguredLocalAccess, ConfiguredSource, CredentialRef, SettingsFile, SourceSettings,
    StoredSettings, all_secret_keys, delete_provider_secret, fresh_credential_ref,
    fresh_secret_scope_id, load_provider_secret, load_scrobbling_settings, platform_secret_store,
    save_provider_secret,
};
use downloads::Downloads;

const SOURCE_CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(crate) struct SelectedSourceRuntime {
    pub(crate) configuration: SourceConfiguration,
    pub(crate) source: Option<Arc<Source>>,
    pub(crate) source_session_epoch: SourceSessionEpoch,
    pub(crate) loaded: Arc<LoadedLibrary>,
    pub(crate) home: Arc<HomeSnapshot>,
    pub(crate) music_folder_id: Option<MusicFolderId>,
}

impl SelectedSourceRuntime {
    pub(crate) fn source_id(&self) -> &SourceId {
        &self.configuration.source_id
    }

    fn qualifier(&self) -> SourceQualifier {
        SourceQualifier {
            source_id: self.source_id().clone(),
            epoch: self.source_session_epoch,
        }
    }

    fn metadata_context(
        &self,
        item_id: &MetadataItemId,
    ) -> Result<Option<MetadataContext>, library::LoadedLibraryError> {
        let Some(source) = self.source.as_ref().cloned() else {
            return Ok(None);
        };
        let Some(subject) = self.loaded.metadata_subject(item_id)? else {
            return Ok(None);
        };
        Ok(Some(MetadataContext {
            source,
            subject,
            local_access: None,
        }))
    }

    fn metadata_editing_available(&self, item_id: &MetadataItemId) -> bool {
        let Some(source) = self.source.as_ref() else {
            return false;
        };
        self.loaded
            .metadata_item(item_id)
            .ok()
            .flatten()
            .is_some_and(|item| source.metadata_editing_available(&item))
    }

    fn metadata_access_context(
        &self,
        item_id: &MetadataItemId,
    ) -> Result<Option<MetadataContext>, MetadataError> {
        let Some(source) = self.source.as_ref().cloned() else {
            return Ok(None);
        };
        if source.needs_metadata_local_access() {
            let Some((subject, local_access)) = self
                .loaded
                .metadata_subject_with_local_access(item_id, None)?
            else {
                return Ok(None);
            };
            return Ok(Some(MetadataContext {
                source,
                subject,
                local_access: Some(local_access),
            }));
        }
        let Some(subject) = self
            .loaded
            .metadata_subject(item_id)
            .map_err(|error| MetadataError::Write(error.to_string()))?
        else {
            return Ok(None);
        };
        Ok(Some(MetadataContext {
            source,
            subject,
            local_access: None,
        }))
    }
}

struct MetadataContext {
    source: Arc<Source>,
    subject: library::MetadataSubject,
    local_access: Option<Vec<LocalAccessTarget>>,
}

#[derive(Clone)]
pub(crate) struct SelectedSourceSession {
    selected: Arc<RwLock<SelectedSourceRuntime>>,
}

impl SelectedSourceSession {
    pub(crate) fn new(selected: SelectedSourceRuntime) -> Self {
        Self {
            selected: Arc::new(RwLock::new(selected)),
        }
    }

    pub(crate) fn snapshot(&self) -> SelectedSourceRuntime {
        self.selected
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn downgrade(&self) -> WeakSelectedSourceSession {
        WeakSelectedSourceSession {
            selected: Arc::downgrade(&self.selected),
        }
    }

    fn replace(&self, selected: SelectedSourceRuntime) {
        *self
            .selected
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = selected;
    }

    #[cfg(test)]
    pub(crate) fn replace_for_test(&self, selected: SelectedSourceRuntime) {
        self.replace(selected);
    }
}

#[derive(Clone)]
pub(crate) struct WeakSelectedSourceSession {
    selected: Weak<RwLock<SelectedSourceRuntime>>,
}

impl WeakSelectedSourceSession {
    pub(crate) fn snapshot(&self) -> Option<SelectedSourceRuntime> {
        self.selected.upgrade().map(|selected| {
            selected
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        })
    }
}

#[derive(Clone)]
pub(crate) struct SourceOutputs {
    pub(crate) events: Sender<SourceEvent>,
    pub(crate) discovery: Sender<DiscoveryUpdate>,
}

pub(crate) struct SourceBootstrap {
    pub(crate) owner: Arc<SourceOwner>,
    pub(crate) configured: ConfiguredSources,
    pub(crate) operation: SourceOperation,
}

pub(crate) struct SourceOwner {
    shared: Arc<Shared>,
    messages: Sender<Message>,
    receiver: Mutex<Option<Receiver<Message>>>,
}

#[derive(Clone)]
pub(crate) struct SourceAcceptanceSender {
    messages: Sender<Message>,
}

struct Shared {
    artwork: Artwork,
    library: Library,
    downloads: Downloads,
    settings: SettingsFile,
    secrets: Arc<SwitchableSecretStore>,
    scrobbler: Arc<Scrobbler>,
    runtime: tokio::runtime::Handle,
    outputs: SourceOutputs,
    selected: RwLock<Option<SelectedSourceSession>>,
    playback: Mutex<Weak<PlaybackOwner>>,
    next_epoch: AtomicU64,
    next_work: AtomicU64,
}

enum Message {
    Request(WorkRequest),
    SelectedLibraryRevealed,
    AlbumReleaseSettingChanged(bool),
    DownloadSettingsChanged,
    Download(DownloadRequest),
    RemoveDownload(RemoveDownloadRequest),
    RemoveDownloadRule {
        source_id: SourceId,
        rule: downloads::DownloadRule,
        delete_downloads: bool,
    },
    CancelDownload {
        source_id: SourceId,
        job_id: String,
    },
    MoveDownload {
        source_id: SourceId,
        job_id: String,
        target_job_id: String,
        after: bool,
    },
    ClearDownloads(SourceId),
    AlbumReleaseFinished(u64),
    Activity {
        qualifier: SourceQualifier,
        update: RecordedActivity,
        next_home: NextHome,
    },
    Progress {
        token: u64,
        progress: SourceReadProgress,
    },
    LocalAccessReady(u64),
    WorkReady(u64),
}

enum WorkRequest {
    Configure(SourceSetup),
    Update(SourceSettingsChange),
    Select(SourceId),
    ChangeSecretStorage {
        mode: SecretStorageMode,
        result: Sender<Result<(), String>>,
    },
    AddLocalFolder(PathBuf),
    RemoveLocalFolder(String),
    Refresh {
        qualifier: SourceQualifier,
        visible: bool,
    },
    SaveLocalAccess {
        input: SourceLocalAccess,
        metadata: Option<MetadataRequest>,
        result: Sender<Result<(), String>>,
    },
    ClearLocalAccess(SourceId),
    Forget(SourceId),
    SetMusicFolder {
        source_id: SourceId,
        folder_id: Option<MusicFolderId>,
    },
    Selected {
        qualifier: SourceQualifier,
        operation: PointOperation,
    },
    CheckSelectedSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceQualifier {
    source_id: SourceId,
    epoch: SourceSessionEpoch,
}

enum PointOperation {
    RefreshHome(HomeSectionKind),
    Favorite {
        item: FavoriteItemId,
        favorite: bool,
        previous: bool,
    },
    Playlist(PlaylistEdit),
    SmartPlaylist(SmartPlaylistOperation),
    ObservedItems(SourceLibraryChange),
    LocalFiles(LocalFilesystemChange),
    EditMetadata {
        edit: MetadataEdit,
        reply: MetadataReply,
    },
    CheckFreshness,
}

struct MetadataReply {
    sender: Option<Sender<Result<(), MetadataError>>>,
    write_started: bool,
}

impl MetadataReply {
    fn new(sender: Sender<Result<(), MetadataError>>) -> Self {
        Self {
            sender: Some(sender),
            write_started: false,
        }
    }

    fn mark_write_started(&mut self) {
        self.write_started = true;
    }

    fn finish(mut self, result: Result<(), MetadataError>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.try_send(result);
        }
    }
}

impl Drop for MetadataReply {
    fn drop(&mut self) {
        let Some(sender) = self.sender.take() else {
            return;
        };
        let error = if self.write_started {
            MetadataError::SavedRefreshFailed(
                "Metadata editing was interrupted before the written metadata was accepted."
                    .to_string(),
            )
        } else {
            MetadataError::Unavailable
        };
        let _ = sender.try_send(Err(error));
    }
}

enum NextHome {
    Keep,
    ActivityKeep,
    Favorite(FavoriteItemId),
    AcceptedPlay(library::TrackId),
    SourceFacts,
}

enum SmartPlaylistOperation {
    Create {
        name: String,
        definition: SmartPlaylistDefinition,
    },
    Update {
        id: SmartPlaylistId,
        name: String,
        definition: SmartPlaylistDefinition,
    },
    Delete(SmartPlaylistId),
    Restore(SmartPlaylistBuiltin),
    Move {
        dragged: SmartPlaylistId,
        target: SmartPlaylistId,
        after: bool,
    },
}

enum WorkPurpose {
    Add,
    Select {
        target: SourceId,
    },
    Refresh {
        qualifier: SourceQualifier,
        visible: bool,
    },
    Update {
        selected: bool,
        progress_source: Option<SourceId>,
    },
    Selected {
        qualifier: SourceQualifier,
        automatic: bool,
    },
}

enum PreparedWork {
    Replacement(PreparedReplacement),
    SelectedUpdate(PreparedSelectedUpdate),
    Configuration {
        configured: ConfiguredSource,
        configuration: Option<SourceConfiguration>,
    },
    InactiveConnection {
        configured: ConfiguredSource,
        configuration: SourceConfiguration,
        credential: Option<String>,
        replaces_account: bool,
    },
    Refresh(PreparedSourceCandidate),
    Point(PointPrepared),
}

#[derive(Clone, Copy)]
enum ReplacementReason {
    Add,
    Select {
        cached: bool,
        refresh_after_select: bool,
    },
    DifferentAccount,
}

enum ReplacementLibrary {
    Cached(Arc<LoadedLibrary>),
    Candidate(Box<PreparedSourceCandidate>),
}

struct PreparedReplacement {
    reason: ReplacementReason,
    previous: Option<ConfiguredSource>,
    configuration: SourceConfiguration,
    source: Option<Arc<Source>>,
    credential: Option<String>,
    library: ReplacementLibrary,
}

struct PreparedSelectedUpdate {
    configured: ConfiguredSource,
    configuration: SourceConfiguration,
    source: Arc<Source>,
    credential: Option<String>,
    candidate: Option<Box<PreparedSourceCandidate>>,
}

struct PreparedSameSource {
    selected: SelectedSourceRuntime,
    playback: Option<(Arc<PlaybackOwner>, crate::playback::PreparedTrackRefresh)>,
}

struct ActiveWork {
    token: u64,
    purpose: WorkPurpose,
    activity_updates: Vec<RecordedActivity>,
    cancelled: Arc<AtomicBool>,
    handle: JoinHandle<Result<PreparedWork, String>>,
}

enum PointPrepared {
    RefreshHome {
        kind: HomeSectionKind,
        source_section: Option<SourceHomeSection>,
    },
    Favorite {
        acceptance: FavoriteAcceptance,
        item: FavoriteItemId,
        previous: bool,
    },
    FavoriteFailed {
        item: FavoriteItemId,
        previous: bool,
        message: String,
    },
    Playlist(PlaylistAcceptance),
    SmartPlaylist(SmartPlaylistOperation),
    LibraryUpdate(PreparedLibraryUpdate),
    Metadata {
        reply: MetadataReply,
        acceptance: Result<PreparedLibraryUpdate, MetadataError>,
    },
    RefreshRequired,
    Unchanged,
}

enum PreparedLibraryUpdate {
    Source(library::SourceLibraryUpdate),
    Local(library::LocalComponentReplacement),
    Full(PreparedSourceCandidate),
}

struct ActiveObserver {
    qualifier: SourceQualifier,
    cancelled: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Drop for ActiveObserver {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.handle.abort();
    }
}

struct ActiveLocalAccess {
    token: u64,
    qualifier: SourceQualifier,
    input: SourceLocalAccess,
    cancelled: Arc<AtomicBool>,
    handle: JoinHandle<Result<Vec<library::LocalAccessFile>, String>>,
}

struct ActiveAlbumRelease {
    token: u64,
    cancelled: Arc<AtomicBool>,
}

struct Actor {
    shared: Arc<Shared>,
    sender: Sender<Message>,
    active: Option<ActiveWork>,
    observer: Option<ActiveObserver>,
    local_access: Option<ActiveLocalAccess>,
    pending: VecDeque<WorkRequest>,
    next_freshness_check: tokio::time::Instant,
    fallback: Option<SourceId>,
    selected_revealed: bool,
    active_album_release: Option<ActiveAlbumRelease>,
}

impl SourceOwner {
    pub(crate) fn open_dormant(
        artwork: Artwork,
        library: Library,
        downloads: Downloads,
        settings: SettingsFile,
        secrets: Arc<SwitchableSecretStore>,
        scrobbler: Arc<Scrobbler>,
        runtime: tokio::runtime::Handle,
        outputs: SourceOutputs,
    ) -> SourceBootstrap {
        let stored = settings.load();
        let operation = match stored.sources.selected_source_id.clone() {
            Some(target) => SourceOperation::Switching {
                target,
                progress: initial_progress(),
            },
            None => SourceOperation::Idle,
        };
        let (messages, receiver) = async_channel::unbounded();
        let shared = Arc::new(Shared {
            artwork,
            library,
            downloads,
            settings,
            secrets,
            scrobbler,
            runtime,
            outputs,
            selected: RwLock::new(None),
            playback: Mutex::new(Weak::new()),
            next_epoch: AtomicU64::new(1),
            next_work: AtomicU64::new(1),
        });
        let owner = Arc::new(Self {
            shared,
            messages,
            receiver: Mutex::new(Some(receiver)),
        });
        SourceBootstrap {
            configured: configured_sources(&stored, None),
            operation,
            owner,
        }
    }

    pub(crate) fn attach_playback(&self, playback: &Arc<PlaybackOwner>) {
        *self
            .shared
            .playback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::downgrade(playback);
    }

    pub(crate) fn acceptance_sender(&self) -> SourceAcceptanceSender {
        SourceAcceptanceSender {
            messages: self.messages.clone(),
        }
    }

    pub(crate) fn start(&self) -> Result<(), String> {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| "the source owner is already running".to_string())?;
        let actor = Actor {
            shared: Arc::clone(&self.shared),
            sender: self.messages.clone(),
            active: None,
            observer: None,
            local_access: None,
            pending: VecDeque::new(),
            next_freshness_check: tokio::time::Instant::now(),
            fallback: None,
            selected_revealed: false,
            active_album_release: None,
        };
        self.shared.runtime.spawn(actor.run(receiver));
        if let Some(source_id) = self.shared.settings.load().sources.selected_source_id {
            self.send(Message::Request(WorkRequest::Select(source_id)));
        }
        Ok(())
    }

    pub(crate) fn selected(&self) -> Option<SelectedSourceRuntime> {
        self.shared.selected()
    }

    fn selected_metadata_context(
        &self,
        source_id: &SourceId,
        source_session_epoch: SourceSessionEpoch,
        item_id: &MetadataItemId,
    ) -> Option<MetadataContext> {
        self.selected()
            .filter(|selected| {
                selected.source_id() == source_id
                    && selected.source_session_epoch == source_session_epoch
            })?
            .metadata_context(item_id)
            .ok()
            .flatten()
    }

    pub(crate) fn album_release_settings_changed(&self, enabled: bool) {
        self.send(Message::AlbumReleaseSettingChanged(enabled));
    }

    pub(crate) fn download_settings_changed(&self) {
        self.send(Message::DownloadSettingsChanged);
    }

    fn send_selected(&self, operation: impl FnOnce(&SelectedSourceRuntime) -> PointOperation) {
        let Some(selected) = self.selected() else {
            return;
        };
        self.send(Message::Request(WorkRequest::Selected {
            qualifier: selected.qualifier(),
            operation: operation(&selected),
        }));
    }

    fn send(&self, message: Message) {
        if self.messages.try_send(message).is_err() {
            warn!("source operation lane is unavailable");
        }
    }
}

impl SourceAcceptanceSender {
    pub(crate) fn publish_activity(
        &self,
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        update: RecordedActivity,
        played_track: Option<library::TrackId>,
    ) {
        let next_home = played_track.map_or(NextHome::ActivityKeep, NextHome::AcceptedPlay);
        if self
            .messages
            .try_send(Message::Activity {
                qualifier: SourceQualifier {
                    source_id,
                    epoch: source_session_epoch,
                },
                update,
                next_home,
            })
            .is_err()
        {
            warn!("accepted activity publication is unavailable");
        }
    }
}

impl Actor {
    async fn run(mut self, receiver: Receiver<Message>) {
        let mut source_check = tokio::time::interval_at(
            tokio::time::Instant::now() + SOURCE_CHECK_INTERVAL,
            SOURCE_CHECK_INTERVAL,
        );
        source_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let message = tokio::select! {
                message = receiver.recv() => match message {
                    Ok(message) => message,
                    Err(_) => break,
                },
                _ = source_check.tick() => {
                    self.queue_freshness_check();
                    self.start_next_work().await;
                    continue;
                }
            };
            match message {
                Message::Request(request) => self.queue_work(request).await,
                Message::SelectedLibraryRevealed => {
                    self.selected_revealed = true;
                    self.start_album_release_lookup();
                }
                Message::AlbumReleaseSettingChanged(enabled) => {
                    if enabled {
                        self.start_album_release_lookup();
                    } else {
                        self.cancel_album_release_lookup(false);
                    }
                }
                Message::DownloadSettingsChanged => {
                    if let Some(selected) = self.shared.selected() {
                        let directory = self
                            .shared
                            .settings
                            .load()
                            .ui
                            .download_directory(selected.source_id());
                        self.shared
                            .downloads
                            .set_directory(selected.source_id().clone(), directory);
                        self.reconcile_downloads(&selected).await;
                    }
                }
                Message::Download(request) => self.download(request).await,
                Message::RemoveDownload(request) => self.remove_download(request),
                Message::RemoveDownloadRule {
                    source_id,
                    rule,
                    delete_downloads,
                } => self.remove_download_rule(source_id, rule, delete_downloads),
                Message::CancelDownload { source_id, job_id } => {
                    self.shared.downloads.cancel(source_id, job_id);
                }
                Message::MoveDownload {
                    source_id,
                    job_id,
                    target_job_id,
                    after,
                } => {
                    self.shared
                        .downloads
                        .move_job(source_id, job_id, target_job_id, after);
                }
                Message::ClearDownloads(source_id) => self.clear_downloads(source_id),
                Message::AlbumReleaseFinished(token) => {
                    if self
                        .active_album_release
                        .as_ref()
                        .is_some_and(|active| active.token == token)
                    {
                        self.active_album_release = None;
                    }
                }
                Message::Activity {
                    qualifier,
                    update,
                    next_home,
                } => {
                    if let Some(selected) = self
                        .shared
                        .selected()
                        .filter(|selected| selected.qualifier() == qualifier)
                    {
                        if let Some(active) = self
                            .active
                            .as_mut()
                            .filter(|active| work_accepts_activity(&active.purpose, &qualifier))
                        {
                            active.activity_updates.push(update.clone());
                        }
                        let library = self.shared.library.clone();
                        let loaded = Arc::clone(&selected.loaded);
                        match blocking(move || {
                            library
                                .apply_recorded_activity(&loaded, &update)
                                .map_err(string_error)
                        })
                        .await
                        {
                            Ok(Some(change)) => {
                                self.publish_change(&selected, change, next_home).await;
                            }
                            Ok(None) => {}
                            Err(error) => {
                                warn!(%error, "could not apply accepted playback activity");
                            }
                        }
                    }
                }
                Message::Progress { token, progress } => {
                    self.publish_progress(token, progress).await;
                }
                Message::LocalAccessReady(token) => {
                    self.finish_local_access(token).await;
                }
                Message::WorkReady(token) => self.finish_work(token).await,
            }
        }
        self.cancel_all_work().await;
    }

    async fn queue_work(&mut self, request: WorkRequest) {
        match request {
            WorkRequest::Refresh { qualifier, visible } => {
                self.queue_refresh(qualifier.source_id, visible).await;
                return;
            }
            WorkRequest::Selected {
                qualifier,
                operation,
            } => {
                self.queue_selected(qualifier, operation).await;
                return;
            }
            WorkRequest::Configure(input) => {
                self.shared
                    .send_event(SourceEvent::Operation(SourceOperation::Adding {
                        progress: initial_progress(),
                    }))
                    .await;
                self.queue_transition(WorkRequest::Configure(input)).await;
                return;
            }
            WorkRequest::AddLocalFolder(path) if self.local_configuration().is_none() => {
                self.shared
                    .send_event(SourceEvent::Operation(SourceOperation::Adding {
                        progress: initial_progress(),
                    }))
                    .await;
                self.queue_transition(WorkRequest::Configure(SourceSetup::Local {
                    roots: vec![path],
                }))
                .await;
                return;
            }
            WorkRequest::Select(source_id) => {
                if self.active.is_none()
                    && self
                        .shared
                        .selected()
                        .is_some_and(|selected| selected.source_id() == &source_id)
                {
                    self.shared
                        .send_event(SourceEvent::Operation(SourceOperation::Idle))
                        .await;
                    return;
                }
                self.shared
                    .send_event(SourceEvent::Operation(SourceOperation::Switching {
                        target: source_id.clone(),
                        progress: initial_progress(),
                    }))
                    .await;
                self.queue_transition(WorkRequest::Select(source_id)).await;
                return;
            }
            request => self.pending.push_back(request),
        }
        self.start_next_work().await;
    }

    async fn queue_transition(&mut self, request: WorkRequest) {
        self.pending.retain(|pending| {
            !matches!(
                pending,
                WorkRequest::Configure(_) | WorkRequest::Select(_) | WorkRequest::Refresh { .. }
            ) && !matches!(
                pending,
                WorkRequest::Selected {
                    operation,
                    ..
                } if automatic_point(operation)
            )
        });
        let waits_for_user_operation = self.active.as_ref().is_some_and(|active| {
            matches!(
                active.purpose,
                WorkPurpose::Selected {
                    automatic: false,
                    ..
                }
            )
        });
        if self.active.is_some() && !waits_for_user_operation {
            self.cancel_work().await;
        }
        self.retire_selected_session().await;
        let mut user_operations = VecDeque::new();
        let mut remaining = VecDeque::new();
        while let Some(pending) = self.pending.pop_front() {
            if matches!(
                &pending,
                WorkRequest::Selected {
                    operation,
                    ..
                } if !automatic_point(operation)
            ) {
                user_operations.push_back(pending);
            } else {
                remaining.push_back(pending);
            }
        }
        user_operations.push_back(request);
        user_operations.append(&mut remaining);
        self.pending = user_operations;
        self.start_next_work().await;
    }

    async fn queue_refresh(&mut self, source_id: SourceId, visible: bool) {
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
        else {
            return;
        };
        let qualifier = selected.qualifier();
        if let Some(active) = self.active.as_mut()
            && let WorkPurpose::Refresh {
                qualifier: active_qualifier,
                visible: active_visible,
            } = &mut active.purpose
            && *active_qualifier == qualifier
        {
            if visible && !*active_visible {
                *active_visible = true;
                self.shared
                    .send_event(SourceEvent::Operation(SourceOperation::Refreshing {
                        source_id,
                        progress: initial_progress(),
                    }))
                    .await;
            }
            return;
        }
        if let Some(WorkRequest::Refresh {
            qualifier: _,
            visible: pending_visible,
        }) = self.pending.iter_mut().find(|request| {
            matches!(
                request,
                WorkRequest::Refresh {
                    qualifier: pending,
                    ..
                } if pending == &qualifier
            )
        }) {
            *pending_visible |= visible;
            return;
        }
        if visible {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Refreshing {
                    source_id: source_id.clone(),
                    progress: initial_progress(),
                }))
                .await;
        }
        self.pending
            .push_back(WorkRequest::Refresh { qualifier, visible });
        self.start_next_work().await;
    }

    async fn queue_selected(&mut self, qualifier: SourceQualifier, operation: PointOperation) {
        if !self.shared.matches_selected(&qualifier) {
            return;
        }
        if let Some(WorkRequest::Selected {
            qualifier: pending_qualifier,
            operation: pending,
        }) = self.pending.back_mut()
            && *pending_qualifier == qualifier
            && merge_automatic_point(pending, &operation)
        {
            return;
        }
        self.pending.push_back(WorkRequest::Selected {
            qualifier,
            operation,
        });
        self.start_next_work().await;
    }

    async fn start_next_work(&mut self) {
        if self.active.is_some() {
            return;
        }
        while let Some(request) = self.pending.pop_front() {
            match request {
                WorkRequest::Configure(input) => {
                    self.begin_add(input).await;
                    return;
                }
                WorkRequest::Select(source_id) => {
                    self.begin_select(source_id).await;
                    if self.active.is_some() {
                        return;
                    }
                }
                update @ (WorkRequest::Update(_)
                | WorkRequest::AddLocalFolder(_)
                | WorkRequest::RemoveLocalFolder(_)) => {
                    if self.start_update(update).await {
                        return;
                    }
                }
                WorkRequest::Forget(source_id) => {
                    self.forget_now(source_id).await;
                }
                WorkRequest::Refresh { qualifier, visible } => {
                    if !self.shared.matches_selected(&qualifier) {
                        continue;
                    }
                    self.start_refresh(qualifier, visible).await;
                    if self.active.is_some() {
                        return;
                    }
                }
                WorkRequest::Selected {
                    qualifier,
                    operation,
                } => {
                    let Some(selected) = self
                        .shared
                        .selected()
                        .filter(|selected| selected.qualifier() == qualifier)
                    else {
                        continue;
                    };
                    let qualifier = selected.qualifier();
                    let automatic = automatic_point(&operation);
                    self.spawn_work(
                        WorkPurpose::Selected {
                            qualifier,
                            automatic,
                        },
                        move |shared, progress, cancelled| async move {
                            run_point(shared, selected, operation, progress, cancelled)
                                .await
                                .map(PreparedWork::Point)
                        },
                    );
                    return;
                }
                WorkRequest::ChangeSecretStorage { mode, result } => {
                    let changed = self.change_secret_storage(mode).await;
                    let _ = result.send(changed).await;
                }
                WorkRequest::SaveLocalAccess {
                    input,
                    metadata,
                    result,
                } => {
                    self.save_local_access(input, metadata, result).await;
                }
                WorkRequest::ClearLocalAccess(source_id) => {
                    self.clear_local_access(source_id).await;
                }
                WorkRequest::SetMusicFolder {
                    source_id,
                    folder_id,
                } => self.set_music_folder(source_id, folder_id).await,
                WorkRequest::CheckSelectedSource => self.queue_freshness_check(),
            }
            if self.active.is_some() {
                return;
            }
        }
    }

    async fn begin_add(&mut self, input: SourceSetup) {
        self.shared.release_selected().await;
        self.spawn_work(WorkPurpose::Add, move |shared, progress, cancelled| {
            prepare_add(shared, input, progress, cancelled)
        });
    }

    async fn begin_select(&mut self, target: SourceId) {
        self.shared.release_selected().await;
        let configured = match configured_source(&self.shared.settings.load().sources, &target) {
            Ok(configured) => configured,
            Err(error) => {
                self.fail_transition(Some(target), error, false).await;
                return;
            }
        };
        self.spawn_work(
            WorkPurpose::Select {
                target: target.clone(),
            },
            move |shared, progress, cancelled| async move {
                prepare_select(shared, configured, progress, cancelled)
                    .await
                    .map(PreparedWork::Replacement)
            },
        );
    }

    async fn change_secret_storage(&mut self, mode: SecretStorageMode) -> Result<(), String> {
        let previous = self.shared.settings.load();
        if previous.ui.secret_storage_mode == mode {
            return Ok(());
        }
        let transition_source_id = self
            .shared
            .selected()
            .filter(|selected| {
                matches!(
                    selected.configuration.editable(),
                    Ok(sources::EditableSource::Credentials { .. })
                )
            })
            .map(|selected| selected.source_id().clone());
        if let Some(source_id) = transition_source_id.as_ref() {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Switching {
                    target: source_id.clone(),
                    progress: initial_progress(),
                }))
                .await;
            self.begin_transition().await;
        }

        let keys = all_secret_keys(&previous);
        let settings = self.shared.settings.clone();
        let changed = blocking(move || {
            let scope = fresh_secret_scope_id()?;
            settings.update(|stored| {
                stored.ui.secret_storage_mode = mode;
                stored.secret_scope_id = scope;
                for descriptor in scrobbling::secret_descriptors() {
                    descriptor.value_mut(&mut stored.scrobbling).clear();
                }
                Ok(stored.clone())
            })
        })
        .await;
        let changed = match changed {
            Ok(changed) => changed,
            Err(error) => {
                if transition_source_id.is_some() {
                    self.fail_transition(transition_source_id, error.clone(), false)
                        .await;
                }
                return Err(error);
            }
        };

        let previous_secrets = self.shared.secrets.replace(platform_secret_store(&changed));
        let _ = blocking(move || {
            for key in keys {
                if let Err(error) = previous_secrets.delete_secret(&key) {
                    warn!(%error, ?key, "failed to remove a secret from the previous backend");
                }
            }
            Ok(())
        })
        .await;

        let scrobbling = load_scrobbling_settings(&self.shared.settings, &self.shared.secrets);
        if let Err(error) = self
            .shared
            .scrobbler
            .update_settings(scrobbling, changed.ui.private_mode)
        {
            warn!(%error, "could not clear external scrobbling accounts");
        }

        if transition_source_id.is_some() {
            self.restore_fallback().await;
            self.shared.publish_configured().await;
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Idle))
                .await;
        }
        Ok(())
    }

    async fn start_update(&mut self, update: WorkRequest) -> bool {
        let (source_id, input, local_roots_changed) = match update {
            WorkRequest::Update(input) => (
                source_settings_id(&input).clone(),
                source_settings_input(input),
                false,
            ),
            WorkRequest::AddLocalFolder(path) => {
                let local = self
                    .local_configuration()
                    .expect("a first Local folder enters the source-add transition");
                let source_id = local.configuration.source_id.clone();
                let mut roots = match local_roots(&local.configuration) {
                    Ok(roots) => roots,
                    Err(error) => {
                        self.shared.send_notice(error).await;
                        return false;
                    }
                };
                if roots.contains(&path) {
                    return false;
                }
                roots.push(path);
                (
                    source_id.clone(),
                    SourceSettingsInput::Local { roots },
                    true,
                )
            }
            WorkRequest::RemoveLocalFolder(path) => {
                let Some(local) = self.local_configuration() else {
                    self.shared
                        .send_notice("Local is not configured".to_string())
                        .await;
                    return false;
                };
                let source_id = local.configuration.source_id.clone();
                let mut roots = match local_roots(&local.configuration) {
                    Ok(roots) => roots,
                    Err(error) => {
                        self.shared.send_notice(error).await;
                        return false;
                    }
                };
                roots.retain(|root| root.to_string_lossy() != path);
                if roots.is_empty() {
                    self.forget_now(source_id).await;
                    return false;
                }
                (
                    source_id.clone(),
                    SourceSettingsInput::Local { roots },
                    true,
                )
            }
            _ => unreachable!("only configured source edits enter the update path"),
        };
        let configured = match configured_source(&self.shared.settings.load().sources, &source_id) {
            Ok(configured) => configured,
            Err(error) => {
                self.shared.send_notice(error).await;
                return false;
            }
        };
        let selected = self
            .shared
            .selected()
            .is_some_and(|selected| selected.source_id() == &source_id);
        let progress_source = (selected && local_roots_changed).then(|| source_id.clone());
        if local_roots_changed && let Some(source_id) = &progress_source {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Refreshing {
                    source_id: source_id.clone(),
                    progress: initial_progress(),
                }))
                .await;
        }
        self.spawn_work(
            WorkPurpose::Update {
                selected,
                progress_source,
            },
            move |shared, progress, cancelled| {
                prepare_update(shared, configured, input, selected, progress, cancelled)
            },
        );
        true
    }

    fn local_configuration(&self) -> Option<ConfiguredSource> {
        self.shared
            .settings
            .load()
            .sources
            .configured
            .iter()
            .find(|source| {
                matches!(
                    source.configuration.editable(),
                    Ok(sources::EditableSource::Local { .. })
                )
            })
            .cloned()
    }

    async fn publish_new_selected(
        &mut self,
        session: SelectedSourceSession,
        playback: PlaybackProjection,
        catch_up: bool,
    ) {
        self.shared.publish_selected(session, playback).await;
        self.start_selected_access(catch_up).await;
    }

    async fn start_selected_access(&mut self, catch_up: bool) {
        let Some(selected) = self.shared.selected() else {
            return;
        };
        self.reconcile_downloads(&selected).await;
        let qualifier = selected.qualifier();
        if self
            .observer
            .as_ref()
            .is_some_and(|observer| observer.qualifier == qualifier)
        {
            return;
        }
        self.stop_observer();
        self.start_local_access_refresh(&selected).await;
        self.next_freshness_check = tokio::time::Instant::now()
            + if catch_up {
                Duration::ZERO
            } else {
                SOURCE_CHECK_INTERVAL
            };
        let Some(source) = selected.source else {
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let item_cancelled = Arc::clone(&cancelled);
        let local_cancelled = Arc::clone(&cancelled);
        let stop_cancelled = Arc::clone(&cancelled);
        let item_sender = self.sender.clone();
        let local_sender = self.sender.clone();
        let item_qualifier = qualifier.clone();
        let local_qualifier = qualifier.clone();
        let handle = self.shared.runtime.spawn(async move {
            let result = source
                .listen_selected_changes(
                    catch_up,
                    move |change| {
                        !item_cancelled.load(Ordering::Acquire)
                            && item_sender
                                .try_send(Message::Request(WorkRequest::Selected {
                                    qualifier: item_qualifier.clone(),
                                    operation: PointOperation::ObservedItems(change),
                                }))
                                .is_ok()
                    },
                    move |change| {
                        !local_cancelled.load(Ordering::Acquire)
                            && local_sender
                                .try_send(Message::Request(WorkRequest::Selected {
                                    qualifier: local_qualifier.clone(),
                                    operation: PointOperation::LocalFiles(change),
                                }))
                                .is_ok()
                    },
                    move || stop_cancelled.load(Ordering::Acquire),
                )
                .await;
            if let Err(error) = result {
                warn!(%error, "selected source change feed stopped");
            }
        });
        self.observer = Some(ActiveObserver {
            qualifier,
            cancelled,
            handle,
        });
        if catch_up {
            self.queue_freshness_check();
        }
    }

    async fn download(&self, request: DownloadRequest) {
        let Some(selected) = self.shared.selected().filter(|selected| {
            selected.source_id() == &request.source_id
                && selected.source_session_epoch == request.source_session_epoch
        }) else {
            return;
        };
        let download_settings = self
            .shared
            .settings
            .load()
            .ui
            .download_settings(selected.source_id());
        let DownloadRequest {
            subject, tracks, ..
        } = request;
        let tracks = match blocking(move || {
            tracks
                .prepare()
                .and_then(|tracks| tracks.materialize_owned())
                .map_err(string_error)
        })
        .await
        {
            Ok(tracks) => tracks,
            Err(error) => {
                self.shared.send_notice(error).await;
                return;
            }
        };
        self.shared.downloads.download(
            selected.source.clone(),
            Arc::clone(&selected.loaded),
            download_settings.directory,
            subject,
            download_settings.quality,
            tracks,
        );
    }

    fn remove_download(&self, request: RemoveDownloadRequest) {
        let Some(selected) = self.shared.selected().filter(|selected| {
            selected.source_id() == &request.source_id
                && selected.source_session_epoch == request.source_session_epoch
        }) else {
            return;
        };
        self.shared.downloads.remove(
            selected.source_id().clone(),
            selected.loaded,
            vec![request.track_id],
            true,
        );
    }

    fn clear_downloads(&self, source_id: SourceId) {
        let loaded = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
            .map(|selected| selected.loaded);
        self.shared.downloads.clear(source_id, loaded, true);
    }

    fn remove_download_rule(
        &self,
        source_id: SourceId,
        rule: downloads::DownloadRule,
        delete_downloads: bool,
    ) {
        let loaded = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
            .map(|selected| selected.loaded);
        self.shared
            .downloads
            .remove_rule(source_id, loaded, rule, delete_downloads);
    }

    async fn reconcile_downloads(&self, selected: &SelectedSourceRuntime) {
        let settings = self
            .shared
            .settings
            .load()
            .ui
            .download_settings(selected.source_id());
        let rules = settings.rules;
        if rules.is_empty() {
            return;
        }
        let loaded = Arc::clone(&selected.loaded);
        let folder = selected.music_folder_id.clone();
        let queued = match blocking(move || {
            rules
                .active()
                .map(|rule| {
                    download_rule_tracks(&loaded, folder.as_ref(), rule)
                        .map(|tracks| (rule, tracks))
                })
                .collect::<library::LoadedLibraryResult<Vec<_>>>()
                .map_err(string_error)
        })
        .await
        {
            Ok(queued) => queued,
            Err(error) => {
                warn!(%error, source_id = %selected.source_id(), "could not prepare automatic downloads");
                return;
            }
        };
        for (rule, tracks) in queued {
            self.shared.downloads.reconcile_rule(
                selected.source.clone(),
                Arc::clone(&selected.loaded),
                settings.directory.clone(),
                rule,
                settings.quality,
                tracks,
            );
        }
    }

    fn stop_observer(&mut self) {
        self.observer.take();
    }

    async fn start_local_access_refresh(&mut self, selected: &SelectedSourceRuntime) {
        let Some(access) = self
            .shared
            .settings
            .load()
            .sources
            .configured
            .iter()
            .find(|configured| configured.configuration.source_id == *selected.source_id())
            .and_then(|configured| configured.local_access.clone())
        else {
            return;
        };
        let input = SourceLocalAccess {
            source_id: selected.source_id().clone(),
            root_path: access.root_path,
            server_prefix: access.server_prefix,
            local_prefix: access.local_prefix,
        };
        let baseline = match selected.loaded.local_access_files() {
            Ok(files) => files,
            Err(error) => {
                self.shared.send_notice(error.to_string()).await;
                return;
            }
        };
        self.cancel_local_access();
        let token = self.shared.next_work.fetch_add(1, Ordering::AcqRel);
        let qualifier = selected.qualifier();
        let task_input = input.clone();
        let task_baseline = baseline;
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_cancelled = Arc::clone(&cancelled);
        let sender = self.sender.clone();
        let handle = self.shared.runtime.spawn(async move {
            let root = task_input.root_path;
            let scan_cancelled = Arc::clone(&task_cancelled);
            let result = tokio::task::spawn_blocking(move || {
                sources::read_local_access(&root, &task_baseline, &|_| {}, &|| {
                    scan_cancelled.load(Ordering::Acquire)
                })
                .map_err(string_error)
            })
            .await
            .map_err(string_error)
            .and_then(|result| result);
            let _ = sender.send(Message::LocalAccessReady(token)).await;
            result
        });
        self.local_access = Some(ActiveLocalAccess {
            token,
            qualifier,
            input,
            cancelled,
            handle,
        });
    }

    fn cancel_local_access(&mut self) {
        let Some(active) = self.local_access.take() else {
            return;
        };
        active.cancelled.store(true, Ordering::Release);
        active.handle.abort();
    }

    async fn finish_local_access(&mut self, token: u64) {
        if self
            .local_access
            .as_ref()
            .is_none_or(|active| active.token != token)
        {
            return;
        }
        let active = self
            .local_access
            .take()
            .expect("the checked Local access task is present");
        let ActiveLocalAccess {
            qualifier,
            input,
            handle,
            ..
        } = active;
        let outcome = async {
            let files = handle
                .await
                .map_err(string_error)
                .and_then(|result| result)?;
            let selected = self
                .shared
                .selected()
                .filter(|selected| selected.qualifier() == qualifier)
                .ok_or_else(|| {
                    "the selected source changed before the local file mapping was ready"
                        .to_string()
                })?;
            let still_configured = self
                .shared
                .settings
                .load()
                .sources
                .configured
                .iter()
                .find(|configured| configured.configuration.source_id == input.source_id)
                .and_then(|configured| configured.local_access.as_ref())
                .is_some_and(|configured| {
                    configured.root_path == input.root_path
                        && configured.server_prefix == input.server_prefix
                        && configured.local_prefix == input.local_prefix
                });
            if !still_configured {
                return Err("the local file mapping changed before its scan finished".to_string());
            }
            let library = self.shared.library.clone();
            let loaded = Arc::clone(&selected.loaded);
            let mapping = local_access_mapping(&input);
            blocking(move || {
                library
                    .replace_local_access(&loaded, mapping, files)
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await?;
            if let Err(error) = self.shared.playback().and_then(|playback| {
                playback.stream_inputs_changed(selected.source_id(), selected.source_session_epoch)
            }) {
                warn!(%error, "could not update prepared playback after Local access changed");
            }
            self.shared.publish_configured().await;
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            self.shared.send_notice(error).await;
        }
    }

    fn queue_freshness_check(&mut self) {
        let now = tokio::time::Instant::now();
        if now < self.next_freshness_check {
            return;
        }
        self.next_freshness_check = now + SOURCE_CHECK_INTERVAL;
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.source.is_some())
        else {
            return;
        };
        let qualifier = selected.qualifier();
        if self.pending.iter().any(|request| {
            matches!(
                request,
                WorkRequest::Selected {
                    qualifier: pending,
                    operation: PointOperation::CheckFreshness,
                } if pending == &qualifier
            )
        }) {
            return;
        }
        self.pending.push_back(WorkRequest::Selected {
            qualifier,
            operation: PointOperation::CheckFreshness,
        });
    }

    async fn start_refresh(&mut self, qualifier: SourceQualifier, visible: bool) {
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.qualifier() == qualifier)
        else {
            return;
        };
        self.spawn_work(
            WorkPurpose::Refresh { qualifier, visible },
            move |shared, progress, cancelled| {
                prepare_refresh(shared, selected, progress, cancelled)
            },
        );
    }

    async fn begin_transition(&mut self) {
        self.retire_selected_session().await;
        self.shared.release_selected().await;
    }

    async fn retire_selected_session(&mut self) {
        self.cancel_album_release_lookup(true);
        if self.fallback.is_none() {
            self.fallback = self
                .shared
                .selected()
                .map(|selected| selected.source_id().clone());
        }
        self.shared.stop_playback().await;
        self.stop_observer();
        self.cancel_local_access();
        self.pending.retain(|request| match request {
            WorkRequest::Refresh { .. } => false,
            WorkRequest::Selected { operation, .. } if automatic_point(operation) => false,
            _ => true,
        });
    }

    async fn cancel_all_work(&mut self) {
        self.cancel_album_release_lookup(true);
        self.stop_observer();
        self.cancel_local_access();
        self.cancel_work().await;
    }

    fn start_album_release_lookup(&mut self) {
        if !self.selected_revealed
            || self.active_album_release.is_some()
            || !self
                .shared
                .settings
                .load()
                .ui
                .allows_external_metadata_lookup()
        {
            return;
        }
        let Some(session) = self.shared.selected_session() else {
            return;
        };
        let selected = session.snapshot();
        let token = self.shared.next_work.fetch_add(1, Ordering::AcqRel);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.active_album_release = Some(ActiveAlbumRelease {
            token,
            cancelled: Arc::clone(&cancelled),
        });
        let library = self.shared.library.clone();
        let settings = self.shared.settings.clone();
        let events = self.shared.outputs.events.clone();
        let source_id = selected.source_id().clone();
        let source_session_epoch = selected.source_session_epoch;
        let selected = session.downgrade();
        let sender = self.sender.clone();
        drop(self.shared.runtime.spawn_blocking(move || {
            run_selected_album_release_lookup(
                library,
                settings,
                events,
                source_id,
                source_session_epoch,
                selected,
                cancelled,
            );
            let _ = sender.try_send(Message::AlbumReleaseFinished(token));
        }));
    }

    fn cancel_album_release_lookup(&mut self, reset_reveal: bool) {
        if let Some(active) = self.active_album_release.take() {
            active.cancelled.store(true, Ordering::Release);
        }
        if reset_reveal {
            self.selected_revealed = false;
        }
    }

    async fn cancel_work(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        active.cancelled.store(true, Ordering::Release);
        active.handle.abort();
        let _ = active.handle.await;
    }

    fn spawn_work<F, Work>(&mut self, purpose: WorkPurpose, prepare: F)
    where
        F: FnOnce(
                Arc<Shared>,
                Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
                Arc<AtomicBool>,
            ) -> Work
            + Send
            + 'static,
        Work: Future<Output = Result<PreparedWork, String>> + Send + 'static,
    {
        assert!(
            self.active.is_none(),
            "the selected source work lane is already occupied"
        );
        let token = self.shared.next_work.fetch_add(1, Ordering::AcqRel);
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_cancelled = Arc::clone(&cancelled);
        let shared = Arc::clone(&self.shared);
        let sender = self.sender.clone();
        let progress_sender = self.sender.clone();
        let progress = Arc::new(move |progress| {
            let _ = progress_sender.try_send(Message::Progress { token, progress });
        });
        let handle = self.shared.runtime.spawn(async move {
            let result = prepare(shared, progress, task_cancelled).await;
            let _ = sender.send(Message::WorkReady(token)).await;
            result
        });
        self.active = Some(ActiveWork {
            token,
            purpose,
            activity_updates: Vec::new(),
            cancelled,
            handle,
        });
    }

    async fn publish_progress(&self, token: u64, progress: SourceReadProgress) {
        let Some(active) = self.active.as_ref().filter(|active| active.token == token) else {
            return;
        };
        let operation = match &active.purpose {
            WorkPurpose::Add => SourceOperation::Adding {
                progress: source_progress(progress),
            },
            WorkPurpose::Select { target } => SourceOperation::Switching {
                target: target.clone(),
                progress: source_progress(progress),
            },
            WorkPurpose::Refresh {
                qualifier,
                visible: true,
                ..
            } => SourceOperation::Refreshing {
                source_id: qualifier.source_id.clone(),
                progress: source_progress(progress),
            },
            WorkPurpose::Refresh { visible: false, .. } | WorkPurpose::Selected { .. } => return,
            WorkPurpose::Update {
                progress_source: Some(source_id),
                ..
            } => SourceOperation::Refreshing {
                source_id: source_id.clone(),
                progress: source_progress(progress),
            },
            WorkPurpose::Update {
                progress_source: None,
                ..
            } => return,
        };
        self.shared
            .send_event(SourceEvent::Operation(operation))
            .await;
    }

    async fn finish_work(&mut self, token: u64) {
        let Some(active) = self.active.take().filter(|active| active.token == token) else {
            return;
        };
        let purpose = active.purpose;
        let activity_updates = active.activity_updates;
        let result = active
            .handle
            .await
            .map_err(string_error)
            .and_then(|result| result);
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.target_failed(purpose, error).await;
                self.start_next_work().await;
                return;
            }
        };
        match (purpose, prepared) {
            (
                WorkPurpose::Add | WorkPurpose::Select { .. },
                PreparedWork::Replacement(prepared),
            ) => {
                let add_form = matches!(prepared.reason, ReplacementReason::Add);
                let source_id = (!add_form).then(|| prepared.configuration.source_id.clone());
                if let Err(error) = self.commit_replacement(prepared).await {
                    self.fail_transition(source_id, error, add_form).await;
                }
            }
            (
                WorkPurpose::Update { selected: true, .. },
                PreparedWork::SelectedUpdate(prepared),
            ) => {
                match self
                    .commit_selected_update(prepared, activity_updates)
                    .await
                {
                    Ok(()) => {
                        self.stop_observer();
                        self.cancel_local_access();
                        self.start_selected_access(true).await;
                        self.shared
                            .send_event(SourceEvent::Operation(SourceOperation::Idle))
                            .await;
                    }
                    Err(error) => self.selected_update_failed(error).await,
                }
            }
            (WorkPurpose::Refresh { qualifier, visible }, PreparedWork::Refresh(prepared)) => {
                if let Some(selected) = self
                    .shared
                    .selected()
                    .filter(|selected| selected.qualifier() == qualifier)
                {
                    if let Err(error) = self
                        .commit_refresh(selected.clone(), prepared, visible, activity_updates)
                        .await
                    {
                        self.refresh_failed(&selected, visible, error).await;
                    }
                }
            }
            (
                WorkPurpose::Update {
                    selected,
                    progress_source,
                },
                PreparedWork::Configuration {
                    configured,
                    configuration,
                },
            ) => {
                self.commit_configuration(configured, selected, configuration)
                    .await;
                if progress_source.is_some() {
                    self.shared
                        .send_event(SourceEvent::Operation(SourceOperation::Idle))
                        .await;
                }
            }
            (
                WorkPurpose::Update {
                    selected: false, ..
                },
                PreparedWork::InactiveConnection {
                    configured,
                    configuration,
                    credential,
                    replaces_account,
                },
            ) => {
                self.commit_inactive_connection(
                    configured,
                    configuration,
                    credential,
                    replaces_account,
                )
                .await;
            }
            (WorkPurpose::Selected { qualifier, .. }, PreparedWork::Point(prepared)) => {
                if self.shared.matches_selected(&qualifier) {
                    self.commit_point(qualifier, prepared, activity_updates)
                        .await;
                }
            }
            _ => unreachable!("source preparation returned the wrong operation result"),
        }
        self.start_next_work().await;
    }

    async fn commit_replacement(&mut self, replacement: PreparedReplacement) -> Result<(), String> {
        let PreparedReplacement {
            reason,
            previous,
            configuration,
            source,
            credential,
            library,
        } = replacement;
        let refresh_after_select = matches!(
            reason,
            ReplacementReason::Select {
                refresh_after_select: true,
                ..
            }
        ) && source.is_some();
        let selected_source_id = configuration.source_id.clone();
        let loaded = self.accept_replacement_library(library).await?;
        if matches!(reason, ReplacementReason::DifferentAccount)
            || matches!(reason, ReplacementReason::Add) && previous.is_none()
        {
            let library = self.shared.library.clone();
            let loaded = Arc::clone(&loaded);
            blocking(move || {
                library
                    .initialize_smart_playlists(&loaded)
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await?;
        }
        let music_folder_id = match reason {
            ReplacementReason::DifferentAccount => None,
            _ => previous
                .as_ref()
                .and_then(|configured| configured.music_folder_id.clone()),
        };
        let local_access = previous
            .as_ref()
            .and_then(|configured| configured.local_access.clone());
        let selected = self
            .prepare_runtime(
                configuration.clone(),
                source,
                loaded,
                music_folder_id.clone(),
                local_access.clone(),
            )
            .await?;
        let staged_credential = if matches!(reason, ReplacementReason::Select { .. }) {
            None
        } else {
            self.stage_credential(credential).await?
        };
        let previous_credential = previous
            .as_ref()
            .and_then(|configured| configured.credential_ref.clone());
        let credential_ref = match reason {
            ReplacementReason::DifferentAccount => staged_credential.clone(),
            _ => staged_credential
                .clone()
                .or_else(|| previous_credential.clone()),
        };
        let mut configured = ConfiguredSource {
            configuration,
            credential_ref,
            music_folder_id,
            local_access,
        };
        let replaced_source_id = matches!(reason, ReplacementReason::DifferentAccount).then(|| {
            previous
                .as_ref()
                .expect("an account replacement has a previous source")
                .configuration
                .source_id
                .clone()
        });
        if replaced_source_id.is_some() {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Switching {
                    target: configured.configuration.source_id.clone(),
                    progress: initial_progress(),
                }))
                .await;
        }
        match reason {
            ReplacementReason::Select { .. } => {}
            ReplacementReason::Add => {}
            ReplacementReason::DifferentAccount => self.begin_transition().await,
        }
        let result: Result<(), String> = async {
            let (session, playback) = self.install_runtime(selected).await?;
            let selected = session.snapshot();
            configured.music_folder_id = selected.music_folder_id.clone();
            if let Some(previous) = replaced_source_id.as_ref() {
                self.save_selected_account_replacement(previous, configured.clone())
                    .await?;
            } else {
                self.save_selected_configuration(configured.clone()).await?;
            }
            let catch_up = matches!(
                reason,
                ReplacementReason::Select {
                    cached: true,
                    refresh_after_select: false,
                }
            );
            self.publish_new_selected(session, playback, catch_up).await;
            self.fallback = None;
            if let Some(previous) = replaced_source_id {
                self.remove_replaced_source_data(previous).await;
            }
            if refresh_after_select {
                self.queue_refresh(selected_source_id, true).await;
            } else {
                self.shared
                    .send_event(SourceEvent::Operation(SourceOperation::Idle))
                    .await;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.shared.stop_playback().await;
            self.delete_staged_credential(staged_credential.as_ref())
                .await;
            return Err(error);
        }
        if previous_credential != configured.credential_ref {
            self.delete_staged_credential(previous_credential.as_ref())
                .await;
        }
        Ok(())
    }

    async fn accept_replacement_library(
        &self,
        library: ReplacementLibrary,
    ) -> Result<Arc<LoadedLibrary>, String> {
        Ok(match library {
            ReplacementLibrary::Cached(loaded) => loaded,
            ReplacementLibrary::Candidate(candidate) => {
                blocking(move || {
                    candidate
                        .accept()
                        .map(|commit| commit.loaded)
                        .map_err(string_error)
                })
                .await?
            }
        })
    }

    async fn commit_selected_update(
        &mut self,
        update: PreparedSelectedUpdate,
        activity_updates: Vec<RecordedActivity>,
    ) -> Result<(), String> {
        let PreparedSelectedUpdate {
            configured,
            configuration,
            source,
            credential,
            candidate,
        } = update;
        let previous = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &configuration.source_id)
            .ok_or_else(|| "the selected source is no longer active".to_string())?;
        let previous_credential = configured.credential_ref.clone();
        let staged_credential = self.stage_credential(credential).await?;
        let replacement = ConfiguredSource {
            configuration: configuration.clone(),
            credential_ref: staged_credential
                .clone()
                .or_else(|| previous_credential.clone()),
            music_folder_id: configured.music_folder_id.clone(),
            local_access: configured.local_access.clone(),
        };
        let result: Result<(), String> = async {
            let prepared = match candidate {
                Some(candidate) => {
                    self.prepare_same_source_candidate(
                        &previous,
                        configuration.clone(),
                        Some(Arc::clone(&source)),
                        configured.music_folder_id,
                        configured.local_access,
                        *candidate,
                        activity_updates,
                    )
                    .await?
                }
                None => None,
            };
            if !self.shared.matches_selected(&previous.qualifier()) {
                return Err(
                    "the selected source changed while its settings were prepared".to_string(),
                );
            }
            self.save_selected_configuration(replacement.clone())
                .await?;
            match prepared {
                Some(prepared) => {
                    let publishes_configuration = prepared.playback.is_some();
                    self.publish_same_source_candidate(prepared).await?;
                    if !publishes_configuration {
                        self.shared.publish_configured().await;
                    }
                }
                None => {
                    let mut selected = previous.clone();
                    selected.configuration = configuration;
                    selected.source = Some(source);
                    if !self.shared.replace_selected(selected) {
                        return Err(
                            "the selected source changed while its settings were saved".to_string()
                        );
                    }
                    self.shared.publish_configured().await;
                }
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            self.delete_staged_credential(staged_credential.as_ref())
                .await;
            return result;
        }
        if previous_credential != replacement.credential_ref {
            self.delete_staged_credential(previous_credential.as_ref())
                .await;
        }
        Ok(())
    }

    async fn commit_refresh(
        &mut self,
        previous: SelectedSourceRuntime,
        prepared: PreparedSourceCandidate,
        visible: bool,
        activity_updates: Vec<RecordedActivity>,
    ) -> Result<(), String> {
        if !self.shared.matches_selected(&previous.qualifier()) {
            return Err(
                "the selected source changed before the refreshed library was accepted".to_string(),
            );
        }
        let stored = self.shared.settings.load();
        let configured = stored
            .sources
            .configured
            .iter()
            .find(|configured| configured.configuration.source_id == *previous.source_id());
        let requested_folder = configured.and_then(|configured| configured.music_folder_id.clone());
        let local_access =
            configured.and_then(|configured| configured.local_access.as_ref().cloned());
        let Some(prepared) = self
            .prepare_same_source_candidate(
                &previous,
                previous.configuration.clone(),
                previous.source.clone(),
                requested_folder.clone(),
                local_access,
                prepared,
                activity_updates,
            )
            .await?
        else {
            if visible && self.shared.matches_selected(&previous.qualifier()) {
                self.shared
                    .send_event(SourceEvent::Operation(SourceOperation::Idle))
                    .await;
            }
            return Ok(());
        };
        if prepared.selected.music_folder_id != requested_folder {
            let settings = self.shared.settings.clone();
            let source_id = previous.source_id().clone();
            let folder_for_settings = prepared.selected.music_folder_id.clone();
            if let Err(error) =
                blocking(move || save_music_folder(&settings, &source_id, folder_for_settings))
                    .await
            {
                warn!(
                    %error,
                    source_id = %previous.source_id(),
                    "could not save the normalized music folder"
                );
            }
        }
        if !self.shared.matches_selected(&prepared.selected.qualifier()) {
            return Err(
                "the selected source changed before the refreshed library was accepted".to_string(),
            );
        }
        let selected = self.publish_same_source_candidate(prepared).await?;
        self.start_local_access_refresh(&selected).await;
        self.shared
            .send_event(SourceEvent::Operation(SourceOperation::Idle))
            .await;
        Ok(())
    }

    async fn prepare_same_source_candidate(
        &self,
        previous: &SelectedSourceRuntime,
        configuration: SourceConfiguration,
        source: Option<Arc<Source>>,
        requested_folder: Option<MusicFolderId>,
        local_access: Option<ConfiguredLocalAccess>,
        candidate: PreparedSourceCandidate,
        activity_updates: Vec<RecordedActivity>,
    ) -> Result<Option<PreparedSameSource>, String> {
        let change = candidate.change();
        if change == CandidateChange::None {
            blocking(move || candidate.accept().map_err(string_error)).await?;
            return Ok(None);
        }
        let folder = normalize_music_folder(candidate.loaded(), requested_folder)?;
        if let Some(access) = local_access {
            let library = self.shared.library.clone();
            let loaded = Arc::clone(candidate.loaded());
            blocking(move || {
                library
                    .configure_local_access(&loaded, configured_local_access_mapping(&access))
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await?;
        }
        let playback = if change == CandidateChange::Library {
            let playback = self.shared.playback()?;
            let prepared = playback.prepare_track_refresh(previous.source_session_epoch)?;
            Some((playback, prepared))
        } else {
            None
        };
        let commit = blocking(move || candidate.accept().map_err(string_error)).await?;
        if change == CandidateChange::Library {
            replay_activity_updates(
                self.shared.library.clone(),
                Arc::clone(&commit.loaded),
                activity_updates,
            )
            .await?;
        }
        let downloads = self.shared.downloads.clone();
        let loaded_for_downloads = Arc::clone(&commit.loaded);
        let source_for_downloads = source.clone();
        let download_directory = self
            .shared
            .settings
            .load()
            .ui
            .download_directory(&configuration.source_id);
        downloads
            .attach(
                source_for_downloads,
                &loaded_for_downloads,
                download_directory,
            )
            .await?;
        let library = self.shared.library.clone();
        let loaded = Arc::clone(&commit.loaded);
        let home_folder = folder.clone();
        let home = blocking(move || {
            library
                .home(&loaded, home_folder.as_ref())
                .map_err(string_error)
        })
        .await?;
        Ok(Some(PreparedSameSource {
            selected: SelectedSourceRuntime {
                configuration,
                source,
                source_session_epoch: previous.source_session_epoch,
                loaded: commit.loaded,
                home,
                music_folder_id: folder,
            },
            playback,
        }))
    }

    async fn publish_same_source_candidate(
        &mut self,
        prepared: PreparedSameSource,
    ) -> Result<SelectedSourceRuntime, String> {
        let PreparedSameSource { selected, playback } = prepared;
        match playback {
            None => {
                self.shared.publish_home_replacement(selected.clone()).await;
            }
            Some((playback, prepared)) => {
                self.cancel_album_release_lookup(false);
                self.shared.publish_library_replacement(&selected).await;
                let loaded = Arc::clone(&selected.loaded);
                match blocking(move || playback.apply_track_refresh(prepared, &loaded)).await {
                    Ok(projection) => {
                        self.shared
                            .publish_selected_playback(&selected, projection)
                            .await;
                    }
                    Err(error) => {
                        warn!(%error, "could not update Playback after accepting refreshed source facts");
                        self.shared.send_notice(error).await;
                    }
                }
                self.start_album_release_lookup();
            }
        }
        self.reconcile_downloads(&selected).await;
        Ok(selected)
    }

    async fn refresh_failed(&self, selected: &SelectedSourceRuntime, visible: bool, error: String) {
        if visible {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Failed {
                    source_id: Some(selected.source_id().clone()),
                    message: error,
                    add_form: false,
                }))
                .await;
        } else {
            warn!(%error, "background source refresh failed");
        }
    }

    async fn commit_configuration(
        &mut self,
        mut configured: ConfiguredSource,
        selected: bool,
        configuration: Option<SourceConfiguration>,
    ) {
        let result = if let Some(configuration) = configuration {
            configured.configuration = configuration.clone();
            let result = self.save_configuration(configured).await;
            if result.is_ok()
                && selected
                && let Some(mut active) = self.shared.selected()
            {
                active.configuration = configuration;
                self.shared.replace_selected(active);
            }
            result
        } else {
            Ok(())
        };
        match result {
            Ok(()) => self.shared.publish_configured().await,
            Err(error) => self.shared.send_notice(error).await,
        }
        if selected {
            self.start_selected_access(true).await;
        }
    }

    async fn commit_inactive_connection(
        &self,
        configured: ConfiguredSource,
        configuration: SourceConfiguration,
        credential: Option<String>,
        replaces_account: bool,
    ) {
        let result = if replaces_account {
            self.replace_inactive_account(configured, configuration, credential)
                .await
        } else {
            self.save_inactive_connected(configured, configuration, credential)
                .await
        };
        match result {
            Ok(()) => self.shared.publish_configured().await,
            Err(error) => self.shared.send_notice(error).await,
        }
    }

    async fn selected_update_failed(&mut self, error: String) {
        self.shared.send_notice(error).await;
        self.shared
            .send_event(SourceEvent::Operation(SourceOperation::Idle))
            .await;
        self.start_selected_access(true).await;
    }

    async fn commit_point(
        &mut self,
        qualifier: SourceQualifier,
        prepared: PointPrepared,
        activity_updates: Vec<RecordedActivity>,
    ) {
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.qualifier() == qualifier)
        else {
            return;
        };
        match prepared {
            PointPrepared::RefreshHome {
                kind,
                source_section,
            } => {
                let library = self.shared.library.clone();
                let loaded = Arc::clone(&selected.loaded);
                let folder = selected.music_folder_id.clone();
                let current = Arc::clone(&selected.home);
                match blocking(move || {
                    match source_section {
                        Some(section) => {
                            library.accept_home_section(&loaded, folder.as_ref(), &current, section)
                        }
                        None => library.refresh_rufin_home_section(
                            &loaded,
                            folder.as_ref(),
                            &current,
                            kind,
                        ),
                    }
                    .map_err(string_error)
                })
                .await
                {
                    Ok(home) => {
                        if let Some(mut active) = self
                            .shared
                            .selected()
                            .filter(|active| active.qualifier() == qualifier)
                        {
                            active.home = Arc::clone(&home);
                            self.shared.replace_selected(active);
                        }
                        self.shared
                            .send_event(SourceEvent::Home(HomePublication {
                                source_id: qualifier.source_id,
                                source_session_epoch: qualifier.epoch,
                                kind,
                                home,
                            }))
                            .await;
                    }
                    Err(error) => self.shared.send_notice(error).await,
                }
            }
            PointPrepared::Favorite {
                acceptance,
                item,
                previous,
            } => {
                let library = self.shared.library.clone();
                let loaded = Arc::clone(&selected.loaded);
                match blocking(move || {
                    library
                        .accept_favorite(&loaded, acceptance)
                        .map_err(string_error)
                })
                .await
                {
                    Ok(accepted) => {
                        self.publish_change(&selected, accepted, NextHome::Favorite(item))
                            .await;
                    }
                    Err(error) => {
                        self.shared
                            .send_event(SourceEvent::FavoriteFailure(FavoriteFailure {
                                source_id: qualifier.source_id,
                                source_session_epoch: qualifier.epoch,
                                item_id: item,
                                authoritative_favorite: previous,
                                message: error,
                            }))
                            .await;
                    }
                }
            }
            PointPrepared::FavoriteFailed {
                item,
                previous,
                message,
            } => {
                self.shared
                    .send_event(SourceEvent::FavoriteFailure(FavoriteFailure {
                        source_id: qualifier.source_id,
                        source_session_epoch: qualifier.epoch,
                        item_id: item,
                        authoritative_favorite: previous,
                        message,
                    }))
                    .await;
            }
            PointPrepared::Playlist(acceptance) => {
                let library = self.shared.library.clone();
                let loaded = Arc::clone(&selected.loaded);
                match blocking(move || {
                    library
                        .accept_playlist(&loaded, acceptance)
                        .map_err(string_error)
                })
                .await
                {
                    Ok(Some(change)) => {
                        self.publish_change(&selected, change, NextHome::Keep).await
                    }
                    Ok(None) => {}
                    Err(error) => self.shared.send_notice(error).await,
                }
            }
            PointPrepared::SmartPlaylist(operation) => {
                let library = self.shared.library.clone();
                let loaded = Arc::clone(&selected.loaded);
                match blocking(move || match operation {
                    SmartPlaylistOperation::Create { name, definition } => library
                        .create_smart_playlist(&loaded, name, definition)
                        .map_err(string_error),
                    SmartPlaylistOperation::Update {
                        id,
                        name,
                        definition,
                    } => library
                        .update_smart_playlist(&loaded, id, name, definition)
                        .map_err(string_error),
                    SmartPlaylistOperation::Delete(id) => library
                        .delete_smart_playlist(&loaded, &id)
                        .map_err(string_error),
                    SmartPlaylistOperation::Restore(builtin) => library
                        .restore_builtin_smart_playlist(&loaded, builtin)
                        .map_err(string_error),
                    SmartPlaylistOperation::Move {
                        dragged,
                        target,
                        after,
                    } => library
                        .move_smart_playlist_relative(&loaded, dragged, target, after)
                        .map_err(string_error),
                })
                .await
                {
                    Ok(Some(change)) => {
                        self.publish_change(&selected, change, NextHome::Keep).await
                    }
                    Ok(None) => {}
                    Err(error) => self.shared.send_notice(error).await,
                }
            }
            PointPrepared::LibraryUpdate(update) => {
                if let Err(error) = self
                    .accept_prepared_library_update(selected, update, activity_updates)
                    .await
                {
                    warn!(%error, "could not accept a selected library update");
                }
            }
            PointPrepared::Metadata { reply, acceptance } => {
                let accepted = match acceptance {
                    Err(error) => Err(error),
                    Ok(update) => self
                        .accept_prepared_library_update(selected, update, activity_updates)
                        .await
                        .map_err(MetadataError::SavedRefreshFailed),
                };
                reply.finish(accepted);
            }
            PointPrepared::RefreshRequired => {
                self.queue_refresh(qualifier.source_id, false).await;
            }
            PointPrepared::Unchanged => {}
        }
    }

    async fn accept_prepared_library_update(
        &mut self,
        selected: SelectedSourceRuntime,
        update: PreparedLibraryUpdate,
        activity_updates: Vec<RecordedActivity>,
    ) -> Result<(), String> {
        match update {
            PreparedLibraryUpdate::Source(update) => {
                self.accept_source_update(&selected, update).await
            }
            PreparedLibraryUpdate::Local(replacement) => {
                self.accept_local_component(&selected, replacement).await
            }
            PreparedLibraryUpdate::Full(candidate) => {
                self.commit_refresh(selected, candidate, false, activity_updates)
                    .await
            }
        }
    }

    async fn accept_source_update(
        &mut self,
        selected: &SelectedSourceRuntime,
        update: library::SourceLibraryUpdate,
    ) -> Result<(), String> {
        let next_home = if update.albums.is_empty()
            && update.tracks.is_empty()
            && update.removed_tracks.is_empty()
        {
            NextHome::Keep
        } else {
            NextHome::SourceFacts
        };
        let library = self.shared.library.clone();
        let loaded = Arc::clone(&selected.loaded);
        let change = blocking(move || {
            library
                .accept_source_update(&loaded, update)
                .map_err(string_error)
        })
        .await?;
        if let Some(change) = change {
            self.publish_change(selected, change, next_home).await;
        }
        Ok(())
    }

    async fn accept_local_component(
        &mut self,
        selected: &SelectedSourceRuntime,
        replacement: library::LocalComponentReplacement,
    ) -> Result<(), String> {
        let library = self.shared.library.clone();
        let loaded = Arc::clone(&selected.loaded);
        let change = blocking(move || {
            library
                .accept_local_component(&loaded, replacement)
                .map_err(string_error)
        })
        .await?;
        if let Some(change) = change {
            self.publish_change(selected, change, NextHome::SourceFacts)
                .await;
        }
        Ok(())
    }

    async fn publish_change(
        &mut self,
        selected: &SelectedSourceRuntime,
        change: AcceptedLibraryChange,
        next_home: NextHome,
    ) {
        if !self.shared.matches_selected(&selected.qualifier()) {
            return;
        }
        let download_rules = self
            .shared
            .settings
            .load()
            .ui
            .download_rules(selected.source_id());
        let reconcile_downloads = should_reconcile_downloads(&change, &next_home, download_rules);
        let removed_downloads = change
            .tracks
            .iter()
            .filter(|replacement| replacement.track.is_none())
            .map(|replacement| replacement.id.clone())
            .collect::<Vec<_>>();
        if !removed_downloads.is_empty() {
            self.shared.downloads.remove(
                selected.source_id().clone(),
                Arc::clone(&selected.loaded),
                removed_downloads,
                false,
            );
        }
        let source_facts = matches!(&next_home, NextHome::SourceFacts);
        if source_facts {
            self.cancel_album_release_lookup(false);
        }
        let tracks = change
            .tracks
            .iter()
            .filter_map(|replacement| replacement.track.clone())
            .collect::<Vec<_>>();
        if !tracks.is_empty() {
            let projection = match self.shared.playback() {
                Ok(playback) => {
                    let source_id = selected.source_id().clone();
                    let epoch = selected.source_session_epoch;
                    blocking(move || playback.refresh_accepted_tracks(&source_id, epoch, tracks))
                        .await
                }
                Err(error) => Err(error),
            };
            match projection {
                Ok(projection) => {
                    self.shared
                        .publish_selected_playback(selected, projection)
                        .await;
                }
                Err(error) => {
                    self.shared.send_notice(error).await;
                }
            }
        }
        let home = match next_home {
            NextHome::Keep | NextHome::ActivityKeep => None,
            NextHome::Favorite(favorite) => {
                let library = self.shared.library.clone();
                let loaded = Arc::clone(&selected.loaded);
                let current = Arc::clone(&selected.home);
                let folder = selected.music_folder_id.clone();
                match blocking(move || {
                    library
                        .home_after_favorite(&loaded, folder.as_ref(), &current, &favorite)
                        .map_err(string_error)
                })
                .await
                {
                    Ok(home) => Some(home),
                    Err(error) => {
                        warn!(%error, source_id = %selected.source_id(), "could not update changed items in the next Home snapshot");
                        None
                    }
                }
            }
            NextHome::AcceptedPlay(track_id) => {
                let library = self.shared.library.clone();
                let loaded = Arc::clone(&selected.loaded);
                let current = Arc::clone(&selected.home);
                let folder = selected.music_folder_id.clone();
                match blocking(move || {
                    library
                        .home_after_play(&loaded, folder.as_ref(), &current, &track_id)
                        .map_err(string_error)
                })
                .await
                {
                    Ok(home) => Some(home),
                    Err(error) => {
                        warn!(%error, source_id = %selected.source_id(), "could not prepare Home after an accepted play");
                        None
                    }
                }
            }
            NextHome::SourceFacts => {
                let library = self.shared.library.clone();
                let loaded = Arc::clone(&selected.loaded);
                let folder = selected.music_folder_id.clone();
                match blocking(move || library.home(&loaded, folder.as_ref()).map_err(string_error))
                    .await
                {
                    Ok(home) => Some(home),
                    Err(error) => {
                        warn!(%error, source_id = %selected.source_id(), "could not prepare the next Home snapshot");
                        None
                    }
                }
            }
        };
        if let Some(home) = &home {
            let Some(mut active) = self
                .shared
                .selected()
                .filter(|active| active.qualifier() == selected.qualifier())
            else {
                return;
            };
            active.home = Arc::clone(&home);
            self.shared.replace_selected(active);
        }
        self.shared
            .send_event(SourceEvent::LibraryUpdate(SelectedLibraryUpdate {
                source_id: selected.source_id().clone(),
                source_session_epoch: selected.source_session_epoch,
                change,
                home,
            }))
            .await;
        if reconcile_downloads {
            self.reconcile_downloads(selected).await;
        }
        if source_facts {
            self.start_album_release_lookup();
        }
    }

    async fn target_failed(&mut self, purpose: WorkPurpose, error: String) {
        match purpose {
            WorkPurpose::Add => self.fail_transition(None, error, true).await,
            WorkPurpose::Select { target } => {
                let returns_to_onboarding = self.fallback.is_none()
                    && self.shared.selected().is_none()
                    && self
                        .shared
                        .settings
                        .load()
                        .sources
                        .selected_source_id
                        .is_none();
                if returns_to_onboarding {
                    self.shared.publish_configured().await;
                }
                self.fail_transition(Some(target), error, returns_to_onboarding)
                    .await;
            }
            WorkPurpose::Refresh { qualifier, visible } => {
                if visible {
                    self.shared
                        .send_event(SourceEvent::Operation(SourceOperation::Failed {
                            source_id: Some(qualifier.source_id),
                            message: error,
                            add_form: false,
                        }))
                        .await;
                } else {
                    warn!(%error, "background source refresh failed");
                }
            }
            WorkPurpose::Update { selected, .. } => {
                if selected {
                    self.selected_update_failed(error).await;
                } else {
                    self.shared.send_notice(error).await;
                }
            }
            WorkPurpose::Selected {
                automatic: true, ..
            } => {
                warn!(%error, "background selected source update failed");
            }
            WorkPurpose::Selected {
                automatic: false, ..
            } => self.shared.send_notice(error).await,
        }
    }

    async fn fail_transition(
        &mut self,
        source_id: Option<SourceId>,
        message: String,
        add_form: bool,
    ) {
        self.restore_fallback().await;
        self.shared
            .send_event(SourceEvent::Operation(SourceOperation::Failed {
                source_id,
                message,
                add_form,
            }))
            .await;
    }

    async fn restore_fallback(&mut self) {
        let Some(source_id) = self.fallback.take() else {
            return;
        };
        let configured = match configured_source(&self.shared.settings.load().sources, &source_id) {
            Ok(configured) => configured,
            Err(error) => {
                warn!(%error, %source_id, "could not restore the previous configured source");
                return;
            }
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(|_: SourceReadProgress| {});
        let ready =
            match prepare_select(Arc::clone(&self.shared), configured, progress, cancelled).await {
                Ok(ready) => ready,
                Err(error) => {
                    warn!(%error, %source_id, "could not restore the previous source");
                    return;
                }
            };
        let PreparedReplacement {
            reason: ReplacementReason::Select { .. },
            previous: Some(configured),
            configuration,
            source,
            library,
            ..
        } = ready
        else {
            unreachable!("source restoration prepares a selection")
        };
        let requested_music_folder_id = configured.music_folder_id.clone();
        let loaded = match self.accept_replacement_library(library).await {
            Ok(loaded) => loaded,
            Err(error) => {
                warn!(%error, %source_id, "could not prepare the previous source");
                return;
            }
        };
        let selected = match self
            .prepare_runtime(
                configuration,
                source,
                loaded,
                configured.music_folder_id,
                configured.local_access,
            )
            .await
        {
            Ok(selected) => selected,
            Err(error) => {
                warn!(%error, %source_id, "could not prepare the previous source");
                return;
            }
        };
        match self.install_runtime(selected).await {
            Ok((session, playback)) => {
                let selected = session.snapshot();
                if selected.music_folder_id != requested_music_folder_id {
                    let settings = self.shared.settings.clone();
                    let selected_source_id = selected.source_id().clone();
                    let folder_id = selected.music_folder_id.clone();
                    if let Err(error) = blocking(move || {
                        save_music_folder(&settings, &selected_source_id, folder_id)
                    })
                    .await
                    {
                        warn!(
                            %error,
                            %source_id,
                            "could not save the normalized music folder"
                        );
                    }
                }
                self.publish_new_selected(session, playback, true).await;
            }
            Err(error) => {
                warn!(%error, %source_id, "could not restore the previous Playback session");
            }
        }
    }

    async fn save_local_access(
        &mut self,
        input: SourceLocalAccess,
        metadata: Option<MetadataRequest>,
        completion: Sender<Result<(), String>>,
    ) {
        if let Some(request) = metadata {
            if request.source_id != input.source_id {
                let _ = completion
                    .send(Err(
                        "the local file mapping belongs to a different source".to_string()
                    ))
                    .await;
                return;
            }
            let Some(selected) = self.shared.selected().filter(|selected| {
                selected.source_id() == &request.source_id
                    && selected.source_session_epoch == request.source_session_epoch
            }) else {
                let _ = completion
                    .send(Err(
                        "the metadata item belongs to an inactive source session".to_string(),
                    ))
                    .await;
                return;
            };
            let mapping = local_access_mapping(&input);
            let context = match selected
                .loaded
                .metadata_subject_with_local_access(&request.item_id, Some(&mapping))
            {
                Ok(Some((subject, local_access))) => MetadataContext {
                    source: match selected.source.as_ref().cloned() {
                        Some(source) => source,
                        None => {
                            let _ = completion
                                .send(Err("the selected source is unavailable".to_string()))
                                .await;
                            return;
                        }
                    },
                    subject,
                    local_access: Some(local_access),
                },
                Ok(None) => {
                    let _ = completion
                        .send(Err("the metadata item is no longer available".to_string()))
                        .await;
                    return;
                }
                Err(error) => {
                    let _ = completion.send(Err(error.to_string())).await;
                    return;
                }
            };
            if let Err(error) = context
                .source
                .read_metadata(context.subject, context.local_access)
                .await
            {
                let _ = completion.send(Err(error.to_string())).await;
                return;
            }
            let previous_access = self
                .shared
                .settings
                .load()
                .sources
                .configured
                .iter()
                .find(|configured| configured.configuration.source_id == input.source_id)
                .and_then(|configured| configured.local_access.clone());
            self.cancel_local_access();
            let library = self.shared.library.clone();
            let loaded = Arc::clone(&selected.loaded);
            let settings = self.shared.settings.clone();
            let saved = input.clone();
            if let Err(error) = blocking(move || {
                accept_metadata_local_access_mapping(
                    &library,
                    &loaded,
                    mapping,
                    previous_access,
                    || save_local_access_setting(&settings, &saved),
                )
            })
            .await
            {
                let _ = completion.send(Err(error)).await;
                return;
            }
            if let Err(error) = self.shared.playback().and_then(|playback| {
                playback.stream_inputs_changed(selected.source_id(), selected.source_session_epoch)
            }) {
                warn!(%error, "could not update prepared playback after Local access changed");
            }
            let _ = completion.send(Ok(())).await;
            self.shared.publish_configured().await;
            return;
        }
        let settings = self.shared.settings.clone();
        let saved = input.clone();
        if let Err(error) = blocking(move || save_local_access_setting(&settings, &saved)).await {
            let _ = completion.send(Err(error)).await;
            return;
        }
        let _ = completion.send(Ok(())).await;
        let selected = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &input.source_id);
        if let Some(selected) = selected {
            let library = self.shared.library.clone();
            let loaded = Arc::clone(&selected.loaded);
            let mapping = local_access_mapping(&input);
            if let Err(error) = blocking(move || {
                library
                    .configure_local_access(&loaded, mapping)
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await
            {
                self.shared.send_notice(error).await;
            } else if let Err(error) = self.shared.playback().and_then(|playback| {
                playback.stream_inputs_changed(selected.source_id(), selected.source_session_epoch)
            }) {
                warn!(%error, "could not update prepared playback after Local access changed");
            }
            self.shared.publish_configured().await;
            self.start_local_access_refresh(&selected).await;
        } else {
            self.shared.publish_configured().await;
        }
    }

    async fn clear_local_access(&mut self, source_id: SourceId) {
        if self
            .local_access
            .as_ref()
            .is_some_and(|active| active.qualifier.source_id == source_id)
        {
            self.cancel_local_access();
        }
        let settings = self.shared.settings.clone();
        let settings_source_id = source_id.clone();
        let result = blocking(move || {
            settings.update(|stored| {
                let configured = stored
                    .sources
                    .configured
                    .iter_mut()
                    .find(|source| source.configuration.source_id == settings_source_id)
                    .ok_or_else(|| "the configured source no longer exists".to_string())?;
                configured.local_access = None;
                Ok(())
            })
        })
        .await;
        match result {
            Ok(()) => {
                let selected = self
                    .shared
                    .selected()
                    .filter(|selected| selected.source_id() == &source_id);
                let library = self.shared.library.clone();
                let store_result = if let Some(selected) = selected.as_ref() {
                    let loaded = Arc::clone(&selected.loaded);
                    blocking(move || {
                        library
                            .clear_local_access(&loaded)
                            .map(|_| ())
                            .map_err(string_error)
                    })
                    .await
                } else {
                    blocking(move || {
                        library
                            .discard_local_access(source_id.clone())
                            .map_err(string_error)
                    })
                    .await
                };
                if let Err(error) = store_result {
                    self.shared.send_notice(error).await;
                } else if let Some(selected) = selected
                    && let Err(error) = self.shared.playback().and_then(|playback| {
                        playback.stream_inputs_changed(
                            selected.source_id(),
                            selected.source_session_epoch,
                        )
                    })
                {
                    warn!(%error, "could not update prepared playback after Local access was cleared");
                }
                self.shared.publish_configured().await;
            }
            Err(error) => self.shared.send_notice(error).await,
        }
    }

    async fn forget_now(&mut self, source_id: SourceId) {
        let stored = self.shared.settings.load();
        let removed = stored
            .sources
            .configured
            .iter()
            .find(|source| source.configuration.source_id == source_id)
            .cloned();
        let selected = self
            .shared
            .selected()
            .is_some_and(|selected| selected.source_id() == &source_id);
        let replacement = selected
            .then(|| replacement_source(&stored.sources, &source_id))
            .flatten();
        if selected {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Switching {
                    target: replacement
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| source_id.clone()),
                    progress: initial_progress(),
                }))
                .await;
            self.begin_transition().await;
        }
        let settings = self.shared.settings.clone();
        let id_for_settings = source_id.clone();
        let saved = blocking(move || {
            settings.update(|stored| {
                stored
                    .sources
                    .configured
                    .retain(|source| source.configuration.source_id != id_for_settings);
                if stored.sources.selected_source_id.as_ref() == Some(&id_for_settings) {
                    stored.sources.selected_source_id = None;
                }
                Ok(())
            })
        })
        .await;
        if let Err(error) = saved {
            if selected {
                self.fail_transition(Some(source_id), error, false).await;
            } else {
                self.shared.send_notice(error).await;
            }
            return;
        }
        self.fallback = None;
        self.remove_replaced_source_data(source_id.clone()).await;
        if let Some(reference) = removed.and_then(|source| source.credential_ref) {
            self.delete_staged_credential(Some(&reference)).await;
        }
        if let Some(replacement) = replacement {
            self.pending.push_front(WorkRequest::Select(replacement));
            return;
        }
        self.shared.publish_configured().await;
        self.shared
            .send_event(SourceEvent::Operation(SourceOperation::Idle))
            .await;
    }

    async fn set_music_folder(&mut self, source_id: SourceId, folder_id: Option<MusicFolderId>) {
        let Some(mut selected) = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
        else {
            return;
        };
        let folder_id = match normalize_music_folder(&selected.loaded, folder_id) {
            Ok(folder) => folder,
            Err(error) => {
                self.shared.send_notice(error).await;
                return;
            }
        };
        let library = self.shared.library.clone();
        let loaded = Arc::clone(&selected.loaded);
        let home_folder = folder_id.clone();
        selected.home = match blocking(move || {
            library
                .home(&loaded, home_folder.as_ref())
                .map_err(string_error)
        })
        .await
        {
            Ok(home) => home,
            Err(error) => {
                self.shared.send_notice(error).await;
                return;
            }
        };
        let settings = self.shared.settings.clone();
        let source_for_settings = source_id.clone();
        let folder_for_settings = folder_id.clone();
        if let Err(error) = blocking(move || {
            save_music_folder(&settings, &source_for_settings, folder_for_settings)
        })
        .await
        {
            self.shared.send_notice(error).await;
            return;
        }
        selected.music_folder_id = folder_id;
        self.shared.publish_library_replacement(&selected).await;
        self.reconcile_downloads(&selected).await;
    }

    async fn stage_credential(
        &self,
        credential: Option<String>,
    ) -> Result<Option<CredentialRef>, String> {
        let Some(credential) = credential else {
            return Ok(None);
        };
        let reference = fresh_credential_ref()?;
        let secrets = Arc::clone(&self.shared.secrets);
        let saved_reference = reference.clone();
        blocking(move || save_provider_secret(&secrets, &saved_reference, credential)).await?;
        Ok(Some(reference))
    }

    async fn delete_staged_credential(&self, reference: Option<&CredentialRef>) {
        let Some(reference) = reference.cloned() else {
            return;
        };
        let secrets = Arc::clone(&self.shared.secrets);
        if let Err(error) = blocking(move || delete_provider_secret(&secrets, &reference)).await {
            warn!(%error, "could not delete a replaced source credential");
        }
    }

    async fn save_selected_configuration(
        &self,
        configured: ConfiguredSource,
    ) -> Result<(), String> {
        let settings = self.shared.settings.clone();
        blocking(move || {
            settings.update(|stored| {
                if let Some(saved) = stored.sources.configured.iter_mut().find(|saved| {
                    saved.configuration.source_id == configured.configuration.source_id
                }) {
                    *saved = configured.clone();
                } else {
                    stored.sources.configured.push(configured.clone());
                }
                stored.sources.selected_source_id =
                    Some(configured.configuration.source_id.clone());
                Ok(())
            })
        })
        .await
    }

    async fn save_selected_account_replacement(
        &self,
        previous: &SourceId,
        configured: ConfiguredSource,
    ) -> Result<(), String> {
        let settings = self.shared.settings.clone();
        let previous = previous.clone();
        blocking(move || replace_source_account(&settings, &previous, configured, true)).await
    }

    async fn save_configuration(&self, configured: ConfiguredSource) -> Result<(), String> {
        let settings = self.shared.settings.clone();
        blocking(move || replace_saved_source(&settings, configured)).await
    }

    async fn save_inactive_connected(
        &self,
        configured: ConfiguredSource,
        configuration: SourceConfiguration,
        credential: Option<String>,
    ) -> Result<(), String> {
        let credential_ref = self.stage_credential(credential).await?;
        let replacement = ConfiguredSource {
            configuration,
            credential_ref: credential_ref
                .clone()
                .or_else(|| configured.credential_ref.clone()),
            music_folder_id: configured.music_folder_id,
            local_access: configured.local_access,
        };
        if let Err(error) = self.save_configuration(replacement.clone()).await {
            self.delete_staged_credential(credential_ref.as_ref()).await;
            return Err(error);
        }
        if configured.credential_ref != replacement.credential_ref {
            self.delete_staged_credential(configured.credential_ref.as_ref())
                .await;
        }
        Ok(())
    }

    async fn replace_inactive_account(
        &self,
        configured: ConfiguredSource,
        configuration: SourceConfiguration,
        credential: Option<String>,
    ) -> Result<(), String> {
        let previous_source_id = configured.configuration.source_id.clone();
        let credential_ref = self.stage_credential(credential).await?;
        let replacement = ConfiguredSource {
            configuration,
            credential_ref: credential_ref.clone(),
            music_folder_id: None,
            local_access: configured.local_access,
        };
        let settings = self.shared.settings.clone();
        let previous_for_settings = previous_source_id.clone();
        let replacement_for_settings = replacement.clone();
        if let Err(error) = blocking(move || {
            replace_source_account(
                &settings,
                &previous_for_settings,
                replacement_for_settings,
                false,
            )
        })
        .await
        {
            self.delete_staged_credential(credential_ref.as_ref()).await;
            return Err(error);
        }
        self.remove_replaced_source_data(previous_source_id).await;
        if configured.credential_ref != replacement.credential_ref {
            self.delete_staged_credential(configured.credential_ref.as_ref())
                .await;
        }
        Ok(())
    }

    async fn remove_replaced_source_data(&self, source_id: SourceId) {
        let loaded = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
            .map(|selected| selected.loaded);
        self.shared
            .downloads
            .clear(source_id.clone(), loaded, false);
        let library = self.shared.library.clone();
        let source_for_store = source_id.clone();
        if let Err(error) = blocking(move || {
            library
                .remove_source_data(&source_for_store)
                .map_err(string_error)
        })
        .await
        {
            self.shared.send_notice(error).await;
        }
        if let Err(error) = self.shared.artwork.invalidate_source(&source_id) {
            self.shared.send_notice(error.to_string()).await;
        }
    }

    async fn prepare_runtime(
        &self,
        configuration: SourceConfiguration,
        source: Option<Arc<Source>>,
        loaded: Arc<LoadedLibrary>,
        music_folder_id: Option<MusicFolderId>,
        local_access: Option<ConfiguredLocalAccess>,
    ) -> Result<SelectedSourceRuntime, String> {
        let music_folder_id = normalize_music_folder(&loaded, music_folder_id)?;
        if let Some(access) = local_access {
            let library = self.shared.library.clone();
            let loaded_for_access = Arc::clone(&loaded);
            blocking(move || {
                library
                    .configure_local_access(
                        &loaded_for_access,
                        configured_local_access_mapping(&access),
                    )
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await?;
        }
        let downloads = self.shared.downloads.clone();
        let loaded_for_downloads = Arc::clone(&loaded);
        let source_for_downloads = source.clone();
        let download_directory = self
            .shared
            .settings
            .load()
            .ui
            .download_directory(&configuration.source_id);
        downloads
            .attach(
                source_for_downloads,
                &loaded_for_downloads,
                download_directory,
            )
            .await?;
        let library = self.shared.library.clone();
        let loaded_for_home = Arc::clone(&loaded);
        let folder_for_home = music_folder_id.clone();
        let home = blocking(move || {
            library
                .home(&loaded_for_home, folder_for_home.as_ref())
                .map_err(string_error)
        })
        .await?;
        Ok(SelectedSourceRuntime {
            configuration,
            source,
            source_session_epoch: SourceSessionEpoch::new(
                self.shared.next_epoch.fetch_add(1, Ordering::AcqRel),
            ),
            loaded,
            home,
            music_folder_id,
        })
    }

    async fn install_runtime(
        &self,
        selected: SelectedSourceRuntime,
    ) -> Result<(SelectedSourceSession, PlaybackProjection), String> {
        let session = SelectedSourceSession::new(selected);
        let playback = self.shared.playback()?;
        let session_for_playback = session.clone();
        let projection = blocking(move || playback.install_selected(session_for_playback)).await?;
        Ok((session, projection))
    }
}

fn work_accepts_activity(purpose: &WorkPurpose, qualifier: &SourceQualifier) -> bool {
    match purpose {
        WorkPurpose::Refresh {
            qualifier: target, ..
        } => target == qualifier,
        WorkPurpose::Selected {
            qualifier: target, ..
        } => target == qualifier,
        WorkPurpose::Update { selected, .. } => *selected,
        WorkPurpose::Add | WorkPurpose::Select { .. } => false,
    }
}

fn download_rule_tracks(
    loaded: &Arc<LoadedLibrary>,
    music_folder_id: Option<&MusicFolderId>,
    rule: downloads::DownloadRule,
) -> library::LoadedLibraryResult<Vec<Track>> {
    match rule {
        downloads::DownloadRule::EntireLibrary => loaded
            .track_list(music_folder_id, TrackSort::Title, false)?
            .materialize_owned(),
        downloads::DownloadRule::Favorites => loaded
            .favorite_download_track_list(music_folder_id)?
            .materialize_owned(),
        downloads::DownloadRule::AllPlaylists => loaded
            .all_playlist_track_list(music_folder_id)?
            .materialize_owned(),
        downloads::DownloadRule::LatestFiveAlbums => loaded
            .latest_album_track_list(music_folder_id, 5)?
            .materialize_owned(),
    }
}

fn should_reconcile_downloads(
    change: &AcceptedLibraryChange,
    next_home: &NextHome,
    rules: ui::DownloadRules,
) -> bool {
    !matches!(
        next_home,
        NextHome::AcceptedPlay(_) | NextHome::ActivityKeep
    ) && ((rules.entire_library && !change.tracks.is_empty())
        || (rules.favorites
            && (!change.tracks.is_empty()
                || !change.albums.is_empty()
                || !change.artists.is_empty()
                || change.favorite.is_some()))
        || (rules.all_playlists && (!change.tracks.is_empty() || !change.playlists.is_empty()))
        || (rules.latest_five_albums && (!change.tracks.is_empty() || !change.albums.is_empty())))
}

impl Shared {
    fn selected_session(&self) -> Option<SelectedSourceSession> {
        self.selected
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn selected(&self) -> Option<SelectedSourceRuntime> {
        self.selected_session().map(|session| session.snapshot())
    }

    fn replace_selected(&self, selected: SelectedSourceRuntime) -> bool {
        let session = self
            .selected
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|session| session.snapshot().qualifier() == selected.qualifier())
            .cloned();
        let Some(session) = session else {
            return false;
        };
        session.replace(selected);
        true
    }

    fn matches_selected(&self, qualifier: &SourceQualifier) -> bool {
        self.selected()
            .is_some_and(|selected| selected.qualifier() == *qualifier)
    }

    fn playback(&self) -> Result<Arc<PlaybackOwner>, String> {
        self.playback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade()
            .ok_or_else(|| "Playback is not attached to the selected source owner".to_string())
    }

    async fn publish_selected(&self, session: SelectedSourceSession, playback: PlaybackProjection) {
        let selected = session.snapshot();
        *self
            .selected
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session);
        if let Ok(playback_owner) = self.playback() {
            playback_owner.publish_selected_products(&playback);
        }
        let stored = self.settings.load();
        let selected = ui_selected(selected);
        self.send_event(SourceEvent::Selected {
            configured: configured_sources(&stored, Some(&selected)),
            selected,
            playback,
        })
        .await;
    }

    async fn publish_library_replacement(&self, selected: &SelectedSourceRuntime) {
        if !self.replace_selected(selected.clone()) {
            return;
        }
        let selected = ui_selected(selected.clone());
        self.send_event(SourceEvent::LibraryReplaced {
            configured: configured_sources(&self.settings.load(), Some(&selected)),
            selected,
        })
        .await;
    }

    async fn publish_home_replacement(&self, selected: SelectedSourceRuntime) {
        if !self.replace_selected(selected.clone()) {
            return;
        }
        self.send_event(SourceEvent::HomeReplaced {
            source_id: selected.source_id().clone(),
            source_session_epoch: selected.source_session_epoch,
            home: selected.home,
        })
        .await;
    }

    async fn publish_selected_playback(
        &self,
        selected: &SelectedSourceRuntime,
        projection: PlaybackProjection,
    ) {
        if let Ok(playback_owner) = self.playback() {
            playback_owner.publish_selected_products(&projection);
        }
        self.send_event(SourceEvent::Playback {
            source_id: selected.source_id().clone(),
            source_session_epoch: selected.source_session_epoch,
            projection,
        })
        .await;
    }

    async fn publish_configured(&self) {
        let selected = self.selected().map(ui_selected);
        self.send_event(SourceEvent::Configured(configured_sources(
            &self.settings.load(),
            selected.as_ref(),
        )))
        .await;
    }

    async fn stop_playback(&self) {
        let Ok(playback) = self.playback() else {
            return;
        };
        if let Err(error) = blocking(move || playback.stop_for_source_switch()).await {
            warn!(%error, "could not stop Playback for a source transition");
        }
    }

    async fn release_selected(&self) {
        if self.selected().is_none() {
            return;
        }
        let (acknowledged, acknowledgement) = async_channel::bounded(1);
        if self
            .outputs
            .events
            .send(SourceEvent::ReleaseSelected { acknowledged })
            .await
            .is_ok()
        {
            let _ = acknowledgement.recv().await;
        }
        self.selected
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    async fn send_notice(&self, message: String) {
        self.send_event(SourceEvent::Notice(message)).await;
    }

    async fn send_event(&self, event: SourceEvent) {
        if self.outputs.events.send(event).await.is_err() {
            warn!("source event lane is unavailable");
        }
    }
}

async fn prepare_add(
    shared: Arc<Shared>,
    input: SourceSetup,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedWork, String> {
    let setup = source_setup_input(input, &shared.settings.load().jellyfin_device_id);
    let connected = Source::connect(setup).await.map_err(string_error)?;
    if cancelled.load(Ordering::Acquire) {
        return Err("source connection was cancelled".to_string());
    }
    let (configuration, source, credential) = connected.into_parts();
    let identity = configuration.input_identity().map_err(string_error)?;
    let source = Arc::new(source);
    let prepared = acquisition::read_source(
        shared.library.clone(),
        identity,
        Arc::clone(&source),
        None,
        Arc::clone(&progress),
        Arc::clone(&cancelled),
    )
    .await?;
    let prepared =
        prepare_candidate_artwork(&shared, Arc::clone(&source), prepared, progress, cancelled)
            .await?;
    let previous = shared
        .settings
        .load()
        .sources
        .configured
        .iter()
        .find(|configured| configured.configuration.source_id == configuration.source_id)
        .cloned();
    Ok(PreparedWork::Replacement(PreparedReplacement {
        reason: ReplacementReason::Add,
        previous,
        configuration,
        source: Some(source),
        credential,
        library: ReplacementLibrary::Candidate(Box::new(prepared)),
    }))
}

async fn prepare_select(
    shared: Arc<Shared>,
    configured: ConfiguredSource,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedReplacement, String> {
    let configuration = configured.configuration.clone();
    let identity = configuration.input_identity().map_err(string_error)?;
    let library = shared.library.clone();
    let source_id = configuration.source_id.clone();
    let source_id_for_store = source_id.clone();
    let cached = blocking(move || {
        library
            .load_source(&source_id_for_store)
            .map_err(string_error)
    })
    .await
    .unwrap_or_else(|error| {
        warn!(%error, "the selected source cache will be rebuilt");
        None
    });
    let cached = match cached {
        Some(loaded) => {
            let cache_match = configuration
                .cache_match(&loaded_input_identity(&loaded))
                .map_err(string_error)?;
            (cache_match != SourceCacheMatch::Incompatible).then_some((loaded, cache_match))
        }
        None => None,
    };
    let opened = match load_credential(&shared, &configured).await {
        Ok(credential) => Source::open(
            configuration.clone(),
            credential,
            Some(shared.settings.load().jellyfin_device_id),
        )
        .map(Arc::new)
        .map_err(string_error),
        Err(error) => Err(error),
    };
    let source = match opened {
        Ok(source) => Some(source),
        Err(error) if cached.is_some() => {
            warn!(%error, %source_id, "live source access is unavailable; using cached library");
            None
        }
        Err(error) => return Err(error),
    };
    let (library, cached, refresh_after_select) = if let Some((loaded, cache_match)) = cached {
        (
            ReplacementLibrary::Cached(loaded),
            true,
            cache_match == SourceCacheMatch::ReaderUpgrade,
        )
    } else {
        let source = source.as_ref().ok_or_else(source_access_unavailable)?;
        let prepared = acquisition::read_source(
            shared.library.clone(),
            identity,
            Arc::clone(source),
            None,
            Arc::clone(&progress),
            Arc::clone(&cancelled),
        )
        .await?;
        (
            ReplacementLibrary::Candidate(Box::new(
                prepare_candidate_artwork(
                    &shared,
                    Arc::clone(source),
                    prepared,
                    progress,
                    cancelled,
                )
                .await?,
            )),
            false,
            false,
        )
    };
    Ok(PreparedReplacement {
        reason: ReplacementReason::Select {
            cached,
            refresh_after_select,
        },
        previous: Some(configured),
        configuration,
        source,
        credential: None,
        library,
    })
}

async fn prepare_refresh(
    shared: Arc<Shared>,
    selected: SelectedSourceRuntime,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedWork, String> {
    prepare_refresh_candidate(shared, selected, progress, cancelled)
        .await
        .map(PreparedWork::Refresh)
}

async fn prepare_refresh_candidate(
    shared: Arc<Shared>,
    selected: SelectedSourceRuntime,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedSourceCandidate, String> {
    let source = selected
        .source
        .as_ref()
        .cloned()
        .ok_or_else(source_access_unavailable)?;
    let identity = selected
        .configuration
        .input_identity()
        .map_err(string_error)?;
    let prepared = acquisition::read_source(
        shared.library.clone(),
        identity,
        Arc::clone(&source),
        Some(Arc::clone(&selected.loaded)),
        Arc::clone(&progress),
        Arc::clone(&cancelled),
    )
    .await?;
    let prepared =
        prepare_candidate_artwork(&shared, source, prepared, progress, cancelled).await?;
    Ok(prepared)
}

async fn prepare_update(
    shared: Arc<Shared>,
    configured: ConfiguredSource,
    input: SourceSettingsInput,
    selected: bool,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedWork, String> {
    let credential = load_credential(&shared, &configured).await?;
    let result = Source::edit(
        configured.configuration.clone(),
        credential,
        input,
        Some(shared.settings.load().jellyfin_device_id),
    )
    .await
    .map_err(string_error)?;
    match result {
        SourceEditResult::Unchanged => Ok(PreparedWork::Configuration {
            configured,
            configuration: None,
        }),
        SourceEditResult::ConfigurationOnly(configuration) => Ok(PreparedWork::Configuration {
            configured,
            configuration: Some(configuration),
        }),
        SourceEditResult::SameAccount(connected) => {
            let (configuration, source, credential) = connected.into_parts();
            if !selected {
                return Ok(PreparedWork::InactiveConnection {
                    configured,
                    configuration,
                    credential,
                    replaces_account: false,
                });
            }
            let identity = configuration.input_identity().map_err(string_error)?;
            let source = Arc::new(source);
            let candidate = {
                let current = shared
                    .selected()
                    .filter(|current| current.source_id() == source.source_id())
                    .ok_or_else(|| "the selected source is no longer active".to_string())?;
                if cache_input_matches(&identity, &current.loaded) {
                    None
                } else {
                    let prepared = acquisition::read_source(
                        shared.library.clone(),
                        identity,
                        Arc::clone(&source),
                        Some(current.loaded),
                        Arc::clone(&progress),
                        Arc::clone(&cancelled),
                    )
                    .await?;
                    Some(Box::new(
                        prepare_candidate_artwork(
                            &shared,
                            Arc::clone(&source),
                            prepared,
                            progress,
                            cancelled,
                        )
                        .await?,
                    ))
                }
            };
            Ok(PreparedWork::SelectedUpdate(PreparedSelectedUpdate {
                configured,
                configuration,
                source,
                credential,
                candidate,
            }))
        }
        SourceEditResult::DifferentAccount(connected) => {
            let (configuration, source, credential) = connected.into_parts();
            if shared
                .settings
                .load()
                .sources
                .configured
                .iter()
                .any(|saved| {
                    saved.configuration.source_id == configuration.source_id
                        && saved.configuration.source_id != configured.configuration.source_id
                })
            {
                return Err("this source account is already configured".to_string());
            }
            if !selected {
                return Ok(PreparedWork::InactiveConnection {
                    configured,
                    configuration,
                    credential,
                    replaces_account: true,
                });
            }
            let identity = configuration.input_identity().map_err(string_error)?;
            let source = Arc::new(source);
            let prepared = acquisition::read_source(
                shared.library.clone(),
                identity,
                Arc::clone(&source),
                None,
                Arc::clone(&progress),
                Arc::clone(&cancelled),
            )
            .await?;
            let prepared = prepare_candidate_artwork(
                &shared,
                Arc::clone(&source),
                prepared,
                progress,
                cancelled,
            )
            .await?;
            Ok(PreparedWork::Replacement(PreparedReplacement {
                reason: ReplacementReason::DifferentAccount,
                previous: Some(configured),
                configuration,
                source: Some(source),
                credential,
                library: ReplacementLibrary::Candidate(Box::new(prepared)),
            }))
        }
    }
}

async fn prepare_candidate_artwork(
    shared: &Shared,
    source: Arc<Source>,
    prepared: PreparedSourceCandidate,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedSourceCandidate, String> {
    if prepared.change() != CandidateChange::Library {
        return Ok(prepared);
    }
    let artwork = shared.artwork.clone();
    blocking(move || {
        if cancelled.load(Ordering::Acquire) {
            return Err("source artwork preparation was cancelled".to_string());
        }
        let source_artwork = prepared.loaded().source_artwork().map_err(string_error)?;
        let total = source_artwork.len();
        progress(SourceReadProgress {
            stage: SourceReadStage::Artwork,
            completed: 0,
            total: Some(total),
        });
        let progress_update = Arc::clone(&progress);
        let cancellation = Arc::clone(&cancelled);
        let summary = artwork
            .prepare_source_artwork(
                SourceImages::new(Arc::clone(&source)),
                Arc::clone(&source_artwork),
                &move |completed, total| {
                    progress_update(SourceReadProgress {
                        stage: SourceReadStage::Artwork,
                        completed,
                        total: Some(total),
                    });
                },
                &move || cancellation.load(Ordering::Acquire),
            )
            .map_err(string_error)?;
        if summary.failed > 0 {
            warn!(
                source_id = %source.source_id(),
                failed = summary.failed,
                total = summary.total,
                "some source artwork remains available for retry"
            );
        }
        Ok(prepared)
    })
    .await
}

async fn run_point(
    shared: Arc<Shared>,
    selected: SelectedSourceRuntime,
    operation: PointOperation,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PointPrepared, String> {
    match operation {
        PointOperation::RefreshHome(kind) => {
            let source_section = match selected.source.as_ref() {
                Some(source) => match source.home_section(kind).await.map_err(string_error)? {
                    NativeSourceResult::Available(section) => Some(section),
                    NativeSourceResult::Unavailable => None,
                },
                None => None,
            };
            Ok(PointPrepared::RefreshHome {
                kind,
                source_section,
            })
        }
        PointOperation::Favorite {
            item,
            favorite,
            previous,
        } => match selected.source.as_ref() {
            Some(source) => match source.set_favorite(item.clone(), favorite).await {
                Ok(acceptance) => Ok(PointPrepared::Favorite {
                    acceptance,
                    item,
                    previous,
                }),
                Err(error) => Ok(PointPrepared::FavoriteFailed {
                    item,
                    previous,
                    message: error.to_string(),
                }),
            },
            None => Ok(PointPrepared::FavoriteFailed {
                item,
                previous,
                message: source_access_unavailable(),
            }),
        },
        PointOperation::Playlist(edit) => selected
            .source
            .as_ref()
            .ok_or_else(source_access_unavailable)?
            .edit_playlist(edit)
            .await
            .map(PointPrepared::Playlist)
            .map_err(string_error),
        PointOperation::SmartPlaylist(operation) => Ok(PointPrepared::SmartPlaylist(operation)),
        PointOperation::ObservedItems(change) => {
            let source = selected
                .source
                .as_ref()
                .ok_or_else(source_access_unavailable)?;
            match prepare_source_change(source, &selected.loaded, change).await? {
                SourceLibraryChangeRead::Exact(update) => Ok(PointPrepared::LibraryUpdate(
                    PreparedLibraryUpdate::Source(update),
                )),
                SourceLibraryChangeRead::Full => Ok(PointPrepared::RefreshRequired),
                SourceLibraryChangeRead::Ignored => Ok(PointPrepared::Unchanged),
            }
        }
        PointOperation::LocalFiles(change) => {
            let source = selected
                .source
                .as_ref()
                .cloned()
                .ok_or_else(source_access_unavailable)?;
            let loaded = Arc::clone(&selected.loaded);
            prepare_local_change(source, loaded, change, cancelled)
                .await
                .map(|replacement| {
                    replacement.map_or(PointPrepared::Unchanged, |replacement| {
                        PointPrepared::LibraryUpdate(PreparedLibraryUpdate::Local(replacement))
                    })
                })
        }
        PointOperation::EditMetadata { edit, mut reply } => {
            let acceptance = prepare_metadata_acceptance(
                shared, selected, edit, progress, cancelled, &mut reply,
            )
            .await;
            Ok(PointPrepared::Metadata { reply, acceptance })
        }
        PointOperation::CheckFreshness => {
            let source = selected
                .source
                .as_ref()
                .ok_or_else(source_access_unavailable)?;
            let freshness = selected.loaded.provider_freshness().map_err(string_error)?;
            match source
                .check_freshness(freshness.as_ref())
                .await
                .map_err(string_error)?
            {
                SourceFreshness::Changed(_) => Ok(PointPrepared::RefreshRequired),
                SourceFreshness::Unavailable
                | SourceFreshness::Unchanged
                | SourceFreshness::Busy => Ok(PointPrepared::Unchanged),
            }
        }
    }
}

async fn prepare_source_change(
    source: &Source,
    loaded: &LoadedLibrary,
    change: SourceLibraryChange,
) -> Result<SourceLibraryChangeRead, String> {
    let contains = |item: &SourceLibraryItemId| selected_contains(loaded, item);
    source
        .read_library_change(change, &contains)
        .await
        .map_err(string_error)
}

async fn prepare_local_change(
    source: Arc<Source>,
    loaded: Arc<LoadedLibrary>,
    change: LocalFilesystemChange,
    cancelled: Arc<AtomicBool>,
) -> Result<Option<library::LocalComponentReplacement>, String> {
    blocking(move || {
        let should_stop = || cancelled.load(Ordering::Acquire);
        let check = source
            .check_local(change, &should_stop)
            .map_err(string_error)?;
        let accepted_files = loaded
            .local_file_baseline(check.file_seeds())
            .map_err(string_error)?;
        let progress = |_: SourceReadProgress| {};
        let Some(change) = source
            .confirm_local_change(check, accepted_files, &progress, &should_stop)
            .map_err(string_error)?
        else {
            return Ok(None);
        };
        let baseline = loaded
            .local_component_baseline(change.component_seeds())
            .map_err(string_error)?;
        source
            .complete_local_change(change, baseline, unix_seconds(), &should_stop)
            .map(Some)
            .map_err(string_error)
    })
    .await
}

async fn prepare_metadata_acceptance(
    shared: Arc<Shared>,
    selected: SelectedSourceRuntime,
    edit: MetadataEdit,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
    reply: &mut MetadataReply,
) -> Result<PreparedLibraryUpdate, MetadataError> {
    let context = selected.metadata_access_context(&edit.item_id)?;
    let Some(context) = context else {
        return Err(MetadataError::Unavailable);
    };
    let MetadataContext {
        source,
        subject,
        local_access,
    } = context;
    reply.mark_write_started();
    match source.write_metadata(subject, edit, local_access).await? {
        MetadataRefresh::Source(change) => {
            match prepare_source_change(&source, &selected.loaded, change).await {
                Ok(SourceLibraryChangeRead::Exact(update)) => {
                    Ok(PreparedLibraryUpdate::Source(update))
                }
                Ok(SourceLibraryChangeRead::Ignored) => Err(MetadataError::SavedRefreshFailed(
                    "the source did not return the written item".to_string(),
                )),
                Ok(SourceLibraryChangeRead::Full) => {
                    prepare_refresh_candidate(shared, selected, progress, cancelled)
                        .await
                        .map(PreparedLibraryUpdate::Full)
                        .map_err(MetadataError::SavedRefreshFailed)
                }
                Err(error) => Err(MetadataError::SavedRefreshFailed(error.to_string())),
            }
        }
        MetadataRefresh::Local(change) => {
            let loaded = Arc::clone(&selected.loaded);
            let source = Arc::clone(&source);
            prepare_local_change(source, loaded, change, cancelled)
                .await
                .and_then(|replacement| {
                    replacement.ok_or_else(|| {
                        "the written files were not accepted by the Local source".to_string()
                    })
                })
                .map(PreparedLibraryUpdate::Local)
                .map_err(MetadataError::SavedRefreshFailed)
        }
    }
}

fn selected_contains(loaded: &LoadedLibrary, item: &SourceLibraryItemId) -> bool {
    match item {
        SourceLibraryItemId::Album(id) => loaded.album(id).ok().flatten().is_some(),
        SourceLibraryItemId::Track(id) => loaded.track(id).ok().flatten().is_some(),
        SourceLibraryItemId::Artist(id) => loaded.artist(id).ok().flatten().is_some(),
        SourceLibraryItemId::Genre(id) => loaded.genre(id).ok().flatten().is_some(),
        SourceLibraryItemId::Playlist(id) => loaded.contains_playlist(id).unwrap_or(false),
        SourceLibraryItemId::MusicFolder(id) => loaded.contains_music_folder(id).unwrap_or(false),
    }
}

fn automatic_point(operation: &PointOperation) -> bool {
    matches!(
        operation,
        PointOperation::ObservedItems(_)
            | PointOperation::LocalFiles(_)
            | PointOperation::CheckFreshness
    )
}

fn merge_automatic_point(current: &mut PointOperation, incoming: &PointOperation) -> bool {
    match (current, incoming) {
        (PointOperation::ObservedItems(current), PointOperation::ObservedItems(incoming)) => {
            current.merge(incoming.clone());
            true
        }
        (PointOperation::LocalFiles(current), PointOperation::LocalFiles(incoming)) => {
            current.merge(incoming.clone());
            true
        }
        (PointOperation::CheckFreshness, PointOperation::CheckFreshness) => true,
        _ => false,
    }
}

async fn load_credential(
    shared: &Shared,
    configured: &ConfiguredSource,
) -> Result<Option<String>, String> {
    let Some(reference) = configured.credential_ref.clone() else {
        return Ok(None);
    };
    let secrets = Arc::clone(&shared.secrets);
    blocking(move || load_provider_secret(&secrets, &reference)).await
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(string_error)?
}

async fn replay_activity_updates(
    library: Library,
    loaded: Arc<LoadedLibrary>,
    updates: Vec<RecordedActivity>,
) -> Result<(), String> {
    if updates.is_empty() {
        return Ok(());
    }
    blocking(move || {
        for update in updates {
            library
                .apply_recorded_activity(&loaded, &update)
                .map_err(string_error)?;
        }
        Ok(())
    })
    .await
}

impl SourcePort for SourceOwner {
    fn configured_source(&self, source_id: &SourceId) -> Result<Option<EditableSource>, String> {
        self.shared
            .settings
            .load()
            .sources
            .configured
            .iter()
            .find(|source| &source.configuration.source_id == source_id)
            .map(|source| editable_source(&source.configuration))
            .transpose()
    }

    fn discover_servers(&self) {
        let events = self.shared.outputs.discovery.clone();
        let _ = events.try_send(DiscoveryUpdate {
            servers: Arc::from([]),
            status: DiscoveryStatus::Searching,
        });
        self.shared.runtime.spawn_blocking(move || {
            let update = match sources::discover_jellyfin_servers(Duration::from_millis(1_500)) {
                Ok(servers) if servers.is_empty() => DiscoveryUpdate {
                    servers: Arc::from([]),
                    status: DiscoveryStatus::Empty,
                },
                Ok(servers) => {
                    let servers = servers
                        .into_iter()
                        .map(|server| DiscoveredServer {
                            name: server.name,
                            address: server.address,
                            id: server.id,
                        })
                        .collect::<Vec<_>>();
                    DiscoveryUpdate {
                        status: DiscoveryStatus::Found(servers.len() as u64),
                        servers: servers.into(),
                    }
                }
                Err(error) => DiscoveryUpdate {
                    servers: Arc::from([]),
                    status: DiscoveryStatus::Failed(error.to_string()),
                },
            };
            let _ = events.try_send(update);
        });
    }

    fn configure_source(&self, input: SourceSetup) {
        self.send(Message::Request(WorkRequest::Configure(input)));
    }

    fn update_source(&self, input: SourceSettingsChange) {
        self.send(Message::Request(WorkRequest::Update(input)));
    }

    fn select_source(&self, source_id: SourceId) {
        self.send(Message::Request(WorkRequest::Select(source_id)));
    }

    fn change_secret_storage(&self, mode: SecretStorageMode) -> Receiver<Result<(), String>> {
        let (result, receiver) = async_channel::bounded(1);
        if self
            .messages
            .try_send(Message::Request(WorkRequest::ChangeSecretStorage {
                mode,
                result: result.clone(),
            }))
            .is_err()
        {
            let _ = result.try_send(Err("source operation lane is unavailable".to_string()));
        }
        receiver
    }

    fn add_local_folder(&self, path: PathBuf) {
        self.send(Message::Request(WorkRequest::AddLocalFolder(path)));
    }

    fn remove_local_folder(&self, path: String) {
        self.send(Message::Request(WorkRequest::RemoveLocalFolder(path)));
    }

    fn refresh_source(&self, source_id: SourceId) {
        let Some(qualifier) = self
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
            .map(|selected| selected.qualifier())
        else {
            return;
        };
        self.send(Message::Request(WorkRequest::Refresh {
            qualifier,
            visible: true,
        }));
    }

    fn check_for_source_changes(&self) {
        self.send(Message::Request(WorkRequest::CheckSelectedSource));
    }

    fn selected_library_revealed(&self) {
        self.send(Message::SelectedLibraryRevealed);
    }

    fn refresh_home(&self, kind: HomeSectionKind) {
        self.send_selected(|_| PointOperation::RefreshHome(kind));
    }

    fn save_local_access(
        &self,
        input: SourceLocalAccess,
        metadata: Option<MetadataRequest>,
    ) -> Receiver<Result<(), String>> {
        let (result, receiver) = async_channel::bounded(1);
        if self
            .messages
            .try_send(Message::Request(WorkRequest::SaveLocalAccess {
                input,
                metadata,
                result: result.clone(),
            }))
            .is_err()
        {
            let _ = result.try_send(Err("source operation lane is unavailable".to_string()));
        }
        receiver
    }

    fn clear_local_access(&self, source_id: SourceId) {
        self.send(Message::Request(WorkRequest::ClearLocalAccess(source_id)));
    }

    fn forget_source(&self, source_id: SourceId) {
        self.send(Message::Request(WorkRequest::Forget(source_id)));
    }

    fn set_music_folder(&self, source_id: SourceId, folder_id: Option<MusicFolderId>) {
        self.send(Message::Request(WorkRequest::SetMusicFolder {
            source_id,
            folder_id,
        }));
    }

    fn set_favorite(&self, item: FavoriteItemId, favorite: bool) {
        self.send_selected(|selected| PointOperation::Favorite {
            previous: favorite_value(&selected.loaded, &item).unwrap_or(!favorite),
            item,
            favorite,
        });
    }

    fn add_playlist_tracks(&self, request: PlaylistTrackAdd) -> usize {
        let Some(selected) = self.selected() else {
            return 0;
        };
        let edit = match self
            .shared
            .library
            .prepare_playlist_add(&selected.loaded, request)
        {
            Ok(Some(edit)) => edit,
            Ok(None) => return 0,
            Err(error) => {
                warn!(%error, "could not prepare playlist tracks");
                return 0;
            }
        };
        let count = match &edit {
            PlaylistEdit::AddTracks { track_ids, .. } => track_ids.len(),
            _ => 0,
        };
        self.send(Message::Request(WorkRequest::Selected {
            qualifier: selected.qualifier(),
            operation: PointOperation::Playlist(edit),
        }));
        count
    }

    fn edit_playlist(&self, edit: PlaylistEdit) {
        self.send_selected(|_| PointOperation::Playlist(edit));
    }

    fn download(&self, request: DownloadRequest) {
        self.send(Message::Download(request));
    }

    fn remove_download(&self, request: RemoveDownloadRequest) {
        self.send(Message::RemoveDownload(request));
    }

    fn remove_download_rule(
        &self,
        source_id: SourceId,
        rule: downloads::DownloadRule,
        delete_downloads: bool,
    ) {
        self.send(Message::RemoveDownloadRule {
            source_id,
            rule,
            delete_downloads,
        });
    }

    fn cancel_download(&self, source_id: SourceId, job_id: String) {
        self.send(Message::CancelDownload { source_id, job_id });
    }

    fn move_download(
        &self,
        source_id: SourceId,
        job_id: String,
        target_job_id: String,
        after: bool,
    ) {
        self.send(Message::MoveDownload {
            source_id,
            job_id,
            target_job_id,
            after,
        });
    }

    fn clear_downloads(&self, source_id: SourceId) {
        self.send(Message::ClearDownloads(source_id));
    }

    fn folder(&self, request: FolderRequest) -> Receiver<Result<FolderContents, String>> {
        let (result, receiver) = async_channel::bounded(1);
        let selected = self.selected().filter(|selected| {
            selected.source_id() == &request.source_id
                && selected.source_session_epoch == request.source_session_epoch
        });
        let selected = selected.map(|selected| (selected.source, Arc::clone(&selected.loaded)));
        self.shared.runtime.spawn(async move {
            let value = match selected {
                Some((Some(source), loaded)) => {
                    match source
                        .folder(request.folder_id.as_ref(), request.music_folder_id.as_ref())
                        .await
                    {
                        Ok(NativeSourceResult::Available(contents)) => {
                            reconcile_folder_contents(&loaded, contents)
                        }
                        Ok(NativeSourceResult::Unavailable) => {
                            cached_folder_contents(loaded, request.folder_id.as_ref())
                        }
                        Err(error) => Err(error.to_string()),
                    }
                }
                Some((None, loaded)) => cached_folder_contents(loaded, request.folder_id.as_ref()),
                None => Err("the folder belongs to an inactive source session".to_string()),
            };
            let _ = result.send(value).await;
        });
        receiver
    }

    fn search(&self, request: SearchRequest) -> Receiver<Result<library::SearchResults, String>> {
        let (result, receiver) = async_channel::bounded(1);
        let selected = self.selected().filter(|selected| {
            selected.source_id() == &request.source_id
                && selected.source_session_epoch == request.source_session_epoch
        });
        let selected = selected.map(|selected| (selected.source, Arc::clone(&selected.loaded)));
        self.shared.runtime.spawn(async move {
            let search = request.search;
            let value = match selected {
                Some((Some(source), loaded)) => match source.search(&search).await {
                    Ok(NativeSourceResult::Available(results)) => {
                        reconcile_search_results(&loaded, results)
                    }
                    Ok(NativeSourceResult::Unavailable) => cached_search(loaded, search).await,
                    Err(error) => Err(error.to_string()),
                },
                Some((None, loaded)) => cached_search(loaded, search).await,
                None => Err("the search belongs to an inactive source session".to_string()),
            };
            let _ = result.send(value).await;
        });
        receiver
    }

    fn metadata_editing_available(&self, request: MetadataRequest) -> bool {
        self.selected()
            .filter(|selected| {
                selected.source_id() == &request.source_id
                    && selected.source_session_epoch == request.source_session_epoch
            })
            .is_some_and(|selected| selected.metadata_editing_available(&request.item_id))
    }

    fn metadata(&self, request: MetadataRequest) -> Receiver<Result<MetadataDraft, MetadataError>> {
        let (result, receiver) = async_channel::bounded(1);
        let selected = self.selected().filter(|selected| {
            selected.source_id() == &request.source_id
                && selected.source_session_epoch == request.source_session_epoch
        });
        self.shared.runtime.spawn(async move {
            let value = match selected {
                Some(selected) => match selected.metadata_access_context(&request.item_id) {
                    Ok(Some(context)) => {
                        context
                            .source
                            .read_metadata(context.subject, context.local_access)
                            .await
                    }
                    Ok(None) => Err(MetadataError::Unavailable),
                    Err(error) => Err(error),
                },
                None => Err(MetadataError::Unavailable),
            };
            let _ = result.send(value).await;
        });
        receiver
    }

    fn edit_metadata(&self, request: MetadataEditRequest) -> Receiver<Result<(), MetadataError>> {
        let (result, receiver) = async_channel::bounded(1);
        let reply = MetadataReply::new(result);
        if request.edit.changes.is_empty() {
            reply.finish(Ok(()));
            return receiver;
        }
        let selected = self.selected().filter(|selected| {
            selected.source_id() == &request.source_id
                && selected.source_session_epoch == request.source_session_epoch
        });
        let Some(selected) = selected else {
            return receiver;
        };
        let operation = PointOperation::EditMetadata {
            edit: request.edit,
            reply,
        };
        let _ = self
            .messages
            .try_send(Message::Request(WorkRequest::Selected {
                qualifier: selected.qualifier(),
                operation,
            }));
        receiver
    }

    fn identify_metadata(
        &self,
        request: MetadataIdentificationRequest,
    ) -> Receiver<Result<Option<library::MetadataValues>, String>> {
        let (result, receiver) = async_channel::bounded(1);
        let external_lookup_allowed = self
            .shared
            .settings
            .load()
            .ui
            .allows_external_metadata_lookup();
        let context = self.selected_metadata_context(
            &request.source_id,
            request.source_session_epoch,
            &request.item_id,
        );
        let Some(context) = context else {
            return receiver;
        };
        self.shared.runtime.spawn(async move {
            let direct_applicable = external_lookup_allowed
                && request
                    .item_id
                    .has_exact_musicbrainz_identity(&request.values);
            let source_search_applicable = context.source.metadata_source_search(&context.subject)
                && !request.values.title.trim().is_empty();
            let value = if !direct_applicable && !source_search_applicable {
                Ok(None)
            } else {
                identify_metadata_with_fallback(
                    context.source,
                    context.subject,
                    request,
                    direct_applicable,
                    source_search_applicable,
                )
                .await
            };
            let _ = result.send(value).await;
        });
        receiver
    }
}

async fn identify_metadata_with_fallback(
    source: Arc<Source>,
    subject: library::MetadataSubject,
    request: MetadataIdentificationRequest,
    direct_applicable: bool,
    source_search_applicable: bool,
) -> Result<Option<library::MetadataValues>, String> {
    let editing = request.editing;
    let current = request.values;
    let direct_item_id = request.item_id;
    let direct_values = current.clone();
    let direct = async move {
        blocking(move || metadata_lookup::identify_metadata(&direct_item_id, &direct_values)).await
    };
    let source_search_values = current.clone();
    let source_search = async move {
        source
            .identify_metadata(&subject, &source_search_values)
            .await
    };
    resolve_identification(
        direct_applicable,
        source_search_applicable,
        &editing,
        &current,
        direct,
        source_search,
    )
    .await
}

async fn resolve_identification<Direct, SourceSearch>(
    direct_applicable: bool,
    source_search_applicable: bool,
    editing: &MetadataEditing,
    current: &library::MetadataValues,
    direct: Direct,
    source_search: SourceSearch,
) -> Result<Option<library::MetadataValues>, String>
where
    Direct: Future<Output = Result<Option<library::MetadataValues>, String>>,
    SourceSearch: Future<Output = Result<Option<library::MetadataValues>, String>>,
{
    let mut direct_failure = None;
    if direct_applicable {
        match direct.await {
            Ok(Some(candidate)) if editing.identification_changes(current, &candidate) => {
                return Ok(Some(candidate));
            }
            Ok(_) => {}
            Err(error) => direct_failure = Some(error),
        }
    }
    if source_search_applicable {
        return match source_search.await {
            Ok(Some(candidate)) if editing.identification_changes(current, &candidate) => {
                Ok(Some(candidate))
            }
            Ok(_) => Ok(None),
            Err(error) => Err(error),
        };
    }
    match direct_failure {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

fn cached_folder_contents(
    loaded: Arc<LoadedLibrary>,
    folder_id: Option<&library::FolderId>,
) -> Result<FolderContents, String> {
    loaded
        .local_folder_contents(folder_id)
        .map(|contents| contents.unwrap_or_default())
        .map_err(string_error)
}

fn reconcile_folder_contents(
    loaded: &LoadedLibrary,
    mut contents: FolderContents,
) -> Result<FolderContents, String> {
    contents.tracks = contents
        .tracks
        .iter()
        .cloned()
        .map(|track| accepted_track_or(loaded, track))
        .collect::<Result<Vec<_>, _>>()?
        .into();
    Ok(contents)
}

fn reconcile_search_results(
    loaded: &LoadedLibrary,
    mut results: library::SearchResults,
) -> Result<library::SearchResults, String> {
    for artist in &mut results.artists {
        if let Some(accepted) = loaded.artist(&artist.id).map_err(string_error)? {
            *artist = (*accepted).clone();
        }
    }
    for album in &mut results.albums {
        if let Some(accepted) = loaded.album(&album.id).map_err(string_error)? {
            *album = (*accepted).clone();
        }
    }
    for track in &mut results.tracks {
        *track = accepted_track_or(loaded, track.clone())?;
    }
    Ok(results)
}

fn accepted_track_or(loaded: &LoadedLibrary, track: Track) -> Result<Track, String> {
    loaded
        .track(&track.id)
        .map(|accepted| accepted.unwrap_or(track))
        .map_err(string_error)
}

async fn cached_search(
    loaded: Arc<LoadedLibrary>,
    request: library::SearchRequest,
) -> Result<library::SearchResults, String> {
    blocking(move || loaded.search(&request).map_err(string_error)).await
}

impl ui::runtime::SmartPlaylistPort for SourceOwner {
    fn create(&self, name: String, definition: SmartPlaylistDefinition) {
        self.send_selected(|_| {
            PointOperation::SmartPlaylist(SmartPlaylistOperation::Create { name, definition })
        });
    }

    fn update(&self, id: SmartPlaylistId, name: String, definition: SmartPlaylistDefinition) {
        self.send_selected(|_| {
            PointOperation::SmartPlaylist(SmartPlaylistOperation::Update {
                id,
                name,
                definition,
            })
        });
    }

    fn delete(&self, id: SmartPlaylistId) {
        self.send_selected(|_| PointOperation::SmartPlaylist(SmartPlaylistOperation::Delete(id)));
    }

    fn restore_builtin(&self, builtin: SmartPlaylistBuiltin) {
        self.send_selected(|_| {
            PointOperation::SmartPlaylist(SmartPlaylistOperation::Restore(builtin))
        });
    }

    fn move_relative(&self, dragged: SmartPlaylistId, target: SmartPlaylistId, after: bool) {
        self.send_selected(|_| {
            PointOperation::SmartPlaylist(SmartPlaylistOperation::Move {
                dragged,
                target,
                after,
            })
        });
    }
}

fn normalize_music_folder(
    loaded: &LoadedLibrary,
    folder_id: Option<MusicFolderId>,
) -> Result<Option<MusicFolderId>, String> {
    let Some(folder_id) = folder_id else {
        return Ok(None);
    };
    loaded
        .contains_music_folder(&folder_id)
        .map(|present| present.then_some(folder_id))
        .map_err(string_error)
}

fn cache_input_matches(identity: &SourceInputIdentity, loaded: &LoadedLibrary) -> bool {
    loaded_input_identity(loaded) == *identity
}

fn loaded_input_identity(loaded: &LoadedLibrary) -> SourceInputIdentity {
    SourceInputIdentity {
        source_id: loaded.source_id().clone(),
        version: loaded.input_version(),
        digest: *loaded.input_digest(),
    }
}

fn configured_source(
    settings: &SourceSettings,
    source_id: &SourceId,
) -> Result<ConfiguredSource, String> {
    settings
        .configured
        .iter()
        .find(|source| &source.configuration.source_id == source_id)
        .cloned()
        .ok_or_else(|| "the configured source no longer exists".to_string())
}

fn replacement_source(settings: &SourceSettings, removed: &SourceId) -> Option<SourceId> {
    settings
        .configured
        .iter()
        .find(|source| &source.configuration.source_id != removed)
        .map(|source| source.configuration.source_id.clone())
}

fn replace_saved_source(
    settings: &SettingsFile,
    configured: ConfiguredSource,
) -> Result<(), String> {
    settings.update(|stored| {
        let saved = stored
            .sources
            .configured
            .iter_mut()
            .find(|saved| saved.configuration.source_id == configured.configuration.source_id)
            .ok_or_else(|| "the configured source no longer exists".to_string())?;
        *saved = configured;
        Ok(())
    })
}

fn replace_source_account(
    settings: &SettingsFile,
    previous: &SourceId,
    configured: ConfiguredSource,
    select: bool,
) -> Result<(), String> {
    let replacement_id = configured.configuration.source_id.clone();
    settings.update(|stored| {
        let saved = stored
            .sources
            .configured
            .iter_mut()
            .find(|saved| &saved.configuration.source_id == previous)
            .expect("the source actor owns configured account replacement");
        *saved = configured;
        if select {
            stored.sources.selected_source_id = Some(replacement_id);
        }
        Ok(())
    })
}

fn save_music_folder(
    settings: &SettingsFile,
    source_id: &SourceId,
    folder_id: Option<MusicFolderId>,
) -> Result<(), String> {
    settings.update(|stored| {
        let configured = stored
            .sources
            .configured
            .iter_mut()
            .find(|configured| configured.configuration.source_id == *source_id)
            .ok_or_else(|| "the configured source no longer exists".to_string())?;
        configured.music_folder_id = folder_id;
        Ok(())
    })
}

fn save_local_access_setting(
    settings: &SettingsFile,
    access: &SourceLocalAccess,
) -> Result<(), String> {
    settings.update(|stored| {
        let configured = stored
            .sources
            .configured
            .iter_mut()
            .find(|source| source.configuration.source_id == access.source_id)
            .ok_or_else(|| "the configured source no longer exists".to_string())?;
        configured.local_access = Some(ConfiguredLocalAccess {
            root_path: access.root_path.clone(),
            server_prefix: access.server_prefix.clone(),
            local_prefix: access.local_prefix.clone(),
        });
        Ok(())
    })
}

fn accept_metadata_local_access_mapping(
    library: &Library,
    loaded: &Arc<LoadedLibrary>,
    mapping: library::LocalAccessMapping,
    previous_access: Option<ConfiguredLocalAccess>,
    save: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    library
        .accept_local_access_mapping(loaded, mapping)
        .map_err(string_error)?;
    let Err(error) = save() else {
        return Ok(());
    };
    let rollback = library
        .configure_local_access_mapping(
            loaded,
            previous_access
                .as_ref()
                .map(configured_local_access_mapping),
        )
        .map_err(string_error);
    match rollback {
        Ok(()) => Err(error),
        Err(rollback) => Err(format!(
            "{error} The previous local file mapping could not be restored: {rollback}"
        )),
    }
}

fn configured_local_access_mapping(access: &ConfiguredLocalAccess) -> library::LocalAccessMapping {
    library::LocalAccessMapping {
        root_path: access.root_path.clone(),
        server_prefix: access.server_prefix.clone(),
        local_prefix: access.local_prefix.clone(),
    }
}

fn local_access_mapping(access: &SourceLocalAccess) -> library::LocalAccessMapping {
    library::LocalAccessMapping {
        root_path: access.root_path.clone(),
        server_prefix: access.server_prefix.clone(),
        local_prefix: access.local_prefix.clone(),
    }
}

fn ui_local_access_status(status: library::LocalAccessStatus) -> LocalAccessStatus {
    LocalAccessStatus {
        sample_source_path: status.sample_source_path,
        sample_local_path: status.sample_local_path,
        direct_match_count: status.direct_match_count,
        prefix_match_count: status.prefix_match_count,
        metadata_match_count: status.metadata_match_count,
        unmatched_count: status.unmatched_count,
        total_track_count: status.total_track_count,
    }
}

fn configured_sources(
    stored: &StoredSettings,
    selected: Option<&SelectedLibrary>,
) -> ConfiguredSources {
    let sources = stored
        .sources
        .configured
        .iter()
        .map(|configured| SourceSummary {
            id: configured.configuration.source_id.clone(),
            kind: configured.configuration.kind.clone(),
            name: configured.configuration.name.clone(),
        })
        .collect::<Vec<_>>();
    let local_folders = stored
        .sources
        .configured
        .iter()
        .flat_map(|configured| local_roots(&configured.configuration).unwrap_or_default())
        .map(|path| LocalFolder {
            path: path.to_string_lossy().to_string(),
        })
        .collect::<Vec<_>>();
    let local_access = stored
        .sources
        .configured
        .iter()
        .map(|configured| {
            let access = configured
                .local_access
                .as_ref()
                .map(|access| SourceLocalAccess {
                    source_id: configured.configuration.source_id.clone(),
                    root_path: access.root_path.clone(),
                    server_prefix: access.server_prefix.clone(),
                    local_prefix: access.local_prefix.clone(),
                });
            let (album_count, track_count) = selected
                .filter(|selected| selected.source_id == configured.configuration.source_id)
                .and_then(|selected| selected.loaded.counts().ok())
                .map(|counts| (counts.albums, counts.tracks))
                .unwrap_or_default();
            let status = access
                .as_ref()
                .and_then(|_| {
                    selected
                        .filter(|selected| selected.source_id == configured.configuration.source_id)
                        .and_then(|selected| selected.loaded.local_access_status().ok())
                })
                .map(ui_local_access_status)
                .unwrap_or_default();
            SourceLocalAccessSummary {
                source_id: configured.configuration.source_id.clone(),
                access,
                status,
                selected_music_folder_name: selected
                    .filter(|selected| selected.source_id == configured.configuration.source_id)
                    .and_then(|selected| {
                        let wanted = selected.music_folder_id.as_ref()?;
                        selected
                            .loaded
                            .music_folders()
                            .ok()?
                            .iter()
                            .find(|folder| &folder.id == wanted)
                            .map(|folder| folder.name.clone())
                    }),
                album_count,
                track_count,
            }
        })
        .collect::<Vec<_>>();
    ConfiguredSources {
        sources: sources.into(),
        selected_source_id: stored.sources.selected_source_id.clone(),
        local_folders: local_folders.into(),
        local_access: local_access.into(),
        first_run: stored.sources.configured.is_empty()
            || stored.sources.selected_source_id.is_none(),
    }
}

fn ui_selected(selected: SelectedSourceRuntime) -> SelectedLibrary {
    let playlist_tracks_can_repeat = selected.configuration.playlist_tracks_can_repeat();
    let artwork = selected.source.as_ref().map_or_else(
        || artwork::SourceImages::cache_only(selected.source_id().clone()),
        |source| artwork::SourceImages::new(Arc::clone(source)),
    );
    SelectedLibrary {
        source_id: selected.source_id().clone(),
        source_session_epoch: selected.source_session_epoch,
        music_folder_id: selected.music_folder_id,
        playlist_tracks_can_repeat,
        artwork,
        loaded: selected.loaded,
        home: selected.home,
    }
}

fn editable_source(configuration: &SourceConfiguration) -> Result<EditableSource, String> {
    match configuration.editable().map_err(string_error)? {
        sources::EditableSource::Credentials {
            credentials,
            jellyfin_use_instant_mix,
            ..
        } => Ok(EditableSource {
            source: SourceSummary {
                id: configuration.source_id.clone(),
                kind: configuration.kind.clone(),
                name: configuration.name.clone(),
            },
            credentials: CredentialPreset {
                source_name: credentials.server_name,
                server_url: credentials.server_url,
                username: credentials.username,
                trust_invalid_cert: credentials.trust_invalid_cert,
            },
            jellyfin_use_instant_mix,
        }),
        sources::EditableSource::Local { .. } => {
            Err("Local folders are edited from the Local source panel".to_string())
        }
    }
}

fn source_setup_input(input: SourceSetup, jellyfin_device_id: &str) -> SourceSetupInput {
    match input {
        SourceSetup::Jellyfin {
            credentials,
            use_instant_mix,
        } => SourceSetupInput::Jellyfin(JellyfinSetupInput {
            credentials: credential_host_input(credentials),
            use_instant_mix,
            device_id: jellyfin_device_id.to_string(),
        }),
        SourceSetup::OpenSubsonic { kind, credentials } => SourceSetupInput::Subsonic {
            flavor: subsonic_flavor(kind),
            credentials: credential_host_input(credentials),
        },
        SourceSetup::Local { roots } => SourceSetupInput::Local(LocalFolderHostInput { roots }),
    }
}

fn source_settings_input(input: SourceSettingsChange) -> SourceSettingsInput {
    match input {
        SourceSettingsChange::Jellyfin {
            source_id: _,
            credentials,
            use_instant_mix,
        } => SourceSettingsInput::Jellyfin(JellyfinSettingsInput {
            credentials: credential_settings_input(credentials),
            use_instant_mix,
        }),
        SourceSettingsChange::OpenSubsonic {
            source_id: _,
            kind: _,
            credentials,
        } => SourceSettingsInput::Subsonic(credential_settings_input(credentials)),
    }
}

fn source_settings_id(input: &SourceSettingsChange) -> &SourceId {
    match input {
        SourceSettingsChange::Jellyfin { source_id, .. }
        | SourceSettingsChange::OpenSubsonic { source_id, .. } => source_id,
    }
}

fn credential_host_input(input: CredentialInput) -> CredentialHostInput {
    CredentialHostInput {
        server_name: input.source_name,
        server_url: input.server_url,
        username: input.username,
        password: input.password,
        trust_invalid_cert: input.trust_invalid_cert,
    }
}

fn credential_settings_input(input: CredentialInput) -> CredentialSettingsInput {
    CredentialSettingsInput {
        name: input.source_name.unwrap_or_default(),
        base_url: input.server_url,
        username: input.username,
        password: input.password,
        trust_invalid_cert: input.trust_invalid_cert,
    }
}

fn subsonic_flavor(kind: OpenSubsonicKind) -> SubsonicFlavor {
    match kind {
        OpenSubsonicKind::Navidrome => SubsonicFlavor::Navidrome,
        OpenSubsonicKind::OpenSubsonic => SubsonicFlavor::Subsonic,
    }
}

fn local_roots(configuration: &SourceConfiguration) -> Result<Vec<PathBuf>, String> {
    match configuration.editable().map_err(string_error)? {
        sources::EditableSource::Local { roots, .. } => Ok(roots),
        _ => Err("the configured source is not Local".to_string()),
    }
}

fn source_progress(progress: SourceReadProgress) -> SourceProgress {
    SourceProgress {
        stage: match progress.stage {
            SourceReadStage::Albums => SourceProgressStage::Albums,
            SourceReadStage::Tracks => SourceProgressStage::Tracks,
            SourceReadStage::Artists => SourceProgressStage::Artists,
            SourceReadStage::Genres => SourceProgressStage::Genres,
            SourceReadStage::Playlists => SourceProgressStage::Playlists,
            SourceReadStage::Home => SourceProgressStage::Home,
            SourceReadStage::Artwork => SourceProgressStage::Artwork,
            SourceReadStage::Files => SourceProgressStage::Files,
            SourceReadStage::Finalizing => SourceProgressStage::Finalizing,
        },
        completed: progress.completed,
        total: progress.total,
    }
}

fn initial_progress() -> SourceProgress {
    SourceProgress {
        stage: SourceProgressStage::Connecting,
        completed: 0,
        total: None,
    }
}

fn favorite_value(loaded: &LoadedLibrary, item: &FavoriteItemId) -> Option<bool> {
    match item {
        FavoriteItemId::Track(id) => loaded.track(id).ok().flatten().map(|track| track.favorite),
        FavoriteItemId::Album(id) => loaded.album(id).ok().flatten().map(|album| album.favorite),
        FavoriteItemId::Artist(id) => loaded
            .artist(id)
            .ok()
            .flatten()
            .map(|artist| artist.favorite),
    }
}

pub(crate) fn source_access_unavailable() -> String {
    "Live source access is unavailable. Check the saved credentials and refresh.".to_string()
}

fn string_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}
