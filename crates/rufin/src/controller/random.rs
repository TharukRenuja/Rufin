use std::thread;

use domain::{GenreId, Track};
use source::{PlayedFilter, RandomTrackRequest};

use super::{
    AppController, ControllerEvent, SNAPSHOT_TRACK_LIMIT, load_settings_for_saved,
    provider_for_saved, provider_tracks::prepare_provider_tracks,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RandomPlayAction {
    PlayNext,
    PlayNow,
    AddLast,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RandomPlayRequest {
    pub action: RandomPlayAction,
    pub limit: usize,
    pub min_year: Option<u16>,
    pub max_year: Option<u16>,
    pub genre_id: Option<GenreId>,
    pub genre_name: Option<String>,
    pub played_filter: PlayedFilter,
}

impl AppController {
    pub fn play_random_tracks(&self, request: RandomPlayRequest) {
        let generation = self.next_play_activation_generation();
        let controller = self.clone();
        thread::spawn(
            move || match controller.random_tracks_for_request(&request) {
                Ok(tracks) => {
                    if controller.play_activation_generation_matches(generation) {
                        controller.apply_random_tracks(request.action, tracks);
                    }
                }
                Err(error) => {
                    if controller.play_activation_generation_matches(generation) {
                        controller.emit_random_error(error);
                    }
                }
            },
        );
    }

    fn random_tracks_for_request(&self, request: &RandomPlayRequest) -> Result<Vec<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Err("No active music server is saved.".to_string());
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        let mut tracks = if saved.server.provider == "fake" {
            self.random_tracks_from_cache(&saved.server.id, request)?
        } else {
            let provider = provider_for_saved(&self.store, &self.runtime, &self.secrets, &saved)?;
            self.runtime
                .block_on(
                    provider
                        .as_music_provider()
                        .random_tracks(RandomTrackRequest {
                            limit: request.limit,
                            min_year: request.min_year,
                            max_year: request.max_year,
                            genre_id: request.genre_id.clone(),
                            genre_name: request.genre_name.clone(),
                            played_filter: request.played_filter,
                        }),
                )
                .map_err(|error| error.to_string())?
        };
        prepare_provider_tracks(self, &saved, &settings, &mut tracks)?;
        Ok(tracks)
    }

    fn random_tracks_from_cache(
        &self,
        server_id: &domain::ServerId,
        request: &RandomPlayRequest,
    ) -> Result<Vec<Track>, String> {
        let mut tracks = self
            .store
            .with_store(|store| store.load_tracks(server_id, 0, SNAPSHOT_TRACK_LIMIT))?
            .items
            .into_iter()
            .filter(|track| {
                request.min_year.is_none_or(|year| track.year >= year)
                    && request.max_year.is_none_or(|year| track.year <= year)
                    && request.genre_name.as_ref().is_none_or(|genre| {
                        track.genres.iter().any(|track_genre| track_genre == genre)
                    })
                    && match request.played_filter {
                        PlayedFilter::All => true,
                        PlayedFilter::Unplayed => track.last_played.is_none(),
                        PlayedFilter::Played => track.last_played.is_some(),
                    }
            })
            .collect::<Vec<_>>();
        tracks.sort_by_key(|track| track.id.as_str().to_string());
        Ok(tracks
            .into_iter()
            .take(request.limit.clamp(1, 500))
            .collect())
    }

    fn apply_random_tracks(&self, action: RandomPlayAction, tracks: Vec<Track>) {
        if tracks.is_empty() {
            self.emit_random_error("No matching random tracks were found.");
            return;
        }
        match action {
            RandomPlayAction::PlayNow => self.play_tracks_now(tracks),
            RandomPlayAction::PlayNext => self.play_random_tracks_next(tracks),
            RandomPlayAction::AddLast => self.append_random_tracks(tracks),
        }
    }

    fn play_random_tracks_next(&self, tracks: Vec<Track>) {
        let result = self.with_queue_mut(|queue| {
            if queue.current().is_some() {
                for track in tracks.iter().rev() {
                    queue.play_next(track);
                }
            } else {
                for track in &tracks {
                    queue.append(track);
                }
            }
            Ok(())
        });
        if let Err(error) = result {
            self.emit_random_error(error);
            return;
        }
        self.persist_and_emit_queue();
    }

    fn append_random_tracks(&self, tracks: Vec<Track>) {
        let result = self.with_queue_mut(|queue| {
            for track in &tracks {
                queue.append(track);
            }
            Ok(())
        });
        if let Err(error) = result {
            self.emit_random_error(error);
            return;
        }
        self.persist_and_emit_queue();
    }

    fn emit_random_error(&self, error: impl Into<String>) {
        let _sent = self.events.send(ControllerEvent::Error(error.into()));
    }
}
