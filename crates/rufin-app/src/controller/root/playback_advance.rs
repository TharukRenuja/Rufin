use super::*;

impl AppController {
    pub(in crate::controller) fn advance_after_end_of_stream(&self) {
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
    pub(in crate::controller) fn advance_after_prepared_track_started(&self, track: PlaybackTrack) {
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
}
