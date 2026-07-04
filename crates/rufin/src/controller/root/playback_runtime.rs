use super::*;
use std::time::{Duration, Instant};

const PLAYBACK_REQUEST_SETTLE: Duration = Duration::from_millis(150);
const PLAYBACK_LOCK_RETRY: Duration = Duration::from_millis(150);

impl AppController {
    pub(in crate::controller) fn send_playback_command(
        &self,
        command: PlaybackCommand,
    ) -> Result<(), String> {
        if matches!(
            &command,
            PlaybackCommand::Stop
                | PlaybackCommand::PrepareNext(None)
                | PlaybackCommand::PlayPrepared { next: None, .. }
        ) {
            clear_next_preload(&self.next_preload);
        }
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
        self.start_current_track_inner(false);
    }
    pub(in crate::controller) fn restart_current_track(&self) {
        self.start_current_track_inner(true);
    }
    fn start_current_track_inner(&self, restart: bool) {
        let Some((source_id, entry, next_entry, position_seconds, playback_settings)) =
            self.current_playback_request()
        else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("Queue is empty.".to_string()));
            return;
        };
        if !restart && self.current_playback_start_matches(&source_id, &entry, position_seconds) {
            return;
        }
        self.cancel_waveform_warm();
        let request_generation =
            next_playback_request_generation(&self.playback_request_generation);
        self.commit_current_playback_start(&source_id, &entry, position_seconds);
        let waveform_enabled = self.load_settings().seekbar_waveform_enabled;
        let controller = self.clone();
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let playback = Arc::clone(&self.playback);
        let queue = Arc::clone(&self.queue);
        let next_preload = Arc::clone(&self.next_preload);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let events = self.events.clone();
        thread::spawn(move || {
            if restart {
                thread::sleep(PLAYBACK_REQUEST_SETTLE);
            }
            if !request_generation_match(
                &playback_request_generation,
                request_generation,
                &queue,
                &source_id,
                &entry,
            ) {
                debug!(
                    request_generation,
                    track_id = %entry.track_id.as_str(),
                    "discarded stale playback request before resolve"
                );
                return;
            }
            let resolve_started = Instant::now();
            let mut lock_retried = false;
            let item = loop {
                match resolve_prepared_item(
                    &store,
                    &runtime,
                    &secrets,
                    &source_id,
                    &entry,
                    &playback_settings,
                ) {
                    Ok(item) => break item,
                    Err(error) => {
                        if request_generation_match(
                            &playback_request_generation,
                            request_generation,
                            &queue,
                            &source_id,
                            &entry,
                        ) && !lock_retried
                            && playback_resolve_error_is_transient(&error)
                        {
                            lock_retried = true;
                            debug!(%error, "retrying playback resolve while store is busy");
                            thread::sleep(PLAYBACK_LOCK_RETRY);
                            continue;
                        }
                        if request_generation_match(
                            &playback_request_generation,
                            request_generation,
                            &queue,
                            &source_id,
                            &entry,
                        ) {
                            controller.report_playback(PlaybackReportKind::Stopped, true);
                            controller.clear_playback_activity();
                            if let Ok(mut snapshot) = playback_snapshot.lock() {
                                snapshot.state = PlaybackState::Stopped;
                                snapshot.buffering_percent = None;
                                snapshot.last_error = Some(error.clone());
                            }
                            controller.emit_playback_snapshot();
                            let _sent = events.send(ControllerEvent::Error(error));
                        }
                        return;
                    }
                }
            };
            if !request_generation_match(
                &playback_request_generation,
                request_generation,
                &queue,
                &source_id,
                &entry,
            ) {
                debug!(
                    request_generation,
                    track_id = %entry.track_id.as_str(),
                    elapsed_ms = resolve_started.elapsed().as_millis(),
                    "discarded stale playback request after resolve"
                );
                return;
            }
            debug!(
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
            clear_next_preload(&next_preload);
            if let Err(error) = playback
                .lock()
                .map_err(|_| "playback lock was poisoned".to_string())
                .and_then(|mut playback| playback.send(command).map_err(|error| error.to_string()))
            {
                controller.report_playback(PlaybackReportKind::Stopped, true);
                controller.clear_playback_activity();
                if let Ok(mut snapshot) = playback_snapshot.lock() {
                    snapshot.state = PlaybackState::Stopped;
                    snapshot.buffering_percent = None;
                    snapshot.last_error = Some(error.clone());
                }
                controller.emit_playback_snapshot();
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if !request_generation_match(
                &playback_request_generation,
                request_generation,
                &queue,
                &source_id,
                &entry,
            ) {
                return;
            }
            controller.accept_current_playback_start(&source_id, &entry, position_seconds);
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
                    next_preload,
                    events.clone(),
                );
            }
            if waveform_enabled {
                request_waveform_for_prepared_item(
                    playback_snapshot,
                    events,
                    source_id,
                    entry,
                    waveform_item,
                );
            }
            controller.warm_waveforms_for_queue();
        });
    }
    fn commit_current_playback_start(
        &self,
        source_id: &SourceId,
        entry: &QueueEntry,
        position_seconds: u32,
    ) {
        self.update_playback_snapshot(|snapshot| {
            snapshot.current_source_id = Some(source_id.clone());
            snapshot.current = Some(entry.clone());
            snapshot.state = PlaybackState::Buffering;
            snapshot.position_seconds = position_seconds;
            snapshot.position_millis = u64::from(position_seconds) * 1_000;
            snapshot.duration_seconds = entry.duration_seconds;
            snapshot.buffering_percent = None;
            snapshot.last_error = None;
            set_waveform_cache_key(
                snapshot,
                Some(waveform_cache_key(
                    source_id,
                    &entry.track_id,
                    entry.duration_seconds,
                )),
            );
        });
        self.emit_playback_snapshot();
    }
    fn accept_current_playback_start(
        &self,
        source_id: &SourceId,
        entry: &QueueEntry,
        position_seconds: u32,
    ) {
        self.start_playback_activity(source_id, entry, position_seconds);
        self.report_playback(PlaybackReportKind::Started, false);
    }
    fn current_playback_start_matches(
        &self,
        source_id: &SourceId,
        entry: &QueueEntry,
        position_seconds: u32,
    ) -> bool {
        self.playback_snapshot.lock().ok().is_some_and(|snapshot| {
            matches!(
                snapshot.state,
                PlaybackState::Buffering | PlaybackState::Playing
            ) && snapshot.position_seconds == position_seconds
                && snapshot.current_source_id.as_ref() == Some(source_id)
                && snapshot.current.as_ref().is_some_and(|current| {
                    current.id == entry.id && current.track_id == entry.track_id
                })
        })
    }
    pub(in crate::controller) fn current_queue_entry(&self) -> Option<(SourceId, QueueEntry, u32)> {
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let entry = queue.current()?.clone();
            Some((queue.source_id().clone(), entry, queue.progress_seconds()))
        })
    }
    pub(in crate::controller) fn current_playback_entry(
        &self,
    ) -> Option<(SourceId, QueueEntry, u32)> {
        self.playback_snapshot.lock().ok().and_then(|snapshot| {
            let source_id = snapshot.current_source_id.clone()?;
            let entry = snapshot.current.clone()?;
            Some((source_id, entry, snapshot.position_seconds))
        })
    }
    pub(in crate::controller) fn set_queue_progress_for_playback_current(
        &self,
        seconds: u32,
    ) -> bool {
        let Some((source_id, entry, _position)) = self.current_playback_entry() else {
            return false;
        };
        self.queue
            .lock()
            .ok()
            .and_then(|mut queue| {
                let queue = queue.as_mut()?;
                if !queue_current_matches(queue, &source_id, &entry) {
                    return None;
                }
                queue.set_progress_seconds(seconds);
                Some(())
            })
            .is_some()
    }
    pub(in crate::controller) fn current_playback_request(
        &self,
    ) -> Option<(
        SourceId,
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
                queue.source_id().clone(),
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
            Arc::clone(&self.next_preload),
            self.events.clone(),
        );
    }
}

fn queue_current_matches(queue: &QueueEngine, source_id: &SourceId, entry: &QueueEntry) -> bool {
    queue.source_id() == source_id
        && queue
            .current()
            .is_some_and(|current| current.id == entry.id && current.track_id == entry.track_id)
}

fn playback_resolve_error_is_transient(error: &str) -> bool {
    error.contains("database is locked") || error.contains("database table is locked")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_resolve_transient_error() {
        assert!(playback_resolve_error_is_transient(
            "sqlite failed: database is locked"
        ));
        assert!(playback_resolve_error_is_transient(
            "sqlite failed: database table is locked"
        ));
        assert!(!playback_resolve_error_is_transient(
            "sqlite failed: disk I/O"
        ));
    }
}
