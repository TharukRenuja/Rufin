use secrets::SecretStorageMode;
use ui::{Settings as UiSettings, SettingsPort};

use super::super::root::UiSettingsStore;

impl SettingsPort for UiSettingsStore {
    fn load(&self) -> UiSettings {
        self.load_settings().ui
    }

    fn load_with_scrobbling_secrets(&self) -> UiSettings {
        self.load_settings_with_scrobbling_secrets().ui
    }

    fn save(&self, settings: &UiSettings) -> Result<UiSettings, String> {
        let mut stored = self.load_settings();
        stored.ui = settings.clone();
        self.save_settings(&stored)?;
        Ok(self.load_settings().ui)
    }

    fn save_with_secret_deletes(&self, settings: &UiSettings) -> Result<UiSettings, String> {
        let mut stored = self.load_settings_with_scrobbling_secrets();
        stored.ui = settings.clone();
        self.save_settings_with_scrobbling_deletes(&stored)?;
        Ok(self.load_settings_with_scrobbling_secrets().ui)
    }

    fn set_secret_backend(&self, mode: SecretStorageMode) -> Result<UiSettings, String> {
        self.set_secret_storage_mode(mode)
            .map(|settings| settings.ui)
    }
}
