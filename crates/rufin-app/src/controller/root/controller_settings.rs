impl AppController {
    pub fn load_settings(&self) -> AppSettings {
        load_settings_from_store(&self.store)
    }
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.store.save_settings(settings)
    }
    pub fn reload_snapshot(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || match load_snapshot(&store) {
            Ok(snapshot) => {
                let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
            }
            Err(error) => {
                let _sent = events.send(ControllerEvent::Error(error));
            }
        });
    }
}
