use super::*;

impl AppController {
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
            Ok(Some((_, tracks))) => self.play_tracks_now(tracks),
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
