use std::path::PathBuf;

use artwork::{
    ArtworkBinding, ArtworkProjection, ArtworkRequest, ExternalPolicy, PrefetchOwner,
    PrefetchPriority, PreparedArtwork, RequestId,
};
use library::SourceId;
use ui::runtime::artwork::ArtworkPort;

use super::super::root::ArtworkCommands;

impl ArtworkPort for ArtworkCommands {
    fn prepare(
        &self,
        source_id: Option<&SourceId>,
        binding: ArtworkBinding,
        fetch_size: u32,
        render_size: u32,
        external: ExternalPolicy,
    ) -> Result<PreparedArtwork, String> {
        match source_id {
            Some(source_id) => Ok(self.prepare_playback_artwork(
                source_id,
                binding,
                fetch_size,
                render_size,
                external,
            )),
            None => self.prepare_artwork(binding, fetch_size, render_size, external),
        }
    }

    fn request(&self, prepared: PreparedArtwork) -> Result<ArtworkProjection, String> {
        self.request_artwork(prepared)
    }

    fn cancel(&self, request_id: RequestId) {
        self.cancel_artwork(request_id);
    }

    fn allocate_prefetch_owner(&self) -> PrefetchOwner {
        self.allocate_artwork_prefetch_owner()
    }

    fn replace_prefetch(
        &self,
        owner: PrefetchOwner,
        priority: PrefetchPriority,
        requests: Vec<ArtworkRequest>,
    ) -> Result<(), String> {
        self.replace_artwork_prefetch(owner, priority, requests)
    }

    fn clear_prefetch(&self, owner: PrefetchOwner) {
        self.clear_artwork_prefetch(owner);
    }

    fn set_prefetch_paused(&self, priority: PrefetchPriority, paused: bool) {
        self.set_artwork_prefetch_paused(priority, paused);
    }

    fn cached_path(
        &self,
        source_id: &SourceId,
        binding: ArtworkBinding,
        fetch_size: u32,
        render_size: u32,
        external: ExternalPolicy,
    ) -> Option<PathBuf> {
        self.cached_artwork_path(source_id, binding, fetch_size, render_size, external)
    }

    fn retry_external(&self) -> Result<(), String> {
        self.retry_external_artwork()
    }
}
