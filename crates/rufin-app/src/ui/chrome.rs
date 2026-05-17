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
    pub(super) content_split: gtk::Paned,
    pub(super) main_menu: gtk::MenuButton,
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
    let content_split = gtk::Paned::new(gtk::Orientation::Horizontal);
    content_split.set_hexpand(true);
    content_split.set_vexpand(true);
    content_split.set_wide_handle(false);
    content_split.set_resize_start_child(true);
    content_split.set_resize_end_child(true);
    content_split.set_shrink_start_child(true);
    content_split.set_shrink_end_child(true);
    content_split.set_start_child(Some(main_area));
    content_split.set_end_child(Some(right_panel));

    let root = gtk::Overlay::new();
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_child(Some(&content_split));

    let main_menu = primary_menu_button();
    main_menu.set_halign(gtk::Align::End);
    main_menu.set_valign(gtk::Align::Start);
    main_menu.set_margin_top(MAIN_MENU_MARGIN_TOP);
    main_menu.set_margin_end(MAIN_MENU_MARGIN_END);
    root.add_overlay(&main_menu);
    root.set_measure_overlay(&main_menu, false);

    ContentChromeParts {
        root,
        content_split,
        main_menu,
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
