use super::*;

impl LibraryCommands {
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
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!("cannot create a playlist without an active source");
                return;
            };
            let track_ids = tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>();
            let active = match selected_active_source(&active_source, &saved.source_id) {
                Ok(active) => active,
                Err(error) => {
                    warn!(%error, "failed to resolve active source for playlist creation");
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
                        warn!(%error, "native playlist creation failed");
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
                image_ref: None,
                representative_albums: Vec::new(),
            };
            let entries = playlist_entries_for_tracks(&playlist_id, &tracks);
            let result = store.with_store(|store| {
                write_playlist_snapshot_for_owner(
                    store,
                    &saved.source_id,
                    &playlist,
                    &entries,
                    create_owner,
                )
            });
            emit_playlist_changed_result(&library_events, playlist.id.clone(), result);
        });
    }
    pub fn rename_playlist(&self, playlist_id: PlaylistId, name: String) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!(%playlist_id, "cannot rename a playlist without an active source");
                return;
            };
            let Some(owner) = cached_playlist_owner(&store, &saved, &playlist_id) else {
                return;
            };
            let active = match selected_active_source(&active_source, &saved.source_id) {
                Ok(active) => active,
                Err(error) => {
                    warn!(%error, %playlist_id, "failed to resolve active source for playlist rename");
                    return;
                }
            };
            if !playlist_operation_supported(owner, SourcePlaylistOperation::Rename, &active) {
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
                    warn!(%error, %playlist_id, "native playlist rename failed");
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.rename_playlist_with_owner(
                    &saved.source_id,
                    &playlist_id,
                    name.trim(),
                    owner,
                )?;
                Ok(())
            });
            emit_playlist_changed_result(&library_events, playlist_id, result);
        });
    }
    pub fn delete_playlist(&self, playlist_id: PlaylistId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!(%playlist_id, "cannot delete a playlist without an active source");
                return;
            };
            let Some(owner) = cached_playlist_owner(&store, &saved, &playlist_id) else {
                return;
            };
            let active = match selected_active_source(&active_source, &saved.source_id) {
                Ok(active) => active,
                Err(error) => {
                    warn!(%error, %playlist_id, "failed to resolve active source for playlist deletion");
                    return;
                }
            };
            if !playlist_operation_supported(owner, SourcePlaylistOperation::Delete, &active) {
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
                    warn!(%error, %playlist_id, "native playlist deletion failed");
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.delete_playlist_with_owner(&saved.source_id, &playlist_id, owner)?;
                Ok(())
            });
            emit_playlist_changed_result(&library_events, playlist_id, result);
        });
    }
    pub fn delete_smart_playlist(&self, smart_playlist_id: SmartPlaylistId) {
        let store = self.store.clone();
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!(%smart_playlist_id, "cannot delete a smart playlist without an active source");
                return;
            };
            let result = store.with_store(|store| {
                store.delete_smart_playlist(&saved.source_id, &smart_playlist_id)?;
                Ok(())
            });
            emit_smart_playlist_changed_result(&library_events, smart_playlist_id, result);
        });
    }
    pub fn restore_builtin_smart_playlist(&self, builtin: SmartPlaylistBuiltin) {
        let store = self.store.clone();
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!(
                    ?builtin,
                    "cannot restore a smart playlist without an active source"
                );
                return;
            };
            match store
                .with_store(|store| store.restore_builtin_smart_playlist(&saved.source_id, builtin))
            {
                Ok(smart_playlist_id) => {
                    emit_smart_playlist_changed_result(&library_events, smart_playlist_id, Ok(()))
                }
                Err(error) => {
                    warn!(%error, ?builtin, "smart playlist restore failed");
                }
            }
        });
    }
    pub fn move_smart_playlist(
        &self,
        dragged_id: SmartPlaylistId,
        target_id: SmartPlaylistId,
        after: bool,
    ) {
        let store = self.store.clone();
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!(%dragged_id, %target_id, "cannot reorder smart playlists without an active source");
                return;
            };
            let result = store.with_store(|store| {
                store.reorder_smart_playlist(&saved.source_id, &dragged_id, &target_id, after)?;
                Ok(())
            });
            emit_smart_playlist_changed_result(&library_events, dragged_id, result);
        });
    }
    pub fn save_smart_playlist(&self, name: String, definition: SmartPlaylistDefinition) {
        let store = self.store.clone();
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!("cannot save a smart playlist without an active source");
                return;
            };
            let id = SmartPlaylistId::new(format!(
                "custom:{}",
                unique_millis().unwrap_or(name.len() as u128)
            ));
            let result = store.with_store(|store| {
                store.save_smart_playlist(&saved.source_id, &id, &name, &definition)?;
                Ok(())
            });
            emit_smart_playlist_changed_result(&library_events, id, result);
        });
    }
    pub fn update_smart_playlist(
        &self,
        smart_playlist_id: SmartPlaylistId,
        name: String,
        definition: SmartPlaylistDefinition,
    ) {
        let store = self.store.clone();
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!(%smart_playlist_id, "cannot update a smart playlist without an active source");
                return;
            };
            let result = store.with_store(|store| {
                store.save_smart_playlist(
                    &saved.source_id,
                    &smart_playlist_id,
                    &name,
                    &definition,
                )?;
                Ok(())
            });
            emit_smart_playlist_changed_result(&library_events, smart_playlist_id, result);
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
        mutate: impl FnOnce(library::PlaylistDetail) -> library::PlaylistDetail + Send + 'static,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!(%playlist_id, "cannot mutate playlist entries without an active source");
                return;
            };
            let before = match store
                .with_store(|store| store.load_playlist_detail(&saved.source_id, &playlist_id))
            {
                Ok(Some(detail)) => detail,
                Ok(None) => {
                    warn!(%playlist_id, "cannot mutate missing cached playlist");
                    return;
                }
                Err(error) => {
                    warn!(%error, %playlist_id, "failed to load playlist before mutation");
                    return;
                }
            };
            let mut after = mutate(before.clone());
            let Some(owner) = cached_playlist_owner(&store, &saved, &playlist_id) else {
                return;
            };
            let active = match selected_active_source(&active_source, &saved.source_id) {
                Ok(active) => active,
                Err(error) => {
                    warn!(%error, %playlist_id, "failed to resolve active source for playlist mutation");
                    return;
                }
            };
            if !playlist_operation_supported(owner, operation, &active) {
                return;
            }
            if owner == SourceFeatureOwner::Native {
                match sync_playlist_mutation(&runtime, &active, operation, &before, &after) {
                    Ok(fresh) => after = fresh,
                    Err(error) => {
                        warn!(%error, %playlist_id, "native playlist entry mutation failed");
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
                image_ref: after.playlist.image_ref.clone(),
                representative_albums: Vec::new(),
                ..after.playlist.clone()
            };
            let result = store.with_store(|store| {
                write_playlist_snapshot_for_owner(
                    store,
                    &saved.source_id,
                    &playlist,
                    &after.entries,
                    owner,
                )
            });
            emit_playlist_changed_result(&library_events, after.playlist.id.clone(), result);
        });
    }
}

fn emit_playlist_changed_result(
    library_events: &Sender<library::LibraryEvent>,
    playlist_id: PlaylistId,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        warn!(%error, %playlist_id, "playlist mutation failed");
        return;
    }
    let _sent = library_events.try_send(library::LibraryEvent::Delta(Box::new(
        library::LibraryDelta::playlist_changed(playlist_id),
    )));
}

fn emit_smart_playlist_changed_result(
    library_events: &Sender<library::LibraryEvent>,
    smart_playlist_id: SmartPlaylistId,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        warn!(%error, %smart_playlist_id, "smart playlist mutation failed");
        return;
    }
    let _sent = library_events.try_send(library::LibraryEvent::Delta(Box::new(
        library::LibraryDelta::smart_playlist_changed(smart_playlist_id),
    )));
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
    saved: &StoredSource,
    playlist_id: &PlaylistId,
) -> Option<SourceFeatureOwner> {
    match store.with_store(|store| store.playlist_owner(&saved.source_id, playlist_id)) {
        Ok(Some(owner)) => Some(owner),
        Ok(None) => {
            warn!(%playlist_id, source_id = %saved.source_id, "cached playlist owner was not found");
            None
        }
        Err(error) => {
            warn!(%error, %playlist_id, source_id = %saved.source_id, "failed to load cached playlist owner");
            None
        }
    }
}

fn playlist_operation_supported(
    owner: SourceFeatureOwner,
    operation: SourcePlaylistOperation,
    active: &ActiveSource,
) -> bool {
    if !active.supports_playlist_operation(operation, owner) {
        let message = format!(
            "{} is not supported for {} playlists by the active source.",
            playlist_operation_label(operation),
            playlist_owner_label(owner)
        );
        warn!(%message, "unsupported playlist operation");
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
