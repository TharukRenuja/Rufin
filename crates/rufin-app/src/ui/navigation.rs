use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{Route, SearchKind, ServerIdentity};

use super::{Shell, icon_button};
use crate::controller::LibrarySnapshot;
use crate::i18n::tr;

pub(super) struct ServerSelector {
    pub normal_button: gtk::MenuButton,
    pub normal_name: gtk::Label,
    pub normal_subtitle: gtk::Label,
    pub compact_button: gtk::MenuButton,
    pub compact_label: gtk::Label,
}

struct ServerSelectorContent {
    name: String,
    subtitle: String,
    detail: String,
    has_server: bool,
}

pub(super) fn build_server_selector() -> ServerSelector {
    let normal_button = gtk::MenuButton::new();
    normal_button.add_css_class("server-selector");
    normal_button.add_css_class("server-card");
    normal_button.set_margin_start(12);
    normal_button.set_margin_end(12);
    normal_button.set_margin_bottom(12);

    let normal_content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    normal_content.set_halign(gtk::Align::Fill);
    normal_content.append(&gtk::Image::from_icon_name("network-server-symbolic"));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let normal_name = gtk::Label::new(None);
    normal_name.set_xalign(0.0);
    normal_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let normal_subtitle = gtk::Label::new(None);
    normal_subtitle.add_css_class("muted");
    normal_subtitle.set_xalign(0.0);
    normal_subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&normal_name);
    labels.append(&normal_subtitle);
    normal_content.append(&labels);
    normal_content.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
    normal_button.set_child(Some(&normal_content));

    let compact_button = gtk::MenuButton::new();
    compact_button.add_css_class("nav-button");
    compact_button.add_css_class("flat");
    compact_button.add_css_class("rail-button");
    compact_button.add_css_class("server-selector");
    let compact_content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    compact_content.set_halign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name("network-server-symbolic");
    icon.set_pixel_size(24);
    compact_content.append(&icon);
    let compact_label = gtk::Label::new(None);
    configure_rail_label(&compact_label);
    compact_content.append(&compact_label);
    compact_button.set_child(Some(&compact_content));

    ServerSelector {
        normal_button,
        normal_name,
        normal_subtitle,
        compact_button,
        compact_label,
    }
}

pub(super) fn update_server_selector(selector: &ServerSelector, library: &LibrarySnapshot) {
    let content = server_selector_content(library);
    let tooltip = format!("{}: {}", tr("Server"), content.name);

    selector.normal_name.set_text(&content.name);
    selector.normal_subtitle.set_text(&content.subtitle);
    selector.normal_button.set_tooltip_text(Some(&tooltip));
    selector
        .normal_button
        .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
    selector
        .normal_button
        .set_popover(Some(&server_selection_popover(&content)));

    selector
        .compact_label
        .set_text(&compact_sidebar_label_text(&content.name));
    selector.compact_button.set_tooltip_text(Some(&tooltip));
    selector
        .compact_button
        .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
    selector
        .compact_button
        .set_popover(Some(&server_selection_popover(&content)));
}

pub(super) fn sidebar_history_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = icon_button(icon_name, label);
    button.add_css_class("sidebar-history-button");
    button
}

pub(super) fn build_normal_navigation(shell: &Rc<Shell>) {
    shell.normal_nav.append(&normal_history_controls(shell));

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Search")));
    search.set_margin_top(8);
    search.set_margin_start(16);
    search.set_margin_end(16);
    let search_shell = Rc::clone(shell);
    search.connect_activate(move |entry| {
        let query = entry.text().trim().to_string();
        if query.is_empty() {
            return;
        }
        search_shell.controller.search(query.clone());
        search_shell.navigate(Route::Search {
            query,
            kind: SearchKind::All,
        });
    });
    shell.normal_nav.append(&search);

    let heading = gtk::Label::new(Some(&tr("My Library")));
    heading.add_css_class("nav-heading");
    heading.set_xalign(0.0);
    heading.set_margin_start(18);
    heading.set_margin_top(18);
    shell.normal_nav.append(&heading);

    for item in nav_items() {
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

    shell
        .normal_nav
        .append(&shell.server_selector.normal_button);
}

pub(super) fn build_compact_navigation(shell: &Rc<Shell>) {
    shell.compact_nav.append(&compact_history_controls(shell));

    for item in nav_items() {
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
    shell
        .compact_nav
        .append(&shell.server_selector.compact_button);
}

fn server_selector_content(library: &LibrarySnapshot) -> ServerSelectorContent {
    let Some(server) = library.server.as_ref() else {
        return ServerSelectorContent {
            name: tr("No server"),
            subtitle: tr("No server"),
            detail: tr("No server"),
            has_server: false,
        };
    };

    let name = server_display_name(server);
    let subtitle = tr("Current server");
    let detail = if server.base_url.trim().is_empty() {
        provider_display_name(&server.provider)
    } else {
        server.base_url.clone()
    };

    ServerSelectorContent {
        name,
        subtitle,
        detail,
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
        "jellyfin" => "Jellyfin".to_string(),
        "fake" => tr("Local"),
        provider => provider.to_string(),
    }
}

fn server_selection_popover(content: &ServerSelectorContent) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrapper.add_css_class("server-selector-popover");

    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("server-option");
    row.set_sensitive(content.has_server);

    let row_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row_content.set_halign(gtk::Align::Fill);
    row_content.append(&gtk::Image::from_icon_name("object-select-symbolic"));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(&content.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let detail = gtk::Label::new(Some(&content.detail));
    detail.add_css_class("muted");
    detail.set_xalign(0.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&name);
    labels.append(&detail);
    row_content.append(&labels);
    row.set_child(Some(&row_content));

    let row_popover = popover.clone();
    row.connect_clicked(move |_| row_popover.popdown());

    wrapper.append(&row);
    popover.set_child(Some(&wrapper));
    popover
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

fn nav_items() -> Vec<NavItem> {
    vec![
        NavItem {
            icon_name: "go-home-symbolic",
            label: "Home",
            route: Route::Home,
        },
        NavItem {
            icon_name: "starred-symbolic",
            label: "Favorites",
            route: Route::Favorites,
        },
        NavItem {
            icon_name: "media-optical-symbolic",
            label: "Albums",
            route: Route::Albums,
        },
        NavItem {
            icon_name: "audio-x-generic-symbolic",
            label: "Tracks",
            route: Route::Tracks,
        },
        NavItem {
            icon_name: "avatar-default-symbolic",
            label: "Album Artists",
            route: Route::AlbumArtists,
        },
        NavItem {
            icon_name: "system-users-symbolic",
            label: "Artists",
            route: Route::Artists,
        },
        NavItem {
            icon_name: "flag-symbolic",
            label: "Genres",
            route: Route::Genres,
        },
        NavItem {
            icon_name: "media-playlist-consecutive-symbolic",
            label: "Playlists",
            route: Route::Playlists,
        },
    ]
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
    content.set_halign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(icon_name);
    content.append(&icon);
    if compact {
        icon.set_pixel_size(24);
        let text = gtk::Label::new(Some(&compact_sidebar_label_text(label)));
        configure_rail_label(&text);
        content.append(&text);
    } else {
        let text = gtk::Label::new(Some(&tr(label)));
        text.set_xalign(0.0);
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
    label.add_css_class("rail-label");
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_lines(2);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
}

fn rail_button(shell: &Rc<Shell>, icon_name: &str, label: &str, route: Route) -> gtk::Button {
    nav_button(shell, icon_name, label, route, true)
}
