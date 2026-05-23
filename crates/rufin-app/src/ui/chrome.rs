use adw::prelude::*;
use gtk::gio;

use crate::i18n::tr;

const MAIN_MENU_MARGIN_END: i32 = 12;
const MAIN_MENU_MARGIN_TOP: i32 = 9;

pub(super) struct MainAreaParts {
    pub(super) root: adw::ToolbarView,
    pub(super) route_title: adw::WindowTitle,
    pub(super) route_host: gtk::Box,
}

pub(super) struct ContentChromeParts {
    pub(super) root: gtk::Overlay,
    pub(super) main_menu: gtk::MenuButton,
    pub(super) right_panel_slot: gtk::ScrolledWindow,
}

pub(super) fn build_main_area() -> MainAreaParts {
    let root = adw::ToolbarView::new();
    root.add_css_class("main-area");
    root.set_hexpand(true);
    root.set_vexpand(true);

    let header = adw::HeaderBar::new();
    header.add_css_class("route-header");
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);

    let route_title = adw::WindowTitle::new("", "");
    route_title.set_valign(gtk::Align::Center);
    header.set_title_widget(Some(&route_title));

    let route_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    route_host.set_hexpand(true);
    route_host.set_vexpand(true);

    root.add_top_bar(&header);
    root.set_content(Some(&route_host));

    MainAreaParts {
        root,
        route_title,
        route_host,
    }
}

pub(super) fn build_content_chrome(
    main_area: &adw::ToolbarView,
    right_panel: &gtk::Box,
) -> ContentChromeParts {
    let main_well = gtk::ScrolledWindow::new();
    // automatic horizontal policy gives the main pane a clip/scroll boundary
    // without letting wide route children become a large toplevel minimum.
    main_well.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    main_well.set_overflow(gtk::Overflow::Hidden);
    main_well.set_min_content_width(0);
    main_well.set_propagate_natural_width(false);
    main_well.set_propagate_natural_height(false);
    main_well.set_hexpand(true);
    main_well.set_vexpand(true);
    main_well.set_child(Some(main_area));

    let right_panel_slot = gtk::ScrolledWindow::new();
    right_panel_slot.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
    right_panel_slot.set_overflow(gtk::Overflow::Hidden);
    right_panel_slot.set_propagate_natural_width(true);
    right_panel_slot.set_propagate_natural_height(true);
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

    let main_menu = primary_menu_button();
    main_menu.set_halign(gtk::Align::End);
    main_menu.set_valign(gtk::Align::Start);
    main_menu.set_margin_top(MAIN_MENU_MARGIN_TOP);
    main_menu.set_margin_end(MAIN_MENU_MARGIN_END);
    root.add_overlay(&main_menu);
    root.set_measure_overlay(&main_menu, false);

    ContentChromeParts {
        root,
        main_menu,
        right_panel_slot,
    }
}

fn primary_menu_button() -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_icon_name("open-menu-symbolic");
    button.set_primary(true);
    let label = tr("Main Menu");
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);
    button.set_menu_model(Some(&primary_menu_model()));
    button
}

fn primary_menu_model() -> gio::Menu {
    let menu = gio::Menu::new();
    let view = gio::Menu::new();
    view.append(
        Some(&tr("Toggle Fullscreen")),
        Some("win.toggle-fullscreen"),
    );
    menu.append_section(None, &view);

    let preferences = gio::Menu::new();
    preferences.append(Some(&tr("Preferences")), Some("win.preferences"));
    preferences.append(Some(&tr("Keyboard Shortcuts")), Some("win.show-shortcuts"));
    menu.append_section(None, &preferences);

    let about = gio::Menu::new();
    about.append(Some(&tr("About Rufin")), Some("win.about"));
    menu.append_section(None, &about);
    menu
}
