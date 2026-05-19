use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{
    LibrarySourceSelection, LocalLibraryFolder, MusicFolder, MusicFolderId, ServerIdentity,
};

use super::{
    Shell,
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
    selected_source: Option<LibrarySourceSelection>,
    active_server: Option<ServerIdentity>,
    servers: Vec<ServerIdentity>,
    local_folders: Vec<LocalLibraryFolder>,
    music_folders: Vec<MusicFolder>,
    selected_music_folder_id: Option<MusicFolderId>,
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
    let tooltip = format!("{}: {}", tr("Source"), content.name);
    let icon_name = source_icon_name(&content);

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

fn server_selector_content(library: LibrarySnapshot) -> ServerSelectorContent {
    let selected_source = library.selected_source.clone();
    let Some(server) = library.server.as_ref() else {
        return ServerSelectorContent {
            name: tr("No source"),
            subtitle: String::new(),
            selected_source,
            active_server: None,
            servers: library.servers,
            local_folders: library.local_folders,
            music_folders: Vec::new(),
            selected_music_folder_id: None,
        };
    };

    let name = server_display_name(server);
    let subtitle = if matches!(selected_source, Some(LibrarySourceSelection::Local)) {
        local_source_detail(&library.local_folders)
    } else {
        library
            .selected_music_folder_id
            .as_ref()
            .and_then(|selected| {
                library
                    .music_folders
                    .iter()
                    .find(|folder| folder.id == *selected)
            })
            .map(|folder| folder.name.clone())
            .unwrap_or_default()
    };

    ServerSelectorContent {
        name,
        subtitle,
        selected_source,
        active_server: Some(server.clone()),
        servers: library.servers,
        local_folders: library.local_folders,
        music_folders: library.music_folders,
        selected_music_folder_id: library.selected_music_folder_id,
    }
}

fn source_icon_name(content: &ServerSelectorContent) -> &'static str {
    match &content.selected_source {
        Some(LibrarySourceSelection::Local) => "folder-symbolic",
        Some(LibrarySourceSelection::Server(_)) => content
            .active_server
            .as_ref()
            .map(server_icon_name)
            .unwrap_or("network-server-symbolic"),
        None => "network-server-symbolic",
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
        "local" | "fake" => "folder-symbolic",
        _ => "network-server-symbolic",
    }
}

fn server_selection_popover(shell: &Rc<Shell>, content: &ServerSelectorContent) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrapper.add_css_class("server-selector-popover");

    wrapper.append(&server_section_label(&tr("Select Source")));
    if content.servers.is_empty() {
        let row = server_option_row(None, &tr("No servers configured"), "", false);
        row.set_sensitive(false);
        wrapper.append(&row);
    } else {
        for server in &content.servers {
            let active = matches!(
                &content.selected_source,
                Some(LibrarySourceSelection::Server(server_id)) if *server_id == server.id
            );
            let title = server_display_name(server);
            let detail = server_detail(server);
            let row = server_option_row(Some(server), &title, &detail, active);
            let row_popover = popover.clone();
            let controller = shell.controller.clone();
            let server_id = server.id.clone();
            row.connect_clicked(move |_| {
                row_popover.popdown();
                controller.select_source(LibrarySourceSelection::Server(server_id.clone()));
            });
            wrapper.append(&row);
        }
    }

    let local_active = matches!(content.selected_source, Some(LibrarySourceSelection::Local));
    let local = server_action_row(
        "folder-symbolic",
        &tr("Local"),
        &local_source_detail(&content.local_folders),
        local_active,
    );
    let row_popover = popover.clone();
    let controller = shell.controller.clone();
    local.connect_clicked(move |_| {
        row_popover.popdown();
        controller.select_source(LibrarySourceSelection::Local);
    });
    wrapper.append(&local);

    if let Some(server) = &content.active_server
        && matches!(
            content.selected_source,
            Some(LibrarySourceSelection::Server(_))
        )
    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        wrapper.append(&separator);
        wrapper.append(&server_section_label(&tr("Server Library")));
        append_server_music_folder_rows(shell, &popover, &wrapper, server, content);
    }

    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    wrapper.append(&separator);

    let manage = gtk::Button::new();
    manage.add_css_class("flat");
    manage.add_css_class("server-option");
    let manage_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    manage_content.append(&gtk::Image::from_icon_name("document-edit-symbolic"));
    let label = gtk::Label::new(Some(&tr("Manage")));
    label.set_xalign(0.0);
    manage_content.append(&label);
    manage.set_child(Some(&manage_content));
    let row_popover = popover.clone();
    let manage_shell = Rc::clone(shell);
    manage.connect_clicked(move |_| {
        row_popover.popdown();
        manage_shell.present_library_preferences_dialog();
    });
    wrapper.append(&manage);

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

fn local_source_detail(folders: &[LocalLibraryFolder]) -> String {
    match folders.len() {
        0 => tr("No local folders configured"),
        1 => folders[0].path.clone(),
        count => format!("{} {}", count, tr("folders")),
    }
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

fn compact_sidebar_label_text(label: &str) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut parts = trimmed.split_whitespace();
    let first = parts.next().unwrap_or(trimmed);
    if parts.next().is_some() {
        first.to_string()
    } else {
        trimmed.to_string()
    }
}

fn configure_normal_selector_label(label: &gtk::Label) {
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_request(NORMAL_SELECTOR_LABEL_WIDTH);
    label.set_size_request(NORMAL_SELECTOR_LABEL_WIDTH, -1);
    label.set_max_width_chars(NORMAL_SELECTOR_LABEL_WIDTH_CHARS);
}

fn configure_rail_label(label: &gtk::Label) {
    label.add_css_class("caption");
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_request(COMPACT_RAIL_LABEL_WIDTH);
    label.set_size_request(COMPACT_RAIL_LABEL_WIDTH, -1);
    label.set_lines(1);
    label.set_max_width_chars(COMPACT_RAIL_LABEL_WIDTH_CHARS);
}
