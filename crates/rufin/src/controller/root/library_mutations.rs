use super::*;

impl AppController {
    pub fn set_album_favorite(&self, album_id: AlbumId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Album(album_id), favorite);
    }
    pub fn set_track_favorite(&self, track_id: TrackId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Track(track_id), favorite);
    }
    pub fn set_artist_favorite(&self, artist_id: ArtistId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Artist(artist_id), favorite);
    }
    pub(in crate::controller) fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                emit_favorite_change_failed(
                    &events,
                    &item_id,
                    !favorite,
                    "No active music source is saved.",
                );
                return;
            };

            let capabilities = source_capabilities_for_saved(&saved);
            let owner = capabilities.favorite_mutations;
            match owner {
                SourceFeatureOwner::Native => {
                    let result =
                        source_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                            runtime
                                .block_on(
                                    provider
                                        .as_music_source()
                                        .set_favorite(item_id.clone(), favorite),
                                )
                                .map_err(|error| error.to_string())
                        });
                    if let Err(error) = result {
                        emit_favorite_change_failed(&events, &item_id, !favorite, error);
                        return;
                    }
                }
                SourceFeatureOwner::Store => {}
            }

            let result = store.with_store(|store| {
                match &item_id {
                    FavoriteItemId::Album(album_id) => {
                        store.set_album_favorite_for_owner(
                            &saved.source.id,
                            album_id,
                            favorite,
                            owner,
                        )?;
                    }
                    FavoriteItemId::Track(track_id) => {
                        store.set_track_favorite_for_owner(
                            &saved.source.id,
                            track_id,
                            favorite,
                            owner,
                        )?;
                    }
                    FavoriteItemId::Artist(artist_id) => {
                        store.set_artist_favorite_for_owner(
                            &saved.source.id,
                            artist_id,
                            favorite,
                            owner,
                        )?;
                    }
                }
                Ok(())
            });
            if let Err(error) = result {
                emit_favorite_change_failed(&events, &item_id, !favorite, error);
                return;
            }

            if let FavoriteItemId::Track(track_id) = &item_id {
                if let Ok(mut queue) = queue.lock()
                    && let Some(queue) = queue.as_mut()
                {
                    queue.set_track_favorite(track_id, favorite);
                    let snapshot = queue.snapshot();
                    let _saved = store.with_store(|store| store.save_queue_snapshot(&snapshot));
                    let _sent = events.send(ControllerEvent::Queue(Box::new(Some(snapshot))));
                }
                if let Ok(mut snapshot) = playback_snapshot.lock()
                    && let Some(current) = snapshot.current.as_mut()
                    && current.track_id == *track_id
                {
                    current.favorite = favorite;
                    let _sent = events.send(ControllerEvent::Playback(Box::new(snapshot.clone())));
                }
            }

            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::FavoriteChanged {
                        item_id,
                        favorite,
                        snapshot: Box::new(snapshot),
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
}

fn emit_favorite_change_failed(
    events: &Sender<ControllerEvent>,
    item_id: &FavoriteItemId,
    previous_favorite: bool,
    error: impl Into<String>,
) {
    let _sent = events.send(ControllerEvent::FavoriteChangeFailed {
        item_id: item_id.clone(),
        previous_favorite,
        error: error.into(),
    });
}
