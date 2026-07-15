use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::favorites::{FAVORITE_ADD_ICON, FAVORITE_REMOVE_ICON};
use localization::tr;

use crate::shell::actions::{PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON, REMOVE_ICON};

const CONTEXT_MENU_PLAYLIST_MAX_HEIGHT: i32 = 320;
const CONTEXT_MENU_PLAYLIST_MIN_WIDTH: i32 = 380;
const CONTEXT_SUBMENU_CLOSE_DELAY_MS: u64 = 120;

pub(crate) const ADD_TO_PLAYLIST_ICON: &str = "rufin-route-playlists-symbolic";
pub(crate) const ALBUM_ICON: &str = "rufin-route-albums-symbolic";
pub(crate) const ARTIST_ICON: &str = "rufin-route-artists-symbolic";
pub(crate) const RADIO_ICON: &str = "rufin-audio-radio-symbolic";

type ContextMenuOpen = Rc<dyn Fn(&gtk::Widget, Option<(f64, f64)>)>;

thread_local! {
    static OPEN_CONTEXT_SUBMENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

pub(crate) struct ContextMenuSurface {
    target: gtk::Widget,
    group_name: &'static str,
    popover: gtk::Popover,
    actions: gio::SimpleActionGroup,
}

impl ContextMenuSurface {
    pub(crate) fn new(
        target: &gtk::Widget,
        group_name: &'static str,
        css_class: &str,
        position: Option<(f64, f64)>,
        child: &impl IsA<gtk::Widget>,
    ) -> Self {
        Self {
            target: target.clone(),
            group_name,
            popover: context_popover(target, css_class, position, child),
            actions: gio::SimpleActionGroup::new(),
        }
    }

    pub(crate) fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    pub(crate) fn add_action(&self, name: &str, run: impl Fn() + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let popover = self.popover.downgrade();
        action.connect_activate(move |_, _| {
            popdown_current_context_submenu();
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            run();
        });
        self.actions.add_action(&action);
    }

    pub(crate) fn popup(self) {
        self.target
            .insert_action_group(self.group_name, Some(&self.actions));
        self.popover.connect_closed(move |popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || {
                popdown_current_context_submenu();
                popover.unparent();
            });
        });
        self.popover.popup();
    }
}

pub(crate) fn context_menu_box() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 0)
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
        popover.popdown();
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
pub(crate) fn context_menu_action(label: &str, action: &str, icon_name: &str) -> gtk::Button {
    context_menu_action_with_label(&tr(label), action, icon_name)
}
fn context_menu_action_with_label(label: &str, action: &str, icon_name: &str) -> gtk::Button {
    let button = context_menu_button(label, icon_name);
    button.set_action_name(Some(action));
    button
}
pub(crate) fn context_menu_submenu_action(
    label: &str,
    action: &str,
    icon_name: &str,
    submenu: &impl IsA<gtk::Widget>,
) -> gtk::Button {
    let button = context_menu_disclosure_button(&tr(label), icon_name);
    button.set_action_name(Some(action));

    let popover = gtk::Popover::new();
    popover.add_css_class("context-submenu");
    popover.set_autohide(false);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Right);
    popover.set_child(Some(submenu));
    popover.set_parent(&button);

    let button_hovered = Rc::new(Cell::new(false));
    let submenu_hovered = Rc::new(Cell::new(false));

    let motion = gtk::EventControllerMotion::new();
    let popover_for_enter = popover.clone();
    let button_hovered_for_enter = Rc::clone(&button_hovered);
    motion.connect_enter(move |_, _, _| {
        button_hovered_for_enter.set(true);
        popup_context_submenu(&popover_for_enter);
    });
    let popover_for_leave = popover.clone();
    let button_hovered_for_leave = Rc::clone(&button_hovered);
    let submenu_hovered_for_leave = Rc::clone(&submenu_hovered);
    motion.connect_leave(move |_| {
        button_hovered_for_leave.set(false);
        schedule_context_submenu_popdown(
            &popover_for_leave,
            Rc::clone(&button_hovered_for_leave),
            Rc::clone(&submenu_hovered_for_leave),
        );
    });
    button.add_controller(motion);

    let submenu_motion = gtk::EventControllerMotion::new();
    let submenu_hovered_for_enter = Rc::clone(&submenu_hovered);
    submenu_motion.connect_enter(move |_, _, _| {
        submenu_hovered_for_enter.set(true);
    });
    let popover_for_leave = popover.downgrade();
    let button_hovered_for_leave = Rc::clone(&button_hovered);
    let submenu_hovered_for_leave = Rc::clone(&submenu_hovered);
    submenu_motion.connect_leave(move |_| {
        submenu_hovered_for_leave.set(false);
        if let Some(popover) = popover_for_leave.upgrade() {
            schedule_context_submenu_popdown(
                &popover,
                Rc::clone(&button_hovered_for_leave),
                Rc::clone(&submenu_hovered_for_leave),
            );
        }
    });
    popover.add_controller(submenu_motion);

    button.connect_unrealize(move |_| {
        forget_context_submenu(&popover);
        popover.unparent();
    });
    button
}
pub(crate) fn radio_context_submenu(group: &str) -> gtk::Box {
    let menu = context_menu_box();
    menu.append(&context_menu_action(
        "Play",
        &format!("{group}.play-radio"),
        PLAY_ICON,
    ));
    menu.append(&context_menu_action(
        "Play Next",
        &format!("{group}.play-radio-next"),
        PLAY_NEXT_ICON,
    ));
    menu.append(&context_menu_action(
        "Play Later",
        &format!("{group}.play-radio-last"),
        PLAY_LATER_ICON,
    ));
    menu
}
fn context_popover(
    target: &gtk::Widget,
    css_class: &str,
    position: Option<(f64, f64)>,
    child: &impl IsA<gtk::Widget>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_autohide(true);
    popover.add_css_class(css_class);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_parent(target);
    popover.set_child(Some(child));
    if let Some((x, y)) = position {
        popover.add_css_class("context-menu-opening");
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }
    let motion = gtk::EventControllerMotion::new();
    let popover_for_motion = popover.downgrade();
    motion.connect_motion(move |_, _, _| {
        if let Some(popover) = popover_for_motion.upgrade() {
            popover.remove_css_class("context-menu-opening");
        }
    });
    popover.add_controller(motion);
    popover
}
pub(crate) fn context_menu_button(label: &str, icon_name: &str) -> gtk::Button {
    let row = context_menu_button_content(label, icon_name);
    let button = gtk::Button::builder()
        .child(&row)
        .tooltip_text(label)
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .build();
    button.add_css_class("flat");
    button.add_css_class("context-menu-button");
    button
}
fn context_menu_disclosure_button(label: &str, icon_name: &str) -> gtk::Button {
    let row = context_menu_button_content(label, icon_name);
    let arrow = gtk::Image::from_icon_name("go-next-symbolic");
    arrow.add_css_class("context-submenu-arrow");
    arrow.set_pixel_size(14);
    row.append(&arrow);

    let button = gtk::Button::builder()
        .child(&row)
        .tooltip_text(label)
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .build();
    button.add_css_class("flat");
    button.add_css_class("context-menu-button");
    button
}
fn context_menu_button_content(label: &str, icon_name: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::Fill);
    row.set_hexpand(true);

    let icon = context_menu_icon(icon_name);
    row.append(&icon);

    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&text);
    row
}
fn context_menu_icon(icon_name: &str) -> gtk::Widget {
    let icon = {
        let image = gtk::Image::from_icon_name(icon_name);
        let pixel_size = if icon_name == PLAY_ICON {
            12
        } else if icon_name == ADD_TO_PLAYLIST_ICON {
            16
        } else if matches!(
            icon_name,
            PLAY_NEXT_ICON
                | PLAY_LATER_ICON
                | ARTIST_ICON
                | ALBUM_ICON
                | RADIO_ICON
                | FAVORITE_ADD_ICON
                | FAVORITE_REMOVE_ICON
                | REMOVE_ICON
        ) {
            20
        } else {
            18
        };
        image.set_pixel_size(pixel_size);
        image.upcast::<gtk::Widget>()
    };
    icon.add_css_class("context-menu-icon");
    icon.set_size_request(20, 20);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon
}
fn popup_context_submenu(popover: &gtk::Popover) {
    OPEN_CONTEXT_SUBMENU.with(|current| {
        let previous = current.borrow().clone();
        if let Some(previous) = previous
            && previous != *popover
        {
            previous.popdown();
        }
        *current.borrow_mut() = Some(popover.clone());
    });
    popover.popup();
}
fn schedule_context_submenu_popdown(
    popover: &gtk::Popover,
    button_hovered: Rc<Cell<bool>>,
    submenu_hovered: Rc<Cell<bool>>,
) {
    let popover = popover.clone();
    glib::timeout_add_local_once(
        Duration::from_millis(CONTEXT_SUBMENU_CLOSE_DELAY_MS),
        move || {
            if !button_hovered.get() && !submenu_hovered.get() {
                forget_context_submenu(&popover);
                popover.popdown();
            }
        },
    );
}
fn popdown_current_context_submenu() {
    OPEN_CONTEXT_SUBMENU.with(|current| {
        if let Some(popover) = current.borrow_mut().take() {
            popover.popdown();
        }
    });
}
fn forget_context_submenu(popover: &gtk::Popover) {
    OPEN_CONTEXT_SUBMENU.with(|current| {
        let is_current = current
            .borrow()
            .as_ref()
            .is_some_and(|current| current == popover);
        if is_current {
            current.borrow_mut().take();
        }
    });
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
