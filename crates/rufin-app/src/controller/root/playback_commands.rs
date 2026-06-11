use super::*;

const MAX_BACKEND_DURATION_SECONDS: u32 = 12 * 60 * 60;

impl AppController {
    pub fn play_pause(&self) {
        let state = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.state)
            .unwrap_or(PlaybackState::Stopped);
        match state {
            PlaybackState::Playing | PlaybackState::Buffering => {
                if state == PlaybackState::Buffering {
                    invalidate_playback_requests(&self.playback_request_generation);
                }
                if let Err(error) = self.send_playback_command(PlaybackCommand::Pause) {
                    let _sent = self.events.send(ControllerEvent::Error(error));
                } else {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Paused;
                        snapshot.buffering_percent = None;
                    });
                    self.persist_current_queue_snapshot();
                    self.emit_playback_snapshot();
                    self.report_playback(PlaybackReportKind::Progress, false);
                }
            }
            PlaybackState::Paused => {
                if let Err(error) = self.send_playback_command(PlaybackCommand::Resume) {
                    let _sent = self.events.send(ControllerEvent::Error(error));
                } else {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Playing;
                        snapshot.buffering_percent = None;
                    });
                    self.emit_playback_snapshot();
                    self.report_playback(PlaybackReportKind::Progress, false);
                }
            }
            PlaybackState::Stopped => self.start_current_track(),
        }
    }
    pub fn stop(&self) {
        self.invalidate_play_activation_requests();
        invalidate_playback_requests(&self.playback_request_generation);
        self.report_playback(PlaybackReportKind::Stopped, false);
        let _result = self.with_queue_mut(|queue| {
            queue.set_progress_seconds(0);
            Ok(())
        });
        if let Err(error) = self.send_playback_command(PlaybackCommand::Stop) {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.state = PlaybackState::Stopped;
            snapshot.position_seconds = 0;
            snapshot.position_millis = 0;
            snapshot.buffering_percent = None;
        });
        self.clear_playback_activity();
        self.persist_and_emit_queue();
    }
    pub fn next_track(&self) {
        self.invalidate_play_activation_requests();
        let mut moved = false;
        let mut had_current = false;
        let result = self.with_queue_mut(|queue| {
            had_current = queue.current().is_some();
            moved = queue.next_track().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !moved {
            if had_current {
                if self.auto_dj_topup() {
                    let result = self.with_queue_mut(|queue| {
                        moved = queue.next_track().is_some();
                        Ok(())
                    });
                    if let Err(error) = result {
                        let _sent = self.events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
                if !moved {
                    self.seek(0);
                    return;
                }
            } else {
                self.stop();
                return;
            }
        }
        self.record_current_skip_if_needed();
        self.start_queue_emit();
        self.start_current_track();
        self.auto_dj_top_up_deferred();
    }
    pub fn previous_track(&self) {
        self.invalidate_play_activation_requests();
        let should_restart_current = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.position_seconds > 10)
            .unwrap_or(false);
        if should_restart_current {
            self.seek(0);
            return;
        }
        let mut moved = false;
        let result = self.with_queue_mut(|queue| {
            moved = queue.previous_track().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !moved {
            self.seek(0);
            return;
        }
        self.start_queue_emit();
        self.start_current_track();
        self.auto_dj_top_up_deferred();
    }
    pub fn seek(&self, seconds: u32) {
        self.seek_millis(u64::from(seconds) * 1_000);
    }
    pub fn seek_millis(&self, millis: u64) {
        let seconds = (millis / 1_000).min(u64::from(u32::MAX)) as u32;
        let _result = self.with_queue_mut(|queue| {
            queue.set_progress_seconds(seconds);
            Ok(())
        });
        if let Err(error) = self.send_playback_command(PlaybackCommand::SeekMillis(millis)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        let queue_snapshot = self.queue_snapshot();
        if let Some(snapshot) = &queue_snapshot {
            self.persist_queue_snapshot_deferred(snapshot.clone());
        }
        self.sync_playback_snapshot_from_queue();
        self.update_playback_snapshot(|snapshot| {
            snapshot.position_seconds = seconds;
            snapshot.position_millis = millis;
        });
        self.record_playback_activity_progress(seconds);
        self.emit_playback_snapshot();
    }
    pub fn set_volume(&self, volume: f64) {
        let volume = volume.clamp(0.0, 1.0);
        if let Err(error) = self.send_playback_command(PlaybackCommand::SetVolume(volume)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        } else {
            self.persist_playback_settings(|settings| {
                settings.volume = volume;
            });
            self.update_playback_snapshot(|snapshot| {
                snapshot.volume = volume;
            });
            self.emit_playback_snapshot();
        }
    }
    pub fn toggle_mute(&self) {
        let muted = self
            .playback_snapshot
            .lock()
            .map(|snapshot| !snapshot.muted)
            .unwrap_or(true);
        if let Err(error) = self.send_playback_command(PlaybackCommand::SetMuted(muted)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        } else {
            self.persist_playback_settings(|settings| {
                settings.muted = muted;
            });
            self.update_playback_snapshot(|snapshot| {
                snapshot.muted = muted;
            });
            self.emit_playback_snapshot();
        }
    }
    pub fn update_playback_settings(&self, mut playback_settings: PlaybackSettings) {
        playback_settings.sanitize();
        let mut settings = self.load_settings();
        if settings.playback != playback_settings {
            settings.playback = playback_settings.clone();
            if let Err(error) = self.save_settings(&settings) {
                let _sent = self.events.send(ControllerEvent::Error(error));
                return;
            }
        }
        if let Err(error) =
            self.send_playback_command(PlaybackCommand::UpdateSettings(playback_settings.clone()))
        {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.volume = playback_settings.volume;
            snapshot.muted = playback_settings.muted;
        });
        self.prepare_next_stream();
        self.emit_playback_snapshot();
    }
    pub fn set_visualizer_enabled(&self, enabled: bool) {
        if let Err(error) =
            self.send_playback_command(PlaybackCommand::SetVisualizerEnabled(enabled))
        {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
    }
    pub fn poll_playback_events(&self) {
        let events = self
            .playback
            .lock()
            .map(|mut playback| playback.drain_events())
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }
        let mut playback_changed = false;
        let mut track_boundary_handled = false;
        for event in events {
            match event {
                PlaybackEvent::StateChanged(state) => {
                    if track_boundary_handled {
                        continue;
                    }
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = state;
                        snapshot.buffering_percent = None;
                    });
                    playback_changed = true;
                }
                PlaybackEvent::PositionChanged {
                    track_id,
                    seconds,
                    millis,
                } => {
                    if track_boundary_handled {
                        continue;
                    }
                    let accepting_position = self
                        .playback_snapshot
                        .lock()
                        .map(|snapshot| {
                            snapshot.state != PlaybackState::Stopped
                                && timing_event_matches_current(&snapshot, track_id.as_ref())
                        })
                        .unwrap_or(false);
                    if !accepting_position {
                        continue;
                    }
                    let _result = self.with_queue_mut(|queue| {
                        queue.set_progress_seconds(seconds);
                        Ok(())
                    });
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.position_seconds = seconds;
                        snapshot.position_millis = millis;
                    });
                    self.record_playback_activity_progress(seconds);
                    self.persist_progress_if_needed(seconds);
                    self.report_playback_progress_if_needed(seconds);
                    playback_changed = true;
                }
                PlaybackEvent::DurationChanged { track_id, seconds } => {
                    if track_boundary_handled {
                        continue;
                    }
                    let accepting_duration = self
                        .playback_snapshot
                        .lock()
                        .map(|snapshot| timing_event_matches_current(&snapshot, track_id.as_ref()))
                        .unwrap_or(false);
                    if !accepting_duration {
                        continue;
                    }
                    let duration_is_plausible = self
                        .playback_snapshot
                        .lock()
                        .map(|snapshot| backend_duration_is_plausible(&snapshot, seconds))
                        .unwrap_or(false);
                    if !duration_is_plausible {
                        continue;
                    }
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.duration_seconds = seconds;
                    });
                    playback_changed = true;
                }
                PlaybackEvent::Buffering(percent) => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Buffering;
                        snapshot.buffering_percent = Some(percent);
                    });
                    playback_changed = true;
                }
                PlaybackEvent::EndOfStream => {
                    self.advance_after_end_of_stream();
                    track_boundary_handled = true;
                    playback_changed = true;
                }
                PlaybackEvent::PreparedTrackStarted(track) => {
                    self.advance_after_prepared_track_started(track);
                    track_boundary_handled = true;
                    playback_changed = true;
                }
                PlaybackEvent::VolumeChanged { volume, muted } => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.volume = volume;
                        snapshot.muted = muted;
                    });
                    playback_changed = true;
                }
                PlaybackEvent::Visualizer(levels) => {
                    let _sent = self.events.send(ControllerEvent::Visualizer(levels));
                }
                PlaybackEvent::Error(error) => {
                    self.report_playback(PlaybackReportKind::Stopped, true);
                    self.clear_playback_activity();
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.last_error = Some(error.clone());
                        snapshot.state = PlaybackState::Stopped;
                        snapshot.buffering_percent = None;
                    });
                    let _sent = self.events.send(ControllerEvent::Error(error));
                    playback_changed = true;
                }
            }
        }
        if playback_changed {
            self.emit_playback_snapshot();
        }
    }
}

fn timing_event_matches_current(snapshot: &PlaybackSnapshot, track_id: Option<&TrackId>) -> bool {
    let Some(track_id) = track_id else {
        return true;
    };
    snapshot
        .current
        .as_ref()
        .is_some_and(|entry| &entry.track_id == track_id)
}

fn backend_duration_is_plausible(snapshot: &PlaybackSnapshot, seconds: u32) -> bool {
    if seconds == 0 || seconds > MAX_BACKEND_DURATION_SECONDS {
        return false;
    }
    let Some(current) = snapshot.current.as_ref() else {
        return true;
    };
    let known = current.duration_seconds;
    if known == 0 {
        return true;
    }
    let max_delta = known.max(60);
    seconds <= known.saturating_add(max_delta)
}
