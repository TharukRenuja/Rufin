use adw::prelude::*;

use crate::i18n::tr;

use super::{
    configure_fill_width_clip,
    layout::{ROUTE_TOP_MARGIN, WINDOW_CHROME_MARGIN_END},
};

const WINDOW_CONTROLS_MARGIN_TOP: i32 = 10;
const WINDOW_DRAG_HANDLE_HEIGHT: i32 = ROUTE_TOP_MARGIN;
const WINDOW_DRAG_HANDLE_MARGIN_START: i32 = 56;

pub(super) struct MainAreaParts {
    pub(super) root: adw::ToolbarView,
    pub(super) route_host: gtk::Box,
}

pub(super) struct ContentChromeParts {
    pub(super) root: gtk::Overlay,
    pub(super) right_panel_slot: gtk::ScrolledWindow,
}

pub(super) fn build_main_area() -> MainAreaParts {
    let root = adw::ToolbarView::new();
    root.add_css_class("main-area");
    root.set_hexpand(true);
    root.set_vexpand(true);

    let route_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    route_host.set_hexpand(true);
    route_host.set_vexpand(true);

    root.set_content(Some(&route_host));

    MainAreaParts { root, route_host }
}

pub(super) fn build_content_chrome(
    main_area: &adw::ToolbarView,
    right_panel: &gtk::Box,
) -> ContentChromeParts {
    let main_well = gtk::Overlay::new();
    main_well.set_overflow(gtk::Overflow::Hidden);
    main_well.set_width_request(1);
    main_well.set_hexpand(true);
    main_well.set_vexpand(true);
    let main_measure_floor = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_measure_floor.set_width_request(1);
    main_measure_floor.set_hexpand(true);
    main_measure_floor.set_vexpand(true);
    main_well.set_child(Some(&main_measure_floor));
    main_area.set_halign(gtk::Align::Fill);
    main_area.set_valign(gtk::Align::Fill);
    main_well.add_overlay(main_area);
    main_well.set_measure_overlay(main_area, false);
    let drag_handle = window_drag_handle_with_margins(
        "window-drag-handle",
        WINDOW_DRAG_HANDLE_HEIGHT,
        WINDOW_DRAG_HANDLE_MARGIN_START,
        0,
    );
    main_well.add_overlay(&drag_handle);
    main_well.set_measure_overlay(&drag_handle, false);

    let right_panel_slot = gtk::ScrolledWindow::new();
    configure_fill_width_clip(&right_panel_slot, gtk::PolicyType::Never);
    right_panel_slot.set_propagate_natural_height(false);
    right_panel_slot.set_hexpand(false);
    right_panel_slot.set_vexpand(true);
    right_panel_slot.set_child(Some(right_panel));

    let content_body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content_body.set_hexpand(true);
    content_body.set_vexpand(true);
    content_body.append(&main_well);
    content_body.append(&right_panel_slot);

    let root = gtk::Overlay::new();
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_child(Some(&content_body));

    let window_controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    window_controls.add_css_class("window-controls");
    window_controls.set_halign(gtk::Align::End);
    window_controls.set_valign(gtk::Align::Start);
    window_controls.set_margin_top(WINDOW_CONTROLS_MARGIN_TOP);
    window_controls.set_margin_end(WINDOW_CHROME_MARGIN_END);

    let close_button = gtk::WindowControls::new(gtk::PackType::End);
    close_button.set_decoration_layout(Some(":close"));
    window_controls.append(&close_button);
    root.add_overlay(&window_controls);
    root.set_measure_overlay(&window_controls, false);

    ContentChromeParts {
        root,
        right_panel_slot,
    }
}

pub(super) fn window_close_controls() -> gtk::Box {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.add_css_class("window-controls");
    controls.set_halign(gtk::Align::End);
    controls.set_valign(gtk::Align::Start);
    controls.set_margin_top(WINDOW_CONTROLS_MARGIN_TOP);
    controls.set_margin_end(WINDOW_CHROME_MARGIN_END);

    let close_button = gtk::WindowControls::new(gtk::PackType::End);
    close_button.set_decoration_layout(Some(":close"));
    controls.append(&close_button);
    controls
}

pub(super) fn configure_primary_menu_button(button: &gtk::Button) {
    let label = tr("Menu");
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);
}

pub(in crate::ui) fn window_drag_handle_with_child(
    css_class: &str,
    child: &impl IsA<gtk::Widget>,
) -> gtk::WindowHandle {
    let handle = gtk::WindowHandle::new();
    handle.add_css_class(css_class);
    handle.set_halign(gtk::Align::Fill);
    handle.set_valign(gtk::Align::Start);
    handle.set_hexpand(true);
    handle.set_vexpand(false);
    handle.set_child(Some(child));
    handle
}

fn window_drag_handle_with_margins(
    css_class: &str,
    height: i32,
    margin_start: i32,
    margin_end: i32,
) -> gtk::WindowHandle {
    let handle =
        window_drag_handle_with_child(css_class, &gtk::Box::new(gtk::Orientation::Horizontal, 0));
    handle.set_margin_start(margin_start);
    handle.set_margin_end(margin_end);
    handle.set_height_request(height);

    if let Some(area) = handle.child() {
        area.set_hexpand(true);
        area.set_height_request(height);
    }
    handle
}
