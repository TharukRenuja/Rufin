use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use rufin_core::ServerIdentity;
use rufin_store::ServerLocalAccess;

use crate::controller::LocalAccessStatus;
use crate::i18n::tr;
use crate::providers::StreamingProvider;

use super::{
    Shell, icon_button,
    layout::{large_popup_content_height, large_popup_content_width},
    text_button,
};

const ADD_SERVER_DIALOG_WIDTH: i32 = 620;
const ADD_SERVER_DIALOG_HEIGHT: i32 = 680;
const ADD_SERVER_CLAMP_WIDTH: i32 = 560;

impl Shell {
    pub(super) fn present_add_server_dialog(self: &Rc<Self>) {
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(&tr("Add Server"), "");
        header.set_title_widget(Some(&title));
        let close = icon_button("window-close-symbolic", "Close");
        header.pack_end(&close);
        toolbar.add_top_bar(&header);

        let child = self.add_server_view();
        toolbar.set_content(Some(&child));
        let dialog = adw::Dialog::builder()
            .content_width(large_popup_content_width(ADD_SERVER_DIALOG_WIDTH))
            .content_height(large_popup_content_height(
                self.window.height(),
                ADD_SERVER_DIALOG_HEIGHT,
            ))
            .child(&toolbar)
            .build();
        let dialog_for_close = dialog.clone();
        close.connect_clicked(move |_| {
            dialog_for_close.close();
        });
        dialog.present(Some(&self.window));
    }

    pub(super) fn present_manage_server_dialog(self: &Rc<Self>, server: ServerIdentity) {
        let (access, access_status) = {
            let library = self.state.library.borrow();
            let access = library
                .server
                .as_ref()
                .filter(|active| active.id == server.id)
                .and_then(|_| library.local_access.clone());
            let status = library
                .server
                .as_ref()
                .filter(|active| active.id == server.id)
                .map(|_| library.local_access_status.clone())
                .unwrap_or_default();
            (access, status)
        };
        let remote = server.provider != "local";
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(&tr("Manage Server"), &server_display_name(&server));
        header.set_title_widget(Some(&title));
        let close = icon_button("window-close-symbolic", "Close");
        header.pack_end(&close);
        toolbar.add_top_bar(&header);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);

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
            tr("Optionally map server tracks to files on this computer.")
        } else {
            tr("Choose the folder to scan and play directly from this computer.")
        };
        let group = adw::PreferencesGroup::builder()
            .title(group_title)
            .description(group_description)
            .build();
        group.add(&folder_row);
        group.add(&server_prefix);
        group.add(&local_prefix);
        group.add(&sample_row);
        group.add(&preview_row);
        content.append(&group);

        let status = gtk::Label::new(None);
        status.add_css_class("muted");
        status.set_wrap(true);
        status.set_xalign(0.0);
        content.append(&status);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.set_halign(gtk::Align::End);
        let remove = text_button("edit-clear-symbolic", "Remove Local Folder");
        remove.set_visible(server.provider != "local" && access.is_some());
        let save = text_button("document-save-symbolic", "Save");
        save.add_css_class("suggested-action");
        actions.append(&remove);
        actions.append(&save);
        content.append(&actions);

        toolbar.set_content(Some(&content));
        let dialog = adw::Dialog::builder()
            .content_width(560)
            .child(&toolbar)
            .build();

        let dialog_for_close = dialog.clone();
        close.connect_clicked(move |_| {
            dialog_for_close.close();
        });
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
                let changed = draft != initial_draft;
                save.set_sensitive(has_location && changed);
                preview_row.set_subtitle(&preview_local_path_text(
                    access_status.sample_server_path.as_deref(),
                    draft.server_prefix.as_str(),
                    draft.local_prefix.as_str(),
                    draft.folder.as_deref(),
                ));
                status.set_text(&local_access_status_text(
                    &draft,
                    remote,
                    changed,
                    &access_status,
                ));
            }
        });
        connect_folder_button(
            &self.window,
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

        let controller = self.controller.clone();
        let server_id = server.id.clone();
        let dialog_for_remove = dialog.clone();
        remove.connect_clicked(move |_| {
            controller.clear_server_local_access(server_id.clone());
            dialog_for_remove.close();
        });

        let controller = self.controller.clone();
        let server_id = server.id.clone();
        let provider = server.provider.clone();
        let status_for_save = status.clone();
        let dialog_for_save = dialog.clone();
        save.connect_clicked(move |_| {
            let Some(root) = folder.borrow().clone() else {
                status_for_save.set_text(&tr("Choose a local music folder."));
                return;
            };
            if provider == "local" {
                controller.add_local_server(root);
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
            dialog_for_save.close();
        });

        update_state();
        dialog.present(Some(&self.window));
    }

    pub(super) fn add_server_view(self: &Rc<Self>) -> gtk::Widget {
        self.start_server_discovery_once();

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(large_popup_content_width(ADD_SERVER_CLAMP_WIDTH));
        clamp.set_tightening_threshold(360);
        clamp.set_margin_top(36);
        clamp.set_margin_bottom(36);
        clamp.set_margin_start(24);
        clamp.set_margin_end(24);
        clamp.set_valign(gtk::Align::Start);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.add_css_class("first-run-content");
        content.set_hexpand(true);

        let intro = gtk::Box::new(gtk::Orientation::Vertical, 6);
        intro.set_margin_bottom(4);
        let intro_title = gtk::Label::new(Some(&tr("Connect to Music Server")));
        intro_title.add_css_class("title-1");
        intro_title.set_xalign(0.0);
        intro_title.set_wrap(true);
        let intro_description = gtk::Label::new(Some(&tr(
            "Choose a provider, pick a discovered server, or enter the address manually.",
        )));
        intro_description.add_css_class("muted");
        intro_description.set_xalign(0.0);
        intro_description.set_wrap(true);
        intro.append(&intro_title);
        intro.append(&intro_description);
        content.append(&intro);

        let provider_titles = StreamingProvider::ALL
            .iter()
            .map(|provider| tr(provider.title()))
            .collect::<Vec<_>>();
        let provider_title_refs = provider_titles
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let provider_options = gtk::StringList::new(&provider_title_refs);
        let provider = adw::ComboRow::builder()
            .title(tr("Provider"))
            .model(&provider_options)
            .selected(0)
            .build();
        let url = adw::EntryRow::builder().title(tr("Server Address")).build();
        url.set_text("http://");
        let username = adw::EntryRow::builder().title(tr("Username")).build();
        let password = adw::PasswordEntryRow::builder()
            .title(tr("Password"))
            .build();
        let trust = adw::SwitchRow::builder()
            .title(tr("Trust invalid certificate"))
            .subtitle(tr("Only use this for a server you control."))
            .active(false)
            .build();

        let server_group = adw::PreferencesGroup::builder().title(tr("Server")).build();
        server_group.add(&provider);
        server_group.add(&url);
        server_group.add(&username);
        server_group.add(&password);
        server_group.add(&trust);
        content.append(&server_group);

        let local_folder = Rc::new(RefCell::new(None::<PathBuf>));
        let local_group = adw::PreferencesGroup::builder()
            .title(tr("Local Library"))
            .description(tr(
                "Choose a folder to scan and play directly from this computer.",
            ))
            .build();
        let local_folder_row = adw::ActionRow::builder()
            .title(tr("Music Folder"))
            .subtitle(tr("No folder selected"))
            .build();
        let local_folder_button = gtk::Button::with_label(&tr("Choose"));
        local_folder_button.set_valign(gtk::Align::Center);
        local_folder_row.add_suffix(&local_folder_button);
        local_folder_row.set_activatable_widget(Some(&local_folder_button));
        local_group.add(&local_folder_row);
        local_group.set_visible(false);
        content.append(&local_group);

        let access_folder = Rc::new(RefCell::new(None::<PathBuf>));
        let access_group = adw::PreferencesGroup::builder()
            .title(tr("Local Playback Access"))
            .description(tr(
                "Optional. Map server tracks to local files on this computer.",
            ))
            .build();
        let access_folder_row = adw::ActionRow::builder()
            .title(tr("Local Folder"))
            .subtitle(tr("No folder selected"))
            .build();
        let access_folder_button = gtk::Button::with_label(&tr("Choose"));
        access_folder_button.set_valign(gtk::Align::Center);
        access_folder_row.add_suffix(&access_folder_button);
        access_folder_row.set_activatable_widget(Some(&access_folder_button));
        let path_prefix = adw::EntryRow::builder().title(tr("Server Prefix")).build();
        access_group.add(&access_folder_row);
        access_group.add(&path_prefix);
        content.append(&access_group);

        let discovered_group = self.discovered_servers_group(&provider, &url);
        content.append(&discovered_group);

        let status = gtk::Label::new(Some(&self.state.library.borrow().sync_status));
        status.add_css_class("muted");
        status.set_wrap(true);
        status.set_xalign(0.0);
        if let Some(error) = &self.state.library.borrow().last_error {
            status.set_text(error);
            status.add_css_class("error-text");
        }

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        actions.set_halign(gtk::Align::End);
        let login = text_button("network-server-symbolic", "Connect");
        login.add_css_class("suggested-action");
        let controller = self.controller.clone();
        let url_input = url.clone();
        let username_input = username.clone();
        let password_input = password.clone();
        let trust_input = trust.clone();
        let provider_input = provider.clone();
        let local_folder_input = Rc::clone(&local_folder);
        let access_folder_input = Rc::clone(&access_folder);
        let path_prefix_input = path_prefix.clone();
        let status_input = status.clone();
        login.connect_clicked(move |_| {
            let provider = StreamingProvider::from_index(provider_input.selected());
            if provider == StreamingProvider::Local {
                let Some(root) = local_folder_input.borrow().clone() else {
                    status_input.set_text(&tr("Choose a local music folder."));
                    return;
                };
                controller.add_local_server(root);
            } else {
                controller.login(
                    provider,
                    url_input.text().to_string(),
                    username_input.text().to_string(),
                    password_input.text().to_string(),
                    trust_input.is_active(),
                    access_folder_input.borrow().clone(),
                    Some(path_prefix_input.text().to_string()),
                );
            }
        });
        actions.append(&login);
        content.append(&actions);

        content.append(&status);

        let remote_widgets = vec![
            url.clone().upcast::<gtk::Widget>(),
            username.clone().upcast::<gtk::Widget>(),
            password.clone().upcast::<gtk::Widget>(),
            trust.clone().upcast::<gtk::Widget>(),
            access_group.clone().upcast::<gtk::Widget>(),
            discovered_group.clone().upcast::<gtk::Widget>(),
        ];
        update_provider_rows(
            StreamingProvider::from_index(provider.selected()),
            &remote_widgets,
            &local_group,
        );
        update_connect_button(
            StreamingProvider::from_index(provider.selected()),
            &local_folder,
            &login,
        );
        let local_group_for_provider = local_group.clone();
        let login_for_provider = login.clone();
        let local_folder_for_provider = Rc::clone(&local_folder);
        provider.connect_selected_notify(move |row| {
            let provider = StreamingProvider::from_index(row.selected());
            update_provider_rows(provider, &remote_widgets, &local_group_for_provider);
            update_connect_button(provider, &local_folder_for_provider, &login_for_provider);
        });

        connect_folder_button(
            &self.window,
            &local_folder_button,
            &local_folder_row,
            Rc::clone(&local_folder),
            {
                let login = login.clone();
                let provider = provider.clone();
                let local_folder = Rc::clone(&local_folder);
                move |_| {
                    update_connect_button(
                        StreamingProvider::from_index(provider.selected()),
                        &local_folder,
                        &login,
                    );
                }
            },
        );
        connect_folder_button(
            &self.window,
            &access_folder_button,
            &access_folder_row,
            Rc::clone(&access_folder),
            |_| {},
        );

        clamp.set_child(Some(&content));
        scroller.set_child(Some(&clamp));
        scroller.upcast()
    }

    fn start_server_discovery_once(&self) {
        if self.state.server_discovery_started.replace(true) {
            return;
        }
        self.state.server_discovery_running.set(true);
        *self.state.server_discovery_status.borrow_mut() =
            "Searching for Jellyfin servers on the local network...".to_string();
        self.controller.discover_servers();
    }

    fn refresh_server_discovery(self: &Rc<Self>) {
        if self.state.server_discovery_running.get() {
            return;
        }
        self.state.server_discovery_running.set(true);
        *self.state.discovered_servers.borrow_mut() = Vec::new();
        *self.state.server_discovery_status.borrow_mut() =
            "Searching for Jellyfin servers on the local network...".to_string();
        self.controller.discover_servers();
        self.render_current_route();
    }

    fn discovered_servers_group(
        self: &Rc<Self>,
        provider: &adw::ComboRow,
        url: &adw::EntryRow,
    ) -> adw::PreferencesGroup {
        let status = self.state.server_discovery_status.borrow().clone();
        let running = self.state.server_discovery_running.get();
        let servers = self.state.discovered_servers.borrow().clone();
        let group = adw::PreferencesGroup::builder()
            .title(tr("Found Servers"))
            .description(status)
            .build();

        if servers.is_empty() {
            let row_title = if running {
                tr("Searching Local Network")
            } else {
                tr("No Servers Found")
            };
            let row = adw::ActionRow::builder().title(row_title).build();
            row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
            if running {
                let spinner = gtk::Spinner::new();
                spinner.start();
                row.add_suffix(&spinner);
            }
            group.add(&row);
        } else {
            for server in servers {
                let subtitle = format!("{} - {}", server.provider, server.address);
                let row = adw::ActionRow::builder()
                    .title(server.name)
                    .subtitle(subtitle)
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
                row.set_activatable(true);
                let provider = provider.clone();
                let url = url.clone();
                let address = server.address;
                row.connect_activated(move |_| {
                    provider.set_selected(0);
                    url.set_text(&address);
                });
                group.add(&row);
            }
        }

        let search_title = if running {
            tr("Searching...")
        } else {
            tr("Search Again")
        };
        let search = adw::ButtonRow::builder()
            .title(search_title)
            .start_icon_name("view-refresh-symbolic")
            .build();
        search.set_sensitive(!running);
        let shell = Rc::clone(self);
        search.connect_activated(move |_| {
            shell.refresh_server_discovery();
        });
        group.add(&search);

        group
    }
}

fn update_provider_rows(
    provider: StreamingProvider,
    remote_widgets: &[gtk::Widget],
    local_group: &adw::PreferencesGroup,
) {
    let local = provider == StreamingProvider::Local;
    for widget in remote_widgets {
        widget.set_visible(!local);
    }
    local_group.set_visible(local);
}

fn update_connect_button(
    provider: StreamingProvider,
    local_folder: &Rc<RefCell<Option<PathBuf>>>,
    login: &gtk::Button,
) {
    login.set_sensitive(provider != StreamingProvider::Local || local_folder.borrow().is_some());
}

fn connect_folder_button(
    window: &adw::ApplicationWindow,
    button: &gtk::Button,
    row: &adw::ActionRow,
    target: Rc<RefCell<Option<PathBuf>>>,
    on_changed: impl Fn(PathBuf) + 'static,
) {
    let window = window.clone();
    let row = row.clone();
    let on_changed: Rc<dyn Fn(PathBuf)> = Rc::new(on_changed);
    button.connect_clicked(move |_| {
        let window = window.clone();
        let row = row.clone();
        let target = Rc::clone(&target);
        let on_changed = Rc::clone(&on_changed);
        gtk::glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title(tr("Select Music Folder"))
                .build();
            let Ok(folder) = dialog.select_folder_future(Some(&window)).await else {
                return;
            };
            let Some(path) = folder.path() else {
                return;
            };
            row.set_subtitle(&path.display().to_string());
            *target.borrow_mut() = Some(path);
            if let Some(path) = target.borrow().as_ref() {
                on_changed(path.clone());
            }
        });
    });
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
        server_prefix: remote
            .then(|| server_prefix.text().trim().to_string())
            .unwrap_or_default(),
        local_prefix: remote
            .then(|| local_prefix.text().trim().to_string())
            .unwrap_or_default(),
    }
}

fn preview_local_path_text(
    sample_server_path: Option<&str>,
    server_prefix: &str,
    local_prefix: &str,
    folder: Option<&Path>,
) -> String {
    let Some(sample) = sample_server_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return tr("No cached server path yet");
    };
    let server_prefix = server_prefix.trim();
    let local_prefix = local_prefix.trim();
    let base = if local_prefix.is_empty() {
        let Some(folder) = folder else {
            return tr("Choose a local prefix.");
        };
        folder.to_path_buf()
    } else {
        PathBuf::from(local_prefix)
    };

    if !server_prefix.is_empty() {
        if !sample.starts_with(server_prefix) {
            return tr("Server sample does not match the server prefix.");
        }
        let suffix = sample[server_prefix.len()..].trim_start_matches(['/', '\\']);
        return base
            .join(path_from_server_suffix(suffix))
            .to_string_lossy()
            .into_owned();
    }

    let sample_path = Path::new(sample);
    if sample_path.is_relative() {
        return base.join(sample_path).to_string_lossy().into_owned();
    }
    if sample_path.is_file() {
        return sample.to_string();
    }
    tr("Enter a matching server prefix to map this path.")
}

fn local_access_status_text(
    draft: &LocalAccessDraft,
    remote: bool,
    changed: bool,
    status: &LocalAccessStatus,
) -> String {
    if draft.folder.is_none() {
        return tr("Choose a local music folder.");
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
