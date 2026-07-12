use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use playback::{RunId, SourceReportPhase};
use tokio::runtime::Runtime;
use tracing::warn;

use crate::source_setup::NativePlaybackReporting;

const REPORT_QUEUE_CAPACITY: usize = 64;
const REPORT_QUEUE_SATURATED: &str = "Source playback reporting is busy; a report was dropped.";
const REPORT_QUEUE_UNAVAILABLE: &str = "Source playback reporting is unavailable.";

struct ReportJob {
    reporter: NativePlaybackReporting,
    report: sources::PlaybackReport,
}

struct PendingReport<T> {
    run: RunId,
    phase: SourceReportPhase,
    payload: T,
}

impl<T> PendingReport<T> {
    fn progress(&self) -> bool {
        self.phase == SourceReportPhase::Progress
    }
}

struct PendingReports<T> {
    capacity: usize,
    items: VecDeque<PendingReport<T>>,
}

impl<T> PendingReports<T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: VecDeque::with_capacity(capacity),
        }
    }

    fn push(&mut self, pending: PendingReport<T>) -> Result<(), PendingReport<T>> {
        if pending.progress()
            && let Some(index) = self
                .items
                .iter()
                .position(|queued| queued.progress() && queued.run == pending.run)
        {
            self.items.remove(index);
            self.items.push_back(pending);
            return Ok(());
        }
        if self.items.len() < self.capacity {
            self.items.push_back(pending);
            return Ok(());
        }
        if !pending.progress()
            && let Some(index) = self.items.iter().position(PendingReport::progress)
        {
            self.items.remove(index);
            self.items.push_back(pending);
            return Ok(());
        }
        Err(pending)
    }

    fn pop(&mut self) -> Option<PendingReport<T>> {
        self.items.pop_front()
    }
}

struct QueueState {
    reports: PendingReports<ReportJob>,
    closed: bool,
}

struct SharedQueue {
    state: Mutex<QueueState>,
    available: Condvar,
}

pub(super) struct SourceReportWorker {
    queue: Arc<SharedQueue>,
}

impl SourceReportWorker {
    pub(super) fn new(runtime: Arc<Runtime>) -> Result<Self, String> {
        let queue = Arc::new(SharedQueue {
            state: Mutex::new(QueueState {
                reports: PendingReports::new(REPORT_QUEUE_CAPACITY),
                closed: false,
            }),
            available: Condvar::new(),
        });
        let worker_queue = Arc::clone(&queue);
        thread::Builder::new()
            .name("rufin-source-report".to_string())
            .spawn(move || run_worker(&runtime, &worker_queue))
            .map_err(|error| error.to_string())?;
        Ok(Self { queue })
    }

    pub(super) fn submit(
        &self,
        run: RunId,
        phase: SourceReportPhase,
        reporter: NativePlaybackReporting,
        report: sources::PlaybackReport,
    ) -> Result<(), &'static str> {
        let mut state = self
            .queue
            .state
            .lock()
            .map_err(|_| REPORT_QUEUE_UNAVAILABLE)?;
        state
            .reports
            .push(PendingReport {
                run,
                phase,
                payload: ReportJob { reporter, report },
            })
            .map_err(|_| REPORT_QUEUE_SATURATED)?;
        drop(state);
        self.queue.available.notify_one();
        Ok(())
    }
}

impl Drop for SourceReportWorker {
    fn drop(&mut self) {
        if let Ok(mut state) = self.queue.state.lock() {
            state.closed = true;
        }
        self.queue.available.notify_one();
    }
}

fn run_worker(runtime: &Runtime, queue: &SharedQueue) {
    while let Some(pending) = next_report(queue) {
        let ReportJob { reporter, report } = pending.payload;
        if let Err(error) = runtime.block_on(reporter.report_playback(report)) {
            warn!(
                %error,
                run = %pending.run,
                phase = ?pending.phase,
                "failed to report playback to source"
            );
        }
    }
}

fn next_report(queue: &SharedQueue) -> Option<PendingReport<ReportJob>> {
    let mut state = queue.state.lock().ok()?;
    loop {
        if let Some(report) = state.reports.pop() {
            return Some(report);
        }
        if state.closed {
            return None;
        }
        state = queue.available.wait(state).ok()?;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;

    use async_trait::async_trait;
    use library::TrackId;
    use sources::{PlaybackReport, PlaybackReportKind, PlaybackReporter, SourceResult};

    use super::*;

    #[test]
    fn newer_progress_replaces_and_repositions_the_pending_run_progress() {
        let mut reports = PendingReports::new(4);
        push(&mut reports, 1, SourceReportPhase::Started, "started");
        push(&mut reports, 1, SourceReportPhase::Progress, "old progress");
        push(
            &mut reports,
            1,
            SourceReportPhase::QualifiedPlay,
            "qualified",
        );
        push(&mut reports, 1, SourceReportPhase::Progress, "new progress");

        assert_eq!(
            payloads(&mut reports),
            ["started", "qualified", "new progress"]
        );
    }

    #[test]
    fn a_nonreplaceable_fact_evicts_progress_without_reordering_other_facts() {
        let mut reports = PendingReports::new(3);
        push(&mut reports, 1, SourceReportPhase::Started, "first started");
        push(&mut reports, 1, SourceReportPhase::Progress, "progress");
        push(
            &mut reports,
            2,
            SourceReportPhase::Started,
            "second started",
        );
        push(&mut reports, 1, SourceReportPhase::Ended, "ended");

        assert_eq!(
            payloads(&mut reports),
            ["first started", "second started", "ended"]
        );
    }

    #[test]
    fn a_full_nonreplaceable_queue_rejects_new_work_without_mutation() {
        let mut reports = PendingReports::new(2);
        push(&mut reports, 1, SourceReportPhase::Started, "started");
        push(
            &mut reports,
            1,
            SourceReportPhase::QualifiedPlay,
            "qualified",
        );
        let rejected = reports
            .push(pending(1, SourceReportPhase::Ended, "ended"))
            .expect_err("queue without progress is saturated");

        assert_eq!(rejected.payload, "ended");
        assert_eq!(payloads(&mut reports), ["started", "qualified"]);
    }

    #[test]
    fn worker_uses_its_single_named_thread() {
        let runtime = Arc::new(Runtime::new().expect("runtime"));
        let (sent, received) = channel();
        let reporter: NativePlaybackReporting = Arc::new(RecordingReporter(sent));
        let worker = SourceReportWorker::new(runtime).expect("worker");

        worker
            .submit(
                RunId::new(7),
                SourceReportPhase::Started,
                reporter,
                report(PlaybackReportKind::Started),
            )
            .expect("submit report");

        let (thread_name, report) = received
            .recv_timeout(Duration::from_secs(1))
            .expect("reported playback");
        assert_eq!(thread_name, "rufin-source-report");
        assert_eq!(report.kind, PlaybackReportKind::Started);
    }

    fn push<T>(reports: &mut PendingReports<T>, run: u64, phase: SourceReportPhase, payload: T) {
        assert!(reports.push(pending(run, phase, payload)).is_ok());
    }

    fn pending<T>(run: u64, phase: SourceReportPhase, payload: T) -> PendingReport<T> {
        PendingReport {
            run: RunId::new(run),
            phase,
            payload,
        }
    }

    fn payloads<T>(reports: &mut PendingReports<T>) -> Vec<T> {
        std::iter::from_fn(|| reports.pop().map(|pending| pending.payload)).collect()
    }

    struct RecordingReporter(Sender<(String, PlaybackReport)>);

    #[async_trait(?Send)]
    impl PlaybackReporter for RecordingReporter {
        async fn report_playback(&self, report: PlaybackReport) -> SourceResult<()> {
            let thread_name = thread::current().name().unwrap_or_default().to_string();
            let _ = self.0.send((thread_name, report));
            Ok(())
        }
    }

    fn report(kind: PlaybackReportKind) -> PlaybackReport {
        PlaybackReport {
            kind,
            track_id: TrackId::fake(1),
            position_seconds: 0,
            paused: false,
            muted: false,
            volume_percent: 100,
            shuffle: false,
            repeat_one: false,
            repeat_all: false,
            failed: false,
        }
    }
}
