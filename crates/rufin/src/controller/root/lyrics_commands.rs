use super::*;

use std::sync::OnceLock;

use metadata::{
    LocalLyricsInput, LyricsCacheUpdate, LyricsRequestKind, ResolveLyrics, decode_cached_lyrics,
    encode_cached_lyrics,
};

pub(super) fn save_cached_lyrics(
    store: &StoreHandle,
    source_id: &SourceId,
    lyrics: &Lyrics,
) -> Result<(), String> {
    let (origin, payload) = encode_cached_lyrics(lyrics)?;
    store.with_store(|store| {
        store.save_lyrics_payload(source_id, &lyrics.track_id, origin, &payload)
    })
}

pub(super) fn load_cached_lyrics(
    store: &StoreHandle,
    source_id: &SourceId,
    track_id: &TrackId,
) -> Result<Option<Lyrics>, String> {
    store
        .with_store(|store| store.load_lyrics_payload(source_id, track_id))?
        .map(|payload| decode_cached_lyrics(&payload))
        .transpose()
}

pub(super) fn delete_remote_cached_lyrics(
    store: &StoreHandle,
    source_id: &SourceId,
    track_id: &TrackId,
) -> Result<bool, String> {
    store.with_store(|store| {
        store.delete_lyrics_payload(source_id, track_id, metadata::REMOTE_LYRICS_ORIGIN)
    })
}

fn lyrics_event(
    media_key: &playback::MediaKey,
    generation: u64,
    lyrics: Option<Lyrics>,
) -> ControllerEvent {
    ControllerEvent::Lyrics {
        media_key: media_key.clone(),
        generation,
        lyrics: Box::new(lyrics),
    }
}

fn lyrics_search_result_allowed(settings: &StoredSettings, result: &LyricsSearchResult) -> bool {
    settings
        .metadata
        .external_lyrics_allowed(settings.private_mode)
        && settings
            .metadata
            .external_lyrics_providers
            .contains(&result.provider)
}

pub(super) fn metadata_runner() -> Result<&'static BoundedRunner, String> {
    static RUNNER: OnceLock<Result<BoundedRunner, String>> = OnceLock::new();
    match RUNNER.get_or_init(|| BoundedRunner::new("Metadata lookup", "rufin-metadata", 4)) {
        Ok(runner) => Ok(runner),
        Err(error) => Err(error.clone()),
    }
}

fn cue_track(store: &StoreHandle, source_id: &SourceId, track_id: &TrackId) -> bool {
    store
        .with_store(|store| store.load_track_source_object(source_id, track_id))
        .ok()
        .flatten()
        .is_some_and(|source| source.source_object_kind == "cue_track")
}

fn local_lyrics_input(
    store: &StoreHandle,
    active: &ActiveSource,
    track: &Track,
    cue_track: bool,
) -> Option<LocalLyricsInput> {
    let audio_path = store
        .with_store(|store| (active.sidecar_file)(store, &active.identity.id, &track.id))
        .ok()
        .flatten()?;
    Some(LocalLyricsInput {
        audio_path,
        title: track.title.clone(),
        cue_track,
    })
}

fn apply_cache_update(
    store: &StoreHandle,
    source_id: &SourceId,
    track_id: &TrackId,
    update: &LyricsCacheUpdate,
) {
    let result = match update {
        LyricsCacheUpdate::None => Ok(()),
        LyricsCacheUpdate::Save(lyrics) => save_cached_lyrics(store, source_id, lyrics),
        LyricsCacheUpdate::DeleteRemote => {
            delete_remote_cached_lyrics(store, source_id, track_id).map(|_| ())
        }
    };
    if let Err(error) = result {
        warn!(%error, track_id = %track_id, "failed to update lyrics cache");
    }
}

impl AppController {
    pub fn request_lyrics_for_media(&self, media_key: playback::MediaKey, kind: LyricsRequestKind) {
        self.request_lyrics(media_key, true, kind);
    }

    pub fn refresh_lyrics_for_current(&self) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            return;
        };
        self.request_lyrics(
            playback::MediaKey {
                source_id,
                track_id: entry.track.id,
            },
            false,
            LyricsRequestKind::Configured,
        );
    }

    pub fn clear_remote_lyrics_for_current(&self) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            return;
        };
        let generation = self
            .lyrics_request_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        match delete_remote_cached_lyrics(&self.store, &source_id, &entry.track.id) {
            Ok(true) => {
                let media_key = playback::MediaKey {
                    source_id,
                    track_id: entry.track.id,
                };
                let _sent = self.events.send(lyrics_event(&media_key, generation, None));
            }
            Ok(false) => {}
            Err(error) => {
                warn!(%error, "failed to clear cached remote lyrics");
            }
        }
    }

    fn request_lyrics(
        &self,
        media_key: playback::MediaKey,
        use_cache: bool,
        kind: LyricsRequestKind,
    ) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            debug!("lyrics request skipped because the queue has no current track");
            return;
        };
        if source_id != media_key.source_id || entry.track.id != media_key.track_id {
            debug!(track_id = %media_key.track_id, "skipped stale lyrics request");
            return;
        }

        let settings = self.load_settings();
        let plan = settings.metadata.lyrics_plan(settings.private_mode, kind);
        let active = selected_active_source(&self.active_source, &source_id).ok();
        let cue_track = cue_track(&self.store, &source_id, &entry.track.id);
        let local = active
            .as_deref()
            .and_then(|active| local_lyrics_input(&self.store, active, &entry.track, cue_track));
        let cached = if use_cache {
            load_cached_lyrics(&self.store, &source_id, &entry.track.id).unwrap_or_else(|error| {
                warn!(%error, track_id = %entry.track.id, "failed to load cached lyrics");
                None
            })
        } else {
            None
        };
        let native = active.and_then(|active| active.lyrics.clone());
        let runtime = Arc::clone(&self.runtime);
        let store = self.store.clone();
        let events = self.events.clone();
        let generation = self
            .lyrics_request_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let current_generation = Arc::clone(&self.lyrics_request_generation);
        let track_id = entry.track.id.clone();
        let job_media_key = media_key.clone();
        let request = ResolveLyrics {
            track: entry.track,
            plan,
            use_cache,
            cue_track,
            local,
            cached,
        };
        let submit = metadata_runner().and_then(|runner| {
            runner.submit(move || {
                let resolution = metadata::resolve_lyrics(
                    request,
                    |search| match native {
                        Some(provider) => runtime
                            .block_on(provider.lyrics(&track_id, search))
                            .map_err(|error| error.to_string()),
                        None => Ok(None),
                    },
                    || current_generation.load(Ordering::Acquire) == generation,
                );
                if current_generation.load(Ordering::Acquire) != generation {
                    debug!(track_id = %track_id, "discarded stale lyrics result");
                    return;
                }
                apply_cache_update(&store, &source_id, &track_id, &resolution.cache);
                let _sent =
                    events.send(lyrics_event(&job_media_key, generation, resolution.lyrics));
            })
        });
        if let Err(error) = submit {
            warn!(%error, track_id = %media_key.track_id, "could not schedule metadata lookup");
            let _sent = self.events.send(lyrics_event(&media_key, generation, None));
        }
    }

    pub fn search_lyrics_for_current(&self, artist_name: String, track_name: String) {
        let artist_name = artist_name.trim().to_string();
        let track_name = track_name.trim().to_string();
        if artist_name.is_empty() && track_name.is_empty() {
            return;
        }
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            debug!("manual lyrics search skipped because no track is playing");
            return;
        };
        let media_key = playback::MediaKey {
            source_id,
            track_id: entry.track.id,
        };
        let generation = self.lyrics_request_generation.load(Ordering::Acquire);
        let settings = self.load_settings();
        if !settings
            .metadata
            .external_lyrics_allowed(settings.private_mode)
        {
            let _sent = self.events.send(ControllerEvent::LyricsSearchResults {
                media_key,
                generation,
                artist_name,
                track_name,
                results: Vec::new(),
            });
            return;
        }
        let providers = settings.metadata.external_lyrics_providers;
        let events = self.events.clone();
        let settings_store = self.store.clone();
        let result_media_key = media_key.clone();
        let rejected_artist_name = artist_name.clone();
        let rejected_track_name = track_name.clone();
        let submit = metadata_runner().and_then(|runner| {
            runner.submit(move || {
                let current = load_settings_from_store(&settings_store);
                let active_providers = providers
                    .into_iter()
                    .filter(|provider| {
                        current
                            .metadata
                            .external_lyrics_providers
                            .contains(provider)
                    })
                    .collect::<Vec<_>>();
                if !current
                    .metadata
                    .external_lyrics_allowed(current.private_mode)
                    || active_providers.is_empty()
                {
                    let _sent = events.send(ControllerEvent::LyricsSearchResults {
                        media_key: result_media_key,
                        generation,
                        artist_name,
                        track_name,
                        results: Vec::new(),
                    });
                    return;
                }
                match metadata::search_lyrics(&active_providers, &artist_name, &track_name) {
                    Ok(mut results) => {
                        let current = load_settings_from_store(&settings_store);
                        if current
                            .metadata
                            .external_lyrics_allowed(current.private_mode)
                        {
                            results.retain(|result| {
                                current
                                    .metadata
                                    .external_lyrics_providers
                                    .contains(&result.provider)
                            });
                        } else {
                            results.clear();
                        }
                        let _sent = events.send(ControllerEvent::LyricsSearchResults {
                            media_key: result_media_key,
                            generation,
                            artist_name,
                            track_name,
                            results,
                        });
                    }
                    Err(error) => {
                        debug!(%error, "manual external lyric search failed");
                        let current = load_settings_from_store(&settings_store);
                        let event = if current
                            .metadata
                            .external_lyrics_allowed(current.private_mode)
                        {
                            ControllerEvent::LyricsSearchFailed {
                                media_key: result_media_key,
                                generation,
                                artist_name,
                                track_name,
                                error,
                            }
                        } else {
                            ControllerEvent::LyricsSearchResults {
                                media_key: result_media_key,
                                generation,
                                artist_name,
                                track_name,
                                results: Vec::new(),
                            }
                        };
                        let _sent = events.send(event);
                    }
                }
            })
        });
        if let Err(error) = submit {
            warn!(%error, "could not schedule manual lyrics search");
            let _sent = self.events.send(ControllerEvent::LyricsSearchFailed {
                media_key,
                generation,
                artist_name: rejected_artist_name,
                track_name: rejected_track_name,
                error,
            });
        }
    }

    pub fn save_lyrics_search_result(
        &self,
        media_key: playback::MediaKey,
        result: LyricsSearchResult,
        output_path: PathBuf,
    ) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            debug!("lyrics save skipped because no track is playing");
            return;
        };
        if source_id != media_key.source_id || entry.track.id != media_key.track_id {
            debug!("lyrics save skipped because the playing track changed");
            return;
        }
        if !lyrics_search_result_allowed(&self.load_settings(), &result) {
            debug!("lyrics save skipped because its provider is no longer enabled");
            return;
        }
        let store = self.store.clone();
        let events = self.events.clone();
        let generation = self
            .lyrics_request_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let current_generation = Arc::clone(&self.lyrics_request_generation);
        let track_id = entry.track.id;
        let result_media_key = media_key.clone();
        let submit = metadata_runner().and_then(|runner| {
            runner.submit(move || {
                if current_generation.load(Ordering::Acquire) != generation {
                    debug!("discarded stale lyrics save request");
                    return;
                }
                let result = metadata::save_lyrics_search_result(track_id, &result, output_path);
                if current_generation.load(Ordering::Acquire) != generation {
                    debug!("discarded stale saved lyrics result");
                    return;
                }
                match result {
                    Ok(Some((path, lyrics))) => {
                        if let Err(error) = save_cached_lyrics(&store, &source_id, &lyrics) {
                            warn!(%error, "failed to cache saved lyrics");
                        }
                        let _sent = events.send(ControllerEvent::LyricsSaved {
                            media_key: result_media_key,
                            generation,
                            path,
                            lyrics,
                        });
                    }
                    Ok(None) => {
                        let _sent = events.send(lyrics_event(&media_key, generation, None));
                    }
                    Err(error) => {
                        warn!(%error, "failed to save lyrics file");
                    }
                }
            })
        });
        if let Err(error) = submit {
            warn!(%error, "could not schedule lyrics save");
        }
    }

    pub fn preview_lyrics_search_result(
        &self,
        media_key: playback::MediaKey,
        result: LyricsSearchResult,
    ) {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            debug!("lyrics preview skipped because no track is playing");
            return;
        };
        if source_id != media_key.source_id || entry.track.id != media_key.track_id {
            debug!("lyrics preview skipped because the playing track changed");
            return;
        }
        if !lyrics_search_result_allowed(&self.load_settings(), &result) {
            debug!("lyrics preview skipped because its provider is no longer enabled");
            return;
        }
        let store = self.store.clone();
        let events = self.events.clone();
        let generation = self
            .lyrics_request_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let current_generation = Arc::clone(&self.lyrics_request_generation);
        let track_id = entry.track.id;
        let submit = metadata_runner().and_then(|runner| {
            runner.submit(move || {
                if current_generation.load(Ordering::Acquire) != generation {
                    debug!("discarded stale lyrics preview request");
                    return;
                }
                let result = metadata::lyrics_from_search_result(track_id, &result);
                if current_generation.load(Ordering::Acquire) != generation {
                    debug!("discarded stale lyrics preview");
                    return;
                }
                match result {
                    Ok(Some(lyrics)) => {
                        if let Err(error) = save_cached_lyrics(&store, &source_id, &lyrics) {
                            warn!(%error, "failed to cache previewed lyrics");
                        }
                        let _sent = events.send(lyrics_event(&media_key, generation, Some(lyrics)));
                    }
                    Ok(None) => {
                        let _sent = events.send(lyrics_event(&media_key, generation, None));
                    }
                    Err(error) => {
                        warn!(%error, "failed to preview lyrics");
                        let _sent = events.send(lyrics_event(&media_key, generation, None));
                    }
                }
            })
        });
        if let Err(error) = submit {
            warn!(%error, "could not schedule lyrics preview");
        }
    }

    pub(crate) fn lyrics_result_is_current(&self, generation: u64) -> bool {
        self.lyrics_request_generation.load(Ordering::Acquire) == generation
    }
}
