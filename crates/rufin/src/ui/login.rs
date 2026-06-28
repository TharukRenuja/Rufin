use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use crate::controller::{LoginRequest, ServerDiscoveryStatus};
use crate::i18n::{tr, tr_with};
use crate::providers::StreamingProvider;
use adw::prelude::*;
use domain::ServerId;

use super::{
    AddServerDialogHandle, Shell,
    chrome::window_close_controls,
    folder_selected_text,
    layout::{large_popup_content_height, large_popup_content_width},
    present_light_dismiss_dialog,
    startup_reveal::connection_progress_status_label,
    text_button,
};

const ADD_SERVER_DIALOG_WIDTH: i32 = 620;
const ADD_SERVER_DIALOG_HEIGHT: i32 = 680;
const ADD_SERVER_CLAMP_WIDTH: i32 = 560;
const RECONNECT_NOTICE: &str = "Connect once more to continue using this server.";

#[derive(Clone)]
struct ServerFormPreset {
    server_id: ServerId,
    provider: StreamingProvider,
    url: String,
    username: String,
    trust_invalid_cert: bool,
    use_jellyfin_instant_mix: bool,
}

struct ProviderSelector {
    widget: gtk::Box,
    buttons: Rc<Vec<(StreamingProvider, gtk::Button)>>,
}

#[derive(Clone)]
pub(in crate::ui) struct AddServerDraft {
    provider: StreamingProvider,
    url: String,
    username: String,
    password: String,
    cert_verify: bool,
    use_jellyfin_instant_mix: bool,
    local_folders: Vec<PathBuf>,
}

impl Shell {
    pub(super) fn present_add_server_dialog_closing(self: &Rc<Self>, extra_dialog: &adw::Dialog) {
        let extra_dialog = extra_dialog.clone();
        self.present_server_dialog(Some(Rc::new(move || {
            extra_dialog.close();
        })));
    }

    fn present_server_dialog(self: &Rc<Self>, on_connect_started: Option<Rc<dyn Fn()>>) {
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(&tr("Add Your Music Library"), "");
        header.set_title_widget(Some(&title));
        toolbar.add_top_bar(&header);

        let dialog = adw::Dialog::builder()
            .content_width(large_popup_content_width(ADD_SERVER_DIALOG_WIDTH))
            .content_height(large_popup_content_height(
                self.window.height(),
                ADD_SERVER_DIALOG_HEIGHT,
            ))
            .build();
        let dialog_for_connect = dialog.clone();
        let on_connect_started = on_connect_started.clone();
        let connect_callback: Rc<dyn Fn()> = Rc::new(move || {
            dialog_for_connect.close();
            if let Some(on_connect_started) = on_connect_started.as_ref() {
                on_connect_started();
            }
        });
        let draft = Rc::new(RefCell::new(self.default_add_server_draft()));
        let child =
            self.server_view_handler(Some(Rc::clone(&connect_callback)), Some(Rc::clone(&draft)));
        toolbar.set_content(Some(&child));
        dialog.set_child(Some(&toolbar));
        *self.state.add_server_dialog.borrow_mut() = Some(AddServerDialogHandle {
            toolbar: toolbar.clone(),
            on_connect_started: Some(connect_callback),
            draft,
        });
        let shell = Rc::clone(self);
        dialog.connect_closed(move |_| {
            shell.state.add_server_dialog.borrow_mut().take();
        });
        present_light_dismiss_dialog(&dialog, &self.window);
    }

    pub(super) fn add_server_view(self: &Rc<Self>) -> gtk::Widget {
        self.server_view_handler(None, None)
    }

    pub(super) fn refresh_add_server_dialog(self: &Rc<Self>) {
        let Some(handle) = self.state.add_server_dialog.borrow().clone() else {
            return;
        };
        let child = self.server_view_handler(handle.on_connect_started, Some(handle.draft));
        handle.toolbar.set_content(Some(&child));
    }

    fn server_view_handler(
        self: &Rc<Self>,
        on_connect_started: Option<Rc<dyn Fn()>>,
        draft: Option<Rc<RefCell<AddServerDraft>>>,
    ) -> gtk::Widget {
        let embedded = on_connect_started.is_none();
        if self.state.first_run_connection_pending.get() {
            return self.connection_progress_view();
        }

        let draft = draft.unwrap_or_else(|| Rc::new(RefCell::new(self.default_add_server_draft())));
        let draft_snapshot = draft.borrow().clone();
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

        let selected_provider = Rc::new(Cell::new(draft_snapshot.provider));
        let provider_selector = build_provider_selector(selected_provider.get());
        let url = adw::EntryRow::builder().title(tr("Server Address")).build();
        url.set_text(&draft_snapshot.url);
        let username = adw::EntryRow::builder().title(tr("Username")).build();
        username.set_text(&draft_snapshot.username);
        let password = adw::PasswordEntryRow::builder()
            .title(tr("Password"))
            .build();
        password.set_text(&draft_snapshot.password);
        let cert_verify = adw::SwitchRow::builder()
            .title(tr("Verify server certificate"))
            .subtitle(tr("Turn off only for a server you control"))
            .active(draft_snapshot.cert_verify)
            .build();
        let instant_mix = adw::SwitchRow::builder()
            .title(tr("Use Jellyfin Instant Mix for recommendations"))
            .subtitle(tr("This uses Jellyfin API for play radio, necessary if you want recommendation plugins to work."))
            .active(draft_snapshot.use_jellyfin_instant_mix)
            .build();

        let server_group = adw::PreferencesGroup::builder().title(tr("Server")).build();
        server_group.add(&url);
        server_group.add(&username);
        server_group.add(&password);
        server_group.add(&cert_verify);
        server_group.add(&instant_mix);
        content.append(&provider_selector.widget);
        content.append(&server_group);

        if embedded {
            let private_mode = adw::SwitchRow::builder()
                .title(tr("Private mode"))
                .active(self.state.settings.borrow().private_mode)
                .build();
            let private_shell = Rc::clone(self);
            private_mode.connect_active_notify(move |row| {
                private_shell.set_private_mode(row.is_active());
            });
            let privacy_group = adw::PreferencesGroup::builder()
                .title(tr("Privacy and Security"))
                .build();
            privacy_group.add(&private_mode);
            content.append(&privacy_group);
        }

        let local_folders = Rc::new(RefCell::new(draft_snapshot.local_folders.clone()));
        let local_group = adw::PreferencesGroup::builder()
            .title(tr("Local Library"))
            .description(tr(
                "Choose one or more folders to scan and play directly from this computer",
            ))
            .build();
        let local_folder_row = adw::ActionRow::builder()
            .title(tr("Music Folders"))
            .subtitle(local_folders_subtitle(&local_folders.borrow()))
            .build();
        let local_folder_button = gtk::Button::with_label(&tr("Choose"));
        local_folder_button.set_valign(gtk::Align::Center);
        local_folder_row.add_suffix(&local_folder_button);
        local_folder_row.set_activatable_widget(Some(&local_folder_button));
        local_group.add(&local_folder_row);
        let add_local_folder_row = adw::ActionRow::builder()
            .title(tr("Add Folder"))
            .subtitle(tr("Add another folder to the Local source"))
            .build();
        let add_local_folder_button = gtk::Button::with_label(&tr("Add"));
        add_local_folder_button.set_valign(gtk::Align::Center);
        add_local_folder_row.add_suffix(&add_local_folder_button);
        add_local_folder_row.set_activatable_widget(Some(&add_local_folder_button));
        local_group.add(&add_local_folder_row);
        local_group.set_visible(false);
        content.append(&local_group);

        let discovered_group = self.discovered_servers_group(
            &selected_provider,
            &provider_selector.buttons,
            &url,
            &draft,
        );
        content.append(&discovered_group);

        let status = gtk::Label::new(None);
        status.add_css_class("muted");
        status.set_wrap(true);
        status.set_xalign(0.0);
        status.set_visible(false);
        if let Some(error) = &self.state.library.borrow().last_error {
            status.set_text(error);
            status.add_css_class("error-text");
            status.set_visible(true);
        }

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        actions.set_halign(gtk::Align::End);
        let login = text_button("network-server-symbolic", "Connect");
        login.add_css_class("suggested-action");
        connect_entry_row_activation(&url, &login);
        connect_entry_row_activation(&username, &login);
        connect_password_entry_row_activation(&password, &login);
        provider_selector
            .widget
            .add_controller(local_provider_enter_controller(&selected_provider, &login));
        let controller = self.controller.clone();
        let url_input = url.clone();
        let username_input = username.clone();
        let password_input = password.clone();
        let cert_verify_input = cert_verify.clone();
        let instant_mix_input = instant_mix.clone();
        let provider_input = Rc::clone(&selected_provider);
        let local_folders_input = Rc::clone(&local_folders);
        let status_input = status.clone();
        let shell = Rc::clone(self);
        let connect_attempt_started = Rc::new(Cell::new(false));
        let connect_attempt_started_for_click = Rc::clone(&connect_attempt_started);
        let on_connect_started_for_click = on_connect_started.clone();
        let login_for_click = login.clone();
        login.connect_clicked(move |_| {
            let provider = provider_input.get();
            let accept_connect_attempt = |message: &str| {
                connect_attempt_started_for_click.set(true);
                status_input.remove_css_class("error-text");
                status_input.set_text(message);
                status_input.set_visible(true);
                login_for_click.set_sensitive(false);
                if let Some(on_connect_started) = on_connect_started_for_click.as_ref() {
                    on_connect_started();
                }
            };
            if provider == StreamingProvider::Local {
                let roots = local_folders_input.borrow().clone();
                if roots.is_empty() {
                    status_input.set_text(&tr("Choose at least one local music folder"));
                    status_input.set_visible(true);
                    return;
                }
                let message = tr("Caching local library...");
                accept_connect_attempt(&message);
                shell.begin_first_run_connection(&message);
                controller.add_local_server_folders(roots);
            } else {
                if !remote_login_ready(&url_input, &username_input, &password_input) {
                    status_input.set_text(&tr("Enter a server address, username, and password"));
                    status_input.set_visible(true);
                    return;
                }
                let message = tr("Connecting to music server...");
                accept_connect_attempt(&message);
                shell.begin_first_run_connection(&message);
                controller.login(LoginRequest {
                    provider,
                    server_url: url_input.text().to_string(),
                    username: username_input.text().to_string(),
                    password: password_input.text().to_string(),
                    trust_invalid_cert: !cert_verify_input.is_active(),
                    use_jellyfin_instant_mix: provider == StreamingProvider::Jellyfin
                        && instant_mix_input.is_active(),
                    local_access_root: None,
                    path_replace_from: None,
                });
            }
        });
        connect_add_server_status_watcher(AddServerStatusWatcher {
            shell: self,
            status: &status,
            login: &login,
            selected_provider: &selected_provider,
            local_folders: &local_folders,
            url: &url,
            username: &username,
            password: &password,
            connect_attempt_started,
        });
        actions.append(&login);
        content.append(&actions);

        content.append(&status);

        let remote_widgets = vec![
            server_group.clone().upcast::<gtk::Widget>(),
            discovered_group.clone().upcast::<gtk::Widget>(),
        ];
        let jellyfin_widgets = vec![instant_mix.clone().upcast::<gtk::Widget>()];
        update_provider_rows(
            selected_provider.get(),
            &remote_widgets,
            &jellyfin_widgets,
            &local_group,
        );
        update_connect_button(
            selected_provider.get(),
            &local_folders,
            &url,
            &username,
            &password,
            &login,
        );
        let refresh_connect_button: Rc<dyn Fn()> = Rc::new({
            let selected_provider = Rc::clone(&selected_provider);
            let local_folders = Rc::clone(&local_folders);
            let url = url.clone();
            let username = username.clone();
            let password = password.clone();
            let login = login.clone();
            move || {
                update_connect_button(
                    selected_provider.get(),
                    &local_folders,
                    &url,
                    &username,
                    &password,
                    &login,
                );
            }
        });
        let local_group_for_provider = local_group.clone();
        for (provider, button) in provider_selector.buttons.iter() {
            let provider = *provider;
            let selected_provider = Rc::clone(&selected_provider);
            let provider_buttons = Rc::clone(&provider_selector.buttons);
            let remote_widgets = remote_widgets.clone();
            let jellyfin_widgets = jellyfin_widgets.clone();
            let local_group = local_group_for_provider.clone();
            let refresh = Rc::clone(&refresh_connect_button);
            let draft = Rc::clone(&draft);
            button.connect_clicked(move |_| {
                select_provider(&selected_provider, &provider_buttons, provider);
                draft.borrow_mut().provider = provider;
                update_provider_rows(provider, &remote_widgets, &jellyfin_widgets, &local_group);
                refresh();
            });
        }
        {
            let refresh = Rc::clone(&refresh_connect_button);
            let draft = Rc::clone(&draft);
            url.connect_text_notify(move |entry| {
                draft.borrow_mut().url = entry.text().to_string();
                refresh();
            });
        }
        {
            let refresh = Rc::clone(&refresh_connect_button);
            let draft = Rc::clone(&draft);
            username.connect_text_notify(move |entry| {
                draft.borrow_mut().username = entry.text().to_string();
                refresh();
            });
        }
        {
            let refresh = Rc::clone(&refresh_connect_button);
            let draft = Rc::clone(&draft);
            password.connect_text_notify(move |entry| {
                draft.borrow_mut().password = entry.text().to_string();
                refresh();
            });
        }
        {
            let draft = Rc::clone(&draft);
            cert_verify.connect_active_notify(move |row| {
                draft.borrow_mut().cert_verify = row.is_active();
            });
        }
        {
            let draft = Rc::clone(&draft);
            instant_mix.connect_active_notify(move |row| {
                draft.borrow_mut().use_jellyfin_instant_mix = row.is_active();
            });
        }

        connect_folder_button(
            &self.window,
            &local_folder_button,
            &local_folder_row,
            Rc::new(RefCell::new(local_folders.borrow().first().cloned())),
            {
                let login = login.clone();
                let selected_provider = Rc::clone(&selected_provider);
                let local_folders = Rc::clone(&local_folders);
                let local_folder_row = local_folder_row.clone();
                let url = url.clone();
                let username = username.clone();
                let password = password.clone();
                let draft = Rc::clone(&draft);
                move |path| {
                    replace_primary_local_folder(&local_folders, path);
                    draft.borrow_mut().local_folders = local_folders.borrow().clone();
                    local_folder_row.set_subtitle(&local_folders_subtitle(&local_folders.borrow()));
                    update_connect_button(
                        selected_provider.get(),
                        &local_folders,
                        &url,
                        &username,
                        &password,
                        &login,
                    );
                }
            },
        );
        connect_add_local_folder_button(
            &self.window,
            &add_local_folder_button,
            &local_folder_row,
            Rc::clone(&local_folders),
            {
                let login = login.clone();
                let selected_provider = Rc::clone(&selected_provider);
                let local_folders = Rc::clone(&local_folders);
                let url = url.clone();
                let username = username.clone();
                let password = password.clone();
                let draft = Rc::clone(&draft);
                move || {
                    draft.borrow_mut().local_folders = local_folders.borrow().clone();
                    update_connect_button(
                        selected_provider.get(),
                        &local_folders,
                        &url,
                        &username,
                        &password,
                        &login,
                    );
                }
            },
        );
        clamp.set_child(Some(&content));
        scroller.set_child(Some(&clamp));
        let view = scroller.upcast::<gtk::Widget>();
        if embedded {
            return connect_view_with_close_controls(view);
        }
        view
    }

    pub(super) fn show_reconnect_notice_if_needed(&self) {
        let Some(preset) = self.saved_server_form_preset() else {
            return;
        };
        let mut shown = self.state.reconnect_toasts_shown.borrow_mut();
        if shown.insert(preset.server_id) {
            self.quick_toast_overlay
                .add_toast(adw::Toast::new(&tr(RECONNECT_NOTICE)));
        }
    }

    fn saved_server_form_preset(&self) -> Option<ServerFormPreset> {
        let library = self.state.library.borrow();
        if !library.first_run {
            return None;
        }
        let server = library.server.as_ref()?;
        let provider = StreamingProvider::from_provider_id(&server.provider)?;
        if provider == StreamingProvider::Local {
            return None;
        }
        let trust_invalid_cert = library
            .server_local_access
            .iter()
            .find(|status| status.server_id == server.id)
            .is_some_and(|status| status.trust_invalid_cert);
        let use_jellyfin_instant_mix = library
            .server_local_access
            .iter()
            .find(|status| status.server_id == server.id)
            .is_some_and(|status| status.use_jellyfin_instant_mix);
        Some(ServerFormPreset {
            server_id: server.id.clone(),
            provider,
            url: server.base_url.clone(),
            username: library.username.clone().unwrap_or_default(),
            trust_invalid_cert,
            use_jellyfin_instant_mix,
        })
    }

    fn default_add_server_draft(&self) -> AddServerDraft {
        if let Some(preset) = self.saved_server_form_preset() {
            return AddServerDraft {
                provider: preset.provider,
                url: preset.url,
                username: preset.username,
                password: String::new(),
                cert_verify: !preset.trust_invalid_cert,
                use_jellyfin_instant_mix: preset.use_jellyfin_instant_mix,
                local_folders: default_music_folder().into_iter().collect(),
            };
        }
        AddServerDraft {
            provider: StreamingProvider::Jellyfin,
            url: "http://".to_string(),
            username: String::new(),
            password: String::new(),
            cert_verify: true,
            use_jellyfin_instant_mix: false,
            local_folders: default_music_folder().into_iter().collect(),
        }
    }

    fn begin_first_run_connection(self: &Rc<Self>, status: &str) {
        self.state.first_run_connection_pending.set(true);
        self.state.first_run_connection_ready.set(false);
        {
            let mut library = self.state.library.borrow_mut();
            library.sync_status = status.to_string();
            library.last_error = None;
        }
        self.render_current_route();
    }

    fn connection_progress_view(self: &Rc<Self>) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(440);
        clamp.set_tightening_threshold(320);
        clamp.set_margin_top(72);
        clamp.set_margin_bottom(72);
        clamp.set_margin_start(24);
        clamp.set_margin_end(24);
        clamp.set_valign(gtk::Align::Center);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
        content.add_css_class("first-run-progress");
        content.set_halign(gtk::Align::Center);
        content.set_valign(gtk::Align::Center);
        content.set_hexpand(true);

        let spinner = gtk::Spinner::new();
        spinner.set_halign(gtk::Align::Center);
        spinner.start();
        content.append(&spinner);

        let title = gtk::Label::new(Some(&tr("Caching Library")));
        title.add_css_class("title-1");
        title.set_justify(gtk::Justification::Center);
        title.set_wrap(true);
        content.append(&title);

        let status_text = {
            let sync_status = self.state.library.borrow().sync_status.clone();
            connection_progress_status_label(&sync_status).unwrap_or_default()
        };
        let status = gtk::Label::new(Some(&status_text));
        status.add_css_class("muted");
        status.set_justify(gtk::Justification::Center);
        status.set_wrap(true);
        status.set_xalign(0.5);
        content.append(&status);

        clamp.set_child(Some(&content));
        scroller.set_child(Some(&clamp));
        scroller.upcast()
    }

    fn start_server_discovery_once(&self) {
        if self.state.server_discovery_started.replace(true) {
            return;
        }
        self.state.server_discovery_running.set(true);
        *self.state.server_discovery_status.borrow_mut() = ServerDiscoveryStatus::Searching;
        self.controller.discover_servers();
    }

    fn refresh_server_discovery(self: &Rc<Self>) {
        if self.state.server_discovery_running.get() {
            return;
        }
        self.state.server_discovery_running.set(true);
        *self.state.discovered_servers.borrow_mut() = Vec::new();
        *self.state.server_discovery_status.borrow_mut() = ServerDiscoveryStatus::Searching;
        self.controller.discover_servers();
        self.render_current_route();
    }

    fn discovered_servers_group(
        self: &Rc<Self>,
        selected_provider: &Rc<Cell<StreamingProvider>>,
        provider_buttons: &Rc<Vec<(StreamingProvider, gtk::Button)>>,
        url: &adw::EntryRow,
        draft: &Rc<RefCell<AddServerDraft>>,
    ) -> adw::PreferencesGroup {
        let status = self.state.server_discovery_status.borrow().clone();
        let running = self.state.server_discovery_running.get();
        let servers = self.state.discovered_servers.borrow().clone();
        let group = if servers.is_empty() {
            adw::PreferencesGroup::builder()
                .title(tr("Found Servers"))
                .description(discovery_status_label(&status))
                .build()
        } else {
            adw::PreferencesGroup::builder()
                .title(tr("Found Servers"))
                .build()
        };

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
                row.add_prefix(&gtk::Image::from_icon_name(
                    "io.github.screwys.Rufin.provider.jellyfin",
                ));
                row.set_activatable(true);
                let selected_provider = Rc::clone(selected_provider);
                let provider_buttons = Rc::clone(provider_buttons);
                let url = url.clone();
                let address = server.address;
                let draft = Rc::clone(draft);
                row.connect_activated(move |_| {
                    select_provider(
                        &selected_provider,
                        &provider_buttons,
                        StreamingProvider::Jellyfin,
                    );
                    {
                        let mut draft = draft.borrow_mut();
                        draft.provider = StreamingProvider::Jellyfin;
                        draft.url = address.clone();
                    }
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

fn discovery_status_label(status: &ServerDiscoveryStatus) -> String {
    match status {
        ServerDiscoveryStatus::Idle => tr("Searching will start automatically"),
        ServerDiscoveryStatus::Searching => {
            tr("Searching for Jellyfin servers on the local network...")
        }
        ServerDiscoveryStatus::Empty => {
            tr("No Jellyfin servers found. Enter the address manually or search again")
        }
        ServerDiscoveryStatus::Found(_) => String::new(),
        ServerDiscoveryStatus::Failed(error) => {
            tr_with("Server discovery failed: {error}", &[("error", error)])
        }
    }
}

fn build_provider_selector(selected: StreamingProvider) -> ProviderSelector {
    let wrapper = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    wrapper.add_css_class("provider-choice-list");
    wrapper.set_homogeneous(true);
    wrapper.set_hexpand(true);

    let mut buttons = Vec::new();
    for provider in StreamingProvider::ALL {
        let button = provider_choice_button(provider, provider == selected);
        wrapper.append(&button);
        buttons.push((provider, button));
    }

    ProviderSelector {
        widget: wrapper,
        buttons: Rc::new(buttons),
    }
}

fn provider_choice_button(provider: StreamingProvider, active: bool) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("provider-choice-button");
    set_provider_choice_active(&button, active);
    button.update_property(&[gtk::accessible::Property::Label(&provider_choice_title(
        provider,
    ))]);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    content.set_halign(gtk::Align::Center);
    content.set_valign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(provider_choice_icon_name(provider));
    icon.set_pixel_size(34);
    icon.set_size_request(34, 34);
    icon.set_halign(gtk::Align::Center);
    content.append(&icon);

    let label = gtk::Label::new(Some(&provider_choice_title(provider)));
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(14);
    content.append(&label);

    button.set_child(Some(&content));
    button
}

fn select_provider(
    selected_provider: &Rc<Cell<StreamingProvider>>,
    buttons: &[(StreamingProvider, gtk::Button)],
    provider: StreamingProvider,
) {
    selected_provider.set(provider);
    for (candidate, button) in buttons {
        set_provider_choice_active(button, *candidate == provider);
    }
}

fn set_provider_choice_active(button: &gtk::Button, active: bool) {
    if active {
        button.add_css_class("active");
    } else {
        button.remove_css_class("active");
    }
}

fn provider_choice_title(provider: StreamingProvider) -> String {
    match provider {
        StreamingProvider::Subsonic => tr("OpenSubsonic"),
        _ => tr(provider.title()),
    }
}

fn provider_choice_icon_name(provider: StreamingProvider) -> &'static str {
    match provider {
        StreamingProvider::Jellyfin => "io.github.screwys.Rufin.provider.jellyfin",
        StreamingProvider::Navidrome => "io.github.screwys.Rufin.provider.navidrome",
        StreamingProvider::Subsonic => "io.github.screwys.Rufin.provider.opensubsonic",
        StreamingProvider::Local => "route-folders-symbolic",
    }
}

fn update_provider_rows(
    provider: StreamingProvider,
    remote_widgets: &[gtk::Widget],
    jellyfin_widgets: &[gtk::Widget],
    local_group: &adw::PreferencesGroup,
) {
    let local = provider == StreamingProvider::Local;
    for widget in remote_widgets {
        widget.set_visible(!local);
    }
    let jellyfin = provider == StreamingProvider::Jellyfin;
    for widget in jellyfin_widgets {
        widget.set_visible(!local && jellyfin);
    }
    local_group.set_visible(local);
}

fn update_connect_button(
    provider: StreamingProvider,
    local_folders: &Rc<RefCell<Vec<PathBuf>>>,
    url: &adw::EntryRow,
    username: &adw::EntryRow,
    password: &adw::PasswordEntryRow,
    login: &gtk::Button,
) {
    let ready = if provider == StreamingProvider::Local {
        !local_folders.borrow().is_empty()
    } else {
        remote_login_ready(url, username, password)
    };
    login.set_sensitive(ready);
}

struct AddServerStatusWatcher<'a> {
    shell: &'a Rc<Shell>,
    status: &'a gtk::Label,
    login: &'a gtk::Button,
    selected_provider: &'a Rc<Cell<StreamingProvider>>,
    local_folders: &'a Rc<RefCell<Vec<PathBuf>>>,
    url: &'a adw::EntryRow,
    username: &'a adw::EntryRow,
    password: &'a adw::PasswordEntryRow,
    connect_attempt_started: Rc<Cell<bool>>,
}

fn connect_add_server_status_watcher(watcher: AddServerStatusWatcher<'_>) {
    let AddServerStatusWatcher {
        shell,
        status,
        login,
        selected_provider,
        local_folders,
        url,
        username,
        password,
        connect_attempt_started,
    } = watcher;
    let shell = Rc::clone(shell);
    let status = status.clone();
    let login = login.clone();
    let selected_provider = Rc::clone(selected_provider);
    let local_folders = Rc::clone(local_folders);
    let url = url.clone();
    let username = username.clone();
    let password = password.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
        if status.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }

        let pending = shell.state.first_run_connection_pending.get();
        let (sync_status, last_error) = {
            let library = shell.state.library.borrow();
            (library.sync_status.clone(), library.last_error.clone())
        };

        if pending {
            status.remove_css_class("error-text");
            let text = connection_progress_status_label(&sync_status).unwrap_or_default();
            status.set_text(&text);
            status.set_visible(!text.trim().is_empty());
            login.set_sensitive(false);
            return gtk::glib::ControlFlow::Continue;
        }

        if let Some(error) = last_error {
            connect_attempt_started.set(false);
            status.set_text(&error);
            status.add_css_class("error-text");
            status.set_visible(true);
            update_connect_button(
                selected_provider.get(),
                &local_folders,
                &url,
                &username,
                &password,
                &login,
            );
            return gtk::glib::ControlFlow::Continue;
        }

        if connect_attempt_started.get() {
            return gtk::glib::ControlFlow::Break;
        }

        gtk::glib::ControlFlow::Continue
    });
}

fn connect_entry_row_activation(entry: &adw::EntryRow, login: &gtk::Button) {
    let login = login.clone();
    entry.connect_entry_activated(move |_| {
        activate_connect_if_ready(&login);
    });
}

fn connect_password_entry_row_activation(entry: &adw::PasswordEntryRow, login: &gtk::Button) {
    let login = login.clone();
    entry.connect_entry_activated(move |_| {
        activate_connect_if_ready(&login);
    });
}

fn local_provider_enter_controller(
    selected_provider: &Rc<Cell<StreamingProvider>>,
    login: &gtk::Button,
) -> gtk::EventControllerKey {
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let login = login.clone();
    let selected_provider = Rc::clone(selected_provider);
    controller.connect_key_pressed(move |_, key, _, _| {
        let local = selected_provider.get() == StreamingProvider::Local;
        let enter = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
        if local && enter && activate_connect_if_ready(&login) {
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    controller
}

fn activate_connect_if_ready(login: &gtk::Button) -> bool {
    if !login.is_sensitive() {
        return false;
    }
    login.emit_clicked();
    true
}

fn remote_login_ready(
    url: &adw::EntryRow,
    username: &adw::EntryRow,
    password: &adw::PasswordEntryRow,
) -> bool {
    let address = url.text();
    let address = address.trim().trim_end_matches('/');
    let address_without_scheme = address
        .strip_prefix("http://")
        .or_else(|| address.strip_prefix("https://"))
        .unwrap_or(address);
    !address_without_scheme.trim().is_empty()
        && !username.text().trim().is_empty()
        && !password.text().trim().is_empty()
}

fn default_music_folder() -> Option<PathBuf> {
    let user_dirs = directories::UserDirs::new()?;
    Some(
        user_dirs
            .audio_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| user_dirs.home_dir().join("Music")),
    )
}

fn path_subtitle(path: &Path) -> String {
    path.display().to_string()
}

fn local_folders_subtitle(folders: &[PathBuf]) -> String {
    match folders {
        [] => tr("No folders selected"),
        [folder] => path_subtitle(folder),
        folders => folder_selected_text(folders.len() as u64),
    }
}

fn replace_primary_local_folder(folders: &Rc<RefCell<Vec<PathBuf>>>, path: PathBuf) {
    let mut folders = folders.borrow_mut();
    if let Some(index) = folders.iter().position(|folder| folder == &path) {
        if index != 0 {
            folders.remove(index);
            folders.insert(0, path);
        }
        return;
    }
    if folders.is_empty() {
        folders.push(path);
    } else {
        folders[0] = path;
    }
}

fn append_local_folder(folders: &Rc<RefCell<Vec<PathBuf>>>, path: PathBuf) {
    let mut folders = folders.borrow_mut();
    if !folders.iter().any(|folder| folder == &path) {
        folders.push(path);
    }
}

pub(super) fn connect_folder_button(
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
            let selected_folder = target.borrow().as_ref().map(gtk::gio::File::for_path);
            let dialog = gtk::FileDialog::builder()
                .title(tr("Select Music Folder"))
                .build();
            if let Some(folder) = selected_folder.as_ref() {
                dialog.set_initial_folder(Some(folder));
            }
            let Ok(folder) = dialog.select_folder_future(Some(&window)).await else {
                return;
            };
            let Some(path) = folder.path() else {
                return;
            };
            row.set_subtitle(&path_subtitle(&path));
            *target.borrow_mut() = Some(path);
            if let Some(path) = target.borrow().as_ref() {
                on_changed(path.clone());
            }
        });
    });
}

fn connect_add_local_folder_button(
    window: &adw::ApplicationWindow,
    button: &gtk::Button,
    row: &adw::ActionRow,
    folders: Rc<RefCell<Vec<PathBuf>>>,
    on_changed: impl Fn() + 'static,
) {
    let window = window.clone();
    let row = row.clone();
    let on_changed: Rc<dyn Fn()> = Rc::new(on_changed);
    button.connect_clicked(move |_| {
        let window = window.clone();
        let row = row.clone();
        let folders = Rc::clone(&folders);
        let on_changed = Rc::clone(&on_changed);
        gtk::glib::spawn_future_local(async move {
            let selected_folder = folders.borrow().last().map(gtk::gio::File::for_path);
            let dialog = gtk::FileDialog::builder()
                .title(tr("Select Music Folder"))
                .build();
            if let Some(folder) = selected_folder.as_ref() {
                dialog.set_initial_folder(Some(folder));
            }
            let Ok(folder) = dialog.select_folder_future(Some(&window)).await else {
                return;
            };
            let Some(path) = folder.path() else {
                return;
            };
            append_local_folder(&folders, path);
            row.set_subtitle(&local_folders_subtitle(&folders.borrow()));
            on_changed();
        });
    });
}

fn connect_view_with_close_controls(view: gtk::Widget) -> gtk::Widget {
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.set_child(Some(&view));
    let controls = window_close_controls();
    overlay.add_overlay(&controls);
    overlay.set_measure_overlay(&controls, false);
    overlay.upcast()
}
