use artwork::{
    ArtworkBinding, ArtworkProjection, ArtworkRequest, ExternalPolicy, PrefetchOwner,
    PrefetchPriority, PreparedArtwork, RequestId,
};
use library::SourceId;
use std::path::PathBuf;
use std::sync::Arc;

pub trait ArtworkPort: Send + Sync {
    fn prepare(
        &self,
        source_id: Option<&SourceId>,
        binding: ArtworkBinding,
        fetch_size: u32,
        render_size: u32,
        external: ExternalPolicy,
    ) -> Result<PreparedArtwork, String>;
    fn request(&self, prepared: PreparedArtwork) -> Result<ArtworkProjection, String>;
    fn cancel(&self, request_id: RequestId);
    fn allocate_prefetch_owner(&self) -> PrefetchOwner;
    fn replace_prefetch(
        &self,
        owner: PrefetchOwner,
        priority: PrefetchPriority,
        requests: Vec<ArtworkRequest>,
    ) -> Result<(), String>;
    fn clear_prefetch(&self, owner: PrefetchOwner);
    fn set_prefetch_paused(&self, priority: PrefetchPriority, paused: bool);
    fn cached_path(
        &self,
        source_id: &SourceId,
        binding: ArtworkBinding,
        fetch_size: u32,
        render_size: u32,
        external: ExternalPolicy,
    ) -> Option<PathBuf>;
    fn retry_external(&self) -> Result<(), String>;
}

pub type ArtworkHandle = Arc<dyn ArtworkPort>;
