use domain::AppSettings;

pub fn external_metadata_lookup(settings: &AppSettings) -> bool {
    settings.external_metadata_enabled && !settings.private_mode
}

pub fn cached_external_metadata_refs(settings: &AppSettings) -> bool {
    settings.external_metadata_enabled
}

pub fn external_lyrics_lookup(settings: &AppSettings) -> bool {
    settings.external_lyrics_enabled && !settings.private_mode
}

pub fn discord_presence(settings: &AppSettings) -> bool {
    settings.discord_presence_enabled && !settings.private_mode
}

pub fn notifications(settings: &AppSettings) -> bool {
    settings.notifications_enabled && !settings.private_mode
}

pub fn external_site_links(settings: &AppSettings) -> bool {
    settings.external_site_links.enabled && !settings.private_mode
}

pub fn playback_reporting(settings: &AppSettings) -> bool {
    !settings.private_mode
}

pub fn release_update_check(settings: &AppSettings) -> bool {
    settings.release_notifications_enabled && !settings.private_mode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_mode_blocks_outbound_activity() {
        let settings = AppSettings {
            private_mode: true,
            external_metadata_enabled: true,
            external_lyrics_enabled: true,
            discord_presence_enabled: true,
            notifications_enabled: true,
            release_notifications_enabled: true,
            ..AppSettings::default()
        };

        assert!(!external_metadata_lookup(&settings));
        assert!(cached_external_metadata_refs(&settings));
        assert!(!external_lyrics_lookup(&settings));
        assert!(!discord_presence(&settings));
        assert!(!notifications(&settings));
        assert!(!playback_reporting(&settings));
        assert!(!release_update_check(&settings));
    }
}
