//! The configured sources and the one selected source session.
//!
//! Rufin owns selection and operation ordering here. Concrete sources acquire
//! facts and perform provider operations; Library accepts and queries them.

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artwork::{Artwork, SourceImages};
use async_channel::{Receiver, Sender};
use library::{
    AcceptedHomeChange, AcceptedLibraryChange, CandidateChange, FavoriteItemId, FolderContents,
    HomeSectionKind, HomeSnapshot, Libraries, Library, LocalAccessTarget, MetadataDraft,
    MetadataEdit, MetadataEditing, MetadataError, MetadataItemId, MusicFolderId, PlaylistEdit,
    PlaylistTrackAdd, PreparedSourceCandidate, RecordedActivity, SmartPlaylistBuiltin,
    SmartPlaylistDefinition, SmartPlaylistId, SourceId, Track, TrackSort,
};
use playback::{PlaybackProjection, SourceSessionEpoch};
use scrobbling::Scrobbler;
use secrets::{SecretStorageMode, SwitchableSecretStore};
use sources::{
    CredentialHostInput, CredentialSettingsInput, JellyfinSettingsInput, JellyfinSetupInput,
    LocalFilesystemChange, LocalFolderHostInput, MetadataRefresh, NativeSourceResult, Source,
    SourceCacheMatch, SourceConfiguration, SourceEditResult, SourceFreshness, SourceInputIdentity,
    SourceLibraryChange, SourceLibraryChangeRead, SourceReadProgress, SourceReadStage,
    SourceSettingsInput, SourceSetupInput, SubsonicFlavor,
};
use tokio::task::JoinHandle;
use tracing::warn;
use ui::runtime::source::{
    ConfiguredSources, CredentialInput, CredentialPreset, DiscoveredServer, DiscoveryStatus,
    DiscoveryUpdate, EditableSource, LocalAccessStatus, LocalFolder, OpenSubsonicKind,
    SelectedSourcePort, SourceLocalAccess, SourceLocalAccessSummary, SourceOperation, SourcePort,
    SourceProgress, SourceProgressStage, SourceSettingsChange, SourceSetup, SourceSummary,
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

/// The current immutable facts for one selected-source session.
///
/// Rufin replaces this value atomically when the source executor, accepted
/// Library, Home, or music-folder scope changes. Consumers resolve it through
/// [`ActiveSource`] instead of retaining a second mutable mirror.
#[derive(Clone)]
pub(crate) struct SelectedSourceState {
    pub(crate) configuration: SourceConfiguration,
    pub(crate) source: Option<Arc<Source>>,
    pub(crate) source_session_epoch: SourceSessionEpoch,
    pub(crate) library: Arc<Library>,
    pub(crate) home: Arc<HomeSnapshot>,
    pub(crate) music_folder_id: Option<MusicFolderId>,
}

impl SelectedSourceState {
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
    ) -> Result<Option<MetadataContext>, library::LibraryQueryError> {
        let Some(source) = self.source.as_ref().cloned() else {
            return Ok(None);
        };
        let Some(subject) = self.library.metadata_subject(item_id)? else {
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
        self.library
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
                .library
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
            .library
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

/// A stable selected-session identity and fence.
///
/// The handle owns no selected facts. Resolving consults SourceOwner's one
/// authoritative slot, and returns `None` as soon as that session is retired.
pub(crate) struct ActiveSource {
    shared: Weak<Shared>,
    source_id: SourceId,
    source_session_epoch: SourceSessionEpoch,
    #[cfg(test)]
    fixed: Mutex<Option<Arc<SelectedSourceState>>>,
}

pub(crate) type WeakActiveSource = Weak<ActiveSource>;

impl ActiveSource {
    fn new(shared: &Arc<Shared>, state: &SelectedSourceState) -> Arc<Self> {
        Arc::new(Self {
            shared: Arc::downgrade(shared),
            source_id: state.source_id().clone(),
            source_session_epoch: state.source_session_epoch,
            #[cfg(test)]
            fixed: Mutex::new(None),
        })
    }

    pub(crate) fn resolve(&self) -> Option<Arc<SelectedSourceState>> {
        #[cfg(test)]
        if let Some(selected) = self
            .fixed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            return Some(selected);
        }
        self.shared
            .upgrade()?
            .resolve_selected(&self.source_id, self.source_session_epoch)
    }

    pub(crate) fn downgrade(self: &Arc<Self>) -> WeakActiveSource {
        Arc::downgrade(self)
    }

    #[cfg(test)]
    pub(crate) fn fixed_for_test(state: SelectedSourceState) -> Arc<Self> {
        let source_id = state.source_id().clone();
        let source_session_epoch = state.source_session_epoch;
        Arc::new(Self {
            shared: Weak::new(),
            source_id,
            source_session_epoch,
            fixed: Mutex::new(Some(Arc::new(state))),
        })
    }

    #[cfg(test)]
    pub(crate) fn replace_for_test(&self, state: SelectedSourceState) {
        *self
            .fixed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(state));
    }
}

fn resolve_observer_session(
    cancelled: &AtomicBool,
    session: &ActiveSource,
) -> Option<Arc<SelectedSourceState>> {
    (!cancelled.load(Ordering::Acquire))
        .then(|| session.resolve())
        .flatten()
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

#[derive(Clone)]
pub(crate) struct SourceOwner {
    shared: Arc<Shared>,
}

pub(crate) type SourceAcceptanceSender = SourceOwner;

struct Shared {
    artwork: Artwork,
    library: Libraries,
    downloads: Downloads,
    settings: SettingsFile,
    secrets: Arc<SwitchableSecretStore>,
    scrobbler: Arc<Scrobbler>,
    runtime: tokio::runtime::Handle,
    outputs: SourceOutputs,
    state: Mutex<OwnerState>,
    lane: tokio::sync::Mutex<()>,
    acceptance_lane: tokio::sync::Mutex<()>,
    interruptible: Mutex<Vec<Weak<AtomicBool>>>,
    playback: Mutex<Weak<PlaybackOwner>>,
    next_epoch: AtomicU64,
    next_token: AtomicU64,
    started: AtomicBool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceQualifier {
    source_id: SourceId,
    epoch: SourceSessionEpoch,
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

enum SelectedLibraryAcceptance {
    Source(library::SourceLibraryUpdate),
    Local(library::LocalComponentReplacement),
    Full(PreparedSourceCandidate),
}

enum PreparedConnectionLibrary {
    Candidate(Box<PreparedSourceCandidate>),
    Accepted {
        library: Arc<Library>,
        cache_match: SourceCacheMatch,
    },
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
    cancelled: Arc<AtomicBool>,
    handle: tokio::task::AbortHandle,
}

impl Drop for ActiveLocalAccess {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.handle.abort();
    }
}

struct ActiveAlbumRelease {
    token: u64,
    cancelled: Arc<AtomicBool>,
}

struct SelectedSlot {
    session: Arc<ActiveSource>,
    current: Arc<SelectedSourceState>,
}

struct SavedSourceConnection {
    configured: ConfiguredSource,
    previous: Option<ConfiguredSource>,
    previous_selected_source_id: Option<SourceId>,
    staged_credential: Option<CredentialRef>,
}

struct RefreshRequest {
    qualifier: SourceQualifier,
    visible: AtomicBool,
    cancelled: Arc<AtomicBool>,
}

struct OwnerState {
    selected: Option<SelectedSlot>,
    observer: Option<ActiveObserver>,
    local_access: Option<ActiveLocalAccess>,
    selected_revealed: bool,
    active_album_release: Option<ActiveAlbumRelease>,
    refresh: Option<Arc<RefreshRequest>>,
}

impl SourceOwner {
    pub(crate) fn open_dormant(
        artwork: Artwork,
        library: Libraries,
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
        let shared = Arc::new(Shared {
            artwork,
            library,
            downloads,
            settings,
            secrets,
            scrobbler,
            runtime,
            outputs,
            state: Mutex::new(OwnerState {
                selected: None,
                observer: None,
                local_access: None,
                selected_revealed: false,
                active_album_release: None,
                refresh: None,
            }),
            lane: tokio::sync::Mutex::new(()),
            acceptance_lane: tokio::sync::Mutex::new(()),
            interruptible: Mutex::new(Vec::new()),
            playback: Mutex::new(Weak::new()),
            next_epoch: AtomicU64::new(1),
            next_token: AtomicU64::new(1),
            started: AtomicBool::new(false),
        });
        let owner = Arc::new(Self { shared });
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
        self.clone()
    }

    pub(crate) fn start(&self) -> Result<(), String> {
        if self.shared.started.swap(true, Ordering::AcqRel) {
            return Err("the source owner is already running".to_string());
        }
        let periodic = self.clone();
        self.shared.runtime.spawn(async move {
            let mut interval = tokio::time::interval(SOURCE_CHECK_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                periodic.request_freshness_check();
            }
        });
        if let Some(source_id) = self.shared.settings.load().sources.selected_source_id {
            SourcePort::select_source(self, source_id);
        }
        Ok(())
    }

    pub(crate) fn album_release_settings_changed(&self, enabled: bool) {
        self.spawn_serialized(false, move |mut operations, _| async move {
            if enabled {
                operations.start_album_release_lookup();
            } else {
                operations.cancel_album_release_lookup(false);
            }
        });
    }

    fn spawn_serialized<F, Work>(&self, interruptible: bool, work: F)
    where
        F: FnOnce(SourceOwner, Arc<AtomicBool>) -> Work + Send + 'static,
        Work: Future<Output = ()> + Send + 'static,
    {
        self.spawn_serialized_with_cancel(interruptible, Arc::new(AtomicBool::new(false)), work);
    }

    fn spawn_serialized_with_cancel<F, Work>(
        &self,
        interruptible: bool,
        cancelled: Arc<AtomicBool>,
        work: F,
    ) where
        F: FnOnce(SourceOwner, Arc<AtomicBool>) -> Work + Send + 'static,
        Work: Future<Output = ()> + Send + 'static,
    {
        if interruptible {
            self.shared.register_interruptible(&cancelled);
        }
        let shared = Arc::clone(&self.shared);
        self.shared.runtime.spawn(async move {
            let lane_owner = Arc::clone(&shared);
            let _lane = lane_owner.lane.lock().await;
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            work(
                SourceOwner {
                    shared: Arc::clone(&shared),
                },
                cancelled,
            )
            .await;
        });
    }

    fn spawn_selected<F, Work>(&self, interruptible: bool, work: F)
    where
        F: FnOnce(SourceOwner, Arc<SelectedSourceState>, Arc<AtomicBool>) -> Work + Send + 'static,
        Work: Future<Output = ()> + Send + 'static,
    {
        let Some(session) = self.shared.selected_session() else {
            return;
        };
        self.spawn_serialized(interruptible, move |operations, cancelled| async move {
            let Some(selected) = session.resolve() else {
                return;
            };
            work(operations, selected, cancelled).await;
        });
    }

    fn spawn_transition<F, Work>(
        &self,
        operation: SourceOperation,
        failure_source: Option<SourceId>,
        add_form: bool,
        work: F,
    ) where
        F: FnOnce(SourceOwner, Arc<AtomicBool>) -> Work + Send + 'static,
        Work: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.shared.cancel_interruptible();
        self.shared.cancel_refresh();
        let _ = self
            .shared
            .outputs
            .events
            .try_send(SourceEvent::Operation(operation));
        self.spawn_serialized(true, move |mut operations, cancelled| async move {
            if let Err(error) = work(operations.clone(), Arc::clone(&cancelled)).await
                && !cancelled.load(Ordering::Acquire)
            {
                operations
                    .fail_transition(failure_source, error, add_form)
                    .await;
            }
        });
    }

    fn request_refresh(&self, source_id: SourceId, visible: bool) {
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
        else {
            return;
        };
        let qualifier = selected.qualifier();
        let request = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(refresh) = state
                .refresh
                .as_ref()
                .filter(|refresh| refresh.qualifier == qualifier)
            {
                if visible {
                    refresh.visible.store(true, Ordering::Release);
                }
                None
            } else {
                let request = Arc::new(RefreshRequest {
                    qualifier,
                    visible: AtomicBool::new(visible),
                    cancelled: Arc::new(AtomicBool::new(false)),
                });
                state.refresh = Some(Arc::clone(&request));
                Some(request)
            }
        };
        if visible {
            let _ = self.shared.outputs.events.try_send(SourceEvent::Operation(
                SourceOperation::Refreshing {
                    source_id: source_id.clone(),
                    progress: initial_progress(),
                },
            ));
        }
        let Some(request) = request else {
            return;
        };
        let request_for_work = Arc::clone(&request);
        self.spawn_serialized_with_cancel(
            true,
            Arc::clone(&request.cancelled),
            move |mut operations, cancelled| async move {
                operations.refresh(request_for_work, cancelled).await;
            },
        );
    }

    fn request_freshness_check(&self) {
        self.spawn_selected(true, |mut operations, selected, cancelled| async move {
            operations.check_freshness(selected, cancelled).await;
        });
    }
}

impl SourceOwner {
    pub(crate) fn publish_activity(
        &self,
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        update: RecordedActivity,
    ) {
        let shared = Arc::clone(&self.shared);
        self.shared.runtime.spawn(async move {
            SourceOwner { shared }
                .accept_activity(
                    SourceQualifier {
                        source_id,
                        epoch: source_session_epoch,
                    },
                    update,
                )
                .await;
        });
    }
}

impl SourceOwner {
    async fn apply_secret_storage_change(&mut self, mode: SecretStorageMode) -> Result<(), String> {
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

        if let Some(source_id) = transition_source_id {
            let configured = configured_source(&changed.sources, &source_id)?;
            let cancelled = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(|_: SourceReadProgress| {});
            if let Err(error) = select_source(self, configured, progress, cancelled).await {
                self.begin_transition().await;
                self.fail_transition(Some(source_id), error.clone(), false)
                    .await;
                return Err(error);
            }
        }
        Ok(())
    }

    async fn apply_source_update(
        &mut self,
        source_id: SourceId,
        input: SourceSettingsInput,
        local_roots_changed: bool,
        cancelled: Arc<AtomicBool>,
    ) {
        let configured = match configured_source(&self.shared.settings.load().sources, &source_id) {
            Ok(configured) => configured,
            Err(error) => return self.shared.warn_nonfatal(&error),
        };
        let selected = self
            .shared
            .selected()
            .is_some_and(|current| current.source_id() == &source_id);
        if selected && local_roots_changed {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Refreshing {
                    source_id: source_id.clone(),
                    progress: initial_progress(),
                }))
                .await;
        }
        let progress_source = (selected && local_roots_changed).then_some(source_id);
        let progress = self.progress(move |progress| {
            progress_source
                .as_ref()
                .map(|source_id| SourceOperation::Refreshing {
                    source_id: source_id.clone(),
                    progress,
                })
        });
        let result: Result<(), String> = async {
            let credential = load_credential(&self.shared, &configured).await?;
            match Source::edit(
                configured.configuration.clone(),
                credential,
                input,
                Some(self.shared.settings.load().jellyfin_device_id),
            )
            .await
            .map_err(string_error)?
            {
                SourceEditResult::Unchanged => {
                    self.shared.publish_configured().await;
                    Ok(())
                }
                SourceEditResult::ConfigurationOnly(configuration) => {
                    let saved = self
                        .save_source_connection(
                            Some(&configured),
                            configuration.clone(),
                            None,
                            false,
                            configured.music_folder_id.clone(),
                            configured.local_access.clone(),
                        )
                        .await?;
                    if selected && let Some(active) = self.shared.selected() {
                        let mut active = (*active).clone();
                        active.configuration = configuration;
                        self.shared.replace_selected(active);
                    }
                    self.finish_source_connection(saved).await;
                    self.shared.publish_configured().await;
                    Ok(())
                }
                SourceEditResult::Connected(connected) => {
                    let (configuration, source, credential) = connected.into_parts();
                    let same_account =
                        configuration.source_id == configured.configuration.source_id;
                    if !same_account
                        && self
                            .shared
                            .settings
                            .load()
                            .sources
                            .configured
                            .iter()
                            .any(|saved| saved.configuration.source_id == configuration.source_id)
                    {
                        return Err("this source account is already configured".to_string());
                    }
                    if !selected {
                        let saved = self
                            .save_source_connection(
                                Some(&configured),
                                configuration,
                                credential,
                                false,
                                same_account
                                    .then_some(configured.music_folder_id.clone())
                                    .flatten(),
                                configured.local_access.clone(),
                            )
                            .await?;
                        self.finish_source_connection(saved).await;
                        self.shared.publish_configured().await;
                        return Ok(());
                    }
                    let source = Arc::new(source);
                    let identity = configuration.input_identity().map_err(string_error)?;
                    let current = self
                        .shared
                        .selected()
                        .ok_or_else(|| "the selected source is no longer active".to_string())?;
                    let prepared_library =
                        if same_account && cache_input_matches(&identity, &current.library) {
                            PreparedConnectionLibrary::Accepted {
                                library: Arc::clone(&current.library),
                                cache_match: SourceCacheMatch::Exact,
                            }
                        } else {
                            PreparedConnectionLibrary::Candidate(Box::new(
                                prepare_source_candidate(
                                    &self.shared,
                                    Arc::clone(&source),
                                    identity,
                                    same_account.then(|| Arc::clone(&current.library)),
                                    progress,
                                    Arc::clone(&cancelled),
                                )
                                .await?,
                            ))
                        };
                    if cancelled.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    self.commit_selected_connection(
                        Some(configured),
                        configuration,
                        Some(source),
                        credential,
                        prepared_library,
                    )
                    .await
                }
            }
        }
        .await;
        if let Err(error) = result {
            self.selected_or_inactive_failure(selected, error).await;
        } else if local_roots_changed {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Idle))
                .await;
        }
    }

    async fn selected_or_inactive_failure(&mut self, selected: bool, error: String) {
        if selected {
            self.selected_update_failed(error).await;
        } else {
            self.shared.warn_nonfatal(&error);
        }
    }

    async fn start_selected_access(&mut self, catch_up: bool) {
        let Some(session) = self.shared.selected_session() else {
            return;
        };
        let Some(selected) = session.resolve() else {
            return;
        };
        let qualifier = selected.qualifier();
        {
            let state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state
                .observer
                .as_ref()
                .is_some_and(|observer| observer.qualifier == qualifier)
            {
                return;
            }
        }
        self.stop_observer();
        self.start_local_access_refresh(&selected).await;
        let Some(source) = selected.source.as_ref().cloned() else {
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let item_cancelled = Arc::clone(&cancelled);
        let local_cancelled = Arc::clone(&cancelled);
        let stop_cancelled = Arc::clone(&cancelled);
        let item_owner = SourceOwner {
            shared: Arc::clone(&self.shared),
        };
        let local_owner = item_owner.clone();
        let item_session = Arc::clone(&session);
        let local_session = Arc::clone(&session);
        let stop_session = Arc::clone(&session);
        let handle = self.shared.runtime.spawn(async move {
            let result = source
                .listen_selected_changes(
                    catch_up,
                    move |change| {
                        if resolve_observer_session(&item_cancelled, &item_session).is_none() {
                            return false;
                        }
                        let session = Arc::clone(&item_session);
                        let observer_cancelled = Arc::clone(&item_cancelled);
                        item_owner.spawn_serialized(
                            true,
                            move |mut operations, cancelled| async move {
                                if let Some(selected) =
                                    resolve_observer_session(&observer_cancelled, &session)
                                {
                                    operations
                                        .accept_observed_change(selected, change, cancelled)
                                        .await;
                                }
                            },
                        );
                        true
                    },
                    move |change| {
                        if resolve_observer_session(&local_cancelled, &local_session).is_none() {
                            return false;
                        }
                        let session = Arc::clone(&local_session);
                        let observer_cancelled = Arc::clone(&local_cancelled);
                        local_owner.spawn_serialized(
                            true,
                            move |mut operations, cancelled| async move {
                                if let Some(selected) =
                                    resolve_observer_session(&observer_cancelled, &session)
                                {
                                    operations
                                        .accept_local_change(selected, change, cancelled)
                                        .await;
                                }
                            },
                        );
                        true
                    },
                    move || {
                        stop_cancelled.load(Ordering::Acquire) || stop_session.resolve().is_none()
                    },
                )
                .await;
            if let Err(error) = result {
                warn!(%error, "selected source change feed stopped");
            }
        });
        let observer = ActiveObserver {
            qualifier,
            cancelled,
            handle,
        };
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state
            .selected
            .as_ref()
            .is_some_and(|slot| Arc::ptr_eq(&slot.session, &session))
        {
            state.observer = Some(observer);
        }
        drop(state);
        if catch_up {
            SourceOwner {
                shared: Arc::clone(&self.shared),
            }
            .request_freshness_check();
        }
    }

    fn stop_observer(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .observer
            .take();
    }

    fn retire_selected_access(&self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.observer.take();
        state.local_access.take();
    }

    async fn start_local_access_refresh(&mut self, selected: &SelectedSourceState) {
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
        let baseline = match selected.library.local_access_files() {
            Ok(files) => files,
            Err(error) => {
                self.shared.warn_nonfatal(&error.to_string());
                return;
            }
        };
        self.cancel_local_access();
        let token = self.shared.next_token.fetch_add(1, Ordering::AcqRel);
        let qualifier = selected.qualifier();
        let task_input = input.clone();
        let task_qualifier = qualifier.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_cancelled = Arc::clone(&cancelled);
        let owner = SourceOwner {
            shared: Arc::clone(&self.shared),
        };
        let handle = self.shared.runtime.spawn(async move {
            let root = task_input.root_path.clone();
            let scan_cancelled = Arc::clone(&task_cancelled);
            let result = tokio::task::spawn_blocking(move || {
                sources::read_local_access(&root, &baseline, &|_| {}, &|| {
                    scan_cancelled.load(Ordering::Acquire)
                })
                .map_err(string_error)
            })
            .await
            .map_err(string_error)
            .and_then(|result| result);
            owner.spawn_serialized(false, move |mut operations, _| async move {
                operations
                    .finish_local_access(token, task_qualifier, task_input, result)
                    .await;
            });
        });
        let active = ActiveLocalAccess {
            token,
            qualifier,
            cancelled,
            handle: handle.abort_handle(),
        };
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .local_access = Some(active);
    }

    fn cancel_local_access(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .local_access
            .take();
    }

    async fn finish_local_access(
        &mut self,
        token: u64,
        qualifier: SourceQualifier,
        input: SourceLocalAccess,
        result: Result<Vec<library::LocalAccessFile>, String>,
    ) {
        let present = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state
                .local_access
                .as_ref()
                .is_none_or(|active| active.token != token)
            {
                false
            } else {
                state.local_access.take();
                true
            }
        };
        if !present {
            return;
        }
        let outcome = async {
            let files = result?;
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
            let library = Arc::clone(&selected.library);
            let mapping = local_access_mapping(&input);
            blocking(move || {
                library
                    .replace_local_access(mapping, files)
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
            self.shared.warn_nonfatal(&error);
        }
    }

    async fn begin_transition(&mut self) {
        self.retire_selected_session().await;
        self.shared.release_selected().await;
    }

    async fn retire_selected_session(&mut self) {
        self.cancel_album_release_lookup(true);
        self.retire_selected_access();
        self.shared.stop_playback().await;
    }

    fn start_album_release_lookup(&mut self) {
        let Some(session) = self.shared.selected_session() else {
            return;
        };
        let Some(selected) = session.resolve() else {
            return;
        };
        let token = self.shared.next_token.fetch_add(1, Ordering::AcqRel);
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.selected_revealed
                || state.active_album_release.is_some()
                || !self
                    .shared
                    .settings
                    .load()
                    .ui
                    .allows_external_metadata_lookup()
            {
                return;
            }
            state.active_album_release = Some(ActiveAlbumRelease {
                token,
                cancelled: Arc::clone(&cancelled),
            });
        }
        let settings = self.shared.settings.clone();
        let events = self.shared.outputs.events.clone();
        let source_id = selected.source_id().clone();
        let source_session_epoch = selected.source_session_epoch;
        let selected = session.downgrade();
        let shared = Arc::clone(&self.shared);
        drop(self.shared.runtime.spawn_blocking(move || {
            run_selected_album_release_lookup(
                settings,
                events,
                source_id,
                source_session_epoch,
                selected,
                cancelled,
            );
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state
                .active_album_release
                .as_ref()
                .is_some_and(|active| active.token == token)
            {
                state.active_album_release = None;
            }
        }));
    }

    fn cancel_album_release_lookup(&mut self, reset_reveal: bool) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = state.active_album_release.take() {
            active.cancelled.store(true, Ordering::Release);
        }
        if reset_reveal {
            state.selected_revealed = false;
        }
    }

    fn progress<F>(&self, operation: F) -> Arc<dyn Fn(SourceReadProgress) + Send + Sync>
    where
        F: Fn(SourceProgress) -> Option<SourceOperation> + Send + Sync + 'static,
    {
        let events = self.shared.outputs.events.clone();
        Arc::new(move |progress| {
            if let Some(operation) = operation(source_progress(progress)) {
                let _ = events.try_send(SourceEvent::Operation(operation));
            }
        })
    }

    async fn refresh(&mut self, request: Arc<RefreshRequest>, cancelled: Arc<AtomicBool>) {
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.qualifier() == request.qualifier)
        else {
            self.shared.finish_refresh(&request);
            return;
        };
        let source_id = selected.source_id().clone();
        let visible = Arc::clone(&request);
        let progress = self.progress(move |progress| {
            visible
                .visible
                .load(Ordering::Acquire)
                .then(|| SourceOperation::Refreshing {
                    source_id: source_id.clone(),
                    progress,
                })
        });
        let prepared = prepare_refresh_candidate(
            Arc::clone(&self.shared),
            (*selected).clone(),
            progress,
            Arc::clone(&cancelled),
        )
        .await;
        if cancelled.load(Ordering::Acquire) {
            self.shared.finish_refresh(&request);
            return;
        }
        let visible = request.visible.load(Ordering::Acquire);
        match prepared {
            Ok(prepared) => {
                let acceptance_owner = Arc::clone(&self.shared);
                let _acceptance = acceptance_owner.acceptance_lane.lock().await;
                if let Err(error) = self
                    .commit_refresh(Arc::clone(&selected), prepared, visible)
                    .await
                {
                    self.refresh_failed(&selected, visible, error).await;
                }
            }
            Err(error) => self.refresh_failed(&selected, visible, error).await,
        }
        self.shared.finish_refresh(&request);
    }

    async fn accept_activity(&mut self, qualifier: SourceQualifier, update: RecordedActivity) {
        let acceptance_owner = Arc::clone(&self.shared);
        let _acceptance = acceptance_owner.acceptance_lane.lock().await;
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.qualifier() == qualifier)
        else {
            return;
        };
        let library = Arc::clone(&selected.library);
        match blocking(move || {
            library
                .apply_recorded_activity(&update)
                .map_err(string_error)
        })
        .await
        {
            Ok(Some(change)) => self.publish_accepted_change(&selected, change).await,
            Ok(None) => {}
            Err(error) => warn!(%error, "could not apply accepted playback activity"),
        }
    }

    async fn commit_selected_connection(
        &mut self,
        previous: Option<ConfiguredSource>,
        configuration: SourceConfiguration,
        source: Option<Arc<Source>>,
        credential: Option<String>,
        prepared_library: PreparedConnectionLibrary,
    ) -> Result<(), String> {
        let same_session = self
            .shared
            .selected()
            .is_some_and(|selected| selected.source_id() == &configuration.source_id);
        let acceptance_owner = Arc::clone(&self.shared);
        let _acceptance = if same_session {
            Some(acceptance_owner.acceptance_lane.lock().await)
        } else {
            None
        };
        if same_session {
            let current = self
                .shared
                .selected()
                .filter(|selected| selected.source_id() == &configuration.source_id)
                .ok_or_else(|| "the selected source changed while it was prepared".to_string())?;
            let saved = self
                .save_source_connection(
                    previous.as_ref(),
                    configuration.clone(),
                    credential,
                    true,
                    previous
                        .as_ref()
                        .and_then(|configured| configured.music_folder_id.clone()),
                    previous
                        .as_ref()
                        .and_then(|configured| configured.local_access.clone()),
                )
                .await?;
            self.retire_selected_access();
            let updated = match prepared_library {
                PreparedConnectionLibrary::Candidate(candidate) => self
                    .accept_same_session_candidate(
                        &current,
                        configuration,
                        source,
                        saved.configured.music_folder_id.clone(),
                        saved.configured.local_access.clone(),
                        *candidate,
                    )
                    .await
                    .map(|_| ()),
                PreparedConnectionLibrary::Accepted { library, .. } => {
                    let mut selected = (*current).clone();
                    selected.configuration = configuration;
                    selected.source = source;
                    selected.library = library;
                    self.shared
                        .replace_selected_runtime(selected)
                        .await
                        .then_some(())
                        .ok_or_else(|| "the selected source changed before cutover".to_string())
                }
            };
            if let Err(error) = updated {
                self.rollback_source_connection(saved).await;
                return Err(error);
            }
            self.start_selected_access(true).await;
            self.shared.publish_configured().await;
            self.finish_source_connection(saved).await;
            return Ok(());
        }
        let (library, cache_match) = match prepared_library {
            PreparedConnectionLibrary::Candidate(candidate) => (
                blocking(move || {
                    (*candidate)
                        .accept()
                        .map(|commit| commit.library)
                        .map_err(string_error)
                })
                .await?,
                None,
            ),
            PreparedConnectionLibrary::Accepted {
                library,
                cache_match,
            } => (library, Some(cache_match)),
        };
        let replaces_account = previous
            .as_ref()
            .is_some_and(|previous| previous.configuration.source_id != configuration.source_id);
        let selected_source_id = configuration.source_id.clone();
        if replaces_account || previous.is_none() {
            let library = Arc::clone(&library);
            blocking(move || {
                library
                    .initialize_smart_playlists()
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await?;
        }
        let music_folder_id = normalize_music_folder(
            &library,
            previous
                .as_ref()
                .filter(|_| !replaces_account)
                .and_then(|configured| configured.music_folder_id.clone()),
        )?;
        let local_access = previous
            .as_ref()
            .and_then(|configured| configured.local_access.clone());
        if let Some(access) = local_access.as_ref() {
            let library = Arc::clone(&library);
            let access = access.clone();
            blocking(move || {
                library
                    .configure_local_access(configured_local_access_mapping(&access))
                    .map_err(string_error)
            })
            .await?;
        }
        let home = {
            let library = Arc::clone(&library);
            let folder = music_folder_id.clone();
            blocking(move || library.home(folder.as_ref()).map_err(string_error)).await?
        };
        let selected = Arc::new(SelectedSourceState {
            configuration: configuration.clone(),
            source,
            source_session_epoch: SourceSessionEpoch::new(
                self.shared.next_epoch.fetch_add(1, Ordering::AcqRel),
            ),
            library,
            home,
            music_folder_id,
        });
        let session = ActiveSource::new(&self.shared, &selected);
        let playback = self.shared.playback()?;
        let prepared_playback = {
            let playback = Arc::clone(&playback);
            let session = Arc::clone(&session);
            let selected = Arc::clone(&selected);
            blocking(move || playback.prepare_selected(session, selected)).await?
        };
        let saved = self
            .save_source_connection(
                previous.as_ref(),
                configuration,
                credential,
                true,
                selected.music_folder_id.clone(),
                local_access,
            )
            .await?;
        if saved.previous.as_ref().is_some_and(|previous| {
            previous.configuration.source_id != saved.configured.configuration.source_id
        }) {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Switching {
                    target: selected_source_id.clone(),
                    progress: initial_progress(),
                }))
                .await;
        }
        self.cancel_album_release_lookup(true);
        self.retire_selected_access();
        let cutover = {
            let playback = Arc::clone(&playback);
            blocking(move || Ok(playback.stop_for_source_switch())).await?
        };
        self.shared.release_selected().await;
        self.shared
            .install_selected_slot(Arc::clone(&session), Arc::clone(&selected));
        self.shared.attach_selected_downloads(&selected).await;
        let playback = playback.install_prepared(prepared_playback, cutover);
        self.shared
            .publish_selected(session, Arc::clone(&selected), playback)
            .await;
        self.start_selected_access(cache_match == Some(SourceCacheMatch::Exact))
            .await;
        self.finish_source_connection(saved).await;
        if cache_match == Some(SourceCacheMatch::ReaderUpgrade) && selected.source.is_some() {
            SourceOwner {
                shared: Arc::clone(&self.shared),
            }
            .request_refresh(selected_source_id, true);
        } else {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Idle))
                .await;
        }
        Ok(())
    }

    async fn commit_refresh(
        &mut self,
        previous: Arc<SelectedSourceState>,
        prepared: PreparedSourceCandidate,
        visible: bool,
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
        let accepted = self
            .accept_same_session_candidate(
                &previous,
                previous.configuration.clone(),
                previous.source.clone(),
                requested_folder.clone(),
                local_access,
                prepared,
            )
            .await?;
        if let Some(selected) = accepted {
            if selected.music_folder_id != requested_folder {
                let settings = self.shared.settings.clone();
                let source_id = previous.source_id().clone();
                let folder = selected.music_folder_id.clone();
                if let Err(error) =
                    blocking(move || save_music_folder(&settings, &source_id, folder)).await
                {
                    warn!(%error, source_id = %previous.source_id(), "could not save the normalized music folder");
                }
            }
            self.start_local_access_refresh(&selected).await;
        }
        if visible {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Idle))
                .await;
        }
        Ok(())
    }

    async fn accept_same_session_candidate(
        &mut self,
        previous: &SelectedSourceState,
        configuration: SourceConfiguration,
        source: Option<Arc<Source>>,
        requested_folder: Option<MusicFolderId>,
        local_access: Option<ConfiguredLocalAccess>,
        candidate: PreparedSourceCandidate,
    ) -> Result<Option<SelectedSourceState>, String> {
        let change = candidate.change();
        if change == CandidateChange::None {
            let commit = blocking(move || candidate.accept().map_err(string_error)).await?;
            let source_changed = match (&previous.source, &source) {
                (Some(previous), Some(next)) => !Arc::ptr_eq(previous, next),
                (None, None) => false,
                _ => true,
            };
            if previous.configuration != configuration || source_changed {
                let mut selected = previous.clone();
                selected.configuration = configuration;
                selected.source = source;
                selected.library = commit.library;
                if !self.shared.replace_selected_runtime(selected.clone()).await {
                    return Err("the selected source changed before cutover".to_string());
                }
                return Ok(Some(selected));
            }
            return Ok(None);
        }
        let folder = normalize_music_folder(candidate.library(), requested_folder)?;
        if let Some(access) = local_access {
            let library = Arc::clone(candidate.library());
            blocking(move || {
                library
                    .configure_local_access(configured_local_access_mapping(&access))
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await?;
        }
        let playback = if change == CandidateChange::Library {
            let playback = self.shared.playback()?;
            let refresh = playback.prepare_track_refresh(previous.source_session_epoch)?;
            Some((playback, refresh))
        } else {
            None
        };
        let commit = blocking(move || candidate.accept().map_err(string_error)).await?;
        let library = Arc::clone(&commit.library);
        let home_folder = folder.clone();
        let home =
            blocking(move || library.home(home_folder.as_ref()).map_err(string_error)).await?;
        let selected = SelectedSourceState {
            configuration,
            source,
            source_session_epoch: previous.source_session_epoch,
            library: commit.library,
            home,
            music_folder_id: folder,
        };
        if let Some((playback, refresh)) = playback {
            self.cancel_album_release_lookup(false);
            self.shared
                .publish_library_replacement(selected.clone())
                .await;
            let library = Arc::clone(&selected.library);
            if let Err(error) =
                blocking(move || playback.apply_track_refresh(refresh, &library)).await
            {
                warn!(%error, "could not update Playback after accepting refreshed source facts");
            }
            self.start_album_release_lookup();
        } else {
            self.shared.publish_home_replacement(selected.clone()).await;
        }
        Ok(Some(selected))
    }

    async fn refresh_failed(&self, selected: &SelectedSourceState, visible: bool, error: String) {
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

    async fn selected_update_failed(&mut self, error: String) {
        self.shared.warn_nonfatal(&error);
        self.shared
            .send_event(SourceEvent::Operation(SourceOperation::Idle))
            .await;
        self.start_selected_access(true).await;
    }

    async fn accept_selected_library_acceptance(
        &mut self,
        selected: Arc<SelectedSourceState>,
        acceptance: SelectedLibraryAcceptance,
    ) -> Result<(), String> {
        let change = match acceptance {
            SelectedLibraryAcceptance::Source(update) => {
                let library = Arc::clone(&selected.library);
                blocking(move || library.accept_source_update(update).map_err(string_error)).await?
            }
            SelectedLibraryAcceptance::Local(replacement) => {
                let library = Arc::clone(&selected.library);
                blocking(move || {
                    library
                        .accept_local_component(replacement)
                        .map_err(string_error)
                })
                .await?
            }
            SelectedLibraryAcceptance::Full(candidate) => {
                return self.commit_refresh(selected, candidate, false).await;
            }
        };
        if let Some(change) = change {
            self.publish_accepted_change(&selected, change).await;
        }
        Ok(())
    }

    async fn refresh_home(&mut self, selected: Arc<SelectedSourceState>, kind: HomeSectionKind) {
        let source_section = match selected.source.as_ref() {
            Some(source) => match source.home_section(kind).await.map_err(string_error) {
                Ok(NativeSourceResult::Available(section)) => Some(section),
                Ok(NativeSourceResult::Unavailable) => None,
                Err(error) => {
                    self.shared.warn_nonfatal(&error);
                    return;
                }
            },
            None => None,
        };
        if !self.shared.matches_selected(&selected.qualifier()) {
            return;
        }
        let library = Arc::clone(&selected.library);
        let folder = selected.music_folder_id.clone();
        let current = Arc::clone(&selected.home);
        let home = blocking(move || {
            match source_section {
                Some(section) => library.accept_home_section(folder.as_ref(), &current, section),
                None => library.refresh_rufin_home_section(folder.as_ref(), &current, kind),
            }
            .map_err(string_error)
        })
        .await;
        match home {
            Ok(home) => {
                let mut replacement = (*selected).clone();
                replacement.home = Arc::clone(&home);
                if self.shared.replace_selected(replacement) {
                    self.shared
                        .send_event(SourceEvent::Home(HomePublication {
                            source_id: selected.source_id().clone(),
                            source_session_epoch: selected.source_session_epoch,
                            kind,
                            home,
                        }))
                        .await;
                }
            }
            Err(error) => self.shared.warn_nonfatal(&error),
        }
    }

    async fn set_favorite(
        &mut self,
        selected: Arc<SelectedSourceState>,
        item: FavoriteItemId,
        favorite: bool,
        previous: bool,
    ) {
        let result = match selected.source.as_ref() {
            Some(source) => source
                .set_favorite(item.clone(), favorite)
                .await
                .map_err(string_error),
            None => Err(source_access_unavailable()),
        };
        let acceptance = match result {
            Ok(acceptance) => acceptance,
            Err(message) => {
                self.shared
                    .send_event(SourceEvent::FavoriteFailure(FavoriteFailure {
                        source_id: selected.source_id().clone(),
                        source_session_epoch: selected.source_session_epoch,
                        item_id: item,
                        authoritative_favorite: previous,
                        message,
                    }))
                    .await;
                return;
            }
        };
        if !self.shared.matches_selected(&selected.qualifier()) {
            return;
        }
        let library = Arc::clone(&selected.library);
        match blocking(move || library.accept_favorite(acceptance).map_err(string_error)).await {
            Ok(change) => {
                self.publish_accepted_change(&selected, change).await;
            }
            Err(message) => {
                self.shared
                    .send_event(SourceEvent::FavoriteFailure(FavoriteFailure {
                        source_id: selected.source_id().clone(),
                        source_session_epoch: selected.source_session_epoch,
                        item_id: item,
                        authoritative_favorite: previous,
                        message,
                    }))
                    .await;
            }
        }
    }

    async fn edit_playlist(&mut self, selected: Arc<SelectedSourceState>, edit: PlaylistEdit) {
        let acceptance = match selected.source.as_ref() {
            Some(source) => source.edit_playlist(edit).await.map_err(string_error),
            None => Err(source_access_unavailable()),
        };
        let acceptance = match acceptance {
            Ok(acceptance) => acceptance,
            Err(error) => {
                self.shared.warn_nonfatal(&error);
                return;
            }
        };
        if !self.shared.matches_selected(&selected.qualifier()) {
            return;
        }
        let library = Arc::clone(&selected.library);
        match blocking(move || library.accept_playlist(acceptance).map_err(string_error)).await {
            Ok(Some(change)) => self.publish_accepted_change(&selected, change).await,
            Ok(None) => {}
            Err(error) => self.shared.warn_nonfatal(&error),
        }
    }

    async fn smart_playlist(
        &mut self,
        selected: Arc<SelectedSourceState>,
        operation: SmartPlaylistOperation,
    ) {
        let library = Arc::clone(&selected.library);
        let change = blocking(move || match operation {
            SmartPlaylistOperation::Create { name, definition } => library
                .create_smart_playlist(name, definition)
                .map_err(string_error),
            SmartPlaylistOperation::Update {
                id,
                name,
                definition,
            } => library
                .update_smart_playlist(id, name, definition)
                .map_err(string_error),
            SmartPlaylistOperation::Delete(id) => {
                library.delete_smart_playlist(&id).map_err(string_error)
            }
            SmartPlaylistOperation::Restore(builtin) => library
                .restore_builtin_smart_playlist(builtin)
                .map_err(string_error),
            SmartPlaylistOperation::Move {
                dragged,
                target,
                after,
            } => library
                .move_smart_playlist_relative(dragged, target, after)
                .map_err(string_error),
        })
        .await;
        match change {
            Ok(Some(change)) => self.publish_accepted_change(&selected, change).await,
            Ok(None) => {}
            Err(error) => self.shared.warn_nonfatal(&error),
        }
    }

    async fn accept_observed_change(
        &mut self,
        selected: Arc<SelectedSourceState>,
        change: SourceLibraryChange,
        _cancelled: Arc<AtomicBool>,
    ) {
        let Some(source) = selected.source.as_ref() else {
            return;
        };
        match source
            .read_library_change(&selected.library, change)
            .await
            .map_err(string_error)
        {
            Ok(SourceLibraryChangeRead::Exact(update)) => {
                if let Err(error) = self
                    .accept_selected_library_acceptance(
                        Arc::clone(&selected),
                        SelectedLibraryAcceptance::Source(update),
                    )
                    .await
                {
                    warn!(%error, "could not accept a selected source update");
                }
            }
            Ok(SourceLibraryChangeRead::Full) => SourceOwner {
                shared: Arc::clone(&self.shared),
            }
            .request_refresh(selected.source_id().clone(), false),
            Ok(SourceLibraryChangeRead::Ignored) => {}
            Err(error) => warn!(%error, "background selected source update failed"),
        }
    }

    async fn accept_local_change(
        &mut self,
        selected: Arc<SelectedSourceState>,
        change: LocalFilesystemChange,
        cancelled: Arc<AtomicBool>,
    ) {
        let Some(source) = selected.source.as_ref().cloned() else {
            return;
        };
        match prepare_local_change(source, Arc::clone(&selected.library), change, cancelled).await {
            Ok(Some(replacement)) => {
                if let Err(error) = self
                    .accept_selected_library_acceptance(
                        Arc::clone(&selected),
                        SelectedLibraryAcceptance::Local(replacement),
                    )
                    .await
                {
                    warn!(%error, "could not accept a selected Local update");
                }
            }
            Ok(None) => {}
            Err(error) => warn!(%error, "background selected Local update failed"),
        }
    }

    async fn check_freshness(
        &mut self,
        selected: Arc<SelectedSourceState>,
        _cancelled: Arc<AtomicBool>,
    ) {
        let Some(source) = selected.source.as_ref() else {
            return;
        };
        let freshness = match selected.library.provider_freshness().map_err(string_error) {
            Ok(freshness) => freshness,
            Err(error) => {
                warn!(%error, "could not check selected source freshness");
                return;
            }
        };
        match source.check_freshness(freshness.as_ref()).await {
            Ok(SourceFreshness::Changed(_)) => SourceOwner {
                shared: Arc::clone(&self.shared),
            }
            .request_refresh(selected.source_id().clone(), false),
            Ok(
                SourceFreshness::Unavailable | SourceFreshness::Unchanged | SourceFreshness::Busy,
            ) => {}
            Err(error) => warn!(%error, "could not check selected source freshness"),
        }
    }

    async fn edit_metadata(
        &mut self,
        selected: Arc<SelectedSourceState>,
        edit: MetadataEdit,
        cancelled: Arc<AtomicBool>,
        mut reply: MetadataReply,
    ) {
        let progress = self.progress(|_| None);
        let acceptance = prepare_metadata_acceptance(
            Arc::clone(&self.shared),
            (*selected).clone(),
            edit,
            progress,
            cancelled,
            &mut reply,
        )
        .await;
        let accepted = match acceptance {
            Err(error) => Err(error),
            Ok(acceptance) => {
                let acceptance_owner = Arc::clone(&self.shared);
                let _acceptance = acceptance_owner.acceptance_lane.lock().await;
                self.accept_selected_library_acceptance(selected, acceptance)
                    .await
                    .map_err(MetadataError::SavedRefreshFailed)
            }
        };
        reply.finish(accepted);
    }

    async fn publish_accepted_change(
        &mut self,
        selected: &SelectedSourceState,
        change: AcceptedLibraryChange,
    ) {
        if !self.shared.matches_selected(&selected.qualifier()) {
            return;
        }
        let downloads_changed = change.download_coverage_changed;
        let album_release_candidates_changed = change.album_release_candidates_changed;
        if album_release_candidates_changed {
            self.cancel_album_release_lookup(false);
        }
        let tracks = change
            .tracks
            .iter()
            .filter_map(|replacement| replacement.track.clone())
            .collect::<Vec<_>>();
        if !tracks.is_empty() {
            let refreshed = match self.shared.playback() {
                Ok(playback) => {
                    let source_id = selected.source_id().clone();
                    let epoch = selected.source_session_epoch;
                    blocking(move || playback.refresh_accepted_tracks(&source_id, epoch, tracks))
                        .await
                }
                Err(error) => Err(error),
            };
            match refreshed {
                Ok(()) => {}
                Err(error) => {
                    self.shared.warn_nonfatal(&error);
                }
            }
        }
        let home = if change.home == AcceptedHomeChange::Keep {
            None
        } else {
            let library = Arc::clone(&selected.library);
            let current = Arc::clone(&selected.home);
            let folder = selected.music_folder_id.clone();
            let home_change = change.home.clone();
            match blocking(move || {
                library
                    .home_after_accepted_change(folder.as_ref(), &current, &home_change)
                    .map_err(string_error)
            })
            .await
            {
                Ok(home) => home,
                Err(error) => {
                    warn!(%error, source_id = %selected.source_id(), "could not prepare Home after an accepted Library change");
                    None
                }
            }
        };
        if let Some(home) = &home {
            let Some(active) = self
                .shared
                .selected()
                .filter(|active| active.qualifier() == selected.qualifier())
            else {
                return;
            };
            let mut replacement = (*active).clone();
            replacement.home = Arc::clone(&home);
            self.shared.replace_selected(replacement);
        }
        if downloads_changed {
            self.shared
                .downloads
                .library_changed(Arc::clone(&selected.library), change.clone());
        }
        self.shared
            .send_event(SourceEvent::LibraryUpdate(SelectedLibraryUpdate {
                source_id: selected.source_id().clone(),
                source_session_epoch: selected.source_session_epoch,
                change,
                home,
            }))
            .await;
        if album_release_candidates_changed {
            self.start_album_release_lookup();
        }
    }

    async fn fail_transition(
        &mut self,
        source_id: Option<SourceId>,
        message: String,
        add_form: bool,
    ) {
        self.shared
            .send_event(SourceEvent::Operation(SourceOperation::Failed {
                source_id,
                message,
                add_form,
            }))
            .await;
    }

    async fn apply_local_access(
        &mut self,
        input: SourceLocalAccess,
        completion: Sender<Result<(), String>>,
    ) {
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
            let library = Arc::clone(&selected.library);
            let mapping = local_access_mapping(&input);
            if let Err(error) = blocking(move || {
                library
                    .configure_local_access(mapping)
                    .map(|_| ())
                    .map_err(string_error)
            })
            .await
            {
                self.shared.warn_nonfatal(&error);
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

    async fn save_metadata_local_access(
        &mut self,
        selected: Arc<SelectedSourceState>,
        input: SourceLocalAccess,
        item_id: MetadataItemId,
        completion: Sender<Result<(), String>>,
    ) {
        if selected.source_id() != &input.source_id {
            let _ = completion
                .send(Err(
                    "the local file mapping belongs to a different source".to_string()
                ))
                .await;
            return;
        }
        let mapping = local_access_mapping(&input);
        let context = match selected
            .library
            .metadata_subject_with_local_access(&item_id, Some(&mapping))
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
        if !self.shared.matches_selected(&selected.qualifier()) {
            let _ = completion
                .send(Err(
                    "the metadata item belongs to an inactive source session".to_string(),
                ))
                .await;
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
        let library = Arc::clone(&selected.library);
        let settings = self.shared.settings.clone();
        let saved = input.clone();
        if let Err(error) = blocking(move || {
            accept_metadata_local_access_mapping(&library, mapping, previous_access, || {
                save_local_access_setting(&settings, &saved)
            })
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
    }

    async fn remove_local_access(&mut self, source_id: SourceId) {
        let cancels_scan = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .local_access
            .as_ref()
            .is_some_and(|active| active.qualifier.source_id == source_id);
        if cancels_scan {
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
                let store_result = if let Some(selected) = selected.as_ref() {
                    let library = Arc::clone(&selected.library);
                    blocking(move || {
                        library
                            .clear_local_access()
                            .map(|_| ())
                            .map_err(string_error)
                    })
                    .await
                } else {
                    let libraries = self.shared.library.clone();
                    blocking(move || {
                        libraries
                            .discard_local_access(source_id.clone())
                            .map_err(string_error)
                    })
                    .await
                };
                if let Err(error) = store_result {
                    self.shared.warn_nonfatal(&error);
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
            Err(error) => self.shared.warn_nonfatal(&error),
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
                self.shared.warn_nonfatal(&error);
            }
            return;
        }
        self.remove_replaced_source_data(source_id.clone()).await;
        if let Some(reference) = removed.and_then(|source| source.credential_ref) {
            self.delete_staged_credential(Some(&reference)).await;
        }
        if let Some(replacement) = replacement {
            let configured =
                match configured_source(&self.shared.settings.load().sources, &replacement) {
                    Ok(configured) => configured,
                    Err(error) => {
                        self.shared.warn_nonfatal(&error);
                        return;
                    }
                };
            let cancelled = Arc::new(AtomicBool::new(false));
            let progress = self.progress(|_| None);
            if let Err(error) = select_source(self, configured, progress, cancelled).await {
                self.shared.warn_nonfatal(&error);
            }
            return;
        }
        self.shared.publish_configured().await;
        self.shared
            .send_event(SourceEvent::Operation(SourceOperation::Idle))
            .await;
    }

    async fn set_music_folder(
        &mut self,
        selected: Arc<SelectedSourceState>,
        folder_id: Option<MusicFolderId>,
    ) {
        let source_id = selected.source_id().clone();
        let folder_id = match normalize_music_folder(&selected.library, folder_id) {
            Ok(folder) => folder,
            Err(error) => {
                self.shared.warn_nonfatal(&error);
                return;
            }
        };
        let library = Arc::clone(&selected.library);
        let home_folder = folder_id.clone();
        let home = match blocking(move || library.home(home_folder.as_ref()).map_err(string_error))
            .await
        {
            Ok(home) => home,
            Err(error) => {
                self.shared.warn_nonfatal(&error);
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
            self.shared.warn_nonfatal(&error);
            return;
        }
        let mut replacement = (*selected).clone();
        replacement.home = home;
        replacement.music_folder_id = folder_id;
        self.shared.publish_library_replacement(replacement).await;
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

    async fn save_source_connection(
        &self,
        previous: Option<&ConfiguredSource>,
        configuration: SourceConfiguration,
        credential: Option<String>,
        select: bool,
        music_folder_id: Option<MusicFolderId>,
        local_access: Option<ConfiguredLocalAccess>,
    ) -> Result<SavedSourceConnection, String> {
        let replaced_source_id = previous
            .filter(|previous| previous.configuration.source_id != configuration.source_id)
            .map(|previous| previous.configuration.source_id.clone());
        let previous_credential = previous.and_then(|source| source.credential_ref.clone());
        let staged_credential = if let Some(credential) = credential {
            let reference = fresh_credential_ref()?;
            let secrets = Arc::clone(&self.shared.secrets);
            let saved_reference = reference.clone();
            blocking(move || save_provider_secret(&secrets, &saved_reference, credential)).await?;
            Some(reference)
        } else {
            None
        };
        let configured = ConfiguredSource {
            configuration,
            credential_ref: if replaced_source_id.is_some() {
                staged_credential.clone()
            } else {
                staged_credential
                    .clone()
                    .or_else(|| previous_credential.clone())
            },
            music_folder_id,
            local_access,
        };
        let previous_selected_source_id = self.shared.settings.load().sources.selected_source_id;
        let settings = self.shared.settings.clone();
        let previous_id = previous.map(|source| source.configuration.source_id.clone());
        let saved = configured.clone();
        let source_id = saved.configuration.source_id.clone();
        if let Err(error) = blocking(move || {
            settings.update(|stored| {
                if previous_id
                    .as_ref()
                    .is_some_and(|previous| previous != &source_id)
                    && stored
                        .sources
                        .configured
                        .iter()
                        .any(|source| source.configuration.source_id == source_id)
                {
                    return Err("this source account is already configured".to_string());
                }
                if let Some(previous) = previous_id.as_ref() {
                    let source = stored
                        .sources
                        .configured
                        .iter_mut()
                        .find(|source| &source.configuration.source_id == previous)
                        .ok_or_else(|| "the configured source no longer exists".to_string())?;
                    *source = saved.clone();
                } else {
                    stored.sources.configured.push(saved.clone());
                }
                if select {
                    stored.sources.selected_source_id = Some(source_id.clone());
                }
                Ok(())
            })
        })
        .await
        {
            self.delete_staged_credential(staged_credential.as_ref())
                .await;
            return Err(error);
        }
        Ok(SavedSourceConnection {
            configured,
            previous: previous.cloned(),
            previous_selected_source_id,
            staged_credential,
        })
    }

    async fn rollback_source_connection(&self, saved: SavedSourceConnection) {
        let SavedSourceConnection {
            configured,
            previous,
            previous_selected_source_id,
            staged_credential,
            ..
        } = saved;
        let settings = self.shared.settings.clone();
        let replacement_id = configured.configuration.source_id;
        match blocking(move || {
            settings.update(|stored| {
                let position = stored
                    .sources
                    .configured
                    .iter()
                    .position(|source| source.configuration.source_id == replacement_id)
                    .ok_or_else(|| "the replacement source no longer exists".to_string())?;
                if let Some(previous) = previous.clone() {
                    stored.sources.configured[position] = previous;
                } else {
                    stored.sources.configured.remove(position);
                }
                stored.sources.selected_source_id = previous_selected_source_id.clone();
                Ok(())
            })
        })
        .await
        {
            Ok(()) => {
                self.delete_staged_credential(staged_credential.as_ref())
                    .await;
            }
            Err(error) => {
                warn!(%error, "could not restore source settings after a failed cutover");
            }
        }
    }

    async fn finish_source_connection(&self, saved: SavedSourceConnection) {
        let previous_credential = saved
            .previous
            .as_ref()
            .and_then(|previous| previous.credential_ref.as_ref());
        if previous_credential != saved.configured.credential_ref.as_ref() {
            self.delete_staged_credential(previous_credential).await;
        }
        if let Some(previous) = saved.previous.filter(|previous| {
            previous.configuration.source_id != saved.configured.configuration.source_id
        }) {
            self.remove_replaced_source_data(previous.configuration.source_id)
                .await;
        }
    }

    async fn remove_replaced_source_data(&self, source_id: SourceId) {
        let library = self
            .shared
            .selected()
            .filter(|selected| selected.source_id() == &source_id)
            .map(|selected| Arc::clone(&selected.library));
        self.shared
            .downloads
            .clear(source_id.clone(), library, false);
        let library = self.shared.library.clone();
        let source_for_store = source_id.clone();
        if let Err(error) = blocking(move || {
            library
                .remove_source_data(&source_for_store)
                .map_err(string_error)
        })
        .await
        {
            self.shared.warn_nonfatal(&error);
        }
        if let Err(error) = self.shared.artwork.invalidate_source(&source_id) {
            self.shared.warn_nonfatal(&error.to_string());
        }
    }
}

impl Shared {
    fn selected_session(&self) -> Option<Arc<ActiveSource>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .selected
            .as_ref()
            .map(|slot| Arc::clone(&slot.session))
    }

    fn selected(&self) -> Option<Arc<SelectedSourceState>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .selected
            .as_ref()
            .map(|slot| Arc::clone(&slot.current))
    }

    fn resolve_selected(
        &self,
        source_id: &SourceId,
        epoch: SourceSessionEpoch,
    ) -> Option<Arc<SelectedSourceState>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .selected
            .as_ref()
            .filter(|slot| {
                slot.current.source_id() == source_id && slot.current.source_session_epoch == epoch
            })
            .map(|slot| Arc::clone(&slot.current))
    }

    fn replace_selected(&self, selected: SelectedSourceState) -> bool {
        let qualifier = selected.qualifier();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(slot) = state
            .selected
            .as_mut()
            .filter(|slot| slot.current.qualifier() == qualifier)
        else {
            return false;
        };
        slot.current = Arc::new(selected);
        true
    }

    fn install_selected_slot(
        &self,
        session: Arc<ActiveSource>,
        selected: Arc<SelectedSourceState>,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .selected = Some(SelectedSlot {
            session,
            current: selected,
        });
    }

    fn matches_selected(&self, qualifier: &SourceQualifier) -> bool {
        self.selected()
            .is_some_and(|selected| selected.qualifier() == *qualifier)
    }

    fn register_interruptible(&self, cancelled: &Arc<AtomicBool>) {
        let mut active = self
            .interruptible
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active.retain(|cancelled| cancelled.strong_count() > 0);
        active.push(Arc::downgrade(cancelled));
    }

    fn cancel_interruptible(&self) {
        for cancelled in self
            .interruptible
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .filter_map(|cancelled| cancelled.upgrade())
        {
            cancelled.store(true, Ordering::Release);
        }
    }

    fn cancel_refresh(&self) {
        if let Some(refresh) = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .refresh
            .take()
        {
            refresh.cancelled.store(true, Ordering::Release);
        }
    }

    fn finish_refresh(&self, request: &Arc<RefreshRequest>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state
            .refresh
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, request))
        {
            state.refresh = None;
        }
    }

    fn playback(&self) -> Result<Arc<PlaybackOwner>, String> {
        self.playback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade()
            .ok_or_else(|| "Playback is not attached to the selected source owner".to_string())
    }

    async fn publish_selected(
        &self,
        session: Arc<ActiveSource>,
        selected: Arc<SelectedSourceState>,
        playback: PlaybackProjection,
    ) {
        if let Ok(playback_owner) = self.playback() {
            playback_owner.publish_selected_products(&playback);
        }
        let stored = self.settings.load();
        let selected = ui_selected(selected, session);
        self.send_event(SourceEvent::Selected {
            configured: configured_sources(&stored, Some(&selected)),
            selected,
            playback: Box::new(playback),
        })
        .await;
    }

    async fn publish_library_replacement(&self, selected: SelectedSourceState) {
        if !self.replace_selected_runtime(selected).await {
            return;
        }
        let Some(session) = self.selected_session() else {
            return;
        };
        let Some(selected) = session.resolve() else {
            return;
        };
        let selected = ui_selected(selected, session);
        self.send_event(SourceEvent::LibraryReplaced {
            configured: configured_sources(&self.settings.load(), Some(&selected)),
            selected,
        })
        .await;
    }

    async fn publish_home_replacement(&self, selected: SelectedSourceState) {
        let source_id = selected.source_id().clone();
        let source_session_epoch = selected.source_session_epoch;
        let home = Arc::clone(&selected.home);
        if !self.replace_selected_runtime(selected).await {
            return;
        }
        self.send_event(SourceEvent::HomeReplaced {
            source_id,
            source_session_epoch,
            home,
        })
        .await;
    }

    async fn replace_selected_runtime(&self, selected: SelectedSourceState) -> bool {
        if !self.replace_selected(selected) {
            return false;
        }
        if let Some(selected) = self.selected() {
            self.attach_selected_downloads(&selected).await;
        }
        true
    }

    async fn attach_selected_downloads(&self, selected: &SelectedSourceState) {
        if let Err(error) = self
            .downloads
            .attach(
                selected.source.clone(),
                &selected.library,
                selected.music_folder_id.clone(),
            )
            .await
        {
            self.warn_nonfatal(&error);
        }
    }

    async fn publish_configured(&self) {
        let selected = self.selected_session().and_then(|session| {
            let selected = session.resolve()?;
            Some(ui_selected(selected, session))
        });
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
        if let Err(error) = blocking(move || Ok(playback.stop_for_source_switch())).await {
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
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.selected = None;
        state.observer = None;
        state.local_access = None;
    }

    fn warn_nonfatal(&self, error: &str) {
        warn!(%error, "operation was not available");
    }

    async fn send_event(&self, event: SourceEvent) {
        if self.outputs.events.send(event).await.is_err() {
            warn!("source event lane is unavailable");
        }
    }
}

async fn add_source(
    owner: &mut SourceOwner,
    input: SourceSetup,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<(), String> {
    let shared = Arc::clone(&owner.shared);
    let setup = source_setup_input(input, &shared.settings.load().jellyfin_device_id);
    let connected = Source::connect(setup).await.map_err(string_error)?;
    if cancelled.load(Ordering::Acquire) {
        return Err("source connection was cancelled".to_string());
    }
    let (configuration, source, credential) = connected.into_parts();
    let identity = configuration.input_identity().map_err(string_error)?;
    let source = Arc::new(source);
    let prepared = prepare_source_candidate(
        &shared,
        Arc::clone(&source),
        identity,
        None,
        progress,
        Arc::clone(&cancelled),
    )
    .await?;
    if cancelled.load(Ordering::Acquire) {
        return Err("source connection was cancelled".to_string());
    }
    let previous = shared
        .settings
        .load()
        .sources
        .configured
        .iter()
        .find(|configured| configured.configuration.source_id == configuration.source_id)
        .cloned();
    if cancelled.load(Ordering::Acquire) {
        return Err("source selection was cancelled".to_string());
    }
    owner
        .commit_selected_connection(
            previous,
            configuration,
            Some(source),
            credential,
            PreparedConnectionLibrary::Candidate(Box::new(prepared)),
        )
        .await
}

async fn select_source(
    owner: &mut SourceOwner,
    configured: ConfiguredSource,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<(), String> {
    let shared = Arc::clone(&owner.shared);
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
    let prepared_library = if let Some((library, cache_match)) = cached {
        PreparedConnectionLibrary::Accepted {
            library,
            cache_match,
        }
    } else {
        let source = source.as_ref().ok_or_else(source_access_unavailable)?;
        let candidate = prepare_source_candidate(
            &shared,
            Arc::clone(source),
            identity,
            None,
            progress,
            Arc::clone(&cancelled),
        )
        .await?;
        if cancelled.load(Ordering::Acquire) {
            return Err("source selection was cancelled".to_string());
        }
        PreparedConnectionLibrary::Candidate(Box::new(candidate))
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("source selection was cancelled".to_string());
    }
    owner
        .commit_selected_connection(
            Some(configured),
            configuration,
            source,
            None,
            prepared_library,
        )
        .await
}

async fn prepare_refresh_candidate(
    shared: Arc<Shared>,
    selected: SelectedSourceState,
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
    prepare_source_candidate(
        &shared,
        source,
        identity,
        Some(Arc::clone(&selected.library)),
        progress,
        cancelled,
    )
    .await
}

async fn prepare_source_candidate(
    shared: &Shared,
    source: Arc<Source>,
    identity: SourceInputIdentity,
    base: Option<Arc<Library>>,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedSourceCandidate, String> {
    let prepared = Arc::clone(&source)
        .prepare_library_candidate(
            shared.library.clone(),
            identity,
            base,
            Arc::clone(&progress),
            Arc::clone(&cancelled),
        )
        .await
        .map_err(string_error)?;
    prepare_candidate_artwork(shared, source, prepared, progress, cancelled).await
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
        let source_artwork = prepared.library().source_artwork().map_err(string_error)?;
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

async fn prepare_local_change(
    source: Arc<Source>,
    loaded: Arc<Library>,
    change: LocalFilesystemChange,
    cancelled: Arc<AtomicBool>,
) -> Result<Option<library::LocalComponentReplacement>, String> {
    blocking(move || {
        let should_stop = || cancelled.load(Ordering::Acquire);
        let progress = |_: SourceReadProgress| {};
        source
            .prepare_local_change(&loaded, change, unix_seconds(), &progress, &should_stop)
            .map_err(string_error)
    })
    .await
}

async fn prepare_metadata_acceptance(
    shared: Arc<Shared>,
    selected: SelectedSourceState,
    edit: MetadataEdit,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
    reply: &mut MetadataReply,
) -> Result<SelectedLibraryAcceptance, MetadataError> {
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
            match source
                .read_library_change(&selected.library, change)
                .await
                .map_err(string_error)
            {
                Ok(SourceLibraryChangeRead::Exact(update)) => {
                    Ok(SelectedLibraryAcceptance::Source(update))
                }
                Ok(SourceLibraryChangeRead::Ignored) => Err(MetadataError::SavedRefreshFailed(
                    "the source did not return the written item".to_string(),
                )),
                Ok(SourceLibraryChangeRead::Full) => {
                    prepare_refresh_candidate(shared, selected, progress, cancelled)
                        .await
                        .map(SelectedLibraryAcceptance::Full)
                        .map_err(MetadataError::SavedRefreshFailed)
                }
                Err(error) => Err(MetadataError::SavedRefreshFailed(error.to_string())),
            }
        }
        MetadataRefresh::Local(change) => {
            let library = Arc::clone(&selected.library);
            let source = Arc::clone(&source);
            prepare_local_change(source, library, change, cancelled)
                .await
                .and_then(|replacement| {
                    replacement.ok_or_else(|| {
                        "the written files were not accepted by the Local source".to_string()
                    })
                })
                .map(SelectedLibraryAcceptance::Local)
                .map_err(MetadataError::SavedRefreshFailed)
        }
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
        self.shared.runtime.spawn(async move {
            let update =
                match sources::discover_jellyfin_servers(Duration::from_millis(1_500)).await {
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
        self.spawn_transition(
            SourceOperation::Adding {
                progress: initial_progress(),
            },
            None,
            true,
            move |mut operations, cancelled| async move {
                let progress =
                    operations.progress(|progress| Some(SourceOperation::Adding { progress }));
                add_source(&mut operations, input, progress, cancelled).await
            },
        );
    }

    fn update_source(&self, input: SourceSettingsChange) {
        let source_id = source_settings_id(&input).clone();
        let input = source_settings_input(input);
        self.spawn_serialized(true, move |mut operations, cancelled| async move {
            operations
                .apply_source_update(source_id, input, false, cancelled)
                .await;
        });
    }

    fn select_source(&self, source_id: SourceId) {
        if self
            .shared
            .selected()
            .is_some_and(|selected| selected.source_id() == &source_id)
        {
            let _ = self
                .shared
                .outputs
                .events
                .try_send(SourceEvent::Operation(SourceOperation::Idle));
            return;
        }
        let target = source_id.clone();
        self.spawn_transition(
            SourceOperation::Switching {
                target: source_id.clone(),
                progress: initial_progress(),
            },
            Some(source_id.clone()),
            false,
            move |mut operations, cancelled| async move {
                let configured =
                    configured_source(&operations.shared.settings.load().sources, &source_id)?;
                let progress_target = target.clone();
                let progress = operations.progress(move |progress| {
                    Some(SourceOperation::Switching {
                        target: progress_target.clone(),
                        progress,
                    })
                });
                select_source(&mut operations, configured, progress, cancelled).await
            },
        );
    }

    fn change_secret_storage(&self, mode: SecretStorageMode) -> Receiver<Result<(), String>> {
        let (result, receiver) = async_channel::bounded(1);
        self.shared.cancel_interruptible();
        self.spawn_serialized(false, move |mut operations, _| async move {
            let changed = operations.apply_secret_storage_change(mode).await;
            let _ = result.send(changed).await;
        });
        receiver
    }

    fn add_local_folder(&self, path: PathBuf) {
        let local = self
            .shared
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
            .cloned();
        let Some(local) = local else {
            self.configure_source(SourceSetup::Local { roots: vec![path] });
            return;
        };
        let mut roots = match local_roots(&local.configuration) {
            Ok(roots) => roots,
            Err(error) => {
                self.shared.warn_nonfatal(&error);
                return;
            }
        };
        if roots.contains(&path) {
            return;
        }
        roots.push(path);
        let source_id = local.configuration.source_id;
        self.spawn_serialized(true, move |mut operations, cancelled| async move {
            operations
                .apply_source_update(
                    source_id,
                    SourceSettingsInput::Local { roots },
                    true,
                    cancelled,
                )
                .await;
        });
    }

    fn remove_local_folder(&self, path: String) {
        let local = self
            .shared
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
            .cloned();
        let Some(local) = local else {
            self.shared.warn_nonfatal("Local is not configured");
            return;
        };
        let mut roots = match local_roots(&local.configuration) {
            Ok(roots) => roots,
            Err(error) => {
                self.shared.warn_nonfatal(&error);
                return;
            }
        };
        roots.retain(|root| root.to_string_lossy() != path);
        let source_id = local.configuration.source_id;
        if roots.is_empty() {
            self.forget_source(source_id);
            return;
        }
        self.spawn_serialized(true, move |mut operations, cancelled| async move {
            operations
                .apply_source_update(
                    source_id,
                    SourceSettingsInput::Local { roots },
                    true,
                    cancelled,
                )
                .await;
        });
    }

    fn refresh_source(&self, source_id: SourceId) {
        self.request_refresh(source_id, true);
    }

    fn check_for_source_changes(&self) {
        self.request_freshness_check();
    }

    fn save_local_access(&self, input: SourceLocalAccess) -> Receiver<Result<(), String>> {
        let (result, receiver) = async_channel::bounded(1);
        self.spawn_serialized(false, move |mut operations, _| async move {
            operations.apply_local_access(input, result).await;
        });
        receiver
    }

    fn clear_local_access(&self, source_id: SourceId) {
        self.spawn_serialized(false, move |mut operations, _| async move {
            operations.remove_local_access(source_id).await;
        });
    }

    fn forget_source(&self, source_id: SourceId) {
        self.shared.cancel_interruptible();
        self.spawn_serialized(false, move |mut operations, _| async move {
            operations.forget_now(source_id).await;
        });
    }
}

impl ActiveSource {
    fn owner(&self) -> Option<SourceOwner> {
        Some(SourceOwner {
            shared: self.shared.upgrade()?,
        })
    }

    fn spawn_selected<F, Work>(&self, interruptible: bool, work: F)
    where
        F: FnOnce(SourceOwner, Arc<SelectedSourceState>, Arc<AtomicBool>) -> Work + Send + 'static,
        Work: Future<Output = ()> + Send + 'static,
    {
        let Some(owner) = self.owner() else {
            return;
        };
        let source_id = self.source_id.clone();
        let epoch = self.source_session_epoch;
        owner.spawn_serialized(interruptible, move |operations, cancelled| async move {
            let Some(selected) = operations.shared.resolve_selected(&source_id, epoch) else {
                return;
            };
            work(operations, selected, cancelled).await;
        });
    }
}

impl SelectedSourcePort for ActiveSource {
    fn selected_library_revealed(&self) {
        self.spawn_selected(false, |mut operations, _, _| async move {
            operations
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .selected_revealed = true;
            operations.start_album_release_lookup();
        });
    }

    fn refresh_home(&self, kind: HomeSectionKind) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations.refresh_home(selected, kind).await;
        });
    }

    fn set_music_folder(&self, folder_id: Option<MusicFolderId>) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations.set_music_folder(selected, folder_id).await;
        });
    }

    fn set_favorite(&self, item: FavoriteItemId, favorite: bool) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            let previous = favorite_value(&selected.library, &item).unwrap_or(!favorite);
            operations
                .set_favorite(selected, item, favorite, previous)
                .await;
        });
    }

    fn add_playlist_tracks(&self, request: PlaylistTrackAdd) -> usize {
        let Some(selected) = self.resolve() else {
            return 0;
        };
        let edit = match selected.library.prepare_playlist_add(request) {
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
        self.edit_playlist(edit);
        count
    }

    fn edit_playlist(&self, edit: PlaylistEdit) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations.edit_playlist(selected, edit).await;
        });
    }

    fn folder(
        &self,
        folder_id: Option<library::FolderId>,
        music_folder_id: Option<MusicFolderId>,
    ) -> Receiver<Result<FolderContents, String>> {
        let (result, receiver) = async_channel::bounded(1);
        let selected = self.resolve();
        let runtime = self.shared.upgrade().map(|shared| shared.runtime.clone());
        if let (Some(selected), Some(runtime)) = (selected, runtime) {
            runtime.spawn(async move {
                let provider = match selected.source.as_ref() {
                    Some(source) => Some(
                        source
                            .folder(folder_id.as_ref(), music_folder_id.as_ref())
                            .await,
                    ),
                    None => None,
                };
                let value = route_folder_result(
                    Arc::clone(&selected.library),
                    folder_id.as_ref(),
                    music_folder_id.as_ref(),
                    provider,
                );
                let _ = result.send(value).await;
            });
        }
        receiver
    }

    fn search(
        &self,
        request: library::SearchRequest,
    ) -> Receiver<Result<library::SearchResults, String>> {
        let (result, receiver) = async_channel::bounded(1);
        let selected = self.resolve();
        let runtime = self.shared.upgrade().map(|shared| shared.runtime.clone());
        if let (Some(selected), Some(runtime)) = (selected, runtime) {
            runtime.spawn(async move {
                let provider = match selected.source.as_ref() {
                    Some(source) => Some(source.search(&request).await),
                    None => None,
                };
                let value =
                    route_search_result(Arc::clone(&selected.library), request, provider).await;
                let _ = result.send(value).await;
            });
        }
        receiver
    }

    fn metadata_editing_available(&self, item_id: &MetadataItemId) -> bool {
        self.resolve()
            .is_some_and(|selected| selected.metadata_editing_available(item_id))
    }

    fn metadata(&self, item_id: MetadataItemId) -> Receiver<Result<MetadataDraft, MetadataError>> {
        let (result, receiver) = async_channel::bounded(1);
        let selected = self.resolve();
        let shared = self.shared.upgrade();
        if let (Some(selected), Some(shared)) = (selected, shared) {
            let runtime = shared.runtime.clone();
            runtime.spawn(async move {
                let qualifier = selected.qualifier();
                let value = fence_selected_completion(
                    &shared,
                    &qualifier,
                    async {
                        match selected.metadata_access_context(&item_id) {
                            Ok(Some(context)) => {
                                context
                                    .source
                                    .read_metadata(context.subject, context.local_access)
                                    .await
                            }
                            Ok(None) => Err(MetadataError::Unavailable),
                            Err(error) => Err(error),
                        }
                    },
                    Err(MetadataError::Unavailable),
                )
                .await;
                let _ = result.send(value).await;
            });
        }
        receiver
    }

    fn edit_metadata(&self, edit: MetadataEdit) -> Receiver<Result<(), MetadataError>> {
        let (result, receiver) = async_channel::bounded(1);
        let reply = MetadataReply::new(result);
        if edit.changes.is_empty() {
            reply.finish(Ok(()));
            return receiver;
        }
        self.spawn_selected(
            false,
            move |mut operations, selected, cancelled| async move {
                operations
                    .edit_metadata(selected, edit, cancelled, reply)
                    .await;
            },
        );
        receiver
    }

    fn identify_metadata(
        &self,
        item_id: MetadataItemId,
        editing: MetadataEditing,
        values: library::MetadataValues,
    ) -> Receiver<Result<Option<library::MetadataValues>, String>> {
        let (result, receiver) = async_channel::bounded(1);
        let external_lookup_allowed = self
            .shared
            .upgrade()
            .is_some_and(|shared| shared.settings.load().ui.allows_external_metadata_lookup());
        let context = self
            .resolve()
            .and_then(|selected| selected.metadata_context(&item_id).ok().flatten());
        let shared = self.shared.upgrade();
        let qualifier = SourceQualifier {
            source_id: self.source_id.clone(),
            epoch: self.source_session_epoch,
        };
        if let (Some(context), Some(shared)) = (context, shared) {
            let runtime = shared.runtime.clone();
            runtime.spawn(async move {
                let value = fence_selected_completion(
                    &shared,
                    &qualifier,
                    async {
                        let direct_applicable = external_lookup_allowed
                            && item_id.has_exact_musicbrainz_identity(&values);
                        let source_search_applicable =
                            context.source.metadata_source_search(&context.subject)
                                && !values.title.trim().is_empty();
                        if !direct_applicable && !source_search_applicable {
                            Ok(None)
                        } else {
                            identify_metadata_with_fallback(
                                context.source,
                                context.subject,
                                item_id,
                                editing,
                                values,
                                direct_applicable,
                                source_search_applicable,
                            )
                            .await
                        }
                    },
                    Ok(None),
                )
                .await;
                let _ = result.send(value).await;
            });
        }
        receiver
    }

    fn save_metadata_local_access(
        &self,
        input: SourceLocalAccess,
        item_id: MetadataItemId,
    ) -> Receiver<Result<(), String>> {
        let (result, receiver) = async_channel::bounded(1);
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations
                .save_metadata_local_access(selected, input, item_id, result)
                .await;
        });
        receiver
    }

    fn create_smart_playlist(&self, name: String, definition: SmartPlaylistDefinition) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations
                .smart_playlist(
                    selected,
                    SmartPlaylistOperation::Create { name, definition },
                )
                .await;
        });
    }

    fn update_smart_playlist(
        &self,
        id: SmartPlaylistId,
        name: String,
        definition: SmartPlaylistDefinition,
    ) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations
                .smart_playlist(
                    selected,
                    SmartPlaylistOperation::Update {
                        id,
                        name,
                        definition,
                    },
                )
                .await;
        });
    }

    fn delete_smart_playlist(&self, id: SmartPlaylistId) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations
                .smart_playlist(selected, SmartPlaylistOperation::Delete(id))
                .await;
        });
    }

    fn restore_builtin_smart_playlist(&self, builtin: SmartPlaylistBuiltin) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations
                .smart_playlist(selected, SmartPlaylistOperation::Restore(builtin))
                .await;
        });
    }

    fn move_smart_playlist(&self, dragged: SmartPlaylistId, target: SmartPlaylistId, after: bool) {
        self.spawn_selected(false, move |mut operations, selected, _| async move {
            operations
                .smart_playlist(
                    selected,
                    SmartPlaylistOperation::Move {
                        dragged,
                        target,
                        after,
                    },
                )
                .await;
        });
    }
}

async fn fence_selected_completion<T>(
    shared: &Shared,
    qualifier: &SourceQualifier,
    completion: impl Future<Output = T>,
    retired: T,
) -> T {
    let value = completion.await;
    if shared.matches_selected(qualifier) {
        value
    } else {
        retired
    }
}

async fn identify_metadata_with_fallback(
    source: Arc<Source>,
    subject: library::MetadataSubject,
    item_id: MetadataItemId,
    editing: MetadataEditing,
    current: library::MetadataValues,
    direct_applicable: bool,
    source_search_applicable: bool,
) -> Result<Option<library::MetadataValues>, String> {
    let direct_item_id = item_id;
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

fn route_folder_result(
    loaded: Arc<Library>,
    folder_id: Option<&library::FolderId>,
    music_folder_id: Option<&MusicFolderId>,
    provider: Option<sources::SourceResult<NativeSourceResult<FolderContents>>>,
) -> Result<FolderContents, String> {
    match provider {
        Some(Ok(NativeSourceResult::Available(contents))) => {
            reconcile_folder_contents(&loaded, contents)
        }
        Some(Ok(NativeSourceResult::Unavailable)) | None => {
            cached_folder_contents(loaded, folder_id, music_folder_id)
        }
        Some(Err(error)) if source_error_allows_cache(&error) => {
            cached_folder_contents(loaded, folder_id, music_folder_id)
        }
        Some(Err(error)) => Err(error.to_string()),
    }
}

async fn route_search_result(
    loaded: Arc<Library>,
    request: library::SearchRequest,
    provider: Option<sources::SourceResult<NativeSourceResult<library::SearchResults>>>,
) -> Result<library::SearchResults, String> {
    match provider {
        Some(Ok(NativeSourceResult::Available(results))) => {
            reconcile_search_results(&loaded, results)
        }
        Some(Ok(NativeSourceResult::Unavailable)) | None => cached_search(loaded, request).await,
        Some(Err(error)) if source_error_allows_cache(&error) => {
            cached_search(loaded, request).await
        }
        Some(Err(error)) => Err(error.to_string()),
    }
}

fn cached_folder_contents(
    loaded: Arc<Library>,
    folder_id: Option<&library::FolderId>,
    music_folder_id: Option<&MusicFolderId>,
) -> Result<FolderContents, String> {
    let local = loaded
        .local_folder_contents(folder_id)
        .map_err(string_error)?;
    if folder_id.is_some() && local.is_some() {
        return Ok(local.unwrap_or_default());
    }
    if let Some(local) = local
        && (!local.folders.is_empty() || !local.tracks.is_empty())
    {
        return Ok(local);
    }
    let tracks = loaded
        .track_list(music_folder_id, TrackSort::Title, false)
        .and_then(|tracks| tracks.materialize_owned())
        .map_err(string_error)?;
    Ok(FolderContents {
        folders: Arc::from([]),
        tracks: tracks.into(),
    })
}

fn source_error_allows_cache(error: &sources::SourceError) -> bool {
    matches!(
        error,
        sources::SourceError::Network(_)
            | sources::SourceError::Server {
                status: 500..=599,
                ..
            }
    )
}

fn reconcile_folder_contents(
    loaded: &Library,
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
    loaded: &Library,
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

fn accepted_track_or(loaded: &Library, track: Track) -> Result<Track, String> {
    loaded
        .track(&track.id)
        .map(|accepted| accepted.unwrap_or(track))
        .map_err(string_error)
}

async fn cached_search(
    loaded: Arc<Library>,
    request: library::SearchRequest,
) -> Result<library::SearchResults, String> {
    blocking(move || loaded.search(&request).map_err(string_error)).await
}

fn normalize_music_folder(
    loaded: &Library,
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

fn cache_input_matches(identity: &SourceInputIdentity, loaded: &Library) -> bool {
    loaded_input_identity(loaded) == *identity
}

fn loaded_input_identity(loaded: &Library) -> SourceInputIdentity {
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
    mapping: library::LocalAccessMapping,
    previous_access: Option<ConfiguredLocalAccess>,
    save: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    library
        .accept_local_access_mapping(mapping)
        .map_err(string_error)?;
    let Err(error) = save() else {
        return Ok(());
    };
    let rollback = library
        .configure_local_access_mapping(
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
                .and_then(|selected| selected.library.counts().ok())
                .map(|counts| (counts.albums, counts.tracks))
                .unwrap_or_default();
            let status = access
                .as_ref()
                .and_then(|_| {
                    selected
                        .filter(|selected| selected.source_id == configured.configuration.source_id)
                        .and_then(|selected| selected.library.local_access_status().ok())
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
                            .library
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

fn ui_selected(
    selected: Arc<SelectedSourceState>,
    operations: Arc<ActiveSource>,
) -> SelectedLibrary {
    let playlist_tracks_can_repeat = selected.configuration.playlist_tracks_can_repeat();
    let artwork = selected.source.as_ref().map_or_else(
        || artwork::SourceImages::cache_only(selected.source_id().clone()),
        |source| artwork::SourceImages::new(Arc::clone(source)),
    );
    SelectedLibrary {
        source_id: selected.source_id().clone(),
        source_session_epoch: selected.source_session_epoch,
        music_folder_id: selected.music_folder_id.clone(),
        playlist_tracks_can_repeat,
        artwork,
        library: Arc::clone(&selected.library),
        home: Arc::clone(&selected.home),
        operations,
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

fn favorite_value(loaded: &Library, item: &FavoriteItemId) -> Option<bool> {
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
