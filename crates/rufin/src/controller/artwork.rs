use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_channel::Receiver;

#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) use artwork::PreparedArtwork;
use artwork::{
    ArtworkBinding, ArtworkEvent, ArtworkRequest, ExternalPolicy, PrefetchOwner, PrefetchPriority,
    RequestId, SourceImages,
};
use library::SourceId;

use crate::source_setup::current_active_source;

use super::root::ArtworkCommands;

#[cfg(test)]
static TEST_ARTWORK_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) fn open(
    cache_root: &Path,
    runtime: Arc<tokio::runtime::Runtime>,
) -> Result<(artwork::Artwork, Receiver<ArtworkEvent>), String> {
    artwork::Artwork::new(cache_root, runtime).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(super) fn open_for_test(
    runtime: Arc<tokio::runtime::Runtime>,
) -> (artwork::Artwork, Receiver<ArtworkEvent>) {
    let root = std::env::temp_dir().join(format!(
        "rufin-artwork-test-{}-{}",
        std::process::id(),
        TEST_ARTWORK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    open(&root, runtime).unwrap_or_else(|error| panic!("failed to open test artwork: {error}"))
}

impl ArtworkCommands {
    pub(crate) fn prepare_artwork(
        &self,
        binding: ArtworkBinding,
        fetch_size: u32,
        render_size: u32,
        external: ExternalPolicy,
    ) -> Result<PreparedArtwork, String> {
        let source = self.active_artwork_source()?;
        let request = artwork_request(binding, fetch_size, render_size, external);
        Ok(self.artwork.prepare(source, request))
    }

    pub(crate) fn prepare_playback_artwork(
        &self,
        source_id: &SourceId,
        binding: ArtworkBinding,
        fetch_size: u32,
        render_size: u32,
        external: ExternalPolicy,
    ) -> PreparedArtwork {
        let active = current_active_source(&self.active_source);
        let matching = active.filter(|active| active.identity.id == *source_id);
        let cache_only = matching.is_none();
        let source = matching.map_or_else(
            || SourceImages::cache_only(source_id.clone()),
            |active| SourceImages::new(active.identity.id.clone(), Arc::clone(&active.images)),
        );
        let mut request = artwork_request(binding, fetch_size, render_size, external);
        if cache_only {
            request.external.allow_network = false;
        }
        self.artwork.prepare(source, request)
    }

    pub(crate) fn request_artwork(
        &self,
        prepared: PreparedArtwork,
    ) -> Result<artwork::ArtworkProjection, String> {
        self.artwork
            .request_prepared(prepared)
            .map_err(|error| error.to_string())
    }

    pub fn cancel_artwork(&self, request_id: RequestId) {
        self.artwork.cancel(request_id);
    }

    pub fn allocate_artwork_prefetch_owner(&self) -> PrefetchOwner {
        self.artwork.allocate_prefetch_owner()
    }

    pub fn replace_artwork_prefetch(
        &self,
        owner: PrefetchOwner,
        priority: PrefetchPriority,
        requests: Vec<ArtworkRequest>,
    ) -> Result<(), String> {
        let source = self.active_artwork_source()?;
        self.artwork
            .replace_prefetch(owner, priority, source, requests);
        Ok(())
    }

    pub fn clear_artwork_prefetch(&self, owner: PrefetchOwner) {
        self.artwork.clear_prefetch(owner);
    }

    pub fn set_artwork_prefetch_paused(&self, priority: PrefetchPriority, paused: bool) {
        self.artwork.set_prefetch_paused(priority, paused);
    }

    pub fn cached_artwork_path(
        &self,
        source_id: &SourceId,
        binding: ArtworkBinding,
        fetch_size: u32,
        render_size: u32,
        external: ExternalPolicy,
    ) -> Option<PathBuf> {
        let request = artwork_request(binding, fetch_size, render_size, external);
        self.artwork.cache_only_file(source_id, &request)
    }

    pub fn retry_external_artwork(&self) -> Result<(), String> {
        self.artwork
            .retry_external()
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

pub(super) fn invalidate_source(
    artwork: &artwork::Artwork,
    source_id: &SourceId,
) -> Result<(), String> {
    artwork
        .invalidate_source(source_id)
        .map_err(|error| error.to_string())
}

fn artwork_request(
    binding: ArtworkBinding,
    fetch_size: u32,
    render_size: u32,
    external: ExternalPolicy,
) -> ArtworkRequest {
    ArtworkRequest::new(binding, fetch_size, render_size).with_external(external)
}
