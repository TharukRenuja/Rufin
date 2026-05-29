use super::*;

#[derive(Debug, Deserialize, Serialize)]
struct CachedWaveform {
    version: u8,
    duration_seconds: u32,
    peaks: Vec<(f64, f64)>,
}

const WAVEFORM_CACHE_VERSION: u8 = 1;

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

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let events = self.events.clone();
        thread::spawn(move || {
            let item = match resolve_prepared_item(
                &store,
                &runtime,
                &secrets,
                &server_id,
                &entry,
                &playback_settings,
            ) {
                Ok(item) => item,
                Err(error) => {
                    warn!(%error, "failed to resolve stream for waveform");
                    return;
                }
            };
            generate_and_publish_waveform(playback_snapshot, events, cache_key, entry, item);
        });
    }
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
    if snapshot.waveform_cache_key == key {
        return;
    }
    snapshot.waveform_cache_key = key;
    snapshot.waveform_peaks = None;
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
    generate_and_publish_waveform(playback_snapshot, events, cache_key, entry, item);
}

fn generate_and_publish_waveform(
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    events: Sender<ControllerEvent>,
    cache_key: String,
    entry: QueueEntry,
    item: PreparedPlaybackItem,
) {
    let peaks = match generate_waveform_peaks(item.stream.uri()) {
        Ok(peaks) => peaks,
        Err(error) => {
            warn!(%error, track_id = %entry.track_id, "failed to generate waveform");
            return;
        }
    };
    let Some(peaks) = sanitize_waveform_peaks(peaks) else {
        return;
    };
    if let Err(error) = save_cached_waveform(&cache_key, entry.duration_seconds, &peaks) {
        warn!(%error, track_id = %entry.track_id, "failed to cache waveform");
    }
    publish_waveform_peaks(&playback_snapshot, &events, &cache_key, peaks);
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

fn load_cached_waveform(cache_key: &str, duration_seconds: u32) -> Option<Vec<(f64, f64)>> {
    let path = waveform_cache_path_for_key(cache_key)?;
    let value = fs::read_to_string(path).ok()?;
    let cached = serde_json::from_str::<CachedWaveform>(&value).ok()?;
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
}
