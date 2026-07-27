use std::{cell::Cell, rc::Rc, time::Duration};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::localization::{bind_widget_accessible_label, bind_widget_tooltip};
use crate::preferences::{
    dialogs::popup::present_light_dismiss_dialog, present_preferences_dialog,
};
use localization::{TRANSLATOR_CREDITS, tr};

use super::{Shell, layout, layout::ResolvedLeftSidebarMode, navigation};

pub(crate) const PLAY_ICON: &str = "rufin-play-symbolic";
pub(crate) const PLAY_NEXT_ICON: &str = "rufin-play-next-symbolic";
pub(crate) const PLAY_LATER_ICON: &str = "rufin-play-later-symbolic";
pub(crate) const EDIT_ICON: &str = "rufin-edit-symbolic";
pub(crate) const ADD_ICON: &str = "rufin-add-symbolic";
pub(crate) const REMOVE_ICON: &str = "rufin-remove-symbolic";
pub(crate) const MORE_ICON: &str = "rufin-more-symbolic";
const SORT_ORDER_ICON: &str = "rufin-sort-name-symbolic";
const SORT_ORDER_DESCENDING_ICON: &str = "rufin-sort-name-descending-symbolic";

pub(crate) fn sort_order_icon(descending: bool) -> &'static str {
    if descending {
        SORT_ORDER_DESCENDING_ICON
    } else {
        SORT_ORDER_ICON
    }
}

const KEY_SEEK_SECONDS: i32 = 10;
const KEY_VOLUME_STEP: f64 = 0.05;
const CONTROL_TOAST_TIMEOUT: u32 = 2;

pub(crate) struct ControlFeedbackState {
    pub(crate) generation: Rc<Cell<u64>>,
}

pub(crate) fn connect_shell_actions(
    shell: &Rc<Shell>,
    normal_main_menu: gtk::Button,
    compact_main_menu: gtk::Button,
) {
    install_window_actions(shell);
    navigation::install_mouse_history_buttons(shell);
    install_main_menu_shortcut(shell, normal_main_menu, compact_main_menu);
    layout::connect_shell_layout(shell);
}

pub(crate) fn install_window_actions(shell: &Rc<Shell>) {
    let go_back = gio::SimpleAction::new("go-back", None);
    let go_back_shell = Rc::clone(shell);
    go_back.connect_activate(move |_, _| go_back_shell.go_back());
    shell.chrome.window.add_action(&go_back);

    let go_forward = gio::SimpleAction::new("go-forward", None);
    let go_forward_shell = Rc::clone(shell);
    go_forward.connect_activate(move |_, _| go_forward_shell.go_forward());
    shell.chrome.window.add_action(&go_forward);

    let preferences = gio::SimpleAction::new("preferences", None);
    let preferences_shell = Rc::clone(shell);
    preferences.connect_activate(move |_, _| present_preferences_dialog(&preferences_shell));
    shell.chrome.window.add_action(&preferences);

    let troubleshooting = gio::SimpleAction::new("troubleshooting", None);
    let troubleshooting_shell = Rc::clone(shell);
    troubleshooting.connect_activate(move |_, _| {
        super::diagnostics::present_diagnostics(&troubleshooting_shell);
    });
    shell.chrome.window.add_action(&troubleshooting);

    let toggle_left_sidebar = gio::SimpleAction::new("toggle-left-sidebar", None);
    let toggle_left_sidebar_shell = Rc::clone(shell);
    toggle_left_sidebar.connect_activate(move |_, _| {
        toggle_left_sidebar_shell.toggle_active_left_sidebar_size();
    });
    shell.chrome.window.add_action(&toggle_left_sidebar);

    let toggle_private_mode = gio::SimpleAction::new("toggle-private-mode", None);
    let private_mode_shell = Rc::clone(shell);
    toggle_private_mode.connect_activate(move |_, _| {
        let enabled = !private_mode_shell.settings.current.borrow().private_mode;
        private_mode_shell.set_private_mode(enabled);
    });
    shell.chrome.window.add_action(&toggle_private_mode);

    let shortcuts = gio::SimpleAction::new("show-shortcuts", None);
    let shortcuts_shell = Rc::clone(shell);
    shortcuts.connect_activate(move |_, _| show_shortcuts_dialog(&shortcuts_shell));
    shell.chrome.window.add_action(&shortcuts);

    let fullscreen = gio::SimpleAction::new("toggle-fullscreen", None);
    let fullscreen_shell = Rc::clone(shell);
    fullscreen.connect_activate(move |_, _| {
        if fullscreen_shell.chrome.window.is_fullscreen() {
            fullscreen_shell.chrome.window.unfullscreen();
        } else {
            fullscreen_shell.chrome.window.fullscreen();
        }
    });
    shell.chrome.window.add_action(&fullscreen);

    add_window_action(shell, "play-pause", &["<Control>space"], {
        let transport = shell.products.playback.transport.clone();
        move || transport.play_pause()
    });
    add_window_action(shell, "previous-track", &["<Control>b"], {
        let transport = shell.products.playback.transport.clone();
        move || transport.previous()
    });
    add_window_action(shell, "next-track", &["<Control>n"], {
        let transport = shell.products.playback.transport.clone();
        move || transport.next()
    });
    add_window_action(shell, "seek-backward", &["<Control>Left"], {
        let shell = Rc::clone(shell);
        move || seek_by(&shell, -KEY_SEEK_SECONDS)
    });
    add_window_action(shell, "seek-forward", &["<Control>Right"], {
        let shell = Rc::clone(shell);
        move || seek_by(&shell, KEY_SEEK_SECONDS)
    });
    add_window_action(shell, "toggle-shuffle", &["<Control>s"], {
        let shell = Rc::clone(shell);
        move || toggle_shuffle_shortcut(&shell)
    });
    add_window_action(shell, "cycle-repeat", &["<Control>r"], {
        let shell = Rc::clone(shell);
        move || cycle_repeat_shortcut(&shell)
    });
    add_window_action(shell, "focus-search", &["<Control>f"], {
        let shell = Rc::clone(shell);
        move || shell.focus_current_route_search()
    });
    add_window_action(shell, "toggle-favorite", &["<Control>l"], {
        let shell = Rc::clone(shell);
        move || shell.toggle_current_track_favorite()
    });
    add_window_action(shell, "toggle-auto-dj", &["<Control>d"], {
        let shell = Rc::clone(shell);
        move || toggle_auto_dj_shortcut(&shell)
    });
    add_window_action(shell, "mute", &["<Control>m"], {
        let shell = Rc::clone(shell);
        move || toggle_mute_shortcut(&shell)
    });
    add_window_action(shell, "volume-up", &["<Control>plus", "<Control>equal"], {
        let shell = Rc::clone(shell);
        move || adjust_volume(&shell, KEY_VOLUME_STEP)
    });
    add_window_action(shell, "volume-down", &["<Control>minus"], {
        let shell = Rc::clone(shell);
        move || adjust_volume(&shell, -KEY_VOLUME_STEP)
    });
    add_window_action(shell, "toggle-queue", &["F9"], {
        let shell = Rc::clone(shell);
        move || shell.toggle_right_panel()
    });
    add_window_action(shell, "toggle-lyrics", &["F8"], {
        let shell = Rc::clone(shell);
        move || shell.toggle_lyrics_panel()
    });
    let about = gio::SimpleAction::new("about", None);
    let about_shell = Rc::clone(shell);
    about.connect_activate(move |_, _| show_about_dialog(&about_shell));
    shell.chrome.window.add_action(&about);

    let release_notes = gio::SimpleAction::new("show-release-notes", None);
    let release_notes_shell = Rc::clone(shell);
    release_notes.connect_activate(move |_, _| release_notes_shell.present_release_notes());
    shell.chrome.window.add_action(&release_notes);

    shell
        .chrome
        .application
        .set_accels_for_action("win.go-back", &["<Alt>Left"]);
    shell
        .chrome
        .application
        .set_accels_for_action("win.go-forward", &["<Alt>Right"]);
    shell
        .chrome
        .application
        .set_accels_for_action("win.preferences", &["<Control>comma"]);
    shell
        .chrome
        .application
        .set_accels_for_action("win.show-shortcuts", &["<Control>question"]);
    shell
        .chrome
        .application
        .set_accels_for_action("win.toggle-fullscreen", &["F11"]);
}

pub(crate) fn install_main_menu_shortcut(
    shell: &Rc<Shell>,
    normal_main_menu: gtk::Button,
    compact_main_menu: gtk::Button,
) {
    let key_controller = gtk::EventControllerKey::new();
    let shortcut_shell = Rc::clone(shell);
    key_controller.connect_key_pressed(move |_, key, _, state| {
        if key == gtk::gdk::Key::F10 && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
            match shortcut_shell.left_sidebar_mode() {
                ResolvedLeftSidebarMode::Compact => {
                    navigation::popup_primary_menu(
                        &shortcut_shell.navigation_view.compact_main_menu.popover,
                    );
                    compact_main_menu.grab_focus();
                }
                _ => {
                    navigation::popup_primary_menu(
                        &shortcut_shell.navigation_view.normal_main_menu.popover,
                    );
                    normal_main_menu.grab_focus();
                }
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    shell.chrome.window.add_controller(key_controller);
}

fn add_window_action(
    shell: &Rc<Shell>,
    name: &str,
    accels: &[&str],
    activate: impl Fn() + 'static,
) {
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| activate());
    shell.chrome.window.add_action(&action);
    if !accels.is_empty() {
        shell
            .chrome
            .application
            .set_accels_for_action(&format!("win.{name}"), accels);
    }
}

fn seek_by(shell: &Shell, delta_seconds: i32) {
    let Some(seconds) = ({
        let player = shell.playback.player.borrow();
        let Some(player) = player.as_ref() else {
            return;
        };
        let duration_seconds =
            (player.transport.duration_millis / 1_000).min(u64::from(u32::MAX)) as u32;
        if player.transport.current.is_none() || duration_seconds == 0 {
            None
        } else {
            let position_seconds =
                (player.transport.position_millis / 1_000).min(u64::from(u32::MAX)) as u32;
            let target = position_seconds as i32 + delta_seconds;
            Some(target.clamp(0, duration_seconds as i32) as u32)
        }
    }) else {
        return;
    };
    shell.products.playback.transport.seek_seconds(seconds);
}

fn adjust_volume(shell: &Rc<Shell>, delta: f64) {
    let Some(volume) = shell
        .playback
        .player
        .borrow()
        .as_ref()
        .map(|player| (player.controls.volume + delta).clamp(0.0, 1.0))
    else {
        return;
    };
    shell.apply_user_volume(volume);
}

fn toggle_shuffle_shortcut(shell: &Shell) {
    let Some(enabled) = shell
        .playback
        .player
        .borrow()
        .as_ref()
        .map(|player| !player.controls.shuffle_enabled)
    else {
        return;
    };
    shell.products.playback.transport.toggle_shuffle();
    let title = if enabled {
        tr("Shuffle on")
    } else {
        tr("Shuffle off")
    };
    shell.show_control_feedback_toast(title);
}

fn cycle_repeat_shortcut(shell: &Shell) {
    let Some(repeat_mode) = shell
        .playback
        .player
        .borrow()
        .as_ref()
        .map(|player| player.controls.repeat_mode)
    else {
        return;
    };
    let title = match repeat_mode {
        playback::RepeatMode::Off => tr("Repeat all"),
        playback::RepeatMode::All => tr("Repeat one"),
        playback::RepeatMode::One => tr("Repeat off"),
    };
    shell.products.playback.transport.cycle_repeat();
    shell.show_control_feedback_toast(title);
}

fn toggle_auto_dj_shortcut(shell: &Shell) {
    let Some(enabled) = shell
        .playback
        .player
        .borrow()
        .as_ref()
        .map(|player| !player.controls.auto_dj_enabled)
    else {
        return;
    };
    shell.products.playback.transport.toggle_auto_dj();
    let title = if enabled {
        tr("Auto DJ on")
    } else {
        tr("Auto DJ off")
    };
    shell.show_control_feedback_toast(title);
}

fn toggle_mute_shortcut(shell: &Rc<Shell>) {
    let Some(muted) = shell
        .playback
        .player
        .borrow()
        .as_ref()
        .map(|player| !player.controls.muted)
    else {
        return;
    };
    shell.apply_user_muted(muted);
    let title = if muted { tr("Muted") } else { tr("Unmuted") };
    shell.show_control_feedback_toast(title);
}

fn show_shortcuts_dialog(shell: &Shell) {
    let dialog = adw::ShortcutsDialog::builder()
        .title(tr("Keyboard Shortcuts"))
        .build();
    let section = adw::ShortcutsSection::new(Some(&tr("General")));
    section.add(adw::ShortcutsItem::new(&tr("Back"), "Back <Alt>Left"));
    section.add(adw::ShortcutsItem::new(
        &tr("Forward"),
        "Forward <Alt>Right",
    ));
    section.add(adw::ShortcutsItem::new(&tr("Menu"), "F10"));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Search"),
        "win.focus-search",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Preferences"),
        "win.preferences",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Keyboard Shortcuts"),
        "win.show-shortcuts",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Toggle Fullscreen"),
        "win.toggle-fullscreen",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Show/hide right sidebar"),
        "win.toggle-queue",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Show/hide lyrics"),
        "win.toggle-lyrics",
    ));
    dialog.add(section);

    let section = adw::ShortcutsSection::new(Some(&tr("Playback")));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Play/Pause"),
        "win.play-pause",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Previous"),
        "win.previous-track",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Next"),
        "win.next-track",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Seek Backward"),
        "win.seek-backward",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Seek Forward"),
        "win.seek-forward",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Shuffle"),
        "win.toggle-shuffle",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Repeat"),
        "win.cycle-repeat",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Favorite"),
        "win.toggle-favorite",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Auto DJ"),
        "win.toggle-auto-dj",
    ));
    section.add(adw::ShortcutsItem::from_action(&tr("Mute"), "win.mute"));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Volume Up"),
        "win.volume-up",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Volume Down"),
        "win.volume-down",
    ));
    dialog.add(section);
    present_light_dismiss_dialog(&dialog, &shell.chrome.window);
}

fn show_about_dialog(shell: &Shell) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Rufin")
        .application_icon("io.github.screwys.Rufin")
        .developer_name("screwy")
        .developers(["screwy <screwygit@proton.me>"])
        .translator_credits(TRANSLATOR_CREDITS)
        .version(env!("CARGO_PKG_VERSION"))
        .website("https://github.com/screwys/Rufin")
        .issue_url("https://github.com/screwys/Rufin/issues")
        .copyright("© 2026 screwy")
        .license_type(gtk::License::Custom)
        .license(
            "This application comes with absolutely no warranty and is licensed under GNU General Public Licence, version 3 or later.",
        )
        .comments(tr(
            "Thank you for trying out Rufin! If you have problems or suggestions, please open an issue in Github.",
        ))
        .build();
    present_light_dismiss_dialog(&dialog, &shell.chrome.window);
}

pub(crate) fn set_active_class(widget: &impl IsA<gtk::Widget>, active: bool) {
    if active {
        widget.add_css_class("active-toggle");
    } else {
        widget.remove_css_class("active-toggle");
    }
}

pub(crate) fn icon_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = base_icon_button(icon_name);
    bind_widget_tooltip(&button, label);
    button
}

pub(crate) fn icon_button_without_tooltip(icon_name: &str, label: &str) -> gtk::Button {
    let button = base_icon_button(icon_name);
    bind_widget_accessible_label(&button, label);
    button
}

fn base_icon_button(icon_name: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon_name);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_valign(gtk::Align::Center);
    button
}

#[derive(Clone, Copy)]
pub(crate) enum ActionButtonVariant {
    CoverSideTransport,
    CoverPrimaryTransport,
    CoverCornerMenu,
    CoverCornerFavorite,
    DetailAction,
    DetailPrimary,
    DetailFavorite,
}

pub(crate) fn configure_action_button(
    button: &gtk::Button,
    variant: ActionButtonVariant,
    icon_name: Option<&str>,
) {
    let is_cover = matches!(
        variant,
        ActionButtonVariant::CoverSideTransport
            | ActionButtonVariant::CoverPrimaryTransport
            | ActionButtonVariant::CoverCornerMenu
            | ActionButtonVariant::CoverCornerFavorite
    );
    if is_cover {
        button.add_css_class("cover-hover-button");
        button.add_css_class("cover-hover-animated");
    } else {
        button.add_css_class("detail-showcase-action-button");
    }

    let nudge_icon = match variant {
        ActionButtonVariant::CoverSideTransport => {
            button.add_css_class("cover-side-button");
            pin_action_button(button, 34);
            true
        }
        ActionButtonVariant::CoverPrimaryTransport => {
            button.add_css_class("cover-play-button");
            pin_action_button(button, 54);
            true
        }
        ActionButtonVariant::CoverCornerMenu => {
            button.add_css_class("cover-menu-button");
            pin_action_button(button, 34);
            false
        }
        ActionButtonVariant::CoverCornerFavorite => {
            button.add_css_class("cover-favorite-button");
            pin_action_button(button, 34);
            false
        }
        ActionButtonVariant::DetailAction => true,
        ActionButtonVariant::DetailPrimary => {
            button.add_css_class("detail-showcase-play-button");
            true
        }
        ActionButtonVariant::DetailFavorite => false,
    };

    if let (true, Some(icon_name)) = (nudge_icon, icon_name) {
        nudge_transport_action_icon(button, icon_name);
    }
    let face_class = if is_cover {
        "cover-hover-face"
    } else {
        "detail-showcase-action-face"
    };
    wrap_button_child_in_action_layers(button, face_class);
}

fn pin_action_button(button: &gtk::Button, size: i32) {
    button.set_size_request(size, size);
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
}

fn nudge_transport_action_icon(button: &gtk::Button, icon_name: &str) {
    let start_margin = if icon_name == PLAY_ICON {
        4
    } else if icon_name == PLAY_NEXT_ICON || icon_name == PLAY_LATER_ICON {
        2
    } else {
        return;
    };
    let Some(child) = button.child() else {
        return;
    };
    if let Ok(image) = child.downcast::<gtk::Image>() {
        image.set_margin_start(start_margin);
    }
}

fn wrap_button_child_in_action_layers(button: &gtk::Button, face_class: &str) {
    let Some(child) = button.child() else {
        return;
    };
    button.set_child(None::<&gtk::Widget>);
    child.set_halign(gtk::Align::Center);
    child.set_valign(gtk::Align::Center);

    let shadow = gtk::CenterBox::new();
    shadow.add_css_class("action-button-shadow");
    shadow.set_can_target(false);

    let face = gtk::CenterBox::new();
    face.add_css_class(face_class);
    face.set_can_target(false);
    face.set_center_widget(Some(&child));
    shadow.set_center_widget(Some(&face));
    button.set_child(Some(&shadow));
}

pub(crate) fn text_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("pill-button");
    button.add_css_class("pill");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
    button
}

impl Shell {
    pub(crate) fn show_control_feedback_toast(&self, title: String) {
        if !self.settings.current.borrow().control_notifications_enabled {
            return;
        }
        let generation = self.control_feedback.generation.get() + 1;
        self.control_feedback.generation.set(generation);
        self.chrome.control_feedback_label.set_text(&title);
        self.chrome.control_feedback_label.set_visible(true);
        let label = self.chrome.control_feedback_label.clone();
        let active_generation = Rc::clone(&self.control_feedback.generation);
        glib::timeout_add_local_once(
            Duration::from_secs(u64::from(CONTROL_TOAST_TIMEOUT)),
            move || {
                if active_generation.get() == generation {
                    label.set_visible(false);
                }
            },
        );
    }
}
