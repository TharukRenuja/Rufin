use super::*;

impl AppController {
    pub(in crate::controller) fn persist_progress_if_needed(&self, seconds: u32) {
        let Some((server_id, current, _position)) = self.current_playback_entry() else {
            return;
        };
        let snapshot = self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            (queue.server_id() == &server_id
                && queue.current().is_some_and(|entry| {
                    entry.id == current.id && entry.track_id == current.track_id
                }))
            .then(|| queue.snapshot())
        });
        let Some(snapshot) = snapshot else {
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
    pub(in crate::controller) fn report_playback_progress_if_needed(&self, seconds: u32) {
        let Some(current) = self.playback_snapshot.lock().ok().and_then(|snapshot| {
            (snapshot.state == PlaybackState::Playing)
                .then(|| snapshot.current.clone())
                .flatten()
        }) else {
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
    pub(in crate::controller) fn report_playback(&self, kind: PlaybackReportKind, failed: bool) {
        let snapshot = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default();
        let Some(current) = snapshot.current.clone() else {
            return;
        };
        let Some(server_id) = snapshot.current_server_id.clone() else {
            return;
        };
        let settings = self.load_settings_with_scrobbling_secrets();
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
}
