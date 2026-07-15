use library::Track;
use playback::{Provenance, RandomPlayRequest};
use sources::{RandomTrackDomain, RandomTrackRequest};
use tracing::warn;

use crate::source_setup::{current_active_source, selected_active_source};

use super::{root::PlaybackCommands, source_tracks::hydrate_source_tracks};

impl PlaybackCommands {
    pub(crate) fn random_track_domain(&self) -> Option<RandomTrackDomain> {
        current_active_source(&self.active_source).map(|active| active.random_tracks.domain)
    }

    pub fn play_random(&self, request: RandomPlayRequest) {
        let placement = request.placement.into();
        let reservation = match self.reserve_queue_materialization(placement) {
            Ok(reservation) => reservation,
            Err(error) => {
                self.emit_random_error(error);
                return;
            }
        };
        let controller = self.clone();
        let rejected = reservation.clone();
        if let Err(error) = self.submit_playback_materialization(move || {
            match controller.random_tracks_for_request(reservation.source_id(), &request) {
                Ok(tracks) if tracks.is_empty() => controller.reject_queue_materialization(
                    reservation,
                    "No matching random tracks were found.",
                ),
                Ok(tracks) => {
                    if let Err(error) = controller.apply_reserved_tracks(
                        reservation.clone(),
                        tracks,
                        Provenance::Random,
                    ) {
                        controller.reject_queue_materialization(reservation, error);
                    }
                }
                Err(error) => controller.reject_queue_materialization(reservation, error),
            }
        }) {
            self.reject_queue_materialization(rejected, error);
        }
    }

    fn random_tracks_for_request(
        &self,
        source_id: &library::SourceId,
        request: &RandomPlayRequest,
    ) -> Result<Vec<Track>, String> {
        let saved = self
            .store
            .with_store(|store| store.stored_source(source_id))?
            .ok_or_else(|| "The reserved music source is no longer saved.".to_string())?;
        let active = selected_active_source(&self.active_source, source_id)?;
        let source_request = RandomTrackRequest {
            limit: request.limit,
            min_year: request.min_year,
            max_year: request.max_year,
            genre_id: request.genre_id.clone(),
            genre_name: request.genre_name.clone(),
            played_filter: request.played_filter,
        };
        let mut tracks = active.random_tracks.random_tracks(
            &self.store,
            &self.runtime,
            source_id,
            source_request,
        )?;
        hydrate_source_tracks(&self.store, &saved.source_id, &mut tracks)?;
        Ok(tracks)
    }

    fn emit_random_error(&self, error: impl Into<String>) {
        let error = error.into();
        warn!(%error, "random play request failed");
    }
}
