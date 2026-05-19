use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{MusicFolder, MusicFolderId, Route, ServerIdentity, SidebarRouteItem};
use rufin_store::ServerLocalAccess;

use super::{
    Shell, icon_button,
    layout::{COMPACT_RAIL_WIDTH, NORMAL_SIDEBAR_WIDTH},
};
use crate::controller::LibrarySnapshot;
use crate::i18n::tr;

const COMPACT_RAIL_ICON_SIZE: i32 = 22;
const COMPACT_RAIL_LABEL_WIDTH: i32 = COMPACT_RAIL_WIDTH - 8;
const COMPACT_RAIL_LABEL_WIDTH_CHARS: i32 = 8;
const NORMAL_SELECTOR_NON_LABEL_WIDTH: i32 = 100;
const NORMAL_SELECTOR_LABEL_WIDTH: i32 = NORMAL_SIDEBAR_WIDTH - NORMAL_SELECTOR_NON_LABEL_WIDTH;
const NORMAL_SELECTOR_LABEL_WIDTH_CHARS: i32 = 12;

pub(super) struct ServerSelector {
    pub normal_button: gtk::MenuButton,
    pub normal_icon: gtk::Image,
    pub normal_name: gtk::Label,
    pub normal_subtitle: gtk::Label,
    pub compact_button: gtk::MenuButton,
    pub compact_icon: gtk::Image,
    pub compact_label: gtk::Label,
}

struct ServerSelectorContent {
    name: String,
    subtitle: String,
    detail: String,
    active_server: Option<ServerIdentity>,
    servers: Vec<ServerIdentity>,
    local_access: Option<ServerLocalAccess>,
    music_folders: Vec<MusicFolder>,
    selected_music_folder_id: Option<MusicFolderId>,
    has_server: bool,
}

pub(super) fn build_server_selector() -> ServerSelector {
    let normal_button = gtk::MenuButton::new();
    normal_button.add_css_class("server-selector");
    normal_button.add_css_class("flat");
    normal_button.set_always_show_arrow(false);
    normal_button.set_can_shrink(true);
    normal_button.set_margin_start(8);
    normal_button.set_margin_end(8);
    normal_button.set_margin_bottom(4);
    normal_button.set_size_request(NORMAL_SIDEBAR_WIDTH - 16, -1);

    let normal_content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    normal_content.set_halign(gtk::Align::Fill);
    let normal_icon = gtk::Image::from_icon_name("network-server-symbolic");
    normal_icon.set_pixel_size(20);
    normal_icon.set_size_request(20, 20);
    normal_content.append(&normal_icon);

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let normal_name = gtk::Label::new(None);
    configure_normal_selector_label(&normal_name);
    let normal_subtitle = gtk::Label::new(None);
    normal_subtitle.add_css_class("muted");
    configure_normal_selector_label(&normal_subtitle);
    labels.append(&normal_name);
    labels.append(&normal_subtitle);
    normal_content.append(&labels);
    normal_content.append(&gtk::Image::from_icon_name("view-more-symbolic"));
    normal_button.set_child(Some(&normal_content));

    let compact_button = gtk::MenuButton::new();
    compact_button.add_css_class("nav-button");
    compact_button.add_css_class("flat");
    compact_button.add_css_class("rail-button");
    compact_button.add_css_class("server-selector");
    compact_button.set_always_show_arrow(false);
    compact_button.set_can_shrink(true);
    compact_button.set_size_request(COMPACT_RAIL_WIDTH - 2, -1);
    let compact_content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    compact_content.set_halign(gtk::Align::Center);
    compact_content.set_size_request(COMPACT_RAIL_LABEL_WIDTH, -1);
    let compact_icon = gtk::Image::from_icon_name("network-server-symbolic");
    compact_icon.set_pixel_size(COMPACT_RAIL_ICON_SIZE);
    compact_content.append(&compact_icon);
    let compact_label = gtk::Label::new(None);
    configure_rail_label(&compact_label);
    compact_content.append(&compact_label);
    compact_button.set_child(Some(&compact_content));

    ServerSelector {
        normal_button,
        normal_icon,
        normal_name,
        normal_subtitle,
        compact_button,
        compact_icon,
        compact_label,
    }
}

pub(super) fn update_server_selector(shell: &Rc<Shell>) {
    let selector = &shell.server_selector;
    let library = shell.state.library.borrow().clone();
    let content = server_selector_content(library);
    let tooltip = format!("{}: {}", tr("Server"), content.name);
    let icon_name = content
        .active_server
        .as_ref()
        .map(server_icon_name)
        .unwrap_or("network-server-symbolic");

    selector.normal_icon.set_icon_name(Some(&icon_name));
    selector.normal_name.set_text(&content.name);
    selector.normal_subtitle.set_text(&content.subtitle);
    selector
        .normal_subtitle
        .set_visible(!content.subtitle.is_empty());
    selector.normal_button.set_tooltip_text(Some(&tooltip));
    selector
        .normal_button
        .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
    selector
        .normal_button
        .set_popover(Some(&server_selection_popover(shell, &content)));

    selector.compact_icon.set_icon_name(Some(&icon_name));
    selector
        .compact_label
        .set_text(&compact_sidebar_label_text(&content.name));
    selector.compact_button.set_tooltip_text(Some(&tooltip));
    selector
        .compact_button
        .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
    selector
        .compact_button
        .set_popover(Some(&server_selection_popover(shell, &content)));
}

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

fn server_selector_content(library: LibrarySnapshot) -> ServerSelectorContent {
    let Some(server) = library.server.as_ref() else {
        return ServerSelectorContent {
            name: tr("No server"),
            subtitle: String::new(),
            detail: tr("No server"),
            active_server: None,
            servers: library.servers,
            local_access: None,
            music_folders: Vec::new(),
            selected_music_folder_id: None,
            has_server: false,
        };
    };

    let name = server_display_name(server);
    let subtitle = library
        .selected_music_folder_id
        .as_ref()
        .and_then(|selected| {
            library
                .music_folders
                .iter()
                .find(|folder| folder.id == *selected)
        })
        .map(|folder| folder.name.clone())
        .unwrap_or_default();
    let detail = if server.base_url.trim().is_empty() {
        provider_display_name(&server.provider)
    } else {
        server.base_url.clone()
    };

    ServerSelectorContent {
        name,
        subtitle,
        detail,
        active_server: Some(server.clone()),
        servers: library.servers,
        local_access: library.local_access,
        music_folders: library.music_folders,
        selected_music_folder_id: library.selected_music_folder_id,
        has_server: true,
    }
}

fn server_display_name(server: &ServerIdentity) -> String {
    let name = server.name.trim();
    if name.is_empty() {
        provider_display_name(&server.provider)
    } else {
        name.to_string()
    }
}

fn provider_display_name(provider: &str) -> String {
    match provider {
        "jellyfin" => tr("Jellyfin"),
        "navidrome" => tr("Navidrome"),
        "subsonic" | "opensubsonic" => tr("Subsonic / OpenSubsonic"),
        "local" | "fake" => tr("Local"),
        provider => provider.to_string(),
    }
}

fn server_icon_name(server: &ServerIdentity) -> &'static str {
    provider_icon_name(&server.provider)
}

fn provider_icon_name(provider: &str) -> &'static str {
    match provider {
        "jellyfin" => "io.github.screwys.Rufin.provider.jellyfin",
        "navidrome" => "io.github.screwys.Rufin.provider.navidrome",
        "subsonic" | "opensubsonic" => "io.github.screwys.Rufin.provider.opensubsonic",
        "local" | "fake" => "folder-music-symbolic",
        _ => "network-server-symbolic",
    }
}

fn server_selection_popover(shell: &Rc<Shell>, content: &ServerSelectorContent) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrapper.add_css_class("server-selector-popover");

    wrapper.append(&server_section_label(&tr("Select Server")));
    if content.servers.is_empty() {
        let row = server_option_row(None, &content.name, &content.detail, content.has_server);
        row.set_sensitive(false);
        wrapper.append(&row);
    } else {
        for server in &content.servers {
            let active = content
                .active_server
                .as_ref()
                .is_some_and(|active| active.id == server.id);
            let title = server_display_name(server);
            let detail = server_detail(server);
            let row = server_option_row(Some(server), &title, &detail, active);
            let row_popover = popover.clone();
            let controller = shell.controller.clone();
            let server_id = server.id.clone();
            row.connect_clicked(move |_| {
                row_popover.popdown();
                controller.activate_server(server_id.clone());
            });
            wrapper.append(&row);
        }
    }

    if let Some(server) = &content.active_server {
        let manage = server_action_row(
            "document-edit-symbolic",
            &tr("Manage Server"),
            &tr("Configure local folder access"),
            false,
        );
        let row_popover = popover.clone();
        let manage_shell = Rc::clone(shell);
        let managed_server = server.clone();
        manage.connect_clicked(move |_| {
            row_popover.popdown();
            manage_shell.present_manage_server_dialog(managed_server.clone());
        });
        wrapper.append(&manage);

        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        wrapper.append(&separator);
        wrapper.append(&server_section_label(&tr("Server Library")));
        append_server_music_folder_rows(shell, &popover, &wrapper, server, content);

        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        wrapper.append(&separator);
        wrapper.append(&server_section_label(&tr("Local Files")));
        append_local_file_rows(
            shell,
            &popover,
            &wrapper,
            &server,
            content.local_access.as_ref(),
        );
    }

    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    wrapper.append(&separator);

    let add = gtk::Button::new();
    add.add_css_class("flat");
    add.add_css_class("server-option");
    add.add_css_class("server-add-option");
    let add_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    add_content.append(&gtk::Image::from_icon_name("list-add-symbolic"));
    let label = gtk::Label::new(Some(&tr("Add Server")));
    label.set_xalign(0.0);
    add_content.append(&label);
    add.set_child(Some(&add_content));
    let row_popover = popover.clone();
    let add_shell = Rc::clone(shell);
    add.connect_clicked(move |_| {
        row_popover.popdown();
        add_shell.present_add_server_dialog();
    });
    wrapper.append(&add);

    popover.set_child(Some(&wrapper));
    popover
}

fn append_server_music_folder_rows(
    shell: &Rc<Shell>,
    popover: &gtk::Popover,
    wrapper: &gtk::Box,
    server: &ServerIdentity,
    content: &ServerSelectorContent,
) {
    let all_active = content.selected_music_folder_id.is_none();
    let all = server_action_row("folder-symbolic", &tr("All Music"), "", all_active);
    let row_popover = popover.clone();
    let controller = shell.controller.clone();
    let server_id = server.id.clone();
    all.connect_clicked(move |_| {
        row_popover.popdown();
        controller.set_selected_music_folder(server_id.clone(), None);
    });
    wrapper.append(&all);

    for folder in &content.music_folders {
        let active = content
            .selected_music_folder_id
            .as_ref()
            .is_some_and(|selected| *selected == folder.id);
        let row = server_action_row("folder-music-symbolic", &folder.name, "", active);
        let row_popover = popover.clone();
        let controller = shell.controller.clone();
        let server_id = server.id.clone();
        let folder_id = folder.id.clone();
        row.connect_clicked(move |_| {
            row_popover.popdown();
            controller.set_selected_music_folder(server_id.clone(), Some(folder_id.clone()));
        });
        wrapper.append(&row);
    }
}

fn append_local_file_rows(
    shell: &Rc<Shell>,
    popover: &gtk::Popover,
    wrapper: &gtk::Box,
    server: &ServerIdentity,
    access: Option<&ServerLocalAccess>,
) {
    if server.provider != "local" {
        let none = server_action_row(
            "audio-volume-muted-symbolic",
            &tr("None"),
            &tr("Use server streams only"),
            access.is_none(),
        );
        let row_popover = popover.clone();
        let controller = shell.controller.clone();
        let server_id = server.id.clone();
        none.connect_clicked(move |_| {
            row_popover.popdown();
            controller.clear_server_local_access(server_id.clone());
        });
        wrapper.append(&none);
    }

    match access {
        Some(access) => {
            let title = local_folder_title(access);
            let detail = local_folder_path(access);
            let folder = server_action_row("folder-music-symbolic", &title, &detail, true);
            let row_popover = popover.clone();
            let manage_shell = Rc::clone(shell);
            let server = server.clone();
            folder.connect_clicked(move |_| {
                row_popover.popdown();
                manage_shell.present_manage_server_dialog(server.clone());
            });
            wrapper.append(&folder);
        }
        None => {
            let choose = server_action_row(
                "folder-open-symbolic",
                &tr("Choose Folder"),
                &tr("Add local folder access"),
                false,
            );
            let row_popover = popover.clone();
            let manage_shell = Rc::clone(shell);
            let server = server.clone();
            choose.connect_clicked(move |_| {
                row_popover.popdown();
                manage_shell.present_manage_server_dialog(server.clone());
            });
            wrapper.append(&choose);
        }
    }
}

fn server_option_row(
    server: Option<&ServerIdentity>,
    title: &str,
    detail: &str,
    active: bool,
) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("server-option");
    if active {
        row.add_css_class("active");
    }

    let row_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row_content.set_halign(gtk::Align::Fill);
    let icon_name = server
        .map(server_icon_name)
        .unwrap_or("network-server-symbolic");
    row_content.append(&gtk::Image::from_icon_name(&icon_name));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let detail = gtk::Label::new(Some(detail));
    detail.add_css_class("muted");
    detail.set_xalign(0.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&name);
    labels.append(&detail);
    row_content.append(&labels);
    if active {
        row_content.append(&gtk::Image::from_icon_name("object-select-symbolic"));
    }
    row.set_child(Some(&row_content));
    row
}

fn server_action_row(icon_name: &str, title: &str, detail: &str, active: bool) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("server-option");
    if active {
        row.add_css_class("active");
    }

    let row_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row_content.set_halign(gtk::Align::Fill);
    row_content.append(&gtk::Image::from_icon_name(icon_name));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let detail = gtk::Label::new(Some(detail));
    detail.add_css_class("muted");
    detail.set_xalign(0.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&name);
    labels.append(&detail);
    row_content.append(&labels);
    if active {
        row_content.append(&gtk::Image::from_icon_name("object-select-symbolic"));
    }
    row.set_child(Some(&row_content));
    row
}

fn server_section_label(label: &str) -> gtk::Label {
    let section = gtk::Label::new(Some(label));
    section.add_css_class("server-section-label");
    section.set_xalign(0.0);
    section.set_margin_top(2);
    section.set_margin_start(4);
    section
}

fn server_detail(server: &ServerIdentity) -> String {
    if server.base_url.trim().is_empty() {
        provider_display_name(&server.provider)
    } else {
        server.base_url.clone()
    }
}

fn local_folder_path(access: &ServerLocalAccess) -> String {
    access
        .path_replace_to
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&access.root_path)
        .to_string()
}

fn local_folder_title(access: &ServerLocalAccess) -> String {
    let path = local_folder_path(access);
    Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or(path)
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
    button.set_tooltip_text(Some(&tr(label)));

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

fn configure_normal_selector_label(label: &gtk::Label) {
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_request(NORMAL_SELECTOR_LABEL_WIDTH);
    label.set_size_request(NORMAL_SELECTOR_LABEL_WIDTH, -1);
    label.set_width_chars(1);
    label.set_max_width_chars(NORMAL_SELECTOR_LABEL_WIDTH_CHARS);
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
