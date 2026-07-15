mod local_cache;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use library::StoredSource;
use library::{SourceId, Track, TrackId};
use playback::{Placement, Provenance, RadioPlayRequest, RadioSeed};
use sources::{GeneratedTrackSeed, GeneratedTrackStrategy, GeneratedTracksRequest};
use tracing::{info, warn};

use super::{
    load_settings_from_store, root::PlaybackCommands, source_tracks::hydrate_source_tracks,
};
use crate::source_setup::{
    GeneratedTrackExecutor, GeneratedTracks, current_active_source, selected_active_source,
};

use local_cache::local_generated_tracks_from_cache;
pub(in crate::controller) use local_cache::spread_radio_tracks;

const GENERATED_RADIO_ITEM_COUNT: usize = 20;

pub(crate) fn cached_generated_track_executor(source_id: SourceId) -> GeneratedTrackExecutor {
    Arc::new(move |store, _runtime, _saved, _settings, seed, limit| {
        let mut tracks = local_generated_tracks_from_cache(store, &source_id, seed, limit)?;
        dedupe_tracks(&mut tracks);
        Ok(tracks)
    })
}

pub(crate) fn native_generated_track_executor(
    executor: GeneratedTracks,
    strategy: GeneratedTrackStrategy,
) -> GeneratedTrackExecutor {
    Arc::new(move |store, runtime, saved, _settings, seed, limit| {
        let mut tracks = runtime
            .block_on(executor.generated_tracks(GeneratedTracksRequest {
                seed,
                limit,
                strategy,
            }))
            .map_err(|error| error.to_string())?;
        dedupe_tracks(&mut tracks);
        hydrate_source_tracks(store, &saved.source_id, &mut tracks)?;
        Ok(tracks)
    })
}

#[derive(Clone, Debug)]
struct GeneratedRadioRequest {
    placement: Placement,
    seed: GeneratedTrackSeed,
    seed_track: Option<Track>,
    limit: usize,
}

impl PlaybackCommands {
    pub fn manual_radio_supported(&self, kind: sources::GeneratedTrackSeedKind) -> bool {
        current_active_source(&self.active_source)
            .is_some_and(|active| active.manual_radio.seed_domain.contains(&kind))
    }

    pub fn play_radio(&self, request: RadioPlayRequest) {
        let (seed, seed_track) = generated_radio_seed(request.seed);
        self.play_generated_radio(GeneratedRadioRequest {
            placement: request.placement.into(),
            seed,
            seed_track,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    fn play_generated_radio(&self, request: GeneratedRadioRequest) {
        let placement = request.placement;
        let reservation = match self.reserve_queue_materialization(placement) {
            Ok(reservation) => reservation,
            Err(error) => {
                self.emit_generated_radio_error(error);
                return;
            }
        };
        let controller = self.clone();
        let seed_kind = generated_seed_kind(&request.seed);
        let started = Instant::now();
        info!(
            seed_kind,
            limit = request.limit,
            "started generated radio load"
        );
        let rejected = reservation.clone();
        if let Err(error) = self.submit_playback_materialization(move || {
            match controller.generated_tracks_for_source(
                reservation.source_id(),
                &request,
                reservation.current_track_id(),
            ) {
                Ok(tracks) if tracks.is_empty() => controller.reject_queue_materialization(
                    reservation,
                    "No matching radio tracks were found.",
                ),
                Ok(tracks) => {
                    info!(
                        seed_kind,
                        tracks = tracks.len(),
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "loaded generated radio tracks"
                    );
                    if let Err(error) = controller.apply_reserved_tracks(
                        reservation.clone(),
                        tracks,
                        Provenance::Radio,
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

    fn generated_tracks_for_source(
        &self,
        source_id: &SourceId,
        request: &GeneratedRadioRequest,
        current_track_id: Option<&TrackId>,
    ) -> Result<Vec<Track>, String> {
        let saved = self
            .store
            .with_store(|store| store.stored_source(source_id))?
            .ok_or_else(|| "The reserved music source is no longer saved.".to_string())?;
        let mut tracks =
            self.generated_tracks_for_saved(&saved, request.seed.clone(), request.limit)?;
        if let Some(seed_track) = request.seed_track.as_ref() {
            let seed_id = seed_track.id.clone();
            tracks.retain(|track| track.id != seed_id);
            if current_track_id != Some(&seed_id) {
                tracks.insert(0, seed_track.clone());
            }
        }
        dedupe_tracks(&mut tracks);
        Ok(tracks)
    }

    pub(in crate::controller) fn generated_tracks_for_saved(
        &self,
        saved: &StoredSource,
        seed: GeneratedTrackSeed,
        limit: usize,
    ) -> Result<Vec<Track>, String> {
        let settings = load_settings_from_store(&self.store);
        let active = selected_active_source(&self.active_source, &saved.source_id)?;
        if !active.manual_radio.accepts(&seed) {
            return Err("Radio is not available for this item from the active source.".to_string());
        }
        (active.manual_radio.executor)(&self.store, &self.runtime, saved, &settings, seed, limit)
    }

    fn emit_generated_radio_error(&self, error: impl Into<String>) {
        let error = error.into();
        warn!(%error, "generated radio request failed");
    }
}

fn dedupe_tracks(tracks: &mut Vec<Track>) {
    let mut seen = HashSet::<TrackId>::new();
    tracks.retain(|track| seen.insert(track.id.clone()));
}

fn generated_radio_seed(seed: RadioSeed) -> (GeneratedTrackSeed, Option<Track>) {
    match seed {
        RadioSeed::Track(track) => (GeneratedTrackSeed::Track(track.id.clone()), Some(track)),
        RadioSeed::Album(album) => (GeneratedTrackSeed::Album(album.id), None),
        RadioSeed::Artist(artist) => (GeneratedTrackSeed::Artist(artist.id), None),
        RadioSeed::Genre(genre) => (
            GeneratedTrackSeed::Genre {
                id: Some(genre.id),
                name: genre.name,
            },
            None,
        ),
        RadioSeed::Playlist(playlist) => (GeneratedTrackSeed::Playlist(playlist.id), None),
    }
}

fn generated_seed_kind(seed: &GeneratedTrackSeed) -> &'static str {
    match seed {
        GeneratedTrackSeed::Track(_) => "track",
        GeneratedTrackSeed::Album(_) => "album",
        GeneratedTrackSeed::Artist(_) => "artist",
        GeneratedTrackSeed::Genre { .. } => "genre",
        GeneratedTrackSeed::Playlist(_) => "playlist",
    }
}
