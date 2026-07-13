use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gdk_pixbuf::{Colorspace, Pixbuf};
use library::{ImageRef, SourceId};
use sources::{ImageBytes, ImageProvider, SourceError, SourceResult};
use tempfile::TempDir;
use tokio::runtime::{Builder, Runtime};

use crate::{
    Artwork, ArtworkEvent, ArtworkRequest, CandidateSet, ExternalPolicy, Readiness, RequestId,
    SourceImages,
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
    started: bool,
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
        self.wait_for(|state| state.started);
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
        let deadline = Instant::now() + Duration::from_secs(2);
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
        state.started = true;
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
    let images = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let source_id = SourceId::new("source-one");
    let source = SourceImages::new(source_id.clone(), images.clone());
    let request = request("cover-one");
    let (artwork, events) =
        Artwork::new(temporary.path(), runtime).expect("artwork service starts");

    let first = artwork
        .request(source.clone(), request.clone())
        .expect("first request")
        .request_id;
    let second = artwork
        .request(source, request.clone())
        .expect("second request")
        .request_id;
    let ready = wait_for_ready(&events, &[first, second]);

    assert_eq!(ready, HashSet::from([first, second]));
    assert_eq!(images.calls.load(Ordering::Relaxed), 1);
    assert!(!artwork.has_pending_request(first));
    assert!(!artwork.has_pending_request(second));
    let cached = artwork
        .cache_only_file(&source_id, &request)
        .expect("cache-only file");
    assert!(cached.is_file());
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
    let candidates = CandidateSet::album_text("Artist", "Album");
    let (artwork, _events) = Artwork::new(temporary.path(), runtime()).expect("artwork service");
    let source = SourceImages::cache_only(source_id.clone());
    let base = ArtworkRequest::new(candidates.clone(), 96, 96)
        .with_external(ExternalPolicy::new(false, false, ""));
    let initial = artwork.binding_identity(&source, &base);

    let resized = ArtworkRequest::new(candidates.clone(), 256, 192)
        .with_external(ExternalPolicy::new(false, false, ""));
    let resized_identity = artwork.binding_identity(&source, &resized);
    assert_eq!(initial.visual, resized_identity.visual);
    assert_ne!(initial.request, resized_identity.request);

    let network = ArtworkRequest::new(candidates.clone(), 96, 96)
        .with_external(ExternalPolicy::new(false, true, "key"));
    let network_identity = artwork.binding_identity(&source, &network);
    assert_eq!(initial.visual, network_identity.visual);
    assert_ne!(initial.request, network_identity.request);

    let lastfm_only = ArtworkRequest::new(candidates.clone(), 96, 96)
        .with_external(ExternalPolicy::new(false, true, "key").with_musicbrainz(false));
    let lastfm_only_identity = artwork.binding_identity(&source, &lastfm_only);
    assert_eq!(network_identity.visual, lastfm_only_identity.visual);
    assert_ne!(network_identity.request, lastfm_only_identity.request);

    artwork.retry_external().expect("retry external artwork");
    let retried = artwork.binding_identity(&source, &network);
    assert_eq!(network_identity.visual, retried.visual);
    assert_ne!(network_identity.request, retried.request);

    let cached = ArtworkRequest::new(candidates, 96, 96)
        .with_external(ExternalPolicy::new(true, true, "key"));
    let cached_identity = artwork.binding_identity(&source, &cached);
    assert_ne!(retried.visual, cached_identity.visual);
    assert_ne!(retried.request, cached_identity.request);

    artwork
        .invalidate_source(&source_id)
        .expect("source invalidation");
    let invalidated = artwork.binding_identity(&source, &cached);
    assert_ne!(cached_identity.visual, invalidated.visual);
    assert_ne!(cached_identity.request, invalidated.request);

    let native = ArtworkRequest::new(
        CandidateSet::from_native(Some(&ImageRef::new("native-cover", None))),
        96,
        96,
    );
    let cache_only_native = artwork.binding_identity(&source, &native);
    let provider = Arc::new(StaticImages {
        calls: AtomicUsize::new(0),
        bytes: png_bytes(),
    });
    let fetchable_source = SourceImages::new(source_id, provider);
    let fetchable_native = artwork.binding_identity(&fetchable_source, &native);
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
    let candidates = CandidateSet::from_native(Some(&ImageRef::new("large-cover", None)));
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
        let cached = Pixbuf::from_file(path).expect("decode sized cache file");
        assert_eq!(cached.width().max(cached.height()), size as i32);
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
    let candidates = CandidateSet::album_text("Artist", "Album");
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

fn request(id: &str) -> ArtworkRequest {
    let image_ref = ImageRef::new(id, None);
    ArtworkRequest::new(CandidateSet::from_native(Some(&image_ref)), 96, 96)
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

fn png_bytes_at(width: i32, height: i32) -> Vec<u8> {
    let pixbuf = Pixbuf::new(Colorspace::Rgb, true, 8, width, height).expect("test pixbuf");
    pixbuf.fill(0x2f_81_f7_ff);
    pixbuf
        .save_to_bufferv("png", &[])
        .expect("encode test artwork")
}

fn wait_for_ready(events: &Receiver<ArtworkEvent>, wanted: &[RequestId]) -> HashSet<RequestId> {
    let mut ready = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while ready.len() < wanted.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "artwork requests did not finish");
        let event = events
            .recv_timeout(remaining)
            .expect("artwork result event");
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
        let event = events
            .recv_timeout(remaining)
            .expect("missing artwork event");
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
    while let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
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
