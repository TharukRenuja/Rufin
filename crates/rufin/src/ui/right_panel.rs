use std::rc::Rc;

use adw::prelude::*;
use domain::RightSidebarMode;

use crate::i18n::tr;
use crate::lyrics::LyricsPane;

use super::{Shell, icon_button, layout::MIN_RESTORED_WINDOW_HEIGHT, player::BOTTOM_PLAYER_HEIGHT};

const QUEUE_LYRICS_DEFAULT_LYRICS_HEIGHT: i32 = 300;

pub(super) struct RightPanelParts {
    pub(super) root: gtk::Box,
    pub(super) queue_panel: gtk::Box,
    pub(super) queue_search: gtk::SearchEntry,
    pub(super) queue_clear_button: gtk::Button,
    pub(super) queue_lyrics_split: gtk::Paned,
    pub(super) lyrics_pane: LyricsPane,
}

pub(super) fn build_right_panel() -> RightPanelParts {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("right-panel");
    root.set_hexpand(true);
    root.set_vexpand(true);

    let queue_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    queue_header.add_css_class("sidebar-header");
    queue_header.add_css_class("queue-toolbar");
    queue_header.set_valign(gtk::Align::Center);
    queue_header.set_margin_top(6);
    queue_header.set_margin_bottom(2);
    queue_header.set_margin_start(12);
    queue_header.set_margin_end(96);

    let queue_search = gtk::SearchEntry::new();
    queue_search.add_css_class("queue-search");
    let search_label = tr("Search queue");
    queue_search.update_property(&[gtk::accessible::Property::Label(&search_label)]);
    queue_search.set_hexpand(true);
    queue_search.set_height_request(34);
    queue_header.append(&queue_search);

    let queue_clear_button = icon_button("edit-clear-symbolic", "Clear queue");
    queue_header.append(&queue_clear_button);
    root.append(&queue_header);

    let queue_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    queue_panel.add_css_class("queue-panel");
    queue_panel.set_vexpand(true);
    queue_panel.set_margin_top(8);
    queue_panel.set_margin_start(8);
    queue_panel.set_margin_end(8);
    queue_panel.set_margin_bottom(0);

    let lyrics_pane = LyricsPane::new(&tr("Lyrics"));
    let queue_lyrics_split = gtk::Paned::new(gtk::Orientation::Vertical);
    queue_lyrics_split.add_css_class("queue-lyrics-split");
    queue_lyrics_split.set_vexpand(true);
    queue_lyrics_split.set_wide_handle(false);
    queue_lyrics_split.set_resize_start_child(true);
    queue_lyrics_split.set_resize_end_child(false);
    queue_lyrics_split.set_shrink_start_child(true);
    queue_lyrics_split.set_shrink_end_child(true);
    queue_lyrics_split.set_start_child(Some(&queue_panel));
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
        if !self.state.lyrics_panel_visible.get() {
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

    pub(super) fn remember_queue_lyrics_open_position(&self) {
        if !self.state.lyrics_panel_visible.get() {
            return;
        }
        self.save_queue_lyrics_split_position(
            self.queue_lyrics_split.height(),
            self.queue_lyrics_split.position(),
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

    pub(super) fn toggle_right_panel(self: &Rc<Self>) {
        let visible = self.state.resolved_right_sidebar.get().is_visible();
        self.set_right_sidebar_visible(!visible);
    }

    pub(super) fn set_right_sidebar_visible(self: &Rc<Self>, visible: bool) {
        if !visible {
            self.remember_queue_lyrics_open_position();
        }
        let active_profile = super::layout::resolve_layout(
            &self.state.settings.borrow().layout,
            self.layout_width(),
        )
        .profile;
        self.update_app_settings("right sidebar setting", |settings| {
            let profile = match active_profile {
                super::layout::ActiveLayoutProfile::Default => &mut settings.layout.default_profile,
                super::layout::ActiveLayoutProfile::Narrow => &mut settings.layout.narrow_profile,
            };
            if visible {
                if profile.right_sidebar.is_visible() {
                    return false;
                }
                profile.right_sidebar = profile.last_visible_right_sidebar;
            } else {
                if !profile.right_sidebar.is_visible() {
                    return false;
                }
                profile.last_visible_right_sidebar = profile.right_sidebar;
                profile.right_sidebar = RightSidebarMode::Hidden;
            }
            settings.layout.sanitize();
            true
        });
        self.update_layout();
        self.queue_post_layout_route_render();
    }

    pub(super) fn update_right_panel_button(&self) {
        let visible = self.state.resolved_right_sidebar.get().is_visible();
        let label = tr(if visible {
            "Hide sidebar"
        } else {
            "Show sidebar"
        });
        self.player_controls.queue_icon_open.set(visible);
        self.player_controls.queue_icon.queue_draw();
        self.player_controls
            .queue_button
            .set_tooltip_text(Some(&label));
        self.player_controls
            .queue_button
            .update_property(&[gtk::accessible::Property::Label(&label)]);
        self.player_controls.lyrics_button.set_visible(visible);
    }

    pub(super) fn toggle_lyrics_panel(self: &Rc<Self>) {
        let visible = !self.state.resolved_right_sidebar.get().is_visible()
            || !self.state.lyrics_panel_visible.get();
        self.set_lyrics_panel_visible(visible);
    }

    pub(super) fn set_lyrics_panel_visible(self: &Rc<Self>, visible: bool) {
        if visible && !self.state.resolved_right_sidebar.get().is_visible() {
            self.set_right_sidebar_visible(true);
        }
        if !visible {
            self.remember_queue_lyrics_open_position();
        }

        if self.state.lyrics_panel_visible.replace(visible) == visible {
            self.update_lyrics_panel_button();
            return;
        }

        self.save_lyrics_panel_visibility(visible);
        self.update_lyrics_panel_button();
        apply_lyrics_panel_visibility(Rc::clone(self), visible);
    }

    pub(super) fn update_lyrics_panel_button(&self) {
        let visible = self.state.lyrics_panel_visible.get();
        let label = tr(if visible {
            "Hide lyrics"
        } else {
            "Show lyrics"
        });
        self.player_controls.lyrics_icon_open.set(visible);
        self.player_controls.lyrics_icon.queue_draw();
        self.player_controls
            .lyrics_button
            .remove_css_class("active-toggle");
        self.player_controls
            .lyrics_button
            .set_visible(self.state.resolved_right_sidebar.get().is_visible());
        self.player_controls
            .lyrics_button
            .set_tooltip_text(Some(&label));
        self.player_controls
            .lyrics_button
            .update_property(&[gtk::accessible::Property::Label(&label)]);
    }
}

pub(super) fn connect_queue_lyrics_split(shell: &Rc<Shell>) {
    let saved_height = shell.state.settings.borrow().queue_lyrics_height;
    shell
        .queue_lyrics_split
        .set_position(queue_lyrics_initial_position(
            queue_lyrics_restore_available_height(shell),
            saved_height,
        ));
    let resize_shell = Rc::clone(shell);
    shell
        .queue_lyrics_split
        .connect_position_notify(move |_| resize_shell.schedule_queue_panel_render());
}

pub(super) fn apply_lyrics_panel_visibility(shell: Rc<Shell>, visible: bool) {
    shell.lyrics_pane.widget().set_visible(visible);
    if visible {
        let available_height = queue_lyrics_restore_available_height(&shell);
        let saved_height = shell.state.settings.borrow().queue_lyrics_height;
        shell
            .queue_lyrics_split
            .set_position(queue_lyrics_initial_position(
                available_height,
                saved_height,
            ));
    } else {
        let split_height = shell.queue_lyrics_split.height();
        if split_height > 0 {
            shell.queue_lyrics_split.set_position(split_height);
        }
    }
    shell.schedule_queue_panel_render();
}

fn queue_lyrics_restore_available_height(shell: &Shell) -> i32 {
    let split_height = shell.queue_lyrics_split.height();
    if split_height > 1 {
        return split_height;
    }
    let window_height = shell.window.height();
    if window_height > MIN_RESTORED_WINDOW_HEIGHT {
        return queue_lyrics_estimated_split_height(window_height);
    }
    if let Some(window_height) = shell.state.settings.borrow().window_height
        && window_height > MIN_RESTORED_WINDOW_HEIGHT
    {
        return queue_lyrics_estimated_split_height(window_height);
    }
    queue_lyrics_estimated_split_height(900)
}

fn queue_lyrics_estimated_split_height(window_height: i32) -> i32 {
    (window_height - BOTTOM_PLAYER_HEIGHT - 48).max(1)
}

pub(super) fn queue_lyrics_saved_height(split_height: i32, position: i32) -> Option<i32> {
    if split_height <= 1 {
        return None;
    }
    Some(queue_lyrics_height_for_position(split_height, position))
}

pub(super) fn clamp_queue_lyrics_position(available_height: i32, position: i32) -> i32 {
    if available_height <= 1 {
        return available_height.max(0);
    }
    position.clamp(1, available_height - 1)
}

pub(super) fn queue_lyrics_default_position(available_height: i32) -> i32 {
    queue_lyrics_position_for_height(available_height, QUEUE_LYRICS_DEFAULT_LYRICS_HEIGHT)
}

pub(super) fn queue_lyrics_height_for_position(available_height: i32, position: i32) -> i32 {
    let position = clamp_queue_lyrics_position(available_height, position);
    available_height.saturating_sub(position).max(1)
}

pub(super) fn queue_lyrics_position_for_height(available_height: i32, height: i32) -> i32 {
    if available_height <= 1 {
        return available_height.max(0);
    }
    let height = height.clamp(1, available_height - 1);
    available_height - height
}

pub(super) fn queue_lyrics_initial_position(
    available_height: i32,
    saved_height: Option<i32>,
) -> i32 {
    saved_height
        .filter(|height| *height > 0)
        .map(|height| queue_lyrics_position_for_height(available_height, height))
        .unwrap_or_else(|| queue_lyrics_default_position(available_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_keep_possible() {
        let available_height = 756;
        let position = queue_lyrics_default_position(available_height);

        assert_eq!(available_height - position, 300);
    }

    #[test]
    fn right_preserve_ideal() {
        let available_height = 360;
        let position = clamp_queue_lyrics_position(available_height, 280);

        assert_eq!(position, 280);
    }
}
