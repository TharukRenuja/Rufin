use super::{Shell, navigation};
use crate::localization::relocalize_bound_widgets;
use adw::prelude::*;
use localization::tr;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) struct LocalizationState {
    pub(super) bindings: RefCell<Vec<Box<dyn Fn()>>>,
}

impl Shell {
    pub(crate) fn relocalize_visible_ui(self: &Rc<Self>) {
        self.relocalize_static_controls();
        self.rebuild_sidebar_navigation();
        relocalize_bound_widgets();
        self.invalidate_queue_panel_render_state();
        self.render_queue_panel();
        self.render_lyrics_panel();
        self.update_bottom_player();
        self.update_fullscreen_player();
        self.update_right_panel_button();
        self.update_lyrics_panel_button();
    }

    pub(crate) fn install_locale_bindings(self: &Rc<Self>) {
        if !self.localization.bindings.borrow().is_empty() {
            return;
        }

        self.bind_locale({
            let normal_button = self.navigation_view.normal_main_menu.button.clone();
            let normal_source_button = self.navigation_view.server_selector.normal_button.clone();
            let compact_button = self.navigation_view.compact_main_menu.button.clone();
            let compact_source_button = self.navigation_view.server_selector.compact_button.clone();
            let shell = Rc::clone(self);
            move || {
                navigation::relocalize_primary_menu_button(
                    &normal_button,
                    &normal_source_button,
                    &shell.navigation_view.normal_main_menu.popover,
                    &shell.navigation_view.normal_main_menu.click_handler,
                    &shell,
                    false,
                );
                navigation::relocalize_primary_menu_button(
                    &compact_button,
                    &compact_source_button,
                    &shell.navigation_view.compact_main_menu.popover,
                    &shell.navigation_view.compact_main_menu.click_handler,
                    &shell,
                    true,
                );
            }
        });

        self.bind_locale({
            let area = self.player_view.player_controls.cover.area.clone();
            move || {
                let label = tr("Open fullscreen player");
                area.set_tooltip_text(Some(&label));
                area.update_property(&[gtk::accessible::Property::Label(&label)]);
            }
        });
        self.bind_icon_locale(
            &self.player_view.player_controls.previous_button,
            "Previous",
        );
        self.bind_icon_locale(&self.player_view.player_controls.next_button, "Next");
        self.bind_icon_locale(&self.player_view.player_controls.shuffle_button, "Shuffle");
        self.bind_icon_locale(
            &self.player_view.player_controls.random_button,
            "Play random",
        );
        self.bind_icon_locale(
            &self.player_view.player_controls.favorite_button,
            "Favorite",
        );
        self.bind_icon_locale(&self.player_view.player_controls.mute_button, "Mute");
        self.bind_icon_locale(
            &self.player_view.player_controls.audio_output_button,
            "Audio output",
        );

        self.bind_icon_locale(
            &self.player_view.fullscreen_player.close_button,
            "Close fullscreen player",
        );
        self.bind_icon_locale(
            &self.player_view.fullscreen_player.inline_close_button,
            "Close fullscreen player",
        );
        for (button, label, msgid) in &self.player_view.fullscreen_player.tabs {
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
            let stack = self.player_view.fullscreen_player.stack.clone();
            let child = self
                .player_view
                .fullscreen_player
                .lyrics_pane
                .widget()
                .clone();
            move || stack.page(&child).set_title(Some(&tr("Lyrics")))
        });
        self.bind_locale({
            let stack = self.player_view.fullscreen_player.stack.clone();
            let child = self.player_view.fullscreen_player.queue_panel.clone();
            move || stack.page(&child).set_title(Some(&tr("Queue")))
        });
        self.bind_locale({
            let stack = self.player_view.fullscreen_player.stack.clone();
            let child = self.player_view.fullscreen_player.visualizer_panel.clone();
            move || stack.page(&child).set_title(Some(&tr("Visualizer")))
        });
        self.bind_locale({
            let stack = self.player_view.fullscreen_player.stack.clone();
            let child = self.player_view.fullscreen_player.equalizer_panel.clone();
            move || stack.page(&child).set_title(Some(&tr("Equalizer")))
        });
        self.bind_label_locale(
            &self.player_view.fullscreen_player.equalizer_enabled_label,
            "Enable equalizer",
        );
        self.bind_label_locale(
            &self.player_view.fullscreen_player.equalizer_preset_label,
            "Preset",
        );
        self.bind_button_label_locale(
            &self.player_view.fullscreen_player.equalizer_reset_button,
            "Reset",
        );

        self.bind_locale({
            let entry = self.right_panel.queue_search.clone();
            move || {
                let label = tr("Search queue");
                entry.update_property(&[gtk::accessible::Property::Label(&label)]);
            }
        });
        self.bind_icon_locale(&self.right_panel.queue_clear_button, "Clear queue");
    }

    fn bind_locale(&self, update: impl Fn() + 'static) {
        let update = Box::new(update) as Box<dyn Fn()>;
        update();
        self.localization.bindings.borrow_mut().push(update);
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
        for binding in self.localization.bindings.borrow().iter() {
            binding();
        }
        self.relocalize_fullscreen_player_controls();
    }
}

fn relocalize_icon_button(button: &gtk::Button, label: &str) {
    let label = tr(label);
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);
}
