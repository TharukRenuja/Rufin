use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

use artwork::{
    ArtworkEvent, ArtworkRequest, CandidateSet, ExternalPolicy, RequestId, SourceImages,
};
use library::SourceId;
use tracing::warn;

use crate::StoredSettings;
use crate::source_setup::current_active_source;

use super::{AppController, ControllerEvent};

pub(crate) struct PreparedArtwork {
    pub(crate) identity: artwork::ArtworkBindingIdentity,
    source: SourceImages,
    request: ArtworkRequest,
}

#[cfg(test)]
static TEST_ARTWORK_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) fn open(
    cache_root: &Path,
    runtime: Arc<tokio::runtime::Runtime>,
) -> Result<(artwork::Artwork, Receiver<ArtworkEvent>), String> {
    artwork::Artwork::new(cache_root, runtime).map_err(|error| error.to_string())
}

pub(super) fn forward_events(
    receiver: Receiver<ArtworkEvent>,
    events: Sender<ControllerEvent>,
) -> Result<(), String> {
    thread::Builder::new()
        .name("rufin-artwork-events".to_string())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                if let ArtworkEvent::Changed(projection) = &event
                    && let artwork::Readiness::Failed(error) = &projection.readiness
                {
                    warn!(
                        request_id = projection.request_id.get(),
                        %error,
                        "artwork request failed"
                    );
                }
                if events.send(ControllerEvent::Artwork(event)).is_err() {
                    break;
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to start artwork event forwarding: {error}"))
}

#[cfg(test)]
pub(super) fn open_for_test(
    runtime: Arc<tokio::runtime::Runtime>,
    events: Sender<ControllerEvent>,
) -> artwork::Artwork {
    let root = std::env::temp_dir().join(format!(
        "rufin-artwork-test-{}-{}",
        std::process::id(),
        TEST_ARTWORK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let (artwork, receiver) =
        open(&root, runtime).unwrap_or_else(|error| panic!("failed to open test artwork: {error}"));
    forward_events(receiver, events)
        .unwrap_or_else(|error| panic!("failed to forward test artwork events: {error}"));
    artwork
}

impl AppController {
    pub(crate) fn prepare_artwork(
        &self,
        candidates: CandidateSet,
        fetch_size: u32,
        render_size: u32,
        settings: &StoredSettings,
    ) -> Result<PreparedArtwork, String> {
        let source = self.active_artwork_source()?;
        let request = artwork_request(candidates, fetch_size, render_size, settings);
        Ok(self.prepare_artwork_request(source, request))
    }

    pub(crate) fn prepare_playback_artwork(
        &self,
        source_id: &SourceId,
        candidates: CandidateSet,
        fetch_size: u32,
        render_size: u32,
        settings: &StoredSettings,
    ) -> PreparedArtwork {
        let active = current_active_source(&self.active_source);
        let matching = active.filter(|active| active.identity.id == *source_id);
        let cache_only = matching.is_none();
        let source = matching.map_or_else(
            || SourceImages::cache_only(source_id.clone()),
            |active| SourceImages::new(active.identity.id.clone(), Arc::clone(&active.images)),
        );
        let mut request = artwork_request(candidates, fetch_size, render_size, settings);
        if cache_only {
            request.external.allow_network = false;
        }
        self.prepare_artwork_request(source, request)
    }

    fn prepare_artwork_request(
        &self,
        source: SourceImages,
        request: ArtworkRequest,
    ) -> PreparedArtwork {
        let identity = self.artwork.binding_identity(&source, &request);
        PreparedArtwork {
            identity,
            source,
            request,
        }
    }

    pub(crate) fn request_artwork(
        &self,
        prepared: PreparedArtwork,
    ) -> Result<artwork::ArtworkProjection, String> {
        self.artwork
            .request(prepared.source, prepared.request)
            .map_err(|error| error.to_string())
    }

    pub fn cancel_artwork(&self, request_id: RequestId) {
        self.artwork.cancel(request_id);
    }

    pub fn cached_artwork_path(
        &self,
        source_id: &SourceId,
        candidates: CandidateSet,
        fetch_size: u32,
        render_size: u32,
        settings: &StoredSettings,
    ) -> Option<PathBuf> {
        let request = artwork_request(candidates, fetch_size, render_size, settings);
        self.artwork.cache_only_file(source_id, &request)
    }

    pub fn retry_external_artwork(&self) -> Result<(), String> {
        self.artwork
            .retry_external()
            .map_err(|error| error.to_string())
    }

    pub fn invalidate_artwork_source(&self, source_id: &SourceId) -> Result<(), String> {
        self.artwork
            .invalidate_source(source_id)
            .map_err(|error| error.to_string())
    }

    fn active_artwork_source(&self) -> Result<SourceImages, String> {
        let active = current_active_source(&self.active_source)
            .ok_or_else(|| "No source is active for artwork.".to_string())?;
        Ok(SourceImages::new(
            active.identity.id.clone(),
            Arc::clone(&active.images),
        ))
    }
}

fn artwork_request(
    candidates: CandidateSet,
    fetch_size: u32,
    render_size: u32,
    settings: &StoredSettings,
) -> ArtworkRequest {
    ArtworkRequest::new(candidates, fetch_size, render_size)
        .with_external(artwork_external_policy(settings))
}

fn artwork_external_policy(settings: &StoredSettings) -> ExternalPolicy {
    ExternalPolicy::new(
        settings.metadata.external_metadata_enabled,
        settings.metadata.external_metadata_enabled && !settings.private_mode,
        settings.lastfm_api_key.clone(),
    )
}
