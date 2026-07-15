use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::{Receiver, TryRecvError};
use async_trait::async_trait;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use library::{AlbumArtwork, AlbumId, ImageRef, SourceId};
use sources::{ImageBytes, ImageProvider, SourceError, SourceResult};
use tempfile::TempDir;
use tokio::runtime::{Builder, Runtime};

use crate::{
    Artwork, ArtworkBinding, ArtworkEvent, ArtworkRequest, ExternalPolicy, PrefetchPriority,
    Readiness, RequestId, SourceImages,
};

struct StaticImages {
    calls: AtomicUsize,
    bytes: Vec<u8>,
}

#[async_trait(?Send)]
impl ImageProvider for StaticImages {
    async fn image_bytes(&self, _image_ref: &ImageRef, _size: u32) -> SourceResult<ImageBytes> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ImageBytes {
            bytes: self.bytes.clone(),
            content_type: Some("image/png".to_string()),
        })
    }
}

struct MissingImages {
    calls: AtomicUsize,
}

#[async_trait(?Send)]
impl ImageProvider for MissingImages {
    async fn image_bytes(&self, _image_ref: &ImageRef, _size: u32) -> SourceResult<ImageBytes> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(SourceError::NotFound)
    }
}

#[derive(Default)]
struct GateState {
    started: usize,
    released: bool,
    finished: bool,
}

#[derive(Default)]
struct BlockingImages {
    state: Mutex<GateState>,
    changed: Condvar,
}

impl BlockingImages {
    fn wait_started(&self) {
        self.wait_started_count(1);
    }

    fn wait_started_count(&self, count: usize) {
        self.wait_for(|state| state.started >= count);
    }

    fn started_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .started
    }

    fn release(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.released = true;
        self.changed.notify_all();
    }

    fn wait_finished(&self) {
        self.wait_for(|state| state.finished);
    }

    fn wait_for(&self, condition: impl Fn(&GateState) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !condition(&state) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "artwork worker did not reach gate");
            state = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

#[async_trait(?Send)]
impl ImageProvider for BlockingImages {
    async fn image_bytes(&self, _image_ref: &ImageRef, _size: u32) -> SourceResult<ImageBytes> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.started += 1;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.finished = true;
        self.changed.notify_all();
        Ok(ImageBytes {
            bytes: png_bytes(),
            content_type: Some("image/png".to_string()),
        })
    }
}

#[test]
fn duplicate_visible_requests_share_fetch_and_leave_no_terminal_projection() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let runtime = runtime();
    let images = Arc::new(BlockingImages::default());
    let source_id = SourceId::new("source-one");
    let source = SourceImages::new(source_id.clone(), images.clone());
    let request = request("cover-one");
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime).expect("artwork service starts");

    let first = artwork
        .request_prepared(artwork.prepare(source.clone(), request.clone()))
        .expect("first request")
        .request_id;
    images.wait_started();
    let second = artwork
        .request_prepared(artwork.prepare(source, request.clone()))
        .expect("second request")
        .request_id;
    images.release();
    let ready = wait_for_ready(&events, &[first, second]);

    assert_eq!(ready, HashSet::from([first, second]));
    assert_eq!(images.started_count(), 1);
    assert!(!artwork.has_pending_request(first));
    assert!(!artwork.has_pending_request(second));
    let cached = artwork
        .cache_only_file(&source_id, &request)
        .expect("cache-only file");
    assert!(cached.is_file());
}

#[test]
fn prepared_artwork_reports_only_shared_decoded_cache_hits() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let source_id = SourceId::new("source-prepared-ready");
    let source = SourceImages::new(source_id.clone(), images.clone());
    let request = request("prepared-ready-cover");
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let miss = artwork.prepare(source.clone(), request.clone());
    assert!(miss.ready.is_none());
    assert_eq!(images.calls.load(Ordering::Relaxed), 0);

    let request_id = artwork
        .request_prepared(miss)
        .expect("request prepared miss")
        .request_id;
    wait_for_ready(&events, &[request_id]);
    assert_eq!(images.calls.load(Ordering::Relaxed), 1);

    let hit = artwork.prepare(source, request.clone());
    assert!(hit.ready.is_some());
    assert_eq!(images.calls.load(Ordering::Relaxed), 1);

    let (cold_artwork, _cold_events) =
        Artwork::new(temporary.path(), runtime()).expect("cold artwork service starts");
    let filesystem_only = cold_artwork.prepare(SourceImages::cache_only(source_id), request);
    assert!(filesystem_only.ready.is_none());
}

#[test]
fn larger_decoded_cover_satisfies_a_smaller_request_without_another_fetch() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes_at(800, 800),
    });
    let source = SourceImages::new(
        SourceId::new("source-reusable-decoded-size"),
        images.clone(),
    );
    let candidates = ArtworkBinding::from_native(Some(&ImageRef::new("shared-cover", None)));
    let large = ArtworkRequest::new(candidates.clone(), 256, 256);
    let small = ArtworkRequest::new(candidates, 96, 96);
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let request_id = artwork
        .request_prepared(artwork.prepare(source.clone(), large))
        .expect("large artwork request")
        .request_id;
    wait_for_ready(&events, &[request_id]);

    let reused = artwork
        .prepare(source, small)
        .ready
        .expect("the decoded grid cover satisfies the row request");
    assert_eq!(reused.width(), 256);
    assert_eq!(reused.height(), 256);
    assert_eq!(images.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn prefetch_completion_populates_the_shared_decoded_cache_without_an_event() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let source = SourceImages::new(SourceId::new("source-prefetch-cache"), images.clone());
    let request = request("prefetch-cache-cover");
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");
    assert!(
        artwork
            .prepare(source.clone(), request.clone())
            .ready
            .is_none()
    );

    let owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Viewport,
        source.clone(),
        vec![request.clone()],
    );
    wait_for_prepared_ready(&artwork, &source, &request);

    assert_eq!(images.calls.load(Ordering::Relaxed), 1);
    assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn prefetch_coalesces_one_visual_target_to_its_largest_requested_size() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes_at(800, 800),
    });
    let source = SourceImages::new(
        SourceId::new("source-prefetch-coalesced-size"),
        images.clone(),
    );
    let candidates = ArtworkBinding::from_native(Some(&ImageRef::new("coalesced-cover", None)));
    let small = ArtworkRequest::new(candidates.clone(), 96, 48);
    let large = ArtworkRequest::new(candidates, 256, 256);
    let (artwork, _events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Background,
        source.clone(),
        vec![small.clone(), large.clone()],
    );
    wait_for_prepared_ready(&artwork, &source, &large);

    assert_eq!(images.calls.load(Ordering::Relaxed), 1);
    assert!(artwork.prepare(source, small).ready.is_some());
}

#[test]
fn demand_uses_the_worker_reserved_from_all_prefetch_lanes() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let blockers = Arc::new(BlockingImages::default());
    let background_source = SourceImages::new(
        SourceId::new("source-prefetch-priority-background"),
        blockers.clone(),
    );
    let target = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let target_source = SourceImages::new(
        SourceId::new("source-prefetch-priority-viewport"),
        target.clone(),
    );
    let target_request = request("viewport-target");
    let (artwork, _events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let background_owner = artwork.allocate_prefetch_owner();
    let background = (0..super::pipeline::WORKERS)
        .map(|index| request(&format!("background-priority-{index}")))
        .collect();
    artwork.replace_prefetch(
        background_owner,
        PrefetchPriority::Background,
        background_source,
        background,
    );
    blockers.wait_started_count(super::pipeline::WORKERS - 1);

    let viewport_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        viewport_owner,
        PrefetchPriority::Viewport,
        target_source.clone(),
        vec![target_request.clone()],
    );
    thread::sleep(Duration::from_millis(50));
    assert_eq!(target.calls.load(Ordering::Relaxed), 0);

    let request_id = artwork
        .request_prepared(artwork.prepare(target_source, target_request))
        .expect("demand promotes the queued viewport request")
        .request_id;
    wait_for_ready(&_events, &[request_id]);

    assert_eq!(target.calls.load(Ordering::Relaxed), 1);
    assert_eq!(blockers.started_count(), super::pipeline::WORKERS - 1);
    blockers.release();
    blockers.wait_finished();
}

#[test]
fn active_job_accounting_survives_prefetch_promotion_to_demand() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let promoted = Arc::new(BlockingImages::default());
    let promoted_source = SourceImages::new(
        SourceId::new("source-active-accounting-promoted"),
        promoted.clone(),
    );
    let promoted_request = request("active-accounting-promoted");
    let background = Arc::new(BlockingImages::default());
    let background_source = SourceImages::new(
        SourceId::new("source-active-accounting-background"),
        background.clone(),
    );
    let viewport = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let viewport_source = SourceImages::new(
        SourceId::new("source-active-accounting-viewport"),
        viewport.clone(),
    );
    let viewport_request = request("active-accounting-viewport");
    let demand = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let demand_source = SourceImages::new(
        SourceId::new("source-active-accounting-demand"),
        demand.clone(),
    );
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let promoted_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        promoted_owner,
        PrefetchPriority::Background,
        promoted_source.clone(),
        vec![promoted_request.clone()],
    );
    promoted.wait_started();
    artwork
        .request(promoted_source, promoted_request)
        .expect("active prefetch is promoted to demand");

    let background_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        background_owner,
        PrefetchPriority::Background,
        background_source,
        vec![
            request("active-accounting-background-1"),
            request("active-accounting-background-2"),
        ],
    );
    background.wait_started_count(2);

    let viewport_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        viewport_owner,
        PrefetchPriority::Viewport,
        viewport_source.clone(),
        vec![viewport_request.clone()],
    );
    thread::sleep(Duration::from_millis(50));
    assert_eq!(viewport.calls.load(Ordering::Relaxed), 0);

    let demand_id = artwork
        .request(demand_source, request("active-accounting-demand"))
        .expect("reserved worker accepts demand")
        .request_id;
    wait_for_ready(&events, &[demand_id]);
    assert_eq!(demand.calls.load(Ordering::Relaxed), 1);

    promoted.release();
    background.release();
    promoted.wait_finished();
    background.wait_finished();
    wait_for_prepared_ready(&artwork, &viewport_source, &viewport_request);
}

#[test]
fn idle_prefetch_uses_one_worker_and_yields_to_route_warm_work() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let idle = Arc::new(BlockingImages::default());
    let idle_source = SourceImages::new(SourceId::new("source-prefetch-idle"), idle.clone());
    let route = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let route_source = SourceImages::new(
        SourceId::new("source-prefetch-route-background"),
        route.clone(),
    );
    let route_request = request("route-background-target");
    let (artwork, _events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let idle_owner = artwork.allocate_prefetch_owner();
    let idle_batch = (0..super::pipeline::WORKERS)
        .map(|index| request(&format!("idle-prefetch-{index}")))
        .collect();
    artwork.replace_prefetch(idle_owner, PrefetchPriority::Idle, idle_source, idle_batch);
    idle.wait_started();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(idle.started_count(), 1);

    let route_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        route_owner,
        PrefetchPriority::Background,
        route_source.clone(),
        vec![route_request.clone()],
    );
    wait_for_prepared_ready(&artwork, &route_source, &route_request);

    assert_eq!(route.calls.load(Ordering::Relaxed), 1);
    assert_eq!(idle.started_count(), 1);
    artwork.clear_prefetch(idle_owner);
    idle.release();
    idle.wait_finished();
}

#[test]
fn retry_rekeys_queued_external_work_before_it_can_fetch_twice() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(BlockingImages::default());
    let source = SourceImages::new(
        SourceId::new("source-prefetch-external-retry"),
        images.clone(),
    );
    let candidates = ArtworkBinding::album_artwork(&AlbumArtwork {
        id: AlbumId::new("album-prefetch-external-retry"),
        title: "Album".to_string(),
        artist: "Artist".to_string(),
        image_ref: Some(ImageRef::new("native-before-external", None)),
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    });
    let request = ArtworkRequest::new(candidates, 96, 96);
    let (artwork, _events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    artwork.set_prefetch_paused(PrefetchPriority::Background, true);
    let owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Background,
        source.clone(),
        vec![request.clone()],
    );
    artwork.retry_external().expect("retry external artwork");
    artwork.set_prefetch_paused(PrefetchPriority::Background, false);

    images.wait_started();
    images.release();
    wait_for_prepared_ready(&artwork, &source, &request);
    assert_eq!(images.started_count(), 1);
}

#[test]
fn paused_prefetch_does_not_prevent_demand_from_starting() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let blockers = Arc::new(BlockingImages::default());
    let prefetch_source =
        SourceImages::new(SourceId::new("source-prefetch-paused"), blockers.clone());
    let target = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let target_source = SourceImages::new(
        SourceId::new("source-prefetch-paused-demand"),
        target.clone(),
    );
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    artwork.set_prefetch_paused(PrefetchPriority::Background, true);
    let owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Background,
        prefetch_source,
        vec![request("paused-prefetch")],
    );
    let request_id = artwork
        .request(target_source, request("demand-while-prefetch-paused"))
        .expect("demand request while prefetch is paused")
        .request_id;
    wait_for_ready(&events, &[request_id]);

    assert_eq!(target.calls.load(Ordering::Relaxed), 1);
    assert_eq!(blockers.started_count(), 0);
    artwork.set_prefetch_paused(PrefetchPriority::Background, false);
    blockers.wait_started();
    blockers.release();
    blockers.wait_finished();
}

#[test]
fn demand_promotes_and_reuses_a_matching_queued_prefetch_job() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let blockers = Arc::new(BlockingImages::default());
    let blocking_source = SourceImages::new(
        SourceId::new("source-prefetch-promotion-blockers"),
        blockers.clone(),
    );
    let target = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let target_source = SourceImages::new(
        SourceId::new("source-prefetch-promotion-target"),
        target.clone(),
    );
    let target_request = request("promotion-target");
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let blocker_owner = artwork.allocate_prefetch_owner();
    let background = (0..super::pipeline::WORKERS)
        .map(|index| request(&format!("promotion-blocker-{index}")))
        .collect();
    artwork.replace_prefetch(
        blocker_owner,
        PrefetchPriority::Background,
        blocking_source,
        background,
    );
    blockers.wait_started_count(super::pipeline::WORKERS - 1);

    let target_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        target_owner,
        PrefetchPriority::Background,
        target_source.clone(),
        vec![target_request.clone()],
    );
    let request_id = artwork
        .request_prepared(artwork.prepare(target_source, target_request))
        .expect("promoted demand request")
        .request_id;
    wait_for_ready(&events, &[request_id]);

    assert_eq!(target.calls.load(Ordering::Relaxed), 1);
    assert_eq!(blockers.started_count(), super::pipeline::WORKERS - 1);
    blockers.release();
    blockers.wait_finished();
}

#[test]
fn replacing_and_clearing_an_owner_drops_obsolete_queued_prefetch() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let blockers = Arc::new(BlockingImages::default());
    let blocking_source = SourceImages::new(
        SourceId::new("source-prefetch-replace-blockers"),
        blockers.clone(),
    );
    let obsolete = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let obsolete_source = SourceImages::new(
        SourceId::new("source-prefetch-replace-obsolete"),
        obsolete.clone(),
    );
    let replacement = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let replacement_source = SourceImages::new(
        SourceId::new("source-prefetch-replace-cleared"),
        replacement.clone(),
    );
    let (artwork, _events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let blocker_owner = artwork.allocate_prefetch_owner();
    let background = (0..super::pipeline::WORKERS - 1)
        .map(|index| request(&format!("replace-blocker-{index}")))
        .collect();
    artwork.replace_prefetch(
        blocker_owner,
        PrefetchPriority::Background,
        blocking_source,
        background,
    );
    blockers.wait_started_count(super::pipeline::WORKERS - 1);

    let owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Background,
        obsolete_source,
        vec![request("obsolete-prefetch")],
    );
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Background,
        replacement_source,
        vec![request("cleared-prefetch")],
    );
    artwork.clear_prefetch(owner);
    blockers.release();
    blockers.wait_finished();
    thread::sleep(Duration::from_millis(100));

    assert_eq!(obsolete.calls.load(Ordering::Relaxed), 0);
    assert_eq!(replacement.calls.load(Ordering::Relaxed), 0);
}

#[test]
fn clearing_viewport_ownership_keeps_a_shared_background_prefetch_queued() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let target = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let target_source = SourceImages::new(
        SourceId::new("source-prefetch-shared-target"),
        target.clone(),
    );
    let target_request = request("shared-priority-target");
    let sentinel = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let sentinel_source = SourceImages::new(
        SourceId::new("source-prefetch-viewport-sentinel"),
        sentinel.clone(),
    );
    let sentinel_request = request("viewport-sentinel");
    let (artwork, _events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    artwork.set_prefetch_paused(PrefetchPriority::Viewport, true);
    artwork.set_prefetch_paused(PrefetchPriority::Background, true);
    let background_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        background_owner,
        PrefetchPriority::Background,
        target_source.clone(),
        vec![target_request.clone()],
    );
    let shared_viewport_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        shared_viewport_owner,
        PrefetchPriority::Viewport,
        target_source.clone(),
        vec![target_request.clone()],
    );
    let sentinel_owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        sentinel_owner,
        PrefetchPriority::Viewport,
        sentinel_source.clone(),
        vec![sentinel_request.clone()],
    );

    artwork.clear_prefetch(shared_viewport_owner);
    artwork.set_prefetch_paused(PrefetchPriority::Viewport, false);
    wait_for_prepared_ready(&artwork, &sentinel_source, &sentinel_request);
    assert_eq!(sentinel.calls.load(Ordering::Relaxed), 1);
    assert_eq!(target.calls.load(Ordering::Relaxed), 0);

    artwork.set_prefetch_paused(PrefetchPriority::Background, false);
    wait_for_prepared_ready(&artwork, &target_source, &target_request);
    assert_eq!(target.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn clearing_an_active_prefetch_keeps_its_completed_decode_reusable() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(BlockingImages::default());
    let source = SourceImages::new(
        SourceId::new("source-prefetch-clear-active"),
        images.clone(),
    );
    let request = request("clear-active-target");
    let (artwork, _events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let owner = artwork.allocate_prefetch_owner();
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Viewport,
        source.clone(),
        vec![request.clone()],
    );
    images.wait_started();
    artwork.clear_prefetch(owner);
    images.release();
    images.wait_finished();

    wait_for_prepared_ready(&artwork, &source, &request);
    assert_eq!(images.started_count(), 1);
}

#[test]
fn a_full_background_batch_cannot_block_new_demand() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let blockers = Arc::new(BlockingImages::default());
    let background_source = SourceImages::new(
        SourceId::new("source-prefetch-bounds-background"),
        blockers.clone(),
    );
    let target = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let target_source = SourceImages::new(
        SourceId::new("source-prefetch-bounds-demand"),
        target.clone(),
    );
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let owner = artwork.allocate_prefetch_owner();
    let background_requests = (0..320)
        .map(|index| request(&format!("bounded-background-{index}")))
        .collect::<Vec<_>>();
    artwork.replace_prefetch(
        owner,
        PrefetchPriority::Background,
        background_source.clone(),
        background_requests.clone(),
    );
    blockers.wait_started_count(super::pipeline::WORKERS - 1);

    let request_id = artwork
        .request(target_source, request("bounded-demand"))
        .expect("demand displaces queued background prefetch")
        .request_id;
    wait_for_ready(&events, &[request_id]);

    assert_eq!(target.calls.load(Ordering::Relaxed), 1);
    assert_eq!(blockers.started_count(), super::pipeline::WORKERS - 1);
    blockers.release();
    blockers.wait_started_count(320);
    wait_for_all_prepared_ready(&artwork, &background_source, &background_requests);
    assert_eq!(blockers.started_count(), 320);
}

#[test]
fn cancel_drops_a_pending_binding_before_its_fetch_finishes() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(BlockingImages::default());
    let source = SourceImages::new(SourceId::new("source-cancel"), images.clone());
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");
    let request_id = artwork
        .request(source, request("slow-cover"))
        .expect("slow request")
        .request_id;

    images.wait_started();
    artwork.cancel(request_id);
    images.release();
    images.wait_finished();

    assert!(!artwork.has_pending_request(request_id));
    assert_no_terminal_event(&events, request_id);
}

#[test]
fn source_invalidation_rejects_an_in_flight_result_and_removes_its_file() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(BlockingImages::default());
    let source_id = SourceId::new("source-stale");
    let source = SourceImages::new(source_id.clone(), images.clone());
    let request = request("stale-cover");
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");
    let request_id = artwork
        .request(source, request.clone())
        .expect("stale request")
        .request_id;

    images.wait_started();
    artwork
        .invalidate_source(&source_id)
        .expect("source invalidation");
    images.release();
    images.wait_finished();

    assert!(!artwork.has_pending_request(request_id));
    assert!(artwork.cache_only_file(&source_id, &request).is_none());
    assert_no_terminal_event(&events, request_id);
}

#[test]
fn binding_identity_separates_visual_changes_from_rerequest_changes() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let source_id = SourceId::new("source-identity");
    let candidates = ArtworkBinding::album_text("Artist", "Album");
    let (artwork, _events) = Artwork::new(temporary.path(), runtime()).expect("artwork service");
    let source = SourceImages::cache_only(source_id.clone());
    let base = ArtworkRequest::new(candidates.clone(), 96, 96)
        .with_external(ExternalPolicy::new(false, false, ""));
    let initial = artwork.prepare(source.clone(), base).identity;

    let resized = ArtworkRequest::new(candidates.clone(), 256, 192)
        .with_external(ExternalPolicy::new(false, false, ""));
    let resized_identity = artwork.prepare(source.clone(), resized).identity;
    assert_eq!(initial.visual, resized_identity.visual);
    assert_ne!(initial.request, resized_identity.request);

    let network = ArtworkRequest::new(candidates.clone(), 96, 96)
        .with_external(ExternalPolicy::new(false, true, "key"));
    let network_identity = artwork.prepare(source.clone(), network.clone()).identity;
    assert_eq!(initial.visual, network_identity.visual);
    assert_ne!(initial.request, network_identity.request);

    let lastfm_only = ArtworkRequest::new(candidates.clone(), 96, 96)
        .with_external(ExternalPolicy::new(false, true, "key").with_musicbrainz(false));
    let lastfm_only_identity = artwork.prepare(source.clone(), lastfm_only).identity;
    assert_eq!(network_identity.visual, lastfm_only_identity.visual);
    assert_ne!(network_identity.request, lastfm_only_identity.request);

    artwork.retry_external().expect("retry external artwork");
    let retried = artwork.prepare(source.clone(), network).identity;
    assert_eq!(network_identity.visual, retried.visual);
    assert_ne!(network_identity.request, retried.request);

    let cached = ArtworkRequest::new(candidates, 96, 96)
        .with_external(ExternalPolicy::new(true, true, "key"));
    let cached_identity = artwork.prepare(source.clone(), cached.clone()).identity;
    assert_ne!(retried.visual, cached_identity.visual);
    assert_ne!(retried.request, cached_identity.request);

    artwork
        .invalidate_source(&source_id)
        .expect("source invalidation");
    let invalidated = artwork.prepare(source.clone(), cached).identity;
    assert_ne!(cached_identity.visual, invalidated.visual);
    assert_ne!(cached_identity.request, invalidated.request);

    let native = ArtworkRequest::new(
        ArtworkBinding::from_native(Some(&ImageRef::new("native-cover", None))),
        96,
        96,
    );
    let cache_only_native = artwork.prepare(source, native.clone()).identity;
    let provider = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let fetchable_source = SourceImages::new(source_id, provider);
    let fetchable_native = artwork.prepare(fetchable_source, native).identity;
    assert_eq!(cache_only_native.visual, fetchable_native.visual);
    assert_ne!(cache_only_native.request, fetchable_native.request);
}

#[test]
fn provider_images_are_cached_at_each_requested_size() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let runtime = runtime();
    let images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes_at(800, 600),
    });
    let source_id = SourceId::new("source-sized-cache");
    let source = SourceImages::new(source_id.clone(), images.clone());
    let candidates = ArtworkBinding::from_native(Some(&ImageRef::new("large-cover", None)));
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime).expect("artwork service starts");

    for size in [96, 256, 512] {
        let request = ArtworkRequest::new(candidates.clone(), size, size);
        let request_id = artwork
            .request(source.clone(), request.clone())
            .expect("sized request")
            .request_id;
        wait_for_ready(&events, &[request_id]);
        let path = artwork
            .cache_only_file(&source_id, &request)
            .expect("sized cache file");
        let bytes = fs::read(path).expect("read sized cache file");
        let cached = crate::decode_rgba(&bytes, u32::MAX).expect("decode sized cache file");
        assert_eq!(cached.width().max(cached.height()), size);
    }

    assert_eq!(images.calls.load(Ordering::Relaxed), 3);
}

#[test]
fn cache_only_sources_are_scoped_and_do_not_create_native_misses() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let source_id = SourceId::new("source-cached");
    let request = request("shared-cover");
    let seeded_images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let (seeder, seed_events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");
    let seeded = seeder
        .request(
            SourceImages::new(source_id.clone(), seeded_images.clone()),
            request.clone(),
        )
        .expect("seed source artwork")
        .request_id;
    wait_for_ready(&seed_events, &[seeded]);
    assert_eq!(seeded_images.calls.load(Ordering::Relaxed), 1);

    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("fresh artwork service starts");
    let cached = artwork
        .request(SourceImages::cache_only(source_id), request.clone())
        .expect("request matching cached source")
        .request_id;
    wait_for_ready(&events, &[cached]);

    let other_source_id = SourceId::new("source-other");
    let uncached = artwork
        .request(
            SourceImages::cache_only(other_source_id.clone()),
            request.clone(),
        )
        .expect("request other cached source")
        .request_id;
    wait_for_missing(&events, uncached);

    let other_images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let fetched = artwork
        .request(
            SourceImages::new(other_source_id, other_images.clone()),
            request,
        )
        .expect("fetch after cache-only miss")
        .request_id;
    wait_for_ready(&events, &[fetched]);
    assert_eq!(other_images.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn cancelled_queue_entries_do_not_strand_later_artwork() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let blockers = Arc::new(BlockingImages::default());
    let blocking_source = SourceImages::new(
        SourceId::new("source-cancelled-queue-blockers"),
        blockers.clone(),
    );
    let images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let source = SourceImages::new(SourceId::new("source-cancelled-queue"), images.clone());
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    for index in 0..super::pipeline::WORKERS {
        artwork
            .request(
                blocking_source.clone(),
                request(&format!("blocking-cover-{index}")),
            )
            .expect("blocking artwork request");
    }
    blockers.wait_started_count(super::pipeline::WORKERS);

    let mut request_ids = Vec::new();
    for index in 0..=super::pipeline::WORKERS {
        request_ids.push(
            artwork
                .request(source.clone(), request(&format!("queue-cover-{index}")))
                .expect("queued artwork request")
                .request_id,
        );
    }
    for request_id in request_ids.iter().take(super::pipeline::WORKERS) {
        artwork.cancel(*request_id);
    }
    blockers.release();

    let last = *request_ids.last().expect("last queued request");
    wait_for_ready(&events, &[last]);
    assert_eq!(images.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn terminal_missing_is_cached_until_the_source_is_invalidated() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let images = Arc::new(MissingImages {
        calls: AtomicUsize::new(0),
    });
    let source_id = SourceId::new("source-missing");
    let source = SourceImages::new(source_id.clone(), images.clone());
    let request = request("absent-cover");
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");

    let first = artwork
        .request(source.clone(), request.clone())
        .expect("first missing request")
        .request_id;
    wait_for_missing(&events, first);
    let second = artwork
        .request(source.clone(), request.clone())
        .expect("cached missing request")
        .request_id;
    wait_for_missing(&events, second);
    assert_eq!(images.calls.load(Ordering::Relaxed), 1);

    artwork
        .invalidate_source(&source_id)
        .expect("source invalidation");
    let third = artwork
        .request(source, request)
        .expect("request after invalidation")
        .request_id;
    wait_for_missing(&events, third);
    assert_eq!(images.calls.load(Ordering::Relaxed), 2);
}

#[test]
fn disabled_external_policy_does_not_reuse_decoded_external_art() {
    let temporary = TempDir::new().expect("temporary artwork directory");
    let source_id = SourceId::new("source-external-policy");
    let candidates = ArtworkBinding::album_text("Artist", "Album");
    let candidate = candidates.candidates().first().expect("album candidate");
    let layout = crate::cache::current_layout(temporary.path()).expect("cache layout");
    crate::cache::FilesystemCache::new(layout)
        .expect("filesystem cache")
        .write_ready(&source_id, candidate, 96, &png_bytes())
        .expect("seed external artwork");
    let images = Arc::new(MissingImages {
        calls: AtomicUsize::new(0),
    });
    let source = SourceImages::new(source_id, images.clone());
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime()).expect("artwork service starts");
    let allowed = ArtworkRequest::new(candidates.clone(), 96, 96)
        .with_external(ExternalPolicy::new(true, false, ""));

    let ready = artwork
        .request(source.clone(), allowed)
        .expect("cached external request")
        .request_id;
    wait_for_ready(&events, &[ready]);

    let denied = artwork
        .request(source, ArtworkRequest::new(candidates, 96, 96))
        .expect("disabled external request")
        .request_id;
    wait_for_missing(&events, denied);

    assert_eq!(images.calls.load(Ordering::Relaxed), 0);
}

#[test]
fn product_artwork_formats_normalize_to_rgba_png() {
    for format in [
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Jpeg,
        ImageFormat::Png,
        ImageFormat::Tiff,
        ImageFormat::WebP,
    ] {
        let normalized = crate::decode::normalize_for_cache(encoded_image(7, 5, format), 64)
            .expect("supported artwork format");
        let decoded = crate::decode_rgba(normalized.bytes(), 64).expect("normalized artwork");

        assert_eq!((decoded.width(), decoded.height()), (7, 5));
        assert_eq!(decoded.row_stride(), 7 * 4);
        assert_eq!(decoded.rgba().len(), 7 * 5 * 4);
    }
}

#[test]
fn normalization_applies_embedded_orientation_before_scaling() {
    let mut jpeg = encoded_image(2, 3, ImageFormat::Jpeg);
    jpeg.splice(2..2, exif_rotate_90());

    let normalized =
        crate::decode::normalize_for_cache(jpeg, 64).expect("oriented JPEG normalization");
    let decoded = crate::decode_rgba(normalized.bytes(), 64).expect("oriented cached artwork");

    assert_eq!((decoded.width(), decoded.height()), (3, 2));
}

#[test]
fn decoded_pixels_keep_straight_rgba_channel_order() {
    let image = RgbaImage::from_pixel(1, 1, Rgba([0x2f, 0x81, 0xf7, 0x42]));
    let bytes = write_image(DynamicImage::ImageRgba8(image), ImageFormat::Png);

    let decoded = crate::decode_rgba(&bytes, 1).expect("RGBA artwork");

    assert_eq!(decoded.rgba(), &[0x2f, 0x81, 0xf7, 0x42]);
}

fn request(id: &str) -> ArtworkRequest {
    let image_ref = ImageRef::new(id, None);
    ArtworkRequest::new(ArtworkBinding::from_native(Some(&image_ref)), 96, 96)
}

fn runtime() -> Arc<Runtime> {
    Arc::new(
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test Tokio runtime"),
    )
}

fn png_bytes() -> Vec<u8> {
    png_bytes_at(2, 2)
}

fn png_bytes_at(width: u32, height: u32) -> Vec<u8> {
    encoded_image(width, height, ImageFormat::Png)
}

fn encoded_image(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, Rgba([0x2f, 0x81, 0xf7, 0xff]));
    let image = DynamicImage::ImageRgba8(image);
    let image = if format == ImageFormat::Jpeg {
        DynamicImage::ImageRgb8(image.into_rgb8())
    } else {
        image
    };
    write_image(image, format)
}

fn write_image(image: DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, format)
        .expect("encode test artwork");
    bytes.into_inner()
}

fn exif_rotate_90() -> [u8; 36] {
    [
        0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0x00, 0x00, b'M', b'M', 0x00, 0x2a, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x01, 0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x06,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]
}

fn wait_for_prepared_ready(artwork: &Artwork, source: &SourceImages, request: &ArtworkRequest) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if artwork
            .prepare(source.clone(), request.clone())
            .ready
            .is_some()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "prefetched artwork did not enter the decoded cache"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_all_prepared_ready(
    artwork: &Artwork,
    source: &SourceImages,
    requests: &[ArtworkRequest],
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if requests.iter().all(|request| {
            artwork
                .prepare(source.clone(), request.clone())
                .ready
                .is_some()
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the retained prefetch batch did not finish"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_ready(events: &Receiver<ArtworkEvent>, wanted: &[RequestId]) -> HashSet<RequestId> {
    let mut ready = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while ready.len() < wanted.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "artwork requests did not finish");
        let event = recv_with_timeout(events, remaining).expect("artwork result event");
        if let ArtworkEvent::Changed(projection) = event
            && wanted.contains(&projection.request_id)
            && matches!(projection.readiness, Readiness::Ready(_))
        {
            ready.insert(projection.request_id);
        }
    }
    ready
}

fn wait_for_missing(events: &Receiver<ArtworkEvent>, wanted: RequestId) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "missing artwork request did not finish"
        );
        let event = recv_with_timeout(events, remaining).expect("missing artwork event");
        if let ArtworkEvent::Changed(projection) = event
            && projection.request_id == wanted
            && matches!(projection.readiness, Readiness::Missing)
        {
            return;
        }
    }
}

fn assert_no_terminal_event(events: &Receiver<ArtworkEvent>, request_id: RequestId) {
    let deadline = Instant::now() + Duration::from_millis(150);
    while let Ok(event) =
        recv_with_timeout(events, deadline.saturating_duration_since(Instant::now()))
    {
        if let ArtworkEvent::Changed(projection) = event
            && projection.request_id == request_id
            && !matches!(projection.readiness, Readiness::Pending)
        {
            panic!("cancelled or invalidated request published a terminal event");
        }
        if Instant::now() >= deadline {
            break;
        }
    }
}

fn recv_with_timeout<T>(receiver: &Receiver<T>, timeout: Duration) -> Result<T, TryRecvError> {
    let deadline = Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1));
            }
            result => return result,
        }
    }
}
