mod eligibility;
mod services;
mod settings;
mod worker;

use playback::ListeningFact;
use tracing::warn;

pub use eligibility::scrobble_threshold_millis;
pub use services::audioscrobbler::{
    AudioscrobblerSession, lastfm_auth_url, librefm_auth_url, request_lastfm_auth_token,
    request_lastfm_session, request_librefm_auth_token, request_librefm_session,
};
pub use settings::{
    AudioscrobblerSettings, LASTFM_API_SECRET, LASTFM_SESSION, LIBREFM_SESSION, LISTENBRAINZ_TOKEN,
    ListenBrainzSettings, SecretDescriptor, Settings, secret_descriptors,
};

use eligibility::Eligibility;
use worker::{QueueResult, Worker};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchOutcome {
    Ignored,
    Queued,
    Dropped,
}

pub struct Scrobbler {
    settings: Settings,
    eligibility: Eligibility,
    worker: Worker,
}

impl Scrobbler {
    pub fn new(mut settings: Settings) -> Result<Self, String> {
        settings.sanitize();
        Ok(Self {
            settings,
            eligibility: Eligibility::default(),
            worker: Worker::new()?,
        })
    }

    pub fn update_settings(&mut self, mut settings: Settings) {
        settings.sanitize();
        self.settings = settings;
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn observe(&mut self, fact: &ListeningFact) -> DispatchOutcome {
        self.observe_with_delivery(fact, true)
    }

    pub fn observe_with_delivery(
        &mut self,
        fact: &ListeningFact,
        delivery_enabled: bool,
    ) -> DispatchOutcome {
        let settings = self.settings.clone();
        let now_playing = matches!(fact, ListeningFact::Started { .. });
        let dispatch_enabled = delivery_enabled && settings.has_target(now_playing);
        let submission = self.eligibility.observe(fact, dispatch_enabled);
        let Some(submission) = submission else {
            return DispatchOutcome::Ignored;
        };
        match self.worker.queue(settings, submission) {
            QueueResult::Queued => DispatchOutcome::Queued,
            QueueResult::Full => {
                warn!("scrobbling worker queue is full; dropping best-effort submission");
                DispatchOutcome::Dropped
            }
            QueueResult::Closed => {
                warn!("scrobbling worker is unavailable; dropping best-effort submission");
                DispatchOutcome::Dropped
            }
        }
    }
}
