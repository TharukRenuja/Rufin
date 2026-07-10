mod local_cache;

use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use domain::{
    Album, Artist, GeneratedTrackSeed, GeneratedTrackStrategy, GeneratedTracksRequest, Genre,
    Playlist, SourceId, Track, TrackId,
};
use library::SavedSource;
use tracing::info;

use super::{
    AppController, ControllerEvent, RandomPlayAction, load_settings_from_store,
    source_tracks::{prepare_cached_tracks, prepare_source_tracks},
};
use crate::sources::{
    GeneratedTrackExecutor, GeneratedTracks, current_active_source, selected_active_source,
};

use local_cache::local_generated_tracks_from_cache;
pub(in crate::controller) use local_cache::spread_radio_tracks;

const GENERATED_RADIO_ITEM_COUNT: usize = 20;

pub(crate) fn cached_generated_track_executor(source_id: SourceId) -> GeneratedTrackExecutor {
    Arc::new(move |store, _runtime, saved, settings, seed, limit| {
        let mut tracks = local_generated_tracks_from_cache(store, &source_id, seed, limit)?;
        dedupe_tracks(&mut tracks);
        prepare_cached_tracks(store, saved, settings, &mut tracks)?;
        Ok(tracks)
    })
}

pub(crate) fn native_generated_track_executor(
    executor: GeneratedTracks,
    strategy: GeneratedTrackStrategy,
) -> GeneratedTrackExecutor {
    Arc::new(move |store, runtime, saved, settings, seed, limit| {
        let mut tracks = runtime
            .block_on(executor.generated_tracks(GeneratedTracksRequest {
                seed,
                limit,
                strategy,
            }))
            .map_err(|error| error.to_string())?;
        dedupe_tracks(&mut tracks);
        prepare_source_tracks(store, saved, settings, &mut tracks)?;
        Ok(tracks)
    })
}

#[derive(Clone, Debug)]
struct GeneratedRadioRequest {
    action: RandomPlayAction,
    seed: GeneratedTrackSeed,
    seed_track: Option<Track>,
    limit: usize,
}

impl AppController {
    pub fn manual_radio_supported(&self, kind: domain::GeneratedTrackSeedKind) -> bool {
        current_active_source(&self.active_source)
            .is_some_and(|active| active.manual_radio.seed_domain.contains(&kind))
    }

    pub fn play_track_radio(&self, track: Track) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNow,
            seed: GeneratedTrackSeed::Track(track.id.clone()),
            seed_track: Some(track),
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_track_radio_next(&self, track: Track) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNext,
            seed: GeneratedTrackSeed::Track(track.id.clone()),
            seed_track: Some(track),
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_track_radio_last(&self, track: Track) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::AddLast,
            seed: GeneratedTrackSeed::Track(track.id.clone()),
            seed_track: Some(track),
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_album_radio(&self, album: Album) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNow,
            seed: GeneratedTrackSeed::Album(album.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_album_radio_next(&self, album: Album) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNext,
            seed: GeneratedTrackSeed::Album(album.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_album_radio_last(&self, album: Album) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::AddLast,
            seed: GeneratedTrackSeed::Album(album.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_artist_radio(&self, artist: Artist) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNow,
            seed: GeneratedTrackSeed::Artist(artist.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_artist_radio_next(&self, artist: Artist) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNext,
            seed: GeneratedTrackSeed::Artist(artist.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_artist_radio_last(&self, artist: Artist) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::AddLast,
            seed: GeneratedTrackSeed::Artist(artist.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_genre_radio(&self, genre: Genre) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNow,
            seed: GeneratedTrackSeed::Genre {
                id: Some(genre.id),
                name: genre.name,
            },
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_genre_radio_next(&self, genre: Genre) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNext,
            seed: GeneratedTrackSeed::Genre {
                id: Some(genre.id),
                name: genre.name,
            },
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_genre_radio_last(&self, genre: Genre) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::AddLast,
            seed: GeneratedTrackSeed::Genre {
                id: Some(genre.id),
                name: genre.name,
            },
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_playlist_radio(&self, playlist: Playlist) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNow,
            seed: GeneratedTrackSeed::Playlist(playlist.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_playlist_radio_next(&self, playlist: Playlist) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::PlayNext,
            seed: GeneratedTrackSeed::Playlist(playlist.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    pub fn play_playlist_radio_last(&self, playlist: Playlist) {
        self.play_generated_radio(GeneratedRadioRequest {
            action: RandomPlayAction::AddLast,
            seed: GeneratedTrackSeed::Playlist(playlist.id),
            seed_track: None,
            limit: GENERATED_RADIO_ITEM_COUNT,
        });
    }

    fn play_generated_radio(&self, request: GeneratedRadioRequest) {
        let generation = self.next_play_activation_generation();
        let controller = self.clone();
        let seed_kind = generated_seed_kind(&request.seed);
        let started = Instant::now();
        info!(
            seed_kind,
            limit = request.limit,
            "started generated radio load"
        );
        thread::spawn(
            move || match controller.generated_tracks_for_active_source(&request) {
                Ok(tracks) => {
                    if controller.play_activation_generation_matches(generation) {
                        info!(
                            seed_kind,
                            tracks = tracks.len(),
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            "loaded generated radio tracks"
                        );
                        controller.apply_generated_radio(request.action, tracks);
                    }
                }
                Err(error) => {
                    if controller.play_activation_generation_matches(generation) {
                        controller.emit_generated_radio_error(error);
                    }
                }
            },
        );
    }

    fn generated_tracks_for_active_source(
        &self,
        request: &GeneratedRadioRequest,
    ) -> Result<Vec<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_source())? else {
            return Err("No active music server is saved.".to_string());
        };
        let mut tracks =
            self.generated_tracks_for_saved(&saved, request.seed.clone(), request.limit)?;
        if let Some(seed_track) = request.seed_track.as_ref() {
            let seed_id = seed_track.id.clone();
            tracks.retain(|track| track.id != seed_id);
            if tracks.is_empty() {
                return Ok(Vec::new());
            }
            tracks.insert(0, seed_track.clone());
        }
        dedupe_tracks(&mut tracks);
        Ok(tracks)
    }

    pub(in crate::controller) fn generated_tracks_for_saved(
        &self,
        saved: &SavedSource,
        seed: GeneratedTrackSeed,
        limit: usize,
    ) -> Result<Vec<Track>, String> {
        let settings = load_settings_from_store(&self.store);
        let active = selected_active_source(&self.active_source, &saved.source.id)?;
        if !active.manual_radio.accepts(&seed) {
            return Err("Radio is not available for this item from the active source.".to_string());
        }
        (active.manual_radio.executor)(&self.store, &self.runtime, saved, &settings, seed, limit)
    }

    fn apply_generated_radio(&self, action: RandomPlayAction, tracks: Vec<Track>) {
        if tracks.is_empty() {
            self.emit_generated_radio_error("No matching radio tracks were found.");
            return;
        }
        match action {
            RandomPlayAction::PlayNow => self.play_tracks_now(tracks),
            RandomPlayAction::PlayNext => self.play_generated_radio_next(tracks),
            RandomPlayAction::AddLast => self.append_generated_radio(tracks),
        }
    }

    fn play_generated_radio_next(&self, tracks: Vec<Track>) {
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
            self.emit_generated_radio_error(error);
            return;
        }
        self.persist_and_emit_queue();
    }

    fn append_generated_radio(&self, tracks: Vec<Track>) {
        let result = self.with_queue_mut(|queue| {
            for track in &tracks {
                queue.append(track);
            }
            Ok(())
        });
        if let Err(error) = result {
            self.emit_generated_radio_error(error);
            return;
        }
        self.persist_and_emit_queue();
    }

    fn emit_generated_radio_error(&self, error: impl Into<String>) {
        let _sent = self.events.send(ControllerEvent::Error(error.into()));
    }
}

fn dedupe_tracks(tracks: &mut Vec<Track>) {
    let mut seen = HashSet::<TrackId>::new();
    tracks.retain(|track| seen.insert(track.id.clone()));
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

pub(in crate::controller) fn saved_server_for_generated_queue(
    controller: &AppController,
    source_id: &SourceId,
) -> Result<Option<SavedSource>, String> {
    controller
        .store
        .with_store(|store| store.saved_source(source_id))
}
