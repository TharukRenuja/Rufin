use std::thread;

use domain::{GenreId, Track};
use source::{PlayedFilter, RandomTrackRequest};

use crate::sources::{RandomTrackDomain, current_active_source, selected_active_source};

use super::{
    AppController, ControllerEvent, load_settings_from_store, source_tracks::prepare_source_tracks,
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
    pub(crate) fn random_track_domain(&self) -> Option<RandomTrackDomain> {
        current_active_source(&self.active_source).map(|active| active.random_tracks.domain)
    }

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
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Err("No active music server is saved.".to_string());
        };
        let settings = load_settings_from_store(&self.store);
        let active = selected_active_source(&self.active_source, &saved.source.id)?;
        let source_request = RandomTrackRequest {
            limit: request.limit,
            min_year: request.min_year,
            max_year: request.max_year,
            genre_id: request.genre_id.clone(),
            genre_name: request.genre_name.clone(),
            played_filter: request.played_filter,
        };
        active
            .random_tracks
            .domain
            .validate(&source_request)
            .map_err(str::to_string)?;
        let mut tracks = self
            .runtime
            .block_on(active.random_tracks.executor.random_tracks(source_request))
            .map_err(|error| error.to_string())?;
        prepare_source_tracks(&self.store, &saved, &settings, &mut tracks)?;
        Ok(tracks)
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
