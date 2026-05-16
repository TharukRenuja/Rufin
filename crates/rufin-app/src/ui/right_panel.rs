use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tracing::warn;

use crate::i18n::tr;
use crate::lyrics::LyricsPane;

use super::{
    MAX_RESTORED_WINDOW_HEIGHT, MIN_RESTORED_WINDOW_HEIGHT, Shell, clamp_content_split_position,
    content_split_initial_position, icon_button, player::BOTTOM_PLAYER_HEIGHT,
};

const QUEUE_LYRICS_MIN_PANE_HEIGHT: i32 = 120;
const QUEUE_LYRICS_READY_MIN_HEIGHT: i32 = QUEUE_LYRICS_MIN_PANE_HEIGHT * 3;
const QUEUE_LYRICS_DEFAULT_QUEUE_UNITS: i32 = 5;
const QUEUE_LYRICS_DEFAULT_LYRICS_UNITS: i32 = 2;

pub(super) struct RightPanelParts {
    pub(super) root: gtk::Box,
    pub(super) queue_panel: gtk::Box,
    pub(super) queue_clear_button: gtk::Button,
    pub(super) queue_lyrics_split: gtk::Paned,
    pub(super) lyrics_pane: LyricsPane,
}

pub(super) fn build_right_panel() -> RightPanelParts {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("right-panel");
    root.set_vexpand(true);

    let queue_header = adw::HeaderBar::new();
    queue_header.add_css_class("sidebar-header");
    queue_header.set_show_start_title_buttons(false);
    queue_header.set_show_end_title_buttons(false);

    let queue_title = gtk::Label::new(Some(&tr("Queue")));
    queue_title.add_css_class("panel-title");
    queue_header.set_title_widget(Some(&queue_title));

    let queue_clear_button = icon_button("edit-clear-symbolic", "Clear queue");
    queue_header.pack_end(&queue_clear_button);
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
    queue_lyrics_split.set_resize_end_child(true);
    queue_lyrics_split.set_shrink_start_child(true);
    queue_lyrics_split.set_shrink_end_child(true);
    queue_lyrics_split.set_start_child(Some(&queue_panel));
    queue_lyrics_split.set_end_child(Some(lyrics_pane.widget()));
    root.append(&queue_lyrics_split);

    RightPanelParts {
        root,
        queue_panel,
        queue_clear_button,
        queue_lyrics_split,
        lyrics_pane,
    }
}

impl Shell {
    fn save_queue_lyrics_split_position(&self, available_height: i32, position: i32) {
        if !self.state.lyrics_panel_visible.get()
            || self.state.queue_lyrics_position_save_suppressed.get() > 0
        {
            return;
        }
        if available_height < QUEUE_LYRICS_READY_MIN_HEIGHT || position <= 0 {
            return;
        }
        let position = clamp_queue_lyrics_position(available_height, position);
        let ratio = queue_lyrics_position_ratio(available_height, position);
        let mut settings = self.state.settings.borrow_mut();
        if settings.queue_lyrics_position == Some(position)
            && settings.queue_lyrics_ratio == Some(ratio)
        {
            return;
        }
        settings.queue_lyrics_position = Some(position);
        settings.queue_lyrics_ratio = Some(ratio);
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save queue lyrics split position");
        }
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

    pub(super) fn save_right_panel_split_position(&self, split_width: i32, position: i32) {
        if !self.state.right_panel_visible.get() {
            return;
        }
        let mut settings = self.state.settings.borrow_mut();
        if !super::update_right_panel_split_settings(&mut settings, split_width, position) {
            return;
        }
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save right panel split position");
        }
    }

    pub(super) fn save_right_panel_visibility(&self, visible: bool) {
        let mut settings = self.state.settings.borrow_mut();
        if settings.right_panel_visible == visible {
            return;
        }
        settings.right_panel_visible = visible;
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save right panel visibility");
        }
    }

    fn save_lyrics_panel_visibility(&self, visible: bool) {
        let mut settings = self.state.settings.borrow_mut();
        if settings.lyrics_panel_visible == visible {
            return;
        }
        settings.lyrics_panel_visible = visible;
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save lyrics panel visibility");
        }
    }

    pub(super) fn remember_right_panel_open_position(&self) {
        let split_width = self.content_split.width();
        if split_width <= 1 {
            return;
        }
        let current_position = self.content_split.position();
        if current_position <= 1 || current_position >= split_width {
            return;
        }
        let position = clamp_content_split_position(split_width, current_position);
        self.state.split_position.set(position);
        self.save_right_panel_split_position(split_width, position);
    }

    pub(super) fn right_panel_open_position(&self, split_width: i32) -> i32 {
        let stored = self.state.split_position.get();
        let target = if stored > 1 && stored < split_width {
            stored
        } else {
            let saved_ratio = self.state.settings.borrow().right_panel_ratio;
            content_split_initial_position(split_width, saved_ratio)
        };
        clamp_content_split_position(split_width, target)
    }

    pub(super) fn toggle_right_panel(self: &Rc<Self>) {
        self.set_right_panel_visible(!self.state.right_panel_visible.get());
    }

    pub(super) fn set_right_panel_visible(self: &Rc<Self>, visible: bool) {
        if !visible {
            self.remember_right_panel_open_position();
        }

        if self.state.right_panel_visible.replace(visible) == visible {
            self.update_right_panel_button();
            return;
        }

        self.save_right_panel_visibility(visible);
        self.update_right_panel_button();
        apply_right_panel_visibility(Rc::clone(self), visible);
    }

    pub(super) fn update_right_panel_button(&self) {
        let visible = self.state.right_panel_visible.get();
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
        let visible =
            !self.state.right_panel_visible.get() || !self.state.lyrics_panel_visible.get();
        self.set_lyrics_panel_visible(visible);
    }

    pub(super) fn set_lyrics_panel_visible(self: &Rc<Self>, visible: bool) {
        if visible && !self.state.right_panel_visible.get() {
            self.set_right_panel_visible(true);
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
            .set_visible(self.state.right_panel_visible.get());
        self.player_controls
            .lyrics_button
            .set_tooltip_text(Some(&label));
        self.player_controls
            .lyrics_button
            .update_property(&[gtk::accessible::Property::Label(&label)]);
    }
}

pub(super) fn connect_queue_lyrics_split(shell: &Rc<Shell>) {
    let saved_ratio = shell.state.settings.borrow().queue_lyrics_ratio;
    shell
        .queue_lyrics_split
        .set_position(queue_lyrics_initial_position(
            queue_lyrics_available_height(shell),
            saved_ratio,
        ));

    let suppress_split_position_save =
        Rc::clone(&shell.state.queue_lyrics_position_save_suppressed);
    let applied_split_height = Rc::new(Cell::new(0));
    let position_shell = Rc::clone(shell);
    let suppress_for_tick = Rc::clone(&suppress_split_position_save);
    let applied_height_for_tick = Rc::clone(&applied_split_height);
    shell.queue_lyrics_split.add_tick_callback(move |split, _| {
        let available_height = split.height();
        if !position_shell.state.lyrics_panel_visible.get() {
            return glib::ControlFlow::Continue;
        }
        if available_height >= QUEUE_LYRICS_READY_MIN_HEIGHT
            && applied_height_for_tick.replace(available_height) != available_height
        {
            let saved_ratio = position_shell.state.settings.borrow().queue_lyrics_ratio;
            set_queue_lyrics_split_position_without_saving(split, &suppress_for_tick, saved_ratio);
        }
        glib::ControlFlow::Continue
    });

    let shell_for_position = Rc::clone(shell);
    let suppress_for_position = Rc::clone(&suppress_split_position_save);
    shell
        .queue_lyrics_split
        .connect_notify_local(Some("position"), move |split, _| {
            if suppress_for_position.get() > 0 {
                return;
            }
            shell_for_position.save_queue_lyrics_split_position(split.height(), split.position());
        });
}

pub(super) fn apply_right_panel_visibility(shell: Rc<Shell>, visible: bool) {
    let panel = shell.right_panel.clone();
    if panel.parent().is_none() {
        shell.content_split.set_end_child(Some(&panel));
    }

    let split_width = shell.content_split.width();
    panel.set_visible(visible);
    panel.set_opacity(if visible { 1.0 } else { 0.0 });

    if split_width > 1 {
        let position = if visible {
            shell.right_panel_open_position(split_width)
        } else {
            split_width
        };
        shell.content_split.set_position(position);
    }

    shell.update_content_split();
    shell.render_responsive_route_now();
}

pub(super) fn apply_lyrics_panel_visibility(shell: Rc<Shell>, visible: bool) {
    let suppress_save = Rc::clone(&shell.state.queue_lyrics_position_save_suppressed);
    suppress_save.set(suppress_save.get().saturating_add(1));

    shell.lyrics_pane.widget().set_visible(visible);
    let available_height = shell.queue_lyrics_split.height();
    if visible {
        let saved_ratio = shell.state.settings.borrow().queue_lyrics_ratio;
        shell
            .queue_lyrics_split
            .set_position(queue_lyrics_initial_position(available_height, saved_ratio));
    } else if available_height > 0 {
        shell.queue_lyrics_split.set_position(available_height);
    }

    let suppress = Rc::clone(&suppress_save);
    glib::idle_add_local_once(move || {
        suppress.set(suppress.get().saturating_sub(1));
    });
}

fn queue_lyrics_available_height(shell: &Shell) -> i32 {
    let panel_height = shell.right_panel.height();
    if panel_height > QUEUE_LYRICS_MIN_PANE_HEIGHT * 2 {
        return panel_height;
    }
    let window_height = shell.window.height();
    if window_height > MIN_RESTORED_WINDOW_HEIGHT {
        return (window_height - BOTTOM_PLAYER_HEIGHT - 48).max(QUEUE_LYRICS_MIN_PANE_HEIGHT * 2);
    }
    let restored_height = shell
        .state
        .settings
        .borrow()
        .window_height
        .filter(|height| *height >= MIN_RESTORED_WINDOW_HEIGHT)
        .map(|height| height.clamp(MIN_RESTORED_WINDOW_HEIGHT, MAX_RESTORED_WINDOW_HEIGHT))
        .unwrap_or(MAX_RESTORED_WINDOW_HEIGHT);
    (restored_height - BOTTOM_PLAYER_HEIGHT - 48).max(QUEUE_LYRICS_MIN_PANE_HEIGHT * 2)
}

pub(super) fn clamp_queue_lyrics_position(available_height: i32, position: i32) -> i32 {
    let max_position =
        (available_height - QUEUE_LYRICS_MIN_PANE_HEIGHT).max(QUEUE_LYRICS_MIN_PANE_HEIGHT);
    position.clamp(QUEUE_LYRICS_MIN_PANE_HEIGHT, max_position)
}

pub(super) fn queue_lyrics_default_position(available_height: i32) -> i32 {
    let total_units = QUEUE_LYRICS_DEFAULT_QUEUE_UNITS + QUEUE_LYRICS_DEFAULT_LYRICS_UNITS;
    let position = available_height * QUEUE_LYRICS_DEFAULT_QUEUE_UNITS / total_units;
    clamp_queue_lyrics_position(available_height, position)
}

pub(super) fn queue_lyrics_position_ratio(available_height: i32, position: i32) -> f64 {
    if available_height <= 0 {
        return 0.0;
    }
    f64::from(position).clamp(0.0, f64::from(available_height)) / f64::from(available_height)
}

pub(super) fn queue_lyrics_position_from_ratio(available_height: i32, ratio: f64) -> i32 {
    let position = (f64::from(available_height) * ratio.clamp(0.0, 1.0)).round() as i32;
    clamp_queue_lyrics_position(available_height, position)
}

pub(super) fn queue_lyrics_initial_position(
    available_height: i32,
    saved_ratio: Option<f64>,
) -> i32 {
    saved_ratio
        .filter(|ratio| ratio.is_finite())
        .map(|ratio| queue_lyrics_position_from_ratio(available_height, ratio))
        .unwrap_or_else(|| queue_lyrics_default_position(available_height))
}

fn set_queue_lyrics_split_position_without_saving(
    split: &gtk::Paned,
    suppress_save: &Rc<Cell<u32>>,
    saved_ratio: Option<f64>,
) {
    let available_height = split.height();
    if available_height < QUEUE_LYRICS_READY_MIN_HEIGHT {
        return;
    }

    suppress_save.set(suppress_save.get().saturating_add(1));
    split.set_position(queue_lyrics_initial_position(available_height, saved_ratio));
    let suppress = Rc::clone(suppress_save);
    glib::idle_add_local_once(move || {
        suppress.set(suppress.get().saturating_sub(1));
    });
}
