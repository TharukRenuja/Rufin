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
static WAVEFORM_GENERATION_IN_FLIGHT: std::sync::OnceLock<Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();
static WAVEFORM_WARM_ACTIVE: std::sync::OnceLock<Mutex<bool>> = std::sync::OnceLock::new();

struct WaveformGenerationPermit {
    cache_key: String,
}

impl Drop for WaveformGenerationPermit {
    fn drop(&mut self) {
        if let Some(in_flight) = WAVEFORM_GENERATION_IN_FLIGHT.get()
            && let Ok(mut in_flight) = in_flight.lock()
        {
            in_flight.remove(&self.cache_key);
        }
    }
}

struct WaveformWarmPermit;

impl Drop for WaveformWarmPermit {
    fn drop(&mut self) {
        if let Some(active) = WAVEFORM_WARM_ACTIVE.get()
            && let Ok(mut active) = active.lock()
        {
            *active = false;
        }
    }
}

#[derive(Clone)]
struct WaveformWarmRequest {
    server_id: ServerId,
    track_id: TrackId,
    duration_seconds: u32,
    playback_settings: PlaybackSettings,
}

impl AppController {
    pub fn request_waveform_for_current(&self) {
        if !self.load_settings().seekbar_waveform_enabled {
            return;
        }
        let Some((server_id, entry, _next, _position, playback_settings)) =
            self.current_playback_request()
        else {
            return;
        };
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
        let Some(permit) = acquire_waveform_generation_permit(&cache_key) else {
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
                cache_key,
                request.track_id,
                request.duration_seconds,
                uri,
                redacted_uri,
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
        let requests = waveform_warm_requests_for_queue_snapshot(
            &queue_snapshot,
            &settings.playback,
            WAVEFORM_WARM_QUEUE_LIMIT,
        );
        if requests.is_empty() {
            return;
        }
        let Some(permit) = acquire_waveform_warm_permit() else {
            return;
        };
        start_waveform_warm_worker(
            permit,
            requests,
            self.store.clone(),
            Arc::clone(&self.runtime),
            Arc::clone(&self.secrets),
            Arc::clone(&self.playback_snapshot),
            self.events.clone(),
        );
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
    let Some(permit) = acquire_waveform_generation_permit(&cache_key) else {
        return;
    };
    thread::spawn(move || {
        let _permit = permit;
        thread::sleep(CURRENT_WAVEFORM_GENERATION_DELAY);
        generate_and_publish_waveform(
            playback_snapshot,
            events,
            cache_key,
            entry.track_id,
            entry.duration_seconds,
            item.stream.uri().to_string(),
            item.stream.redacted_uri().to_string(),
        );
    });
}

fn generate_and_publish_waveform(
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    events: Sender<ControllerEvent>,
    cache_key: String,
    track_id: TrackId,
    duration_seconds: u32,
    uri: String,
    redacted_uri: String,
) {
    if publish_cached_waveform(&playback_snapshot, &events, &cache_key, duration_seconds) {
        return;
    }
    let temp_source = if waveform_generation_source_is_remote(&uri) {
        match download_remote_waveform_source(&cache_key, &uri) {
            Ok(source) => Some(source),
            Err(error) => {
                warn!(%error, uri = %redacted_uri, "failed to download remote waveform source");
                return;
            }
        }
    } else {
        None
    };
    let source_uri = temp_source
        .as_ref()
        .map(|source| source.uri.as_str())
        .unwrap_or(uri.as_str());
    if !waveform_generation_source_is_local(source_uri) {
        debug!(
            track_id = %track_id,
            "skipped unsupported waveform generation source"
        );
        return;
    }
    let peaks = match generate_waveform_peaks(source_uri) {
        Ok(peaks) => peaks,
        Err(error) => {
            warn!(%error, track_id = %track_id, "failed to generate waveform");
            return;
        }
    };
    let Some(peaks) = sanitize_waveform_peaks(peaks) else {
        return;
    };
    if let Err(error) = save_cached_waveform(&cache_key, duration_seconds, &peaks) {
        warn!(%error, track_id = %track_id, "failed to cache waveform");
    }
    publish_waveform_peaks(&playback_snapshot, &events, &cache_key, peaks);
}

fn start_waveform_warm_worker(
    permit: WaveformWarmPermit,
    requests: Vec<WaveformWarmRequest>,
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    events: Sender<ControllerEvent>,
) {
    thread::spawn(move || {
        let _permit = permit;
        for request in requests {
            warm_waveform_request(
                &store,
                &runtime,
                &secrets,
                &playback_snapshot,
                &events,
                request,
            );
            thread::sleep(WAVEFORM_WARM_DELAY);
        }
    });
}

fn warm_waveform_request(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    events: &Sender<ControllerEvent>,
    request: WaveformWarmRequest,
) {
    let cache_key = waveform_cache_key(
        &request.server_id,
        &request.track_id,
        request.duration_seconds,
    );
    if load_cached_waveform(&cache_key, request.duration_seconds).is_some() {
        return;
    }
    let Some(_permit) = acquire_waveform_generation_permit(&cache_key) else {
        return;
    };
    if load_cached_waveform(&cache_key, request.duration_seconds).is_some() {
        return;
    }
    let Some((uri, redacted_uri)) = waveform_source_for_track(store, runtime, secrets, &request)
    else {
        return;
    };
    if waveform_generation_source_is_remote(&uri)
        && !remote_waveform_warm_can_run(playback_snapshot)
    {
        return;
    }
    generate_and_publish_waveform(
        Arc::clone(playback_snapshot),
        events.clone(),
        cache_key,
        request.track_id,
        request.duration_seconds,
        uri,
        redacted_uri,
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

fn waveform_warm_requests_for_queue_snapshot(
    snapshot: &QueueSnapshot,
    playback_settings: &PlaybackSettings,
    limit: usize,
) -> Vec<WaveformWarmRequest> {
    let start = snapshot.current_index.unwrap_or(0);
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
            playback_settings: playback_settings.clone(),
        })
        .collect()
}

fn acquire_waveform_generation_permit(cache_key: &str) -> Option<WaveformGenerationPermit> {
    let in_flight = WAVEFORM_GENERATION_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let mut in_flight = in_flight.lock().ok()?;
    if !in_flight.insert(cache_key.to_string()) {
        return None;
    }
    Some(WaveformGenerationPermit {
        cache_key: cache_key.to_string(),
    })
}

fn acquire_waveform_warm_permit() -> Option<WaveformWarmPermit> {
    let active = WAVEFORM_WARM_ACTIVE.get_or_init(|| Mutex::new(false));
    let mut active = active.lock().ok()?;
    if *active {
        return None;
    }
    *active = true;
    Some(WaveformWarmPermit)
}

fn waveform_generation_source_is_local(uri: &str) -> bool {
    uri.starts_with("file://")
}

fn waveform_generation_source_is_remote(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

struct TempWaveformSource {
    uri: String,
    path: PathBuf,
}

impl Drop for TempWaveformSource {
    fn drop(&mut self) {
        let _ignored = fs::remove_file(&self.path);
    }
}

fn download_remote_waveform_source(
    cache_key: &str,
    uri: &str,
) -> Result<TempWaveformSource, String> {
    let path =
        remote_waveform_temp_path(cache_key).ok_or_else(|| "No cache directory.".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())?;
    let mut response = client
        .get(uri)
        .send()
        .map_err(|_| "request failed".to_string())?;
    if !response.status().is_success() {
        return Err(format!("request failed with HTTP {}", response.status()));
    }
    let mut file = fs::File::create(&path).map_err(|error| error.to_string())?;
    std::io::copy(&mut response, &mut file).map_err(|error| error.to_string())?;
    let file_uri = reqwest::Url::from_file_path(&path)
        .map_err(|()| {
            format!(
                "Could not turn waveform temp path into a file URI: {}",
                path.display()
            )
        })?
        .to_string();
    Ok(TempWaveformSource {
        uri: file_uri,
        path,
    })
}

fn remote_waveform_temp_path(cache_key: &str) -> Option<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let key_hash = format!("{:x}", md5::compute(cache_key));
    cache_dir().map(|dir| {
        tmp_cache_dir_for_cache_dir(&dir)
            .join("waveforms")
            .join(format!("{key_hash}-{}-{stamp}.audio", std::process::id()))
    })
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
    set_waveform_cache_key_for_duration(snapshot, key, snapshot.duration_seconds);
}

pub(in crate::controller) fn set_waveform_cache_key_for_duration(
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
    fn waveform_cache_key_is_path_safe_and_duration_scoped() {
        let server_id = ServerId::new("server/one");
        let track_id = TrackId::new("album/track:one");

        let key = waveform_cache_key(&server_id, &track_id, 123);

        assert!(key.starts_with("server_one/"));
        assert!(key.ends_with("-123.json"));
        assert!(!key.contains("track:one"));
    }

    #[test]
    fn sanitize_waveform_peaks_drops_invalid_values_and_clamps_amplitudes() {
        let peaks = sanitize_waveform_peaks(vec![(0.5, 1.5), (f64::NAN, 0.2), (-1.0, 0.25)])
            .expect("peaks");

        assert_eq!(peaks, vec![(0.5, 1.0), (0.0, 0.25)]);
        assert_eq!(sanitize_waveform_peaks(Vec::new()), None);
    }

    #[test]
    fn waveform_generation_routes_file_and_remote_sources() {
        assert!(waveform_generation_source_is_local(
            "file:///music/track.flac"
        ));
        assert!(!waveform_generation_source_is_remote(
            "file:///music/track.flac"
        ));
        assert!(!waveform_generation_source_is_local(
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
    }

    #[test]
    fn waveform_generation_permit_blocks_duplicate_keys() {
        let cache_key = "test-server/test-track-42.json";

        let permit = acquire_waveform_generation_permit(cache_key).expect("first permit");
        assert!(acquire_waveform_generation_permit(cache_key).is_none());

        drop(permit);
        assert!(acquire_waveform_generation_permit(cache_key).is_some());
    }

    #[test]
    fn queue_waveform_warm_starts_at_current_and_caps() {
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
                    source_format: None,
                    origin: None,
                })
                .collect(),
            current_index: Some(2),
            repeat_mode: RepeatMode::All,
            shuffle: rufin_core::ShuffleState::default(),
            shuffle_order: Vec::new(),
            progress_seconds: 0,
            source_snapshot: None,
        };

        let requests =
            waveform_warm_requests_for_queue_snapshot(&snapshot, &PlaybackSettings::default(), 2);

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].server_id, server_id);
        assert_eq!(requests[0].track_id, TrackId::new("track-3"));
        assert_eq!(requests[1].track_id, TrackId::new("track-4"));
    }

    #[test]
    fn cached_waveform_parser_rejects_stale_metadata() {
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
