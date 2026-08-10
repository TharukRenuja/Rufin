//! One cancellable loudness-analysis job for the selected source.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use library::{
    Library, LoudnessAlbumInput, LoudnessAnalysisSnapshot, LoudnessItemId,
    LoudnessMeasurementWrite, LoudnessTrackInput, ResolvedStream, SourceId, StreamRequest, TrackId,
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
    task: Option<tokio::task::AbortHandle>,
    restart_requested: bool,
}

impl ActiveAnalysis {
    fn cancel(mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
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
                previous.cancel();
            }
            *active = Some(ActiveAnalysis {
                source_id: source_id.clone(),
                source_session_epoch,
                selected: selected.downgrade(),
                cancelled: Arc::clone(&cancelled),
                task: None,
                restart_requested: false,
            });
        }

        let owner = Arc::downgrade(self);
        let selected = selected.downgrade();
        let task_cancelled = Arc::clone(&cancelled);
        let task = self.runtime.spawn(async move {
            info!(%source_id, "analyzing missing loudness data");
            let summary =
                analyze_selected(selected, Arc::clone(&task_cancelled), analyze_track).await;
            if !task_cancelled.load(Ordering::Acquire) {
                info!(
                    %source_id,
                    tracks = summary.tracks,
                    albums = summary.albums,
                    failures = summary.failures,
                    "finished loudness analysis"
                );
            }
            if let Some(owner) = owner.upgrade() {
                owner.finish(&task_cancelled);
            }
        });
        let task = task.abort_handle();
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = active
            .as_mut()
            .filter(|active| Arc::ptr_eq(&active.cancelled, &cancelled))
        {
            active.task = Some(task);
        } else {
            task.abort();
        }
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
            active.cancel();
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
    analyze: impl AsyncFn(
        &WeakActiveSource,
        &Arc<AtomicBool>,
        &LoudnessTrackInput,
    ) -> Result<LoudnessAnalysis, String>,
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
        drop(state);
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
                match analyze(&selected, &cancelled, &track).await {
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
            match analyze(&selected, &cancelled, &track).await {
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
    analyze_track_with(selected, cancelled, input, prepare_stream).await
}

async fn analyze_track_with(
    selected: &WeakActiveSource,
    cancelled: &Arc<AtomicBool>,
    input: &LoudnessTrackInput,
    resolve_stream: impl AsyncFn(
        Option<Arc<Library>>,
        Option<Arc<sources::Source>>,
        StreamRequest,
    ) -> Result<ResolvedStream, String>,
) -> Result<LoudnessAnalysis, String> {
    let (library, source) = {
        let state = selected
            .upgrade()
            .and_then(|selected| selected.resolve())
            .ok_or_else(|| "the selected source changed".to_string())?;
        (Arc::clone(&state.library), state.source.clone())
    };
    let stream = resolve_stream(
        Some(library),
        source,
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
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    use library::{
        AlbumId, CandidateBatch, CandidateFinish, CandidateHeader, HomeFacts, Libraries,
        LoudnessMeasurement, Track,
    };
    use sources::SourceConfiguration;

    use crate::source::SelectedSourceState;

    use super::*;

    #[test]
    fn cancelling_analysis_aborts_its_pending_task() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build test runtime");
        let owner = LoudnessAnalysisOwner::new(runtime.handle().clone());
        let retained = Arc::new(());
        let retired = Arc::downgrade(&retained);
        let task = runtime.spawn(async move {
            let _retained = retained;
            std::future::pending::<()>().await;
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        *owner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveAnalysis {
            source_id: SourceId::new("local:server:cancel-loudness"),
            source_session_epoch: SourceSessionEpoch::new(1),
            selected: std::sync::Weak::new(),
            cancelled: Arc::clone(&cancelled),
            task: Some(task.abort_handle()),
            restart_requested: false,
        });

        owner.cancel();
        let stopped = runtime
            .block_on(task)
            .expect_err("analysis task was aborted");

        assert!(stopped.is_cancelled());
        assert!(cancelled.load(Ordering::Acquire));
        assert!(retired.upgrade().is_none());
    }

    #[test]
    fn pending_track_work_does_not_retain_the_selected_library() {
        let (selected, retired_state, retired_library, _) = selected_loudness_fixture();

        let started = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&started);
        let mut analysis = Box::pin(analyze_selected(
            selected.downgrade(),
            Arc::new(AtomicBool::new(false)),
            async move |_, _, _| {
                observed.store(true, Ordering::Release);
                std::future::pending::<Result<LoudnessAnalysis, String>>().await
            },
        ));
        let mut context = Context::from_waker(Waker::noop());

        assert!(matches!(
            analysis.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert!(started.load(Ordering::Acquire));
        drop(selected);

        assert!(retired_state.upgrade().is_none());
        assert!(retired_library.upgrade().is_none());
    }

    #[test]
    fn pending_stream_resolution_does_not_retain_the_selected_library() {
        let (selected, retired_state, retired_library, input) = selected_loudness_fixture();
        let started = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&started);
        let selected_handle = selected.downgrade();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut analysis = Box::pin(analyze_track_with(
            &selected_handle,
            &cancelled,
            &input,
            async move |library, source, _| {
                drop(library);
                drop(source);
                observed.store(true, Ordering::Release);
                std::future::pending::<Result<ResolvedStream, String>>().await
            },
        ));
        let mut context = Context::from_waker(Waker::noop());

        assert!(matches!(
            analysis.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert!(started.load(Ordering::Acquire));
        drop(selected);

        assert!(retired_state.upgrade().is_none());
        assert!(retired_library.upgrade().is_none());
    }

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

    fn selected_loudness_fixture() -> (
        Arc<ActiveSource>,
        std::sync::Weak<SelectedSourceState>,
        std::sync::Weak<Library>,
        LoudnessTrackInput,
    ) {
        let input = track_input("pending", 1, None);
        let libraries = Libraries::memory().expect("open in-memory Libraries");
        let source_id = SourceId::new("local:server:loudness-lifetime");
        let mut candidate = libraries
            .begin_source_candidate(CandidateHeader {
                source_id: source_id.clone(),
                input_version: 1,
                input_digest: [1; 32],
            })
            .expect("begin source candidate");
        candidate
            .write(CandidateBatch::Tracks(vec![input.track.clone()]))
            .expect("write candidate Track");
        let library = candidate
            .finish(
                CandidateFinish {
                    freshness: None,
                    home: HomeFacts::RufinDefined,
                    accepted_at: 1,
                },
                None,
            )
            .and_then(|candidate| candidate.accept())
            .expect("accept source candidate")
            .library;
        let selected = ActiveSource::fixed_for_test(SelectedSourceState {
            configuration: SourceConfiguration {
                source_id,
                kind: "local".to_string(),
                name: "Loudness lifetime".to_string(),
                provider_payload: serde_json::json!({
                    "version": 1,
                    "roots": [],
                })
                .to_string(),
            },
            source: None,
            source_session_epoch: SourceSessionEpoch::new(1),
            home: library.home(None).expect("prepare Home"),
            library: Arc::clone(&library),
            music_folder_id: None,
        });
        let selected_state = selected.resolve().expect("selected source state");
        let retired_state = Arc::downgrade(&selected_state);
        let retired_library = Arc::downgrade(&library);
        drop(selected_state);
        drop(library);
        (selected, retired_state, retired_library, input)
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
