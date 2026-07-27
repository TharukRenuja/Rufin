use std::rc::Rc;

use desktop_integration::Settings as RichPresenceSettings;
use library::HomeBlockKind;
use localization::{default_language_preference, sanitize_language_preference};
use lyrics::Settings as LyricsSettings;
use playback::{
    DEFAULT_AUTO_DJ_REFILL_THRESHOLD, MAX_AUTO_DJ_REFILL_THRESHOLD, MIN_AUTO_DJ_REFILL_THRESHOLD,
    PlaybackSettings, RepeatMode,
};
use secrets::SecretStorageMode;
use serde::{Deserialize, Serialize};

use super::{
    ExternalSiteLinkSettings, LayoutSettings, LibraryListKey, LibraryListSettings,
    LibraryListSettingsEntry, SidebarSettings, ThemePreference, default_library_list_settings,
    sanitized_window_size,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub layout: LayoutSettings,
    #[serde(default)]
    pub sidebar: SidebarSettings,
    pub theme_preference: ThemePreference,
    #[serde(default = "default_language_preference")]
    pub language: String,
    pub private_mode: bool,
    pub notifications_enabled: bool,
    #[serde(default = "default_true")]
    pub control_notifications_enabled: bool,
    #[serde(default = "default_true")]
    pub release_notifications_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notification_seen_version: Option<String>,
    #[serde(default = "legacy_secret_storage_mode")]
    pub secret_storage_mode: SecretStorageMode,
    #[serde(flatten)]
    pub lyrics: LyricsSettings,
    #[serde(default = "default_true", rename = "external_metadata_enabled")]
    pub external_album_lookup_enabled: bool,
    #[serde(default)]
    pub external_site_links: ExternalSiteLinkSettings,
    #[serde(default)]
    pub prefer_server_playlist_covers: bool,
    #[serde(default)]
    pub seekbar_waveform_enabled: bool,
    #[serde(default)]
    pub tray_enabled: bool,
    #[serde(default)]
    pub exit_to_tray: bool,
    #[serde(default)]
    pub start_minimized: bool,
    #[serde(default = "default_true")]
    pub type_to_search_enabled: bool,
    #[serde(flatten)]
    pub rich_presence: RichPresenceSettings,
    #[serde(default)]
    pub lastfm_api_key: String,
    #[serde(default)]
    pub auto_dj_enabled: bool,
    #[serde(default)]
    pub shuffle_enabled: bool,
    #[serde(default)]
    pub repeat_mode: RepeatMode,
    #[serde(default = "default_auto_dj_refill_threshold")]
    pub auto_dj_refill_threshold: u8,
    #[serde(default)]
    pub playback: PlaybackSettings,
    #[serde(default)]
    pub home_blocks: Vec<HomeBlockKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<i32>,
    #[serde(default = "default_lyrics_panel_visible")]
    pub lyrics_panel_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_lyrics_height: Option<i32>,
    #[serde(default)]
    pub library_lists: Vec<LibraryListSettingsEntry>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            layout: LayoutSettings::default(),
            sidebar: SidebarSettings::default(),
            theme_preference: ThemePreference::System,
            language: default_language_preference(),
            private_mode: false,
            notifications_enabled: false,
            control_notifications_enabled: true,
            release_notifications_enabled: true,
            release_notification_seen_version: None,
            secret_storage_mode: SecretStorageMode::default(),
            lyrics: LyricsSettings::default(),
            external_album_lookup_enabled: true,
            external_site_links: ExternalSiteLinkSettings::default(),
            prefer_server_playlist_covers: false,
            seekbar_waveform_enabled: true,
            tray_enabled: false,
            exit_to_tray: false,
            start_minimized: false,
            type_to_search_enabled: true,
            rich_presence: RichPresenceSettings::default(),
            lastfm_api_key: String::new(),
            auto_dj_enabled: false,
            shuffle_enabled: false,
            repeat_mode: RepeatMode::Off,
            auto_dj_refill_threshold: DEFAULT_AUTO_DJ_REFILL_THRESHOLD,
            playback: PlaybackSettings::default(),
            home_blocks: default_home_blocks(),
            window_width: None,
            window_height: None,
            lyrics_panel_visible: true,
            queue_lyrics_height: None,
            library_lists: default_library_list_settings(),
        }
    }
}

impl Settings {
    pub fn allows_notifications(&self) -> bool {
        self.notifications_enabled
    }

    pub fn allows_external_site_links(&self) -> bool {
        self.external_site_links.enabled && !self.private_mode
    }

    pub fn allows_external_album_lookup(&self) -> bool {
        self.external_album_lookup_enabled && !self.private_mode
    }

    pub fn sanitize(&mut self) {
        self.rich_presence.sanitize();
        self.playback.sanitize();
        self.lyrics.sanitize();
        self.auto_dj_refill_threshold = self
            .auto_dj_refill_threshold
            .clamp(MIN_AUTO_DJ_REFILL_THRESHOLD, MAX_AUTO_DJ_REFILL_THRESHOLD);
        self.lastfm_api_key = self.lastfm_api_key.trim().to_string();
        self.language = sanitize_language_preference(&self.language);
        self.release_notification_seen_version = self
            .release_notification_seen_version
            .as_deref()
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .map(str::to_string);
        self.layout.sanitize();
        self.sidebar.sanitize();
        if !self.tray_enabled {
            self.exit_to_tray = false;
            self.start_minimized = false;
        }
        if let Some((width, height)) = sanitized_window_size(self.window_width, self.window_height)
        {
            self.window_width = Some(width);
            self.window_height = Some(height);
        } else {
            self.window_width = None;
            self.window_height = None;
        }
        sanitize_home_blocks(&mut self.home_blocks);
        migrate_library_lists(&mut self.library_lists);
    }

    pub fn library_list(&self, key: LibraryListKey) -> LibraryListSettings {
        self.library_lists
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.settings.clone())
            .unwrap_or_else(|| LibraryListSettings::for_key(key))
    }
}

pub trait SettingsPort {
    fn load(&self) -> Settings;
    fn save(&self, settings: &Settings) -> Result<Settings, String>;
}

pub type SettingsHandle = Rc<dyn SettingsPort>;

pub fn default_home_blocks() -> Vec<HomeBlockKind> {
    vec![
        HomeBlockKind::Showcase,
        HomeBlockKind::Explore,
        HomeBlockKind::MostPlayed,
        HomeBlockKind::NewlyAdded,
        HomeBlockKind::RecentlyPlayed,
        HomeBlockKind::RecentlyReleased,
        HomeBlockKind::Genres,
    ]
}

fn legacy_secret_storage_mode() -> SecretStorageMode {
    SecretStorageMode::ConfigFile
}

fn default_true() -> bool {
    true
}

fn default_lyrics_panel_visible() -> bool {
    true
}

fn default_auto_dj_refill_threshold() -> u8 {
    DEFAULT_AUTO_DJ_REFILL_THRESHOLD
}

fn sanitize_home_blocks(blocks: &mut Vec<HomeBlockKind>) {
    let mut seen = Vec::new();
    blocks.retain(|block| {
        if seen.contains(block) {
            false
        } else {
            seen.push(*block);
            true
        }
    });
    if blocks.is_empty() {
        *blocks = default_home_blocks();
    }
}

fn migrate_library_lists(lists: &mut Vec<LibraryListSettingsEntry>) {
    if lists.is_empty() {
        *lists = default_library_list_settings();
    }
    for key in LibraryListKey::all() {
        if !lists.iter().any(|entry| entry.key == key) {
            lists.push(LibraryListSettingsEntry {
                key,
                settings: LibraryListSettings::for_key(key),
            });
        }
    }
    lists.retain(|entry| LibraryListKey::all().contains(&entry.key));
    lists.sort_by_key(|entry| {
        LibraryListKey::all()
            .iter()
            .position(|key| *key == entry.key)
            .unwrap_or(usize::MAX)
    });
    for entry in lists {
        entry.settings.sanitize(entry.key);
    }
}
