use super::*;
use crate::controller::generated_radio::spread_radio_tracks;
use crate::controller::source_tracks::hydrate_source_tracks;
use crate::source_setup::{
    AutoDjCandidateOperation, AutoDjFallbackExecutor, GeneratedTrackExecutor, RandomTrackOperation,
};
use sources::{PlayedFilter, RandomTrackRequest};

pub(crate) fn cached_auto_dj_operation(
    source_id: SourceId,
    generated: GeneratedTrackExecutor,
) -> AutoDjCandidateOperation {
    let fallback: AutoDjFallbackExecutor = Arc::new(
        move |store, _runtime, _saved, _settings, genre_name, limit, current_track_id| {
            let tracks = if let Some(genre_name) = genre_name.as_deref() {
                store.with_store(|store| {
                    store.load_tracks_by_genre_name(&source_id, genre_name, limit)
                })?
            } else {
                store
                    .with_store(|store| store.load_tracks(&source_id, 0, limit))?
                    .items
            };
            Ok(spread_radio_tracks(
                &format!("auto-dj:{}", current_track_id.as_str()),
                tracks,
            ))
        },
    );
    AutoDjCandidateOperation {
        generated,
        fallback,
    }
}

pub(crate) fn native_auto_dj_operation(
    generated: GeneratedTrackExecutor,
    random: RandomTrackOperation,
) -> AutoDjCandidateOperation {
    let fallback: AutoDjFallbackExecutor = Arc::new(
        move |store, runtime, saved, _settings, genre_name, limit, _current_track_id| {
            let request = RandomTrackRequest {
                limit,
                min_year: None,
                max_year: None,
                genre_id: None,
                genre_name,
                played_filter: PlayedFilter::All,
            };
            let mut tracks = random.random_tracks(store, runtime, &saved.source_id, request)?;
            hydrate_source_tracks(store, &saved.source_id, &mut tracks)?;
            Ok(tracks)
        },
    );
    AutoDjCandidateOperation {
        generated,
        fallback,
    }
}
