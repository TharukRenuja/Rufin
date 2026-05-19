use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{Route, SidebarRouteItem};

use super::{Shell, icon_button, layout::COMPACT_RAIL_WIDTH};
use crate::i18n::tr;

const COMPACT_RAIL_ICON_SIZE: i32 = 22;
const COMPACT_RAIL_LABEL_WIDTH: i32 = COMPACT_RAIL_WIDTH - 8;
const COMPACT_RAIL_LABEL_WIDTH_CHARS: i32 = 8;

pub(super) fn sidebar_history_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = icon_button(icon_name, label);
    button.add_css_class("sidebar-history-button");
    button.set_valign(gtk::Align::Center);
    button
}

pub(super) fn build_normal_navigation(shell: &Rc<Shell>) {
    shell.normal_nav.append(&normal_history_controls(shell));

    let heading = gtk::Label::new(Some(&tr("My Library")));
    heading.add_css_class("nav-heading");
    heading.set_xalign(0.0);
    heading.set_margin_start(18);
    heading.set_margin_top(8);
    shell.normal_nav.append(&heading);

    for item in nav_items(shell) {
        shell.normal_nav.append(&nav_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
            false,
        ));
    }

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    shell.normal_nav.append(&spacer);

    if shell.state.settings.borrow().sidebar.server_visible {
        shell
            .normal_nav
            .append(&shell.server_selector.normal_button);
    }
}

pub(super) fn build_compact_navigation(shell: &Rc<Shell>) {
    shell.compact_nav.append(&compact_history_controls(shell));

    for item in nav_items(shell) {
        shell.compact_nav.append(&rail_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
        ));
    }
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    shell.compact_nav.append(&spacer);
    if shell.state.settings.borrow().sidebar.server_visible {
        shell
            .compact_nav
            .append(&shell.server_selector.compact_button);
    }
}

pub(super) fn rebuild_navigation(shell: &Rc<Shell>) {
    clear_box(&shell.normal_nav);
    clear_box(&shell.compact_nav);
    build_normal_navigation(shell);
    build_compact_navigation(shell);
    shell.update_server_selector();
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn normal_history_controls(shell: &Rc<Shell>) -> gtk::Box {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.add_css_class("sidebar-history-controls");
    controls.append(&shell.normal_back_button);
    controls.append(&shell.normal_forward_button);
    controls
}

fn compact_history_controls(shell: &Rc<Shell>) -> gtk::Box {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    controls.add_css_class("rail-history-controls");
    controls.set_halign(gtk::Align::Center);
    controls.append(&shell.compact_back_button);
    controls.append(&shell.compact_forward_button);
    controls
}

#[derive(Clone)]
struct NavItem {
    icon_name: &'static str,
    label: &'static str,
    route: Route,
}

fn nav_items(shell: &Shell) -> Vec<NavItem> {
    shell
        .state
        .settings
        .borrow()
        .sidebar
        .route_items
        .iter()
        .filter(|entry| entry.visible)
        .map(|entry| nav_item(entry.item))
        .collect()
}

fn nav_item(item: SidebarRouteItem) -> NavItem {
    match item {
        SidebarRouteItem::Home => NavItem {
            icon_name: "go-home-symbolic",
            label: "Home",
            route: Route::Home,
        },
        SidebarRouteItem::Favorites => NavItem {
            icon_name: "starred-symbolic",
            label: "Favorites",
            route: Route::Favorites,
        },
        SidebarRouteItem::Albums => NavItem {
            icon_name: "media-optical-symbolic",
            label: "Albums",
            route: Route::Albums,
        },
        SidebarRouteItem::Tracks => NavItem {
            icon_name: "audio-x-generic-symbolic",
            label: "Tracks",
            route: Route::Tracks,
        },
        SidebarRouteItem::Artists => NavItem {
            icon_name: "system-users-symbolic",
            label: "Artists",
            route: Route::Artists,
        },
        SidebarRouteItem::AlbumArtists => NavItem {
            icon_name: "avatar-default-symbolic",
            label: "Album Artists",
            route: Route::AlbumArtists,
        },
        SidebarRouteItem::Genres => NavItem {
            icon_name: "flag-symbolic",
            label: "Genres",
            route: Route::Genres,
        },
        SidebarRouteItem::Folders => NavItem {
            icon_name: "folder-symbolic",
            label: "Folders",
            route: Route::Folders { path: Vec::new() },
        },
        SidebarRouteItem::Playlists => NavItem {
            icon_name: "media-playlist-consecutive-symbolic",
            label: "Playlists",
            route: Route::Playlists,
        },
    }
}

fn nav_button(
    shell: &Rc<Shell>,
    icon_name: &str,
    label: &str,
    route: Route,
    compact: bool,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("nav-button");
    button.add_css_class("flat");
    if compact {
        button.add_css_class("rail-button");
    }
    let accessible_label = tr(label);
    button.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);

    let content = gtk::Box::new(
        if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        },
        8,
    );
    content.set_halign(if compact {
        gtk::Align::Center
    } else {
        gtk::Align::Start
    });
    let icon = gtk::Image::from_icon_name(icon_name);
    content.append(&icon);
    if compact {
        icon.set_pixel_size(COMPACT_RAIL_ICON_SIZE);
        let text = gtk::Label::new(Some(&compact_sidebar_label_text(label)));
        configure_rail_label(&text);
        content.append(&text);
    } else {
        let text = gtk::Label::new(Some(&tr(label)));
        text.set_xalign(0.0);
        text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&text);
    }
    button.set_child(Some(&content));

    let shell = Rc::clone(shell);
    button.connect_clicked(move |_| shell.navigate(route.clone()));
    button
}

fn compact_sidebar_label_text(label: &str) -> String {
    let translated = tr(label);
    let compact = {
        let words = translated.split_whitespace().collect::<Vec<_>>();
        if words.len() == 2 {
            Some(format!("{}\n{}", words[0], words[1]))
        } else {
            None
        }
    };
    compact.unwrap_or(translated)
}

fn configure_rail_label(label: &gtk::Label) {
    label.add_css_class("rail-label");
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_lines(2);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_request(COMPACT_RAIL_LABEL_WIDTH);
    label.set_size_request(COMPACT_RAIL_LABEL_WIDTH, -1);
    label.set_width_chars(1);
    label.set_max_width_chars(COMPACT_RAIL_LABEL_WIDTH_CHARS);
}

fn rail_button(shell: &Rc<Shell>, icon_name: &str, label: &str, route: Route) -> gtk::Button {
    nav_button(shell, icon_name, label, route, true)
}
