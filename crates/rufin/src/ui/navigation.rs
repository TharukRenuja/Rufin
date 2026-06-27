use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use domain::{Route, SidebarRouteItem};
use gtk::glib;

use super::{
    Shell, chrome,
    layout::{COMPACT_RAIL_WIDTH, ResolvedLeftSidebarMode},
};
use crate::i18n::tr;

const NORMAL_NAV_ICON_SIZE: i32 = 16;
const COMPACT_NAV_ICON_SIZE: i32 = 20;
#[cfg(test)]
const ROUTE_ICON_PREFIX: &str = "route-";
const COMPACT_RAIL_LABEL_WIDTH: i32 = COMPACT_RAIL_WIDTH - 8;
const COMPACT_RAIL_LABEL_WIDTH_CHARS: i32 = 8;
const PRIMARY_MENU_POPOVER_WIDTH: i32 = 236;
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
const NAV_ROUTE_ICONS: [(&str, &str, &str); 10] = [
    (
        NAV_ROUTE_HOME_CLASS,
        "route-home-symbolic",
        "route-home-selected-symbolic",
    ),
    (
        NAV_ROUTE_FAVORITES_CLASS,
        "route-favorites-symbolic",
        "route-favorites-selected-symbolic",
    ),
    (
        NAV_ROUTE_ALBUMS_CLASS,
        "route-albums-symbolic",
        "route-albums-selected-symbolic",
    ),
    (
        NAV_ROUTE_TRACKS_CLASS,
        "route-tracks-symbolic",
        "route-tracks-selected-symbolic",
    ),
    (
        NAV_ROUTE_ARTISTS_CLASS,
        "route-artists-symbolic",
        "route-artists-selected-symbolic",
    ),
    (
        NAV_ROUTE_ALBUM_ARTISTS_CLASS,
        "route-album-artists-symbolic",
        "route-album-artists-selected-symbolic",
    ),
    (
        NAV_ROUTE_GENRES_CLASS,
        "route-genres-symbolic",
        "route-genres-selected-symbolic",
    ),
    (
        NAV_ROUTE_FOLDERS_CLASS,
        "route-folders-symbolic",
        "route-folders-selected-symbolic",
    ),
    (
        NAV_ROUTE_PLAYLISTS_CLASS,
        "route-playlists-symbolic",
        "route-playlists-selected-symbolic",
    ),
    (
        NAV_ROUTE_SMART_PLAYLISTS_CLASS,
        "route-smart-playlists-symbolic",
        "route-smart-playlists-selected-symbolic",
    ),
];

pub(super) fn build_normal_navigation(shell: &Rc<Shell>) {
    shell.normal_nav.append(&primary_menu_button(
        &shell.normal_main_menu,
        &shell.server_selector.normal_button,
        &shell.normal_main_menu_popover,
        &shell.normal_main_menu_click_handler,
        shell,
        false,
    ));
    for item in nav_items(shell) {
        shell.normal_nav.append(&nav_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
            false,
        ));
    }

    shell.normal_nav.append(&sidebar_spacer());
}

pub(super) fn build_compact_navigation(shell: &Rc<Shell>) {
    shell.compact_nav.append(&primary_menu_button(
        &shell.compact_main_menu,
        &shell.server_selector.compact_button,
        &shell.compact_main_menu_popover,
        &shell.compact_main_menu_click_handler,
        shell,
        true,
    ));
    for item in nav_items(shell) {
        shell.compact_nav.append(&rail_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
        ));
    }
    shell.compact_nav.append(&sidebar_spacer());
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

pub(super) fn relocalize_primary_menu_button(
    button: &gtk::Button,
    source_button: &gtk::Button,
    popover_slot: &RefCell<Option<gtk::Popover>>,
    handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    shell: &Rc<Shell>,
    compact: bool,
) {
    chrome::configure_primary_menu_button(button);
    button.set_child(Some(&sidebar_menu_content(compact)));
    update_primary_menu_popover(
        button,
        popover_slot,
        handler_slot,
        primary_menu_popover(source_button, shell),
    );
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn sidebar_spacer() -> gtk::Box {
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    spacer
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
        if let (Some((normal_icon_name, selected_icon_name)), Some(icon)) =
            (nav_route_icon_names(&widget), nav_button_icon(&widget))
        {
            icon.set_icon_name(Some(if selected {
                selected_icon_name
            } else {
                normal_icon_name
            }));
        }
    }
}

fn nav_button_icon(widget: &gtk::Widget) -> Option<gtk::Image> {
    widget
        .first_child()
        .and_then(|child| child.downcast::<gtk::Box>().ok())
        .and_then(|content| content.first_child())
        .and_then(|child| child.downcast::<gtk::Image>().ok())
}

fn nav_route_icon_names(widget: &gtk::Widget) -> Option<(&'static str, &'static str)> {
    NAV_ROUTE_ICONS
        .into_iter()
        .find_map(|(route_class, normal_icon_name, selected_icon_name)| {
            widget
                .has_css_class(route_class)
                .then_some((normal_icon_name, selected_icon_name))
        })
}

fn primary_menu_button(
    button: &gtk::Button,
    source_button: &gtk::Button,
    popover_slot: &RefCell<Option<gtk::Popover>>,
    handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    shell: &Rc<Shell>,
    compact: bool,
) -> gtk::Button {
    button.add_css_class("nav-button");
    button.add_css_class("primary-menu-button");
    button.add_css_class("flat");
    if compact {
        button.add_css_class("rail-button");
    }
    relocalize_primary_menu_button(
        button,
        source_button,
        popover_slot,
        handler_slot,
        shell,
        compact,
    );
    button.clone()
}

fn update_primary_menu_popover(
    button: &gtk::Button,
    popover_slot: &RefCell<Option<gtk::Popover>>,
    handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    popover: gtk::Popover,
) {
    if let Some(handler) = handler_slot.borrow_mut().take() {
        button.disconnect(handler);
    }
    if let Some(current) = popover_slot.borrow_mut().replace(popover.clone()) {
        if current.is_visible() {
            current.popdown();
        }
        current.unparent();
    }
    popover.set_parent(button);
    let row_popover = popover.clone();
    let handler = button.connect_clicked(move |_| row_popover.popup());
    *handler_slot.borrow_mut() = Some(handler);
}

pub(super) fn popup_primary_menu(popover_slot: &RefCell<Option<gtk::Popover>>) {
    if let Some(popover) = popover_slot.borrow().as_ref() {
        popover.popup();
    }
}

fn primary_menu_popover(source_button: &gtk::Button, shell: &Rc<Shell>) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Right);

    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 1);
    wrapper.add_css_class("primary-menu-popover");
    wrapper.set_width_request(PRIMARY_MENU_POPOVER_WIDTH);
    detach_from_parent(source_button);
    wrapper.append(source_button);
    wrapper.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    wrapper.append(&primary_menu_action_row(
        &popover,
        &tr("Preferences"),
        "win.preferences",
        "preferences-system-symbolic",
    ));
    let (private_mode_row, private_mode_label, _) = primary_menu_labeled_action_row(
        &popover,
        &primary_menu_private_mode_label(shell.as_ref()),
        "win.toggle-private-mode",
        "system-lock-screen-symbolic",
    );
    let private_mode_shell = Rc::clone(shell);
    let private_mode_label_for_notify = private_mode_label.clone();
    popover.connect_visible_notify(move |popover| {
        if popover.is_visible() {
            private_mode_label_for_notify.set_text(&primary_menu_private_mode_label(
                private_mode_shell.as_ref(),
            ));
        }
    });
    wrapper.append(&private_mode_row);
    wrapper.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    wrapper.append(&primary_menu_action_row(
        &popover,
        &tr("Keyboard Shortcuts"),
        "win.show-shortcuts",
        "preferences-desktop-keyboard-shortcuts-symbolic",
    ));
    wrapper.append(&primary_menu_action_row(
        &popover,
        &tr("Toggle Fullscreen"),
        "win.toggle-fullscreen",
        "view-fullscreen-symbolic",
    ));
    let (sidebar_toggle_row, sidebar_toggle_label, sidebar_toggle_icon) =
        primary_menu_labeled_action_row(
            &popover,
            &primary_menu_sidebar_toggle_label(shell.as_ref()),
            "win.toggle-left-sidebar",
            primary_menu_sidebar_toggle_icon(shell.as_ref()),
        );
    let label_shell = Rc::clone(shell);
    let sidebar_toggle_label_for_notify = sidebar_toggle_label.clone();
    let sidebar_toggle_icon_for_notify = sidebar_toggle_icon.clone();
    popover.connect_visible_notify(move |popover| {
        if popover.is_visible() {
            sidebar_toggle_label_for_notify
                .set_text(&primary_menu_sidebar_toggle_label(label_shell.as_ref()));
            sidebar_toggle_icon_for_notify
                .set_icon_name(Some(primary_menu_sidebar_toggle_icon(label_shell.as_ref())));
        }
    });
    wrapper.append(&sidebar_toggle_row);
    wrapper.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    wrapper.append(&primary_menu_action_row(
        &popover,
        &tr("Version History"),
        "win.show-release-notes",
        "view-list-symbolic",
    ));
    wrapper.append(&primary_menu_action_row(
        &popover,
        &tr("About Rufin"),
        "win.about",
        "help-about-symbolic",
    ));
    popover.set_child(Some(&wrapper));
    popover
}

fn detach_from_parent(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref();
    if let Some(parent) = widget.parent() {
        if let Ok(parent_box) = parent.downcast::<gtk::Box>() {
            parent_box.remove(widget);
        } else {
            widget.unparent();
        }
    }
}

fn primary_menu_action_row(
    popover: &gtk::Popover,
    label: &str,
    action_name: &'static str,
    icon_name: &str,
) -> gtk::Button {
    primary_menu_labeled_action_row(popover, label, action_name, icon_name).0
}

fn primary_menu_labeled_action_row(
    popover: &gtk::Popover,
    label: &str,
    action_name: &'static str,
    icon_name: &str,
) -> (gtk::Button, gtk::Label, gtk::Image) {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("primary-menu-row");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    content.set_halign(gtk::Align::Fill);
    let icon = gtk::Image::from_icon_name(icon_name);
    content.append(&icon);
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    content.append(&text);
    row.set_child(Some(&content));

    let row_popover = popover.clone();
    row.connect_clicked(move |button| {
        row_popover.popdown();
        let _ = button.activate_action(action_name, None);
    });
    (row, text, icon)
}

fn primary_menu_sidebar_toggle_label(shell: &Shell) -> String {
    if shell.state.resolved_left_sidebar.get() == ResolvedLeftSidebarMode::Full {
        tr("Collapse sidebar")
    } else {
        tr("Expand sidebar")
    }
}

fn primary_menu_sidebar_toggle_icon(shell: &Shell) -> &'static str {
    if shell.state.resolved_left_sidebar.get() == ResolvedLeftSidebarMode::Full {
        "sidebar-hide-symbolic"
    } else {
        "sidebar-show-symbolic"
    }
}

fn primary_menu_private_mode_label(shell: &Shell) -> String {
    if shell.state.settings.borrow().private_mode {
        tr("Turn off private mode")
    } else {
        tr("Turn on private mode")
    }
}

fn sidebar_menu_content(compact: bool) -> gtk::Box {
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

    let icon_size = if compact {
        COMPACT_NAV_ICON_SIZE
    } else {
        NORMAL_NAV_ICON_SIZE
    };
    let icon = gtk::Image::from_icon_name("open-menu-symbolic");
    icon.set_pixel_size(icon_size);
    icon.set_size_request(icon_size, icon_size);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    content.append(&icon);

    if compact {
        let text = gtk::Label::new(Some(&compact_sidebar_label_text("Menu")));
        configure_rail_label(&text);
        content.append(&text);
    } else {
        let text = gtk::Label::new(Some(&tr("Menu")));
        configure_sidebar_entry_label(&text);
        content.append(&text);
    }

    content
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
            icon_name: "route-home-symbolic",
            label: "Home",
            route: Route::Home,
        },
        SidebarRouteItem::Favorites => NavItem {
            icon_name: "route-favorites-symbolic",
            label: "Favorites",
            route: Route::Favorites,
        },
        SidebarRouteItem::Albums => NavItem {
            icon_name: "route-albums-symbolic",
            label: "Albums",
            route: Route::Albums,
        },
        SidebarRouteItem::Tracks => NavItem {
            icon_name: "route-tracks-symbolic",
            label: "Tracks",
            route: Route::Tracks,
        },
        SidebarRouteItem::Artists => NavItem {
            icon_name: "route-artists-symbolic",
            label: "Artists",
            route: Route::Artists,
        },
        SidebarRouteItem::AlbumArtists => NavItem {
            icon_name: "route-album-artists-symbolic",
            label: "Album Artists",
            route: Route::AlbumArtists,
        },
        SidebarRouteItem::Genres => NavItem {
            icon_name: "route-genres-symbolic",
            label: "Genres",
            route: Route::Genres,
        },
        SidebarRouteItem::Folders => NavItem {
            icon_name: "route-folders-symbolic",
            label: "Folders",
            route: Route::Folders { path: Vec::new() },
        },
        SidebarRouteItem::Playlists => NavItem {
            icon_name: "route-playlists-symbolic",
            label: "Playlists",
            route: Route::Playlists,
        },
        SidebarRouteItem::SmartPlaylists => NavItem {
            icon_name: "route-smart-playlists-symbolic",
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
    let icon_size = if compact {
        COMPACT_NAV_ICON_SIZE
    } else {
        NORMAL_NAV_ICON_SIZE
    };
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("nav-icon");
    icon.set_pixel_size(icon_size);
    icon.set_size_request(icon_size, icon_size);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    content.append(&icon);
    if compact {
        let text = gtk::Label::new(Some(&compact_sidebar_label_text(label)));
        configure_rail_label(&text);
        content.append(&text);
    } else {
        let text = gtk::Label::new(Some(&tr(label)));
        configure_sidebar_entry_label(&text);
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
    configure_sidebar_entry_label(label);
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

fn configure_sidebar_entry_label(label: &gtk::Label) {
    label.add_css_class("sidebar-entry-label");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
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
    use domain::{PlaylistId, SmartPlaylistId};
    use std::path::PathBuf;

    #[test]
    fn navigation_playlist_classes() {
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

    #[test]
    fn navigation_uses_bundled_assets() {
        for item in SidebarRouteItem::all() {
            let nav = nav_item(item);
            assert!(
                nav.icon_name.starts_with(ROUTE_ICON_PREFIX),
                "{} should use an app-bundled sidebar icon",
                nav.icon_name
            );

            let path = bundled_sidebar_icon_path(nav.icon_name);
            assert!(
                path.is_file(),
                "{} should be bundled at {}",
                nav.icon_name,
                path.display()
            );
        }
        for (_, _, selected_icon_name) in NAV_ROUTE_ICONS {
            let selected_path = bundled_sidebar_icon_path(selected_icon_name);
            assert!(
                selected_path.is_file(),
                "{} should be bundled at {}",
                selected_icon_name,
                selected_path.display()
            );
        }
    }

    fn bundled_sidebar_icon_path(icon_name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/icons/hicolor/scalable/actions")
            .join(format!("{icon_name}.svg"))
    }
}
