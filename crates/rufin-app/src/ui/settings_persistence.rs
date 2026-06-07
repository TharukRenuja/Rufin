use std::rc::Rc;

use crate::i18n::{self, tr};
use adw::prelude::*;
use rufin_core::{
    AppSettings, DiscordDisplayType, DiscordLinkType, HomeBlockKind, LibraryListKey,
    LibraryListSettings, PlaybackSettings, Route, ScrobblingSettings, TrackTableSettings,
    sanitized_window_size,
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
        self.controller.reload_snapshot();
        *self.state.lyrics.borrow_mut() = None;
        self.state.lyrics_auto_search_attempted.borrow_mut().clear();
        self.render_lyrics_panel();
        if current_playback_track_id(&self.state.player.borrow()).is_some() {
            self.controller.refresh_lyrics_for_current();
        }
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

    fn relocalize_static_controls(&self) {
        chrome::relocalize_primary_menu_button(&self.main_menu);
        relocalize_icon_button(&self.normal_back_button, "Back");
        relocalize_icon_button(&self.compact_back_button, "Back");
        relocalize_icon_button(&self.normal_forward_button, "Forward");
        relocalize_icon_button(&self.compact_forward_button, "Forward");

        let cover_label = tr("Open fullscreen player");
        self.player_controls
            .cover
            .area
            .set_tooltip_text(Some(&cover_label));
        self.player_controls
            .cover
            .area
            .update_property(&[gtk::accessible::Property::Label(&cover_label)]);

        relocalize_icon_button(&self.player_controls.previous_button, "Previous");
        relocalize_icon_button(&self.player_controls.next_button, "Next");
        relocalize_icon_button(&self.player_controls.shuffle_button, "Shuffle");
        relocalize_icon_button(&self.player_controls.random_button, "Play random");
        relocalize_icon_button(&self.player_controls.favorite_button, "Favorite");
        relocalize_icon_button(&self.player_controls.mute_button, "Mute");
        relocalize_icon_button(
            &self.fullscreen_player.close_button,
            "Close fullscreen player",
        );

        let search_label = tr("Search queue");
        self.queue_search
            .update_property(&[gtk::accessible::Property::Label(&search_label)]);
        relocalize_icon_button(&self.queue_clear_button, "Clear queue");
        self.lyrics_pane.set_title(&tr("Lyrics"));
        let lyrics_title = tr("Lyrics");
        self.fullscreen_player
            .stack
            .page(self.fullscreen_player.lyrics_pane.widget())
            .set_title(Some(&lyrics_title));
        let queue_title = tr("Queue");
        self.fullscreen_player
            .stack
            .page(&self.fullscreen_player.queue_panel)
            .set_title(Some(&queue_title));
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

    pub(super) fn update_track_table_settings(&self, update: impl FnOnce(&mut TrackTableSettings)) {
        self.update_app_settings("track table settings", |settings| {
            update(&mut settings.track_table);
            settings.track_table.sanitize();
            true
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
                    .push(rufin_core::LibraryListSettingsEntry {
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
            update(&mut settings.playback);
            settings.playback.sanitize();
            true
        }) {
            self.controller
                .update_playback_settings(settings.playback.clone());
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
