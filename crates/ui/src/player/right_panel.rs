use std::rc::Rc;

use crate::RightSidebarMode;
use adw::prelude::*;

use crate::shell::Shell;
use crate::shell::actions::icon_button;
use crate::shell::layout::{
    ActiveLayoutProfile, MIN_RESTORED_WINDOW_HEIGHT, WINDOW_CHROME_MARGIN_END, resolve_layout,
};
use localization::tr;

use super::bottom::BOTTOM_PLAYER_HEIGHT;
use super::lyrics::LyricsPane;

const QUEUE_LYRICS_DEFAULT_LYRICS_HEIGHT: i32 = 300;
const QUEUE_HEADER_TOP_MARGIN: i32 = 10;
const QUEUE_HEADER_BUTTON_SIZE: i32 = 34;
const QUEUE_HEADER_BUTTON_SPACING: i32 = 6;
const QUEUE_HEADER_END_MARGIN: i32 =
    WINDOW_CHROME_MARGIN_END + QUEUE_HEADER_BUTTON_SIZE + QUEUE_HEADER_BUTTON_SPACING;

pub(crate) struct RightPanelWidgets {
    pub(crate) right_split: gtk::Paned,
    pub(crate) right_panel_slot: gtk::ScrolledWindow,
    pub(crate) right_resize_handle: gtk::Box,
    pub(crate) root: gtk::Box,
    pub(crate) queue_panel: gtk::Box,
    pub(crate) queue_search: gtk::SearchEntry,
    pub(crate) queue_clear_button: gtk::Button,
    pub(crate) queue_lyrics_split: gtk::Paned,
    pub(crate) lyrics_pane: LyricsPane,
}

pub(crate) struct RightPanelParts {
    pub(crate) root: gtk::Box,
    pub(crate) queue_panel: gtk::Box,
    pub(crate) queue_search: gtk::SearchEntry,
    pub(crate) queue_clear_button: gtk::Button,
    pub(crate) queue_lyrics_split: gtk::Paned,
    pub(crate) lyrics_pane: LyricsPane,
}

pub(crate) fn build_right_panel() -> RightPanelParts {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("right-panel");
    root.set_hexpand(true);
    root.set_vexpand(true);

    let queue_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    queue_header.add_css_class("sidebar-header");
    queue_header.add_css_class("queue-toolbar");
    queue_header.set_valign(gtk::Align::Center);
    queue_header.set_margin_top(QUEUE_HEADER_TOP_MARGIN);
    queue_header.set_margin_bottom(0);
    queue_header.set_margin_start(12);
    queue_header.set_margin_end(QUEUE_HEADER_END_MARGIN);

    let queue_search = gtk::SearchEntry::new();
    queue_search.add_css_class("queue-search");
    let search_label = tr("Search queue");
    queue_search.update_property(&[gtk::accessible::Property::Label(&search_label)]);
    queue_search.set_hexpand(true);
    queue_search.set_height_request(30);
    queue_header.append(&queue_search);

    let queue_clear_button = icon_button("edit-clear-symbolic", "Clear queue");
    queue_header.append(&queue_clear_button);
    let queue_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    queue_panel.add_css_class("queue-panel");
    queue_panel.set_vexpand(true);
    queue_panel.set_margin_top(8);
    queue_panel.set_margin_start(8);
    queue_panel.set_margin_end(8);
    queue_panel.set_margin_bottom(0);

    let queue_region = gtk::Box::new(gtk::Orientation::Vertical, 0);
    queue_region.set_vexpand(true);
    queue_region.append(&queue_header);
    queue_region.append(&queue_panel);

    let lyrics_pane = LyricsPane::new(&tr("Lyrics"));
    lyrics_pane.align_header_actions_start();
    let queue_lyrics_split = gtk::Paned::new(gtk::Orientation::Vertical);
    queue_lyrics_split.add_css_class("queue-lyrics-split");
    queue_lyrics_split.set_vexpand(true);
    queue_lyrics_split.set_wide_handle(false);
    queue_lyrics_split.set_resize_start_child(true);
    queue_lyrics_split.set_resize_end_child(false);
    queue_lyrics_split.set_shrink_start_child(true);
    queue_lyrics_split.set_shrink_end_child(true);
    queue_lyrics_split.set_start_child(Some(&queue_region));
    queue_lyrics_split.set_end_child(Some(lyrics_pane.widget()));
    root.append(&queue_lyrics_split);

    RightPanelParts {
        root,
        queue_panel,
        queue_search,
        queue_clear_button,
        queue_lyrics_split,
        lyrics_pane,
    }
}

impl Shell {
    fn save_queue_lyrics_split_position(&self, split_height: i32, position: i32) {
        if !self.lyrics.panel_visible.get() {
            return;
        }
        let Some(height) = queue_lyrics_saved_height(split_height, position) else {
            return;
        };
        self.update_app_settings("queue lyrics split position", |settings| {
            if settings.queue_lyrics_height == Some(height) {
                return false;
            }
            settings.queue_lyrics_height = Some(height);
            true
        });
    }

    pub(crate) fn remember_queue_lyrics_open_position(&self) {
        if !self.lyrics.panel_visible.get() {
            return;
        }
        self.save_queue_lyrics_split_position(
            self.right_panel.queue_lyrics_split.height(),
            self.right_panel.queue_lyrics_split.position(),
        );
    }

    fn save_lyrics_panel_visibility(&self, visible: bool) {
        self.update_app_settings("lyrics panel visibility", |settings| {
            if settings.lyrics_panel_visible == visible {
                return false;
            }
            settings.lyrics_panel_visible = visible;
            true
        });
    }

    pub(crate) fn toggle_right_panel(self: &Rc<Self>) {
        let visible = self.right_sidebar_visible();
        self.set_right_sidebar_visible(!visible);
    }

    pub(crate) fn set_right_sidebar_visible(self: &Rc<Self>, visible: bool) {
        if !visible {
            self.remember_queue_lyrics_open_position();
        }
        let active_profile =
            resolve_layout(&self.settings.current.borrow().layout, self.layout_width()).profile;
        self.update_app_settings("right sidebar setting", |settings| {
            let profile = match active_profile {
                ActiveLayoutProfile::Default => &mut settings.layout.default_profile,
                ActiveLayoutProfile::Narrow => &mut settings.layout.narrow_profile,
            };
            if visible {
                if profile.right_sidebar.is_visible() {
                    return false;
                }
                profile.right_sidebar = RightSidebarMode::Visible;
            } else {
                if !profile.right_sidebar.is_visible() {
                    return false;
                }
                profile.right_sidebar = RightSidebarMode::Hidden;
            }
            settings.layout.sanitize();
            true
        });
        self.update_layout();
        self.chrome.window.queue_resize();
    }

    pub(crate) fn update_right_panel_button(&self) {
        let visible = self.right_sidebar_visible();
        let label = if visible {
            tr("Hide sidebar")
        } else {
            tr("Show sidebar")
        };
        self.player_view
            .player_controls
            .queue_icon_open
            .set(visible);
        self.player_view.player_controls.queue_icon.queue_draw();
        self.player_view
            .player_controls
            .queue_button
            .set_tooltip_text(Some(&label));
        self.player_view
            .player_controls
            .queue_button
            .update_property(&[gtk::accessible::Property::Label(&label)]);
        self.player_view
            .player_controls
            .lyrics_button
            .set_visible(visible);
    }

    pub(crate) fn toggle_lyrics_panel(self: &Rc<Self>) {
        let visible = !self.right_sidebar_visible() || !self.lyrics.panel_visible.get();
        self.set_lyrics_panel_visible(visible);
    }

    pub(crate) fn set_lyrics_panel_visible(self: &Rc<Self>, visible: bool) {
        if visible && !self.right_sidebar_visible() {
            self.set_right_sidebar_visible(true);
        }
        if !visible {
            self.remember_queue_lyrics_open_position();
        }

        if self.lyrics.panel_visible.replace(visible) == visible {
            self.update_lyrics_panel_button();
            return;
        }

        self.save_lyrics_panel_visibility(visible);
        self.update_lyrics_panel_button();
        apply_lyrics_panel_visibility(Rc::clone(self), visible);
    }

    pub(crate) fn update_lyrics_panel_button(&self) {
        let visible = self.lyrics.panel_visible.get();
        let label = if visible {
            tr("Hide lyrics")
        } else {
            tr("Show lyrics")
        };
        self.player_view
            .player_controls
            .lyrics_icon_open
            .set(visible);
        self.player_view.player_controls.lyrics_icon.queue_draw();
        self.player_view
            .player_controls
            .lyrics_button
            .remove_css_class("active-toggle");
        self.player_view
            .player_controls
            .lyrics_button
            .set_visible(self.right_sidebar_visible());
        self.player_view
            .player_controls
            .lyrics_button
            .set_tooltip_text(Some(&label));
        self.player_view
            .player_controls
            .lyrics_button
            .update_property(&[gtk::accessible::Property::Label(&label)]);
    }
}

pub(crate) fn connect_queue_lyrics_split(shell: &Rc<Shell>) {
    let saved_height = shell.settings.current.borrow().queue_lyrics_height;
    shell
        .right_panel
        .queue_lyrics_split
        .set_position(queue_lyrics_initial_position(
            queue_lyrics_restore_available_height(shell),
            saved_height,
        ));
}

pub(crate) fn apply_lyrics_panel_visibility(shell: Rc<Shell>, visible: bool) {
    shell.right_panel.lyrics_pane.widget().set_visible(visible);
    if visible {
        let available_height = queue_lyrics_restore_available_height(&shell);
        let saved_height = shell.settings.current.borrow().queue_lyrics_height;
        shell
            .right_panel
            .queue_lyrics_split
            .set_position(queue_lyrics_initial_position(
                available_height,
                saved_height,
            ));
    } else {
        let split_height = shell.right_panel.queue_lyrics_split.height();
        if split_height > 0 {
            shell
                .right_panel
                .queue_lyrics_split
                .set_position(split_height);
        }
    }
    shell.schedule_queue_panel_render();
}

fn queue_lyrics_restore_available_height(shell: &Shell) -> i32 {
    let split_height = shell.right_panel.queue_lyrics_split.height();
    if split_height > 1 {
        return split_height;
    }
    let window_height = shell.chrome.window.height();
    if window_height > MIN_RESTORED_WINDOW_HEIGHT {
        return queue_lyrics_estimated_split_height(window_height);
    }
    if let Some(window_height) = shell.settings.current.borrow().window_height
        && window_height > MIN_RESTORED_WINDOW_HEIGHT
    {
        return queue_lyrics_estimated_split_height(window_height);
    }
    queue_lyrics_estimated_split_height(900)
}

fn queue_lyrics_estimated_split_height(window_height: i32) -> i32 {
    (window_height - BOTTOM_PLAYER_HEIGHT - 48).max(1)
}

fn queue_lyrics_saved_height(split_height: i32, position: i32) -> Option<i32> {
    if split_height <= 1 {
        return None;
    }
    Some(queue_lyrics_height_for_position(split_height, position))
}

fn clamp_queue_lyrics_position(available_height: i32, position: i32) -> i32 {
    if available_height <= 1 {
        return available_height.max(0);
    }
    position.clamp(1, available_height - 1)
}

fn queue_lyrics_default_position(available_height: i32) -> i32 {
    queue_lyrics_position_for_height(available_height, QUEUE_LYRICS_DEFAULT_LYRICS_HEIGHT)
}

fn queue_lyrics_height_for_position(available_height: i32, position: i32) -> i32 {
    let position = clamp_queue_lyrics_position(available_height, position);
    available_height.saturating_sub(position).max(1)
}

fn queue_lyrics_position_for_height(available_height: i32, height: i32) -> i32 {
    if available_height <= 1 {
        return available_height.max(0);
    }
    let height = height.clamp(1, available_height - 1);
    available_height - height
}

fn queue_lyrics_initial_position(available_height: i32, saved_height: Option<i32>) -> i32 {
    saved_height
        .filter(|height| *height > 0)
        .map(|height| queue_lyrics_position_for_height(available_height, height))
        .unwrap_or_else(|| queue_lyrics_default_position(available_height))
}
