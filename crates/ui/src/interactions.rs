use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use localization::tr;

use crate::shell::Shell;

const CONTEXT_MENU_PLAYLIST_MAX_HEIGHT: i32 = 320;
const CONTEXT_MENU_PLAYLIST_MIN_WIDTH: i32 = 380;

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

    pub(crate) fn append_action(&self, label: &str, action: &str) {
        self.menu.append(
            Some(&tr(label)),
            Some(&format!("{}.{}", self.group_name, action)),
        );
    }

    pub(crate) fn append_submenu(&self, label: &str, submenu: &gio::Menu) {
        self.menu.append_submenu(Some(&tr(label)), submenu);
    }

    pub(crate) fn add_action(&self, name: &str, run: impl Fn() + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let popover = self.popover.downgrade();
        action.connect_activate(move |_, _| {
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
                popover.unparent();
            });
        });
        self.popover.popup();
    }
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
pub(crate) fn radio_context_submenu(group: &str) -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some(&tr("Play")), Some(&format!("{group}.play-radio")));
    menu.append(
        Some(&tr("Play Next")),
        Some(&format!("{group}.play-radio-next")),
    );
    menu.append(
        Some(&tr("Play Later")),
        Some(&format!("{group}.play-radio-last")),
    );
    menu
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
