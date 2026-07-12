use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use reqwest::blocking::Client;
use tracing::warn;

use crate::eligibility::Submission;
use crate::services::{audioscrobbler, listenbrainz};
use crate::settings::Settings;

const WORKER_CAPACITY: usize = 32;
const USER_AGENT: &str = concat!("Rufin/", env!("CARGO_PKG_VERSION"));

struct Job {
    settings: Settings,
    submission: Submission,
}

pub(crate) enum QueueResult {
    Queued,
    Full,
    Closed,
}

pub(crate) struct Worker {
    sender: SyncSender<Job>,
    _thread: JoinHandle<()>,
}

impl Worker {
    pub(crate) fn new() -> Result<Self, String> {
        let (sender, receiver) = sync_channel::<Job>(WORKER_CAPACITY);
        let client = Client::builder()
            .timeout(Duration::from_secs(6))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| error.to_string())?;
        let thread = std::thread::Builder::new()
            .name("rufin-scrobbling".to_string())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    submit(&client, &job.settings, &job.submission);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            sender,
            _thread: thread,
        })
    }

    pub(crate) fn queue(&self, settings: Settings, submission: Submission) -> QueueResult {
        match self.sender.try_send(Job {
            settings,
            submission,
        }) {
            Ok(()) => QueueResult::Queued,
            Err(TrySendError::Full(_)) => QueueResult::Full,
            Err(TrySendError::Disconnected(_)) => QueueResult::Closed,
        }
    }
}

fn submit(client: &Client, settings: &Settings, submission: &Submission) {
    for (service, service_settings) in [
        (audioscrobbler::Service::LastFm, &settings.lastfm),
        (audioscrobbler::Service::LibreFm, &settings.librefm),
    ] {
        if let Err(error) = audioscrobbler::submit(client, service, service_settings, submission) {
            warn!(%error, ?service, "audioscrobbler submission failed");
        }
    }
    if let Err(error) = listenbrainz::submit(client, &settings.listenbrainz, submission) {
        warn!(%error, "ListenBrainz submission failed");
    }
}
