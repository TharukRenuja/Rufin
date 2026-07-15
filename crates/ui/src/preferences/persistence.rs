use std::{cell::RefCell, rc::Rc};

use crate::{
    LeftSidebarMode, LibraryListKey, LibraryListSettings, Settings as UiSettings, SettingsHandle,
};
use ::library::HomeBlockKind;
use adw::prelude::*;
use localization::set_language_preference;
use metadata::ExternalLyricsProvider;
use playback::PlaybackSettings;
use rich_presence::{DisplayType, LinkType};
use scrobbling::Settings;
use secrets::SecretStorageMode;
use tracing::warn;

use crate::player::state::{current_playback_media_key, current_playback_track_id};
use crate::routes::playlist_picker::refresh_context_playlist_picker;
use crate::shell::Shell;
use crate::shell::layout::{ActiveLayoutProfile, ResolvedLeftSidebarMode, resolve_layout};

pub(crate) struct SettingsState {
    pub(crate) current: RefCell<UiSettings>,
    pub(crate) persistence: SettingsHandle,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SettingsSaveMode {
    PreserveScrobblingSecrets,
    DeleteMissingScrobblingSecrets,
}

impl Shell {
    pub(crate) fn update_app_settings(
        &self,
        warning_action: &'static str,
        update: impl FnOnce(&mut UiSettings) -> bool,
    ) -> Option<UiSettings> {
        self.update_app_settings_with_loader(
            warning_action,
            SettingsSaveMode::PreserveScrobblingSecrets,
            update,
        )
    }

    pub(super) fn update_app_settings_with_scrobbling_secrets(
        &self,
        warning_action: &'static str,
        update: impl FnOnce(&mut UiSettings) -> bool,
    ) -> Option<UiSettings> {
        self.update_app_settings_with_loader(
            warning_action,
            SettingsSaveMode::DeleteMissingScrobblingSecrets,
            update,
        )
    }

    fn update_app_settings_with_loader(
        &self,
        warning_action: &'static str,
        mode: SettingsSaveMode,
        update: impl FnOnce(&mut UiSettings) -> bool,
    ) -> Option<UiSettings> {
        let mut settings = match mode {
            SettingsSaveMode::PreserveScrobblingSecrets => self.settings.persistence.load(),
            SettingsSaveMode::DeleteMissingScrobblingSecrets => {
                self.settings.persistence.load_with_scrobbling_secrets()
            }
        };
        if !update(&mut settings) {
            return None;
        }
        settings.sanitize();
        let save_result = if mode == SettingsSaveMode::DeleteMissingScrobblingSecrets {
            self.settings
                .persistence
                .save_with_secret_deletes(&settings)
        } else {
            self.settings.persistence.save(&settings)
        };
        match save_result {
            Ok(committed) => {
                *self.settings.current.borrow_mut() = committed.clone();
                self.schedule_source_artwork_warm();
                Some(committed)
            }
            Err(error) => {
                warn!(%error, action = warning_action, "failed to save settings");
                None
            }
        }
    }

    pub(super) fn retry_external_artwork(self: &Rc<Self>, warning_action: &'static str) {
        if let Err(error) = self.products.artwork.retry_external() {
            warn!(%error, action = warning_action, "failed to retry external artwork");
            return;
        }
        self.refresh_artwork_policy();
    }

    fn refresh_lyrics_for_changed_settings(self: &Rc<Self>) {
        let search_dialog = self
            .lyrics
            .search_dialog
            .borrow()
            .as_ref()
            .map(|dialog| dialog.dialog.clone());
        if let Some(dialog) = search_dialog {
            dialog.close();
        }
        let media_key = current_playback_media_key(&self.playback.player.borrow());
        *self.lyrics.current.borrow_mut() = None;
        self.lyrics.auto_search_attempted.borrow_mut().clear();
        *self.lyrics.loading_media.borrow_mut() = media_key.clone();
        if let Some(media_key) = media_key {
            self.lyrics
                .auto_search_attempted
                .borrow_mut()
                .insert(media_key);
            self.products.lyrics.refresh_current();
        }
        self.render_lyrics_panel();
    }

    pub(super) fn set_external_lyrics_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("lyrics setting", |settings| {
                if settings.metadata.external_lyrics_enabled == enabled {
                    return false;
                }
                settings.metadata.external_lyrics_enabled = enabled;
                true
            })
            .is_none()
        {
            return;
        }
        self.refresh_lyrics_for_changed_settings();
    }

    pub(super) fn set_external_metadata_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("metadata setting", |settings| {
                if settings.metadata.external_metadata_enabled == enabled {
                    return false;
                }
                settings.metadata.external_metadata_enabled = enabled;
                true
            })
            .is_none()
        {
            return;
        }
        if enabled {
            self.retry_external_artwork("metadata setting");
        } else {
            self.refresh_artwork_policy();
        }
    }

    pub(super) fn set_external_site_links_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("external site links setting", |settings| {
                if settings.external_site_links.enabled == enabled {
                    return false;
                }
                settings.external_site_links.enabled = enabled;
                true
            })
            .is_some()
        {
            self.reconcile_mounted_route();
        }
    }

    pub(super) fn set_lastfm_site_links_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("Last.fm site links setting", |settings| {
                if settings.external_site_links.lastfm == enabled {
                    return false;
                }
                settings.external_site_links.lastfm = enabled;
                true
            })
            .is_some()
        {
            self.reconcile_mounted_route();
        }
    }

    pub(super) fn set_musicbrainz_site_links_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("MusicBrainz site links setting", |settings| {
                if settings.external_site_links.musicbrainz == enabled {
                    return false;
                }
                settings.external_site_links.musicbrainz = enabled;
                true
            })
            .is_some()
        {
            self.reconcile_mounted_route();
        }
    }

    pub(super) fn set_server_site_links_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("server site links setting", |settings| {
                if settings.external_site_links.server == enabled {
                    return false;
                }
                settings.external_site_links.server = enabled;
                true
            })
            .is_some()
        {
            self.reconcile_mounted_route();
        }
    }

    pub(super) fn set_prefer_server_lyrics(self: &Rc<Self>, enabled: bool) {
        let Some(settings) = self.update_app_settings("lyrics search setting", |settings| {
            if settings.metadata.prefer_server_lyrics == enabled {
                return false;
            }
            settings.metadata.prefer_server_lyrics = enabled;
            true
        }) else {
            return;
        };
        if settings.metadata.external_lyrics_enabled
            && current_playback_track_id(&self.playback.player.borrow()).is_some()
        {
            self.refresh_lyrics_for_changed_settings();
        }
    }

    pub(super) fn set_prefer_server_playlist_covers(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("playlist cover setting", |settings| {
                if settings.prefer_server_playlist_covers == enabled {
                    return false;
                }
                settings.prefer_server_playlist_covers = enabled;
                true
            })
            .is_none()
        {
            return;
        }
        self.reconcile_mounted_route();
        refresh_context_playlist_picker(self);
    }

    pub(super) fn set_external_lyrics_provider_enabled(
        self: &Rc<Self>,
        provider: ExternalLyricsProvider,
        enabled: bool,
    ) {
        let Some(settings) = self.update_app_settings("lyrics provider setting", |settings| {
            let has_provider = settings
                .metadata
                .external_lyrics_providers
                .contains(&provider);
            if has_provider == enabled {
                return false;
            }
            if enabled {
                settings.metadata.external_lyrics_providers.push(provider);
            } else {
                settings
                    .metadata
                    .external_lyrics_providers
                    .retain(|candidate| *candidate != provider);
            }
            true
        }) else {
            return;
        };
        if settings.metadata.external_lyrics_enabled
            && current_playback_track_id(&self.playback.player.borrow()).is_some()
        {
            self.refresh_lyrics_for_changed_settings();
        }
    }

    pub(crate) fn set_private_mode(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("private mode setting", |settings| {
                if settings.private_mode == enabled {
                    return false;
                }
                settings.private_mode = enabled;
                true
            })
            .is_none()
        {
            return;
        }
        #[cfg(unix)]
        self.refresh_tray_private_mode();
        self.reconcile_mounted_route();
        self.refresh_artwork_policy();
        self.refresh_lyrics_for_changed_settings();
    }

    pub(super) fn set_notifications_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("notification setting", |settings| {
                if settings.notifications_enabled == enabled {
                    return false;
                }
                settings.notifications_enabled = enabled;
                true
            })
            .is_some()
            && !enabled
        {
            self.withdraw_now_playing_notification();
        }
    }

    pub(super) fn set_control_notifications_enabled(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("control notification setting", |settings| {
            if settings.control_notifications_enabled == enabled {
                return false;
            }
            settings.control_notifications_enabled = enabled;
            true
        });
    }

    pub(super) fn set_release_notifications_enabled(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("release notification setting", |settings| {
            if settings.release_notifications_enabled == enabled {
                return false;
            }
            settings.release_notifications_enabled = enabled;
            true
        });
    }

    pub(super) fn mark_release_notification_seen(self: &Rc<Self>, version: &str) {
        let version = version.trim().to_string();
        if version.is_empty() {
            return;
        }
        self.update_app_settings("release notification seen state", |settings| {
            if settings.release_notification_seen_version.as_deref() == Some(version.as_str()) {
                return false;
            }
            settings.release_notification_seen_version = Some(version);
            true
        });
    }

    pub(super) fn set_secret_storage_mode(self: &Rc<Self>, mode: SecretStorageMode) -> bool {
        match self.settings.persistence.set_secret_backend(mode) {
            Ok(settings) => {
                *self.settings.current.borrow_mut() = settings;
                true
            }
            Err(error) => {
                warn!(%error, "failed to change secret storage mode");
                false
            }
        }
    }

    pub(super) fn set_type_to_search_enabled(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("type to search setting", |settings| {
            if settings.type_to_search_enabled == enabled {
                return false;
            }
            settings.type_to_search_enabled = enabled;
            true
        });
    }

    pub(crate) fn toggle_active_left_sidebar_size(self: &Rc<Self>) {
        let next_mode = if self.left_sidebar_mode() == ResolvedLeftSidebarMode::Full {
            LeftSidebarMode::Compact
        } else {
            LeftSidebarMode::Full
        };
        self.set_active_left_sidebar_mode(next_mode);
    }

    pub(crate) fn set_active_left_sidebar_mode(self: &Rc<Self>, mode: LeftSidebarMode) {
        let active_profile =
            resolve_layout(&self.settings.current.borrow().layout, self.layout_width()).profile;
        if self
            .update_app_settings("left sidebar setting", |settings| {
                let profile = match active_profile {
                    ActiveLayoutProfile::Default => &mut settings.layout.default_profile,
                    ActiveLayoutProfile::Narrow => &mut settings.layout.narrow_profile,
                };
                if profile.left_sidebar == mode {
                    return false;
                }
                profile.left_sidebar = mode;
                settings.layout.sanitize();
                true
            })
            .is_none()
        {
            return;
        }
        self.update_layout();
        self.chrome.window.queue_resize();
    }

    pub(crate) fn save_preferred_left_sidebar_width(&self, width: i32) {
        self.update_app_settings("left sidebar width", |settings| {
            let width = width.clamp(crate::MIN_LEFT_SIDEBAR_WIDTH, crate::MAX_LEFT_SIDEBAR_WIDTH);
            if settings.layout.preferred_left_sidebar_width == width {
                return false;
            }
            settings.layout.preferred_left_sidebar_width = width;
            true
        });
    }

    pub(crate) fn save_left_sidebar_drag(self: &Rc<Self>, mode: LeftSidebarMode, width: i32) {
        let active_profile =
            resolve_layout(&self.settings.current.borrow().layout, self.layout_width()).profile;
        self.update_app_settings("left sidebar drag", |settings| {
            let profile = match active_profile {
                ActiveLayoutProfile::Default => &mut settings.layout.default_profile,
                ActiveLayoutProfile::Narrow => &mut settings.layout.narrow_profile,
            };
            let mut changed = false;
            if profile.left_sidebar != mode {
                profile.left_sidebar = mode;
                changed = true;
            }
            if mode == LeftSidebarMode::Full {
                let width =
                    width.clamp(crate::MIN_LEFT_SIDEBAR_WIDTH, crate::MAX_LEFT_SIDEBAR_WIDTH);
                if settings.layout.preferred_left_sidebar_width != width {
                    settings.layout.preferred_left_sidebar_width = width;
                    changed = true;
                }
            }
            changed
        });
        self.update_layout();
    }

    pub(crate) fn save_preferred_right_sidebar_width(&self, width: i32) {
        self.update_app_settings("right sidebar width", |settings| {
            let width = width.clamp(
                crate::MIN_RIGHT_SIDEBAR_WIDTH,
                crate::MAX_RIGHT_SIDEBAR_WIDTH,
            );
            if settings.layout.preferred_right_sidebar_width == width {
                return false;
            }
            settings.layout.preferred_right_sidebar_width = width;
            true
        });
    }

    pub(super) fn set_seekbar_waveform_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("seekbar waveform setting", |settings| {
                if settings.seekbar_waveform_enabled == enabled {
                    return false;
                }
                settings.seekbar_waveform_enabled = enabled;
                true
            })
            .is_none()
        {
            return;
        }
        self.update_bottom_player();
        if enabled {
            self.products.playback.waveform.request_current();
        }
    }

    pub(super) fn set_language_preference(self: &Rc<Self>, language: String) -> bool {
        let Some(settings) = self.update_app_settings("language setting", |settings| {
            if settings.language == language {
                return false;
            }
            settings.language = language;
            true
        }) else {
            return false;
        };

        set_language_preference(&settings.language);
        self.relocalize_visible_ui();
        true
    }

    fn refresh_artwork_policy(self: &Rc<Self>) {
        self.refresh_artwork_bindings();
        #[cfg(unix)]
        self.update_mpris_player();
    }

    pub(super) fn set_discord_presence_enabled(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("Discord presence setting", |settings| {
            if settings.rich_presence.enabled == enabled {
                return false;
            }
            settings.rich_presence.enabled = enabled;
            true
        });
    }

    pub(super) fn set_discord_display_type(self: &Rc<Self>, display_type: DisplayType) {
        self.update_app_settings("Discord display setting", |settings| {
            if settings.rich_presence.display_type == display_type {
                return false;
            }
            settings.rich_presence.display_type = display_type;
            true
        });
    }

    pub(super) fn set_discord_link_type(self: &Rc<Self>, link_type: LinkType) {
        self.update_app_settings("Discord link setting", |settings| {
            if settings.rich_presence.link_type == link_type {
                return false;
            }
            settings.rich_presence.link_type = link_type;
            true
        });
    }

    pub(super) fn set_discord_show_paused(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("Discord paused setting", |settings| {
            if settings.rich_presence.show_paused == enabled {
                return false;
            }
            settings.rich_presence.show_paused = enabled;
            true
        });
    }

    pub(super) fn set_discord_show_as_listening(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("Discord activity type setting", |settings| {
            if settings.rich_presence.show_as_listening == enabled {
                return false;
            }
            settings.rich_presence.show_as_listening = enabled;
            true
        });
    }

    pub(super) fn set_discord_show_state_icon(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("Discord state icon setting", |settings| {
            if settings.rich_presence.show_state_icon == enabled {
                return false;
            }
            settings.rich_presence.show_state_icon = enabled;
            true
        });
    }

    pub(super) fn update_scrobbling_settings(
        self: &Rc<Self>,
        warning_action: &'static str,
        update: impl FnOnce(&mut Settings) -> bool,
    ) {
        self.update_app_settings_with_scrobbling_secrets(warning_action, |settings| {
            let changed = update(&mut settings.scrobbling);
            if changed {
                settings.scrobbling.sanitize();
            }
            changed
        });
    }

    pub(crate) fn update_library_list_settings(
        &self,
        key: LibraryListKey,
        update: impl FnOnce(&mut LibraryListSettings),
    ) {
        let committed = self.update_app_settings("library list settings", |settings| {
            if !settings.library_lists.iter().any(|entry| entry.key == key) {
                settings
                    .library_lists
                    .push(crate::LibraryListSettingsEntry {
                        key,
                        settings: LibraryListSettings::for_key(key),
                    });
            }
            if let Some(entry) = settings
                .library_lists
                .iter_mut()
                .find(|entry| entry.key == key)
            {
                let previous = entry.settings.clone();
                update(&mut entry.settings);
                entry.settings.sanitize(key);
                return entry.settings != previous;
            }
            false
        });
        if committed.is_some() {
            self.reconcile_mounted_route();
        }
    }

    pub(crate) fn update_playback_settings(
        self: &Rc<Self>,
        update: impl FnOnce(&mut PlaybackSettings),
    ) {
        if let Some(settings) = self.update_app_settings("playback settings", |settings| {
            let previous = settings.playback.clone();
            update(&mut settings.playback);
            settings.playback.sanitize();
            settings.playback != previous
        }) {
            self.sync_fullscreen_equalizer_controls(&settings.playback.equalizer);
            self.update_bottom_player();
        }
    }

    pub(super) fn set_home_blocks(self: &Rc<Self>, blocks: Vec<HomeBlockKind>) {
        if self
            .update_app_settings("home block settings", |settings| {
                if settings.home_blocks == blocks {
                    return false;
                }
                settings.home_blocks = blocks;
                true
            })
            .is_none()
        {
            return;
        }
        self.reconcile_mounted_route();
    }
}
