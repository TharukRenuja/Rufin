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
        let active_source = Arc::clone(&self.active_source);
        let events = self.events.clone();
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

            let active = match selected_active_source(&active_source, &saved.source_id) {
                Ok(active) => active,
                Err(error) => {
                    emit_favorite_change_failed(&events, &item_id, !favorite, error);
                    return;
                }
            };
            let owner = active.favorites.owner();
            match &active.favorites {
                OperationOwner::Native(executor) => {
                    let result = runtime
                        .block_on(executor.set_favorite(item_id.clone(), favorite))
                        .map_err(|error| error.to_string());
                    if let Err(error) = result {
                        emit_favorite_change_failed(&events, &item_id, !favorite, error);
                        return;
                    }
                }
                OperationOwner::Store => {}
            }

            let result = store.with_store(|store| {
                match &item_id {
                    FavoriteItemId::Album(album_id) => {
                        store.set_album_favorite_for_owner(
                            &saved.source_id,
                            album_id,
                            favorite,
                            owner,
                        )?;
                    }
                    FavoriteItemId::Track(track_id) => {
                        store.set_track_favorite_for_owner(
                            &saved.source_id,
                            track_id,
                            favorite,
                            owner,
                        )?;
                    }
                    FavoriteItemId::Artist(artist_id) => {
                        store.set_artist_favorite_for_owner(
                            &saved.source_id,
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
