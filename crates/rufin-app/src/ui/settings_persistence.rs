use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{
    AppSettings, DensityMode, DiscordDisplayType, DiscordLinkType, HomeBlockKind, LibraryListKey,
    LibraryListSettings, PlaybackSettings, Route, ScrobblingSettings, TrackTableSettings,
};
use tracing::warn;

use super::{
    Shell, current_playback_track_id,
    layout::{restored_window_size, update_right_panel_split_settings},
};

impl Shell {
    pub(super) fn sync_auto_dj_setting_from_playback(&self, enabled: bool) {
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
        let mut settings = self.controller.load_settings();
        if !update(&mut settings) {
            return None;
        }
        settings.migrate_defaults();
        *self.state.settings.borrow_mut() = settings.clone();
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, action = warning_action, "failed to save settings");
        }
        Some(settings)
    }

    pub(super) fn set_density_mode(self: &Rc<Self>, density_mode: DensityMode) {
        self.state.density_mode.set(density_mode);
        self.update_app_settings("density setting", |settings| {
            if settings.density_mode == density_mode {
                return false;
            }
            settings.density_mode = density_mode;
            true
        });
        self.update_density();
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
            .is_some()
        {
            self.controller.reload_snapshot();
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

    pub(super) fn set_ask_lyrics_save_path(self: &Rc<Self>, enabled: bool) {
        self.update_app_settings("lyrics save path setting", |settings| {
            if settings.ask_lyrics_save_path == enabled {
                return false;
            }
            settings.ask_lyrics_save_path = enabled;
            true
        });
    }

    pub(super) fn set_lyrics_export_folder(self: &Rc<Self>, folder: Option<String>) {
        self.update_app_settings("lyrics export folder setting", |settings| {
            if settings.lyrics_export_folder == folder {
                return false;
            }
            settings.lyrics_export_folder = folder;
            true
        });
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
                if settings.lastfm_api_key == api_key {
                    return false;
                }
                settings.lastfm_api_key = api_key;
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
        self.update_app_settings(warning_action, |settings| {
            let changed = update(&mut settings.scrobbling);
            if changed {
                settings.scrobbling.sanitize();
            }
            changed
        });
    }

    pub(super) fn save_window_state(&self) {
        self.update_app_settings("window state", |settings| {
            let mut changed = false;

            if !self.window.is_maximized()
                && !self.window.is_fullscreen()
                && let Some((width, height)) =
                    restored_window_size(Some(self.window.width()), Some(self.window.height()))
                && (settings.window_width != Some(width) || settings.window_height != Some(height))
            {
                settings.window_width = Some(width);
                settings.window_height = Some(height);
                changed = true;
            }

            let density = self.right_panel_density();
            let split_position = if self.state.right_panel_visible.get() {
                self.content_split.position()
            } else {
                self.right_panel_split_position_for(density)
            };
            if update_right_panel_split_settings(
                settings,
                self.content_split.width(),
                split_position,
                density,
            ) {
                changed = true;
            }
            let right_panel_visible = self.state.right_panel_visible.get();
            if settings.right_panel_visible != right_panel_visible {
                settings.right_panel_visible = right_panel_visible;
                changed = true;
            }

            changed
        });
    }

    pub(super) fn update_track_table_settings(&self, update: impl FnOnce(&mut TrackTableSettings)) {
        self.update_app_settings("track table settings", |settings| {
            update(&mut settings.track_table);
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
