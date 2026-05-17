use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;

use crate::i18n::tr;
use crate::providers::StreamingProvider;

use super::{Shell, text_button};

impl Shell {
    pub(super) fn present_add_server_dialog(self: &Rc<Self>) {
        let child = self.add_server_view();
        let dialog = adw::Dialog::builder()
            .content_width(620)
            .content_height(720)
            .child(&child)
            .build();
        dialog.present(Some(&self.window));
    }

    pub(super) fn add_server_view(self: &Rc<Self>) -> gtk::Widget {
        self.start_server_discovery_once();

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(560);
        clamp.set_tightening_threshold(360);
        clamp.set_margin_top(36);
        clamp.set_margin_bottom(36);
        clamp.set_margin_start(24);
        clamp.set_margin_end(24);
        clamp.set_valign(gtk::Align::Center);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.add_css_class("first-run-content");
        content.set_hexpand(true);

        let intro = adw::StatusPage::builder()
            .icon_name("network-server-symbolic")
            .title(tr("Connect to Music Server"))
            .description(tr(
                "Choose a provider, pick a discovered server, or enter the address manually.",
            ))
            .build();
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
            .title(tr("Local Folder Access"))
            .description(tr(
                "Optional. Match server paths to local files for sidecar lyrics and exports.",
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
        let path_prefix = adw::EntryRow::builder()
            .title(tr("Server Path Prefix"))
            .build();
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
        let local_group_for_provider = local_group.clone();
        provider.connect_selected_notify(move |row| {
            let provider = StreamingProvider::from_index(row.selected());
            update_provider_rows(provider, &remote_widgets, &local_group_for_provider);
        });

        connect_folder_button(
            &self.window,
            &local_folder_button,
            &local_folder_row,
            Rc::clone(&local_folder),
        );
        connect_folder_button(
            &self.window,
            &access_folder_button,
            &access_folder_row,
            Rc::clone(&access_folder),
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

fn connect_folder_button(
    window: &adw::ApplicationWindow,
    button: &gtk::Button,
    row: &adw::ActionRow,
    target: Rc<RefCell<Option<PathBuf>>>,
) {
    let window = window.clone();
    let row = row.clone();
    button.connect_clicked(move |_| {
        let window = window.clone();
        let row = row.clone();
        let target = Rc::clone(&target);
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
        });
    });
}
