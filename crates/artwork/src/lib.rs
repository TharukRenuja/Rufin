use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_channel::Receiver;

use library::SourceId;
use sources::ImageProvider;
use thiserror::Error;
use tokio::runtime::Runtime;

mod cache;
mod decode;
mod fetch;
mod pipeline;
mod selection;

#[cfg(test)]
mod tests;

pub use decode::{DecodedImage, RgbaImage, decode_rgba, square_thumbnail_png};
pub use selection::{ArtworkBinding, ArtworkPresentation};

#[derive(Clone)]
pub struct SourceImages {
    pub source_id: SourceId,
    pub(crate) provider: Option<Arc<dyn ImageProvider + Send + Sync>>,
}

impl SourceImages {
    pub fn new(source_id: SourceId, provider: Arc<dyn ImageProvider + Send + Sync>) -> Self {
        Self {
            source_id,
            provider: Some(provider),
        }
    }

    pub fn cache_only(source_id: SourceId) -> Self {
        Self {
            source_id,
            provider: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExternalPolicy {
    pub allow_cached: bool,
    pub allow_network: bool,
    pub allow_musicbrainz: bool,
    pub lastfm_api_key: String,
}

impl ExternalPolicy {
    pub fn new(allow_cached: bool, allow_network: bool, lastfm_api_key: impl Into<String>) -> Self {
        Self {
            allow_cached,
            allow_network,
            allow_musicbrainz: true,
            lastfm_api_key: lastfm_api_key.into(),
        }
    }

    pub const fn with_musicbrainz(mut self, allow: bool) -> Self {
        self.allow_musicbrainz = allow;
        self
    }

    pub const fn disabled() -> Self {
        Self {
            allow_cached: false,
            allow_network: false,
            allow_musicbrainz: false,
            lastfm_api_key: String::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtworkRequest {
    pub binding: ArtworkBinding,
    pub fetch_size: u32,
    pub render_size: u32,
    pub external: ExternalPolicy,
}

impl ArtworkRequest {
    pub fn new(binding: ArtworkBinding, fetch_size: u32, render_size: u32) -> Self {
        Self {
            binding,
            fetch_size: fetch_size.max(1),
            render_size: render_size.max(1),
            external: ExternalPolicy::disabled(),
        }
    }

    pub fn with_external(mut self, external: ExternalPolicy) -> Self {
        self.external = external;
        self
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ArtworkKey(String);

impl ArtworkKey {
    fn new(identity: String) -> Self {
        Self(format!("{:x}", md5::compute(identity.as_bytes())))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RequestId(u64);

impl RequestId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PrefetchOwner(u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PrefetchPriority {
    Viewport,
    Background,
    Idle,
}

#[derive(Clone, Debug)]
pub enum Readiness {
    Pending,
    Ready(Arc<DecodedImage>),
    Missing,
    Failed(Arc<str>),
}

#[derive(Clone, Debug)]
pub struct ArtworkProjection {
    pub request_id: RequestId,
    pub readiness: Readiness,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtworkVisualIdentity(String);

impl ArtworkVisualIdentity {
    fn new(identity: String) -> Self {
        Self(format!("{:x}", md5::compute(identity.as_bytes())))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtworkRequestIdentity(String);

impl ArtworkRequestIdentity {
    fn new(identity: String) -> Self {
        Self(format!("{:x}", md5::compute(identity.as_bytes())))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtworkBindingIdentity {
    pub visual: ArtworkVisualIdentity,
    pub request: ArtworkRequestIdentity,
}

pub struct PreparedArtwork {
    pub identity: ArtworkBindingIdentity,
    pub ready: Option<Arc<DecodedImage>>,
    source: SourceImages,
    request: ArtworkRequest,
}

#[derive(Clone, Debug)]
pub enum ArtworkEvent {
    Changed(ArtworkProjection),
    Invalidated(RequestId),
}

#[derive(Debug, Error)]
pub enum ArtworkError {
    #[error("artwork cache failed: {0}")]
    Cache(#[from] std::io::Error),
    #[error("artwork decode failed: {0}")]
    Decode(String),
    #[error("artwork pipeline is busy")]
    Busy,
    #[error("artwork fetch setup failed: {0}")]
    FetchSetup(String),
}

#[derive(Clone)]
pub struct Artwork {
    pipeline: Arc<pipeline::Pipeline>,
}

impl Artwork {
    pub fn new(
        cache_root: impl AsRef<Path>,
        runtime: Arc<Runtime>,
    ) -> Result<(Self, Receiver<ArtworkEvent>), ArtworkError> {
        let cache_root = cache::current_layout(cache_root.as_ref())?;
        let (pipeline, events) = pipeline::Pipeline::new(&cache_root, runtime)?;
        Ok((
            Self {
                pipeline: Arc::new(pipeline),
            },
            events,
        ))
    }

    pub fn request(
        &self,
        source: SourceImages,
        request: ArtworkRequest,
    ) -> Result<ArtworkProjection, ArtworkError> {
        self.pipeline.request(source, request)
    }

    pub fn prepare(&self, source: SourceImages, request: ArtworkRequest) -> PreparedArtwork {
        let (identity, ready) = self.pipeline.binding_identity_and_ready(&source, &request);
        PreparedArtwork {
            identity,
            ready,
            source,
            request,
        }
    }

    pub fn request_prepared(
        &self,
        prepared: PreparedArtwork,
    ) -> Result<ArtworkProjection, ArtworkError> {
        self.request(prepared.source, prepared.request)
    }

    pub fn allocate_prefetch_owner(&self) -> PrefetchOwner {
        self.pipeline.allocate_prefetch_owner()
    }

    pub fn replace_prefetch(
        &self,
        owner: PrefetchOwner,
        priority: PrefetchPriority,
        source: SourceImages,
        requests: Vec<ArtworkRequest>,
    ) {
        self.pipeline
            .replace_prefetch(owner, priority, source, requests);
    }

    pub fn clear_prefetch(&self, owner: PrefetchOwner) {
        self.pipeline.clear_prefetch(owner);
    }

    pub fn set_prefetch_paused(&self, priority: PrefetchPriority, paused: bool) {
        self.pipeline.set_prefetch_paused(priority, paused);
    }

    pub fn cancel(&self, request_id: RequestId) {
        self.pipeline.cancel(request_id);
    }

    #[cfg(test)]
    pub(crate) fn has_pending_request(&self, request_id: RequestId) -> bool {
        self.pipeline.projection(request_id).is_some()
    }

    pub fn cache_only_file(
        &self,
        source_id: &SourceId,
        request: &ArtworkRequest,
    ) -> Option<PathBuf> {
        self.pipeline.cache_only_file(source_id, request)
    }

    pub fn binding_identity(
        &self,
        source: &SourceImages,
        request: &ArtworkRequest,
    ) -> ArtworkBindingIdentity {
        self.pipeline.binding_identity(source, request)
    }

    pub fn retry_external(&self) -> Result<(), ArtworkError> {
        self.pipeline.retry_external()
    }

    pub fn invalidate_source(&self, source_id: &SourceId) -> Result<(), ArtworkError> {
        self.pipeline.invalidate_source(source_id)
    }

    pub fn resolve_public_album_url(
        &self,
        candidates: &ArtworkBinding,
        size: u32,
        external: &ExternalPolicy,
    ) -> Result<Option<String>, String> {
        self.pipeline
            .resolve_public_album_url(candidates, size, external)
    }
}
