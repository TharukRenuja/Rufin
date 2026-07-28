use adw::prelude::*;

use crate::layout::configure_fill_width_clip;
use localization::tr;

use super::layout::WINDOW_CHROME_MARGIN_END;

const WINDOW_CONTROLS_MARGIN_TOP: i32 = 10;
const WINDOW_DRAG_HANDLE_HEIGHT: i32 = 10;
const WINDOW_DRAG_HANDLE_MARGIN_START: i32 = 56;
pub(super) const RIGHT_RESIZE_HANDLE_WIDTH: i32 = 4;
pub(crate) const ROUTE_VIEWPORT_CLASS: &str = "route-viewport";

pub(crate) struct WindowChrome {
    pub(crate) application: adw::Application,
    pub(crate) window: adw::ApplicationWindow,
    pub(crate) toast_overlay: adw::ToastOverlay,
    pub(crate) quick_toast_overlay: adw::ToastOverlay,
    pub(super) control_feedback_label: gtk::Label,
    pub(super) root_stack: gtk::Stack,
    pub(crate) app_root_overlay: gtk::Overlay,
    pub(crate) app_content_stack: gtk::Stack,
    pub(super) login_host: gtk::Box,
    pub(super) startup_loading_host: gtk::Box,
}

pub(super) struct MainAreaParts {
    pub(super) root: adw::ToolbarView,
    pub(super) route_host: gtk::Stack,
}

pub(super) struct ContentChromeParts {
    pub(super) root: gtk::Overlay,
    pub(super) right_split: gtk::Paned,
    pub(super) right_panel_slot: gtk::ScrolledWindow,
    pub(super) right_resize_handle: gtk::Box,
}

pub(super) fn build_main_area() -> MainAreaParts {
    let root = adw::ToolbarView::new();
    root.add_css_class("main-area");
    root.set_hexpand(true);
    root.set_vexpand(true);

    let route_host = gtk::Stack::new();
    route_host.add_css_class(ROUTE_VIEWPORT_CLASS);
    route_host.set_hhomogeneous(false);
    route_host.set_vhomogeneous(false);
    route_host.set_interpolate_size(false);
    route_host.set_transition_type(gtk::StackTransitionType::None);
    route_host.set_transition_duration(0);
    route_host.set_width_request(1);
    route_host.set_halign(gtk::Align::Fill);
    route_host.set_hexpand(true);
    route_host.set_vexpand(true);
    route_host.set_overflow(gtk::Overflow::Hidden);

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
    main_area.set_width_request(1);
    main_area.set_halign(gtk::Align::Fill);
    main_area.set_valign(gtk::Align::Fill);
    main_area.set_overflow(gtk::Overflow::Hidden);
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

    let right_split = gtk::Paned::new(gtk::Orientation::Horizontal);
    configure_right_split(&right_split);
    right_split.set_start_child(Some(&main_well));
    right_split.set_end_child(Some(&right_panel_slot));
    right_split.set_hexpand(true);
    right_split.set_vexpand(true);

    let root = gtk::Overlay::new();
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_child(Some(&right_split));

    let right_resize_handle = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right_resize_handle.add_css_class("right-sidebar-resize-handle");
    right_resize_handle.set_width_request(RIGHT_RESIZE_HANDLE_WIDTH);
    right_resize_handle.set_halign(gtk::Align::Start);
    right_resize_handle.set_valign(gtk::Align::Fill);
    right_resize_handle.set_vexpand(true);
    right_resize_handle.set_focusable(false);
    right_resize_handle.set_cursor_from_name(Some("col-resize"));
    let resize_label = tr("Hold and drag to resize");
    right_resize_handle.update_property(&[gtk::accessible::Property::Label(&resize_label)]);
    right_resize_handle.set_visible(false);
    root.add_overlay(&right_resize_handle);
    root.set_measure_overlay(&right_resize_handle, false);

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
        right_split,
        right_panel_slot,
        right_resize_handle,
    }
}

fn configure_right_split(right_split: &gtk::Paned) {
    right_split.add_css_class("right-pane-split");
    right_split.set_focusable(false);
    // Rufin owns the four-pixel input target. A non-wide GtkPaned adds a
    // hidden six-pixel pointer gutter on both sides of its separator, which
    // would leave a second resize owner underneath the shell gesture.
    right_split.set_wide_handle(true);
    // The shell sets an exact divider position for the allocation it is about
    // to grant. Preserve that start position when GtkPaned receives its new
    // width instead of applying GtkPaned's own parent-resize distribution to
    // the already-resolved position.
    right_split.set_resize_start_child(false);
    right_split.set_shrink_start_child(true);
    right_split.set_resize_end_child(true);
    right_split.set_shrink_end_child(false);
}

pub(crate) fn window_close_controls() -> gtk::Box {
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

pub(crate) fn window_drag_handle_with_child(
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
