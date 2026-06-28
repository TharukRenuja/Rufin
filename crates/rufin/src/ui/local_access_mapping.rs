use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use domain::{LibrarySourceSelection, ServerId, ServerIdentity};
use gtk::gio;
use library::ServerLocalAccess;

use crate::controller::{LocalAccessStatus, ServerSettingsInput};
use crate::i18n::{tr, trn_with};
use crate::providers::StreamingProvider;

use super::{
    Shell,
    layout::{
        compact_field_row_group, install_compact_field_row_responsiveness,
        large_popup_content_width, style_compact_field_row,
    },
    login::connect_folder_button,
    text_button,
};

const MANAGE_SERVER_CLAMP_WIDTH: i32 = 560;

#[derive(Clone)]
struct ManageServerExitSlot {
    navigation: adw::NavigationView,
    on_close: Rc<dyn Fn()>,
}

pub(in crate::ui) fn manage_server_navigation_page(
    shell: &Rc<Shell>,
    server: ServerIdentity,
    navigation: &adw::NavigationView,
    preferences_dialog: &adw::Dialog,
    on_close: Rc<dyn Fn()>,
) -> adw::NavigationPage {
    let title = server_display_name(&server);
    let content = manage_server_content(
        shell,
        server,
        ManageServerExitSlot {
            navigation: navigation.clone(),
            on_close,
        },
        preferences_dialog,
    );
    adw::NavigationPage::new(&content, &title)
}

fn manage_server_content(
    shell: &Rc<Shell>,
    server: ServerIdentity,
    exit: ManageServerExitSlot,
    preferences_dialog: &adw::Dialog,
) -> gtk::Widget {
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
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(large_popup_content_width(MANAGE_SERVER_CLAMP_WIDTH));
    clamp.set_tightening_threshold(360);
    clamp.set_margin_top(8);
    clamp.set_margin_bottom(20);
    clamp.set_margin_start(24);
    clamp.set_margin_end(24);
    clamp.set_valign(gtk::Align::Start);
    scroller.set_child(Some(&clamp));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.add_css_class("manage-server-content");
    content.set_hexpand(true);
    clamp.set_child(Some(&content));
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
    let group = adw::PreferencesGroup::builder().title(group_title).build();
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
    status.add_css_class("manage-server-status");
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
    scroller.upcast()
}

fn close_manage_server(exit: &ManageServerExitSlot) {
    exit.navigation.pop();
    (exit.on_close)();
}

fn server_settings_group(shell: &Rc<Shell>, server: &ServerIdentity, remote: bool) -> gtk::Box {
    let (saved_username, saved_trust_invalid_cert, saved_use_jellyfin_instant_mix) = {
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
            summary.is_some_and(|summary| summary.use_jellyfin_instant_mix),
        )
    };

    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let fields_group = adw::PreferencesGroup::builder()
        .title(tr("Server Settings"))
        .description(provider_display_name(&server.provider))
        .build();

    let (name_address_row, name, address) =
        server_name_address_row(&server.name, &server.base_url, remote);
    fields_group.add(&name_address_row);
    section.append(&fields_group);

    let rows_group = adw::PreferencesGroup::new();

    let username = adw::EntryRow::builder()
        .title(tr("Username"))
        .text(&saved_username)
        .build();
    style_compact_field_row(&username);
    username.set_visible(remote);
    rows_group.add(&username);

    let password = adw::PasswordEntryRow::builder()
        .title(tr("Password"))
        .build();
    style_compact_field_row(&password);
    password.set_visible(remote);
    rows_group.add(&password);

    let cert_verify = adw::SwitchRow::builder()
        .title(tr("Verify server certificate"))
        .subtitle(tr("Off only for a server you control"))
        .active(!saved_trust_invalid_cert)
        .build();
    cert_verify.set_visible(remote);
    rows_group.add(&cert_verify);

    let instant_mix = adw::SwitchRow::builder()
        .title(tr("Use Jellyfin Instant Mix for recommendations"))
        .subtitle(tr("This uses Jellyfin API for play radio, necessary if you want recommendation plugins to work."))
        .active(saved_use_jellyfin_instant_mix)
        .build();
    instant_mix.set_visible(server.provider == "jellyfin");
    rows_group.add(&instant_mix);

    let save = button_row("Save Server Settings", "document-save-symbolic");
    save.add_css_class("suggested-action");
    rows_group.add(&save);
    section.append(&rows_group);

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
        controller.update_server_settings(ServerSettingsInput {
            server_id: server_id.clone(),
            name: name.text().trim().to_string(),
            base_url,
            username,
            password: password.text().to_string(),
            trust_invalid_cert: !cert_verify.is_active(),
            use_jellyfin_instant_mix: provider == "jellyfin" && instant_mix.is_active(),
        });
    });

    section
}

fn server_name_address_row(
    name_text: &str,
    address_text: &str,
    show_address: bool,
) -> (gtk::Box, adw::EntryRow, adw::EntryRow) {
    let fields = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    fields.set_homogeneous(true);
    fields.set_halign(gtk::Align::Fill);
    fields.set_hexpand(true);
    fields.set_margin_top(0);
    fields.set_margin_bottom(0);

    let name = adw::EntryRow::builder()
        .title(tr("Name"))
        .text(name_text)
        .build();
    style_compact_field_row(&name);
    let name_group = compact_field_row_group(&name);
    fields.append(&name_group);

    let address = adw::EntryRow::builder()
        .title(tr("Server Address"))
        .text(address_text)
        .build();
    style_compact_field_row(&address);
    let address_group = compact_field_row_group(&address);
    address_group.set_visible(show_address);
    fields.append(&address_group);

    install_compact_field_row_responsiveness(&fields);

    (fields, name, address)
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
    let row = adw::PreferencesRow::new();
    let actions = action_button_box();

    if !selected {
        let select = row_action_button("Use This Source", "object-select-symbolic");
        let controller = shell.controller.clone();
        let server_id = server.id.clone();
        let exit = exit.clone();
        let preferences_dialog = preferences_dialog.clone();
        select.connect_clicked(move |_| {
            controller.select_source(LibrarySourceSelection::Server(server_id.clone()));
            close_manage_server(&exit);
            preferences_dialog.close();
        });
        actions.append(&select);
    }

    let resync = row_action_button("Resync Library", "view-refresh-symbolic");
    let controller = shell.controller.clone();
    let server_id = server.id.clone();
    let preferences_dialog_for_resync = preferences_dialog.clone();
    resync.connect_clicked(move |_| {
        controller.resync_server(server_id.clone());
        preferences_dialog_for_resync.close();
    });
    actions.append(&resync);

    let clear_cache = row_action_button("Clear Cached Library", "edit-clear-symbolic");
    let clear_shell = Rc::clone(shell);
    let server_id = server.id.clone();
    let server_name = server_display_name(server);
    clear_cache.connect_clicked(move |_| {
        confirm_clear_server_cache(&clear_shell, server_id.clone(), &server_name);
    });
    actions.append(&clear_cache);

    let forget = row_action_button("Forget Server", "window-close-symbolic");
    forget.add_css_class("destructive-action");
    let forget_shell = Rc::clone(shell);
    let server_id = server.id.clone();
    let server_name = server_display_name(server);
    let exit = exit.clone();
    let preferences_dialog = preferences_dialog.clone();
    forget.connect_clicked(move |_| {
        confirm_forget_server(
            &forget_shell,
            server_id.clone(),
            &server_name,
            exit.clone(),
            preferences_dialog.clone(),
        );
    });
    actions.append(&forget);
    row.set_child(Some(&actions));
    row.set_activatable(false);
    row.set_selectable(false);
    group.add(&row);

    group
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

fn button_row(title: &str, icon_name: &str) -> adw::ButtonRow {
    let row = adw::ButtonRow::builder()
        .title(tr(title))
        .start_icon_name(icon_name)
        .end_icon_name("go-next-symbolic")
        .build();
    row.add_css_class("manage-server-action-row");
    row
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
        let suffix = sample
            .get(server_prefix.len()..)
            .unwrap_or_default()
            .trim_start_matches(['/', '\\']);
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

    let total = status.total_track_count.to_string();
    let direct = status.direct_match_count.to_string();
    let prefix = status.prefix_match_count.to_string();
    let metadata = status.metadata_match_count.to_string();
    let unmatched = status.unmatched_count.to_string();
    let args = [
        ("direct", direct.as_str()),
        ("prefix", prefix.as_str()),
        ("metadata", metadata.as_str()),
        ("unmatched", unmatched.as_str()),
        ("total", total.as_str()),
    ];
    if changed {
        trn_with(
            "Unsaved changes. {direct} direct, {prefix} prefix, {metadata} metadata, {unmatched} unmatched of {total} track.",
            "Unsaved changes. {direct} direct, {prefix} prefix, {metadata} metadata, {unmatched} unmatched of {total} tracks.",
            status.total_track_count as u64,
            &args,
        )
    } else {
        trn_with(
            "Saved mapping. {direct} direct, {prefix} prefix, {metadata} metadata, {unmatched} unmatched of {total} track.",
            "Saved mapping. {direct} direct, {prefix} prefix, {metadata} metadata, {unmatched} unmatched of {total} tracks.",
            status.total_track_count as u64,
            &args,
        )
    }
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
    let raw_prefix = value.get(..prefix_end)?;
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
                    value: value.get(part_start..index).unwrap_or_default(),
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
            value: value.get(part_start..).unwrap_or_default(),
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
