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
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music server is saved.".to_string(),
                ));
                return;
            };

            let capabilities = source_capabilities_for_saved(&saved);
            let Some(owner) = capabilities.favorite_mutations.owner() else {
                let _sent = events.send(ControllerEvent::Error(
                    "Favorite changes are not supported by the active source.".to_string(),
                ));
                return;
            };
            match owner {
                SourceFeatureOwner::Native => {
                    let result = provider_for_saved(&store, &runtime, &secrets, &saved).and_then(
                        |provider| {
                            runtime
                                .block_on(
                                    provider
                                        .as_music_provider()
                                        .set_favorite(item_id.clone(), favorite),
                                )
                                .map_err(|error| error.to_string())
                        },
                    );
                    if let Err(error) = result {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
                SourceFeatureOwner::Store => {}
            }

            let result = store.with_store(|store| {
                match &item_id {
                    FavoriteItemId::Album(album_id) => {
                        store.set_album_favorite(&saved.server.id, album_id, favorite)?;
                    }
                    FavoriteItemId::Track(track_id) => {
                        store.set_track_favorite(&saved.server.id, track_id, favorite)?;
                    }
                    FavoriteItemId::Artist(artist_id) => {
                        store.set_artist_favorite(&saved.server.id, artist_id, favorite)?;
                    }
                }
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
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
