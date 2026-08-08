//! One cancellable loudness-analysis job for the selected source.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use library::{
    LoudnessAlbumInput, LoudnessAnalysisSnapshot, LoudnessItemId, LoudnessMeasurementWrite,
    LoudnessTrackInput, SourceId, StreamRequest, TrackId,
};
use playback::{LoudnessNormalizationMode, SourceSessionEpoch};
use playback_gstreamer::{LoudnessAnalysis, album_loudness, analyze_loudness_cancellable};
use tracing::{info, warn};

use crate::playback::prepare_stream;
use crate::source::{ActiveSource, WeakActiveSource};

struct ActiveAnalysis {
    source_id: SourceId,
    source_session_epoch: SourceSessionEpoch,
    selected: WeakActiveSource,
    cancelled: Arc<AtomicBool>,
    restart_requested: bool,
}

#[derive(Default)]
struct AnalysisSummary {
    tracks: usize,
    albums: usize,
    failures: usize,
}

struct AlbumWork {
    album: LoudnessAlbumInput,
    tracks: Vec<LoudnessTrackInput>,
}

#[derive(Default)]
struct AnalysisPlan {
    albums: Vec<AlbumWork>,
    tracks: Vec<LoudnessTrackInput>,
}

pub(crate) struct LoudnessAnalysisOwner {
    runtime: tokio::runtime::Handle,
    active: Mutex<Option<ActiveAnalysis>>,
}

impl LoudnessAnalysisOwner {
    pub(crate) fn new(runtime: tokio::runtime::Handle) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            active: Mutex::new(None),
        })
    }

    pub(crate) fn settings_changed(
        self: &Arc<Self>,
        mode: LoudnessNormalizationMode,
        selected: Option<Arc<ActiveSource>>,
    ) {
        if mode == LoudnessNormalizationMode::Off {
            self.cancel();
            return;
        }
        let Some(selected) = selected else {
            self.cancel();
            return;
        };
        let Some(state) = selected.resolve() else {
            self.cancel();
            return;
        };
        let source_id = state.source_id().clone();
        let source_session_epoch = state.source_session_epoch;
        drop(state);

        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active.as_ref().is_some_and(|active| {
                active.source_id == source_id && active.source_session_epoch == source_session_epoch
            }) {
                return;
            }
            if let Some(previous) = active.take() {
                previous.cancelled.store(true, Ordering::Release);
            }
            *active = Some(ActiveAnalysis {
                source_id: source_id.clone(),
                source_session_epoch,
                selected: selected.downgrade(),
                cancelled: Arc::clone(&cancelled),
                restart_requested: false,
            });
        }

        let owner = Arc::downgrade(self);
        let selected = selected.downgrade();
        self.runtime.spawn(async move {
            info!(%source_id, "analyzing missing loudness data");
            let summary = analyze_selected(selected, Arc::clone(&cancelled)).await;
            if !cancelled.load(Ordering::Acquire) {
                info!(
                    %source_id,
                    tracks = summary.tracks,
                    albums = summary.albums,
                    failures = summary.failures,
                    "finished loudness analysis"
                );
            }
            if let Some(owner) = owner.upgrade() {
                owner.finish(&cancelled);
            }
        });
    }

    pub(crate) fn library_changed(
        self: &Arc<Self>,
        mode: LoudnessNormalizationMode,
        selected: Option<Arc<ActiveSource>>,
    ) {
        if mode == LoudnessNormalizationMode::Off {
            self.cancel();
            return;
        }
        let Some(selected) = selected else {
            self.cancel();
            return;
        };
        let Some(state) = selected.resolve() else {
            self.cancel();
            return;
        };
        let source_id = state.source_id();
        let source_session_epoch = state.source_session_epoch;
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = active.as_mut().filter(|active| {
            &active.source_id == source_id && active.source_session_epoch == source_session_epoch
        }) {
            active.restart_requested = true;
            return;
        }
        drop(active);
        drop(state);
        self.settings_changed(mode, Some(selected));
    }

    pub(crate) fn cancel(&self) {
        if let Some(active) = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            active.cancelled.store(true, Ordering::Release);
        }
    }

    fn finish(self: &Arc<Self>, cancelled: &Arc<AtomicBool>) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let restart = if active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(&active.cancelled, cancelled))
        {
            active.take().and_then(|active| {
                active
                    .restart_requested
                    .then(|| active.selected.upgrade())
                    .flatten()
            })
        } else {
            None
        };
        drop(active);
        if let Some(selected) = restart {
            self.settings_changed(LoudnessNormalizationMode::Track, Some(selected));
        }
    }
}

async fn analyze_selected(
    selected: WeakActiveSource,
    cancelled: Arc<AtomicBool>,
) -> AnalysisSummary {
    let mut summary = AnalysisSummary::default();
    let mut attempted_tracks = HashSet::new();
    let mut attempted_albums = HashSet::new();

    loop {
        if analysis_cancelled(&selected, &cancelled) {
            break;
        }
        let Some(state) = selected.upgrade().and_then(|selected| selected.resolve()) else {
            break;
        };
        let snapshot = match state.library.loudness_analysis_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(%error, "could not read tracks for loudness analysis");
                break;
            }
        };
        let plan = analysis_plan(&snapshot, &attempted_tracks, &attempted_albums);
        if plan.albums.is_empty() && plan.tracks.is_empty() {
            break;
        }

        for work in plan.albums {
            attempted_albums.insert(work.album.analysis_key);
            let mut analyses = Vec::with_capacity(work.tracks.len());
            let mut writes = Vec::new();
            let mut complete = true;
            for track in work.tracks {
                attempted_tracks.insert(track.analysis_key);
                match analyze_track(&selected, &cancelled, &track).await {
                    Ok(analysis) => {
                        if track.current.is_none() {
                            writes.push(LoudnessMeasurementWrite {
                                item: LoudnessItemId::Track(track.track.id.clone()),
                                analysis_key: track.analysis_key,
                                measurement: analysis.measurement(),
                            });
                        }
                        analyses.push(analysis);
                    }
                    Err(error) => {
                        complete = false;
                        if !analysis_cancelled(&selected, &cancelled) {
                            warn!(
                                track_id = %track.track.id,
                                %error,
                                "could not analyze track loudness"
                            );
                            summary.failures += 1;
                        }
                        break;
                    }
                }
            }
            if analysis_cancelled(&selected, &cancelled) {
                break;
            }
            if complete {
                match album_loudness(&analyses) {
                    Ok(measurement) => {
                        let track_count = writes.len();
                        writes.push(LoudnessMeasurementWrite {
                            item: LoudnessItemId::Album(work.album.album_id.clone()),
                            analysis_key: work.album.analysis_key,
                            measurement,
                        });
                        if store_measurements(&selected, writes) {
                            summary.tracks += track_count;
                            summary.albums += 1;
                        }
                    }
                    Err(error) => {
                        warn!(album_id = %work.album.album_id, %error, "could not combine album loudness");
                        summary.failures += 1;
                    }
                }
            }
        }

        for track in plan.tracks {
            if analysis_cancelled(&selected, &cancelled) {
                break;
            }
            attempted_tracks.insert(track.analysis_key);
            match analyze_track(&selected, &cancelled, &track).await {
                Ok(analysis)
                    if store_measurements(
                        &selected,
                        vec![LoudnessMeasurementWrite {
                            item: LoudnessItemId::Track(track.track.id.clone()),
                            analysis_key: track.analysis_key,
                            measurement: analysis.measurement(),
                        }],
                    ) =>
                {
                    summary.tracks += 1;
                }
                Ok(_) => {}
                Err(error) => {
                    if !analysis_cancelled(&selected, &cancelled) {
                        warn!(
                            track_id = %track.track.id,
                            %error,
                            "could not analyze track loudness"
                        );
                        summary.failures += 1;
                    }
                }
            }
        }
    }

    summary
}

fn analysis_plan(
    snapshot: &LoudnessAnalysisSnapshot,
    attempted_tracks: &HashSet<[u8; 32]>,
    attempted_albums: &HashSet<[u8; 32]>,
) -> AnalysisPlan {
    let tracks = snapshot
        .tracks
        .iter()
        .cloned()
        .map(|track| (track.track.id.clone(), track))
        .collect::<HashMap<_, _>>();
    let mut claimed = HashSet::<TrackId>::new();
    let mut plan = AnalysisPlan::default();

    for album in snapshot
        .albums
        .iter()
        .filter(|album| album.current.is_none() && !attempted_albums.contains(&album.analysis_key))
    {
        let album_tracks = album
            .track_ids
            .iter()
            .filter_map(|track_id| tracks.get(track_id).cloned())
            .collect::<Vec<_>>();
        if album_tracks.len() != album.track_ids.len() {
            continue;
        }
        claimed.extend(album.track_ids.iter().cloned());
        plan.albums.push(AlbumWork {
            album: album.clone(),
            tracks: album_tracks,
        });
    }

    plan.tracks = snapshot
        .tracks
        .iter()
        .filter(|track| {
            track.current.is_none()
                && !claimed.contains(&track.track.id)
                && !attempted_tracks.contains(&track.analysis_key)
        })
        .cloned()
        .collect();
    plan
}

async fn analyze_track(
    selected: &WeakActiveSource,
    cancelled: &Arc<AtomicBool>,
    input: &LoudnessTrackInput,
) -> Result<LoudnessAnalysis, String> {
    let state = selected
        .upgrade()
        .and_then(|selected| selected.resolve())
        .ok_or_else(|| "the selected source changed".to_string())?;
    let stream = prepare_stream(
        Some(Arc::clone(&state.library)),
        state.source.clone(),
        StreamRequest::original(input.track.id.clone()),
    )
    .await?;
    if analysis_cancelled(selected, cancelled) {
        return Err("loudness analysis cancelled".to_string());
    }
    let selected = selected.clone();
    let cancelled = Arc::clone(cancelled);
    tokio::task::spawn_blocking(move || {
        analyze_loudness_cancellable(&stream, || analysis_cancelled(&selected, &cancelled))
    })
    .await
    .map_err(|_| "loudness analysis worker stopped unexpectedly".to_string())?
}

fn store_measurements(selected: &WeakActiveSource, writes: Vec<LoudnessMeasurementWrite>) -> bool {
    let Some(state) = selected.upgrade().and_then(|selected| selected.resolve()) else {
        return false;
    };
    match state.library.store_loudness(writes) {
        Ok(()) => true,
        Err(error) => {
            warn!(%error, "could not store loudness analysis");
            false
        }
    }
}

fn analysis_cancelled(selected: &WeakActiveSource, cancelled: &AtomicBool) -> bool {
    cancelled.load(Ordering::Acquire)
        || selected
            .upgrade()
            .and_then(|selected| selected.resolve())
            .is_none()
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, LoudnessMeasurement, Track};

    use super::*;

    #[test]
    fn a_missing_album_reanalyzes_every_member_but_stores_no_duplicate_track_work() {
        let first = track_input("first", 1, Some(measurement(-20.0)));
        let second = track_input("second", 2, None);
        let album = LoudnessAlbumInput {
            album_id: AlbumId::new("album"),
            analysis_key: [3; 32],
            track_ids: vec![first.track.id.clone(), second.track.id.clone()].into(),
            current: None,
        };
        let snapshot = LoudnessAnalysisSnapshot {
            tracks: vec![first, second].into(),
            albums: vec![album].into(),
        };

        let plan = analysis_plan(&snapshot, &HashSet::new(), &HashSet::new());

        assert_eq!(plan.albums.len(), 1);
        assert_eq!(plan.albums[0].tracks.len(), 2);
        assert!(plan.tracks.is_empty());
    }

    #[test]
    fn a_valid_album_does_not_hide_a_missing_track_measurement() {
        let first = track_input("first", 1, Some(measurement(-20.0)));
        let second = track_input("second", 2, None);
        let album = LoudnessAlbumInput {
            album_id: AlbumId::new("album"),
            analysis_key: [3; 32],
            track_ids: vec![first.track.id.clone(), second.track.id.clone()].into(),
            current: Some(measurement(-19.0)),
        };
        let snapshot = LoudnessAnalysisSnapshot {
            tracks: vec![first, second].into(),
            albums: vec![album].into(),
        };

        let plan = analysis_plan(&snapshot, &HashSet::new(), &HashSet::new());

        assert!(plan.albums.is_empty());
        assert_eq!(plan.tracks.len(), 1);
        assert_eq!(plan.tracks[0].track.id, TrackId::new("second"));
    }

    fn track_input(id: &str, key: u8, current: Option<LoudnessMeasurement>) -> LoudnessTrackInput {
        LoudnessTrackInput {
            track: serde_json::from_value::<Track>(serde_json::json!({
                "id": id,
                "album_id": "album",
                "title": id,
                "artist": "Artist",
                "album": "Album",
                "year": 2026,
                "duration_seconds": 180,
                "favorite": false,
                "disc_number": 1,
                "track_number": key
            }))
            .expect("track"),
            analysis_key: [key; 32],
            current,
        }
    }

    fn measurement(lufs: f64) -> LoudnessMeasurement {
        LoudnessMeasurement::new(Some(lufs), 0.8).expect("measurement")
    }
}
