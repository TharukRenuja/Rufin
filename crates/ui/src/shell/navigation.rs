use std::{cell::RefCell, rc::Rc};

use crate::SidebarRouteItem;
use crate::interactions::{
    popdown_on_anchor_unmap, replace_native_menu_checkmarks, show_native_menu_icons,
};
use crate::preferences::source::selector::source_submenu;
use crate::routes::route::Route;
use adw::prelude::*;
use gtk::{gio, glib};

use super::{
    Shell, chrome,
    layout::{COMPACT_RAIL_WIDTH, ResolvedLeftSidebarMode},
    route::RouteStack,
};
use localization::{msgid, tr};

const NORMAL_NAV_ICON_SIZE: i32 = 16;
const COMPACT_NAV_ICON_SIZE: i32 = 20;
const MOUSE_BACK_BUTTON: u32 = 8;
const MOUSE_FORWARD_BUTTON: u32 = 9;
#[cfg(test)]
const ROUTE_ICON_PREFIX: &str = "rufin-route-";
const COMPACT_RAIL_LABEL_WIDTH: i32 = COMPACT_RAIL_WIDTH - 8;
const COMPACT_RAIL_LABEL_WIDTH_CHARS: i32 = 8;
const NAV_SELECTED_CLASS: &str = "selected";
const NAV_ROUTE_HOME_CLASS: &str = "nav-route-home";
const NAV_ROUTE_SEARCH_CLASS: &str = "nav-route-search";
const NAV_ROUTE_FAVORITES_CLASS: &str = "nav-route-favorites";
const NAV_ROUTE_HISTORY_CLASS: &str = "nav-route-history";
const NAV_ROUTE_ALBUMS_CLASS: &str = "nav-route-albums";
const NAV_ROUTE_TRACKS_CLASS: &str = "nav-route-tracks";
const NAV_ROUTE_ARTISTS_CLASS: &str = "nav-route-artists";
const NAV_ROUTE_ALBUM_ARTISTS_CLASS: &str = "nav-route-album-artists";
const NAV_ROUTE_GENRES_CLASS: &str = "nav-route-genres";
const NAV_ROUTE_MOODS_CLASS: &str = "nav-route-moods";
const NAV_ROUTE_FOLDERS_CLASS: &str = "nav-route-folders";
const NAV_ROUTE_PLAYLISTS_CLASS: &str = "nav-route-playlists";
const NAV_ROUTE_SMART_PLAYLISTS_CLASS: &str = "nav-route-smart-playlists";
const PRIMARY_MENU_CLASS: &str = "rufin-primary-menu";
const NAV_ROUTE_ICONS: [(&str, &str, &str); 13] = [
    (
        NAV_ROUTE_HOME_CLASS,
        "rufin-route-home-symbolic",
        "rufin-route-home-selected-symbolic",
    ),
    (
        NAV_ROUTE_SEARCH_CLASS,
        "rufin-route-search-symbolic",
        "rufin-route-search-selected-symbolic",
    ),
    (
        NAV_ROUTE_FAVORITES_CLASS,
        "rufin-route-favorites-symbolic",
        "rufin-route-favorites-selected-symbolic",
    ),
    (
        NAV_ROUTE_HISTORY_CLASS,
        "rufin-route-history-symbolic",
        "rufin-route-history-selected-symbolic",
    ),
    (
        NAV_ROUTE_ALBUMS_CLASS,
        "rufin-route-albums-symbolic",
        "rufin-route-albums-selected-symbolic",
    ),
    (
        NAV_ROUTE_TRACKS_CLASS,
        "rufin-route-tracks-symbolic",
        "rufin-route-tracks-selected-symbolic",
    ),
    (
        NAV_ROUTE_ARTISTS_CLASS,
        "rufin-route-artists-symbolic",
        "rufin-route-artists-selected-symbolic",
    ),
    (
        NAV_ROUTE_ALBUM_ARTISTS_CLASS,
        "rufin-route-album-artists-symbolic",
        "rufin-route-album-artists-selected-symbolic",
    ),
    (
        NAV_ROUTE_GENRES_CLASS,
        "rufin-route-genres-symbolic",
        "rufin-route-genres-selected-symbolic",
    ),
    (
        NAV_ROUTE_MOODS_CLASS,
        "rufin-route-moods-symbolic",
        "rufin-route-moods-selected-symbolic",
    ),
    (
        NAV_ROUTE_FOLDERS_CLASS,
        "rufin-route-folders-symbolic",
        "rufin-route-folders-selected-symbolic",
    ),
    (
        NAV_ROUTE_PLAYLISTS_CLASS,
        "rufin-route-playlists-symbolic",
        "rufin-route-playlists-selected-symbolic",
    ),
    (
        NAV_ROUTE_SMART_PLAYLISTS_CLASS,
        "rufin-route-smart-playlists-symbolic",
        "rufin-route-smart-playlists-selected-symbolic",
    ),
];

pub(crate) struct NavigationState {
    pub(crate) routes: RefCell<RouteStack>,
}

pub(super) struct PrimaryMenuWidgets {
    pub(super) button: gtk::Button,
    pub(super) popover: RefCell<Option<gtk::PopoverMenu>>,
    pub(super) click_handler: RefCell<Option<glib::SignalHandlerId>>,
    pub(super) unmap_handler: RefCell<Option<glib::SignalHandlerId>>,
}

pub(crate) struct NavigationWidgets {
    pub(super) split_view: adw::OverlaySplitView,
    pub(super) left_resize_handle: gtk::Box,
    pub(super) normal_nav_slot: gtk::ScrolledWindow,
    pub(super) compact_nav_slot: gtk::ScrolledWindow,
    pub(super) tiny_nav_button: gtk::Button,
    pub(super) normal_nav: gtk::Box,
    pub(super) compact_nav: gtk::Box,
    pub(super) normal_main_menu: PrimaryMenuWidgets,
    pub(super) compact_main_menu: PrimaryMenuWidgets,
}

pub(super) fn build_normal_navigation(shell: &Rc<Shell>) {
    shell
        .navigation_view
        .normal_nav
        .append(&primary_menu_button(
            &shell.navigation_view.normal_main_menu.button,
            &shell.navigation_view.normal_main_menu.popover,
            &shell.navigation_view.normal_main_menu.click_handler,
            &shell.navigation_view.normal_main_menu.unmap_handler,
            shell,
            false,
        ));
    for item in nav_items(shell) {
        shell.navigation_view.normal_nav.append(&nav_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
            false,
        ));
    }

    shell.navigation_view.normal_nav.append(&sidebar_spacer());
}

pub(super) fn build_compact_navigation(shell: &Rc<Shell>) {
    shell
        .navigation_view
        .compact_nav
        .append(&primary_menu_button(
            &shell.navigation_view.compact_main_menu.button,
            &shell.navigation_view.compact_main_menu.popover,
            &shell.navigation_view.compact_main_menu.click_handler,
            &shell.navigation_view.compact_main_menu.unmap_handler,
            shell,
            true,
        ));
    for item in nav_items(shell) {
        shell.navigation_view.compact_nav.append(&rail_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
        ));
    }
    shell.navigation_view.compact_nav.append(&sidebar_spacer());
}

pub(super) fn rebuild_navigation(shell: &Rc<Shell>) {
    clear_box(&shell.navigation_view.normal_nav);
    clear_box(&shell.navigation_view.compact_nav);
    build_normal_navigation(shell);
    build_compact_navigation(shell);
    update_navigation_selection(shell.as_ref());
}

impl Shell {
    pub(crate) fn rebuild_sidebar_navigation(self: &Rc<Self>) {
        rebuild_navigation(self);
        self.update_layout();
    }
}

pub(super) fn update_navigation_selection(shell: &Shell) {
    let active_route_class = nav_route_class(shell.navigation.routes.borrow().current());
    update_navigation_selection_in(&shell.navigation_view.normal_nav, active_route_class);
    update_navigation_selection_in(&shell.navigation_view.compact_nav, active_route_class);
}

pub(super) fn relocalize_primary_menu_button(
    button: &gtk::Button,
    popover_slot: &RefCell<Option<gtk::PopoverMenu>>,
    handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    unmap_handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    shell: &Rc<Shell>,
    compact: bool,
) {
    chrome::configure_primary_menu_button(button);
    button.set_child(Some(&sidebar_menu_content(compact)));
    update_primary_menu_popover(
        button,
        popover_slot,
        handler_slot,
        unmap_handler_slot,
        primary_menu_popover(shell),
        shell,
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

        if !widget.has_css_class("nav-button") {
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
    popover_slot: &RefCell<Option<gtk::PopoverMenu>>,
    handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    unmap_handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
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
        popover_slot,
        handler_slot,
        unmap_handler_slot,
        shell,
        compact,
    );
    button.clone()
}

fn update_primary_menu_popover(
    button: &gtk::Button,
    popover_slot: &RefCell<Option<gtk::PopoverMenu>>,
    handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    unmap_handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    popover: gtk::PopoverMenu,
    shell: &Rc<Shell>,
) {
    if let Some(handler) = handler_slot.borrow_mut().take() {
        button.disconnect(handler);
    }
    if let Some(handler) = unmap_handler_slot.borrow_mut().take() {
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
    let row_shell = Rc::downgrade(shell);
    let handler = button.connect_clicked(move |_| {
        if let Some(shell) = row_shell.upgrade() {
            refresh_primary_menu(&row_popover, &shell);
            row_popover.popup();
        }
    });
    let unmap_handler = popdown_on_anchor_unmap(button, &popover);
    *handler_slot.borrow_mut() = Some(handler);
    *unmap_handler_slot.borrow_mut() = Some(unmap_handler);
}

pub(super) fn popup_primary_menu(
    shell: &Rc<Shell>,
    popover_slot: &RefCell<Option<gtk::PopoverMenu>>,
) {
    if let Some(popover) = popover_slot.borrow().as_ref() {
        refresh_primary_menu(popover, shell);
        popover.popup();
    }
}

fn refresh_primary_menu(popover: &gtk::PopoverMenu, shell: &Rc<Shell>) {
    popover.set_menu_model(Some(&primary_menu_model(shell)));
    style_primary_menu(popover);
}

fn primary_menu_popover(shell: &Rc<Shell>) -> gtk::PopoverMenu {
    let popover = gtk::PopoverMenu::from_model_full(
        &primary_menu_model(shell),
        gtk::PopoverMenuFlags::NESTED,
    );
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Right);
    style_primary_menu(&popover);
    popover
}

fn style_primary_menu(popover: &gtk::PopoverMenu) {
    popover.add_css_class(PRIMARY_MENU_CLASS);
    show_native_menu_icons(popover);
    replace_native_menu_checkmarks(popover);
}

fn primary_menu_model(shell: &Rc<Shell>) -> gio::Menu {
    let menu = gio::Menu::new();

    let source = gio::Menu::new();
    let (source_name, source_icon_name, source_menu) = source_submenu(shell);
    let source_item = gio::MenuItem::new_submenu(Some(&source_name), &source_menu);
    source_item.set_icon(&gio::ThemedIcon::new(source_icon_name));
    source.append_item(&source_item);
    menu.append_section(None, &source);

    let preferences = gio::Menu::new();
    append_menu_action(
        &preferences,
        &tr("Preferences"),
        "win.preferences",
        "preferences-system-symbolic",
    );
    append_menu_action(
        &preferences,
        &primary_menu_private_mode_label(shell.as_ref()),
        "win.toggle-private-mode",
        "system-lock-screen-symbolic",
    );
    menu.append_section(None, &preferences);

    let window = gio::Menu::new();
    append_menu_action(
        &window,
        &tr("Keyboard Shortcuts"),
        "win.show-shortcuts",
        "preferences-desktop-keyboard-shortcuts-symbolic",
    );
    append_menu_action(
        &window,
        &tr("Toggle Fullscreen"),
        "win.toggle-fullscreen",
        "view-fullscreen-symbolic",
    );
    append_menu_action(
        &window,
        &primary_menu_sidebar_toggle_label(shell.as_ref()),
        "win.toggle-left-sidebar",
        primary_menu_sidebar_toggle_icon(shell.as_ref()),
    );
    menu.append_section(None, &window);

    let information = gio::Menu::new();
    append_menu_action(
        &information,
        &tr("Version History"),
        "win.show-release-notes",
        "rufin-view-list-symbolic",
    );
    append_menu_action(
        &information,
        &tr("Troubleshooting"),
        "win.troubleshooting",
        "utilities-terminal-symbolic",
    );
    append_menu_action(
        &information,
        &tr("About Rufin"),
        "win.about",
        "help-about-symbolic",
    );
    menu.append_section(None, &information);

    menu
}

fn append_menu_action(menu: &gio::Menu, label: &str, action: &str, icon_name: &str) {
    let item = gio::MenuItem::new(Some(label), Some(action));
    item.set_icon(&gio::ThemedIcon::new(icon_name));
    menu.append_item(&item);
}

fn primary_menu_sidebar_toggle_label(shell: &Shell) -> String {
    if shell.left_sidebar_mode() == ResolvedLeftSidebarMode::Full {
        tr("Collapse sidebar")
    } else {
        tr("Expand sidebar")
    }
}

fn primary_menu_sidebar_toggle_icon(shell: &Shell) -> &'static str {
    if shell.left_sidebar_mode() == ResolvedLeftSidebarMode::Full {
        "rufin-sidebar-hide-symbolic"
    } else {
        "sidebar-show-symbolic"
    }
}

fn primary_menu_private_mode_label(shell: &Shell) -> String {
    if shell.settings.current.borrow().private_mode {
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
        .settings
        .current
        .borrow()
        .sidebar
        .route_items
        .iter()
        .filter(|entry| entry.visible)
        .map(|entry| nav_item(entry.item))
        .collect()
}

pub(crate) fn sidebar_route_at_position(shell: &Shell, position: usize) -> Option<Route> {
    sidebar_route_at_position_in(
        &shell.settings.current.borrow().sidebar.route_items,
        position,
    )
}

fn sidebar_route_at_position_in(
    route_items: &[crate::SidebarRouteItemSettings],
    position: usize,
) -> Option<Route> {
    let item = route_items
        .iter()
        .filter(|entry| entry.visible)
        .nth(position.checked_sub(1)?)?;
    Some(nav_item(item.item).route)
}

fn nav_item(item: SidebarRouteItem) -> NavItem {
    match item {
        SidebarRouteItem::Home => NavItem {
            icon_name: "rufin-route-home-symbolic",
            label: msgid("Home"),
            route: Route::Home,
        },
        SidebarRouteItem::Search => NavItem {
            icon_name: "rufin-route-search-symbolic",
            label: msgid("Search"),
            route: Route::Search,
        },
        SidebarRouteItem::Favorites => NavItem {
            icon_name: "rufin-route-favorites-symbolic",
            label: msgid("Favorites"),
            route: Route::Favorites,
        },
        SidebarRouteItem::History => NavItem {
            icon_name: "rufin-route-history-symbolic",
            label: msgid("History"),
            route: Route::History,
        },
        SidebarRouteItem::Albums => NavItem {
            icon_name: "rufin-route-albums-symbolic",
            label: msgid("Albums"),
            route: Route::Albums,
        },
        SidebarRouteItem::Tracks => NavItem {
            icon_name: "rufin-route-tracks-symbolic",
            label: msgid("Tracks"),
            route: Route::Tracks,
        },
        SidebarRouteItem::Artists => NavItem {
            icon_name: "rufin-route-artists-symbolic",
            label: msgid("Artists"),
            route: Route::Artists,
        },
        SidebarRouteItem::AlbumArtists => NavItem {
            icon_name: "rufin-route-album-artists-symbolic",
            label: msgid("Album Artists"),
            route: Route::AlbumArtists,
        },
        SidebarRouteItem::Genres => NavItem {
            icon_name: "rufin-route-genres-symbolic",
            label: msgid("Genres"),
            route: Route::Genres,
        },
        SidebarRouteItem::Moods => NavItem {
            icon_name: "rufin-route-moods-symbolic",
            label: msgid("Moods"),
            route: Route::Moods,
        },
        SidebarRouteItem::Folders => NavItem {
            icon_name: "rufin-route-folders-symbolic",
            label: msgid("Folders"),
            route: Route::Folders { path: Vec::new() },
        },
        SidebarRouteItem::Playlists => NavItem {
            icon_name: "rufin-route-playlists-symbolic",
            label: msgid("Playlists"),
            route: Route::Playlists,
        },
        SidebarRouteItem::SmartPlaylists => NavItem {
            icon_name: "rufin-route-smart-playlists-symbolic",
            label: msgid("Smart Playlists"),
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
    button.connect_clicked(move |_| {
        if shell.navigation_view.split_view.is_collapsed() {
            shell.navigation_view.split_view.set_show_sidebar(false);
        }
        shell.navigate(route.clone());
    });
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
        Route::Search => Some(NAV_ROUTE_SEARCH_CLASS),
        Route::Favorites => Some(NAV_ROUTE_FAVORITES_CLASS),
        Route::History => Some(NAV_ROUTE_HISTORY_CLASS),
        Route::Albums | Route::AlbumDetail(_) => Some(NAV_ROUTE_ALBUMS_CLASS),
        Route::Tracks => Some(NAV_ROUTE_TRACKS_CLASS),
        Route::Artists
        | Route::ArtistDetail(_)
        | Route::ArtistDiscography(_)
        | Route::ArtistTracks(_) => Some(NAV_ROUTE_ARTISTS_CLASS),
        Route::AlbumArtists => Some(NAV_ROUTE_ALBUM_ARTISTS_CLASS),
        Route::Genres | Route::GenreDetail(_) => Some(NAV_ROUTE_GENRES_CLASS),
        Route::Moods | Route::MoodDetail(_) => Some(NAV_ROUTE_MOODS_CLASS),
        Route::Folders { .. } => Some(NAV_ROUTE_FOLDERS_CLASS),
        Route::Playlists | Route::PlaylistDetail(_) => Some(NAV_ROUTE_PLAYLISTS_CLASS),
        Route::SmartPlaylists | Route::SmartPlaylistDetail(_) => {
            Some(NAV_ROUTE_SMART_PLAYLISTS_CLASS)
        }
    }
}

pub(super) fn install_mouse_history_buttons(shell: &Rc<Shell>) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);

    let history_shell = Rc::clone(shell);
    click.connect_pressed(move |click, _, _, _| match click.current_button() {
        MOUSE_BACK_BUTTON => {
            click.set_state(gtk::EventSequenceState::Claimed);
            history_shell.go_back();
        }
        MOUSE_FORWARD_BUTTON => {
            click.set_state(gtk::EventSequenceState::Claimed);
            history_shell.go_forward();
        }
        _ => {}
    });

    shell.chrome.window.add_controller(click);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_position_uses_visible_configured_route_order() {
        let route_items = vec![
            crate::SidebarRouteItemSettings {
                item: SidebarRouteItem::Playlists,
                visible: false,
            },
            crate::SidebarRouteItemSettings {
                item: SidebarRouteItem::Albums,
                visible: true,
            },
            crate::SidebarRouteItemSettings {
                item: SidebarRouteItem::Home,
                visible: true,
            },
        ];

        assert_eq!(
            sidebar_route_at_position_in(&route_items, 1),
            Some(Route::Albums)
        );
        assert_eq!(
            sidebar_route_at_position_in(&route_items, 2),
            Some(Route::Home)
        );
        assert_eq!(sidebar_route_at_position_in(&route_items, 0), None);
        assert_eq!(sidebar_route_at_position_in(&route_items, 3), None);
    }
    use library::{PlaylistId, SmartPlaylistId};
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
        assert_eq!(
            nav_route_class(&Route::Genres),
            Some(NAV_ROUTE_GENRES_CLASS)
        );
        assert_eq!(nav_route_class(&Route::Moods), Some(NAV_ROUTE_MOODS_CLASS));
        assert_ne!(
            nav_route_class(&Route::Genres),
            nav_route_class(&Route::Moods)
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
