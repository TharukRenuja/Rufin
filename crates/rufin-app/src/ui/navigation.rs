use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{Route, SidebarRouteItem};

use super::{Shell, icon_button, layout::COMPACT_RAIL_WIDTH};
use crate::i18n::tr;

const COMPACT_RAIL_ICON_SIZE: i32 = 22;
const COMPACT_RAIL_LABEL_WIDTH: i32 = COMPACT_RAIL_WIDTH - 8;
const COMPACT_RAIL_LABEL_WIDTH_CHARS: i32 = 8;
const NAV_SELECTED_CLASS: &str = "selected";
const NAV_ROUTE_HOME_CLASS: &str = "nav-route-home";
const NAV_ROUTE_FAVORITES_CLASS: &str = "nav-route-favorites";
const NAV_ROUTE_ALBUMS_CLASS: &str = "nav-route-albums";
const NAV_ROUTE_TRACKS_CLASS: &str = "nav-route-tracks";
const NAV_ROUTE_ARTISTS_CLASS: &str = "nav-route-artists";
const NAV_ROUTE_ALBUM_ARTISTS_CLASS: &str = "nav-route-album-artists";
const NAV_ROUTE_GENRES_CLASS: &str = "nav-route-genres";
const NAV_ROUTE_FOLDERS_CLASS: &str = "nav-route-folders";
const NAV_ROUTE_PLAYLISTS_CLASS: &str = "nav-route-playlists";
const NAV_ROUTE_SMART_PLAYLISTS_CLASS: &str = "nav-route-smart-playlists";

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
    update_navigation_selection(shell.as_ref());
}

pub(super) fn update_navigation_selection(shell: &Shell) {
    let active_route_class = nav_route_class(shell.state.routes.borrow().current());
    update_navigation_selection_in(&shell.normal_nav, active_route_class);
    update_navigation_selection_in(&shell.compact_nav, active_route_class);
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn update_navigation_selection_in(container: &gtk::Box, active_route_class: Option<&str>) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();

        if !widget.has_css_class("nav-button") || widget.has_css_class("server-selector") {
            continue;
        }

        let selected = active_route_class
            .map(|route_class| widget.has_css_class(route_class))
            .unwrap_or(false);
        if selected {
            widget.add_css_class(NAV_SELECTED_CLASS);
        } else {
            widget.remove_css_class(NAV_SELECTED_CLASS);
        }
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
            icon_name: "avatar-default-symbolic",
            label: "Artists",
            route: Route::Artists,
        },
        SidebarRouteItem::AlbumArtists => NavItem {
            icon_name: "system-users-symbolic",
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
            icon_name: "view-list-symbolic",
            label: "Playlists",
            route: Route::Playlists,
        },
        SidebarRouteItem::SmartPlaylists => NavItem {
            icon_name: "view-filter-symbolic",
            label: "Smart Playlists",
            route: Route::SmartPlaylists,
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
    if let Some(route_class) = nav_route_class(&route) {
        button.add_css_class(route_class);
    }
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

fn nav_route_class(route: &Route) -> Option<&'static str> {
    match route {
        Route::Home => Some(NAV_ROUTE_HOME_CLASS),
        Route::Favorites => Some(NAV_ROUTE_FAVORITES_CLASS),
        Route::Albums | Route::AlbumDetail(_) => Some(NAV_ROUTE_ALBUMS_CLASS),
        Route::Tracks => Some(NAV_ROUTE_TRACKS_CLASS),
        Route::Artists
        | Route::ArtistDetail(_)
        | Route::ArtistDiscography(_)
        | Route::ArtistTracks(_) => Some(NAV_ROUTE_ARTISTS_CLASS),
        Route::AlbumArtists => Some(NAV_ROUTE_ALBUM_ARTISTS_CLASS),
        Route::Genres | Route::GenreDetail(_) => Some(NAV_ROUTE_GENRES_CLASS),
        Route::Folders { .. } => Some(NAV_ROUTE_FOLDERS_CLASS),
        Route::Playlists | Route::PlaylistDetail(_) => Some(NAV_ROUTE_PLAYLISTS_CLASS),
        Route::SmartPlaylists | Route::SmartPlaylistDetail(_) => {
            Some(NAV_ROUTE_SMART_PLAYLISTS_CLASS)
        }
        Route::Search { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rufin_core::{PlaylistId, SmartPlaylistId};

    #[test]
    fn playlist_and_smart_playlist_routes_have_distinct_nav_classes() {
        assert_eq!(
            nav_route_class(&Route::Playlists),
            Some(NAV_ROUTE_PLAYLISTS_CLASS)
        );
        assert_eq!(
            nav_route_class(&Route::PlaylistDetail(PlaylistId::new("playlist"))),
            Some(NAV_ROUTE_PLAYLISTS_CLASS)
        );
        assert_eq!(
            nav_route_class(&Route::SmartPlaylists),
            Some(NAV_ROUTE_SMART_PLAYLISTS_CLASS)
        );
        assert_eq!(
            nav_route_class(&Route::SmartPlaylistDetail(SmartPlaylistId::new("smart"))),
            Some(NAV_ROUTE_SMART_PLAYLISTS_CLASS)
        );
        assert_ne!(
            nav_route_class(&Route::Playlists),
            nav_route_class(&Route::SmartPlaylists)
        );
    }
}
