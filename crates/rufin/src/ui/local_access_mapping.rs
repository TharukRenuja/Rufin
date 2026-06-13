use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use domain::{LibrarySourceSelection, ServerId, ServerIdentity};
use gtk::gio;
use library::ServerLocalAccess;

use crate::controller::LocalAccessStatus;
use crate::i18n::tr;
use crate::providers::StreamingProvider;

use super::{Shell, login::connect_folder_button, text_button};

type ManageServerExitSlot = adw::NavigationView;

pub(in crate::ui) fn manage_server_navigation_page(
    shell: &Rc<Shell>,
    server: ServerIdentity,
    navigation: &adw::NavigationView,
    preferences_dialog: &adw::Dialog,
) -> adw::NavigationPage {
    let title = server_display_name(&server);
    let toolbar = manage_server_toolbar(shell, server, navigation.clone(), preferences_dialog);
    adw::NavigationPage::new(&toolbar, &title)
}

fn manage_server_toolbar(
    shell: &Rc<Shell>,
    server: ServerIdentity,
    exit: ManageServerExitSlot,
    preferences_dialog: &adw::Dialog,
) -> adw::ToolbarView {
    let (access, access_status, selected) = {
        let library = shell.state.library.borrow();
        let summary = library
            .server_local_access
            .iter()
            .find(|summary| summary.server_id == server.id)
            .cloned();
        let access = summary
            .as_ref()
            .and_then(|summary| summary.access.clone())
            .or_else(|| {
                library
                    .server
                    .as_ref()
                    .filter(|active| active.id == server.id)
                    .and_then(|_| library.local_access.clone())
            });
        let status = summary
            .as_ref()
            .map(|summary| summary.status.clone())
            .or_else(|| {
                library
                    .server
                    .as_ref()
                    .filter(|active| active.id == server.id)
                    .map(|_| library.local_access_status.clone())
            })
            .unwrap_or_default();
        let selected = matches!(
            &library.selected_source,
            Some(LibrarySourceSelection::Server(server_id)) if *server_id == server.id
        );
        (access, status, selected)
    };
    let remote = server.provider != "local";
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title = adw::WindowTitle::new(&tr("Manage Server"), &server_display_name(&server));
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.set_show_back_button(true);
    header.set_title_widget(Some(&title));
    toolbar.add_top_bar(&header);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    scroller.set_child(Some(&content));
    content.append(&server_settings_group(shell, &server, remote));

    let folder = Rc::new(RefCell::new(
        access
            .as_ref()
            .map(|access| PathBuf::from(&access.root_path)),
    ));
    let saved_local_prefix = access
        .as_ref()
        .map(local_access_display_path)
        .unwrap_or_default();
    let saved_server_prefix = access
        .as_ref()
        .and_then(|access| access.path_replace_from.as_deref())
        .unwrap_or_default()
        .to_string();
    let mut display_local_prefix = saved_local_prefix.clone();
    let mut display_server_prefix = saved_server_prefix.clone();
    if display_server_prefix.trim().is_empty()
        && let (Some(server_path), Some(local_path)) = (
            access_status.sample_server_path.as_deref(),
            access_status.sample_local_path.as_deref(),
        )
        && let Some((suggested_server_prefix, suggested_local_prefix)) =
            infer_path_prefixes(server_path, local_path)
    {
        display_server_prefix = suggested_server_prefix;
        display_local_prefix = suggested_local_prefix;
    }
    let initial_draft = LocalAccessDraft {
        folder: folder.borrow().clone(),
        server_prefix: if remote {
            saved_server_prefix.trim().to_string()
        } else {
            String::new()
        },
        local_prefix: if remote {
            saved_local_prefix.trim().to_string()
        } else {
            String::new()
        },
    };

    let folder_row = adw::ActionRow::builder()
        .title(tr("Local Folder"))
        .subtitle(
            access
                .as_ref()
                .map(|access| access.root_path.clone())
                .unwrap_or_else(|| tr("No folder selected")),
        )
        .build();
    let folder_button = gtk::Button::with_label(&tr("Choose"));
    folder_button.set_valign(gtk::Align::Center);
    folder_row.add_suffix(&folder_button);
    folder_row.set_activatable_widget(Some(&folder_button));

    let server_prefix = adw::EntryRow::builder()
        .title(tr("Server Prefix"))
        .text(&display_server_prefix)
        .build();
    server_prefix.set_visible(remote);

    let local_prefix = adw::EntryRow::builder()
        .title(tr("Local Prefix"))
        .text(&display_local_prefix)
        .build();
    local_prefix.set_visible(remote);

    let sample_subtitle = access_status
        .sample_server_path
        .clone()
        .unwrap_or_else(|| tr("No cached server path yet"));
    let sample_row = adw::ActionRow::builder()
        .title(tr("Server Sample"))
        .subtitle(sample_subtitle)
        .build();
    sample_row.set_visible(remote);

    let preview_row = adw::ActionRow::builder()
        .title(tr("Mapped Local Path"))
        .subtitle(preview_local_path_text(
            access_status.sample_server_path.as_deref(),
            server_prefix.text().as_str(),
            local_prefix.text().as_str(),
            folder.borrow().as_deref(),
        ))
        .build();
    preview_row.set_visible(remote);

    let group_title = if remote {
        tr("Local Playback Access")
    } else {
        tr("Local Library")
    };
    let group_description = if remote {
        tr("Optionally map server tracks to files on this computer")
    } else {
        tr("Choose the folder to scan and play directly from this computer")
    };
    let group = adw::PreferencesGroup::builder()
        .title(group_title)
        .description(group_description)
        .build();
    let mapping_expander = if remote {
        let subtitle = if access.is_some() {
            tr("Local playback mapping configured")
        } else {
            tr("Map server tracks to local files")
        };
        let expander = adw::ExpanderRow::builder()
            .title(tr("Local Playback Mapping"))
            .subtitle(subtitle)
            .build();
        expander.add_row(&folder_row);
        expander.add_row(&server_prefix);
        expander.add_row(&local_prefix);
        expander.add_row(&sample_row);
        expander.add_row(&preview_row);
        group.add(&expander);
        Some(expander)
    } else {
        group.add(&folder_row);
        group.add(&server_prefix);
        group.add(&local_prefix);
        group.add(&sample_row);
        group.add(&preview_row);
        None
    };
    content.append(&group);

    let status = gtk::Label::new(None);
    status.add_css_class("muted");
    status.set_wrap(true);
    status.set_xalign(0.0);
    content.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let remove = text_button("edit-clear-symbolic", "Clear Mapping");
    remove.set_visible(server.provider != "local" && access.is_some());
    let save = text_button("document-save-symbolic", "Save Mapping");
    save.add_css_class("suggested-action");
    actions.append(&remove);
    actions.append(&save);
    content.append(&actions);
    if let Some(expander) = mapping_expander.as_ref() {
        status.set_visible(expander.is_expanded());
        actions.set_visible(expander.is_expanded());
        expander.connect_expanded_notify({
            let status = status.clone();
            let actions = actions.clone();
            move |expander| {
                let expanded = expander.is_expanded();
                status.set_visible(expanded);
                actions.set_visible(expanded);
            }
        });
    }

    content.append(&server_actions_group(
        shell,
        &server,
        selected,
        &exit,
        preferences_dialog,
    ));
    toolbar.set_content(Some(&scroller));

    let update_state = Rc::new({
        let folder = Rc::clone(&folder);
        let server_prefix = server_prefix.clone();
        let local_prefix = local_prefix.clone();
        let preview_row = preview_row.clone();
        let status = status.clone();
        let save = save.clone();
        let initial_draft = initial_draft.clone();
        let access_status = access_status.clone();
        move || {
            let draft = local_access_draft(&folder, &server_prefix, &local_prefix, remote);
            let has_location =
                draft.folder.is_some() && (!remote || !draft.local_prefix.trim().is_empty());
            let local_prefix_exists = !remote || Path::new(draft.local_prefix.trim()).is_dir();
            let changed = draft != initial_draft;
            let preview = preview_local_path_preview(
                access_status.sample_server_path.as_deref(),
                draft.server_prefix.as_str(),
                draft.local_prefix.as_str(),
                draft.folder.as_deref(),
            );
            save.set_sensitive(
                has_location && local_prefix_exists && changed && (!remote || preview.saveable),
            );
            preview_row.set_subtitle(&preview.text);
            status.set_text(&local_access_status_text(
                &draft,
                remote,
                changed,
                &access_status,
            ));
        }
    });
    connect_folder_button(
        &shell.window,
        &folder_button,
        &folder_row,
        Rc::clone(&folder),
        {
            let local_prefix = local_prefix.clone();
            let update_state = Rc::clone(&update_state);
            move |path| {
                if remote {
                    local_prefix.set_text(&path.display().to_string());
                }
                update_state();
            }
        },
    );
    server_prefix.connect_text_notify({
        let update_state = Rc::clone(&update_state);
        move |_| update_state()
    });
    local_prefix.connect_text_notify({
        let update_state = Rc::clone(&update_state);
        move |_| update_state()
    });

    let controller = shell.controller.clone();
    let server_id = server.id.clone();
    let exit_for_remove = exit.clone();
    remove.connect_clicked(move |_| {
        controller.clear_server_local_access(server_id.clone());
        close_manage_server(&exit_for_remove);
    });

    let controller = shell.controller.clone();
    let server_id = server.id.clone();
    let provider = server.provider.clone();
    let status_for_save = status.clone();
    let exit_for_save = exit.clone();
    let preferences_dialog_for_save = preferences_dialog.clone();
    save.connect_clicked(move |_| {
        let Some(root) = folder.borrow().clone() else {
            status_for_save.set_text(&tr("Choose a local music folder"));
            return;
        };
        if provider == "local" {
            controller.add_local_server(root);
            preferences_dialog_for_save.close();
        } else {
            let local_prefix_text = local_prefix.text().to_string();
            if local_prefix_text.trim().is_empty() {
                status_for_save.set_text(&tr("Enter a local prefix."));
                return;
            }
            controller.save_server_local_access(
                server_id.clone(),
                root,
                Some(server_prefix.text().to_string()),
                Some(local_prefix_text),
            );
        }
        close_manage_server(&exit_for_save);
    });

    update_state();
    toolbar
}

fn close_manage_server(exit: &ManageServerExitSlot) {
    exit.pop();
}

fn server_settings_group(
    shell: &Rc<Shell>,
    server: &ServerIdentity,
    remote: bool,
) -> adw::PreferencesGroup {
    let (saved_username, saved_trust_invalid_cert) = {
        let library = shell.state.library.borrow();
        let summary = library
            .server_local_access
            .iter()
            .find(|summary| summary.server_id == server.id);
        (
            summary
                .and_then(|summary| summary.username.clone())
                .unwrap_or_default(),
            summary.is_some_and(|summary| summary.trust_invalid_cert),
        )
    };

    let group = adw::PreferencesGroup::builder()
        .title(tr("Server Settings"))
        .build();

    group.add(&info_row(
        "Provider",
        &provider_display_name(&server.provider),
    ));

    let name = adw::EntryRow::builder()
        .title(tr("Name"))
        .text(&server.name)
        .build();
    group.add(&name);

    let address = adw::EntryRow::builder()
        .title(tr("Server Address"))
        .text(&server.base_url)
        .build();
    address.set_visible(remote);
    group.add(&address);

    let username = adw::EntryRow::builder()
        .title(tr("Username"))
        .text(&saved_username)
        .build();
    username.set_visible(remote);
    group.add(&username);

    let password = adw::PasswordEntryRow::builder()
        .title(tr("Password"))
        .build();
    password.set_visible(remote);
    group.add(&password);

    let cert_verify = adw::SwitchRow::builder()
        .title(tr("Verify server certificate"))
        .subtitle(tr("Turn off only for a server you control"))
        .active(!saved_trust_invalid_cert)
        .build();
    cert_verify.set_visible(remote);
    group.add(&cert_verify);

    let save = button_row("Save Server Settings", "document-save-symbolic");
    save.add_css_class("suggested-action");
    group.add(&save);

    let controller = shell.controller.clone();
    let server_id = server.id.clone();
    let provider = server.provider.clone();
    let original_address = server.base_url.clone();
    let original_username = saved_username.clone();
    save.connect_activated(move |_| {
        let base_url = if provider == "local" {
            original_address.clone()
        } else {
            address.text().trim().to_string()
        };
        let username = if provider == "local" {
            original_username.clone()
        } else {
            username.text().trim().to_string()
        };
        controller.update_server_settings(
            server_id.clone(),
            name.text().trim().to_string(),
            base_url,
            username,
            password.text().to_string(),
            !cert_verify.is_active(),
        );
    });

    group
}

fn server_actions_group(
    shell: &Rc<Shell>,
    server: &ServerIdentity,
    selected: bool,
    exit: &ManageServerExitSlot,
    preferences_dialog: &adw::Dialog,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Server Actions"))
        .build();

    if !selected {
        let select = button_row("Use This Source", "object-select-symbolic");
        let controller = shell.controller.clone();
        let server_id = server.id.clone();
        let exit = exit.clone();
        let preferences_dialog = preferences_dialog.clone();
        select.connect_activated(move |_| {
            controller.select_source(LibrarySourceSelection::Server(server_id.clone()));
            close_manage_server(&exit);
            preferences_dialog.close();
        });
        group.add(&select);
    }

    let resync = button_row("Resync Library", "view-refresh-symbolic");
    let controller = shell.controller.clone();
    let server_id = server.id.clone();
    resync.connect_activated(move |_| controller.resync_server(server_id.clone()));
    group.add(&resync);

    let clear_cache = button_row("Clear Cached Library", "edit-clear-symbolic");
    let clear_shell = Rc::clone(shell);
    let server_id = server.id.clone();
    let server_name = server_display_name(server);
    clear_cache.connect_activated(move |_| {
        confirm_clear_server_cache(&clear_shell, server_id.clone(), &server_name);
    });
    group.add(&clear_cache);

    let forget = button_row("Forget Server", "user-trash-symbolic");
    forget.add_css_class("destructive-action");
    let forget_shell = Rc::clone(shell);
    let server_id = server.id.clone();
    let server_name = server_display_name(server);
    let exit = exit.clone();
    let preferences_dialog = preferences_dialog.clone();
    forget.connect_activated(move |_| {
        confirm_forget_server(
            &forget_shell,
            server_id.clone(),
            &server_name,
            exit.clone(),
            preferences_dialog.clone(),
        );
    });
    group.add(&forget);

    group
}

fn info_row(title: &str, value: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(tr(title))
        .subtitle(if value.trim().is_empty() {
            tr("Not set")
        } else {
            value.to_string()
        })
        .build()
}

fn button_row(title: &str, icon_name: &str) -> adw::ButtonRow {
    adw::ButtonRow::builder()
        .title(tr(title))
        .start_icon_name(icon_name)
        .end_icon_name("go-next-symbolic")
        .build()
}

fn confirm_clear_server_cache(shell: &Rc<Shell>, server_id: ServerId, server_name: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Clear Cached Library"))
        .body(format!(
            "{} {}",
            tr("This removes cached library metadata for"),
            server_name
        ))
        .build();
    let cancel = tr("Cancel");
    let clear = tr("Clear Cache");
    dialog.add_responses(&[("cancel", cancel.as_str()), ("clear", clear.as_str())]);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
    let controller = shell.controller.clone();
    dialog.choose(
        Some(&shell.window),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() == "clear" {
                controller.clear_server_cache(server_id.clone());
            }
        },
    );
}

fn confirm_forget_server(
    shell: &Rc<Shell>,
    server_id: ServerId,
    server_name: &str,
    exit: ManageServerExitSlot,
    preferences_dialog: adw::Dialog,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Forget Server"))
        .body(format!(
            "{} {}",
            tr("This removes the server, cached library metadata, queue snapshot, and saved token for"),
            server_name
        ))
        .build();
    let cancel = tr("Cancel");
    let forget = tr("Forget Server");
    dialog.add_responses(&[("cancel", cancel.as_str()), ("forget", forget.as_str())]);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("forget", adw::ResponseAppearance::Destructive);
    let controller = shell.controller.clone();
    dialog.choose(
        Some(&shell.window),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() == "forget" {
                controller.forget_server(server_id.clone());
                close_manage_server(&exit);
                preferences_dialog.close();
            }
        },
    );
}

fn server_display_name(server: &ServerIdentity) -> String {
    if server.name.trim().is_empty() {
        StreamingProvider::from_provider_id(&server.provider)
            .map(|provider| tr(provider.title()))
            .unwrap_or_else(|| server.provider.clone())
    } else {
        server.name.clone()
    }
}

fn provider_display_name(provider: &str) -> String {
    StreamingProvider::from_provider_id(provider)
        .map(|provider| tr(provider.title()))
        .unwrap_or_else(|| provider.to_string())
}

fn local_access_display_path(access: &ServerLocalAccess) -> String {
    access
        .path_replace_to
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&access.root_path)
        .to_string()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalAccessDraft {
    folder: Option<PathBuf>,
    server_prefix: String,
    local_prefix: String,
}

fn local_access_draft(
    folder: &Rc<RefCell<Option<PathBuf>>>,
    server_prefix: &adw::EntryRow,
    local_prefix: &adw::EntryRow,
    remote: bool,
) -> LocalAccessDraft {
    LocalAccessDraft {
        folder: folder.borrow().clone(),
        server_prefix: if remote {
            server_prefix.text().trim().to_string()
        } else {
            String::new()
        },
        local_prefix: if remote {
            local_prefix.text().trim().to_string()
        } else {
            String::new()
        },
    }
}

fn preview_local_path_text(
    sample_server_path: Option<&str>,
    server_prefix: &str,
    local_prefix: &str,
    folder: Option<&Path>,
) -> String {
    preview_local_path_preview(sample_server_path, server_prefix, local_prefix, folder).text
}

struct LocalPathPreview {
    text: String,
    saveable: bool,
}

fn preview_local_path_preview(
    sample_server_path: Option<&str>,
    server_prefix: &str,
    local_prefix: &str,
    folder: Option<&Path>,
) -> LocalPathPreview {
    let Some(sample) = sample_server_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return LocalPathPreview {
            text: tr("No cached server path yet"),
            saveable: true,
        };
    };
    let server_prefix = server_prefix.trim();
    let local_prefix = local_prefix.trim();
    let base = if local_prefix.is_empty() {
        let Some(folder) = folder else {
            return LocalPathPreview {
                text: tr("Choose a local prefix."),
                saveable: false,
            };
        };
        folder.to_path_buf()
    } else {
        PathBuf::from(local_prefix)
    };

    if !server_prefix.is_empty() {
        if !sample.starts_with(server_prefix) {
            return LocalPathPreview {
                text: tr("Server sample does not match the server prefix."),
                saveable: false,
            };
        }
        let suffix = sample[server_prefix.len()..].trim_start_matches(['/', '\\']);
        return LocalPathPreview {
            text: base
                .join(path_from_server_suffix(suffix))
                .to_string_lossy()
                .into_owned(),
            saveable: true,
        };
    }

    let sample_path = Path::new(sample);
    if sample_path.is_relative() {
        return LocalPathPreview {
            text: base.join(sample_path).to_string_lossy().into_owned(),
            saveable: true,
        };
    }
    if sample_path.is_file() {
        return LocalPathPreview {
            text: sample.to_string(),
            saveable: true,
        };
    }
    LocalPathPreview {
        text: tr("Enter a matching server prefix to map this path."),
        saveable: false,
    }
}

fn local_access_status_text(
    draft: &LocalAccessDraft,
    remote: bool,
    changed: bool,
    status: &LocalAccessStatus,
) -> String {
    if draft.folder.is_none() {
        return tr("Choose a local music folder");
    }
    if !remote {
        return if changed {
            tr("Save to rescan this local library.")
        } else {
            tr("Local library folder is saved.")
        };
    }
    if draft.local_prefix.trim().is_empty() {
        return tr("Choose a local prefix.");
    }
    if !Path::new(draft.local_prefix.trim()).is_dir() {
        return tr("Choose an existing local prefix.");
    }
    if status.total_track_count == 0 {
        return if changed {
            tr("Save to apply this mapping after the next sync.")
        } else {
            tr("No cached tracks yet. Sync the server to preview matches.")
        };
    }

    let lead = if changed {
        tr("Unsaved changes.")
    } else {
        tr("Saved mapping.")
    };
    format!(
        "{} {} direct, {} prefix, {} metadata, {} unmatched of {} tracks.",
        lead,
        status.direct_match_count,
        status.prefix_match_count,
        status.metadata_match_count,
        status.unmatched_count,
        status.total_track_count
    )
}

fn infer_path_prefixes(server_path: &str, local_path: &str) -> Option<(String, String)> {
    let server_parts = path_component_spans(server_path);
    let local_parts = path_component_spans(local_path);
    let suffix_len = common_suffix_len(&server_parts, &local_parts);
    if suffix_len == 0 || suffix_len > server_parts.len() || suffix_len > local_parts.len() {
        return None;
    }
    let server_prefix = prefix_before_suffix(server_path, &server_parts, suffix_len)?;
    let local_prefix = prefix_before_suffix(local_path, &local_parts, suffix_len)?;
    Some((server_prefix, local_prefix))
}

fn common_suffix_len(server_parts: &[PathComponent], local_parts: &[PathComponent]) -> usize {
    server_parts
        .iter()
        .rev()
        .zip(local_parts.iter().rev())
        .take_while(|(server, local)| server.value.eq_ignore_ascii_case(local.value))
        .count()
}

fn prefix_before_suffix(value: &str, parts: &[PathComponent], suffix_len: usize) -> Option<String> {
    let suffix_start_index = parts.len().checked_sub(suffix_len)?;
    let prefix_end = parts.get(suffix_start_index)?.start;
    let raw_prefix = &value[..prefix_end];
    let trimmed = raw_prefix.trim_end_matches(['/', '\\']);
    if !trimmed.is_empty() {
        return Some(trimmed.to_string());
    }
    raw_prefix
        .chars()
        .find(|character| *character == '/' || *character == '\\')
        .map(|character| character.to_string())
}

#[derive(Clone, Debug)]
struct PathComponent<'a> {
    value: &'a str,
    start: usize,
}

fn path_component_spans(value: &str) -> Vec<PathComponent<'_>> {
    let mut parts = Vec::new();
    let mut start = None;
    for (index, character) in value.char_indices() {
        if character == '/' || character == '\\' {
            if let Some(part_start) = start.take()
                && part_start < index
            {
                parts.push(PathComponent {
                    value: &value[part_start..index],
                    start: part_start,
                });
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(part_start) = start
        && part_start < value.len()
    {
        parts.push(PathComponent {
            value: &value[part_start..],
            start: part_start,
        });
    }
    parts
}

fn path_from_server_suffix(suffix: &str) -> PathBuf {
    suffix
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<PathBuf>()
}
