use super::*;

impl AppController {
    // Route conversion has not been wired yet, so this seam is intentionally idle in Task 5.
    #[allow(dead_code)]
    pub fn play_activation(&self, activation: PlayActivation) {
        let activation = match normalize_loaded_source_activation(activation) {
            Ok(activation) => activation,
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
                return;
            }
        };
        match (activation.action, activation.target) {
            (PlayAction::ReplaceNow, NormalizedPlayTarget::TrackOnly(track)) => {
                self.play_now(track);
            }
            (PlayAction::ReplaceNow, NormalizedPlayTarget::Replacement(replacement)) => {
                self.replace_queue_from_activation(replacement);
            }
            _ => {
                let _sent = self.events.send(ControllerEvent::Error(
                    "This play action is not available for the selected source.".to_string(),
                ));
            }
        }
    }

    fn replace_queue_from_activation(&self, replacement: QueueReplacement) {
        let result = self.with_queue_mut(|queue| {
            queue
                .replace_all(replacement)
                .map(|_| ())
                .map_err(|_| "The selected source could not be queued.".to_string())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }

    pub fn play_tracks_now(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            let _sent = self.events.send(ControllerEvent::Error(
                "No tracks are available to play.".to_string(),
            ));
            return;
        }
        let result = self.with_queue_mut(|queue| {
            queue.clear();
            let mut tracks = tracks.into_iter();
            if let Some(first) = tracks.next() {
                queue.play_now(&first);
            }
            for track in tracks {
                queue.append(&track);
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }
    pub fn play_now(&self, track: Track) {
        self.play_tracks_now(vec![track]);
    }
    pub fn play_album_now(&self, album_id: AlbumId) {
        match self.cached_album_detail(&album_id) {
            Ok(Some((album, tracks))) => {
                if tracks.is_empty() {
                    let _sent = self.events.send(ControllerEvent::Error(
                        "No tracks are available to play.".to_string(),
                    ));
                    return;
                }
                self.play_activation(Self::album_play_activation(
                    album.id,
                    tracks,
                    Self::selected_music_folder_id_for_active_server(&self.store),
                ));
            }
            Ok(None) => {
                let _sent = self.events.send(ControllerEvent::Error(
                    "The selected cached album was not found.".to_string(),
                ));
            }
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
            }
        }
    }

    fn album_play_activation(
        album_id: AlbumId,
        tracks: Vec<Track>,
        selected_music_folder_id: Option<MusicFolderId>,
    ) -> PlayActivation {
        let anchor_track_id = tracks
            .first()
            .expect("album activation has a first track")
            .id
            .clone();
        PlayActivation {
            action: PlayAction::ReplaceNow,
            target: PlayTarget::LoadedSource {
                source_key: PlaySourceKey {
                    descriptor: PlaySourceDescriptor::Album {
                        album_id,
                        selected_music_folder_id,
                    },
                    order: SourceOrder::Canonical,
                },
                completeness: LoadedCompleteness::Complete,
                items: tracks
                    .into_iter()
                    .enumerate()
                    .map(|(source_index, track)| PlaySourceItem {
                        track,
                        source_index,
                        source_item_id: None,
                    })
                    .collect(),
                anchor: PlayAnchor {
                    track_id: anchor_track_id,
                    source_index: 0,
                    source_item_id: None,
                },
            },
        }
    }

    fn selected_music_folder_id_for_active_server(store: &StoreHandle) -> Option<MusicFolderId> {
        store
            .with_store(|store| {
                let Some(saved) = store.active_server()? else {
                    return Ok(None);
                };
                store.selected_music_folder_id(&saved.server.id)
            })
            .ok()
            .flatten()
    }
    pub fn play_next(&self, track: Track) {
        let result = self.with_queue_mut(|queue| {
            queue.play_next(&track);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn play_last(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            let _sent = self.events.send(ControllerEvent::Error(
                "No tracks are available to add to the queue.".to_string(),
            ));
            return;
        }
        let result = self.with_queue_mut(|queue| {
            for track in &tracks {
                queue.append(track);
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn remove_from_queue(&self, entry_id: QueueEntryId) {
        let mut removed_current = false;
        let mut has_current_after_remove = false;
        let result = self.with_queue_mut(|queue| {
            let current_id = queue.current().map(|entry| entry.id.clone());
            let removed = queue.remove(&entry_id).is_some();
            removed_current = removed && current_id.as_ref() == Some(&entry_id);
            has_current_after_remove = queue.current().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if removed_current {
            self.record_current_skip_if_needed();
        }
        if removed_current && !has_current_after_remove {
            invalidate_playback_requests(&self.playback_request_generation);
            let _result = self.send_playback_command(PlaybackCommand::Stop);
            self.clear_playback_activity();
        }
        self.persist_and_emit_queue();
        if removed_current && has_current_after_remove {
            self.start_current_track();
        }
    }
    pub fn activate_queue_entry(&self, entry_id: QueueEntryId) {
        let previous_current = self
            .queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref()?.current().map(|entry| entry.id.clone()));
        let result = self.with_queue_mut(|queue| {
            if queue.activate(&entry_id) {
                Ok(())
            } else {
                Err("The selected queue entry was not found.".to_string())
            }
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if previous_current
            .as_ref()
            .is_some_and(|current| current != &entry_id)
        {
            self.record_current_skip_if_needed();
        }
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }
    pub fn move_queue_entry_after_current(&self, entry_id: QueueEntryId) {
        let result = self.with_queue_mut(|queue| {
            if queue.move_after_current(&entry_id) {
                Ok(())
            } else {
                Err("The selected queue entry was not found.".to_string())
            }
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn clear_queue(&self) {
        let had_current = self
            .queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref()?.current().map(|entry| entry.id.clone()))
            .is_some();
        let result = self.with_queue_mut(|queue| {
            queue.clear();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if had_current {
            invalidate_playback_requests(&self.playback_request_generation);
            self.record_current_skip_if_needed();
            self.clear_playback_activity();
        }
        let _result = self.send_playback_command(PlaybackCommand::Stop);
        self.persist_and_emit_queue();
    }
    pub fn toggle_shuffle(&self) {
        let result = self.with_queue_mut(|queue| {
            let enabled = !queue.shuffle().enabled;
            let seed = if enabled {
                shuffle_seed()
            } else {
                queue.shuffle().seed
            };
            queue.set_shuffle(enabled, seed);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
    pub fn cycle_repeat(&self) {
        let result = self.with_queue_mut(|queue| {
            let next = match queue.repeat_mode() {
                RepeatMode::Off => RepeatMode::All,
                RepeatMode::All => RepeatMode::One,
                RepeatMode::One => RepeatMode::Off,
            };
            queue.set_repeat_mode(next);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }
}
