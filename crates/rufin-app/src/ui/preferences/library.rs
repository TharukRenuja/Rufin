use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{LibrarySourceSelection, ServerIdentity};

use crate::controller::{LibrarySnapshot, ServerLocalAccessSnapshot};
use crate::i18n::tr;

use super::{Shell, button_row};

pub(super) fn library_page(
    shell: &Rc<Shell>,
    dialog: &adw::PreferencesDialog,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Library"))
        .icon_name("folder-music-symbolic")
        .build();

    let library = shell.state.library.borrow().clone();

    let sources_group = adw::PreferencesGroup::builder()
        .title(tr("Sources"))
        .description(tr(
            "Choose sources from the sidebar. Configure server mappings and local folders here.",
        ))
        .build();

    if library.servers.is_empty() {
        let row = adw::ActionRow::builder()
            .title(tr("No servers configured"))
            .subtitle(tr(
                "Add a server to use Jellyfin, Subsonic, or OpenSubsonic.",
            ))
            .build();
        sources_group.add(&row);
    } else {
        for server in &library.servers {
            let selected = matches!(
                &library.selected_source,
                Some(LibrarySourceSelection::Server(server_id)) if *server_id == server.id
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
            let settings = gtk::Button::with_label(&tr("Settings"));
            settings.set_valign(gtk::Align::Center);
            row.add_suffix(&settings);
            row.set_activatable_widget(Some(&settings));
            let settings_shell = Rc::clone(shell);
            let server = server.clone();
            settings.connect_clicked(move |_| {
                settings_shell.present_manage_server_dialog(server.clone());
            });
            sources_group.add(&row);
        }
    }

    let local_selected = matches!(library.selected_source, Some(LibrarySourceSelection::Local));
    let local_row = adw::ActionRow::builder()
        .title(tr("Local"))
        .subtitle(local_source_subtitle(&library))
        .subtitle_lines(2)
        .build();
    local_row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
    if local_selected {
        local_row.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
    }
    sources_group.add(&local_row);
    page.add(&sources_group);

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
            row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
            let remove = gtk::Button::from_icon_name("user-trash-symbolic");
            remove.set_tooltip_text(Some(&tr("Remove")));
            remove.add_css_class("flat");
            remove.add_css_class("destructive-action");
            remove.set_valign(gtk::Align::Center);
            row.add_suffix(&remove);
            row.set_activatable_widget(Some(&remove));
            let controller = shell.controller.clone();
            let dialog = dialog.clone();
            let path = folder.path.clone();
            remove.connect_clicked(move |_| {
                controller.remove_local_library_folder(path.clone());
                dialog.close();
            });
            local_group.add(&row);
        }
    }

    let add_local = button_row("Add Local Folder", "folder-new-symbolic");
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
        format!(
            "{}: {} {}, {} {}. {}",
            tr("Cached"),
            library.cached_album_count,
            tr("albums"),
            library.cached_track_count,
            tr("tracks"),
            library.sync_status
        )
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

fn local_source_subtitle(library: &LibrarySnapshot) -> String {
    let folder_count = match library.local_folders.len() {
        0 => tr("No local folders configured"),
        1 => tr("1 folder"),
        count => format!("{} {}", count, tr("folders")),
    };
    if matches!(library.selected_source, Some(LibrarySourceSelection::Local)) {
        format!("{}\n{}", folder_count, library.sync_status)
    } else {
        folder_count
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
        "local" | "fake" => "folder-symbolic",
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
