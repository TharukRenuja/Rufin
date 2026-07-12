use super::*;

impl AppController {
    pub fn play_pause(&self) {
        self.send_session_command(playback::SessionCommand::PlayPause);
    }

    pub fn play(&self) {
        self.send_session_command(playback::SessionCommand::Play);
    }

    pub fn pause(&self) {
        self.send_session_command(playback::SessionCommand::Pause);
    }

    pub fn stop(&self) {
        self.send_session_command(playback::SessionCommand::Stop);
    }

    pub fn next_track(&self) {
        self.send_session_command(playback::SessionCommand::Next);
    }

    pub fn previous_track(&self) {
        self.send_session_command(playback::SessionCommand::Previous);
    }

    pub fn seek(&self, seconds: u32) {
        self.seek_millis(u64::from(seconds).saturating_mul(1_000));
    }

    pub fn seek_millis(&self, millis: u64) {
        self.send_session_command(playback::SessionCommand::Seek(millis));
    }

    pub fn set_volume(&self, volume: f64) {
        self.send_session_command(playback::SessionCommand::SetVolume(volume));
    }

    pub fn persist_volume(&self, volume: f64) {
        self.set_volume(volume);
        self.send_session_command(playback::SessionCommand::PersistOutputState);
    }

    pub fn set_muted(&self, muted: bool) {
        self.send_session_command(playback::SessionCommand::SetMuted(muted));
    }

    pub fn set_visualizer_enabled(&self, enabled: bool) {
        self.send_session_command(playback::SessionCommand::SetVisualizerEnabled(enabled));
    }

    pub fn poll_playback_events(&self) {
        if let Some(product) = self.playback_product_if_present() {
            product.poll();
        }
    }

    pub fn toggle_auto_dj(&self) {
        let mut settings = self.load_settings();
        settings.auto_dj_enabled = !settings.auto_dj_enabled;
        self.save_and_apply_settings(&settings);
    }

    pub(in crate::controller) fn save_and_apply_settings(&self, settings: &StoredSettings) {
        if let Err(error) = self.save_settings(settings) {
            let _ = self.events.send(ControllerEvent::Error(error));
        }
    }
}
