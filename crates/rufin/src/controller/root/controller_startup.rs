use super::*;
use library::AlbumIdentityCandidate;
#[cfg(test)]
use library::SyncCommit;
use std::collections::BTreeSet;
use std::sync::OnceLock;

const RELEASE_TYPE_LOOKUP_LIMIT: usize = 500;
#[cfg(test)]
pub(in crate::controller) const SYNC_CANCELLED_ERROR: &str = "Sync cancelled.";
static RELEASE_TYPE_LOOKUPS_IN_FLIGHT: OnceLock<Mutex<HashSet<SourceId>>> = OnceLock::new();

pub(in crate::controller) fn start_album_identity_lookup(
    controller: &AppController,
    saved: &SavedSource,
) {
    let settings = load_settings_from_store(&controller.store);
    if !external_metadata::enabled(&settings) {
        return;
    }
    let source_id = saved.source.id.clone();
    if !mark_release_type_lookup_in_flight(&source_id) {
        return;
    }

    let store = controller.store.clone();
    let events = controller.events.clone();
    thread::spawn(move || {
        let result = run_album_identity_lookup(&store, &events, &source_id);
        clear_release_type_lookup_in_flight(&source_id);
        if let Err(error) = result {
            warn!(%error, source_id = %source_id.as_str(), "failed to enrich album identity");
        }
    });
}

fn mark_release_type_lookup_in_flight(source_id: &SourceId) -> bool {
    RELEASE_TYPE_LOOKUPS_IN_FLIGHT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut running| running.insert(source_id.clone()))
        .unwrap_or(false)
}

fn clear_release_type_lookup_in_flight(source_id: &SourceId) {
    if let Ok(mut running) = RELEASE_TYPE_LOOKUPS_IN_FLIGHT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        running.remove(source_id);
    }
}

fn run_album_identity_lookup(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    source_id: &SourceId,
) -> Result<(), String> {
    let candidates = store.with_store(|store| {
        store.load_album_identity_candidates(source_id, RELEASE_TYPE_LOOKUP_LIMIT)
    })?;
    if candidates.is_empty() {
        return Ok(());
    }

    info!(
        source_id = %source_id.as_str(),
        candidate_count = candidates.len(),
        "started album identity enrichment"
    );
    let mut updated = Vec::new();
    let mut misses = 0_usize;
    let mut errors = 0_usize;
    for candidate in &candidates {
        if !sync_target_is_current(store, source_id) {
            break;
        }
        match lookup_album_identity(candidate) {
            Ok(metadata) => {
                store.with_store(|store| {
                    store.update_album_identity_metadata(
                        source_id,
                        &candidate.album_id,
                        &metadata.release_types,
                        metadata.is_compilation,
                    )
                })?;
                updated.push(candidate.album_id.clone());
            }
            Err(error) if external_metadata::is_expected_release_type_lookup_miss(&error) => {
                misses += 1;
                store.with_store(|store| {
                    store.save_album_identity_miss(
                        source_id,
                        &candidate.album_id,
                        &candidate.identity_key,
                        &error,
                    )
                })?;
            }
            Err(error) => {
                errors += 1;
                warn!(
                    %error,
                    album_id = %candidate.album_id.as_str(),
                    "failed to look up album identity"
                );
            }
        }
    }
    if !updated.is_empty() {
        let _sent = events.send(ControllerEvent::LibraryDelta(Box::new(LibraryDelta {
            albums: EntityDelta {
                fields: updated.clone(),
                ..EntityDelta::default()
            },
            ..LibraryDelta::default()
        })));
    }
    info!(
        source_id = %source_id.as_str(),
        updated = updated.len(),
        misses,
        errors,
        "completed album identity enrichment"
    );
    Ok(())
}

fn lookup_album_identity(
    candidate: &AlbumIdentityCandidate,
) -> Result<external_metadata::AlbumReleaseMetadata, String> {
    external_metadata::fetch_album_release_metadata(
        candidate.musicbrainz_release_group_id.as_deref(),
        candidate.musicbrainz_album_id.as_deref(),
    )
}

pub(in crate::controller) fn refresh_queue_refs(controller: &AppController, saved: &SavedSource) {
    let Some(original_snapshot) = controller
        .queue
        .lock()
        .ok()
        .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot))
    else {
        return;
    };
    snapshot_queue_refs(controller, saved, original_snapshot);
}

pub(in crate::controller) fn snapshot_queue_refs(
    controller: &AppController,
    saved: &SavedSource,
    original_snapshot: QueueSnapshot,
) {
    if original_snapshot.source_id != saved.source.id {
        return;
    }
    let mut normalized_entries = original_snapshot.entries.clone();
    let settings = load_settings_from_store(&controller.store);
    match queue_track_refs(&controller.store, saved, &settings, &mut normalized_entries) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            warn!(%error, "failed to refresh queue image refs after sync");
            return;
        }
    }
    let mut queue = match controller.queue.lock() {
        Ok(queue) => queue,
        Err(error) => {
            warn!(%error, "failed to lock queue after local sync");
            return;
        }
    };
    let Some(current_snapshot) = queue.as_ref().map(QueueEngine::snapshot) else {
        return;
    };
    if !queue_snapshot_entries_match(&original_snapshot, &current_snapshot) {
        return;
    }
    let mut refreshed_snapshot = current_snapshot.clone();
    for (entry, normalized_entry) in refreshed_snapshot
        .entries
        .iter_mut()
        .zip(normalized_entries)
    {
        entry.image_ref = normalized_entry.image_ref;
    }
    if refreshed_snapshot.entries == current_snapshot.entries {
        return;
    }
    *queue = Some(QueueEngine::restore(refreshed_snapshot.clone()));
    drop(queue);

    defer_queue_snapshot(
        controller.store.clone(),
        controller.events.clone(),
        Arc::clone(&controller.queue_persist_generation),
        refreshed_snapshot.clone(),
    );
    sync_queue_snapshot(
        &controller.queue,
        &controller.playback_snapshot,
        &controller.auto_dj_enabled,
    );
    let _sent = controller
        .events
        .send(ControllerEvent::Queue(Box::new(Some(refreshed_snapshot))));
    let playback = controller
        .playback_snapshot
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default();
    let _sent = controller
        .events
        .send(ControllerEvent::Playback(Box::new(playback)));
}

fn queue_snapshot_entries_match(left: &QueueSnapshot, right: &QueueSnapshot) -> bool {
    left.source_id == right.source_id
        && left.entries.len() == right.entries.len()
        && left
            .entries
            .iter()
            .zip(&right.entries)
            .all(|(left, right)| {
                left.id == right.id
                    && left.track_id == right.track_id
                    && left.album_id == right.album_id
            })
}

pub(in crate::controller) fn sync_target_is_current(
    store: &StoreHandle,
    source_id: &SourceId,
) -> bool {
    store
        .with_store(|store| {
            Ok(store
                .active_source()?
                .is_some_and(|saved| saved.source.id == *source_id))
        })
        .unwrap_or(false)
}

pub(in crate::controller) fn start_home_refresh_thread(
    context: HomeRefreshContext,
    saved: SavedSource,
    target: HomeRefreshTarget,
) {
    let source_id = saved.source.id.clone();
    let Ok(active) = selected_active_source(&context.active_source, &source_id) else {
        return;
    };
    let permit = match context.home_refresh_in_flight.acquire(source_id) {
        Ok(Some(permit)) => permit,
        Ok(None) => return,
        Err(error) => {
            let _sent = context.events.send(ControllerEvent::Error(error));
            return;
        }
    };

    thread::spawn(move || {
        let result = match target {
            HomeRefreshTarget::Section(kind) => refresh_home_section_for_active(
                &context.store,
                &context.runtime,
                &context.active_source,
                &active,
                kind,
            ),
        }
        .and_then(|()| load_snapshot(&context.store).map(Box::new));
        drop(permit);
        match result {
            Ok(snapshot) => {
                let _sent = context
                    .events
                    .send(home_refresh_completed_event(target, snapshot));
            }
            Err(error) => {
                warn!(%error, "failed to refresh home sections");
            }
        }
    });
}
pub(in crate::controller) fn home_refresh_completed_event(
    target: HomeRefreshTarget,
    snapshot: Box<LibrarySnapshot>,
) -> ControllerEvent {
    ControllerEvent::HomeSectionsUpdated {
        snapshot,
        include_explore: matches!(target, HomeRefreshTarget::Section(HomeSectionKind::Explore)),
    }
}
pub(in crate::controller) fn start_explore_prefetch_thread(
    context: ExplorePrefetchContext,
    saved: SavedSource,
) {
    let source_id = saved.source.id.clone();
    let Ok(active) = selected_active_source(&context.active_source, &source_id) else {
        return;
    };
    let permit = match context
        .explore_prefetch_in_flight
        .acquire(source_id.clone())
    {
        Ok(Some(permit)) => permit,
        Ok(None) => return,
        Err(error) => {
            let _sent = context.events.send(ControllerEvent::Error(error));
            return;
        }
    };

    thread::spawn(move || {
        let result = prefetch_home_section_for_active(
            &context.store,
            &context.runtime,
            &context.active_source,
            &active,
            &saved,
            HomeSectionKind::Explore,
        );
        drop(permit);
        match result {
            Ok(section) => {
                let _sent = context
                    .events
                    .send(ControllerEvent::HomeSectionPrefetched { source_id, section });
            }
            Err(error) => {
                warn!(%error, "failed to prefetch Explore section");
            }
        }
    });
}
pub(in crate::controller) fn start_home_promotion(
    store: StoreHandle,
    events: Sender<ControllerEvent>,
    source_id: SourceId,
    section: HomeSection,
) {
    thread::spawn(move || {
        let result = promote_prefetched_home_section(&store, &source_id, &section)
            .and_then(|()| load_snapshot(&store).map(Box::new));
        match result {
            Ok(snapshot) => {
                let _sent = events.send(ControllerEvent::HomeSectionsUpdated {
                    snapshot,
                    include_explore: false,
                });
            }
            Err(error) => {
                warn!(%error, "failed to promote prefetched home section");
            }
        }
    });
}

pub(crate) fn local_sync_operation(
    source_id: SourceId,
    identity: SourceIdentity,
    load: crate::sources::LocalLoader,
    roots: crate::sources::LocalRootsLoader,
) -> crate::sources::LibrarySyncOperation {
    Arc::new(
        move |store, runtime, scope, generation, progress, cancellation| {
            if cancellation.is_cancelled() {
                return Err(library_sync::SyncError::Cancelled);
            }
            let base_cache_revision = store.with_store_sync(|store| {
                store
                    .source_cache_revision(&source_id)
                    .map_err(library_sync::SyncError::from)
            })?;
            let (source, complete_coverage) = {
                let mut report = |scan| {
                    progress(library_sync::Progress::LocalScan(local_scan_progress(scan)));
                };
                let bounded = match scope {
                    library_sync::ReconcileScope::Objects(changes) => {
                        let paths = changes.iter().map(PathBuf::from).collect::<BTreeSet<_>>();
                        let (manifest, cue_dependencies) = store.with_store_sync(|store| {
                            Ok((
                                store.load_local_manifest(&source_id)?,
                                store.load_local_cue_dependencies(&source_id)?,
                            ))
                        })?;
                        LocalSource::from_roots_with_manifest_paths(
                            roots(),
                            identity.clone(),
                            manifest,
                            &cue_dependencies,
                            &paths,
                            &mut report,
                            || cancellation.is_cancelled(),
                        )
                    }
                    library_sync::ReconcileScope::All | library_sync::ReconcileScope::None => {
                        Ok(None)
                    }
                };
                bounded
                    .and_then(|source| match source {
                        Some(source) => Ok((source, false)),
                        None => load(&mut report, cancellation).map(|source| (source, true)),
                    })
                    .map_err(|error| {
                        if cancellation.is_cancelled() {
                            library_sync::SyncError::Cancelled
                        } else {
                            library_sync::SyncError::Source(error)
                        }
                    })?
            };
            let scan = source.manifest_scan();
            info!(
                generation,
                tag_reads = scan.counters.tag_reads,
                unchanged_reused = scan.counters.unchanged_reused,
                deleted = scan.counters.deleted,
                artwork_changed = scan.counters.artwork_changed,
                filesystem_walk_elapsed_ms = scan.counters.filesystem_walk_elapsed_ms,
                manifest_compare_elapsed_ms = scan.counters.manifest_compare_elapsed_ms,
                "completed manifest-backed local scan"
            );
            let observation = runtime.block_on(library_sync::acquire_local(
                &source,
                scan,
                &|| cancellation.is_cancelled(),
                progress,
            ))?;
            store.with_store_sync(|store| {
                let mut attempt = library_sync::SyncAttempt {
                    store,
                    source_id: &source_id,
                    generation,
                    base_cache_revision,
                    cancellation,
                    progress,
                };
                library_sync::commit_local(&mut attempt, complete_coverage, observation)
            })
        },
    )
}

pub(crate) fn remote_sync_operation<T>(
    source: Arc<T>,
    changes: Option<crate::sources::LibraryChangeResolverHandle>,
) -> crate::sources::LibrarySyncOperation
where
    T: source::MusicSource
        + source::MusicFolderProvider
        + source::PlaylistReader
        + source::SourceObjectKeyProvider
        + Send
        + Sync
        + 'static,
{
    let source_id = source::MusicSource::identity(source.as_ref()).id.clone();
    Arc::new(
        move |store, runtime, scope, generation, progress, cancellation| {
            info!(generation, "started source cache sync");
            store.with_store_sync(|store| {
                let base_cache_revision = store.source_cache_revision(&source_id)?;
                let mut attempt = library_sync::SyncAttempt {
                    store,
                    source_id: &source_id,
                    generation,
                    base_cache_revision,
                    cancellation,
                    progress,
                };
                let remote = library_sync::RemoteLibrary {
                    core: source.as_ref(),
                    music_folders: source.as_ref(),
                    playlists: source.as_ref(),
                    keys: source.as_ref(),
                };
                if let (library_sync::ReconcileScope::Objects(objects), Some(changes)) =
                    (scope, changes.as_deref())
                {
                    match runtime.block_on(library_sync::sync_remote_changes(
                        &mut attempt,
                        changes,
                        objects,
                    ))? {
                        library_sync::ChangeSyncOutcome::Committed(commit) => {
                            return Ok(library_sync::SyncOutcome::Committed(commit));
                        }
                        library_sync::ChangeSyncOutcome::Ignored => {
                            return Ok(library_sync::SyncOutcome::Ignored);
                        }
                        library_sync::ChangeSyncOutcome::NeedsFull => {}
                    }
                }
                let local_access = remote_local_access_observation(
                    attempt.store,
                    attempt.source_id,
                    attempt.cancellation,
                    &mut *attempt.progress,
                )?;
                runtime
                    .block_on(library_sync::sync_remote(
                        &mut attempt,
                        remote,
                        local_access.as_ref(),
                    ))
                    .map(|commit| library_sync::SyncOutcome::Committed(Box::new(commit)))
            })
        },
    )
}

fn remote_local_access_observation(
    store: &Store,
    source_id: &SourceId,
    cancellation: &library_sync::CancellationToken,
    progress: &mut dyn FnMut(library_sync::Progress),
) -> library_sync::SyncResult<Option<library_sync::LocalAccessObservation>> {
    if cancellation.is_cancelled() {
        return Err(library_sync::SyncError::Cancelled);
    }
    let Some(access) = store.source_local_access(source_id)? else {
        return Ok(None);
    };
    let root = PathBuf::from(&access.root_path);
    let manifest = store.load_local_manifest(source_id)?;
    let identity = match LocalSource::identity_for_root(&root) {
        Ok(identity) => identity,
        Err(error) => {
            warn!(%error, %source_id, root = %root.display(), "local playback mapping is unavailable");
            return Ok(None);
        }
    };
    let source = match LocalSource::from_roots_with_manifest_scan(
        vec![root.clone()],
        identity,
        manifest,
        |scan| progress(library_sync::Progress::LocalScan(local_scan_progress(scan))),
        || cancellation.is_cancelled(),
    ) {
        Ok(source) => source,
        Err(_error) if cancellation.is_cancelled() => {
            return Err(library_sync::SyncError::Cancelled);
        }
        Err(error) => {
            warn!(%error, %source_id, root = %root.display(), "local playback mapping scan failed");
            return Ok(None);
        }
    };
    let scan = source.into_manifest_scan();
    info!(
        source_id = %source_id,
        tag_reads = scan.counters.tag_reads,
        unchanged_reused = scan.counters.unchanged_reused,
        deleted = scan.counters.deleted,
        filesystem_walk_elapsed_ms = scan.counters.filesystem_walk_elapsed_ms,
        manifest_compare_elapsed_ms = scan.counters.manifest_compare_elapsed_ms,
        "completed local playback mapping scan"
    );
    Ok(Some(
        library_sync::LocalAccessObservation::from_manifest_scan(scan),
    ))
}

fn local_scan_progress(
    progress: source_local::LocalScanProgress,
) -> library_sync::LocalScanProgress {
    let stage = match progress.stage {
        source_local::LocalScanStage::Walking => library_sync::LocalScanStage::Walking,
        source_local::LocalScanStage::ReadingTags => library_sync::LocalScanStage::ReadingTags,
        source_local::LocalScanStage::BuildingLibrary => {
            library_sync::LocalScanStage::BuildingLibrary
        }
    };
    library_sync::LocalScanProgress {
        stage,
        roots_walked: progress.roots_walked,
        directory_entries_visited: progress.directory_entries_visited,
        audio_candidates: progress.audio_candidates,
        processed_tracks: progress.processed_tracks,
        total_tracks: progress.total_tracks,
    }
}

#[cfg(test)]
fn sync_error_text(error: library_sync::SyncError) -> String {
    match error {
        library_sync::SyncError::Cancelled => SYNC_CANCELLED_ERROR.to_string(),
        error => error.to_string(),
    }
}
pub(in crate::controller) fn refresh_home_section_for_active(
    store: &StoreHandle,
    runtime: &Runtime,
    active_source: &ActiveSourceSlot,
    active: &Arc<ActiveSource>,
    kind: HomeSectionKind,
) -> Result<(), String> {
    let source_id = &active.identity.id;
    let (generation, base_cache_revision) = store.with_store(|store| {
        let state = store.sync_state(source_id)?;
        Ok((state.generation, state.cache_revision))
    })?;
    let section = (active.home_section)(store, runtime, kind)?;
    crate::sources::with_active_source_instance(active_source, active, || {
        let commit =
            cache_home_section(store, source_id, &section, generation, base_cache_revision)?;
        prune_successful_sync_image_cache(store, source_id, commit.pruned_cover_entries);
        Ok(())
    })
}
pub(in crate::controller) fn prefetch_home_section_for_active(
    store: &StoreHandle,
    runtime: &Runtime,
    active_source: &ActiveSourceSlot,
    active: &Arc<ActiveSource>,
    saved: &SavedSource,
    kind: HomeSectionKind,
) -> Result<HomeSection, String> {
    let source_id = &active.identity.id;
    let (generation, base_cache_revision) = store.with_store(|store| {
        let state = store.sync_state(source_id)?;
        Ok((state.generation, state.cache_revision))
    })?;
    let mut section = (active.home_section)(store, runtime, kind)?;
    let commit = crate::sources::with_active_source_instance(active_source, active, || {
        store.with_store(|store| {
            store.save_home_section_prefetch(source_id, generation, base_cache_revision, &section)
        })
    })?;
    prune_successful_sync_image_cache(store, source_id, commit.pruned_cover_entries);
    home_image_refs(store, saved, &mut section)?;
    Ok(section)
}
#[cfg(test)]
pub(in crate::controller) async fn sync_local_source_with_events(
    store: &StoreHandle,
    source_id: &SourceId,
    source: &LocalSource,
    events: Sender<ControllerEvent>,
) -> Result<(), String> {
    let (generation, base_cache_revision) = store.with_store(|store| {
        Ok((
            store.begin_sync(source_id)?,
            store.source_cache_revision(source_id)?,
        ))
    })?;
    let cancellation = library_sync::CancellationToken::new();
    sync_local_source_for_test(
        store,
        source_id,
        source,
        generation,
        base_cache_revision,
        &mut |progress| {
            let _sent = events.send(ControllerEvent::SourceSyncChanged(
                library_sync::SourceSyncChanged {
                    source_id: source_id.clone(),
                    epoch: 0,
                    phase: library_sync::SyncPhase::Running,
                    progress: Some(progress),
                    failure: None,
                    manual: false,
                },
            ));
        },
        &cancellation,
    )
    .await
    .map(|_| ())
}
#[cfg(test)]
pub(in crate::controller) async fn sync_local_source_outcome(
    store: &StoreHandle,
    source_id: &SourceId,
    source: &LocalSource,
) -> Result<SyncCommit, String> {
    let (generation, base_cache_revision) = store.with_store(|store| {
        Ok((
            store.begin_sync(source_id)?,
            store.source_cache_revision(source_id)?,
        ))
    })?;
    let cancellation = library_sync::CancellationToken::new();
    sync_local_source_for_test(
        store,
        source_id,
        source,
        generation,
        base_cache_revision,
        &mut |_| {},
        &cancellation,
    )
    .await
}
#[cfg(test)]
async fn sync_local_source_for_test(
    store: &StoreHandle,
    source_id: &SourceId,
    source: &LocalSource,
    generation: i64,
    base_cache_revision: i64,
    progress: &mut dyn FnMut(library_sync::Progress),
    cancellation: &library_sync::CancellationToken,
) -> Result<SyncCommit, String> {
    let observation = library_sync::acquire_local(
        source,
        source.manifest_scan(),
        &|| cancellation.is_cancelled(),
        progress,
    )
    .await
    .map_err(sync_error_text)?;
    store.with_store_session(|store| {
        let mut attempt = library_sync::SyncAttempt {
            store,
            source_id,
            generation,
            base_cache_revision,
            cancellation,
            progress,
        };
        match library_sync::commit_local(&mut attempt, true, observation)
            .map_err(sync_error_text)?
        {
            library_sync::SyncOutcome::Committed(commit) => Ok(*commit),
            library_sync::SyncOutcome::Ignored => {
                Err("complete Local sync did not commit".to_string())
            }
        }
    })
}

pub(in crate::controller) fn local_access_status_for_server(
    store: &StoreHandle,
    access: Option<&SourceLocalAccess>,
) -> Result<LocalAccessStatus, String> {
    store.with_store(|store| local_access_status_from_store(store, access))
}

pub(in crate::controller) fn local_access_status_from_store(
    store: &Store,
    access: Option<&SourceLocalAccess>,
) -> StoreResult<LocalAccessStatus> {
    let Some(access) = access else {
        return Ok(LocalAccessStatus::default());
    };
    let facts = store.local_access_status_facts(access)?;
    let sample_local_path = facts.sample_metadata_path.clone().or_else(|| {
        facts
            .sample_source_path
            .as_deref()
            .and_then(|raw| potential_local_path_text(raw, access))
    });
    Ok(LocalAccessStatus {
        sample_source_path: facts.sample_source_path,
        sample_local_path,
        direct_match_count: facts.direct_match_count,
        prefix_match_count: facts.prefix_match_count,
        metadata_match_count: facts.metadata_match_count,
        unmatched_count: facts.unmatched_count,
        total_track_count: facts.total_track_count,
    })
}
pub(in crate::controller) fn potential_local_path_text(
    raw: &str,
    access: &SourceLocalAccess,
) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    if let Some(mapped) = map_server_path_to_local(raw, access) {
        return Some(mapped.to_string_lossy().into_owned());
    }
    let direct = Path::new(raw);
    if direct.is_absolute() {
        return Some(direct.to_string_lossy().into_owned());
    }
    None
}
