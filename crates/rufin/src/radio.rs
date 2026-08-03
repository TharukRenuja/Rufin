//! Random play, manual radio, and AutoDJ composition.
//!
//! Concrete Sources acquire native candidates, the selected `Library`
//! applies Rufin's common fallback and dedupe rules, and Playback alone mutates
//! the queue. This module retains no recommendation catalog or selected-source
//! state.

use std::sync::Arc;

use library::{
    Library, NativeRadioResult, RadioComposition, RadioSeed, RandomComposition, RandomPlayedFilter,
    Track,
};
use playback::{
    AutoDjRequest, Batch, BatchItem, Placement, Playback, Provenance, RadioPlayRequest,
    RandomPlayRequest,
};
use sources::{GeneratedTracksRequest, NativeSourceResult, RandomTrackRequest};
use tracing::warn;

use crate::playback::random_u64;
use crate::source::WeakActiveSource;

const MANUAL_RADIO_COUNT: usize = 20;

pub(crate) fn request_auto_dj(
    runtime: tokio::runtime::Handle,
    selected: WeakActiveSource,
    playback: Playback,
    request: AutoDjRequest,
) {
    let Some(initial) = selected.upgrade().and_then(|selected| selected.resolve()) else {
        return;
    };
    if initial.library.source_id() != &request.source_id {
        let _ = playback.auto_dj_unavailable(
            request.source_id,
            request.seed_occurrence,
            Some("the selected source changed".to_string()),
        );
        return;
    }
    let source = initial.source.clone();
    runtime.spawn(async move {
        let limit = request.requested_count.saturating_mul(4).clamp(1, 500);
        let native = match source.as_ref() {
            Some(source) => {
                source
                    .generated_tracks(GeneratedTracksRequest {
                        seed: RadioSeed::Track(request.seed_track_id.clone()),
                        limit,
                    })
                    .await
            }
            None => Ok(NativeSourceResult::Unavailable),
        };
        let native = match native {
            Ok(NativeSourceResult::Available(tracks)) => NativeRadioResult::Candidates(tracks),
            Ok(NativeSourceResult::Unavailable) | Err(_) => NativeRadioResult::Unavailable,
        };
        let Some(current) = selected.upgrade().and_then(|selected| selected.resolve()) else {
            return;
        };
        let result = compose_radio(
            Arc::clone(&current.library),
            RadioComposition {
                seed: RadioSeed::Track(request.seed_track_id),
                native,
                excluded_track_ids: Vec::new(),
                limit: request.requested_count,
                include_seed_track: false,
                variation: random_u64(),
            },
        )
        .await;
        let _ = tokio::task::spawn_blocking(move || match result {
            Ok(candidates) => playback.complete_auto_dj_candidates(
                request.source_id,
                request.seed_occurrence,
                candidates,
                request.requested_count,
                random_u64(),
            ),
            Err(error) => playback.auto_dj_unavailable(
                request.source_id,
                request.seed_occurrence,
                Some(error.to_string()),
            ),
        })
        .await;
    });
}

pub(crate) fn play_radio(
    runtime: tokio::runtime::Handle,
    selected: WeakActiveSource,
    playback: Playback,
    request: RadioPlayRequest,
) -> Option<tokio::task::JoinHandle<()>> {
    let placement: Placement = request.placement.into();
    let reservation = match playback.reserve_materialization(placement) {
        Ok(reservation) => reservation,
        Err(error) => {
            warn!(%error, "could not reserve radio queue work");
            return None;
        }
    };
    let Some(initial) = selected.upgrade().and_then(|selected| selected.resolve()) else {
        return None;
    };
    let source = initial.source.clone();
    Some(runtime.spawn(async move {
        let include_seed_track = match &request.seed {
            RadioSeed::Track(seed_track_id) => {
                reservation.current_track_id.as_ref() != Some(seed_track_id)
            }
            RadioSeed::Album(_)
            | RadioSeed::Artist(_)
            | RadioSeed::Genre { .. }
            | RadioSeed::Playlist(_) => false,
        };
        let excluded_track_ids = if matches!(placement, Placement::Replace { .. }) {
            reservation.current_track_id.clone().into_iter().collect()
        } else {
            reservation.queued_track_ids.clone()
        };
        let native = match source.as_ref() {
            Some(source) => {
                source
                    .generated_tracks(GeneratedTracksRequest {
                        seed: request.seed.clone(),
                        limit: MANUAL_RADIO_COUNT,
                    })
                    .await
            }
            None => Ok(NativeSourceResult::Unavailable),
        };
        let native = match native {
            Ok(NativeSourceResult::Available(tracks)) => NativeRadioResult::Candidates(tracks),
            Ok(NativeSourceResult::Unavailable) | Err(_) => NativeRadioResult::Unavailable,
        };
        let Some(current) = selected.upgrade().and_then(|selected| selected.resolve()) else {
            return;
        };
        let composed = compose_radio(
            Arc::clone(&current.library),
            RadioComposition {
                seed: request.seed,
                native,
                excluded_track_ids,
                limit: MANUAL_RADIO_COUNT,
                include_seed_track,
                variation: random_u64(),
            },
        )
        .await;
        complete_materialization(
            playback,
            reservation,
            placement,
            composed.map_err(|error| error.to_string()),
            Provenance::Radio,
        )
        .await;
    }))
}

pub(crate) fn play_random(
    runtime: tokio::runtime::Handle,
    selected: WeakActiveSource,
    playback: Playback,
    request: RandomPlayRequest,
) -> Option<tokio::task::JoinHandle<()>> {
    let placement: Placement = request.placement.into();
    let reservation = match playback.reserve_materialization(placement) {
        Ok(reservation) => reservation,
        Err(error) => {
            warn!(%error, "could not reserve random queue work");
            return None;
        }
    };
    let Some(initial) = selected.upgrade().and_then(|selected| selected.resolve()) else {
        return None;
    };
    let source = initial.source.clone();
    Some(runtime.spawn(async move {
        let native_request = RandomTrackRequest {
            limit: request.limit,
            min_year: request.min_year,
            max_year: request.max_year,
            genre_id: request.genre_id.clone(),
            genre_name: request.genre_name.clone(),
            played_filter: match request.played_filter {
                playback::PlayedFilter::All => sources::PlayedFilter::All,
                playback::PlayedFilter::Unplayed => sources::PlayedFilter::Unplayed,
                playback::PlayedFilter::Played => sources::PlayedFilter::Played,
            },
        };
        let native = match source.as_ref() {
            Some(source) => source.random_tracks(native_request).await,
            None => Ok(NativeSourceResult::Unavailable),
        };
        let Some(current) = selected.upgrade().and_then(|selected| selected.resolve()) else {
            return;
        };
        let native = match native {
            Ok(NativeSourceResult::Available(tracks)) => tracks,
            Ok(NativeSourceResult::Unavailable) | Err(_) => Vec::new(),
        };
        let tracks = compose_random(
            Arc::clone(&current.library),
            RandomComposition {
                native,
                limit: request.limit,
                min_year: request.min_year,
                max_year: request.max_year,
                genre_id: request.genre_id,
                genre_name: request.genre_name,
                played: match request.played_filter {
                    playback::PlayedFilter::All => RandomPlayedFilter::All,
                    playback::PlayedFilter::Unplayed => RandomPlayedFilter::Unplayed,
                    playback::PlayedFilter::Played => RandomPlayedFilter::Played,
                },
                music_folder_id: current.music_folder_id.clone(),
                variation: random_u64(),
            },
        )
        .await;
        complete_materialization(playback, reservation, placement, tracks, Provenance::Random)
            .await;
    }))
}

async fn compose_radio(
    loaded: Arc<Library>,
    request: RadioComposition,
) -> Result<Vec<Track>, String> {
    tokio::task::spawn_blocking(move || {
        loaded
            .compose_radio(request)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("radio composition worker failed: {error}"))?
}

async fn compose_random(
    loaded: Arc<Library>,
    request: RandomComposition,
) -> Result<Vec<Track>, String> {
    tokio::task::spawn_blocking(move || {
        let tracks = loaded
            .compose_random(request)
            .map_err(|error| error.to_string())?;
        if tracks.is_empty() {
            Err("no matching random tracks were found".to_string())
        } else {
            Ok(tracks)
        }
    })
    .await
    .map_err(|error| format!("random composition worker failed: {error}"))?
}

async fn complete_materialization(
    playback: Playback,
    reservation: playback::MaterializationReservation,
    placement: Placement,
    tracks: Result<Vec<Track>, String>,
    provenance: Provenance,
) {
    let _ = tokio::task::spawn_blocking(move || match tracks {
        Ok(tracks) if !tracks.is_empty() => {
            let batch = Batch::new(
                tracks
                    .into_iter()
                    .map(|track| BatchItem::new(track, provenance.clone()))
                    .collect(),
            );
            playback.complete_materialization(
                reservation.id,
                reservation.source_id,
                batch,
                placement,
            )
        }
        Ok(_) => playback
            .fail_materialization(
                reservation.id,
                reservation.source_id,
                placement,
                "no matching tracks were found".to_string(),
            )
            .map(|_| false),
        Err(error) => playback
            .fail_materialization(reservation.id, reservation.source_id, placement, error)
            .map(|_| false),
    })
    .await;
}
