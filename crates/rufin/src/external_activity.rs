use crate::StoredSettings;

pub fn notifications(settings: &StoredSettings) -> bool {
    settings.notifications_enabled
}

pub fn external_site_links(settings: &StoredSettings) -> bool {
    settings.external_site_links.enabled && !settings.private_mode
}

pub fn release_update_check(settings: &StoredSettings) -> bool {
    settings.release_notifications_enabled && !settings.private_mode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_mode_blocks_outbound_activity() {
        let settings = StoredSettings {
            private_mode: true,
            notifications_enabled: true,
            release_notifications_enabled: true,
            ..StoredSettings::default()
        };

        assert!(notifications(&settings));
        assert!(!release_update_check(&settings));
    }
}
