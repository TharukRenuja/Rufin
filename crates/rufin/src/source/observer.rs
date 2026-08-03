use std::collections::VecDeque;

use tokio::task::JoinHandle;

use super::*;

pub(super) struct ActiveObserver {
    pub(super) qualifier: SourceQualifier,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) observed: Arc<Mutex<ObservedChangeState>>,
    pub(super) handle: JoinHandle<()>,
}

pub(super) enum SelectedObservedChange {
    Source(SourceLibraryChange),
    Local(LocalFilesystemChange),
    #[cfg(test)]
    Probe(tests::ObservedChangeProbe),
}

pub(super) struct ObservedChangeState {
    pub(super) active: Option<u64>,
    pending: VecDeque<SelectedObservedChange>,
}

impl ObservedChangeState {
    pub(super) fn new() -> Self {
        Self {
            active: None,
            pending: VecDeque::new(),
        }
    }

    pub(super) fn submit(
        &mut self,
        token: u64,
        change: SelectedObservedChange,
    ) -> Option<SelectedObservedChange> {
        if self.active.is_none() {
            self.active = Some(token);
            return Some(change);
        }
        match (self.pending.back_mut(), change) {
            (
                Some(SelectedObservedChange::Source(current)),
                SelectedObservedChange::Source(incoming),
            ) => current.merge(incoming),
            (
                Some(SelectedObservedChange::Local(current)),
                SelectedObservedChange::Local(incoming),
            ) => current.merge(incoming),
            (_, change) => self.pending.push_back(change),
        }
        None
    }

    pub(super) fn next(&mut self, token: u64) -> Option<SelectedObservedChange> {
        if self.active != Some(token) {
            return None;
        }
        if let Some(change) = self.pending.pop_front() {
            self.active = None;
            Some(change)
        } else {
            self.active = None;
            None
        }
    }

    pub(super) fn activate(&mut self, token: u64) {
        self.active = Some(token);
    }

    fn cancel(&mut self, token: u64) {
        if self.active != Some(token) {
            return;
        }
        self.stop();
    }

    pub(super) fn stop(&mut self) {
        self.active = None;
        self.pending.clear();
    }
}

pub(super) struct ObservedChangeRun {
    state: Arc<Mutex<ObservedChangeState>>,
    token: u64,
    clear_on_drop: bool,
}

impl ObservedChangeRun {
    pub(super) fn new(state: Arc<Mutex<ObservedChangeState>>, token: u64) -> Self {
        Self {
            state,
            token,
            clear_on_drop: true,
        }
    }

    pub(super) fn token(&self) -> u64 {
        self.token
    }

    pub(super) fn finish(mut self) {
        self.clear_on_drop = false;
    }
}

impl Drop for ObservedChangeRun {
    fn drop(&mut self) {
        if self.clear_on_drop {
            self.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .cancel(self.token);
        }
    }
}

impl Drop for ActiveObserver {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.observed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stop();
        self.handle.abort();
    }
}

pub(super) struct RefreshRequest {
    pub(super) qualifier: SourceQualifier,
    pub(super) visible: AtomicBool,
    pub(super) started: AtomicBool,
    pub(super) announced: AtomicBool,
    pub(super) cancelled: Arc<AtomicBool>,
}

pub(super) struct FreshnessAdmission {
    next_check: tokio::time::Instant,
    pub(super) pending: Option<u64>,
}

impl FreshnessAdmission {
    pub(super) fn new(now: tokio::time::Instant) -> Self {
        Self {
            next_check: now,
            pending: None,
        }
    }

    pub(super) fn defer(&mut self, now: tokio::time::Instant) {
        self.next_check = now + SOURCE_CHECK_INTERVAL;
    }

    pub(super) fn admit(&mut self, token: u64, catch_up: bool, now: tokio::time::Instant) -> bool {
        if !catch_up && now < self.next_check {
            return false;
        }
        self.next_check = now + SOURCE_CHECK_INTERVAL;
        if self.pending.is_some() {
            return false;
        }
        self.pending = Some(token);
        true
    }

    pub(super) fn finish(&mut self, token: u64) {
        if self.pending == Some(token) {
            self.pending = None;
        }
    }

    pub(super) fn cancel(&mut self) {
        self.pending = None;
    }
}

struct PendingFreshnessCheck {
    shared: Weak<Shared>,
    token: u64,
}

impl Drop for PendingFreshnessCheck {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.finish_freshness_check(self.token);
        }
    }
}

impl SourceOwner {
    pub(super) fn request_refresh(&self, source_id: SourceId, visible: bool) {
        self.request_refresh_while_active(source_id, visible, None);
    }

    pub(super) fn request_refresh_while_active(
        &self,
        source_id: SourceId,
        visible: bool,
        parent_cancelled: Option<&AtomicBool>,
    ) {
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
            if parent_cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Acquire)) {
                return;
            }
            if let Some(refresh) = state
                .refresh
                .as_ref()
                .filter(|refresh| refresh.qualifier == qualifier)
            {
                if visible {
                    refresh.visible.store(true, Ordering::Release);
                    if refresh.started.load(Ordering::Acquire)
                        && !refresh.announced.swap(true, Ordering::AcqRel)
                    {
                        let _ = self.shared.outputs.events.try_send(SourceEvent::Operation(
                            SourceOperation::Refreshing {
                                source_id: refresh.qualifier.source_id.clone(),
                                progress: initial_progress(),
                            },
                        ));
                    }
                }
                None
            } else {
                let request = Arc::new(RefreshRequest {
                    qualifier,
                    visible: AtomicBool::new(visible),
                    started: AtomicBool::new(false),
                    announced: AtomicBool::new(false),
                    cancelled: Arc::new(AtomicBool::new(false)),
                });
                let registration = self
                    .shared
                    .register_interruptible(Arc::clone(&request.cancelled));
                state.refresh = Some(Arc::clone(&request));
                Some((request, registration))
            }
        };
        let Some((request, registration)) = request else {
            return;
        };
        let request_for_work = Arc::clone(&request);
        self.spawn_registered(
            Some(registration),
            Arc::clone(&request.cancelled),
            move |mut operations, cancelled| async move {
                operations.refresh(request_for_work, cancelled).await;
            },
        );
    }

    pub(super) fn request_freshness_check(&self, catch_up: bool) {
        let Some(session) = self.shared.selected_session() else {
            return;
        };
        let Some(selected) = session
            .resolve()
            .filter(|selected| selected.source.is_some())
        else {
            return;
        };
        let qualifier = selected.qualifier();
        let cancelled = Arc::new(AtomicBool::new(false));
        let registration = self.shared.reserve_interruptible();
        let registration = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state
                .freshness
                .admit(registration.token, catch_up, tokio::time::Instant::now())
            {
                return;
            }
            self.shared
                .register_reserved_interruptible(registration, Arc::clone(&cancelled));
            registration
        };
        let pending = PendingFreshnessCheck {
            shared: Arc::downgrade(&self.shared),
            token: registration.token,
        };
        self.spawn_registered(
            Some(registration),
            cancelled,
            move |mut operations, cancelled| async move {
                let _pending = pending;
                let Some(selected) = session
                    .resolve()
                    .filter(|selected| selected.qualifier() == qualifier)
                else {
                    return;
                };
                operations.check_freshness(selected, cancelled).await;
            },
        );
    }
    pub(super) fn queue_observed_change(
        &self,
        state: &Arc<Mutex<ObservedChangeState>>,
        session: &Arc<ActiveSource>,
        observer_cancelled: &Arc<AtomicBool>,
        change: SelectedObservedChange,
    ) -> bool {
        if resolve_observer_session(observer_cancelled, session).is_none() {
            return false;
        }
        let mut observed = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if observer_cancelled.load(Ordering::Acquire) {
            return false;
        }
        let registration = self.shared.reserve_interruptible();
        let first = observed.submit(registration.token, change);
        if let Some(first) = first {
            let cancelled = Arc::new(AtomicBool::new(false));
            self.shared
                .register_reserved_interruptible(registration, Arc::clone(&cancelled));
            self.start_observed_changes(
                Arc::clone(state),
                Arc::clone(session),
                Arc::clone(observer_cancelled),
                cancelled,
                registration,
                first,
            );
        }
        true
    }

    pub(super) fn start_observed_changes(
        &self,
        state: Arc<Mutex<ObservedChangeState>>,
        session: Arc<ActiveSource>,
        observer_cancelled: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        registration: InterruptibleRegistration,
        change: SelectedObservedChange,
    ) {
        let next_state = Arc::clone(&state);
        let run = ObservedChangeRun::new(state, registration.token);
        self.spawn_registered(
            Some(registration),
            cancelled,
            move |mut operations, cancelled| async move {
                let Some(selected) = resolve_observer_session(&observer_cancelled, &session) else {
                    return;
                };
                match change {
                    SelectedObservedChange::Source(change) => {
                        operations
                            .accept_observed_change(selected, change, Arc::clone(&cancelled))
                            .await;
                    }
                    SelectedObservedChange::Local(change) => {
                        operations
                            .accept_local_change(selected, change, Arc::clone(&cancelled))
                            .await;
                    }
                    #[cfg(test)]
                    SelectedObservedChange::Probe(probe) => probe.accept().await,
                }
                if cancelled.load(Ordering::Acquire)
                    || resolve_observer_session(&observer_cancelled, &session).is_none()
                {
                    return;
                }
                let mut observed = next_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let next = observed.next(run.token());
                if let Some(next) = next {
                    let registration = operations.shared.reserve_interruptible();
                    let next_cancelled = Arc::new(AtomicBool::new(false));
                    operations
                        .shared
                        .register_reserved_interruptible(registration, Arc::clone(&next_cancelled));
                    observed.activate(registration.token);
                    operations.start_observed_changes(
                        Arc::clone(&next_state),
                        session,
                        observer_cancelled,
                        next_cancelled,
                        registration,
                        next,
                    );
                }
                drop(observed);
                run.finish();
            },
        );
    }

    pub(super) fn stop_observer(&mut self) {
        let observer = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .observer
            .take();
        drop(observer);
    }

    pub(super) async fn refresh(
        &mut self,
        request: Arc<RefreshRequest>,
        cancelled: Arc<AtomicBool>,
    ) {
        let Some(selected) = self
            .shared
            .selected()
            .filter(|selected| selected.qualifier() == request.qualifier)
        else {
            self.shared.finish_refresh(&request);
            return;
        };
        let source_id = selected.source_id().clone();
        request.started.store(true, Ordering::Release);
        if request.visible.load(Ordering::Acquire)
            && !request.announced.swap(true, Ordering::AcqRel)
        {
            self.shared
                .send_event(SourceEvent::Operation(SourceOperation::Refreshing {
                    source_id: source_id.clone(),
                    progress: initial_progress(),
                }))
                .await;
        }
        let visible = Arc::clone(&request);
        let progress = self.progress(Arc::clone(&cancelled), move |progress| {
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
        let result = match prepared {
            Ok(prepared) => {
                let acceptance_owner = Arc::clone(&self.shared);
                let _acceptance = acceptance_owner.acceptance_lane.lock().await;
                if !self.shared.protect_interruptible_commit(&cancelled) {
                    self.shared.finish_refresh(&request);
                    return;
                }
                self.commit_refresh(Arc::clone(&selected), prepared).await
            }
            Err(error) => Err(error),
        };
        if cancelled.load(Ordering::Acquire) {
            self.shared.finish_refresh(&request);
            return;
        }
        let visible = self.shared.finish_refresh(&request).unwrap_or(false);
        match result {
            Ok(()) if visible => {
                self.shared
                    .send_event(SourceEvent::Operation(SourceOperation::Idle))
                    .await;
            }
            Ok(()) => {}
            Err(error) => self.refresh_failed(&selected, visible, error).await,
        }
    }

    pub(super) async fn refresh_failed(
        &self,
        selected: &SelectedSourceState,
        visible: bool,
        error: String,
    ) {
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

    pub(super) async fn accept_observed_change(
        &mut self,
        selected: Arc<SelectedSourceState>,
        change: SourceLibraryChange,
        cancelled: Arc<AtomicBool>,
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
                let acceptance_owner = Arc::clone(&self.shared);
                let _acceptance = acceptance_owner.acceptance_lane.lock().await;
                if !self.shared.protect_interruptible_commit(&cancelled) {
                    return;
                }
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
            Ok(SourceLibraryChangeRead::Full) => {
                SourceOwner {
                    shared: Arc::clone(&self.shared),
                }
                .request_refresh_while_active(
                    selected.source_id().clone(),
                    false,
                    Some(&cancelled),
                );
            }
            Ok(SourceLibraryChangeRead::Ignored) => {}
            Err(error) => warn!(%error, "background selected source update failed"),
        }
    }

    pub(super) async fn check_freshness(
        &mut self,
        selected: Arc<SelectedSourceState>,
        cancelled: Arc<AtomicBool>,
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
            Ok(SourceFreshness::Changed(_)) => {
                SourceOwner {
                    shared: Arc::clone(&self.shared),
                }
                .request_refresh_while_active(
                    selected.source_id().clone(),
                    false,
                    Some(&cancelled),
                );
            }
            Ok(
                SourceFreshness::Unavailable | SourceFreshness::Unchanged | SourceFreshness::Busy,
            ) => {}
            Err(error) => warn!(%error, "could not check selected source freshness"),
        }
    }
}
