use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use sources::SourceLocalAccessPresentation;
use sources::{LibrarySourceSelection, SourceIdentity};

use localization::tr;

use super::{
    PreferencesNavigationControls,
    layout::button_row,
    source::{
        configured_source_display_name, configured_source_icon_name,
        configured_source_kind_display_name,
    },
};
use crate::shell::Shell;
use localization::{album_count_text, track_count_text};

const SERVER_PROVIDER_ICON_SIZE: i32 = 28;

pub(super) fn library_page(
    shell: &Rc<Shell>,
    dialog: &adw::Dialog,
    navigation_controls: &PreferencesNavigationControls,
    open_add_server: bool,
) -> gtk::Widget {
    let navigation = adw::NavigationView::new();
    navigation_controls.set_navigation(&navigation);
    navigation_controls.set_nested_page_visible(false);
    let page = library_sources_page(shell, dialog, &navigation, navigation_controls);
    let root = adw::NavigationPage::new(&page, &tr("Library"));
    navigation.push(&root);
    if open_add_server {
        let page = shell.add_server_navigation_page(&navigation, dialog);
        navigation.push(&page);
        navigation_controls.set_nested_page_visible(true);
    }
    navigation.upcast::<gtk::Widget>()
}

fn library_sources_page(
    shell: &Rc<Shell>,
    dialog: &adw::Dialog,
    navigation: &adw::NavigationView,
    navigation_controls: &PreferencesNavigationControls,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Library"))
        .icon_name("rufin-route-tracks-symbolic")
        .build();

    let library = shell.source.presentation.borrow().clone();

    let servers_group = adw::PreferencesGroup::builder()
        .title(tr("Servers"))
        .description(tr(
            "Configure saved music sources and local playback mappings.",
        ))
        .build();

    if library.sources.is_empty() {
        let row = adw::ActionRow::builder()
            .title(tr("No remote sources configured"))
            .subtitle(tr(
                "Add a server to use Jellyfin, Navidrome, or OpenSubsonic.",
            ))
            .build();
        servers_group.add(&row);
    } else {
        for server in &library.sources {
            let selected = matches!(
                &library.selected_source,
                Some(LibrarySourceSelection::Source(source_id))
                    if source_id.as_str() == server.id.as_str()
            );
            let summary = library
                .source_local_access
                .iter()
                .find(|summary| summary.source_id == server.id);
            let account = shell
                .products
                .source
                .configured_source(&server.id)
                .ok()
                .flatten()
                .map(|saved| saved.credentials.username);
            let row = adw::ActionRow::builder()
                .title(configured_source_display_name(server))
                .subtitle(source_summary_subtitle(server, summary, account.as_deref()))
                .subtitle_lines(4)
                .build();
            let icon = gtk::Image::from_icon_name(configured_source_icon_name(server));
            icon.set_pixel_size(SERVER_PROVIDER_ICON_SIZE);
            icon.set_size_request(SERVER_PROVIDER_ICON_SIZE, SERVER_PROVIDER_ICON_SIZE);
            icon.set_valign(gtk::Align::Center);
            row.add_prefix(&icon);
            if selected {
                row.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
            }
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            row.set_activatable(true);
            let settings_shell = Rc::clone(shell);
            let navigation = navigation.clone();
            let navigation_controls = navigation_controls.clone();
            let dialog = dialog.clone();
            let server = server.clone();
            row.connect_activated(move |_| {
                let navigation_controls_for_close = navigation_controls.clone();
                let on_close: Rc<dyn Fn()> = Rc::new(move || {
                    navigation_controls_for_close.set_nested_page_visible(false);
                });
                let page = crate::preferences::source::local_access::manage_server_navigation_page(
                    &settings_shell,
                    server.clone(),
                    &navigation,
                    &dialog,
                    on_close,
                );
                navigation.push(&page);
                navigation_controls.set_nested_page_visible(true);
            });
            servers_group.add(&row);
        }
    }

    let add_server = button_row("Add server", "list-add-symbolic");
    let add_shell = Rc::clone(shell);
    let navigation = navigation.clone();
    let navigation_controls = navigation_controls.clone();
    let add_server_dialog = dialog.clone();
    add_server.connect_activated(move |_| {
        let page = add_shell.add_server_navigation_page(&navigation, &add_server_dialog);
        navigation.push(&page);
        navigation_controls.set_nested_page_visible(true);
    });
    servers_group.add(&add_server);
    page.add(&servers_group);

    let local_group = adw::PreferencesGroup::builder()
        .title(tr("Local Folders"))
        .description(tr(
            "These folders are combined into the Local source and shown through folder browsing.",
        ))
        .build();
    if library.local_folders.is_empty() {
        let row = adw::ActionRow::builder()
            .title(tr("No local folders configured"))
            .subtitle(tr("Add folders to use the Local source."))
            .build();
        local_group.add(&row);
    } else {
        for folder in &library.local_folders {
            let row = adw::ActionRow::builder()
                .title(local_folder_title(&folder.path))
                .subtitle(folder.path.clone())
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("rufin-route-folders-symbolic"));
            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.set_tooltip_text(Some(&tr("Remove")));
            remove.add_css_class("flat");
            remove.add_css_class("destructive-action");
            remove.set_valign(gtk::Align::Center);
            row.add_suffix(&remove);
            row.set_activatable(false);
            let remove_shell = Rc::clone(shell);
            let path = folder.path.clone();
            let row_for_remove = row.clone();
            remove.connect_clicked(move |_| {
                confirm_remove_local_folder(&remove_shell, path.clone(), row_for_remove.clone());
            });
            local_group.add(&row);
        }
    }

    let local_actions = adw::PreferencesRow::new();
    let action_buttons = action_button_box();
    let add_local = row_action_button("Add a music folder", "folder-new-symbolic");
    let add_shell = Rc::clone(shell);
    let add_dialog = dialog.clone();
    add_local.connect_clicked(move |_| {
        let shell = Rc::clone(&add_shell);
        let dialog = add_dialog.clone();
        gtk::glib::spawn_future_local(async move {
            let chooser = gtk::FileDialog::builder()
                .title(tr("Select Music Folder"))
                .build();
            let Ok(folder) = chooser
                .select_folder_future(Some(&shell.chrome.window))
                .await
            else {
                return;
            };
            let Some(path) = folder.path() else {
                return;
            };
            shell.products.source.add_local_library_folder(path);
            dialog.close();
        });
    });
    action_buttons.append(&add_local);
    let resync_local = row_action_button("Resync Library", "view-refresh-symbolic");
    resync_local.set_sensitive(!library.local_folders.is_empty());
    let source = shell.products.source.clone();
    let resync_dialog = dialog.clone();
    resync_local.connect_clicked(move |_| {
        source.resync_local_library();
        resync_dialog.close();
    });
    action_buttons.append(&resync_local);
    local_actions.set_child(Some(&action_buttons));
    local_actions.set_activatable(false);
    local_actions.set_selectable(false);
    local_group.add(&local_actions);
    page.add(&local_group);

    page
}

fn action_button_box() -> gtk::Box {
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    actions.set_homogeneous(true);
    actions.set_halign(gtk::Align::Fill);
    actions.set_hexpand(true);
    actions.set_margin_top(6);
    actions.set_margin_bottom(6);
    actions.set_margin_start(8);
    actions.set_margin_end(8);
    actions
}

fn row_action_button(title: &str, icon_name: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.set_halign(gtk::Align::Fill);
    button.set_hexpand(true);
    button.set_tooltip_text(Some(&tr(title)));
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    content.set_valign(gtk::Align::Center);
    content.append(&gtk::Image::from_icon_name(icon_name));
    let label = gtk::Label::new(Some(&tr(title)));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(0);
    label.set_max_width_chars(18);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_lines(2);
    content.append(&label);
    button.set_child(Some(&content));
    button
}

fn confirm_remove_local_folder(shell: &Rc<Shell>, path: String, row: adw::ActionRow) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Remove Local Folder"))
        .body(format!(
            "{}\n{}",
            tr("This removes the folder from the Local source."),
            path
        ))
        .build();
    let cancel = tr("Cancel");
    let remove = tr("Remove");
    dialog.add_responses(&[("cancel", cancel.as_str()), ("remove", remove.as_str())]);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    let source = shell.products.source.clone();
    dialog.choose(
        Some(&shell.chrome.window),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() == "remove" {
                source.remove_local_library_folder(path.clone());
                row.set_visible(false);
            }
        },
    );
}

fn source_summary_subtitle(
    server: &SourceIdentity,
    summary: Option<&SourceLocalAccessPresentation>,
    username: Option<&str>,
) -> String {
    let address = if server.base_url.trim().is_empty() {
        String::new()
    } else {
        server.base_url.clone()
    };
    let folder = summary
        .and_then(|summary| summary.selected_music_folder_name.as_deref())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| tr("All Music"));
    let mapping = local_mapping_status(summary);
    let account = username
        .filter(|username| !username.trim().is_empty())
        .map(|username| format!("{}: {}", tr("User"), username))
        .unwrap_or_default();
    let cache = summary.map(source_cache_line).unwrap_or_default();
    let provider_line = metadata_line([configured_source_kind_display_name(&server.kind), address]);
    let folder_line = metadata_line([account, format!("{}: {}", tr("Music Folder"), folder)]);
    let cache_line = metadata_line([cache, mapping]);
    [provider_line, folder_line, cache_line]
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn metadata_line(parts: impl IntoIterator<Item = String>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" - ")
}

fn source_cache_line(summary: &SourceLocalAccessPresentation) -> String {
    format!(
        "{}: {}, {}",
        tr("Cached"),
        album_count_text(summary.cached_album_count as u64),
        track_count_text(summary.cached_track_count as u64)
    )
}

fn local_mapping_status(summary: Option<&SourceLocalAccessPresentation>) -> String {
    let Some(summary) = summary else {
        return tr("No local playback mapping");
    };
    if summary.access.is_none() {
        return tr("No local playback mapping");
    }
    let status = &summary.status;
    if status.total_track_count == 0 {
        return tr("Local mapping saved. Sync to preview matches.");
    }
    format!(
        "{}: {} direct, {} prefix, {} metadata, {} unmatched",
        tr("Local mapping"),
        status.direct_match_count,
        status.prefix_match_count,
        status.metadata_match_count,
        status.unmatched_count
    )
}

fn local_folder_title(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.to_string())
}
