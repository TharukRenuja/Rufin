//! Durable retry work for completed external scrobbles.
//!
//! Now-playing and source-native reports remain transient. Only a qualified
//! completed play that an external service has not accepted belongs here.

use crate::{Library, LibraryResult};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ScrobbleService {
    LastFm,
    LibreFm,
    ListenBrainz,
}

impl ScrobbleService {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::LastFm => "lastfm",
            Self::LibreFm => "librefm",
            Self::ListenBrainz => "listenbrainz",
        }
    }

    pub(crate) fn from_stored(value: &str) -> Option<Self> {
        match value {
            "lastfm" => Some(Self::LastFm),
            "librefm" => Some(Self::LibreFm),
            "listenbrainz" => Some(Self::ListenBrainz),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingScrobbleId {
    pub service: ScrobbleService,
    pub account_id: String,
    pub play_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingScrobble {
    pub id: PendingScrobbleId,
    pub track_title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub duration_millis: u64,
    pub started_at: i64,
    pub attempts: u32,
    pub next_attempt_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewScrobble {
    pub id: PendingScrobbleId,
    pub track_title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub duration_millis: u64,
    pub started_at: i64,
}

impl Library {
    /// Persists one qualified play for all configured external services.
    pub fn queue_scrobbles(&self, scrobbles: Vec<NewScrobble>) -> LibraryResult<usize> {
        for scrobble in &scrobbles {
            validate_new(scrobble)?;
        }
        Ok(self.store.queue_scrobbles(scrobbles)?)
    }

    pub fn due_scrobbles(
        &self,
        service: ScrobbleService,
        account_id: &str,
        now: i64,
        limit: usize,
    ) -> LibraryResult<Vec<PendingScrobble>> {
        if account_id.is_empty() || now < 0 || limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .store
            .due_scrobbles(service, account_id.to_string(), now, limit.min(500))?)
    }

    pub fn complete_scrobble(&self, id: PendingScrobbleId) -> LibraryResult<()> {
        self.store.complete_scrobble(id)?;
        Ok(())
    }

    /// Drops retry work that can no longer be sent to the account that owned it.
    pub fn discard_scrobbles(
        &self,
        service: ScrobbleService,
        account_id: &str,
    ) -> LibraryResult<usize> {
        if account_id.is_empty() {
            return Ok(0);
        }
        Ok(self
            .store
            .discard_scrobbles(service, account_id.to_string())?)
    }

    pub fn defer_scrobble(&self, id: PendingScrobbleId, next_attempt_at: i64) -> LibraryResult<()> {
        if next_attempt_at < 0 {
            return Err(crate::LibraryError::Persistence(
                "deferred scrobble is missing its next attempt".to_string(),
            ));
        }
        self.store.defer_scrobble(id, next_attempt_at)?;
        Ok(())
    }

    /// Keeps completed work dormant until this account's credentials change.
    pub fn block_scrobbles(
        &self,
        service: ScrobbleService,
        account_id: &str,
        error: impl Into<String>,
    ) -> LibraryResult<usize> {
        let error = error.into();
        if account_id.is_empty() || error.is_empty() {
            return Err(crate::LibraryError::Persistence(
                "credential-blocked scrobble is missing its error".to_string(),
            ));
        }
        Ok(self
            .store
            .block_scrobbles(service, account_id.to_string(), error)?)
    }

    /// Makes credential-blocked work due after this same account is reauthorized.
    pub fn wake_scrobbles(
        &self,
        service: ScrobbleService,
        account_id: &str,
        now: i64,
    ) -> LibraryResult<usize> {
        if account_id.is_empty() || now < 0 {
            return Ok(0);
        }
        Ok(self
            .store
            .wake_scrobbles(service, account_id.to_string(), now)?)
    }
}

fn validate_new(scrobble: &NewScrobble) -> LibraryResult<()> {
    if scrobble.id.account_id.is_empty()
        || scrobble.id.play_id.is_empty()
        || scrobble.track_title.is_empty()
        || scrobble.artist_name.is_empty()
        || scrobble.started_at < 0
    {
        return Err(crate::LibraryError::Persistence(
            "completed scrobble is missing required identity".to_string(),
        ));
    }
    Ok(())
}
