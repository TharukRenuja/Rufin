use super::*;
use std::time::Duration;

const VERIFICATION_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

impl SourceCommands {
    /// Watch only the selected source, not sources being synced manually
    pub fn refresh_source_freshness(&self) {
        let selected = match self.store.with_store(|store| store.active_source()) {
            Ok(selected) => selected,
            Err(error) => {
                warn!(%error, "failed to load active source for freshness refresh");
                return;
            }
        };
        let target = match selected {
            Some(saved) => match self.active_source.read() {
                Ok(active) => active
                    .as_ref()
                    .filter(|active| active.identity.id == saved.source_id)
                    .cloned()
                    .map(|active| (saved, active)),
                Err(_) => {
                    warn!("active source lock was poisoned during freshness refresh");
                    return;
                }
            },
            None => None,
        };
        let (installed, cancelled) = match self.sync_coordinator.lock() {
            Ok(mut coordinator) => match target {
                Some((saved, active)) => {
                    let source_id = saved.source_id;
                    let (cancellation, cancelled) = coordinator.activate(source_id.clone());
                    (Some((active, source_id, cancellation)), cancelled)
                }
                None => (None, coordinator.deactivate()),
            },
            Err(_) => {
                warn!("source sync coordinator lock was poisoned during freshness refresh");
                return;
            }
        };
        if let Some(cancelled) = cancelled {
            emit_source_sync_idle(
                self,
                &cancelled.source_id,
                cancelled.epoch,
                cancelled.manual,
            );
        }
        let Some((active, source_id, cancellation)) = installed else {
            return;
        };
        start_freshness_feed(
            self.clone(),
            active,
            source_id.clone(),
            cancellation.clone(),
        );
        start_verification_deadline(self.clone(), source_id.clone(), cancellation.clone());
        self.request_automatic_source_sync(
            &source_id,
            &cancellation,
            library_sync::RequestKind::ActiveVerification,
            library_sync::ReconcileScope::All,
        );
    }

    fn request_automatic_source_sync(
        &self,
        source_id: &SourceId,
        cancellation: &library_sync::CancellationToken,
        kind: library_sync::RequestKind,
        scope: library_sync::ReconcileScope,
    ) -> bool {
        if cancellation.is_cancelled() {
            return false;
        }
        let start = if sync_target_is_current(&self.store, source_id) {
            self.sync_coordinator
                .lock()
                .ok()
                .and_then(|mut coordinator| {
                    coordinator.request_active(source_id, cancellation, kind, scope)
                })
        } else {
            None
        };
        if let Some(start) = start {
            self.dispatch_source_sync(start);
        }
        !cancellation.is_cancelled()
    }

    pub(in crate::controller) fn request_manual_source_sync(&self, source_id: SourceId) {
        let (start, joined) = match self.sync_coordinator.lock() {
            Ok(mut coordinator) => {
                let start = coordinator.request(
                    source_id.clone(),
                    library_sync::RequestKind::Manual,
                    library_sync::ReconcileScope::All,
                );
                let joined_epoch = start
                    .is_none()
                    .then(|| coordinator.running(&source_id).map(|(epoch, _)| epoch))
                    .flatten();
                (start, joined_epoch)
            }
            Err(_) => {
                warn!(%source_id, "source sync coordinator lock was poisoned for manual sync");
                return;
            }
        };
        if let Some(epoch) = joined {
            emit_source_sync_running(self, &source_id, epoch, None);
        }
        if let Some(start) = start {
            self.dispatch_source_sync(start);
        }
    }

    pub(in crate::controller) fn request_inactive_source_sync(&self, source_id: SourceId) {
        let start = self
            .sync_coordinator
            .lock()
            .ok()
            .and_then(|mut coordinator| {
                coordinator.request(
                    source_id,
                    library_sync::RequestKind::ActiveVerification,
                    library_sync::ReconcileScope::All,
                )
            });
        if let Some(start) = start {
            self.dispatch_source_sync(start);
        }
    }

    pub(in crate::controller) fn forget_source_sync(&self, source_id: &SourceId) {
        forget_source_sync_state(&self.sync_coordinator, &self.source_events.sync, source_id);
    }

    fn dispatch_source_sync(&self, start: library_sync::Start) {
        let source_id = start.source_id.clone();
        emit_source_sync_running(self, &source_id, start.epoch, None);
        let target = self
            .store
            .with_store(|store| store.stored_source(&source_id))
            .and_then(|saved| {
                saved.ok_or_else(|| "The selected source is no longer saved.".to_string())
            })
            .and_then(|saved| {
                let active =
                    selected_active_source(&self.active_source, &source_id).or_else(|_| {
                        crate::source_setup::activate_configured_source(
                            &self.store,
                            &self.secrets,
                            &saved,
                        )
                    })?;
                Ok((saved, active))
            });
        match target {
            Ok((saved, active)) => run_source_sync(self.clone(), saved, active, start),
            Err(error) => finish_unavailable_source_sync(self, start, error),
        }
    }
}

pub(in crate::controller) fn forget_source_sync_state(
    coordinator: &Arc<Mutex<library_sync::SyncCoordinator>>,
    events: &Sender<library_sync::LibrarySyncEvent>,
    source_id: &SourceId,
) {
    let cancelled = coordinator
        .lock()
        .ok()
        .and_then(|mut coordinator| coordinator.forget(source_id));
    if let Some(cancelled) = cancelled {
        let _sent = events.try_send(library_sync::LibrarySyncEvent::SyncChanged(
            library_sync::SourceSyncChanged {
                source_id: cancelled.source_id,
                epoch: cancelled.epoch,
                phase: library_sync::SyncPhase::Idle,
                progress: None,
                failure: None,
                manual: cancelled.manual,
            },
        ));
    }
}

pub(in crate::controller) fn deactivate_source_sync_state(
    coordinator: &Arc<Mutex<library_sync::SyncCoordinator>>,
    events: &Sender<library_sync::LibrarySyncEvent>,
) {
    let cancelled = coordinator
        .lock()
        .ok()
        .and_then(|mut coordinator| coordinator.deactivate());
    if let Some(cancelled) = cancelled {
        let _sent = events.try_send(library_sync::LibrarySyncEvent::SyncChanged(
            library_sync::SourceSyncChanged {
                source_id: cancelled.source_id,
                epoch: cancelled.epoch,
                phase: library_sync::SyncPhase::Idle,
                progress: None,
                failure: None,
                manual: cancelled.manual,
            },
        ));
    }
}

fn start_freshness_feed(
    controller: SourceCommands,
    active: Arc<ActiveSource>,
    source_id: SourceId,
    cancellation: library_sync::CancellationToken,
) {
    let Some(freshness) = active.freshness.clone() else {
        return;
    };
    thread::spawn(move || {
        controller.runtime.block_on(freshness.run(
            &cancellation,
            &|kind, scope| {
                controller.request_automatic_source_sync(&source_id, &cancellation, kind, scope)
            },
            &|error| {
                warn!(%error, source_id = %source_id, "source freshness input failed");
            },
        ));
    });
}

fn start_verification_deadline(
    controller: SourceCommands,
    source_id: SourceId,
    cancellation: library_sync::CancellationToken,
) {
    thread::spawn(move || {
        while controller
            .runtime
            .block_on(cancellation.wait(VERIFICATION_INTERVAL))
        {
            controller.request_automatic_source_sync(
                &source_id,
                &cancellation,
                library_sync::RequestKind::ActiveVerification,
                library_sync::ReconcileScope::All,
            );
        }
    });
}

fn run_source_sync(
    controller: SourceCommands,
    saved: StoredSource,
    active: Arc<ActiveSource>,
    start: library_sync::Start,
) {
    let source_id = start.source_id.clone();
    thread::spawn(move || {
        if start.cancellation.is_cancelled()
            || !source_sync_epoch_is_current(&controller, &source_id, start.epoch)
        {
            return;
        }
        let generation = match controller.store.with_store_sync(|store| {
            store
                .begin_sync(&source_id)
                .map_err(library_sync::SyncError::from)
        }) {
            Ok(generation) => generation,
            Err(error) => {
                finish_source_sync(
                    &controller,
                    &source_id,
                    start.epoch,
                    Some(error.to_string()),
                );
                return;
            }
        };
        let progress_controller = controller.clone();
        let progress_source_id = source_id.clone();
        let mut progress = move |progress| {
            emit_source_sync_running(
                &progress_controller,
                &progress_source_id,
                start.epoch,
                Some(progress),
            );
        };
        let result = (active.sync)(
            &controller.store,
            &controller.runtime,
            &start.scope,
            generation,
            &mut progress,
            &start.cancellation,
        );

        match result {
            Ok(library_sync::SyncOutcome::Committed(commit)) => {
                let prepared = prepare_source_sync_commit(&saved, generation, *commit);
                let PreparedSourceSyncCommit { update, effects } = prepared;
                publish_source_sync_commit(&controller, &source_id, start.epoch, update);
                start_source_sync_effects(
                    controller.store.clone(),
                    Arc::clone(&controller.playback_product),
                    controller.library_events.clone(),
                    saved,
                    effects,
                );
            }
            Ok(library_sync::SyncOutcome::Ignored) => {
                finish_source_sync_without_commit(&controller, &source_id, start.epoch, generation);
            }
            Err(_error) if start.cancellation.is_cancelled() => {
                finish_source_sync_without_commit(&controller, &source_id, start.epoch, generation);
            }
            Err(library_sync::SyncError::Store(StoreError::StaleCacheRevision { .. })) => {
                retry_source_sync(&controller, &source_id, start.epoch, generation);
            }
            Err(error) => {
                mark_source_sync_failed(&controller.store, &source_id, generation, &error);
                finish_source_sync(
                    &controller,
                    &source_id,
                    start.epoch,
                    Some(error.to_string()),
                );
            }
        }
    });
}

struct PreparedSourceSyncCommit {
    update: library_sync::LibraryCommitted,
    effects: SourceSyncEffects,
}

struct SourceSyncEffects {
    generation: i64,
    cache_revision: i64,
    work: SourceSyncWork,
}

struct SourceSyncWork {
    refresh_queue_refs: bool,
    enrich_album_identity: bool,
}

fn prepare_source_sync_commit(
    saved: &StoredSource,
    generation: i64,
    commit: SyncCommit,
) -> PreparedSourceSyncCommit {
    let SyncCommit {
        delta,
        cache_revision,
        ..
    } = commit;
    let work = source_sync_work(&delta);
    PreparedSourceSyncCommit {
        update: library_sync::LibraryCommitted {
            source_id: saved.source_id.clone(),
            revision: cache_revision,
            delta,
        },
        effects: SourceSyncEffects {
            generation,
            cache_revision,
            work,
        },
    }
}

fn source_sync_work(delta: &LibraryDelta) -> SourceSyncWork {
    let reset = delta.reset.is_some();
    let track_refs_changed = !delta.tracks.added.is_empty()
        || !delta.tracks.deleted.is_empty()
        || !delta.tracks.fields.is_empty()
        || !delta.tracks.cover_refs.is_empty();
    let album_refs_changed = !delta.albums.added.is_empty()
        || !delta.albums.deleted.is_empty()
        || !delta.albums.fields.is_empty()
        || !delta.albums.links.is_empty()
        || !delta.albums.cover_refs.is_empty();
    let album_identity_changed = !delta.albums.added.is_empty() || !delta.albums.fields.is_empty();
    SourceSyncWork {
        refresh_queue_refs: reset
            || delta.local_matches_changed
            || track_refs_changed
            || album_refs_changed,
        enrich_album_identity: reset || album_identity_changed,
    }
}

fn start_source_sync_effects(
    store: StoreHandle,
    playback_product: Arc<RwLock<Option<Arc<PlaybackProduct>>>>,
    library_events: LibraryEventSender,
    saved: StoredSource,
    effects: SourceSyncEffects,
) {
    thread::spawn(move || {
        let current_revision =
            store.with_store(|store| store.source_cache_revision(&saved.source_id));
        if current_revision != Ok(effects.cache_revision) {
            return;
        }
        let selected = sync_target_is_current(&store, &saved.source_id);
        if selected && effects.work.refresh_queue_refs {
            refresh_queue_refs(&store, &playback_product, &saved);
        }
        if selected && effects.work.enrich_album_identity {
            start_album_identity_lookup(&store, &library_events, &saved, effects.cache_revision);
        }
        info!(
            generation = effects.generation,
            cache_revision = effects.cache_revision,
            source_id = %saved.source_id,
            "completed source sync effects"
        );
    });
}

fn publish_source_sync_commit(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
    update: library_sync::LibraryCommitted,
) -> bool {
    finish_source_sync_with(controller, source_id, epoch, None, move |manual| {
        let _sent = controller
            .source_events
            .sync
            .try_send(library_sync::LibrarySyncEvent::Committed { update, manual });
    })
}

fn finish_source_sync(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
    failure: Option<String>,
) -> bool {
    finish_source_sync_with(controller, source_id, epoch, failure, |_| {})
}

fn finish_source_sync_with(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
    failure: Option<String>,
    on_finished: impl FnOnce(bool),
) -> bool {
    let Ok(mut coordinator) = controller.sync_coordinator.lock() else {
        return false;
    };
    let finish = coordinator.finish(source_id, epoch);
    let (manual, follow_up) = match finish {
        library_sync::Finish::Ignored => return false,
        library_sync::Finish::Finished { manual, follow_up } => (manual, follow_up),
    };
    on_finished(manual);
    if let Some(failure) = failure {
        let _sent =
            controller
                .source_events
                .sync
                .try_send(library_sync::LibrarySyncEvent::SyncChanged(
                    library_sync::SourceSyncChanged {
                        source_id: source_id.clone(),
                        epoch,
                        phase: library_sync::SyncPhase::Failed,
                        progress: None,
                        failure: Some(failure),
                        manual,
                    },
                ));
    } else if follow_up.is_none() {
        emit_source_sync_idle(controller, source_id, epoch, manual);
    }
    drop(coordinator);
    if let Some(start) = follow_up {
        controller.dispatch_source_sync(start);
    }
    true
}

fn finish_unavailable_source_sync(
    controller: &SourceCommands,
    start: library_sync::Start,
    error: String,
) {
    finish_source_sync(controller, &start.source_id, start.epoch, Some(error));
}

fn source_sync_epoch_is_current(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
) -> bool {
    controller
        .sync_coordinator
        .lock()
        .ok()
        .and_then(|coordinator| coordinator.running_manual(source_id, epoch))
        .is_some()
}

fn emit_source_sync_running(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
    progress: Option<library_sync::Progress>,
) {
    let Ok(coordinator) = controller.sync_coordinator.lock() else {
        return;
    };
    let Some(manual) = coordinator.running_manual(source_id, epoch) else {
        return;
    };
    let _sent =
        controller
            .source_events
            .sync
            .try_send(library_sync::LibrarySyncEvent::SyncChanged(
                library_sync::SourceSyncChanged {
                    source_id: source_id.clone(),
                    epoch,
                    phase: library_sync::SyncPhase::Running,
                    progress,
                    failure: None,
                    manual,
                },
            ));
}

fn emit_source_sync_idle(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
    manual: bool,
) {
    let _sent =
        controller
            .source_events
            .sync
            .try_send(library_sync::LibrarySyncEvent::SyncChanged(
                library_sync::SourceSyncChanged {
                    source_id: source_id.clone(),
                    epoch,
                    phase: library_sync::SyncPhase::Idle,
                    progress: None,
                    failure: None,
                    manual,
                },
            ));
}

fn finish_source_sync_without_commit(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
    generation: i64,
) {
    let _result = controller
        .store
        .with_store(|store| store.finish_sync_without_commit(source_id, generation));
    finish_source_sync(controller, source_id, epoch, None);
}

fn retry_source_sync(
    controller: &SourceCommands,
    source_id: &SourceId,
    epoch: u64,
    generation: i64,
) {
    let _result = controller
        .store
        .with_store(|store| store.finish_sync_without_commit(source_id, generation));
    let Ok(mut coordinator) = controller.sync_coordinator.lock() else {
        return;
    };
    let finish = coordinator.retry(source_id, epoch);
    let follow_up = match finish {
        library_sync::Finish::Ignored => return,
        library_sync::Finish::Finished { follow_up, .. } => follow_up,
    };
    drop(coordinator);
    if let Some(start) = follow_up {
        controller.dispatch_source_sync(start);
    }
}

fn mark_source_sync_failed(
    store: &StoreHandle,
    source_id: &SourceId,
    generation: i64,
    error: &library_sync::SyncError,
) {
    let _result = store.with_store(|store| {
        store
            .fail_sync(source_id, generation, &error.to_string())
            .map(|_| ())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::root::controller_bootstrap::bootstrap_memory_for_test;

    #[test]
    fn cache_clear_rebuild_restores_automatic_freshness() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = saved_source();
        store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source_id)
            })
            .expect("select source");
        let (owners, events) = owners_from_store_for_test(store);
        let calls = Arc::new(AtomicU64::new(0));
        let sync_calls = Arc::clone(&calls);
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        {
            let mut active = owners.source.active_source.write().expect("active source");
            let active = Arc::get_mut(active.as_mut().expect("selected source"))
                .expect("exclusive selected source");
            active.freshness = None;
            active.sync = Arc::new(move |_, _, _, _, _, _| {
                sync_calls.fetch_add(1, Ordering::SeqCst);
                entered_tx.send(()).expect("report sync start");
                release_rx
                    .lock()
                    .expect("release sync")
                    .recv_timeout(Duration::from_secs(1))
                    .expect("sync release");
                Ok(library_sync::SyncOutcome::Ignored)
            });
        }

        owners.source.clear_source_cache(saved.source_id.clone());

        let _snapshot = wait_for_source_presentation(&events);
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("cache rebuild start");
        let lease = (0..100)
            .find_map(|_| {
                let lease = owners
                    .source
                    .sync_coordinator
                    .lock()
                    .expect("sync coordinator")
                    .active_cancellation(&saved.source_id);
                if lease.is_none() {
                    thread::sleep(Duration::from_millis(10));
                }
                lease
            })
            .expect("active freshness lease");
        assert!(owners.source.request_automatic_source_sync(
            &saved.source_id,
            &lease,
            library_sync::RequestKind::ActiveVerification,
            library_sync::ReconcileScope::All,
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        release_tx.send(()).expect("finish cache rebuild");
        loop {
            match wait_for_typed_event(
                &events.library_sync,
                Duration::from_secs(1),
                "cache rebuild completion",
            ) {
                library_sync::LibrarySyncEvent::SyncChanged(change)
                    if change.source_id == saved.source_id
                        && change.phase == library_sync::SyncPhase::Idle =>
                {
                    break;
                }
                library_sync::LibrarySyncEvent::SyncChanged(change)
                    if change.source_id == saved.source_id
                        && change.phase == library_sync::SyncPhase::Running =>
                {
                    assert!(!change.manual, "cache rebuild should stay silent");
                }
                _ => {}
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ignored_sync_finishes_without_a_library_commit() {
        let (owners, events, ..) = bootstrap_memory_for_test();
        let saved = saved_source();
        owners
            .source
            .store
            .with_store(|store| store.save_source(&saved))
            .expect("save source");
        let revision = owners
            .source
            .store
            .with_store(|store| store.source_cache_revision(&saved.source_id))
            .expect("cache revision");
        let start = owners
            .source
            .sync_coordinator
            .lock()
            .expect("sync coordinator")
            .request(
                saved.source_id.clone(),
                library_sync::RequestKind::Freshness,
                library_sync::ReconcileScope::objects(sources::SourceObjectChanges::new([
                    "movie-one".to_string(),
                ])),
            )
            .expect("start sync");
        let generation = owners
            .source
            .store
            .with_store(|store| store.begin_sync(&saved.source_id))
            .expect("begin sync");

        finish_source_sync_without_commit(
            &owners.source,
            &saved.source_id,
            start.epoch,
            generation,
        );

        let state = owners
            .source
            .store
            .with_store(|store| store.sync_state(&saved.source_id))
            .expect("sync state");
        assert_eq!(state.status, "idle");
        assert_eq!(state.cache_revision, revision);
        assert!(
            owners
                .source
                .sync_coordinator
                .lock()
                .expect("sync coordinator")
                .running(&saved.source_id)
                .is_none()
        );
        let emitted = drain_typed_events(&events.library_sync);
        assert!(
            emitted
                .iter()
                .all(|event| !matches!(event, library_sync::LibrarySyncEvent::Committed { .. }))
        );
        assert!(emitted.iter().any(|event| matches!(
            event,
            library_sync::LibrarySyncEvent::SyncChanged(change)
                if change.source_id == saved.source_id
                    && change.phase == library_sync::SyncPhase::Idle
        )));
    }

    #[test]
    fn committed_event_uses_finished_manual_interest() {
        let (owners, events, ..) = bootstrap_memory_for_test();
        let saved = saved_source();
        let receive_commit = || loop {
            if let library_sync::LibrarySyncEvent::Committed { update, manual } =
                wait_for_typed_event(
                    &events.library_sync,
                    Duration::from_secs(1),
                    "library commit",
                )
            {
                break (update, manual);
            }
        };

        let automatic = owners
            .source
            .sync_coordinator
            .lock()
            .expect("sync coordinator")
            .request(
                saved.source_id.clone(),
                library_sync::RequestKind::Freshness,
                library_sync::ReconcileScope::All,
            )
            .expect("automatic sync");
        assert!(publish_source_sync_commit(
            &owners.source,
            &saved.source_id,
            automatic.epoch,
            library_sync::LibraryCommitted {
                source_id: saved.source_id.clone(),
                revision: 1,
                delta: LibraryDelta::default(),
            },
        ));
        let (_, automatic_manual) = receive_commit();
        assert!(!automatic_manual);

        let joined = {
            let mut coordinator = owners
                .source
                .sync_coordinator
                .lock()
                .expect("sync coordinator");
            let joined = coordinator
                .request(
                    saved.source_id.clone(),
                    library_sync::RequestKind::ActiveVerification,
                    library_sync::ReconcileScope::All,
                )
                .expect("joined sync");
            assert!(
                coordinator
                    .request(
                        saved.source_id.clone(),
                        library_sync::RequestKind::Manual,
                        library_sync::ReconcileScope::All,
                    )
                    .is_none()
            );
            joined
        };
        assert!(publish_source_sync_commit(
            &owners.source,
            &saved.source_id,
            joined.epoch,
            library_sync::LibraryCommitted {
                source_id: saved.source_id.clone(),
                revision: 2,
                delta: LibraryDelta::default(),
            },
        ));
        let (_, joined_manual) = receive_commit();
        assert!(joined_manual);
    }

    #[test]
    fn inactive_selected_source_deactivates_old_freshness_and_work() {
        let (owners, events, ..) = bootstrap_memory_for_test();
        let saved = saved_source();
        owners
            .source
            .store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source_id)
            })
            .expect("select source");
        let (lease, start) = {
            let mut coordinator = owners
                .source
                .sync_coordinator
                .lock()
                .expect("sync coordinator");
            let (lease, cancelled) = coordinator.activate(saved.source_id.clone());
            assert!(cancelled.is_none());
            let start = coordinator
                .request_active(
                    &saved.source_id,
                    &lease,
                    library_sync::RequestKind::Freshness,
                    library_sync::ReconcileScope::All,
                )
                .expect("automatic work");
            (lease, start)
        };

        owners.source.refresh_source_freshness();

        assert!(lease.is_cancelled());
        assert!(start.cancellation.is_cancelled());
        assert!(
            owners
                .source
                .sync_coordinator
                .lock()
                .expect("sync coordinator")
                .running(&saved.source_id)
                .is_none()
        );
        assert!(
            drain_typed_events(&events.library_sync)
                .into_iter()
                .any(|event| matches!(
                    event,
                    library_sync::LibrarySyncEvent::SyncChanged(change)
                        if change.source_id == saved.source_id
                            && change.epoch == start.epoch
                            && change.phase == library_sync::SyncPhase::Idle
                ))
        );
    }
}
