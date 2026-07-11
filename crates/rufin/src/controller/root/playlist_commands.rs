use super::*;

impl AppController {
    pub fn playlist_creation_supported(&self) -> bool {
        current_active_source(&self.active_source).is_some()
    }

    pub fn playlist_operation_supported(
        &self,
        owner: SourceFeatureOwner,
        operation: SourcePlaylistOperation,
    ) -> bool {
        current_active_source(&self.active_source)
            .is_some_and(|active| active.supports_playlist_operation(operation, owner))
    }

    pub fn create_playlist(&self, name: String, tracks: Vec<Track>) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let track_ids = tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>();
            let active = match selected_active_source(&active_source, &saved.source.id) {
                Ok(active) => active,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let create_owner = active.playlist_creation.owner();
            let playlist_id = match &active.playlist_creation {
                OperationOwner::Native(executor) => match runtime
                    .block_on(executor.create_playlist(&name, &track_ids))
                    .map_err(|error| error.to_string())
                {
                    Ok(playlist_id) => playlist_id,
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                },
                OperationOwner::Store => PlaylistId::new(format!(
                    "rufin:playlist:{}",
                    unique_millis().unwrap_or(tracks.len() as u128)
                )),
            };
            let playlist = Playlist {
                id: playlist_id.clone(),
                name: name.trim().to_string(),
                owner: Some(create_owner),
                track_count: tracks.len() as u32,
                duration_seconds: tracks.iter().map(|track| track.duration_seconds).sum(),
                top_genres: Vec::new(),
                image_refs: track_cover_refs_for_items(&tracks),
                image_ref: tracks.iter().find_map(|track| track.image_ref.clone()),
            };
            let entries = playlist_entries_for_tracks(&playlist_id, &tracks);
            let result = store.with_store(|store| {
                write_playlist_snapshot_for_owner(
                    store,
                    &saved.source.id,
                    &playlist,
                    &entries,
                    create_owner,
                )
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn rename_playlist(&self, playlist_id: PlaylistId, name: String) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let Some(owner) = cached_playlist_owner(&store, &events, &saved, &playlist_id) else {
                return;
            };
            let active = match selected_active_source(&active_source, &saved.source.id) {
                Ok(active) => active,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if !playlist_operation_supported(
                owner,
                SourcePlaylistOperation::Rename,
                &active,
                &events,
            ) {
                return;
            }
            if owner == SourceFeatureOwner::Native {
                let Some(executor) = active.playlist_rows.rename.as_ref() else {
                    return;
                };
                let result = runtime
                    .block_on(executor.rename_playlist(&playlist_id, &name))
                    .map_err(|error| error.to_string());
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.rename_playlist_with_owner(
                    &saved.source.id,
                    &playlist_id,
                    name.trim(),
                    owner,
                )?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn delete_playlist(&self, playlist_id: PlaylistId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let Some(owner) = cached_playlist_owner(&store, &events, &saved, &playlist_id) else {
                return;
            };
            let active = match selected_active_source(&active_source, &saved.source.id) {
                Ok(active) => active,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if !playlist_operation_supported(
                owner,
                SourcePlaylistOperation::Delete,
                &active,
                &events,
            ) {
                return;
            }
            if owner == SourceFeatureOwner::Native {
                let Some(executor) = active.playlist_rows.delete.as_ref() else {
                    return;
                };
                let result = runtime
                    .block_on(executor.delete_playlist(&playlist_id))
                    .map_err(|error| error.to_string());
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.delete_playlist_with_owner(&saved.source.id, &playlist_id, owner)?;
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
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let result = store.with_store(|store| {
                store.delete_smart_playlist(&saved.source.id, &smart_playlist_id)?;
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
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let result = store.with_store(|store| {
                store.restore_builtin_smart_playlist(&saved.source.id, builtin)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }
    pub fn move_smart_playlist(
        &self,
        dragged_id: SmartPlaylistId,
        target_id: SmartPlaylistId,
        after: bool,
    ) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let result = store.with_store(|store| {
                store.reorder_smart_playlist(&saved.source.id, &dragged_id, &target_id, after)?;
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
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let id = SmartPlaylistId::new(format!(
                "custom:{}",
                unique_millis().unwrap_or(name.len() as u128)
            ));
            let result = store.with_store(|store| {
                store.save_smart_playlist(&saved.source.id, &id, &name, &definition)?;
                Ok(())
            });
            emit_smart_playlist_changed_result(&store, &events, id, result);
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
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let result = store.with_store(|store| {
                store.save_smart_playlist(
                    &saved.source.id,
                    &smart_playlist_id,
                    &name,
                    &definition,
                )?;
                Ok(())
            });
            emit_smart_playlist_changed_result(&store, &events, smart_playlist_id, result);
        });
    }
    pub fn add_tracks_to_playlist(&self, playlist_id: PlaylistId, tracks: Vec<Track>) {
        self.mutate_playlist_entries(
            playlist_id,
            SourcePlaylistOperation::AddTracks,
            move |mut detail| {
                let mut entries = detail.entries;
                entries.extend(playlist_entries_for_tracks(&detail.playlist.id, &tracks));
                detail.tracks.extend(tracks);
                detail.entries = entries;
                detail
            },
        );
    }
    pub fn remove_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String) {
        self.mutate_playlist_entries(
            playlist_id,
            SourcePlaylistOperation::RemoveEntries,
            move |mut detail| {
                detail.entries.retain(|entry| entry.entry_id != entry_id);
                detail.tracks = detail
                    .entries
                    .iter()
                    .map(|entry| entry.track.clone())
                    .collect();
                detail
            },
        );
    }
    pub fn move_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String, new_index: usize) {
        self.mutate_playlist_entries(
            playlist_id,
            SourcePlaylistOperation::ReorderEntries,
            move |mut detail| {
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
            },
        );
    }
    pub(in crate::controller) fn mutate_playlist_entries(
        &self,
        playlist_id: PlaylistId,
        operation: SourcePlaylistOperation,
        mutate: impl FnOnce(source::PlaylistDetail) -> source::PlaylistDetail + Send + 'static,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music source is saved.".to_string(),
                ));
                return;
            };
            let before = match store
                .with_store(|store| store.load_playlist_detail(&saved.source.id, &playlist_id))
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
            let Some(owner) = cached_playlist_owner(&store, &events, &saved, &playlist_id) else {
                return;
            };
            let active = match selected_active_source(&active_source, &saved.source.id) {
                Ok(active) => active,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if !playlist_operation_supported(owner, operation, &active, &events) {
                return;
            }
            if owner == SourceFeatureOwner::Native {
                match sync_playlist_mutation(&runtime, &active, operation, &before, &after) {
                    Ok(fresh) => after = fresh,
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
                image_refs: track_cover_refs_for_items(
                    &after
                        .entries
                        .iter()
                        .map(|entry| entry.track.clone())
                        .collect::<Vec<_>>(),
                ),
                image_ref: after
                    .entries
                    .iter()
                    .find_map(|entry| entry.track.image_ref.clone())
                    .or(after.playlist.image_ref.clone()),
                ..after.playlist.clone()
            };
            let result = store.with_store(|store| {
                write_playlist_snapshot_for_owner(
                    store,
                    &saved.source.id,
                    &playlist,
                    &after.entries,
                    owner,
                )
            });
            emit_playlist_changed_result(&store, &events, after.playlist.id.clone(), result);
        });
    }
}

fn write_playlist_snapshot_for_owner(
    store: &Store,
    source_id: &SourceId,
    playlist: &Playlist,
    entries: &[PlaylistEntry],
    owner: SourceFeatureOwner,
) -> StoreResult<()> {
    let mode = playlist_write_mode_for_owner(store, source_id, owner)?;
    store.replace_playlist_snapshot(source_id, playlist, entries, mode)
}

fn playlist_write_mode_for_owner(
    store: &Store,
    source_id: &SourceId,
    owner: SourceFeatureOwner,
) -> StoreResult<PlaylistWriteMode> {
    match owner {
        SourceFeatureOwner::Native => Ok(PlaylistWriteMode::NativeSync {
            generation: store.sync_state(source_id)?.generation,
        }),
        SourceFeatureOwner::Store => Ok(PlaylistWriteMode::StoreOwned),
    }
}

fn cached_playlist_owner(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    saved: &SavedSource,
    playlist_id: &PlaylistId,
) -> Option<SourceFeatureOwner> {
    match store.with_store(|store| store.playlist_owner(&saved.source.id, playlist_id)) {
        Ok(Some(owner)) => Some(owner),
        Ok(None) => {
            let _sent = events.send(ControllerEvent::Error(
                "The selected cached playlist was not found.".to_string(),
            ));
            None
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
            None
        }
    }
}

fn playlist_operation_supported(
    owner: SourceFeatureOwner,
    operation: SourcePlaylistOperation,
    active: &ActiveSource,
    events: &Sender<ControllerEvent>,
) -> bool {
    if !active.supports_playlist_operation(operation, owner) {
        let message = format!(
            "{} is not supported for {} playlists by the active source.",
            playlist_operation_label(operation),
            playlist_owner_label(owner)
        );
        let _sent = events.send(ControllerEvent::Error(message.to_string()));
        false
    } else {
        true
    }
}

fn playlist_operation_label(operation: SourcePlaylistOperation) -> &'static str {
    match operation {
        SourcePlaylistOperation::Rename => "Playlist rename",
        SourcePlaylistOperation::Delete => "Playlist deletion",
        SourcePlaylistOperation::AddTracks => "Adding tracks",
        SourcePlaylistOperation::RemoveEntries => "Removing playlist entries",
        SourcePlaylistOperation::ReorderEntries => "Reordering playlist entries",
    }
}

fn playlist_owner_label(owner: SourceFeatureOwner) -> &'static str {
    match owner {
        SourceFeatureOwner::Native => "native",
        SourceFeatureOwner::Store => "store-owned",
    }
}
