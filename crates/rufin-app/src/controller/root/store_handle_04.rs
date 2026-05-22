impl AppController {
    fn send_playback_command(&self, command: PlaybackCommand) -> Result<(), String> {
        self.playback
            .lock()
            .map_err(|_| "playback lock was poisoned".to_string())?
            .send(command)
            .map_err(|error| error.to_string())
    }
    fn persist_playback_settings(&self, update: impl FnOnce(&mut PlaybackSettings)) {
        let mut settings = self.load_settings();
        update(&mut settings.playback);
        settings.playback.sanitize();
        if let Err(error) = self.save_settings(&settings) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
    }
    fn start_current_track(&self) {
        let Some((server_id, entry, next_entry, position_seconds, playback_settings)) =
            self.current_playback_request()
        else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("Queue is empty.".to_string()));
            return;
        };
        self.update_playback_snapshot(|snapshot| {
            snapshot.current = Some(entry.clone());
            snapshot.state = PlaybackState::Buffering;
            snapshot.position_seconds = position_seconds;
            snapshot.position_millis = u64::from(position_seconds) * 1_000;
            snapshot.duration_seconds = entry.duration_seconds;
            snapshot.last_error = None;
        });
        self.emit_playback_snapshot();
        self.report_playback(PlaybackReportKind::Started, false);
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let events = self.events.clone();
        thread::spawn(move || {
            let item = match resolve_prepared_item(
                &store,
                &runtime,
                &secrets,
                &server_id,
                &entry,
                &playback_settings,
            ) {
                Ok(item) => item,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let next = next_entry.and_then(|entry| {
                match resolve_prepared_item(
                    &store,
                    &runtime,
                    &secrets,
                    &server_id,
                    &entry,
                    &playback_settings,
                ) {
                    Ok(item) => Some(item),
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        None
                    }
                }
            });
            let command = PlaybackCommand::PlayPrepared {
                item,
                next,
                start_position_seconds: position_seconds,
                settings: playback_settings,
            };
            if let Err(error) = playback
                .lock()
                .map_err(|_| "playback lock was poisoned".to_string())
                .and_then(|mut playback| playback.send(command).map_err(|error| error.to_string()))
            {
                if let Ok(mut snapshot) = playback_snapshot.lock() {
                    snapshot.state = PlaybackState::Stopped;
                    snapshot.last_error = Some(error.clone());
                }
                let _sent = events.send(ControllerEvent::Error(error));
            }
        });
    }
    fn current_queue_entry(&self) -> Option<(ServerId, QueueEntry, u32)> {
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let snapshot = queue.snapshot();
            let entry = queue.current()?.clone();
            Some((snapshot.server_id, entry, snapshot.progress_seconds))
        })
    }
    fn current_playback_request(
        &self,
    ) -> Option<(
        ServerId,
        QueueEntry,
        Option<QueueEntry>,
        u32,
        PlaybackSettings,
    )> {
        let playback_settings = self.load_settings().playback;
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let snapshot = queue.snapshot();
            let entry = queue.current()?.clone();
            let next = next_queue_entry_after_current(queue);
            Some((
                snapshot.server_id,
                entry,
                next,
                snapshot.progress_seconds,
                playback_settings,
            ))
        })
    }
    fn prepare_next_stream(&self) {
        prepare_next_stream_from_handles(
            self.store.clone(),
            Arc::clone(&self.runtime),
            Arc::clone(&self.secrets),
            Arc::clone(&self.playback),
            Arc::clone(&self.queue),
            self.events.clone(),
        );
    }
    fn persist_progress_if_needed(&self, seconds: u32) {
        let Some(snapshot) = self.queue_snapshot() else {
            return;
        };
        let bucket = seconds / 10;
        let should_save = self
            .last_progress_snapshot
            .lock()
            .map(|mut last| {
                let changed = last.as_ref() != Some(&(snapshot.server_id.clone(), bucket));
                if changed {
                    *last = Some((snapshot.server_id.clone(), bucket));
                }
                changed
            })
            .unwrap_or(false);
        if should_save {
            let _result = self
                .store
                .with_store(|store| store.save_queue_snapshot(&snapshot));
        }
    }
    fn report_playback_progress_if_needed(&self, seconds: u32) {
        let Some(current) = self
            .playback_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.current.clone())
        else {
            return;
        };
        let bucket = seconds / 10;
        let should_report = self
            .last_report_snapshot
            .lock()
            .map(|mut last| {
                let changed = last.as_ref() != Some(&(current.track_id.clone(), bucket));
                if changed {
                    *last = Some((current.track_id.clone(), bucket));
                }
                changed
            })
            .unwrap_or(false);
        if should_report {
            self.report_playback(PlaybackReportKind::Progress, false);
        }
    }
    fn report_playback(&self, kind: PlaybackReportKind, failed: bool) {
        let snapshot = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default();
        let Some(current) = snapshot.current.clone() else {
            return;
        };
        let Some((server_id, _, _)) = self.current_queue_entry() else {
            return;
        };
        let settings = self.load_settings();
        if settings.private_mode {
            return;
        }
        external_scrobbling::report(
            &settings,
            &self.external_scrobble_state,
            kind,
            failed,
            &snapshot,
            &current,
        );
        let report = PlaybackReport {
            kind,
            track_id: current.track_id,
            position_seconds: snapshot.position_seconds,
            paused: snapshot.state == PlaybackState::Paused,
            muted: snapshot.muted,
            volume_percent: (snapshot.volume.clamp(0.0, 1.0) * 100.0).round() as u8,
            shuffle: snapshot.shuffle_enabled,
            repeat_one: snapshot.repeat_mode == RepeatMode::One,
            repeat_all: snapshot.repeat_mode == RepeatMode::All,
            failed,
        };
        report_playback_async(
            self.store.clone(),
            Arc::clone(&self.runtime),
            Arc::clone(&self.secrets),
            self.events.clone(),
            server_id,
            report,
        );
    }
    fn advance_after_end_of_stream(&self) {
        self.report_playback(PlaybackReportKind::Stopped, false);
        self.auto_dj_top_up_or_emit_error();
        let mut has_next = false;
        let result = self.with_queue_mut(|queue| {
            has_next = queue.advance_after_end_of_stream().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if has_next {
            self.persist_and_emit_queue();
            self.start_current_track();
        } else {
            self.stop();
        }
    }
    fn advance_after_prepared_track_started(&self, track: PlaybackTrack) {
        self.report_playback(PlaybackReportKind::Stopped, false);
        self.auto_dj_top_up_or_emit_error();
        let mut has_next = false;
        let result = self.with_queue_mut(|queue| {
            has_next = queue.advance_after_end_of_stream().is_some();
            if has_next && queue.current().is_some_and(|entry| entry.track_id != track.id) {
                warn!(
                    expected_track_id = %track.id.as_str(),
                    actual_track_id = queue.current().map(|entry| entry.track_id.as_str()).unwrap_or(""),
                    "prepared playback advanced to a different queue entry"
                );
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !has_next {
            self.stop();
            return;
        }
        self.persist_and_emit_queue();
        self.update_playback_snapshot(|snapshot| {
            snapshot.state = PlaybackState::Playing;
            snapshot.position_seconds = 0;
            snapshot.position_millis = 0;
            snapshot.duration_seconds = track.duration_seconds;
            snapshot.buffering_percent = None;
            snapshot.last_error = None;
        });
        self.emit_playback_snapshot();
        self.report_playback(PlaybackReportKind::Started, false);
    }
    fn start_sync(&self, saved: SavedServer) {
        start_sync_thread(self.sync_context(), saved);
    }
}
