use super::*;

impl AppController {
    pub(in crate::controller) fn advance_after_end_of_stream(&self) {
        self.report_playback(PlaybackReportKind::Stopped, false);
        self.record_playback_activity();
        let mut has_next = false;
        let mut had_current = false;
        let result = self.with_queue_mut(|queue| {
            had_current = queue.current().is_some();
            has_next = queue.advance_after_end_of_stream().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !has_next && had_current && self.auto_dj_topup() {
            let result = self.with_queue_mut(|queue| {
                has_next = queue.advance_after_end_of_stream().is_some();
                Ok(())
            });
            if let Err(error) = result {
                let _sent = self.events.send(ControllerEvent::Error(error));
                return;
            }
        }
        if has_next {
            self.start_queue_emit();
            self.start_current_track();
            self.auto_dj_top_up_deferred();
        } else {
            self.stop();
        }
    }
    pub(in crate::controller) fn advance_after_prepared_track_started(&self, track: PlaybackTrack) {
        let expected_next_track_id = self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            next_queue_entry_after_current(queue).map(|entry| entry.track_id)
        });
        if expected_next_track_id.as_ref() != Some(&track.id) {
            warn!(
                expected_track_id = %track.id.as_str(),
                actual_next_track_id = expected_next_track_id
                    .as_ref()
                    .map(|id| id.as_str())
                    .unwrap_or(""),
                "ignored stale prepared playback start"
            );
            return;
        }
        self.report_playback(PlaybackReportKind::Stopped, false);
        self.record_playback_activity();
        let mut has_next = false;
        let result = self.with_queue_mut(|queue| {
            has_next = queue.advance_after_end_of_stream().is_some();
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
        self.start_queue_emit();
        self.sync_playback_snapshot_from_queue();
        self.update_playback_snapshot(|snapshot| {
            snapshot.state = PlaybackState::Playing;
            snapshot.position_seconds = 0;
            snapshot.position_millis = 0;
            snapshot.duration_seconds = track.duration_seconds;
            snapshot.buffering_percent = None;
            snapshot.last_error = None;
        });
        if let Some((server_id, entry, position_seconds)) = self.current_queue_entry() {
            self.start_playback_activity(&server_id, &entry, position_seconds);
        }
        self.emit_playback_snapshot();
        self.report_playback(PlaybackReportKind::Started, false);
        self.auto_dj_top_up_deferred();
        self.prepare_next_stream();
        self.request_waveform_for_current();
    }
}
