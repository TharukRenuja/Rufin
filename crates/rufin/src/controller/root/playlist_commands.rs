use super::*;

enum PlaylistTrackMaterialization {
    Loaded(Vec<Track>),
    Context(library::play_context::PlayContextDescriptor),
}

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
        self.create_playlist_with(name, PlaylistTrackMaterialization::Loaded(tracks));
    }

    pub fn create_playlist_from_context(
        &self,
        name: String,
        descriptor: library::play_context::PlayContextDescriptor,
    ) {
        self.create_playlist_with(name, PlaylistTrackMaterialization::Context(descriptor));
    }

    fn create_playlist_with(&self, name: String, tracks: PlaylistTrackMaterialization) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let loaded = store.with_store(|store| {
                let Some(saved) = store.active_source()? else {
                    return Ok(None);
                };
                let tracks = match tracks {
                    PlaylistTrackMaterialization::Loaded(tracks) => tracks,
                    PlaylistTrackMaterialization::Context(descriptor) => store
                        .materialize_play_context_items(
                            &saved.source_id,
                            &library::play_context::PlayContext {
                                descriptor,
                                order: library::play_context::PlayContextOrder::Canonical,
                            },
                        )?
                        .into_iter()
                        .map(|item| item.track)
                        .collect(),
                };
                Ok(Some((saved, tracks)))
            });
            let Some((saved, tracks)) = loaded.unwrap_or_else(|error| {
                warn!(%error, "failed to prepare playlist creation");
                None
            }) else {
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
            move |_, _, mut detail| {
                let mut entries = detail.entries;
                entries.extend(playlist_entries_for_tracks(&detail.playlist.id, &tracks));
                detail.tracks.extend(tracks);
                detail.entries = entries;
                Ok(Some(detail))
            },
        );
    }

    pub fn add_context_to_playlist(
        &self,
        playlist_id: PlaylistId,
        descriptor: library::play_context::PlayContextDescriptor,
        skip_duplicates: bool,
    ) {
        self.mutate_playlist_entries(
            playlist_id,
            SourcePlaylistOperation::AddTracks,
            move |store, saved, mut detail| {
                let context = library::play_context::PlayContext {
                    descriptor,
                    order: library::play_context::PlayContextOrder::Canonical,
                };
                let mut tracks = store
                    .materialize_play_context_items(&saved.source_id, &context)?
                    .into_iter()
                    .map(|item| item.track)
                    .collect::<Vec<_>>();
                if skip_duplicates {
                    let existing = detail
                        .entries
                        .iter()
                        .map(|entry| &entry.track.id)
                        .collect::<HashSet<_>>();
                    tracks.retain(|track| !existing.contains(&track.id));
                }
                if tracks.is_empty() {
                    return Ok(None);
                }
                detail
                    .entries
                    .extend(playlist_entries_for_tracks(&detail.playlist.id, &tracks));
                detail.tracks.extend(tracks);
                Ok(Some(detail))
            },
        );
    }
    pub fn remove_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String) {
        self.mutate_playlist_entries(
            playlist_id,
            SourcePlaylistOperation::RemoveEntries,
            move |_, _, mut detail| {
                detail.entries.retain(|entry| entry.entry_id != entry_id);
                detail.tracks = detail
                    .entries
                    .iter()
                    .map(|entry| entry.track.clone())
                    .collect();
                Ok(Some(detail))
            },
        );
    }
    pub fn move_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String, new_index: usize) {
        self.mutate_playlist_entries(
            playlist_id,
            SourcePlaylistOperation::ReorderEntries,
            move |_, _, mut detail| {
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
                Ok(Some(detail))
            },
        );
    }
    pub(in crate::controller) fn mutate_playlist_entries(
        &self,
        playlist_id: PlaylistId,
        operation: SourcePlaylistOperation,
        mutate: impl FnOnce(
            &Store,
            &StoredSource,
            library::PlaylistDetail,
        ) -> StoreResult<Option<library::PlaylistDetail>>
        + Send
        + 'static,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let library_events = self.library_events.clone();
        thread::spawn(move || {
            let prepared = store.with_store(|store| {
                let Some(saved) = store.active_source()? else {
                    return Ok(None);
                };
                let Some(before) = store.load_playlist_detail(&saved.source_id, &playlist_id)?
                else {
                    return Ok(None);
                };
                let Some(after) = mutate(store, &saved, before.clone())? else {
                    return Ok(None);
                };
                Ok(Some((saved, before, after)))
            });
            let Some((saved, before, mut after)) = (match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    warn!(%error, %playlist_id, "failed to prepare playlist mutation");
                    return;
                }
            }) else {
                return;
            };
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

fn sync_playlist_mutation(
    runtime: &Runtime,
    active: &ActiveSource,
    operation: SourcePlaylistOperation,
    before: &library::PlaylistDetail,
    after: &library::PlaylistDetail,
) -> Result<library::PlaylistDetail, String> {
    let reader = match operation {
        SourcePlaylistOperation::AddTracks => {
            let operation = active.playlist_rows.add_tracks.as_ref().ok_or_else(|| {
                "Adding tracks is not supported for native playlists by the active source."
                    .to_string()
            })?;
            let before_ids = before
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<HashSet<_>>();
            let added = after
                .entries
                .iter()
                .filter(|entry| !before_ids.contains(entry.entry_id.as_str()))
                .map(|entry| entry.track.id.clone())
                .collect::<Vec<_>>();
            if !added.is_empty() {
                runtime
                    .block_on(
                        operation
                            .executor
                            .add_playlist_tracks(&before.playlist.id, &added),
                    )
                    .map_err(|error| error.to_string())?;
            }
            &operation.readback
        }
        SourcePlaylistOperation::RemoveEntries => {
            let operation = active
                .playlist_rows
                .remove_entries
                .as_ref()
                .ok_or_else(|| {
                    "Removing entries is not supported for native playlists by the active source."
                        .to_string()
                })?;
            let after_ids = after
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<HashSet<_>>();
            let removed = before
                .entries
                .iter()
                .filter(|entry| !after_ids.contains(entry.entry_id.as_str()))
                .map(|entry| entry.entry_id.clone())
                .collect::<Vec<_>>();
            if !removed.is_empty() {
                runtime
                    .block_on(
                        operation
                            .executor
                            .remove_playlist_entries(&before.playlist.id, &removed),
                    )
                    .map_err(|error| error.to_string())?;
            }
            &operation.readback
        }
        SourcePlaylistOperation::ReorderEntries => {
            let operation = active.playlist_rows.move_entry.as_ref().ok_or_else(|| {
                "Reordering entries is not supported for native playlists by the active source."
                    .to_string()
            })?;
            for (new_index, entry) in after.entries.iter().enumerate() {
                let Some(old_index) = before
                    .entries
                    .iter()
                    .position(|candidate| candidate.entry_id == entry.entry_id)
                else {
                    continue;
                };
                if old_index != new_index {
                    runtime
                        .block_on(operation.executor.move_playlist_entry(
                            &before.playlist.id,
                            &entry.entry_id,
                            new_index,
                        ))
                        .map_err(|error| error.to_string())?;
                }
            }
            &operation.readback
        }
        SourcePlaylistOperation::Rename | SourcePlaylistOperation::Delete => {
            return Err("The requested operation does not mutate playlist entries.".to_string());
        }
    };
    runtime
        .block_on(reader.playlist_detail(&before.playlist.id))
        .map_err(|error| error.to_string())
}

fn playlist_entries_for_tracks(playlist_id: &PlaylistId, tracks: &[Track]) -> Vec<PlaylistEntry> {
    let prefix = unique_millis().unwrap_or(0);
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| PlaylistEntry {
            entry_id: format!("{}:{prefix}:{index}", playlist_id.as_str()),
            track: track.clone(),
        })
        .collect()
}

fn unique_millis() -> Option<u128> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
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
