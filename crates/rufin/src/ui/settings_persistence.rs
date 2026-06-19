use std::rc::Rc;

use crate::i18n::{self, tr};
use adw::prelude::*;
use domain::{
    AppSettings, DiscordDisplayType, DiscordLinkType, ExternalLyricsProvider, HomeBlockKind,
    LibraryListKey, LibraryListSettings, PlaybackSettings, Route, ScrobblingSettings,
    SecretStorageMode, sanitized_window_size,
};
use tracing::warn;

use super::{Shell, chrome, current_playback_track_id};

#[derive(Clone, Copy, Eq, PartialEq)]
enum AppControllerSettingsMode {
    PreserveScrobblingSecrets,
    DeleteMissingScrobblingSecrets,
}

impl Shell {
    pub(super) fn save_window_state(&self) {
        self.remember_queue_lyrics_open_position();
        if self.window.is_maximized() || self.window.is_fullscreen() {
            return;
        }
        let Some((width, height)) =
            sanitized_window_size(Some(self.window.width()), Some(self.window.height()))
        else {
            return;
        };

        self.update_app_settings("window state", |settings| {
            if settings.window_width == Some(width) && settings.window_height == Some(height) {
                return false;
            }
            settings.window_width = Some(width);
            settings.window_height = Some(height);
            true
        });
    }

    pub(super) fn sync_auto_dj(&self, enabled: bool) {
        let mut settings = self.state.settings.borrow_mut();
        if settings.auto_dj_enabled != enabled {
            settings.auto_dj_enabled = enabled;
        }
    }

    pub(super) fn update_app_settings(
        &self,
        warning_action: &'static str,
        update: impl FnOnce(&mut AppSettings) -> bool,
    ) -> Option<AppSettings> {
        self.update_app_settings_with_loader(
            warning_action,
            AppControllerSettingsMode::PreserveScrobblingSecrets,
            update,
        )
    }

    pub(super) fn update_app_settings_with_scrobbling_secrets(
        &self,
        warning_action: &'static str,
        update: impl FnOnce(&mut AppSettings) -> bool,
    ) -> Option<AppSettings> {
        self.update_app_settings_with_loader(
            warning_action,
            AppControllerSettingsMode::DeleteMissingScrobblingSecrets,
            update,
        )
    }

    fn update_app_settings_with_loader(
        &self,
        warning_action: &'static str,
        mode: AppControllerSettingsMode,
        update: impl FnOnce(&mut AppSettings) -> bool,
    ) -> Option<AppSettings> {
        let mut settings = match mode {
            AppControllerSettingsMode::PreserveScrobblingSecrets => self.controller.load_settings(),
            AppControllerSettingsMode::DeleteMissingScrobblingSecrets => {
                self.controller.load_settings_with_scrobbling_secrets()
            }
        };
        if !update(&mut settings) {
            return None;
        }
        settings.migrate_defaults();
        *self.state.settings.borrow_mut() = settings.clone();
        let save_result = if mode == AppControllerSettingsMode::DeleteMissingScrobblingSecrets {
            self.controller
                .save_settings_with_scrobbling_deletes(&settings)
        } else {
            self.controller.save_settings(&settings)
        };
        if let Err(error) = save_result {
            warn!(%error, action = warning_action, "failed to save settings");
        }
        Some(settings)
    }

    pub(super) fn retry_external_cover_lookups(self: &Rc<Self>, warning_action: &'static str) {
        if let Err(error) = self.controller.retry_external_cover_lookups() {
            warn!(%error, action = warning_action, "failed to retry external cover lookups");
            return;
        }
        self.refresh_cover_surfaces();
    }

    pub(super) fn set_external_lyrics_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("lyrics setting", |settings| {
                if settings.external_lyrics_enabled == enabled {
                    return false;
                }
                settings.external_lyrics_enabled = enabled;
                true
            })
            .is_none()
        {
            return;
        }
        *self.state.lyrics.borrow_mut() = None;
        self.state.lyrics_auto_search_attempted.borrow_mut().clear();
        self.render_lyrics_panel();
        if current_playback_track_id(&self.state.player.borrow()).is_some() {
            self.controller.refresh_lyrics_for_current();
        }
    }

    pub(super) fn set_external_metadata_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("metadata setting", |settings| {
                if settings.external_metadata_enabled == enabled {
                    return false;
                }
                settings.external_metadata_enabled = enabled;
                true
            })
            .is_none()
        {
            return;
        }
        if enabled {
            self.retry_external_cover_lookups("metadata setting");
        } else {
            self.refresh_cover_surfaces();
        }
        self.controller.reload_snapshot();
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
            self.render_current_route_preserving_scroll();
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
            self.render_current_route_preserving_scroll();
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
            self.render_current_route_preserving_scroll();
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
            self.render_current_route_preserving_scroll();
        }
    }

    pub(super) fn set_prefer_server_lyrics(self: &Rc<Self>, enabled: bool) {
        let Some(settings) = self.update_app_settings("lyrics search setting", |settings| {
            if settings.prefer_server_lyrics == enabled {
                return false;
            }
            settings.prefer_server_lyrics = enabled;
            true
        }) else {
            return;
        };
        if settings.external_lyrics_enabled
            && current_playback_track_id(&self.state.player.borrow()).is_some()
        {
            *self.state.lyrics.borrow_mut() = None;
            self.state.lyrics_auto_search_attempted.borrow_mut().clear();
            self.render_lyrics_panel();
            self.controller.refresh_lyrics_for_current();
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
        self.controller.reload_snapshot();
        self.refresh_cover_surfaces();
    }

    pub(super) fn set_external_lyrics_provider_enabled(
        self: &Rc<Self>,
        provider: ExternalLyricsProvider,
        enabled: bool,
    ) {
        let Some(settings) = self.update_app_settings("lyrics provider setting", |settings| {
            let has_provider = settings.external_lyrics_providers.contains(&provider);
            if has_provider == enabled {
                return false;
            }
            if enabled {
                settings.external_lyrics_providers.push(provider);
            } else {
                settings
                    .external_lyrics_providers
                    .retain(|candidate| *candidate != provider);
            }
            true
        }) else {
            return;
        };
        if settings.external_lyrics_enabled
            && current_playback_track_id(&self.state.player.borrow()).is_some()
        {
            *self.state.lyrics.borrow_mut() = None;
            self.state.lyrics_auto_search_attempted.borrow_mut().clear();
            self.render_lyrics_panel();
            self.controller.refresh_lyrics_for_current();
        }
    }

    pub(super) fn set_private_mode(self: &Rc<Self>, enabled: bool) {
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
        self.update_discord_presence(&self.state.player.borrow());
        self.render_lyrics_panel();
    }

    pub(super) fn set_notifications_enabled(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("notification setting", |settings| {
            if settings.notifications_enabled == enabled {
                return false;
            }
            settings.notifications_enabled = enabled;
            true
        });
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
        match self.controller.set_secret_storage_mode(mode) {
            Ok(settings) => {
                *self.state.settings.borrow_mut() = settings;
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
            self.controller.request_waveform_for_current();
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

        i18n::set_language_preference(&settings.language);
        self.relocalize_visible_ui();
        true
    }

    fn relocalize_visible_ui(self: &Rc<Self>) {
        self.relocalize_static_controls();
        self.rebuild_sidebar_navigation();
        self.render_current_route_preserving_scroll();
        self.invalidate_queue_panel_render_state();
        self.render_queue_panel();
        self.render_lyrics_panel();
        self.update_bottom_player();
        self.update_fullscreen_player();
        self.update_right_panel_button();
        self.update_lyrics_panel_button();
    }

    fn refresh_cover_surfaces(self: &Rc<Self>) {
        self.prepare_cover_retry();
        self.render_current_route_preserving_scroll();
        self.update_bottom_player();
        self.update_fullscreen_player();
        #[cfg(unix)]
        self.update_mpris_player();
    }

    pub(in crate::ui) fn install_locale_bindings(&self) {
        if !self.state.locale_bindings.borrow().is_empty() {
            return;
        }

        self.bind_locale({
            let button = self.main_menu.clone();
            move || chrome::relocalize_primary_menu_button(&button)
        });

        self.bind_locale({
            let area = self.player_controls.cover.area.clone();
            move || {
                let label = tr("Open fullscreen player");
                area.set_tooltip_text(Some(&label));
                area.update_property(&[gtk::accessible::Property::Label(&label)]);
            }
        });
        self.bind_icon_locale(&self.player_controls.previous_button, "Previous");
        self.bind_icon_locale(&self.player_controls.next_button, "Next");
        self.bind_icon_locale(&self.player_controls.shuffle_button, "Shuffle");
        self.bind_icon_locale(&self.player_controls.random_button, "Play random");
        self.bind_icon_locale(&self.player_controls.menu_button, "More actions");
        self.bind_icon_locale(&self.player_controls.favorite_button, "Favorite");
        self.bind_icon_locale(&self.player_controls.mute_button, "Mute");

        self.bind_icon_locale(
            &self.fullscreen_player.close_button,
            "Close fullscreen player",
        );
        self.bind_icon_locale(
            &self.fullscreen_player.inline_close_button,
            "Close fullscreen player",
        );
        for (button, label, msgid) in &self.fullscreen_player.tabs {
            let button = button.clone();
            let label = label.clone();
            let msgid = *msgid;
            self.bind_locale(move || {
                let text = tr(msgid);
                label.set_text(&text);
                button.set_tooltip_text(Some(&text));
                button.update_property(&[gtk::accessible::Property::Label(&text)]);
            });
        }
        self.bind_locale({
            let stack = self.fullscreen_player.stack.clone();
            let child = self.fullscreen_player.lyrics_pane.widget().clone();
            move || stack.page(&child).set_title(Some(&tr("Lyrics")))
        });
        self.bind_locale({
            let stack = self.fullscreen_player.stack.clone();
            let child = self.fullscreen_player.queue_panel.clone();
            move || stack.page(&child).set_title(Some(&tr("Queue")))
        });
        self.bind_locale({
            let stack = self.fullscreen_player.stack.clone();
            let child = self.fullscreen_player.visualizer_panel.clone();
            move || stack.page(&child).set_title(Some(&tr("Visualizer")))
        });
        self.bind_locale({
            let stack = self.fullscreen_player.stack.clone();
            let child = self.fullscreen_player.equalizer_panel.clone();
            move || stack.page(&child).set_title(Some(&tr("Equalizer")))
        });
        self.bind_label_locale(
            &self.fullscreen_player.equalizer_enabled_label,
            "Enable equalizer",
        );
        self.bind_label_locale(&self.fullscreen_player.equalizer_preset_label, "Preset");
        self.bind_button_label_locale(&self.fullscreen_player.equalizer_reset_button, "Reset");

        self.bind_locale({
            let entry = self.queue_search.clone();
            move || {
                let label = tr("Search queue");
                entry.update_property(&[gtk::accessible::Property::Label(&label)]);
            }
        });
        self.bind_icon_locale(&self.queue_clear_button, "Clear queue");
        self.bind_locale({
            let pane = self.lyrics_pane.clone();
            move || pane.set_title(&tr("Lyrics"))
        });
    }

    fn bind_locale(&self, update: impl Fn() + 'static) {
        let update = Box::new(update) as Box<dyn Fn()>;
        update();
        self.state.locale_bindings.borrow_mut().push(update);
    }

    fn bind_icon_locale(&self, button: &gtk::Button, msgid: &'static str) {
        let button = button.clone();
        self.bind_locale(move || relocalize_icon_button(&button, msgid));
    }

    fn bind_button_label_locale(&self, button: &gtk::Button, msgid: &'static str) {
        let button = button.clone();
        self.bind_locale(move || button.set_label(&tr(msgid)));
    }

    fn bind_label_locale(&self, label: &gtk::Label, msgid: &'static str) {
        let label = label.clone();
        self.bind_locale(move || label.set_text(&tr(msgid)));
    }

    fn relocalize_static_controls(&self) {
        for binding in self.state.locale_bindings.borrow().iter() {
            binding();
        }
        self.relocalize_fullscreen_player_controls();
    }

    pub(super) fn set_discord_presence_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("Discord presence setting", |settings| {
                if settings.discord_presence_enabled == enabled {
                    return false;
                }
                settings.discord_presence_enabled = enabled;
                true
            })
            .is_some()
        {
            self.retry_external_cover_lookups("Last.fm API key setting");
            self.update_discord_presence(&self.state.player.borrow());
        }
    }

    pub(super) fn set_discord_display_type(self: &Rc<Self>, display_type: DiscordDisplayType) {
        if self
            .update_app_settings("Discord display setting", |settings| {
                if settings.discord_display_type == display_type {
                    return false;
                }
                settings.discord_display_type = display_type;
                true
            })
            .is_some()
        {
            self.update_discord_presence(&self.state.player.borrow());
        }
    }

    pub(super) fn set_discord_link_type(self: &Rc<Self>, link_type: DiscordLinkType) {
        if self
            .update_app_settings("Discord link setting", |settings| {
                if settings.discord_link_type == link_type {
                    return false;
                }
                settings.discord_link_type = link_type;
                true
            })
            .is_some()
        {
            self.update_discord_presence(&self.state.player.borrow());
        }
    }

    pub(super) fn set_discord_show_paused(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("Discord paused setting", |settings| {
                if settings.discord_show_paused == enabled {
                    return false;
                }
                settings.discord_show_paused = enabled;
                true
            })
            .is_some()
        {
            self.update_discord_presence(&self.state.player.borrow());
        }
    }

    pub(super) fn set_discord_show_as_listening(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("Discord activity type setting", |settings| {
                if settings.discord_show_as_listening == enabled {
                    return false;
                }
                settings.discord_show_as_listening = enabled;
                true
            })
            .is_some()
        {
            self.update_discord_presence(&self.state.player.borrow());
        }
    }

    pub(super) fn set_discord_show_state_icon(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("Discord state icon setting", |settings| {
                if settings.discord_show_state_icon == enabled {
                    return false;
                }
                settings.discord_show_state_icon = enabled;
                true
            })
            .is_some()
        {
            self.update_discord_presence(&self.state.player.borrow());
        }
    }

    pub(super) fn set_lastfm_api_key(self: &Rc<Self>, api_key: String) {
        let api_key = api_key.trim().to_string();
        if self
            .update_app_settings("Last.fm API key setting", |settings| {
                if settings.lastfm_api_key == api_key
                    && settings.scrobbling.lastfm.api_key == api_key
                {
                    return false;
                }
                settings.lastfm_api_key = api_key;
                settings.scrobbling.lastfm.api_key = settings.lastfm_api_key.clone();
                true
            })
            .is_some()
        {
            self.update_discord_presence(&self.state.player.borrow());
        }
    }

    pub(super) fn update_scrobbling_settings(
        self: &Rc<Self>,
        warning_action: &'static str,
        update: impl FnOnce(&mut ScrobblingSettings) -> bool,
    ) {
        self.update_app_settings_with_scrobbling_secrets(warning_action, |settings| {
            let changed = update(&mut settings.scrobbling);
            if changed {
                settings.scrobbling.sanitize();
            }
            changed
        });
    }

    pub(super) fn update_library_list_settings(
        &self,
        key: LibraryListKey,
        update: impl FnOnce(&mut LibraryListSettings),
    ) {
        self.update_app_settings("library list settings", |settings| {
            if !settings.library_lists.iter().any(|entry| entry.key == key) {
                settings
                    .library_lists
                    .push(domain::LibraryListSettingsEntry {
                        key,
                        settings: LibraryListSettings::for_key(key),
                    });
            }
            if let Some(entry) = settings
                .library_lists
                .iter_mut()
                .find(|entry| entry.key == key)
            {
                update(&mut entry.settings);
                entry.settings.sanitize(key);
            }
            true
        });
    }

    pub(super) fn update_playback_settings(
        self: &Rc<Self>,
        update: impl FnOnce(&mut PlaybackSettings),
    ) {
        if let Some(settings) = self.update_app_settings("playback settings", |settings| {
            let previous = settings.playback.clone();
            update(&mut settings.playback);
            settings.playback.sanitize();
            settings.playback != previous
        }) {
            self.controller
                .update_playback_settings(settings.playback.clone());
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
        if matches!(self.state.routes.borrow().current(), Route::Home) {
            self.render_current_route();
        }
    }
}

fn relocalize_icon_button(button: &gtk::Button, label: &str) {
    let label = tr(label);
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);
}
