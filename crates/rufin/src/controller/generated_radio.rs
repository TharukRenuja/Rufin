use std::collections::HashSet;
use std::thread;
use std::time::Instant;

use domain::{
    Album, Artist, GeneratedTrackSeed, GeneratedTracksRequest, Genre, Playlist, ServerId, Track,
    TrackId,
};
use library::SavedServer;
use tracing::info;

use super::{
    AppController, ControllerEvent, RandomPlayAction, SNAPSHOT_TRACK_LIMIT,
    load_settings_for_saved, provider_for_saved,
    provider_tracks::{generated_track_strategy_for_saved, prepare_provider_tracks},
    root::local_source_saved,
};

const GENERATED_RADIO_ITEM_COUNT: usize = 20;

#[derive(Clone, Debug)]
struct GeneratedRadioRequest {
    action: RandomPlayAction,
    seed: GeneratedTrackSeed,
    seed_track: Option<Track>,
    limit: usize,
}

impl AppController {
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
            move || match controller.generated_tracks_for_active_server(&request) {
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

    fn generated_tracks_for_active_server(
        &self,
        request: &GeneratedRadioRequest,
    ) -> Result<Vec<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
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
        saved: &SavedServer,
        seed: GeneratedTrackSeed,
        limit: usize,
    ) -> Result<Vec<Track>, String> {
        let settings = load_settings_for_saved(&self.store, saved);
        let mut tracks = if saved.server.provider == "fake" {
            self.generated_tracks_from_cache(&saved.server.id, seed, limit)?
        } else {
            let provider = provider_for_saved(&self.store, &self.runtime, &self.secrets, saved)?;
            self.runtime
                .block_on(
                    provider
                        .as_music_provider()
                        .generated_tracks(GeneratedTracksRequest {
                            seed,
                            limit,
                            strategy: generated_track_strategy_for_saved(saved),
                        }),
                )
                .map_err(|error| error.to_string())?
        };
        dedupe_tracks(&mut tracks);
        prepare_provider_tracks(self, saved, &settings, &mut tracks)?;
        Ok(tracks)
    }

    fn generated_tracks_from_cache(
        &self,
        server_id: &ServerId,
        seed: GeneratedTrackSeed,
        limit: usize,
    ) -> Result<Vec<Track>, String> {
        let tracks = self
            .store
            .with_store(|store| store.load_tracks(server_id, 0, SNAPSHOT_TRACK_LIMIT))?
            .items;
        let mut candidates = match seed {
            GeneratedTrackSeed::Track(track_id) => {
                let seed = self
                    .store
                    .with_store(|store| store.load_track(server_id, &track_id))?
                    .ok_or_else(|| "The selected track is no longer available.".to_string())?;
                tracks
                    .into_iter()
                    .filter(|track| track.id != seed.id)
                    .filter(|track| {
                        track.artist_id == seed.artist_id
                            || track.album_id == seed.album_id
                            || track.genres.iter().any(|genre| seed.genres.contains(genre))
                    })
                    .collect::<Vec<_>>()
            }
            GeneratedTrackSeed::Album(album_id) => tracks
                .into_iter()
                .filter(|track| track.album_id != album_id)
                .collect(),
            GeneratedTrackSeed::Artist(artist_id) => tracks
                .into_iter()
                .filter(|track| track.artist_id.as_ref() == Some(&artist_id))
                .collect(),
            GeneratedTrackSeed::Genre { id: _, name } => tracks
                .into_iter()
                .filter(|track| track.genres.iter().any(|genre| genre == &name))
                .collect(),
            GeneratedTrackSeed::Playlist(_) => {
                return Err("Playlist radio is not supported for this source.".to_string());
            }
        };
        candidates.sort_by_key(|track| track.id.as_str().to_string());
        Ok(candidates.into_iter().take(limit.clamp(1, 500)).collect())
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
    server_id: &ServerId,
) -> Result<Option<SavedServer>, String> {
    let saved = controller
        .store
        .with_store(|store| store.saved_server(server_id))?;
    Ok(saved.or_else(|| {
        (server_id.as_str() == super::root::LOCAL_SOURCE_SERVER_ID).then(local_source_saved)
    }))
}
