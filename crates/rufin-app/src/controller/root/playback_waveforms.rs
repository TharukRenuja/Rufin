use super::*;

#[derive(Debug, Deserialize, Serialize)]
struct CachedWaveform {
    version: u8,
    duration_seconds: u32,
    peaks: Vec<(f64, f64)>,
}

const WAVEFORM_CACHE_VERSION: u8 = 1;
const WAVEFORM_WARM_QUEUE_LIMIT: usize = 4;
const WAVEFORM_WARM_DELAY: std::time::Duration = std::time::Duration::from_millis(750);
const CURRENT_WAVEFORM_GENERATION_DELAY: std::time::Duration =
    std::time::Duration::from_millis(750);
static WAVEFORM_FOREGROUND_IN_FLIGHT: std::sync::OnceLock<Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();
static WAVEFORM_WARM_IN_FLIGHT: std::sync::OnceLock<Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();

struct WaveformGenerationPermit {
    cache_key: String,
    kind: WaveformGenerationKind,
}

impl Drop for WaveformGenerationPermit {
    fn drop(&mut self) {
        if let Some(in_flight) = self.kind.in_flight().get()
            && let Ok(mut in_flight) = in_flight.lock()
        {
            in_flight.remove(&self.cache_key);
        }
    }
}

#[derive(Clone, Copy)]
enum WaveformGenerationKind {
    Foreground,
    Warm,
}

impl WaveformGenerationKind {
    fn in_flight(self) -> &'static std::sync::OnceLock<Mutex<HashSet<String>>> {
        match self {
            Self::Foreground => &WAVEFORM_FOREGROUND_IN_FLIGHT,
            Self::Warm => &WAVEFORM_WARM_IN_FLIGHT,
        }
    }
}

#[derive(Clone)]
struct WaveformWarmRequest {
    server_id: ServerId,
    track_id: TrackId,
    duration_seconds: u32,
    source_format: Option<String>,
    playback_settings: PlaybackSettings,
}

struct WaveformGenerationRequest {
    cache_key: String,
    track_id: TrackId,
    duration_seconds: u32,
    source_format: Option<String>,
    uri: String,
    redacted_uri: String,
}

struct WaveformWarmWorker {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    generation: Arc<AtomicU64>,
    request_generation: u64,
    events: Sender<ControllerEvent>,
}

impl AppController {
    pub fn request_waveform_for_current(&self) {
        if !self.load_settings().seekbar_waveform_enabled {
            return;
        }
        self.cancel_waveform_warm();
        let Some((server_id, entry, _position)) = self.current_playback_entry() else {
            return;
        };
        let playback_settings = self.load_settings().playback;
        let cache_key = waveform_cache_key(&server_id, &entry.track_id, entry.duration_seconds);
        self.update_playback_snapshot(|snapshot| {
            set_waveform_cache_key(snapshot, Some(cache_key.clone()));
        });

        if publish_cached_waveform(
            &self.playback_snapshot,
            &self.events,
            &cache_key,
            entry.duration_seconds,
        ) {
            return;
        }
        let Some(permit) =
            acquire_waveform_generation_permit(&cache_key, WaveformGenerationKind::Foreground)
        else {
            return;
        };

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let events = self.events.clone();
        thread::spawn(move || {
            let _permit = permit;
            let request = WaveformWarmRequest {
                server_id,
                track_id: entry.track_id,
                duration_seconds: entry.duration_seconds,
                source_format: entry.source_format,
                playback_settings,
            };
            let Some((uri, redacted_uri)) =
                waveform_source_for_track(&store, &runtime, &secrets, &request)
            else {
                return;
            };
            generate_and_publish_waveform(
                playback_snapshot,
                events,
                WaveformGenerationRequest {
                    cache_key,
                    track_id: request.track_id,
                    duration_seconds: request.duration_seconds,
                    source_format: request.source_format,
                    uri,
                    redacted_uri,
                },
            );
        });
    }

    pub fn warm_waveforms_for_queue(&self) {
        let settings = self.load_settings();
        if !settings.seekbar_waveform_enabled {
            return;
        }
        let Some(queue_snapshot) = self.queue_snapshot() else {
            return;
        };
        let requests = waveform_warm_requests(
            &queue_snapshot,
            &settings.playback,
            WAVEFORM_WARM_QUEUE_LIMIT,
        );
        if requests.is_empty() {
            return;
        }
        let generation = self.waveform_warm_generation.fetch_add(1, Ordering::AcqRel) + 1;
        start_waveform_warm_worker(
            requests,
            WaveformWarmWorker {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                playback_snapshot: Arc::clone(&self.playback_snapshot),
                generation: Arc::clone(&self.waveform_warm_generation),
                request_generation: generation,
                events: self.events.clone(),
            },
        );
    }

    pub(in crate::controller) fn cancel_waveform_warm(&self) {
        self.waveform_warm_generation.fetch_add(1, Ordering::AcqRel);
    }
}

pub(in crate::controller) fn request_waveform_for_prepared_item(
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    events: Sender<ControllerEvent>,
    server_id: ServerId,
    entry: QueueEntry,
    item: PreparedPlaybackItem,
) {
    let cache_key = waveform_cache_key(&server_id, &entry.track_id, entry.duration_seconds);
    if publish_cached_waveform(
        &playback_snapshot,
        &events,
        &cache_key,
        entry.duration_seconds,
    ) {
        return;
    }
    let Some(permit) =
        acquire_waveform_generation_permit(&cache_key, WaveformGenerationKind::Foreground)
    else {
        return;
    };
    thread::spawn(move || {
        let _permit = permit;
        thread::sleep(CURRENT_WAVEFORM_GENERATION_DELAY);
        generate_and_publish_waveform(
            playback_snapshot,
            events,
            WaveformGenerationRequest {
                cache_key,
                track_id: entry.track_id,
                duration_seconds: entry.duration_seconds,
                source_format: entry.source_format,
                uri: item.stream.uri().to_string(),
                redacted_uri: item.stream.redacted_uri().to_string(),
            },
        );
    });
}

fn generate_and_publish_waveform(
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    events: Sender<ControllerEvent>,
    request: WaveformGenerationRequest,
) {
    if publish_cached_waveform(
        &playback_snapshot,
        &events,
        &request.cache_key,
        request.duration_seconds,
    ) {
        return;
    }
    if !waveform_generation_source_and_format_is_supported(
        &request.uri,
        request.source_format.as_deref(),
    ) {
        debug!(
            track_id = %request.track_id,
            "skipped unsupported waveform generation source"
        );
        return;
    }
    let peaks = match generate_waveform_peaks(&request.uri) {
        Ok(peaks) => peaks,
        Err(error) => {
            warn!(%error, track_id = %request.track_id, uri = %request.redacted_uri, "failed to generate waveform");
            return;
        }
    };
    let Some(peaks) = sanitize_waveform_peaks(peaks) else {
        return;
    };
    if let Err(error) = save_cached_waveform(&request.cache_key, request.duration_seconds, &peaks) {
        warn!(%error, track_id = %request.track_id, "failed to cache waveform");
    }
    publish_waveform_peaks(&playback_snapshot, &events, &request.cache_key, peaks);
}

fn start_waveform_warm_worker(requests: Vec<WaveformWarmRequest>, worker: WaveformWarmWorker) {
    thread::spawn(move || {
        for request in requests {
            if !waveform_warm_generation_matches(&worker.generation, worker.request_generation) {
                return;
            }
            thread::sleep(WAVEFORM_WARM_DELAY);
            if !waveform_warm_generation_matches(&worker.generation, worker.request_generation) {
                return;
            }
            warm_waveform_request(&worker, request);
            if !waveform_warm_generation_matches(&worker.generation, worker.request_generation) {
                return;
            }
        }
    });
}

fn warm_waveform_request(worker: &WaveformWarmWorker, request: WaveformWarmRequest) {
    if !waveform_warm_generation_matches(&worker.generation, worker.request_generation) {
        return;
    }
    let cache_key = waveform_cache_key(
        &request.server_id,
        &request.track_id,
        request.duration_seconds,
    );
    if load_cached_waveform(&cache_key, request.duration_seconds).is_some() {
        return;
    }
    let Some(_permit) =
        acquire_waveform_generation_permit(&cache_key, WaveformGenerationKind::Warm)
    else {
        return;
    };
    if load_cached_waveform(&cache_key, request.duration_seconds).is_some() {
        return;
    }
    if !waveform_warm_generation_matches(&worker.generation, worker.request_generation) {
        return;
    }
    let Some((uri, redacted_uri)) =
        waveform_source_for_track(&worker.store, &worker.runtime, &worker.secrets, &request)
    else {
        return;
    };
    if !waveform_warm_generation_matches(&worker.generation, worker.request_generation) {
        return;
    }
    if waveform_generation_source_is_remote(&uri)
        && !remote_waveform_warm_can_run(&worker.playback_snapshot)
    {
        return;
    }
    generate_and_publish_waveform(
        Arc::clone(&worker.playback_snapshot),
        worker.events.clone(),
        WaveformGenerationRequest {
            cache_key,
            track_id: request.track_id,
            duration_seconds: request.duration_seconds,
            source_format: request.source_format,
            uri,
            redacted_uri,
        },
    );
}

fn waveform_source_for_track(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    request: &WaveformWarmRequest,
) -> Option<(String, String)> {
    let stream = resolve_stream(
        store,
        runtime,
        secrets,
        &request.server_id,
        &request.track_id,
        &request.playback_settings,
    )
    .map_err(|error| {
        warn!(%error, "failed to resolve stream for waveform");
        error
    })
    .ok()?;
    Some((stream.uri().to_string(), stream.redacted_uri().to_string()))
}

fn remote_waveform_warm_can_run(playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>) -> bool {
    playback_snapshot
        .lock()
        .map(|snapshot| {
            snapshot.state != PlaybackState::Buffering && snapshot.buffering_percent.is_none()
        })
        .unwrap_or(false)
}

fn waveform_warm_requests(
    snapshot: &QueueSnapshot,
    playback_settings: &PlaybackSettings,
    limit: usize,
) -> Vec<WaveformWarmRequest> {
    let start = snapshot
        .current_index
        .map(|index| index.saturating_add(1))
        .unwrap_or(0);
    snapshot
        .entries
        .iter()
        .skip(start)
        .take(limit)
        .filter(|entry| entry.duration_seconds > 0)
        .map(|entry| WaveformWarmRequest {
            server_id: snapshot.server_id.clone(),
            track_id: entry.track_id.clone(),
            duration_seconds: entry.duration_seconds,
            source_format: entry.source_format.clone(),
            playback_settings: playback_settings.clone(),
        })
        .collect()
}

fn acquire_waveform_generation_permit(
    cache_key: &str,
    kind: WaveformGenerationKind,
) -> Option<WaveformGenerationPermit> {
    let in_flight = kind.in_flight().get_or_init(|| Mutex::new(HashSet::new()));
    let mut in_flight = in_flight.lock().ok()?;
    if !in_flight.insert(cache_key.to_string()) {
        return None;
    }
    Some(WaveformGenerationPermit {
        cache_key: cache_key.to_string(),
        kind,
    })
}

fn waveform_warm_generation_matches(generation: &Arc<AtomicU64>, request_generation: u64) -> bool {
    generation.load(Ordering::Acquire) == request_generation
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

fn publish_cached_waveform(
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    events: &Sender<ControllerEvent>,
    cache_key: &str,
    duration_seconds: u32,
) -> bool {
    let Some(peaks) = load_cached_waveform(cache_key, duration_seconds) else {
        return false;
    };
    publish_waveform_peaks(playback_snapshot, events, cache_key, peaks);
    true
}

fn publish_waveform_peaks(
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    events: &Sender<ControllerEvent>,
    cache_key: &str,
    peaks: Vec<(f64, f64)>,
) {
    let Ok(mut snapshot) = playback_snapshot.lock() else {
        return;
    };
    if snapshot.waveform_cache_key.as_deref() != Some(cache_key) {
        return;
    }
    snapshot.waveform_peaks = Some(Arc::new(peaks));
    let event_snapshot = snapshot.clone();
    drop(snapshot);
    let _sent = events.send(ControllerEvent::Playback(Box::new(event_snapshot)));
}

pub(in crate::controller) fn waveform_cache_key(
    server_id: &ServerId,
    track_id: &TrackId,
    duration_seconds: u32,
) -> String {
    let track_hash = format!("{:x}", md5::compute(track_id.as_str()));
    format!(
        "{}/{}-{}.json",
        encode_key_part(server_id.as_str()),
        track_hash,
        duration_seconds
    )
}

pub(in crate::controller) fn waveform_cache_key_for_queue(
    queue: Option<&QueueEngine>,
) -> Option<String> {
    let queue = queue?;
    let snapshot = queue.snapshot();
    let entry = queue.current()?;
    Some(waveform_cache_key(
        &snapshot.server_id,
        &entry.track_id,
        entry.duration_seconds,
    ))
}

pub(in crate::controller) fn set_waveform_cache_key(
    snapshot: &mut PlaybackSnapshot,
    key: Option<String>,
) {
    waveform_duration_key(snapshot, key, snapshot.duration_seconds);
}

pub(in crate::controller) fn waveform_duration_key(
    snapshot: &mut PlaybackSnapshot,
    key: Option<String>,
    duration_seconds: u32,
) {
    if snapshot.waveform_cache_key != key {
        snapshot.waveform_cache_key = key;
        snapshot.waveform_peaks = None;
    }
    let Some(cache_key) = snapshot.waveform_cache_key.as_deref() else {
        return;
    };
    if snapshot.waveform_peaks.is_none() {
        snapshot.waveform_peaks = cached_waveform_peaks(cache_key, duration_seconds);
    }
}

pub(in crate::controller) fn cached_waveform_peaks(
    cache_key: &str,
    duration_seconds: u32,
) -> Option<Arc<Vec<(f64, f64)>>> {
    load_cached_waveform(cache_key, duration_seconds).map(Arc::new)
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

    #[test]
    fn playback_waveform_scoped() {
        let server_id = ServerId::new("server/one");
        let track_id = TrackId::new("album/track:one");

        let key = waveform_cache_key(&server_id, &track_id, 123);

        assert!(key.starts_with("server_one/"));
        assert!(key.ends_with("-123.json"));
        assert!(!key.contains("track:one"));
    }

    #[test]
    fn playback_clamp_amplitudes() {
        let peaks = sanitize_waveform_peaks(vec![(0.5, 1.5), (f64::NAN, 0.2), (-1.0, 0.25)])
            .expect("peaks");

        assert_eq!(peaks, vec![(0.5, 1.0), (0.0, 0.25)]);
        assert_eq!(sanitize_waveform_peaks(Vec::new()), None);
    }

    #[test]
    fn playback_waveform_sources() {
        assert!(waveform_generation_source_is_local(
            "file:///music/track.flac"
        ));
        assert!(waveform_generation_source_is_supported(
            "file:///music/track.flac"
        ));
        assert!(!waveform_generation_source_is_remote(
            "file:///music/track.flac"
        ));
        assert!(!waveform_generation_source_is_local(
            "https://music.example/stream"
        ));
        assert!(waveform_generation_source_is_supported(
            "https://music.example/stream"
        ));
        assert!(waveform_generation_source_is_remote(
            "https://music.example/stream"
        ));
        assert!(waveform_generation_source_is_remote(
            "http://music.example/stream"
        ));
        assert!(!waveform_generation_source_is_remote(
            "fake://music.example/stream"
        ));
        assert!(!waveform_generation_source_is_supported(
            "fake://music.example/stream"
        ));
    }

    #[test]
    fn playback_waveform_key() {
        let cache_key = "test-server/test-track-42.json";

        let permit = acquire_waveform_generation_permit(cache_key, WaveformGenerationKind::Warm)
            .expect("first warm permit");
        assert!(
            acquire_waveform_generation_permit(cache_key, WaveformGenerationKind::Warm).is_none()
        );
        let foreground =
            acquire_waveform_generation_permit(cache_key, WaveformGenerationKind::Foreground)
                .expect("foreground permit");
        assert!(
            acquire_waveform_generation_permit(cache_key, WaveformGenerationKind::Foreground)
                .is_none()
        );

        drop(permit);
        assert!(
            acquire_waveform_generation_permit(cache_key, WaveformGenerationKind::Warm).is_some()
        );
        drop(foreground);
    }

    #[test]
    fn playback_cap_current() {
        let server_id = ServerId::new("server-one");
        let snapshot = QueueSnapshot {
            server_id: server_id.clone(),
            entries: (1..=5)
                .map(|number| QueueEntry {
                    id: QueueEntryId::new(format!("queue-{number}")),
                    track_id: TrackId::new(format!("track-{number}")),
                    album_id: None,
                    title: format!("Track {number}"),
                    artist: "Example Artist".to_string(),
                    artist_id: None,
                    album: "Example Album".to_string(),
                    year: 2024,
                    duration_seconds: 180 + number,
                    favorite: false,
                    image_ref: None,
                    local_path: None,
                    source_format: (number == 4).then(|| "flac".to_string()),
                    origin: None,
                })
                .collect(),
            current_index: Some(2),
            repeat_mode: RepeatMode::All,
            shuffle: rufin_core::ShuffleState::default(),
            shuffle_order: Vec::new(),
            progress_seconds: 0,
        };

        let requests = waveform_warm_requests(&snapshot, &PlaybackSettings::default(), 2);

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].server_id, server_id);
        assert_eq!(requests[0].track_id, TrackId::new("track-4"));
        assert_eq!(requests[0].source_format.as_deref(), Some("flac"));
        assert_eq!(requests[1].track_id, TrackId::new("track-5"));
        assert_eq!(requests[1].source_format, None);
    }

    #[test]
    fn playback_waveform_rejects_dsd_formats() {
        assert!(!waveform_generation_format_is_supported(Some("dsf")));
        assert!(!waveform_generation_format_is_supported(Some(
            "audio/x-dsf"
        )));
        assert!(!waveform_generation_format_is_supported(Some("DSD64")));
        assert!(!waveform_generation_format_is_supported(Some(".dff")));
        assert!(waveform_generation_format_is_supported(Some("flac")));
        assert!(waveform_generation_format_is_supported(Some("audio/flac")));
        assert!(waveform_generation_format_is_supported(None));
    }

    #[test]
    fn playback_reject_meta() {
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
}
