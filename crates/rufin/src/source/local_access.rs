use super::*;

pub(super) struct ActiveLocalAccess {
    pub(super) token: u64,
    pub(super) qualifier: SourceQualifier,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) handle: tokio::task::AbortHandle,
}

impl Drop for ActiveLocalAccess {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.handle.abort();
    }
}

impl SourceOwner {
    pub(super) async fn start_local_access_refresh(&mut self, selected: &SelectedSourceState) {
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

    pub(super) fn cancel_local_access(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .local_access
            .take();
    }

    pub(super) async fn finish_local_access(
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

    pub(super) async fn accept_local_change(
        &mut self,
        selected: Arc<SelectedSourceState>,
        change: LocalFilesystemChange,
        cancelled: Arc<AtomicBool>,
    ) {
        let Some(source) = selected.source.as_ref().cloned() else {
            return;
        };
        match prepare_local_change(
            source,
            Arc::clone(&selected.library),
            change,
            Arc::clone(&cancelled),
        )
        .await
        {
            Ok(Some(replacement)) => {
                let acceptance_owner = Arc::clone(&self.shared);
                let _acceptance = acceptance_owner.acceptance_lane.lock().await;
                if !self.shared.protect_interruptible_commit(&cancelled) {
                    return;
                }
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

    pub(super) async fn apply_local_access(
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

    pub(super) async fn save_metadata_local_access(
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

    pub(super) async fn remove_local_access(&mut self, source_id: SourceId) {
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
}

pub(super) async fn prepare_local_change(
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
pub(super) fn save_local_access_setting(
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
        configured.local_access = Some(local_access_mapping(access));
        Ok(())
    })
}
pub(super) fn accept_metadata_local_access_mapping(
    library: &Library,
    mapping: library::LocalAccessMapping,
    previous_access: Option<library::LocalAccessMapping>,
    save: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    library
        .accept_local_access_mapping(mapping)
        .map_err(string_error)?;
    let Err(error) = save() else {
        return Ok(());
    };
    let rollback = library
        .configure_local_access_mapping(previous_access)
        .map_err(string_error);
    match rollback {
        Ok(()) => Err(error),
        Err(rollback) => Err(format!(
            "{error} The previous local file mapping could not be restored: {rollback}"
        )),
    }
}
pub(super) fn local_access_mapping(access: &SourceLocalAccess) -> library::LocalAccessMapping {
    library::LocalAccessMapping {
        root_path: access.root_path.clone(),
        server_prefix: access.server_prefix.clone(),
        local_prefix: access.local_prefix.clone(),
    }
}
