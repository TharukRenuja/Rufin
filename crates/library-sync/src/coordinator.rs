use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use library::{LibraryDelta, SourceId};

use crate::{Progress, ReconcileScope};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncPhase {
    Running,
    Idle,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSyncChanged {
    pub source_id: SourceId,
    pub epoch: u64,
    pub phase: SyncPhase,
    pub progress: Option<Progress>,
    pub failure: Option<String>,
    pub manual: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryCommitted {
    pub source_id: SourceId,
    pub revision: i64,
    pub delta: LibraryDelta,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RequestKind {
    Manual,
    ActiveVerification,
    Freshness,
}

impl RequestKind {
    fn is_automatic(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn can_commit(&self) -> bool {
        !self.is_cancelled()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub async fn wait(&self, duration: Duration) -> bool {
        let mut remaining = duration;
        while !remaining.is_zero() {
            if self.is_cancelled() {
                return false;
            }
            let step = remaining.min(Duration::from_millis(500));
            tokio::time::sleep(step).await;
            remaining = remaining.saturating_sub(step);
        }
        !self.is_cancelled()
    }

    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
}

#[derive(Clone, Debug)]
pub struct Start {
    pub source_id: SourceId,
    pub epoch: u64,
    pub scope: ReconcileScope,
    pub cancellation: CancellationToken,
}

#[derive(Debug)]
pub enum Finish {
    Ignored,
    Finished {
        manual: bool,
        follow_up: Option<Start>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelledRun {
    pub source_id: SourceId,
    pub epoch: u64,
    pub manual: bool,
}

#[derive(Clone, Debug, Default)]
struct Work {
    interests: BTreeSet<RequestKind>,
    scope: ReconcileScope,
}

impl Work {
    fn new(interest: RequestKind, scope: ReconcileScope) -> Self {
        Self {
            interests: BTreeSet::from([interest]),
            scope,
        }
    }

    fn join(&mut self, interest: RequestKind, scope: ReconcileScope) {
        self.interests.insert(interest);
        self.scope.merge(scope);
    }

    fn add_interest(&mut self, interest: RequestKind) {
        self.interests.insert(interest);
    }

    fn is_empty(&self) -> bool {
        self.interests.is_empty()
    }

    fn is_manual(&self) -> bool {
        self.interests.contains(&RequestKind::Manual)
    }

    fn remove_automatic_interests(&mut self) {
        self.interests.retain(|interest| !interest.is_automatic());
        if self.interests.is_empty() {
            self.scope = ReconcileScope::None;
        }
    }
}

#[derive(Clone, Debug)]
struct Running {
    epoch: u64,
    work: Work,
    cancellation: CancellationToken,
}

#[derive(Default)]
struct SourceState {
    running: Option<Running>,
    pending: Work,
}

/// Each source has one queue; selecting another source stops automatic work but
/// keeps manual work
#[derive(Default)]
pub struct SyncCoordinator {
    next_epoch: u64,
    active: Option<(SourceId, CancellationToken)>,
    sources: BTreeMap<SourceId, SourceState>,
}

impl SyncCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(
        &mut self,
        source_id: SourceId,
        interest: RequestKind,
        scope: ReconcileScope,
    ) -> Option<Start> {
        if scope.is_empty() {
            return None;
        }
        let state = self.sources.entry(source_id.clone()).or_default();
        let Some(running) = state.running.as_mut() else {
            return Some(self.start(source_id, Work::new(interest, scope)));
        };

        if interest == RequestKind::Freshness {
            state.pending.join(interest, scope);
        } else if !running.work.scope.is_all() {
            running.work.add_interest(interest);
            state.pending.join(interest, scope);
        } else {
            running.work.join(interest, scope);
        }
        None
    }

    pub fn activate(&mut self, source_id: SourceId) -> (CancellationToken, Option<CancelledRun>) {
        let cancelled = self.deactivate();
        let cancellation = CancellationToken::new();
        self.active = Some((source_id, cancellation.clone()));
        (cancellation, cancelled)
    }

    pub fn active_cancellation(&self, source_id: &SourceId) -> Option<CancellationToken> {
        self.active
            .as_ref()
            .filter(|(active_source_id, _)| active_source_id == source_id)
            .map(|(_, cancellation)| cancellation.clone())
    }

    pub fn request_active(
        &mut self,
        source_id: &SourceId,
        cancellation: &CancellationToken,
        interest: RequestKind,
        scope: ReconcileScope,
    ) -> Option<Start> {
        let current = self
            .active
            .as_ref()
            .is_some_and(|(active_source_id, active)| {
                active_source_id == source_id && active.same(cancellation) && !active.is_cancelled()
            });
        if !current || !interest.is_automatic() {
            return None;
        }
        self.request(source_id.clone(), interest, scope)
    }

    pub fn finish(&mut self, source_id: &SourceId, epoch: u64) -> Finish {
        let (manual, next) = {
            let Some(state) = self.sources.get_mut(source_id) else {
                return Finish::Ignored;
            };
            let Some(running) = state.running.take() else {
                return Finish::Ignored;
            };
            if running.epoch != epoch {
                state.running = Some(running);
                return Finish::Ignored;
            }

            let manual = running.work.is_manual();
            let pending = std::mem::take(&mut state.pending);
            let next = if pending.is_empty() {
                None
            } else {
                Some(pending)
            };
            (manual, next)
        };
        let follow_up = next.map(|work| self.start(source_id.clone(), work));
        if follow_up.is_none() {
            self.sources.remove(source_id);
        }
        Finish::Finished { manual, follow_up }
    }

    pub fn retry(&mut self, source_id: &SourceId, epoch: u64) -> Finish {
        let work = {
            let Some(state) = self.sources.get_mut(source_id) else {
                return Finish::Ignored;
            };
            let Some(mut running) = state.running.take() else {
                return Finish::Ignored;
            };
            if running.epoch != epoch {
                state.running = Some(running);
                return Finish::Ignored;
            }

            let pending = std::mem::take(&mut state.pending);
            running.work.interests.extend(pending.interests);
            running.work.scope.merge(pending.scope);
            running.work
        };
        let manual = work.is_manual();
        let follow_up = Some(self.start(source_id.clone(), work));
        Finish::Finished { manual, follow_up }
    }

    pub fn running(&self, source_id: &SourceId) -> Option<(u64, bool)> {
        let running = self.sources.get(source_id)?.running.as_ref()?;
        Some((running.epoch, running.work.is_manual()))
    }

    pub fn running_manual(&self, source_id: &SourceId, epoch: u64) -> Option<bool> {
        let (running_epoch, manual) = self.running(source_id)?;
        (running_epoch == epoch).then_some(manual)
    }

    pub fn deactivate(&mut self) -> Option<CancelledRun> {
        let (source_id, cancellation) = self.active.take()?;
        cancellation.cancel();
        self.remove_automatic_work(&source_id)
    }

    fn remove_automatic_work(&mut self, source_id: &SourceId) -> Option<CancelledRun> {
        let state = self.sources.get_mut(source_id)?;

        state.pending.remove_automatic_interests();

        let cancel_running = state.running.as_mut().is_some_and(|running| {
            running.work.remove_automatic_interests();
            running.work.is_empty()
        });
        if !cancel_running {
            return None;
        }

        let cancelled = state.running.take().map(|running| {
            running.cancellation.cancel();
            CancelledRun {
                source_id: source_id.clone(),
                epoch: running.epoch,
                manual: running.work.is_manual(),
            }
        });
        self.sources.remove(source_id);
        cancelled
    }

    pub fn forget(&mut self, source_id: &SourceId) -> Option<CancelledRun> {
        if self
            .active
            .as_ref()
            .is_some_and(|(active_source_id, _)| active_source_id == source_id)
            && let Some((_, cancellation)) = self.active.take()
        {
            cancellation.cancel();
        }
        if let Some(mut state) = self.sources.remove(source_id)
            && let Some(running) = state.running.take()
        {
            let manual = running.work.is_manual();
            running.cancellation.cancel();
            return Some(CancelledRun {
                source_id: source_id.clone(),
                epoch: running.epoch,
                manual,
            });
        }
        None
    }

    fn start(&mut self, source_id: SourceId, work: Work) -> Start {
        let epoch = self.next_epoch();
        let cancellation = CancellationToken::new();
        let start = Start {
            source_id: source_id.clone(),
            epoch,
            scope: work.scope.clone(),
            cancellation: cancellation.clone(),
        };
        self.sources.entry(source_id).or_default().running = Some(Running {
            epoch: start.epoch,
            work,
            cancellation,
        });
        start
    }

    fn next_epoch(&mut self) -> u64 {
        self.next_epoch += 1;
        self.next_epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(number: u32) -> SourceId {
        SourceId::new(format!("source-{number}"))
    }

    fn objects(ids: &[&str]) -> ReconcileScope {
        ReconcileScope::objects(sources::SourceObjectChanges::new(
            ids.iter().map(|id| (*id).to_string()),
        ))
    }

    #[test]
    fn scope_union_is_order_independent_and_all_absorbs_objects() {
        let mut left = objects(&["track-a", "track-b"]);
        left.merge(objects(&["track-b", "track-c"]));
        let mut right = objects(&["track-c", "track-b"]);
        right.merge(objects(&["track-a", "track-b"]));
        assert_eq!(left, right);

        left.merge(ReconcileScope::All);
        left.merge(objects(&["track-d"]));
        assert_eq!(left, ReconcileScope::All);
    }

    #[test]
    fn late_object_inputs_form_one_finite_follow_up() {
        let source_id = source(0);
        let mut coordinator = SyncCoordinator::new();
        let running = coordinator
            .request(
                source_id.clone(),
                RequestKind::Freshness,
                objects(&["track-a"]),
            )
            .expect("first input starts");
        coordinator.request(
            source_id.clone(),
            RequestKind::Freshness,
            objects(&["track-b", "track-a"]),
        );
        coordinator.request(
            source_id.clone(),
            RequestKind::Freshness,
            objects(&["track-c"]),
        );

        let Finish::Finished {
            follow_up: Some(follow_up),
            ..
        } = coordinator.finish(&source_id, running.epoch)
        else {
            panic!("late inputs should form one follow-up");
        };
        assert_eq!(follow_up.scope, objects(&["track-a", "track-b", "track-c"]));
        assert!(matches!(
            coordinator.finish(&source_id, follow_up.epoch),
            Finish::Finished {
                manual: false,
                follow_up: None
            }
        ));
    }

    #[test]
    fn manual_full_request_survives_deactivation_from_bounded_work() {
        let source_id = source(2);
        let mut coordinator = SyncCoordinator::new();
        let (lease, cancelled) = coordinator.activate(source_id.clone());
        assert!(cancelled.is_none());
        let bounded = coordinator
            .request_active(
                &source_id,
                &lease,
                RequestKind::Freshness,
                objects(&["track-a"]),
            )
            .expect("bounded input starts");
        coordinator.request(source_id.clone(), RequestKind::Manual, ReconcileScope::All);

        assert_eq!(coordinator.deactivate(), None);
        assert!(lease.is_cancelled());
        assert!(!bounded.cancellation.is_cancelled());
        let Finish::Finished {
            manual,
            follow_up: Some(full),
        } = coordinator.finish(&source_id, bounded.epoch)
        else {
            panic!("manual complete work should follow bounded work");
        };
        assert!(manual);
        assert_eq!(full.scope, ReconcileScope::All);
        assert_eq!(coordinator.running(&source_id), Some((full.epoch, true)));
    }

    #[test]
    fn manual_joins_running_work_and_freshness_queues_one_follow_up() {
        let source_id = source(1);
        let mut coordinator = SyncCoordinator::new();
        let running = coordinator
            .request(
                source_id.clone(),
                RequestKind::ActiveVerification,
                ReconcileScope::All,
            )
            .expect("first request starts");

        assert!(
            coordinator
                .request(source_id.clone(), RequestKind::Manual, ReconcileScope::All)
                .is_none()
        );
        coordinator.request(
            source_id.clone(),
            RequestKind::ActiveVerification,
            ReconcileScope::All,
        );
        coordinator.request(
            source_id.clone(),
            RequestKind::Freshness,
            ReconcileScope::All,
        );
        coordinator.request(
            source_id.clone(),
            RequestKind::Freshness,
            ReconcileScope::All,
        );
        assert_eq!(coordinator.running(&source_id), Some((running.epoch, true)));

        let Finish::Finished {
            manual,
            follow_up: Some(follow_up),
        } = coordinator.finish(&source_id, running.epoch)
        else {
            panic!("freshness should start one follow-up");
        };
        assert!(manual);
        assert_eq!(
            coordinator.running(&source_id),
            Some((follow_up.epoch, false))
        );
        assert!(matches!(
            coordinator.finish(&source_id, running.epoch),
            Finish::Ignored
        ));
        assert!(matches!(
            coordinator.finish(&source_id, follow_up.epoch),
            Finish::Finished {
                manual: false,
                follow_up: None
            }
        ));
    }

    #[test]
    fn activating_another_source_cancels_the_old_feed_and_automatic_work() {
        let automatic_source = source(3);
        let next_source = source(4);
        let mut coordinator = SyncCoordinator::new();
        let (old_lease, cancelled) = coordinator.activate(automatic_source.clone());
        assert!(cancelled.is_none());

        let automatic = coordinator
            .request_active(
                &automatic_source,
                &old_lease,
                RequestKind::Freshness,
                ReconcileScope::All,
            )
            .expect("automatic request starts");
        coordinator.request_active(
            &automatic_source,
            &old_lease,
            RequestKind::Freshness,
            ReconcileScope::All,
        );
        let (new_lease, cancelled) = coordinator.activate(next_source.clone());
        assert_eq!(
            cancelled,
            Some(CancelledRun {
                source_id: automatic_source.clone(),
                epoch: automatic.epoch,
                manual: false,
            })
        );
        assert!(old_lease.is_cancelled());
        assert!(!new_lease.is_cancelled());
        assert!(automatic.cancellation.is_cancelled());
        assert!(
            coordinator
                .request_active(
                    &automatic_source,
                    &old_lease,
                    RequestKind::Freshness,
                    ReconcileScope::All,
                )
                .is_none()
        );
        assert!(
            coordinator
                .request_active(
                    &next_source,
                    &new_lease,
                    RequestKind::Freshness,
                    ReconcileScope::All,
                )
                .is_some()
        );
        assert!(matches!(
            coordinator.finish(&automatic_source, automatic.epoch),
            Finish::Ignored
        ));
    }

    #[test]
    fn forget_invalidates_the_epoch_and_stale_finish_is_ignored() {
        let source_id = source(6);
        let mut coordinator = SyncCoordinator::new();
        let (lease, cancelled) = coordinator.activate(source_id.clone());
        assert!(cancelled.is_none());
        let first = coordinator
            .request(source_id.clone(), RequestKind::Manual, ReconcileScope::All)
            .expect("first request starts");
        coordinator.forget(&source_id);
        assert!(lease.is_cancelled());
        assert!(first.cancellation.is_cancelled());

        let second = coordinator
            .request(source_id.clone(), RequestKind::Manual, ReconcileScope::All)
            .expect("second request starts");
        assert_ne!(first.epoch, second.epoch);
        assert!(matches!(
            coordinator.finish(&source_id, first.epoch),
            Finish::Ignored
        ));
        assert!(!second.cancellation.is_cancelled());
        assert!(matches!(
            coordinator.finish(&source_id, second.epoch),
            Finish::Finished {
                manual: true,
                follow_up: None
            }
        ));
    }

    #[test]
    fn finishing_one_source_does_not_change_another() {
        let first_source = source(7);
        let second_source = source(8);
        let mut coordinator = SyncCoordinator::new();
        let first = coordinator
            .request(
                first_source.clone(),
                RequestKind::Manual,
                ReconcileScope::All,
            )
            .expect("first source starts");
        let second = coordinator
            .request(
                second_source.clone(),
                RequestKind::Freshness,
                ReconcileScope::All,
            )
            .expect("second source starts");

        assert!(matches!(
            coordinator.finish(&first_source, first.epoch),
            Finish::Finished {
                manual: true,
                follow_up: None
            }
        ));
        assert_eq!(
            coordinator.running(&second_source),
            Some((second.epoch, false))
        );
        assert!(matches!(
            coordinator.finish(&second_source, second.epoch),
            Finish::Finished {
                manual: false,
                follow_up: None
            }
        ));
    }

    #[test]
    fn cancellation_is_visible_immediately() {
        let cancellation = CancellationToken::new();
        assert!(cancellation.can_commit());
        let cancel = cancellation.clone();
        let thread = std::thread::spawn(move || {
            cancel.cancel();
        });

        thread.join().expect("cancellation thread");
        assert!(cancellation.is_cancelled());
        assert!(!cancellation.can_commit());
    }

    #[test]
    fn stale_work_retries_with_its_scope_and_interests() {
        let source_id = source(8);
        let mut coordinator = SyncCoordinator::new();
        let running = coordinator
            .request(
                source_id.clone(),
                RequestKind::Manual,
                objects(&["track-a"]),
            )
            .expect("first request starts");
        coordinator.request(
            source_id.clone(),
            RequestKind::Freshness,
            objects(&["track-b"]),
        );

        let Finish::Finished {
            manual,
            follow_up: Some(follow_up),
        } = coordinator.retry(&source_id, running.epoch)
        else {
            panic!("stale work should restart");
        };
        assert!(manual);
        assert_eq!(follow_up.scope, objects(&["track-a", "track-b"]));
        assert_eq!(
            coordinator.running(&source_id),
            Some((follow_up.epoch, true))
        );
    }
}
