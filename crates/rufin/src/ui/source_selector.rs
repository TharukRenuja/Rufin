use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use domain::{
    LibrarySourceSelection, LocalLibraryFolder, MusicFolder, MusicFolderId, ServerId,
    ServerIdentity,
};
use gtk::glib;

use super::{Shell, folder_count_text};
use crate::controller::LibrarySnapshot;
use crate::i18n::tr;

const NORMAL_SELECTOR_ICON_SIZE: i32 = 22;
const NORMAL_SELECTOR_LABEL_WIDTH_CHARS: i32 = 18;
const SERVER_OPTION_ICON_TEXT_SPACING: i32 = 10;
const SERVER_OPTION_ICON_SIZE: i32 = 14;
const SERVER_OPTION_CHECK_SIZE: i32 = 13;
const SERVER_SELECTOR_POPOVER_WIDTH: i32 = 236;
const SERVER_SELECTOR_POPOVER_ANCHOR_Y: i32 = 148;
const LOCAL_SOURCE_SERVER_ID: &str = "local:server:library";

pub(super) struct ServerSelector {
    pub normal_button: gtk::Button,
    pub normal_icon: gtk::Image,
    pub normal_name: gtk::Label,
    pub normal_subtitle: gtk::Label,
    normal_popover: RefCell<Option<gtk::Popover>>,
    normal_click_handler: RefCell<Option<glib::SignalHandlerId>>,
    normal_hover_controller: RefCell<Option<gtk::EventControllerMotion>>,
    normal_unmap_handler: RefCell<Option<glib::SignalHandlerId>>,
    pub compact_button: gtk::Button,
    pub compact_icon: gtk::Image,
    pub compact_name: gtk::Label,
    pub compact_subtitle: gtk::Label,
    compact_popover: RefCell<Option<gtk::Popover>>,
    compact_click_handler: RefCell<Option<glib::SignalHandlerId>>,
    compact_hover_controller: RefCell<Option<gtk::EventControllerMotion>>,
    compact_unmap_handler: RefCell<Option<glib::SignalHandlerId>>,
}

struct ServerSelectorContent {
    name: String,
    selected_source: Option<LibrarySourceSelection>,
    active_server: Option<ServerIdentity>,
    servers: Vec<ServerIdentity>,
    local_folders: Vec<LocalLibraryFolder>,
    music_folders: Vec<MusicFolder>,
    selected_music_folder_id: Option<MusicFolderId>,
}

pub(super) fn build_server_selector() -> ServerSelector {
    let normal_button = gtk::Button::new();
    normal_button.add_css_class("server-selector");
    normal_button.add_css_class("menu-source-button");
    normal_button.add_css_class("flat");
    normal_button.set_can_shrink(true);
    normal_button.set_hexpand(true);
    normal_button.set_halign(gtk::Align::Fill);

    let normal_content = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    normal_content.set_hexpand(true);
    normal_content.set_halign(gtk::Align::Fill);
    normal_content.set_valign(gtk::Align::Center);
    normal_content.set_width_request(1);
    let normal_icon = gtk::Image::from_icon_name("network-server-symbolic");
    normal_icon.set_pixel_size(NORMAL_SELECTOR_ICON_SIZE);
    normal_icon.set_size_request(NORMAL_SELECTOR_ICON_SIZE, NORMAL_SELECTOR_ICON_SIZE);
    normal_icon.set_halign(gtk::Align::Center);
    normal_icon.set_valign(gtk::Align::Center);
    normal_content.append(&normal_icon);

    let normal_name = gtk::Label::new(None);
    configure_normal_selector_label(&normal_name);
    normal_name.add_css_class("source-selector-name");
    let normal_subtitle = gtk::Label::new(None);
    normal_subtitle.add_css_class("muted");
    configure_normal_selector_label(&normal_subtitle);
    normal_subtitle.add_css_class("source-selector-detail");
    let normal_labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
    normal_labels.set_hexpand(true);
    normal_labels.append(&normal_name);
    normal_labels.append(&normal_subtitle);
    normal_content.append(&normal_labels);

    let normal_arrow = gtk::Image::from_icon_name("go-next-symbolic");
    normal_arrow.set_pixel_size(12);
    normal_arrow.add_css_class("muted");
    normal_content.append(&normal_arrow);

    normal_button.set_child(Some(&normal_content));

    let compact_button = gtk::Button::new();
    compact_button.add_css_class("flat");
    compact_button.add_css_class("server-selector");
    compact_button.add_css_class("menu-source-button");
    compact_button.set_can_shrink(true);
    compact_button.set_hexpand(true);
    compact_button.set_halign(gtk::Align::Fill);
    compact_button.set_valign(gtk::Align::Center);
    let compact_content = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    compact_content.set_hexpand(true);
    compact_content.set_halign(gtk::Align::Fill);
    compact_content.set_valign(gtk::Align::Center);
    let compact_icon = gtk::Image::from_icon_name("network-server-symbolic");
    compact_icon.set_pixel_size(NORMAL_SELECTOR_ICON_SIZE);
    compact_icon.set_size_request(NORMAL_SELECTOR_ICON_SIZE, NORMAL_SELECTOR_ICON_SIZE);
    compact_icon.set_halign(gtk::Align::Center);
    compact_icon.set_valign(gtk::Align::Center);
    compact_content.append(&compact_icon);
    let compact_name = gtk::Label::new(None);
    configure_normal_selector_label(&compact_name);
    compact_name.add_css_class("source-selector-name");
    let compact_subtitle = gtk::Label::new(None);
    compact_subtitle.add_css_class("muted");
    configure_normal_selector_label(&compact_subtitle);
    compact_subtitle.add_css_class("source-selector-detail");
    let compact_labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
    compact_labels.set_hexpand(true);
    compact_labels.append(&compact_name);
    compact_labels.append(&compact_subtitle);
    compact_content.append(&compact_labels);
    let compact_arrow = gtk::Image::from_icon_name("go-next-symbolic");
    compact_arrow.set_pixel_size(12);
    compact_arrow.add_css_class("muted");
    compact_content.append(&compact_arrow);
    compact_button.set_child(Some(&compact_content));

    ServerSelector {
        normal_button,
        normal_icon,
        normal_name,
        normal_subtitle,
        normal_popover: RefCell::new(None),
        normal_click_handler: RefCell::new(None),
        normal_hover_controller: RefCell::new(None),
        normal_unmap_handler: RefCell::new(None),
        compact_button,
        compact_icon,
        compact_name,
        compact_subtitle,
        compact_popover: RefCell::new(None),
        compact_click_handler: RefCell::new(None),
        compact_hover_controller: RefCell::new(None),
        compact_unmap_handler: RefCell::new(None),
    }
}

pub(super) fn update_server_selector(shell: &Rc<Shell>) {
    let selector = &shell.server_selector;
    let library = shell.state.library.borrow().clone();
    let content = server_selector_content(library);
    let accessible_label = format!("{}: {}", tr("Source"), content.name);
    let icon_name = source_icon_name(&content);
    let subtitle = source_summary_detail(&content);

    selector.normal_icon.set_icon_name(Some(icon_name));
    selector.normal_name.set_text(&content.name);
    selector.normal_subtitle.set_text(&subtitle);
    selector.normal_subtitle.set_visible(!subtitle.is_empty());
    selector
        .normal_button
        .update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    update_selector_popover(
        &selector.normal_button,
        &selector.normal_popover,
        &selector.normal_click_handler,
        &selector.normal_hover_controller,
        &selector.normal_unmap_handler,
        server_selection_popover(shell, &content),
    );

    selector.compact_icon.set_icon_name(Some(icon_name));
    selector.compact_name.set_text(&content.name);
    selector.compact_subtitle.set_text(&subtitle);
    selector.compact_subtitle.set_visible(!subtitle.is_empty());
    selector
        .compact_button
        .update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    update_selector_popover(
        &selector.compact_button,
        &selector.compact_popover,
        &selector.compact_click_handler,
        &selector.compact_hover_controller,
        &selector.compact_unmap_handler,
        server_selection_popover(shell, &content),
    );
}

fn server_selector_content(library: LibrarySnapshot) -> ServerSelectorContent {
    let selected_source = library.selected_source.clone();
    let active_server = selected_source
        .as_ref()
        .and_then(|selection| selected_source_server(selection, &library))
        .or_else(|| library.server.clone());
    let Some(server) = active_server else {
        return ServerSelectorContent {
            name: tr("No source"),
            selected_source,
            active_server: None,
            servers: library.servers,
            local_folders: library.local_folders,
            music_folders: Vec::new(),
            selected_music_folder_id: None,
        };
    };

    let music_folders = if library
        .server
        .as_ref()
        .is_some_and(|loaded| loaded.id == server.id)
    {
        library.music_folders
    } else {
        Vec::new()
    };
    let selected_music_folder_id = if music_folders.is_empty() {
        None
    } else {
        library.selected_music_folder_id
    };
    let name = server_display_name(&server);
    ServerSelectorContent {
        name,
        selected_source,
        active_server: Some(server),
        servers: library.servers,
        local_folders: library.local_folders,
        music_folders,
        selected_music_folder_id,
    }
}

fn selected_source_server(
    selected_source: &LibrarySourceSelection,
    library: &LibrarySnapshot,
) -> Option<ServerIdentity> {
    match selected_source {
        LibrarySourceSelection::Local => Some(local_source_identity()),
        LibrarySourceSelection::Server(server_id) => library
            .servers
            .iter()
            .find(|server| &server.id == server_id)
            .cloned(),
    }
}

fn local_source_identity() -> ServerIdentity {
    ServerIdentity {
        id: ServerId::new(LOCAL_SOURCE_SERVER_ID),
        provider: "local".to_string(),
        name: tr("Local"),
        base_url: String::new(),
    }
}

fn update_selector_popover(
    button: &gtk::Button,
    popover_slot: &RefCell<Option<gtk::Popover>>,
    handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    hover_controller_slot: &RefCell<Option<gtk::EventControllerMotion>>,
    unmap_handler_slot: &RefCell<Option<glib::SignalHandlerId>>,
    popover: gtk::Popover,
) {
    if let Some(handler) = handler_slot.borrow_mut().take() {
        button.disconnect(handler);
    }
    if let Some(handler) = unmap_handler_slot.borrow_mut().take() {
        button.disconnect(handler);
    }
    if let Some(controller) = hover_controller_slot.borrow_mut().take() {
        button.remove_controller(&controller);
    }
    if let Some(current) = popover_slot.borrow_mut().replace(popover.clone()) {
        if current.is_visible() {
            current.popdown();
        }
        current.unparent();
    }
    popover.set_parent(button);
    let button_hovered = Rc::new(Cell::new(false));
    let popover_hovered = Rc::new(Cell::new(false));
    let row_popover = popover.clone();
    let click_button_hovered = Rc::clone(&button_hovered);
    let handler = button.connect_clicked(move |button| {
        click_button_hovered.set(true);
        popup_server_selection(button, &row_popover);
    });

    let popover_motion = gtk::EventControllerMotion::new();
    {
        let popover_hovered = Rc::clone(&popover_hovered);
        popover_motion.connect_enter(move |_, _, _| {
            popover_hovered.set(true);
        });
    }
    {
        let hover_popover = popover.clone();
        let button_hovered = Rc::clone(&button_hovered);
        let popover_hovered = Rc::clone(&popover_hovered);
        popover_motion.connect_leave(move |_| {
            popover_hovered.set(false);
            schedule_server_selection_popdown(&hover_popover, &button_hovered, &popover_hovered);
        });
    }
    popover.add_controller(popover_motion);

    let hover_popover = popover.clone();
    let hover_button = button.clone();
    let enter_button_hovered = Rc::clone(&button_hovered);
    let hover = gtk::EventControllerMotion::new();
    hover.connect_enter(move |_, _, _| {
        enter_button_hovered.set(true);
        popup_server_selection(&hover_button, &hover_popover);
    });
    let leave_popover = popover.clone();
    let leave_button_hovered = Rc::clone(&button_hovered);
    let leave_popover_hovered = Rc::clone(&popover_hovered);
    hover.connect_leave(move |_| {
        leave_button_hovered.set(false);
        schedule_server_selection_popdown(
            &leave_popover,
            &leave_button_hovered,
            &leave_popover_hovered,
        );
    });
    button.add_controller(hover.clone());
    let unmap_popover = popover.clone();
    let unmap_handler = button.connect_unmap(move |_| {
        unmap_popover.popdown();
    });
    *handler_slot.borrow_mut() = Some(handler);
    *hover_controller_slot.borrow_mut() = Some(hover);
    *unmap_handler_slot.borrow_mut() = Some(unmap_handler);
}

fn popup_server_selection(button: &gtk::Button, popover: &gtk::Popover) {
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        button.width(),
        SERVER_SELECTOR_POPOVER_ANCHOR_Y,
        1,
        1,
    )));
    popover.popup();
}

fn schedule_server_selection_popdown(
    popover: &gtk::Popover,
    button_hovered: &Rc<Cell<bool>>,
    popover_hovered: &Rc<Cell<bool>>,
) {
    let popover = popover.clone();
    let button_hovered = Rc::clone(button_hovered);
    let popover_hovered = Rc::clone(popover_hovered);
    glib::timeout_add_local_once(Duration::from_millis(120), move || {
        if !button_hovered.get() && !popover_hovered.get() {
            popover.popdown();
        }
    });
}

fn source_icon_name(content: &ServerSelectorContent) -> &'static str {
    match &content.selected_source {
        Some(LibrarySourceSelection::Local) => "rufin-route-folders-symbolic",
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
        "local" | "fake" => "rufin-route-folders-symbolic",
        _ => "network-server-symbolic",
    }
}

fn server_selection_popover(shell: &Rc<Shell>, content: &ServerSelectorContent) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_autohide(false);
    popover.set_position(gtk::PositionType::Right);
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 1);
    wrapper.add_css_class("server-selector-popover");
    wrapper.set_width_request(SERVER_SELECTOR_POPOVER_WIDTH);

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
            let row = server_option_row(Some(server), &title, "", active);
            if !active {
                let row_popover = popover.clone();
                let controller = shell.controller.clone();
                let server_id = server.id.clone();
                row.connect_clicked(move |_| {
                    popdown_server_selection_stack(&row_popover);
                    controller.select_source(LibrarySourceSelection::Server(server_id.clone()));
                });
            }
            wrapper.append(&row);
        }
    }

    if !content.local_folders.is_empty() {
        let local_active = matches!(content.selected_source, Some(LibrarySourceSelection::Local));
        let local = server_action_row(
            "rufin-route-folders-symbolic",
            &tr("Local"),
            &local_source_popup_detail(&content.local_folders),
            local_active,
        );
        if !local_active {
            let row_popover = popover.clone();
            let controller = shell.controller.clone();
            local.connect_clicked(move |_| {
                popdown_server_selection_stack(&row_popover);
                controller.select_source(LibrarySourceSelection::Local);
            });
        }
        wrapper.append(&local);
    }

    let manage = server_action_row("document-edit-symbolic", &tr("Manage"), "", false);
    let row_popover = popover.clone();
    let manage_shell = Rc::clone(shell);
    manage.connect_clicked(move |_| {
        popdown_server_selection_stack(&row_popover);
        manage_shell.present_library_preferences_dialog();
    });
    wrapper.append(&manage);

    let add_library = server_action_row("list-add-symbolic", &tr("Add music library"), "", false);
    add_library.add_css_class("server-add-option");
    let row_popover = popover.clone();
    let add_library_shell = Rc::clone(shell);
    add_library.connect_clicked(move |_| {
        popdown_server_selection_stack(&row_popover);
        add_library_shell.present_add_server_preferences_dialog();
    });
    wrapper.append(&add_library);

    if let Some(server) = &content.active_server
        && matches!(
            content.selected_source,
            Some(LibrarySourceSelection::Server(_))
        )
    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("server-library-separator");
        wrapper.append(&separator);
        wrapper.append(&server_section_label(&tr("Server Library")));
        append_server_music_folder_rows(shell, &popover, &wrapper, server, content);
    }

    popover.set_child(Some(&wrapper));
    popover
}

fn source_summary_detail(content: &ServerSelectorContent) -> String {
    match &content.selected_source {
        Some(LibrarySourceSelection::Local) => local_source_detail(&content.local_folders),
        Some(LibrarySourceSelection::Server(_)) => content
            .selected_music_folder_id
            .as_ref()
            .and_then(|selected| {
                content
                    .music_folders
                    .iter()
                    .find(|folder| folder.id == *selected)
            })
            .map(|folder| folder.name.clone())
            .unwrap_or_else(|| tr("All Music")),
        None => String::new(),
    }
}

fn local_source_detail(folders: &[LocalLibraryFolder]) -> String {
    match folders.len() {
        0 => tr("No local folders configured"),
        1 => folders[0].path.clone(),
        count => folder_count_text(count as u64),
    }
}

fn local_source_popup_detail(folders: &[LocalLibraryFolder]) -> String {
    folder_count_text(folders.len() as u64)
}

fn append_server_music_folder_rows(
    shell: &Rc<Shell>,
    popover: &gtk::Popover,
    wrapper: &gtk::Box,
    server: &ServerIdentity,
    content: &ServerSelectorContent,
) {
    let all_active = content.selected_music_folder_id.is_none();
    let all = server_action_row(
        "rufin-route-folders-symbolic",
        &tr("All Music"),
        "",
        all_active,
    );
    if !all_active {
        let row_popover = popover.clone();
        let controller = shell.controller.clone();
        let server_id = server.id.clone();
        all.connect_clicked(move |_| {
            popdown_server_selection_stack(&row_popover);
            controller.set_selected_music_folder(server_id.clone(), None);
        });
    }
    wrapper.append(&all);

    for folder in &content.music_folders {
        let active = content
            .selected_music_folder_id
            .as_ref()
            .is_some_and(|selected| *selected == folder.id);
        let row = server_action_row("folder-music-symbolic", &folder.name, "", active);
        if !active {
            let row_popover = popover.clone();
            let controller = shell.controller.clone();
            let server_id = server.id.clone();
            let folder_id = folder.id.clone();
            row.connect_clicked(move |_| {
                popdown_server_selection_stack(&row_popover);
                controller.set_selected_music_folder(server_id.clone(), Some(folder_id.clone()));
            });
        }
        wrapper.append(&row);
    }
}

fn popdown_server_selection_stack(popover: &gtk::Popover) {
    popover.popdown();
    let mut ancestor = popover.parent();
    while let Some(widget) = ancestor {
        ancestor = widget.parent();
        if let Ok(parent_popover) = widget.downcast::<gtk::Popover>() {
            parent_popover.popdown();
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

    let row_content = gtk::Box::new(
        gtk::Orientation::Horizontal,
        SERVER_OPTION_ICON_TEXT_SPACING,
    );
    row_content.set_halign(gtk::Align::Fill);
    let icon_name = server
        .map(server_icon_name)
        .unwrap_or("network-server-symbolic");
    row_content.append(&server_row_icon(icon_name));

    let name = server_row_label(title, detail);
    row_content.append(&name);
    if active {
        row_content.append(&server_row_check_icon());
    }
    row.set_child(Some(&row_content));
    row
}

fn server_action_row(icon_name: &str, title: &str, detail: &str, active: bool) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("server-option");

    let row_content = gtk::Box::new(
        gtk::Orientation::Horizontal,
        SERVER_OPTION_ICON_TEXT_SPACING,
    );
    row_content.set_halign(gtk::Align::Fill);
    row_content.append(&server_row_icon(icon_name));

    let name = server_row_label(title, detail);
    row_content.append(&name);
    if active {
        row_content.append(&server_row_check_icon());
    }
    row.set_child(Some(&row_content));
    row
}

fn server_row_label(title: &str, detail: &str) -> gtk::Label {
    let text = if detail.is_empty() {
        title.to_string()
    } else {
        format!("{title} · {detail}")
    };
    let name = gtk::Label::new(Some(&text));
    name.set_hexpand(true);
    name.set_xalign(0.0);
    name.set_yalign(0.5);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name
}

fn server_section_label(label: &str) -> gtk::Label {
    let section = gtk::Label::new(Some(label));
    section.add_css_class("server-section-label");
    section.set_xalign(0.0);
    section.set_margin_top(1);
    section.set_margin_start(3);
    section
}

fn server_row_icon(icon_name: &str) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(SERVER_OPTION_ICON_SIZE);
    icon.set_size_request(SERVER_OPTION_ICON_SIZE, SERVER_OPTION_ICON_SIZE);
    icon.set_valign(gtk::Align::Center);
    icon
}

fn server_row_check_icon() -> gtk::Image {
    let icon = gtk::Image::from_icon_name("object-select-symbolic");
    icon.set_pixel_size(SERVER_OPTION_CHECK_SIZE);
    icon.set_size_request(SERVER_OPTION_CHECK_SIZE, SERVER_OPTION_CHECK_SIZE);
    icon.set_valign(gtk::Align::Center);
    icon
}

fn configure_normal_selector_label(label: &gtk::Label) {
    label.add_css_class("sidebar-entry-label");
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Fill);
    label.set_valign(gtk::Align::Center);
    label.set_xalign(0.0);
    label.set_yalign(0.5);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_request(1);
    label.set_max_width_chars(NORMAL_SELECTOR_LABEL_WIDTH_CHARS);
}
