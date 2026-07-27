use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use ::library::{MusicFolder, MusicFolderId, SourceId};
use adw::prelude::*;
use gtk::glib;

use crate::preferences::{
    present_add_server_preferences_dialog, present_library_preferences_dialog,
};
use crate::runtime::SelectedLibrary;
use crate::runtime::source::{ConfiguredSources, LocalFolder, SourceSummary};
use crate::shell::Shell;
use localization::tr;

use super::{configured_source_display_name, configured_source_icon_name, folder_count_text};

const NORMAL_SELECTOR_ICON_SIZE: i32 = 22;
const NORMAL_SELECTOR_LABEL_WIDTH_CHARS: i32 = 18;
const SERVER_OPTION_ICON_TEXT_SPACING: i32 = 10;
const SERVER_OPTION_ICON_SIZE: i32 = 14;
const SERVER_OPTION_CHECK_SIZE: i32 = 13;
const SERVER_SELECTOR_POPOVER_WIDTH: i32 = 236;
const SERVER_SELECTOR_POPOVER_ANCHOR_Y: i32 = 148;

pub(crate) struct SourceSelector {
    pub(crate) normal_button: gtk::Button,
    pub(crate) normal_icon: gtk::Image,
    pub(crate) normal_name: gtk::Label,
    pub(crate) normal_subtitle: gtk::Label,
    normal_popover: RefCell<Option<gtk::Popover>>,
    normal_click_handler: RefCell<Option<glib::SignalHandlerId>>,
    normal_hover_controller: RefCell<Option<gtk::EventControllerMotion>>,
    normal_unmap_handler: RefCell<Option<glib::SignalHandlerId>>,
    pub(crate) compact_button: gtk::Button,
    pub(crate) compact_icon: gtk::Image,
    pub(crate) compact_name: gtk::Label,
    pub(crate) compact_subtitle: gtk::Label,
    compact_popover: RefCell<Option<gtk::Popover>>,
    compact_click_handler: RefCell<Option<glib::SignalHandlerId>>,
    compact_hover_controller: RefCell<Option<gtk::EventControllerMotion>>,
    compact_unmap_handler: RefCell<Option<glib::SignalHandlerId>>,
}

struct SourceSelectorContent {
    name: String,
    selected_source_id: Option<SourceId>,
    active_source: Option<SourceSummary>,
    sources: Arc<[SourceSummary]>,
    local_folders: Arc<[LocalFolder]>,
    music_folders: Arc<[Arc<MusicFolder>]>,
    selected_music_folder_id: Option<MusicFolderId>,
}

pub(crate) fn build_source_selector() -> SourceSelector {
    let normal_button = gtk::Button::new();
    normal_button.add_css_class("source-selector");
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
    compact_button.add_css_class("source-selector");
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

    SourceSelector {
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

pub(crate) fn update_source_selector(shell: &Rc<Shell>) {
    let selector = &shell.navigation_view.server_selector;
    let configured = shell.source.configured.borrow();
    let selected = shell.library.selected.borrow();
    let content = source_selector_content(&configured, selected.as_ref());
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
        source_selection_popover(shell, &content),
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
        source_selection_popover(shell, &content),
    );
}

fn source_selector_content(
    configured: &ConfiguredSources,
    selected: Option<&SelectedLibrary>,
) -> SourceSelectorContent {
    let selected_source_id = configured.selected_source_id.clone();
    let active_source = selected_source_id.as_ref().and_then(|selected| {
        configured
            .sources
            .iter()
            .find(|source| &source.id == selected)
            .cloned()
    });
    let Some(server) = active_source else {
        return SourceSelectorContent {
            name: tr("No source"),
            selected_source_id,
            active_source: None,
            sources: Arc::clone(&configured.sources),
            local_folders: Arc::clone(&configured.local_folders),
            music_folders: Arc::from([]),
            selected_music_folder_id: None,
        };
    };

    let music_folders = selected
        .filter(|selected| selected.source_id == server.id)
        .and_then(|selected| selected.loaded.music_folders().ok())
        .unwrap_or_else(|| Arc::from([]));
    let selected_music_folder_id = if music_folders.is_empty() {
        None
    } else {
        selected.and_then(|selected| selected.music_folder_id.clone())
    };
    let name = configured_source_display_name(&server);
    SourceSelectorContent {
        name,
        selected_source_id,
        active_source: Some(server),
        sources: Arc::clone(&configured.sources),
        local_folders: Arc::clone(&configured.local_folders),
        music_folders,
        selected_music_folder_id,
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
        let hover_popover = popover.downgrade();
        let button_hovered = Rc::clone(&button_hovered);
        let popover_hovered = Rc::clone(&popover_hovered);
        popover_motion.connect_leave(move |_| {
            popover_hovered.set(false);
            if let Some(popover) = hover_popover.upgrade() {
                schedule_server_selection_popdown(&popover, &button_hovered, &popover_hovered);
            }
        });
    }
    popover.add_controller(popover_motion);

    let hover_popover = popover.downgrade();
    let hover_button = button.downgrade();
    let enter_button_hovered = Rc::clone(&button_hovered);
    let hover = gtk::EventControllerMotion::new();
    hover.connect_enter(move |_, _, _| {
        enter_button_hovered.set(true);
        if let (Some(button), Some(popover)) = (hover_button.upgrade(), hover_popover.upgrade()) {
            popup_server_selection(&button, &popover);
        }
    });
    let leave_popover = popover.downgrade();
    let leave_button_hovered = Rc::clone(&button_hovered);
    let leave_popover_hovered = Rc::clone(&popover_hovered);
    hover.connect_leave(move |_| {
        leave_button_hovered.set(false);
        if let Some(popover) = leave_popover.upgrade() {
            schedule_server_selection_popdown(
                &popover,
                &leave_button_hovered,
                &leave_popover_hovered,
            );
        }
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

fn source_icon_name(content: &SourceSelectorContent) -> &'static str {
    match &content.selected_source_id {
        Some(_) => content
            .active_source
            .as_ref()
            .map(configured_source_icon_name)
            .unwrap_or("network-server-symbolic"),
        None => "network-server-symbolic",
    }
}

fn source_selection_popover(shell: &Rc<Shell>, content: &SourceSelectorContent) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_autohide(false);
    popover.set_position(gtk::PositionType::Right);
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 1);
    wrapper.add_css_class("source-selector-popover");
    wrapper.set_width_request(SERVER_SELECTOR_POPOVER_WIDTH);

    wrapper.append(&server_section_label(&tr("Select Source")));
    if content.sources.is_empty() && content.local_folders.is_empty() {
        let row = source_option_row(None, &tr("No sources configured"), "", false);
        row.set_sensitive(false);
        wrapper.append(&row);
    } else {
        for index in source_order(&content.sources, content.selected_source_id.as_ref()) {
            let server = &content.sources[index];
            let active = content.selected_source_id.as_ref() == Some(&server.id);
            let title = configured_source_display_name(server);
            let detail = is_local_source(server)
                .then(|| local_source_popup_detail(&content.local_folders))
                .unwrap_or_default();
            let row = source_option_row(Some(server), &title, &detail, active);
            if !active {
                let row_popover = popover.downgrade();
                let source = shell.products.source.clone();
                let source_id = server.id.clone();
                row.connect_clicked(move |_| {
                    if let Some(popover) = row_popover.upgrade() {
                        popdown_server_selection_stack(&popover);
                    }
                    source.select_source(source_id.clone());
                });
            }
            wrapper.append(&row);
        }
    }

    let manage = server_action_row("document-edit-symbolic", &tr("Manage"), "", false);
    let row_popover = popover.downgrade();
    let manage_shell = Rc::clone(shell);
    manage.connect_clicked(move |_| {
        if let Some(popover) = row_popover.upgrade() {
            popdown_server_selection_stack(&popover);
        }
        present_library_preferences_dialog(&manage_shell);
    });
    wrapper.append(&manage);

    let add_library = server_action_row("list-add-symbolic", &tr("Add music library"), "", false);
    add_library.add_css_class("server-add-option");
    let row_popover = popover.downgrade();
    let add_library_shell = Rc::clone(shell);
    add_library.connect_clicked(move |_| {
        if let Some(popover) = row_popover.upgrade() {
            popdown_server_selection_stack(&popover);
        }
        present_add_server_preferences_dialog(&add_library_shell);
    });
    wrapper.append(&add_library);

    if let Some(server) = &content.active_source
        && !content.music_folders.is_empty()
    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("server-library-separator");
        wrapper.append(&separator);
        wrapper.append(&server_section_label(&tr("Server Library")));
        append_source_music_folder_rows(shell, &popover, &wrapper, server, content);
    }

    popover.set_child(Some(&wrapper));
    popover
}

fn source_order(sources: &[SourceSummary], selected: Option<&SourceId>) -> Vec<usize> {
    let mut order = Vec::with_capacity(sources.len());
    if let Some(selected) = selected
        && let Some(index) = sources.iter().position(|source| &source.id == selected)
    {
        order.push(index);
    }
    order.extend(
        sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| (Some(&source.id) != selected).then_some(index)),
    );
    order
}

fn source_summary_detail(content: &SourceSelectorContent) -> String {
    match &content.active_source {
        Some(source) if is_local_source(source) => local_source_detail(&content.local_folders),
        Some(_) => content
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

fn local_source_detail(folders: &[LocalFolder]) -> String {
    match folders.len() {
        0 => tr("No local folders configured"),
        1 => folders[0].path.clone(),
        count => folder_count_text(count as u64),
    }
}

fn local_source_popup_detail(folders: &[LocalFolder]) -> String {
    folder_count_text(folders.len() as u64)
}

fn append_source_music_folder_rows(
    shell: &Rc<Shell>,
    popover: &gtk::Popover,
    wrapper: &gtk::Box,
    server: &SourceSummary,
    content: &SourceSelectorContent,
) {
    let all_active = content.selected_music_folder_id.is_none();
    let all = server_action_row(
        "rufin-route-folders-symbolic",
        &tr("All Music"),
        "",
        all_active,
    );
    if !all_active {
        let row_popover = popover.downgrade();
        let source = shell.products.source.clone();
        let source_id = server.id.clone();
        all.connect_clicked(move |_| {
            if let Some(popover) = row_popover.upgrade() {
                popdown_server_selection_stack(&popover);
            }
            source.set_music_folder(source_id.clone(), None);
        });
    }
    wrapper.append(&all);

    for folder in content.music_folders.iter() {
        let active = content
            .selected_music_folder_id
            .as_ref()
            .is_some_and(|selected| *selected == folder.id);
        let row = server_action_row("folder-music-symbolic", &folder.name, "", active);
        if !active {
            let row_popover = popover.downgrade();
            let source = shell.products.source.clone();
            let source_id = server.id.clone();
            let folder_id = folder.id.clone();
            row.connect_clicked(move |_| {
                if let Some(popover) = row_popover.upgrade() {
                    popdown_server_selection_stack(&popover);
                }
                source.set_music_folder(source_id.clone(), Some(folder_id.clone()));
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

fn source_option_row(
    server: Option<&SourceSummary>,
    title: &str,
    detail: &str,
    active: bool,
) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("source-option");

    let row_content = gtk::Box::new(
        gtk::Orientation::Horizontal,
        SERVER_OPTION_ICON_TEXT_SPACING,
    );
    row_content.set_halign(gtk::Align::Fill);
    let icon_name = server
        .map(configured_source_icon_name)
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

fn is_local_source(source: &SourceSummary) -> bool {
    source.kind == "local"
}

fn server_action_row(icon_name: &str, title: &str, detail: &str, active: bool) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("source-option");

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

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str) -> SourceSummary {
        SourceSummary {
            id: SourceId::new(id),
            kind: "test".to_string(),
            name: id.to_string(),
        }
    }

    #[test]
    fn selected_source_is_first_without_reordering_the_others() {
        let sources = vec![source("first"), source("second"), source("selected")];
        let selected = sources[2].id.clone();
        assert_eq!(source_order(&sources, Some(&selected)), [2, 0, 1]);
        assert_eq!(source_order(&sources, None), [0, 1, 2]);
    }
}
