use super::*;

impl AppController {
    pub fn create_playlist(&self, name: String, tracks: Vec<Track>) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
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
            let track_ids = tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>();
            let playlist_id = if saved.server.provider == "fake" {
                PlaylistId::new(format!(
                    "fake:playlist:{}",
                    unique_millis().unwrap_or(tracks.len() as u128)
                ))
            } else {
                match provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                    runtime
                        .block_on(
                            provider
                                .as_music_provider()
                                .create_playlist(&name, &track_ids),
                        )
                        .map_err(|error| error.to_string())
                }) {
                    Ok(playlist_id) => playlist_id,
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            };
            let playlist = Playlist {
                id: playlist_id.clone(),
                name: name.trim().to_string(),
                track_count: tracks.len() as u32,
                duration_seconds: tracks.iter().map(|track| track.duration_seconds).sum(),
                image_ref: tracks.iter().find_map(|track| track.image_ref.clone()),
            };
            let entries = playlist_entries_for_tracks(&playlist_id, &tracks);
            let result = store.with_store(|store| {
                store.upsert_playlists(&saved.server.id, &[playlist], 0)?;
                store.upsert_tracks(&saved.server.id, &tracks, 0)?;
                store.upsert_playlist_entries(&saved.server.id, &playlist_id, &entries, 0)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn rename_playlist(&self, playlist_id: PlaylistId, name: String) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
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
            if saved.server.provider != "fake" && saved.server.provider != "local" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(
                                provider
                                    .as_music_provider()
                                    .rename_playlist(&playlist_id, &name),
                            )
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.rename_playlist(&saved.server.id, &playlist_id, name.trim())?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn delete_playlist(&self, playlist_id: PlaylistId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
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
            if saved.server.provider != "fake" && saved.server.provider != "local" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(provider.as_music_provider().delete_playlist(&playlist_id))
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.delete_playlist(&saved.server.id, &playlist_id)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn delete_smart_playlist(&self, smart_playlist_id: SmartPlaylistId) {
        let store = self.store.clone();
        let events = self.events.clone();
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
            let result = store.with_store(|store| {
                store.delete_smart_playlist(&saved.server.id, &smart_playlist_id)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn restore_builtin_smart_playlist(&self, builtin: SmartPlaylistBuiltin) {
        let store = self.store.clone();
        let events = self.events.clone();
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
            let result = store.with_store(|store| {
                store.restore_builtin_smart_playlist(&saved.server.id, builtin)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn save_smart_playlist(&self, name: String, definition: SmartPlaylistDefinition) {
        let store = self.store.clone();
        let events = self.events.clone();
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
            let id = SmartPlaylistId::new(format!(
                "custom:{}",
                unique_millis().unwrap_or(name.len() as u128)
            ));
            let result = store.with_store(|store| {
                store.save_smart_playlist(&saved.server.id, &id, &name, &definition)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn update_smart_playlist(
        &self,
        smart_playlist_id: SmartPlaylistId,
        name: String,
        definition: SmartPlaylistDefinition,
    ) {
        let store = self.store.clone();
        let events = self.events.clone();
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
            let result = store.with_store(|store| {
                store.save_smart_playlist(
                    &saved.server.id,
                    &smart_playlist_id,
                    &name,
                    &definition,
                )?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn add_tracks_to_playlist(&self, playlist_id: PlaylistId, tracks: Vec<Track>) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            let mut entries = detail.entries;
            entries.extend(playlist_entries_for_tracks(&detail.playlist.id, &tracks));
            detail.tracks.extend(tracks);
            detail.entries = entries;
            detail
        });
    }
    pub fn remove_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            detail.entries.retain(|entry| entry.entry_id != entry_id);
            detail.tracks = detail
                .entries
                .iter()
                .map(|entry| entry.track.clone())
                .collect();
            detail
        });
    }
    pub fn move_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String, new_index: usize) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            if let Some(old_index) = detail
                .entries
                .iter()
                .position(|entry| entry.entry_id == entry_id)
            {
                let entry = detail.entries.remove(old_index);
                detail
                    .entries
                    .insert(new_index.min(detail.entries.len()), entry);
                detail.tracks = detail
                    .entries
                    .iter()
                    .map(|entry| entry.track.clone())
                    .collect();
            }
            detail
        });
    }
    pub(in crate::controller) fn mutate_playlist_entries(
        &self,
        playlist_id: PlaylistId,
        mutate: impl FnOnce(rufin_provider::PlaylistDetail) -> rufin_provider::PlaylistDetail
        + Send
        + 'static,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
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
            let before = match store
                .with_store(|store| store.load_playlist_detail(&saved.server.id, &playlist_id))
            {
                Ok(Some(detail)) => detail,
                Ok(None) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected cached playlist was not found.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let mut after = mutate(before.clone());
            if saved.server.provider != "fake" {
                match sync_playlist_mutation(&store, &runtime, &secrets, &saved, &before, &after) {
                    Ok(Some(fresh)) => after = fresh,
                    Ok(None) => {}
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            }
            let playlist = Playlist {
                track_count: after.entries.len() as u32,
                duration_seconds: after
                    .entries
                    .iter()
                    .map(|entry| entry.track.duration_seconds)
                    .sum(),
                image_ref: after
                    .entries
                    .iter()
                    .find_map(|entry| entry.track.image_ref.clone())
                    .or(after.playlist.image_ref.clone()),
                ..after.playlist.clone()
            };
            let result = store.with_store(|store| {
                store.upsert_playlists(&saved.server.id, &[playlist], 0)?;
                store.upsert_tracks(&saved.server.id, &after.tracks, 0)?;
                store.upsert_playlist_entries(
                    &saved.server.id,
                    &after.playlist.id,
                    &after.entries,
                    0,
                )?;
                Ok(())
            });
            emit_playlist_changed_result(&store, &events, after.playlist.id.clone(), result);
        });
    }
}
