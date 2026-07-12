use super::{
    ActiveSourceSlot, AppController, BoundedRunner, ControllerEvent, StoreHandle,
    WaveformProjection, encode_key_part, resolve_stream_request, waveform_cache_path_for_key,
};
use library::{SourceId, TrackId};
use playback::PreparedStream;
use playback_gstreamer::generate_waveform_peaks_cancellable;
use serde::{Deserialize, Serialize};
use sources::{StreamQuality, StreamRequest};
use std::{
    collections::HashSet,
    fs,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};
use tokio::runtime::Runtime;
use tracing::{debug, warn};

const WAVEFORM_CACHE_VERSION: u8 = 2;
const WAVEFORM_WARM_QUEUE_LIMIT: usize = 4;
const WAVEFORM_WARM_DELAY: Duration = Duration::from_millis(750);

static WAVEFORM_IN_FLIGHT: OnceLock<Mutex<HashSet<WaveformKey>>> = OnceLock::new();

#[derive(Debug, Deserialize, Serialize)]
struct CachedWaveform {
    version: u8,
    duration_seconds: u32,
    peaks: Vec<(f64, f64)>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct WaveformKey {
    source_id: SourceId,
    track_id: TrackId,
    duration_seconds: u32,
}

impl WaveformKey {
    fn new(source_id: SourceId, track_id: TrackId, duration_seconds: u32) -> Self {
        Self {
            source_id,
            track_id,
            duration_seconds,
        }
    }

    fn cache_key(&self) -> String {
        let track_hash = format!("{:x}", md5::compute(self.track_id.as_str()));
        format!(
            "{}/{}-{}.json",
            encode_key_part(self.source_id.as_str()),
            track_hash,
            self.duration_seconds
        )
    }
}

#[derive(Clone)]
struct WaveformRequest {
    key: WaveformKey,
    stream_request: StreamRequest,
    source_format: Option<String>,
}

struct WaveformGenerationRequest {
    waveform: WaveformRequest,
    stream: PreparedStream,
}

struct WaveformGenerationPermit {
    key: WaveformKey,
}

impl Drop for WaveformGenerationPermit {
    fn drop(&mut self) {
        if let Some(in_flight) = WAVEFORM_IN_FLIGHT.get()
            && let Ok(mut in_flight) = in_flight.lock()
        {
            in_flight.remove(&self.key);
        }
    }
}

struct WaveformWarmWorker {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    active_source: ActiveSourceSlot,
    generation: Arc<AtomicU64>,
    request_generation: u64,
    requested: Arc<Mutex<Option<WaveformKey>>>,
    events: Sender<ControllerEvent>,
}

impl AppController {
    pub fn request_waveform_for_current(&self) {
        let settings = self.load_settings();
        let request = settings.seekbar_waveform_enabled.then(|| {
            self.playback_product_if_present()
                .and_then(|product| product.current_entry())
                .map(|(source_id, entry, _position_millis)| {
                    waveform_request(&source_id, &entry.track, settings.playback.stream_quality)
                })
        });
        let Some(request) = request.flatten() else {
            clear_requested_waveform(&self.waveform_request_key, &self.events);
            self.cancel_waveform_warm();
            return;
        };

        let cached = select_requested_waveform(
            &self.waveform_request_key,
            &self.events,
            request.key.clone(),
        );
        self.cancel_waveform_warm();
        if cached {
            return;
        }
        let Some(permit) = acquire_waveform_generation_permit(&request.key) else {
            return;
        };

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let requested = Arc::clone(&self.waveform_request_key);
        let events = self.events.clone();
        if let Err(error) = waveform_runner().and_then(|runner| {
            runner.submit(move || {
                let _permit = permit;
                let Some(stream) =
                    waveform_source_for_request(&store, &runtime, &active_source, &request)
                else {
                    return;
                };
                let generation = WaveformGenerationRequest {
                    waveform: request,
                    stream,
                };
                if let Some(peaks) = generate_and_cache_waveform(&generation, || {
                    !requested_waveform_matches(&requested, &generation.waveform.key)
                }) {
                    publish_requested_waveform(
                        &requested,
                        &events,
                        &generation.waveform.key,
                        peaks,
                    );
                }
            })
        }) {
            warn!(%error, "could not schedule current waveform");
        }
    }

    pub fn warm_waveforms_for_queue(&self) {
        let generation = self.waveform_warm_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let settings = self.load_settings();
        if !settings.seekbar_waveform_enabled {
            return;
        }
        let Some((source_id, tracks)) = self
            .playback_product_if_present()
            .and_then(|product| product.upcoming_tracks(WAVEFORM_WARM_QUEUE_LIMIT))
        else {
            return;
        };
        let requests = tracks
            .into_iter()
            .filter(|track| track.duration_seconds > 0)
            .map(|track| waveform_request(&source_id, &track, settings.playback.stream_quality))
            .collect::<Vec<_>>();
        if requests.is_empty() {
            return;
        }
        start_waveform_warm_worker(
            requests,
            WaveformWarmWorker {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                active_source: Arc::clone(&self.active_source),
                generation: Arc::clone(&self.waveform_warm_generation),
                request_generation: generation,
                requested: Arc::clone(&self.waveform_request_key),
                events: self.events.clone(),
            },
        );
    }

    pub(in crate::controller) fn cancel_waveform_warm(&self) {
        self.waveform_warm_generation.fetch_add(1, Ordering::AcqRel);
    }
}

fn waveform_request(
    source_id: &SourceId,
    track: &library::Track,
    stream_quality: StreamQuality,
) -> WaveformRequest {
    WaveformRequest {
        key: WaveformKey::new(source_id.clone(), track.id.clone(), track.duration_seconds),
        stream_request: StreamRequest::new(track.id.clone(), stream_quality),
        source_format: track.source_format.clone(),
    }
}

fn start_waveform_warm_worker(requests: Vec<WaveformRequest>, worker: WaveformWarmWorker) {
    if let Err(error) = waveform_runner().and_then(|runner| {
        runner.submit(move || {
            for request in requests {
                if !waveform_warm_can_continue(
                    &worker.generation,
                    worker.request_generation,
                    &worker.requested,
                    &request.key,
                ) {
                    return;
                }
                std::thread::sleep(WAVEFORM_WARM_DELAY);
                if !waveform_warm_can_continue(
                    &worker.generation,
                    worker.request_generation,
                    &worker.requested,
                    &request.key,
                ) {
                    return;
                }
                warm_waveform_request(&worker, request);
            }
        })
    }) {
        warn!(%error, "could not schedule waveform warming");
    }
}

fn waveform_runner() -> Result<&'static BoundedRunner, String> {
    static RUNNER: OnceLock<Result<BoundedRunner, String>> = OnceLock::new();
    match RUNNER.get_or_init(|| BoundedRunner::new("Waveform generation", "rufin-waveform", 4)) {
        Ok(runner) => Ok(runner),
        Err(error) => Err(error.clone()),
    }
}

fn warm_waveform_request(worker: &WaveformWarmWorker, request: WaveformRequest) {
    let cache_key = request.key.cache_key();
    if load_cached_waveform(&cache_key, request.key.duration_seconds).is_some() {
        return;
    }
    let Some(_permit) = acquire_waveform_generation_permit(&request.key) else {
        return;
    };
    if load_cached_waveform(&cache_key, request.key.duration_seconds).is_some()
        || !waveform_warm_can_continue(
            &worker.generation,
            worker.request_generation,
            &worker.requested,
            &request.key,
        )
    {
        return;
    }
    let Some(stream) = waveform_source_for_request(
        &worker.store,
        &worker.runtime,
        &worker.active_source,
        &request,
    ) else {
        return;
    };
    let generation = WaveformGenerationRequest {
        waveform: request,
        stream,
    };
    if let Some(peaks) = generate_and_cache_waveform(&generation, || {
        !waveform_warm_can_continue(
            &worker.generation,
            worker.request_generation,
            &worker.requested,
            &generation.waveform.key,
        )
    }) {
        publish_requested_waveform(
            &worker.requested,
            &worker.events,
            &generation.waveform.key,
            peaks,
        );
    }
}

fn waveform_source_for_request(
    store: &StoreHandle,
    runtime: &Runtime,
    active_source: &ActiveSourceSlot,
    request: &WaveformRequest,
) -> Option<PreparedStream> {
    let stream = resolve_stream_request(
        store,
        runtime,
        active_source,
        &request.key.source_id,
        &request.stream_request,
    )
    .map_err(|error| {
        warn!(%error, track_id = %request.key.track_id, "failed to resolve stream for waveform");
        error
    })
    .ok()?;
    Some(PreparedStream::from(stream))
}

fn generate_and_cache_waveform(
    request: &WaveformGenerationRequest,
    cancelled: impl Fn() -> bool,
) -> Option<Vec<(f64, f64)>> {
    if cancelled() {
        return None;
    }
    if !waveform_generation_source_and_format_is_supported(
        request.stream.uri(),
        request.waveform.source_format.as_deref(),
    ) {
        debug!(
            track_id = %request.waveform.key.track_id,
            "skipped unsupported waveform generation source"
        );
        return None;
    }
    let peaks = match generate_waveform_peaks_cancellable(&request.stream, &cancelled) {
        Ok(peaks) => peaks,
        Err(error) => {
            if !cancelled() {
                warn!(
                    %error,
                    track_id = %request.waveform.key.track_id,
                    uri = %request.stream.redacted_uri(),
                    "failed to generate waveform"
                );
            }
            return None;
        }
    };
    let peaks = sanitize_waveform_peaks(peaks)?;
    if cancelled() {
        return None;
    }
    let cache_key = request.waveform.key.cache_key();
    if let Err(error) =
        save_cached_waveform(&cache_key, request.waveform.key.duration_seconds, &peaks)
    {
        warn!(%error, track_id = %request.waveform.key.track_id, "failed to cache waveform");
    }
    Some(peaks)
}

fn select_requested_waveform(
    requested: &Arc<Mutex<Option<WaveformKey>>>,
    events: &Sender<ControllerEvent>,
    key: WaveformKey,
) -> bool {
    let Ok(mut current) = requested.lock() else {
        return false;
    };
    *current = Some(key.clone());
    drop(current);
    let cache_key = key.cache_key();
    let peaks = load_cached_waveform(&cache_key, key.duration_seconds);
    let cached = peaks.is_some();
    publish_requested_projection(requested, events, &key, peaks);
    cached
}

fn clear_requested_waveform(
    requested: &Arc<Mutex<Option<WaveformKey>>>,
    events: &Sender<ControllerEvent>,
) {
    let Ok(mut current) = requested.lock() else {
        return;
    };
    *current = None;
    drop(current);
    if requested.lock().is_ok_and(|current| current.is_none()) {
        let _sent = events.send(ControllerEvent::Waveform(WaveformProjection::default()));
    }
}

fn publish_requested_waveform(
    requested: &Arc<Mutex<Option<WaveformKey>>>,
    events: &Sender<ControllerEvent>,
    key: &WaveformKey,
    peaks: Vec<(f64, f64)>,
) {
    publish_requested_projection(requested, events, key, Some(peaks));
}

fn publish_requested_projection(
    requested: &Arc<Mutex<Option<WaveformKey>>>,
    events: &Sender<ControllerEvent>,
    key: &WaveformKey,
    peaks: Option<Vec<(f64, f64)>>,
) {
    if !requested_waveform_matches(requested, key) {
        return;
    }
    let _sent = events.send(ControllerEvent::Waveform(WaveformProjection {
        key: Some(key.cache_key()),
        peaks: peaks.map(Arc::new),
    }));
}

fn acquire_waveform_generation_permit(key: &WaveformKey) -> Option<WaveformGenerationPermit> {
    let in_flight = WAVEFORM_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let mut in_flight = in_flight.lock().ok()?;
    if !in_flight.insert(key.clone()) {
        return None;
    }
    Some(WaveformGenerationPermit { key: key.clone() })
}

fn waveform_warm_can_continue(
    generation: &AtomicU64,
    request_generation: u64,
    requested: &Mutex<Option<WaveformKey>>,
    key: &WaveformKey,
) -> bool {
    generation.load(Ordering::Acquire) == request_generation
        || requested_waveform_matches(requested, key)
}

fn requested_waveform_matches(requested: &Mutex<Option<WaveformKey>>, key: &WaveformKey) -> bool {
    requested
        .lock()
        .is_ok_and(|requested| requested.as_ref() == Some(key))
}

fn waveform_generation_source_is_local(uri: &str) -> bool {
    uri.starts_with("file://")
}

fn waveform_generation_source_is_remote(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

fn waveform_generation_source_is_supported(uri: &str) -> bool {
    waveform_generation_source_is_local(uri) || waveform_generation_source_is_remote(uri)
}

fn waveform_generation_format_is_supported(source_format: Option<&str>) -> bool {
    !source_format.is_some_and(is_dsd_waveform_format)
}

fn waveform_generation_source_and_format_is_supported(
    uri: &str,
    source_format: Option<&str>,
) -> bool {
    waveform_generation_source_is_supported(uri)
        && waveform_generation_format_is_supported(source_format)
}

fn is_dsd_waveform_format(value: &str) -> bool {
    let trimmed = value.trim().trim_start_matches('.').to_ascii_lowercase();
    if trimmed.is_empty() {
        return false;
    }
    trimmed
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .any(|part| matches!(part, "dsf" | "dff" | "dsdiff") || part.starts_with("dsd"))
}

fn load_cached_waveform(cache_key: &str, duration_seconds: u32) -> Option<Vec<(f64, f64)>> {
    let path = waveform_cache_path_for_key(cache_key)?;
    let value = fs::read_to_string(path).ok()?;
    cached_waveform_peaks_from_str(&value, duration_seconds)
}

fn cached_waveform_peaks_from_str(value: &str, duration_seconds: u32) -> Option<Vec<(f64, f64)>> {
    let cached = serde_json::from_str::<CachedWaveform>(value).ok()?;
    if cached.version != WAVEFORM_CACHE_VERSION || cached.duration_seconds != duration_seconds {
        return None;
    }
    sanitize_waveform_peaks(cached.peaks)
}

fn save_cached_waveform(
    cache_key: &str,
    duration_seconds: u32,
    peaks: &[(f64, f64)],
) -> Result<(), String> {
    let Some(path) = waveform_cache_path_for_key(cache_key) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = serde_json::to_string(&CachedWaveform {
        version: WAVEFORM_CACHE_VERSION,
        duration_seconds,
        peaks: peaks.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    fs::write(path, value).map_err(|error| error.to_string())
}

fn sanitize_waveform_peaks(peaks: Vec<(f64, f64)>) -> Option<Vec<(f64, f64)>> {
    let peaks = peaks
        .into_iter()
        .filter(|(left, right)| left.is_finite() && right.is_finite())
        .map(|(left, right)| (left.clamp(0.0, 1.0), right.clamp(0.0, 1.0)))
        .collect::<Vec<_>>();
    (!peaks.is_empty()).then_some(peaks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{TryRecvError, channel};

    #[test]
    fn waveform_key_scopes_cache_by_source_track_and_duration() {
        let key = WaveformKey::new(
            SourceId::new("server/one"),
            TrackId::new("album/track:one"),
            123,
        )
        .cache_key();

        assert!(key.starts_with("server_one/"));
        assert!(key.ends_with("-123.json"));
        assert!(!key.contains("track:one"));
    }

    #[test]
    fn waveform_projection_requires_the_exact_requested_key() {
        let requested = Arc::new(Mutex::new(Some(WaveformKey::new(
            SourceId::new("source"),
            TrackId::new("wanted"),
            180,
        ))));
        let stale = WaveformKey::new(SourceId::new("source"), TrackId::new("stale"), 180);
        let wanted = requested
            .lock()
            .expect("request key")
            .clone()
            .expect("wanted key");
        let (events, receiver) = channel();

        publish_requested_waveform(&requested, &events, &stale, vec![(0.1, 0.2)]);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

        publish_requested_waveform(&requested, &events, &wanted, vec![(0.3, 0.4)]);
        let ControllerEvent::Waveform(projection) = receiver.recv().expect("waveform event") else {
            panic!("unexpected controller event");
        };
        assert_eq!(projection.key, Some(wanted.cache_key()));
        assert_eq!(projection.peaks.as_deref(), Some(&vec![(0.3, 0.4)]));
    }

    #[test]
    fn current_reuses_matching_warm_decode_and_cancels_other_warm_work() {
        let warm_key = WaveformKey::new(
            SourceId::new("warm-promotion"),
            TrackId::new("matching"),
            180,
        );
        let unrelated_key = WaveformKey::new(
            SourceId::new("warm-promotion"),
            TrackId::new("unrelated"),
            180,
        );
        let next_current_key = WaveformKey::new(
            SourceId::new("warm-promotion"),
            TrackId::new("next-current"),
            180,
        );
        let generation = AtomicU64::new(7);
        let warm_generation = generation.load(Ordering::Acquire);
        let requested = Arc::new(Mutex::new(None));
        let warm_permit =
            acquire_waveform_generation_permit(&warm_key).expect("matching warm decode");

        *requested.lock().expect("requested waveform") = Some(warm_key.clone());
        generation.fetch_add(1, Ordering::AcqRel);

        assert!(waveform_warm_can_continue(
            &generation,
            warm_generation,
            &requested,
            &warm_key,
        ));
        assert!(!waveform_warm_can_continue(
            &generation,
            warm_generation,
            &requested,
            &unrelated_key,
        ));
        assert!(acquire_waveform_generation_permit(&warm_key).is_none());

        let (events, receiver) = channel();
        publish_requested_waveform(&requested, &events, &warm_key, vec![(0.2, 0.8)]);
        let ControllerEvent::Waveform(projection) = receiver.recv().expect("waveform event") else {
            panic!("unexpected controller event");
        };
        assert_eq!(projection.key, Some(warm_key.cache_key()));
        assert_eq!(projection.peaks.as_deref(), Some(&vec![(0.2, 0.8)]));

        *requested.lock().expect("requested waveform") = Some(next_current_key.clone());
        generation.fetch_add(1, Ordering::AcqRel);
        assert!(!waveform_warm_can_continue(
            &generation,
            warm_generation,
            &requested,
            &warm_key,
        ));
        let current_permit = acquire_waveform_generation_permit(&next_current_key)
            .expect("new current starts immediately");

        drop(current_permit);
        drop(warm_permit);
    }

    #[test]
    fn waveform_generation_accepts_local_and_remote_but_rejects_dsd() {
        assert!(waveform_generation_source_and_format_is_supported(
            "file:///music/track.flac",
            Some("flac")
        ));
        assert!(waveform_generation_source_and_format_is_supported(
            "https://music.example/stream",
            None
        ));
        assert!(!waveform_generation_source_and_format_is_supported(
            "fake://music.example/stream",
            None
        ));
        assert!(!waveform_generation_source_and_format_is_supported(
            "file:///music/track.dsf",
            Some("audio/x-dsf")
        ));
        assert!(!waveform_generation_format_is_supported(Some("DSD64")));
        assert!(!waveform_generation_format_is_supported(Some(".dff")));
    }

    #[test]
    fn waveform_cache_codec_rejects_other_versions_and_durations() {
        let current = serde_json::to_string(&CachedWaveform {
            version: WAVEFORM_CACHE_VERSION,
            duration_seconds: 42,
            peaks: vec![(0.25, 0.75)],
        })
        .expect("cached waveform");
        let stale_duration = serde_json::to_string(&CachedWaveform {
            version: WAVEFORM_CACHE_VERSION,
            duration_seconds: 41,
            peaks: vec![(0.25, 0.75)],
        })
        .expect("cached waveform");
        let stale_version = serde_json::to_string(&CachedWaveform {
            version: WAVEFORM_CACHE_VERSION + 1,
            duration_seconds: 42,
            peaks: vec![(0.25, 0.75)],
        })
        .expect("cached waveform");

        assert_eq!(
            cached_waveform_peaks_from_str(&current, 42),
            Some(vec![(0.25, 0.75)])
        );
        assert_eq!(cached_waveform_peaks_from_str(&stale_duration, 42), None);
        assert_eq!(cached_waveform_peaks_from_str(&stale_version, 42), None);
    }

    #[test]
    fn waveform_peaks_are_finite_and_bounded() {
        let peaks = sanitize_waveform_peaks(vec![(0.5, 1.5), (f64::NAN, 0.2), (-1.0, 0.25)])
            .expect("peaks");

        assert_eq!(peaks, vec![(0.5, 1.0), (0.0, 0.25)]);
        assert_eq!(sanitize_waveform_peaks(Vec::new()), None);
    }
}
