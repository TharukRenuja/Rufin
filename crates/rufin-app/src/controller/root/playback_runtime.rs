use super::*;
use std::time::Instant;

impl AppController {
    pub(in crate::controller) fn send_playback_command(
        &self,
        command: PlaybackCommand,
    ) -> Result<(), String> {
        self.playback
            .lock()
            .map_err(|_| "playback lock was poisoned".to_string())?
            .send(command)
            .map_err(|error| error.to_string())
    }
    pub(in crate::controller) fn warm_playback_backend(&self) {
        let playback = Arc::clone(&self.playback);
        let events = self.events.clone();
        let playback_settings = self.load_settings().playback;
        thread::spawn(move || {
            let started_at = Instant::now();
            let result = playback
                .lock()
                .map_err(|_| "playback lock was poisoned".to_string())
                .and_then(|mut playback| {
                    playback
                        .send(PlaybackCommand::WarmUp(playback_settings))
                        .map_err(|error| error.to_string())
                });
            match result {
                Ok(()) => info!(
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "requested playback backend warmup"
                ),
                Err(error) => {
                    warn!(%error, "failed to warm playback backend");
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub(in crate::controller) fn persist_playback_settings(
        &self,
        update: impl FnOnce(&mut PlaybackSettings),
    ) {
        let mut settings = self.load_settings();
        update(&mut settings.playback);
        settings.playback.sanitize();
        if let Err(error) = self.save_settings(&settings) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
    }
    pub(in crate::controller) fn start_current_track(&self) {
        let Some((server_id, entry, next_entry, position_seconds, playback_settings)) =
            self.current_playback_request()
        else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("Queue is empty.".to_string()));
            return;
        };
        let request_generation =
            next_playback_request_generation(&self.playback_request_generation);
        self.update_playback_snapshot(|snapshot| {
            snapshot.current = Some(entry.clone());
            snapshot.state = PlaybackState::Buffering;
            snapshot.position_seconds = position_seconds;
            snapshot.position_millis = u64::from(position_seconds) * 1_000;
            snapshot.duration_seconds = entry.duration_seconds;
            snapshot.last_error = None;
            set_waveform_cache_key(
                snapshot,
                Some(waveform_cache_key(
                    &server_id,
                    &entry.track_id,
                    entry.duration_seconds,
                )),
            );
        });
        self.start_playback_activity(&server_id, &entry, position_seconds);
        self.emit_playback_snapshot();
        self.report_playback(PlaybackReportKind::Started, false);
        let waveform_enabled = self.load_settings().seekbar_waveform_enabled;
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let playback = Arc::clone(&self.playback);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let events = self.events.clone();
        thread::spawn(move || {
            if !request_generation_match(
                &playback_request_generation,
                request_generation,
                &queue,
                &server_id,
                &entry,
            ) {
                return;
            }
            let resolve_started = Instant::now();
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
                    if request_generation_match(
                        &playback_request_generation,
                        request_generation,
                        &queue,
                        &server_id,
                        &entry,
                    ) {
                        if let Ok(mut snapshot) = playback_snapshot.lock() {
                            snapshot.state = PlaybackState::Stopped;
                            snapshot.buffering_percent = None;
                            snapshot.last_error = Some(error.clone());
                        }
                        let _sent = events.send(ControllerEvent::Error(error));
                    }
                    return;
                }
            };
            if !request_generation_match(
                &playback_request_generation,
                request_generation,
                &queue,
                &server_id,
                &entry,
            ) {
                return;
            }
            debug!(
                track_id = %entry.track_id.as_str(),
                elapsed_ms = resolve_started.elapsed().as_millis(),
                "resolved current playback stream"
            );
            info!(
                track_id = %entry.track_id.as_str(),
                elapsed_ms = resolve_started.elapsed().as_millis(),
                "resolved current playback stream"
            );
            let waveform_item = item.clone();
            let has_next = next_entry.is_some();
            let command = PlaybackCommand::PlayPrepared {
                item,
                next: None,
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
                return;
            }
            info!(
                track_id = %entry.track_id.as_str(),
                elapsed_ms = resolve_started.elapsed().as_millis(),
                "sent playback command"
            );
            if has_next {
                prepare_next_stream_from_handles(
                    store.clone(),
                    Arc::clone(&runtime),
                    Arc::clone(&secrets),
                    Arc::clone(&playback),
                    queue,
                    events.clone(),
                );
            }
            if waveform_enabled {
                request_waveform_for_prepared_item(
                    playback_snapshot,
                    events,
                    server_id,
                    entry,
                    waveform_item,
                );
            }
        });
    }
    pub(in crate::controller) fn current_queue_entry(&self) -> Option<(ServerId, QueueEntry, u32)> {
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let entry = queue.current()?.clone();
            Some((queue.server_id().clone(), entry, queue.progress_seconds()))
        })
    }
    pub(in crate::controller) fn current_playback_request(
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
            let entry = queue.current()?.clone();
            let next = next_queue_entry_after_current(queue);
            Some((
                queue.server_id().clone(),
                entry,
                next,
                queue.progress_seconds(),
                playback_settings,
            ))
        })
    }
    pub(in crate::controller) fn prepare_next_stream(&self) {
        prepare_next_stream_from_handles(
            self.store.clone(),
            Arc::clone(&self.runtime),
            Arc::clone(&self.secrets),
            Arc::clone(&self.playback),
            Arc::clone(&self.queue),
            self.events.clone(),
        );
    }
}
