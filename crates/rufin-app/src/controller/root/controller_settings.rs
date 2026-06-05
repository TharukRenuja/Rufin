use super::*;

impl AppController {
    pub fn load_settings(&self) -> AppSettings {
        self.settings.load_settings()
    }
    pub fn load_settings_with_scrobbling_secrets(&self) -> AppSettings {
        self.settings.load_settings_with_scrobbling_secrets()
    }
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.settings.save_settings(settings)
    }
    pub fn save_settings_with_scrobbling_deletes(
        &self,
        settings: &AppSettings,
    ) -> Result<(), String> {
        self.settings
            .save_settings_with_scrobbling_deletes(settings)
    }
    pub fn reload_snapshot(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || emit_snapshot(&store, &events));
    }
}
