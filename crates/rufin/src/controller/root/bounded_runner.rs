use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

struct Lane {
    sender: SyncSender<Job>,
}

#[derive(Clone)]
pub(super) struct BoundedRunner {
    lanes: Arc<[Lane]>,
    next_lane: Arc<AtomicUsize>,
    label: &'static str,
}

impl BoundedRunner {
    pub(super) fn new(
        label: &'static str,
        thread_name: &'static str,
        workers: usize,
    ) -> Result<Self, String> {
        let mut lanes = Vec::with_capacity(workers);
        for index in 0..workers.max(1) {
            let (sender, receiver) = sync_channel::<Job>(1);
            thread::Builder::new()
                .name(format!("{thread_name}-{index}"))
                .spawn(move || {
                    while let Ok(job) = receiver.recv() {
                        job();
                    }
                })
                .map_err(|error| error.to_string())?;
            lanes.push(Lane { sender });
        }
        Ok(Self {
            lanes: lanes.into(),
            next_lane: Arc::new(AtomicUsize::new(0)),
            label,
        })
    }

    pub(super) fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        let mut job: Job = Box::new(job);
        let start = self.next_lane.fetch_add(1, Ordering::Relaxed);
        for offset in 0..self.lanes.len() {
            let lane = &self.lanes[(start + offset) % self.lanes.len()];
            match lane.sender.try_send(job) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(returned) | TrySendError::Disconnected(returned)) => {
                    job = returned;
                }
            }
        }
        Err(format!("{} is busy; try again.", self.label))
    }
}
