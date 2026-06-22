use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use domain::{LibrarySourceSelection, ServerIdentity};
use gtk::gio;

use crate::controller::{LibrarySnapshot, ServerLocalAccessSnapshot};
use crate::i18n::tr;

use super::super::{album_count_text, track_count_text};
use super::{Shell, button_row};

pub(super) fn library_page(shell: &Rc<Shell>, dialog: &adw::Dialog) -> gtk::Widget {
    let navigation = adw::NavigationView::new();
    let page = library_sources_page(shell, dialog, &navigation);
    let root = adw::NavigationPage::new(&page, &tr("Library"));
    navigation.push(&root);
    navigation.upcast::<gtk::Widget>()
}

fn library_sources_page(
    shell: &Rc<Shell>,
    dialog: &adw::Dialog,
    navigation: &adw::NavigationView,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Library"))
        .icon_name("route-tracks-symbolic")
        .build();

    let library = shell.state.library.borrow().clone();

    let servers_group = adw::PreferencesGroup::builder()
        .title(tr("Servers"))
        .description(tr(
            "Configure saved music servers and local playback mappings.",
        ))
        .build();

    if library.servers.is_empty() {
        let row = adw::ActionRow::builder()
            .title(tr("No servers configured"))
            .subtitle(tr(
                "Add a server to use Jellyfin, Subsonic, or OpenSubsonic.",
            ))
            .build();
        servers_group.add(&row);
    } else {
        for server in &library.servers {
            let selected = matches!(
                &library.selected_source,
                Some(LibrarySourceSelection::Server(server_id))
                    if server_id.as_str() == server.id.as_str()
            );
            let summary = library
                .server_local_access
                .iter()
                .find(|summary| summary.server_id == server.id);
            let row = adw::ActionRow::builder()
                .title(server_display_name(server))
                .subtitle(server_source_subtitle(&library, server, summary, selected))
                .subtitle_lines(4)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(provider_icon_name(
                &server.provider,
            )));
            if selected {
                row.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
            }
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            row.set_activatable(true);
            let settings_shell = Rc::clone(shell);
            let navigation = navigation.clone();
            let dialog = dialog.clone();
            let server = server.clone();
            row.connect_activated(move |_| {
                let page = super::super::local_access_mapping::manage_server_navigation_page(
                    &settings_shell,
                    server.clone(),
                    &navigation,
                    &dialog,
                );
                navigation.push(&page);
            });
            servers_group.add(&row);
        }
    }

    let add_server = button_row("Add server", "list-add-symbolic");
    let add_shell = Rc::clone(shell);
    let add_dialog = dialog.clone();
    add_server.connect_activated(move |_| {
        add_shell.present_add_server_dialog_closing(&add_dialog);
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
            row.add_prefix(&gtk::Image::from_icon_name("route-folders-symbolic"));
            let remove = gtk::Button::from_icon_name("user-trash-symbolic");
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

    let add_local = button_row("Add a music folder", "folder-new-symbolic");
    let add_shell = Rc::clone(shell);
    let add_dialog = dialog.clone();
    add_local.connect_activated(move |_| {
        let shell = Rc::clone(&add_shell);
        let dialog = add_dialog.clone();
        gtk::glib::spawn_future_local(async move {
            let chooser = gtk::FileDialog::builder()
                .title(tr("Select Music Folder"))
                .build();
            let Ok(folder) = chooser.select_folder_future(Some(&shell.window)).await else {
                return;
            };
            let Some(path) = folder.path() else {
                return;
            };
            shell.controller.add_local_library_folder(path);
            dialog.close();
        });
    });
    local_group.add(&add_local);
    page.add(&local_group);

    page
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
    let controller = shell.controller.clone();
    dialog.choose(
        Some(&shell.window),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() == "remove" {
                controller.remove_local_library_folder(path.clone());
                row.set_visible(false);
            }
        },
    );
}

fn server_source_subtitle(
    library: &LibrarySnapshot,
    server: &ServerIdentity,
    summary: Option<&ServerLocalAccessSnapshot>,
    selected: bool,
) -> String {
    let provider = provider_display_name(&server.provider);
    let address = if server.base_url.trim().is_empty() {
        provider.clone()
    } else {
        server.base_url.clone()
    };
    let folder = summary
        .and_then(|summary| summary.selected_music_folder_name.as_deref())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            if selected {
                tr("All Music")
            } else {
                tr("Saved per server")
            }
        });
    let mapping = local_mapping_status(summary);
    let account = if selected {
        library
            .username
            .as_deref()
            .map(|username| format!("{}: {}", tr("User"), username))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let cache = if selected {
        selected_server_cache_line(library)
    } else {
        String::new()
    };
    [
        provider,
        address,
        account,
        format!("{}: {}", tr("Music Folder"), folder),
        mapping,
        cache,
    ]
    .into_iter()
    .filter(|line| !line.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

fn selected_server_cache_line(library: &LibrarySnapshot) -> String {
    let line = format!(
        "{}: {}, {}",
        tr("Cached"),
        album_count_text(library.cached_album_count as u64),
        track_count_text(library.cached_track_count as u64)
    );
    match library_sync_status_detail(&library.sync_status) {
        Some(status) => format!("{line}. {status}"),
        None => line,
    }
}

fn library_sync_status_detail(status: &str) -> Option<String> {
    let status = status.trim();
    match status {
        "" | "Cached library ready" => None,
        "Library sync complete" => Some(tr("Library sync complete")),
        _ => Some(status.to_string()),
    }
}

fn local_mapping_status(summary: Option<&ServerLocalAccessSnapshot>) -> String {
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

fn provider_icon_name(provider: &str) -> &'static str {
    match provider {
        "jellyfin" => "io.github.screwys.Rufin.provider.jellyfin",
        "navidrome" => "io.github.screwys.Rufin.provider.navidrome",
        "subsonic" | "opensubsonic" => "io.github.screwys.Rufin.provider.opensubsonic",
        "local" | "fake" => "route-folders-symbolic",
        _ => "network-server-symbolic",
    }
}

fn local_folder_title(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.to_string())
}
