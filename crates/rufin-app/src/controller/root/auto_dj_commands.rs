use super::*;

impl AppController {
    pub fn refill_auto_dj_queue(&self) -> bool {
        if self.auto_dj_top_up_or_emit_error() {
            self.persist_and_emit_queue();
            return true;
        }
        false
    }

    pub fn toggle_auto_dj(&self) {
        let enabled = self
            .auto_dj_enabled
            .lock()
            .map(|mut current| {
                *current = !*current;
                *current
            })
            .unwrap_or(false);
        let mut settings = self.load_settings();
        settings.auto_dj_enabled = enabled;
        if let Err(error) = self.save_settings(&settings) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.auto_dj_enabled = enabled;
        });
        if !(enabled && self.refill_auto_dj_queue()) {
            self.emit_playback_snapshot();
        }
    }
}
