use std::sync::Arc;
use std::time::Duration;

use sources::{
    LibraryChange, LibraryChangeFeed, LibraryFreshnessProbe, LibraryProbeResult, SourceError,
};

use crate::{CancellationToken, ReconcileScope, RequestKind};

const FEED_RETRY_MIN: Duration = Duration::from_secs(5);
const FEED_RETRY_MAX: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub enum Freshness {
    Events(Arc<dyn LibraryChangeFeed + Send + Sync>),
    Probe {
        interval: Duration,
        probe: Arc<dyn LibraryFreshnessProbe + Send + Sync>,
    },
}

impl Freshness {
    pub async fn run(
        &self,
        cancellation: &CancellationToken,
        request: &dyn Fn(RequestKind, ReconcileScope) -> bool,
        report_error: &dyn Fn(&SourceError),
    ) {
        match self {
            Self::Events(feed) => {
                let mut delay = FEED_RETRY_MIN;
                while !cancellation.is_cancelled() {
                    let result = feed
                        .listen(
                            &mut || request(RequestKind::ActiveVerification, ReconcileScope::All),
                            &mut |change| request(RequestKind::Freshness, scope_for_change(change)),
                            &|| cancellation.is_cancelled(),
                        )
                        .await;
                    if cancellation.is_cancelled() {
                        break;
                    }
                    if let Err(error) = result.as_ref() {
                        report_error(error);
                    }
                    if !cancellation.wait(delay).await {
                        break;
                    }
                    delay = delay.saturating_mul(2).min(FEED_RETRY_MAX);
                }
            }
            Self::Probe { interval, probe } => {
                let mut first = true;
                loop {
                    if !first && !cancellation.wait(*interval).await {
                        break;
                    }
                    first = false;
                    if cancellation.is_cancelled() {
                        break;
                    }
                    match probe.probe().await {
                        Ok(LibraryProbeResult::Changed) => {
                            if !request(RequestKind::Freshness, ReconcileScope::All) {
                                break;
                            }
                        }
                        Ok(
                            LibraryProbeResult::Unchanged
                            | LibraryProbeResult::Unknown
                            | LibraryProbeResult::Busy,
                        ) => {}
                        Err(error) => report_error(&error),
                    }
                }
            }
        }
    }
}

fn scope_for_change(change: LibraryChange) -> ReconcileScope {
    match change {
        LibraryChange::Objects(changes) => ReconcileScope::objects(changes),
        LibraryChange::Full => ReconcileScope::All,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use sources::{SourceObjectChanges, SourceResult};

    use super::*;

    struct ChangedThenDisconnected;

    #[async_trait(?Send)]
    impl LibraryChangeFeed for ChangedThenDisconnected {
        async fn listen(
            &self,
            on_ready: &mut dyn FnMut() -> bool,
            on_change: &mut dyn FnMut(LibraryChange) -> bool,
            should_stop: &dyn Fn() -> bool,
        ) -> SourceResult<()> {
            assert!(!should_stop());
            assert!(on_ready());
            assert!(on_change(LibraryChange::Objects(SourceObjectChanges::new(
                ["track-a".to_string(),]
            ))));
            Err(SourceError::Network("disconnected".to_string()))
        }
    }

    struct ProbeSequence {
        results: Mutex<VecDeque<LibraryProbeResult>>,
    }

    #[async_trait(?Send)]
    impl LibraryFreshnessProbe for ProbeSequence {
        async fn probe(&self) -> SourceResult<LibraryProbeResult> {
            Ok(self
                .results
                .lock()
                .expect("probe results")
                .pop_front()
                .expect("probe result"))
        }
    }

    #[tokio::test]
    async fn event_feed_requests_ready_and_changed_work_before_retry() {
        let freshness = Freshness::Events(Arc::new(ChangedThenDisconnected));
        let cancellation = CancellationToken::new();
        let requests = Mutex::new(Vec::new());
        let errors = Mutex::new(Vec::new());

        freshness
            .run(
                &cancellation,
                &|kind, scope| {
                    let mut requests = requests.lock().expect("requests");
                    requests.push((kind, scope));
                    true
                },
                &|error| {
                    errors.lock().expect("errors").push(error.to_string());
                    cancellation.cancel();
                },
            )
            .await;

        assert_eq!(
            *requests.lock().expect("requests"),
            vec![
                (RequestKind::ActiveVerification, ReconcileScope::All),
                (
                    RequestKind::Freshness,
                    ReconcileScope::objects(SourceObjectChanges::new(["track-a".to_string()])),
                ),
            ]
        );
        assert_eq!(errors.lock().expect("errors").len(), 1);
    }

    #[tokio::test]
    async fn probe_requests_work_only_when_changed() {
        let freshness = Freshness::Probe {
            interval: Duration::ZERO,
            probe: Arc::new(ProbeSequence {
                results: Mutex::new(VecDeque::from([
                    LibraryProbeResult::Unchanged,
                    LibraryProbeResult::Unknown,
                    LibraryProbeResult::Busy,
                    LibraryProbeResult::Changed,
                ])),
            }),
        };
        let cancellation = CancellationToken::new();
        let requests = Mutex::new(Vec::new());

        freshness
            .run(
                &cancellation,
                &|kind, scope| {
                    let mut requests = requests.lock().expect("requests");
                    requests.push((kind, scope));
                    cancellation.cancel();
                    !cancellation.is_cancelled()
                },
                &|error| panic!("unexpected probe error: {error}"),
            )
            .await;

        assert_eq!(
            *requests.lock().expect("requests"),
            vec![(RequestKind::Freshness, ReconcileScope::All)]
        );
    }
}
