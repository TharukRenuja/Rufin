use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use localization::tr;

use crate::shell::Shell;
use crate::shell::actions::{PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON};

const CONTEXT_MENU_PLAYLIST_MAX_HEIGHT: i32 = 320;
const CONTEXT_MENU_PLAYLIST_MIN_WIDTH: i32 = 380;
const NATIVE_MENU_SELECTION_CLASS: &str = "rufin-menu-selection";
const NATIVE_MENU_SELECTED_CLASS: &str = "rufin-menu-selected";

pub(crate) const ADD_TO_PLAYLIST_ICON: &str = "rufin-route-playlists-symbolic";
pub(crate) const ALBUM_ICON: &str = "rufin-route-albums-symbolic";
pub(crate) const ARTIST_ICON: &str = "rufin-route-artists-symbolic";
pub(crate) const DOWNLOAD_ICON: &str = "folder-download-symbolic";
pub(crate) const RADIO_ICON: &str = "rufin-audio-radio-symbolic";

type ContextMenuOpen = Rc<dyn Fn(&gtk::Widget, Option<(f64, f64)>)>;

pub(crate) fn connect_transient_entry_focus_dismissal(shell: &Shell) {
    install_focus_dismissal(
        &shell.chrome.window,
        vec![
            shell.right_panel.queue_search.clone().upcast(),
            shell.right_panel.lyrics_pane.focus_dismiss_target(),
            shell
                .player_view
                .fullscreen_player
                .lyrics_pane
                .focus_dismiss_target(),
        ],
    );
}

fn install_focus_dismissal(window: &adw::ApplicationWindow, targets: Vec<gtk::Widget>) {
    let click_root = window.clone();
    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Denied);
        let Some(focus) = gtk::prelude::RootExt::focus(&click_root) else {
            return;
        };
        let Some(target) = targets
            .iter()
            .find(|target| target.has_focus() || focus.is_ancestor(*target))
        else {
            return;
        };
        if target.compute_bounds(&click_root).is_none_or(|bounds| {
            bounds.contains_point(&gtk::graphene::Point::new(x as f32, y as f32))
        }) {
            return;
        }
        if let Some(root) = target.root() {
            root.set_focus(None::<&gtk::Widget>);
        }
    });
    window.add_controller(click);
}

pub(crate) struct ContextMenuSurface {
    target: gtk::Widget,
    group_name: &'static str,
    menu: gio::Menu,
    popover: gtk::PopoverMenu,
    actions: gio::SimpleActionGroup,
}

impl ContextMenuSurface {
    pub(crate) fn new(
        target: &gtk::Widget,
        group_name: &'static str,
        position: Option<(f64, f64)>,
    ) -> Self {
        let menu = gio::Menu::new();
        Self {
            target: target.clone(),
            group_name,
            popover: context_popover(target, position, &menu),
            menu,
            actions: gio::SimpleActionGroup::new(),
        }
    }

    pub(crate) fn popover(&self) -> &gtk::PopoverMenu {
        &self.popover
    }

    pub(crate) fn append_action(&self, label: &str, action: &str, icon_name: &str) {
        self.menu.append_item(&menu_action_item(
            &tr(label),
            &format!("{}.{}", self.group_name, action),
            icon_name,
        ));
    }

    pub(crate) fn append_submenu(&self, label: &str, submenu: &gio::Menu, icon_name: &str) {
        let item = gio::MenuItem::new_submenu(Some(&tr(label)), submenu);
        item.set_icon(&gio::ThemedIcon::new(icon_name));
        self.menu.append_item(&item);
    }

    pub(crate) fn add_action(&self, name: &str, run: impl Fn() + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let popover = self.popover.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(popover) = popover.upgrade() {
                popdown_native_menu(&popover);
            }
            run();
        });
        self.actions.add_action(&action);
    }

    pub(crate) fn popup(self) {
        self.target
            .insert_action_group(self.group_name, Some(&self.actions));
        show_native_menu_icons(&self.popover);
        keep_parent_grab_for_nested_native_menus(&self.popover);
        let unmap_handler = Rc::new(RefCell::new(Some(popdown_on_anchor_unmap(
            &self.target,
            &self.popover,
        ))));
        let target = self.target.downgrade();
        self.popover.connect_closed(move |popover| {
            popdown_nested_native_menus_from(popover.upcast_ref());
            let popover = popover.clone();
            let target = target.clone();
            let unmap_handler = Rc::clone(&unmap_handler);
            glib::idle_add_local_once(move || {
                if let (Some(target), Some(handler)) =
                    (target.upgrade(), unmap_handler.borrow_mut().take())
                {
                    target.disconnect(handler);
                }
                popover.unparent();
            });
        });
        self.popover.popup();
    }
}

fn menu_action_item(label: &str, action: &str, icon_name: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    item.set_icon(&gio::ThemedIcon::new(icon_name));
    item
}

fn append_menu_action(menu: &gio::Menu, label: &str, action: &str, icon_name: &str) {
    menu.append_item(&menu_action_item(&tr(label), action, icon_name));
}

pub(crate) fn context_menu_scroll_page(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_min_content_width(CONTEXT_MENU_PLAYLIST_MIN_WIDTH);
    scroller.set_propagate_natural_width(true);
    scroller.set_propagate_natural_height(false);
    scroller.set_max_content_height(CONTEXT_MENU_PLAYLIST_MAX_HEIGHT);
    scroller.set_vexpand(true);
    scroller.set_child(Some(child));
    scroller
}
pub(crate) fn close_context_surface(widget: &impl IsA<gtk::Widget>) {
    if let Some(popover) = widget
        .as_ref()
        .ancestor(gtk::Popover::static_type())
        .and_then(|widget| widget.downcast::<gtk::Popover>().ok())
    {
        popdown_popover(&popover);
        return;
    }
    if let Some(dialog) = widget
        .as_ref()
        .ancestor(adw::Dialog::static_type())
        .and_then(|widget| widget.downcast::<adw::Dialog>().ok())
    {
        dialog.close();
    }
}
pub(crate) fn radio_context_submenu(group: &str) -> gio::Menu {
    let menu = gio::Menu::new();
    append_menu_action(&menu, "Play", &format!("{group}.play-radio"), PLAY_ICON);
    append_menu_action(
        &menu,
        "Play Next",
        &format!("{group}.play-radio-next"),
        PLAY_NEXT_ICON,
    );
    append_menu_action(
        &menu,
        "Play Later",
        &format!("{group}.play-radio-last"),
        PLAY_LATER_ICON,
    );
    menu
}

pub(crate) fn show_native_menu_icons(popover: &gtk::PopoverMenu) {
    show_native_menu_icons_from(popover.upcast_ref());
}

pub(crate) fn replace_native_menu_checkmarks(popover: &gtk::PopoverMenu) {
    replace_native_menu_checkmarks_from(popover.upcast_ref());
}

pub(crate) fn keep_parent_grab_for_nested_native_menus(popover: &gtk::PopoverMenu) {
    keep_parent_grab_for_nested_native_menus_from(popover.upcast_ref());
}

pub(crate) fn popdown_native_menu(popover: &gtk::PopoverMenu) {
    popdown_nested_native_menus_from(popover.upcast_ref());
    popover.popdown();
}

pub(crate) fn popdown_on_anchor_unmap(
    anchor: &impl IsA<gtk::Widget>,
    popover: &impl IsA<gtk::Popover>,
) -> glib::SignalHandlerId {
    let popover = popover.as_ref().downgrade();
    anchor.as_ref().connect_unmap(move |_| {
        if let Some(popover) = popover.upgrade() {
            popdown_popover(&popover);
        }
    })
}

fn popdown_popover(popover: &gtk::Popover) {
    if let Ok(menu) = popover.clone().downcast::<gtk::PopoverMenu>() {
        popdown_native_menu(&menu);
    } else {
        popover.popdown();
    }
}

fn popdown_nested_native_menus_from(container: &gtk::Widget) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();

        if widget.type_().name() == "GtkModelButton"
            && let Some(nested_popover) = widget
                .property::<Option<gtk::Popover>>("popover")
                .and_then(|popover| popover.downcast::<gtk::PopoverMenu>().ok())
        {
            popdown_native_menu(&nested_popover);
        }
        popdown_nested_native_menus_from(&widget);
    }
}

fn keep_parent_grab_for_nested_native_menus_from(container: &gtk::Widget) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();

        if widget.type_().name() == "GtkModelButton"
            && let Some(nested_popover) = widget
                .property::<Option<gtk::Popover>>("popover")
                .and_then(|popover| popover.downcast::<gtk::PopoverMenu>().ok())
        {
            // GTK can lose the parent's input grab when an autohide child closes.
            nested_popover.set_autohide(false);
            let nested_popover = nested_popover.downgrade();
            widget.connect_unmap(move |_| {
                if let Some(nested_popover) = nested_popover.upgrade() {
                    popdown_native_menu(&nested_popover);
                }
            });
        }
        keep_parent_grab_for_nested_native_menus_from(&widget);
    }
}

fn replace_native_menu_checkmarks_from(container: &gtk::Widget) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();

        if widget.type_().name() == "GtkModelButton" {
            if generated_menu_button_has_starting_space(&widget) {
                hide_native_menu_indicator_box(&widget);
            }
            if generated_menu_button_is_selection(&widget) {
                style_native_menu_selection(&widget);
            }
            if let Some(nested_popover) = widget
                .property::<Option<gtk::Popover>>("popover")
                .and_then(|popover| popover.downcast::<gtk::PopoverMenu>().ok())
            {
                replace_native_menu_checkmarks(&nested_popover);
            }
        }
        replace_native_menu_checkmarks_from(&widget);
    }
}

fn generated_menu_button_has_starting_space(widget: &gtk::Widget) -> bool {
    let role = widget.property_value("role");
    glib::EnumValue::from_value(&role).is_some_and(|(_, role)| role.nick() != "title")
}

fn generated_menu_button_is_selection(widget: &gtk::Widget) -> bool {
    if widget.type_().name() != "GtkModelButton" {
        return false;
    }

    let role = widget.property_value("role");
    glib::EnumValue::from_value(&role)
        .is_some_and(|(_, role)| matches!(role.nick(), "check" | "radio"))
}

fn style_native_menu_selection(button: &gtk::Widget) {
    if button.has_css_class(NATIVE_MENU_SELECTION_CLASS) {
        return;
    }
    button.add_css_class(NATIVE_MENU_SELECTION_CLASS);

    update_native_menu_selection(button);
    button.connect_notify_local(Some("active"), |button, _| {
        update_native_menu_selection(button);
    });
}

fn hide_native_menu_indicator_box(button: &gtk::Widget) {
    let mut child = button.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if widget.type_().name() == "GtkBox" {
            widget.set_visible(false);
            break;
        }
    }
}

fn update_native_menu_selection(button: &gtk::Widget) {
    if button.property::<bool>("active") {
        button.add_css_class(NATIVE_MENU_SELECTED_CLASS);
    } else {
        button.remove_css_class(NATIVE_MENU_SELECTED_CLASS);
    }
}

fn show_native_menu_icons_from(container: &gtk::Widget) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();

        if widget.type_().name() == "GtkModelButton" {
            show_native_menu_button_icon(&widget);
            if let Some(nested_popover) = widget
                .property::<Option<gtk::Popover>>("popover")
                .and_then(|popover| popover.downcast::<gtk::PopoverMenu>().ok())
            {
                show_native_menu_icons(&nested_popover);
            }
        }
        show_native_menu_icons_from(&widget);
    }
}

fn show_native_menu_button_icon(button: &gtk::Widget) {
    let mut child = button.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();

        if let Ok(image) = widget.clone().downcast::<gtk::Image>() {
            image.set_visible(true);
            image.set_margin_end(8);
        } else if let Ok(label) = widget.downcast::<gtk::Label>() {
            label.set_hexpand(true);
        }
    }
}

fn context_popover(
    target: &gtk::Widget,
    position: Option<(f64, f64)>,
    menu: &gio::Menu,
) -> gtk::PopoverMenu {
    let popover = gtk::PopoverMenu::from_model_full(menu, gtk::PopoverMenuFlags::NESTED);
    popover.set_autohide(true);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }
    popover
}

pub(crate) fn install_context_menu_openers(target: &impl IsA<gtk::Widget>, open: ContextMenuOpen) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_open = Rc::clone(&open);
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(move |click, _, x, y| {
        click.set_state(gtk::EventSequenceState::Claimed);
        if let Some(target) = target_weak.upgrade() {
            click_open(&target, Some((x, y)));
        }
    });
    target.add_controller(click);

    let long_open = Rc::clone(&open);
    let target_weak = target.downgrade();
    let press = gtk::GestureLongPress::new();
    press.set_propagation_phase(gtk::PropagationPhase::Capture);
    press.connect_pressed(move |press, x, y| {
        press.set_state(gtk::EventSequenceState::Claimed);
        if let Some(target) = target_weak.upgrade() {
            long_open(&target, Some((x, y)));
        }
    });
    target.add_controller(press);

    let target_weak = target.downgrade();
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            open(&target, None);
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}

pub(crate) fn add_link_hover(target: &gtk::Widget, label: &gtk::Label, text: &str) {
    let escaped_text = glib::markup_escape_text(text);
    let enter_label = label.downgrade();
    let enter_markup = format!("<u>{escaped_text}</u>");
    let leave_label = label.downgrade();
    let leave_text = text.to_string();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        if let Some(label) = enter_label.upgrade() {
            label.add_css_class("hovered-link");
            label.set_markup(&enter_markup);
        }
    });
    motion.connect_leave(move |_| {
        if let Some(label) = leave_label.upgrade() {
            label.remove_css_class("hovered-link");
            label.set_text(&leave_text);
        }
    });
    target.add_controller(motion);
}

pub(crate) fn add_stateful_link_hover(
    target: &gtk::Widget,
    label: &gtk::Label,
    text: Rc<RefCell<String>>,
) {
    let enter_label = label.downgrade();
    let enter_text = Rc::clone(&text);
    let leave_label = label.downgrade();
    let leave_text = text;
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        if let Some(label) = enter_label.upgrade() {
            let escaped_text = glib::markup_escape_text(enter_text.borrow().as_str());
            label.add_css_class("hovered-link");
            label.set_markup(&format!("<u>{escaped_text}</u>"));
        }
    });
    motion.connect_leave(move |_| {
        if let Some(label) = leave_label.upgrade() {
            label.remove_css_class("hovered-link");
            label.set_text(leave_text.borrow().as_str());
        }
    });
    target.add_controller(motion);
}

pub(crate) fn add_dynamic_link_hover(target: &gtk::Widget, label: &gtk::Label) {
    let enter_label = label.downgrade();
    let leave_label = label.downgrade();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        if let Some(label) = enter_label.upgrade() {
            let text = label.text();
            let escaped_text = glib::markup_escape_text(text.as_str());
            label.add_css_class("hovered-link");
            label.set_markup(&format!("<u>{escaped_text}</u>"));
        }
    });
    motion.connect_leave(move |_| {
        if let Some(label) = leave_label.upgrade() {
            let text = label.text().to_string();
            label.remove_css_class("hovered-link");
            label.set_text(&text);
        }
    });
    target.add_controller(motion);
}

pub(crate) fn add_label_click(label: &gtk::Label, callback: impl Fn() + 'static) {
    add_widget_click(label.upcast_ref(), callback);
}

pub(crate) fn add_widget_click(target: &gtk::Widget, callback: impl Fn() + 'static) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |gesture, press_count, _, _| {
        if press_count == 1 {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            callback();
        }
    });
    target.add_controller(click);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_menu_actions_store_their_icon() {
        let item = menu_action_item("Play", "track.play", PLAY_ICON);
        let serialized = item
            .attribute_value("icon", None)
            .expect("menu item should have an icon");
        let icon = gio::Icon::deserialize(&serialized).expect("menu icon should deserialize");

        assert!(icon.equal(Some(&gio::ThemedIcon::new(PLAY_ICON))));
    }
}
