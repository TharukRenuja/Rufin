use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use library::SavedSource;

use crate::controller::{AppController, ServerDiscoveryStatus};
use crate::i18n::{tr, tr_with};
use crate::sources::{
    CredentialHostInput, CredentialHostPreset, SourceRegistration, default_source_registration,
    resolve_source_registration, source_registrations,
};

use super::{
    AddServerDialogHandle, Shell,
    chrome::window_close_controls,
    folder_selected_text,
    layout::{
        compact_field_row_group, install_compact_field_row_responsiveness,
        large_popup_content_width, style_compact_field_row,
    },
    local_access_mapping::confirm_forget_source,
    startup_reveal::connection_progress_status_label,
    text_button,
};

const ADD_SERVER_CLAMP_WIDTH: i32 = 560;
const RECONNECT_NOTICE: &str = "Connect once more to continue using this server.";

pub(crate) trait SourceSetupFlow {
    fn view(&self, shell: &Rc<Shell>, context: &SetupViewContext) -> gtk::Widget;
}

#[derive(Clone)]
pub(crate) struct SetupViewContext {
    content: gtk::Box,
    flow: Rc<RefCell<Rc<dyn SourceSetupFlow>>>,
    on_connect_started: Option<Rc<dyn Fn()>>,
}

#[derive(Clone, Debug)]
struct CredentialHostDraft {
    name: String,
    url: String,
    username: String,
    password: String,
    cert_verify: bool,
}

struct CredentialSetupFlow {
    registration: &'static SourceRegistration,
    draft: Rc<RefCell<CredentialHostDraft>>,
    submit: Rc<dyn Fn(&AppController, CredentialHostInput)>,
}

struct JellyfinSetupFlow {
    registration: &'static SourceRegistration,
    draft: Rc<RefCell<CredentialHostDraft>>,
    use_instant_mix: Rc<Cell<bool>>,
    submit: Rc<dyn Fn(&AppController, CredentialHostInput, bool)>,
}

struct LocalSetupFlow {
    registration: &'static SourceRegistration,
    folders: Rc<RefCell<Vec<PathBuf>>>,
    submit: Rc<dyn Fn(&AppController, Vec<PathBuf>)>,
}

struct CredentialHost {
    widget: gtk::Box,
    name: adw::EntryRow,
    url: adw::EntryRow,
    username: adw::EntryRow,
    password: adw::PasswordEntryRow,
    cert_verify: adw::SwitchRow,
}

impl CredentialHost {
    fn input(&self) -> CredentialHostInput {
        CredentialHostInput {
            server_name: trimmed_optional_text(&self.name),
            server_url: self.url.text().to_string(),
            username: self.username.text().to_string(),
            password: self.password.text().to_string(),
            trust_invalid_cert: !self.cert_verify.is_active(),
        }
    }

    fn ready(&self) -> bool {
        remote_login_ready(&self.url, &self.username, &self.password)
    }
}

impl Shell {
    pub(in crate::ui) fn add_server_navigation_page(
        self: &Rc<Self>,
        _navigation: &adw::NavigationView,
        preferences_dialog: &adw::Dialog,
    ) -> adw::NavigationPage {
        let preferences_dialog_for_connect = preferences_dialog.clone();
        let connect_callback: Rc<dyn Fn()> = Rc::new(move || {
            preferences_dialog_for_connect.close();
        });
        let context = self.setup_view_context(Some(connect_callback));
        render_setup_flow(self, &context);
        *self.state.add_server_dialog.borrow_mut() = Some(AddServerDialogHandle {
            content: context.content.clone(),
            on_connect_started: context.on_connect_started.clone(),
            flow: Rc::clone(&context.flow),
        });
        adw::NavigationPage::new(&context.content, &tr("Add server"))
    }

    pub(super) fn add_server_view(self: &Rc<Self>) -> gtk::Widget {
        if self.state.first_run_connection_pending.get() {
            return self.connection_progress_view();
        }
        let context = self.setup_view_context(None);
        render_setup_flow(self, &context);
        context.content.upcast()
    }

    pub(super) fn refresh_add_server_dialog(self: &Rc<Self>) {
        let Some(handle) = self.state.add_server_dialog.borrow().clone() else {
            return;
        };
        if handle.content.root().is_none() {
            self.state.add_server_dialog.borrow_mut().take();
            return;
        }
        render_setup_flow(
            self,
            &SetupViewContext {
                content: handle.content,
                flow: handle.flow,
                on_connect_started: handle.on_connect_started,
            },
        );
    }

    fn setup_view_context(
        self: &Rc<Self>,
        on_connect_started: Option<Rc<dyn Fn()>>,
    ) -> SetupViewContext {
        let flow = self.default_source_setup_flow();
        SetupViewContext {
            content: gtk::Box::new(gtk::Orientation::Vertical, 0),
            flow: Rc::new(RefCell::new(flow)),
            on_connect_started,
        }
    }

    fn default_source_setup_flow(self: &Rc<Self>) -> Rc<dyn SourceSetupFlow> {
        let registration = {
            let library = self.state.library.borrow();
            library
                .first_run
                .then_some(())
                .and_then(|()| library.source.as_ref())
                .and_then(|source| resolve_source_registration(&source.kind))
                .unwrap_or_else(default_source_registration)
        };
        (registration.new_setup_flow)(self)
    }

    pub(crate) fn reconnect_saved_source(
        &self,
        registration: &'static SourceRegistration,
    ) -> Option<SavedSource> {
        let library = self.state.library.borrow();
        if !library.first_run {
            return None;
        }
        let source = library.source.as_ref()?;
        let resolved = resolve_source_registration(&source.kind)?;
        same_registration(resolved, registration)
            .then(|| self.controller.configured_source(&source.id))
            .flatten()
    }

    pub(super) fn show_reconnect_notice_if_needed(&self) {
        let library = self.state.library.borrow();
        if !library.first_run {
            return;
        }
        let Some(source) = library.source.as_ref() else {
            return;
        };
        if resolve_source_registration(&source.kind).is_none() {
            return;
        }
        let mut shown = self.state.reconnect_toasts_shown.borrow_mut();
        if shown.insert(source.id.clone()) {
            self.quick_toast_overlay
                .add_toast(adw::Toast::new(&tr(RECONNECT_NOTICE)));
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

        let sync_status = self.state.library.borrow().sync_status.clone();
        let status_text = connection_progress_status_label(&sync_status).unwrap_or_default();
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
}

pub(crate) fn new_local_source_setup_flow(
    _shell: &Rc<Shell>,
    registration: &'static SourceRegistration,
    submit: impl Fn(&AppController, Vec<PathBuf>) + 'static,
) -> Rc<dyn SourceSetupFlow> {
    Rc::new(LocalSetupFlow {
        registration,
        folders: Rc::new(RefCell::new(Vec::new())),
        submit: Rc::new(submit),
    })
}

pub(crate) fn new_jellyfin_source_setup_flow(
    _shell: &Rc<Shell>,
    registration: &'static SourceRegistration,
    preset: Option<CredentialHostPreset>,
    use_instant_mix: bool,
    submit: impl Fn(&AppController, CredentialHostInput, bool) + 'static,
) -> Rc<dyn SourceSetupFlow> {
    Rc::new(JellyfinSetupFlow {
        registration,
        draft: Rc::new(RefCell::new(credential_draft(preset))),
        use_instant_mix: Rc::new(Cell::new(use_instant_mix)),
        submit: Rc::new(submit),
    })
}

pub(crate) fn new_credential_source_setup_flow(
    _shell: &Rc<Shell>,
    registration: &'static SourceRegistration,
    preset: Option<CredentialHostPreset>,
    submit: impl Fn(&AppController, CredentialHostInput) + 'static,
) -> Rc<dyn SourceSetupFlow> {
    Rc::new(CredentialSetupFlow {
        registration,
        draft: Rc::new(RefCell::new(credential_draft(preset))),
        submit: Rc::new(submit),
    })
}

impl SourceSetupFlow for CredentialSetupFlow {
    fn view(&self, shell: &Rc<Shell>, context: &SetupViewContext) -> gtk::Widget {
        let (scroller, content) = setup_scaffold(shell, context, self.registration);
        let host = credential_host(&self.draft, context.on_connect_started.is_some());
        content.append(&host.widget);
        let submit = Rc::clone(&self.submit);
        append_credential_connect(shell, context, &content, host, move |controller, input| {
            submit(controller, input);
        });
        finish_setup_scaffold(
            shell,
            scroller,
            content,
            context.on_connect_started.is_none(),
        )
    }
}

impl SourceSetupFlow for JellyfinSetupFlow {
    fn view(&self, shell: &Rc<Shell>, context: &SetupViewContext) -> gtk::Widget {
        shell.start_server_discovery_once();
        let (scroller, content) = setup_scaffold(shell, context, self.registration);
        let host = credential_host(&self.draft, context.on_connect_started.is_some());
        content.append(&host.widget);

        let instant_mix = adw::SwitchRow::builder()
            .title(tr("Use Jellyfin Instant Mix for recommendations"))
            .subtitle(tr("This uses Jellyfin API for play radio, necessary if you want recommendation plugins to work."))
            .active(self.use_instant_mix.get())
            .build();
        let instant_group = adw::PreferencesGroup::new();
        instant_group.add(&instant_mix);
        content.append(&instant_group);
        let use_instant_mix = Rc::clone(&self.use_instant_mix);
        instant_mix.connect_active_notify(move |row| use_instant_mix.set(row.is_active()));

        content.append(&discovered_servers_group(shell, &host, &self.draft));
        let use_instant_mix = Rc::clone(&self.use_instant_mix);
        let submit = Rc::clone(&self.submit);
        append_credential_connect(
            shell,
            context,
            &content,
            host,
            move |controller, credentials| {
                submit(controller, credentials, use_instant_mix.get());
            },
        );
        finish_setup_scaffold(
            shell,
            scroller,
            content,
            context.on_connect_started.is_none(),
        )
    }
}

impl SourceSetupFlow for LocalSetupFlow {
    fn view(&self, shell: &Rc<Shell>, context: &SetupViewContext) -> gtk::Widget {
        let (scroller, content) = setup_scaffold(shell, context, self.registration);
        let group = adw::PreferencesGroup::builder()
            .title(tr("Local library"))
            .description(tr(
                "Choose one or more folders to scan and play directly from this computer",
            ))
            .build();
        let summary = adw::ActionRow::builder()
            .title(tr("Folders"))
            .subtitle(local_folders_subtitle(&self.folders.borrow()))
            .build();
        let add = gtk::Button::with_label(&tr("Add folder"));
        add.set_valign(gtk::Align::Center);
        summary.add_suffix(&add);
        summary.set_activatable_widget(Some(&add));
        group.add(&summary);
        content.append(&group);

        let status = setup_status_label(shell);
        let login = text_button("folder-music-symbolic", "Connect");
        login.add_css_class("suggested-action");
        let actions = setup_actions(&login);
        content.append(&actions);
        content.append(&status);

        let rows = Rc::new(RefCell::new(Vec::new()));
        let selection = LocalFolderSelectionRows {
            group,
            summary,
            rows,
            folders: Rc::clone(&self.folders),
            login: login.clone(),
        };
        refresh_local_folder_selection_rows(&selection);
        connect_add_local_folder_button(&shell.window, &add, Rc::clone(&self.folders), {
            let selection = selection.clone();
            move || refresh_local_folder_selection_rows(&selection)
        });

        let controller = shell.controller.clone();
        let folders = Rc::clone(&self.folders);
        let shell_for_login = Rc::clone(shell);
        let status_for_login = status.clone();
        let login_for_click = login.clone();
        let attempt = Rc::new(Cell::new(false));
        let attempt_for_click = Rc::clone(&attempt);
        let on_connect_started = context.on_connect_started.clone();
        let submit = Rc::clone(&self.submit);
        login.connect_clicked(move |_| {
            let roots = folders.borrow().clone();
            if roots.is_empty() {
                status_for_login.set_text(&tr("Choose at least one local music folder"));
                status_for_login.set_visible(true);
                return;
            }
            let message = tr("Caching local library...");
            begin_connect_attempt(
                &status_for_login,
                &login_for_click,
                &attempt_for_click,
                on_connect_started.as_ref(),
                &message,
            );
            shell_for_login.begin_first_run_connection(&message);
            submit(&controller, roots);
        });
        source_enter_controller(&login);
        let ready = {
            let folders = Rc::clone(&self.folders);
            let login = login.clone();
            Rc::new(move || login.set_sensitive(!folders.borrow().is_empty())) as Rc<dyn Fn()>
        };
        connect_add_server_status_watcher(shell, &status, &login, ready, attempt);

        finish_setup_scaffold(
            shell,
            scroller,
            content,
            context.on_connect_started.is_none(),
        )
    }
}

fn setup_scaffold(
    shell: &Rc<Shell>,
    context: &SetupViewContext,
    registration: &'static SourceRegistration,
) -> (gtk::ScrolledWindow, gtk::Box) {
    let compact = context.on_connect_started.is_some();
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(large_popup_content_width(ADD_SERVER_CLAMP_WIDTH));
    clamp.set_tightening_threshold(360);
    clamp.set_margin_top(if compact { 8 } else { 36 });
    clamp.set_margin_bottom(if compact { 20 } else { 36 });
    clamp.set_margin_start(24);
    clamp.set_margin_end(24);
    clamp.set_valign(gtk::Align::Start);
    let content = gtk::Box::new(gtk::Orientation::Vertical, if compact { 10 } else { 18 });
    content.add_css_class("first-run-content");
    if compact {
        content.add_css_class("add-server-compact-content");
    }
    content.set_hexpand(true);
    if let Some(saved_sources) = saved_source_recovery_group(shell, compact) {
        content.append(&saved_sources);
    }
    content.append(&source_choice_selector(
        shell,
        context,
        registration,
        compact,
    ));
    clamp.set_child(Some(&content));
    scroller.set_child(Some(&clamp));
    (scroller, content)
}

fn saved_source_recovery_group(shell: &Rc<Shell>, compact: bool) -> Option<adw::PreferencesGroup> {
    if compact {
        return None;
    }
    let sources = {
        let library = shell.state.library.borrow();
        let selected_registration = library
            .source
            .as_ref()
            .and_then(|source| resolve_source_registration(&source.kind));
        if library.first_run && selected_registration.is_none() {
            library.sources.clone()
        } else {
            Vec::new()
        }
    };
    if sources.is_empty() {
        return None;
    }
    let group = adw::PreferencesGroup::builder()
        .title(tr("Saved Sources"))
        .description(tr("Choose a configured source or add another."))
        .build();
    for source in sources {
        let registration = resolve_source_registration(&source.kind);
        let fallback_title = registration
            .map(|registration| tr(registration.picker.title))
            .unwrap_or_else(|| source.kind.clone());
        let title = source.name.trim();
        let row = adw::ActionRow::builder()
            .title(if title.is_empty() {
                fallback_title.as_str()
            } else {
                title
            })
            .subtitle(fallback_title.as_str())
            .activatable(registration.is_some())
            .build();
        let icon = gtk::Image::from_icon_name(
            registration.map_or("network-server-symbolic", |registration| {
                registration.picker.icon_name
            }),
        );
        row.add_prefix(&icon);
        let forget = gtk::Button::from_icon_name("window-close-symbolic");
        forget.set_tooltip_text(Some(&tr("Forget Server")));
        forget.set_valign(gtk::Align::Center);
        forget.add_css_class("flat");
        forget.add_css_class("destructive-action");
        let forget_shell = Rc::clone(shell);
        let forgotten_source_id = source.id.clone();
        let forgotten_source_name = if title.is_empty() {
            fallback_title.clone()
        } else {
            title.to_string()
        };
        forget.connect_clicked(move |_| {
            confirm_forget_source(
                &forget_shell,
                forgotten_source_id.clone(),
                &forgotten_source_name,
                Rc::new(|| {}),
            );
        });
        row.add_suffix(&forget);
        if registration.is_some() {
            let controller = shell.controller.clone();
            let source_id = source.id;
            row.connect_activated(move |_| {
                controller.select_source(domain::LibrarySourceSelection::Source(source_id.clone()));
            });
        }
        group.add(&row);
    }
    Some(group)
}

fn finish_setup_scaffold(
    shell: &Rc<Shell>,
    scroller: gtk::ScrolledWindow,
    content: gtk::Box,
    embedded: bool,
) -> gtk::Widget {
    if embedded {
        let privacy_group = adw::PreferencesGroup::builder()
            .title(tr("Privacy and Security"))
            .build();
        let private_mode = adw::SwitchRow::builder()
            .title(tr("Private mode"))
            .active(shell.state.settings.borrow().private_mode)
            .build();
        let private_shell = Rc::clone(shell);
        private_mode.connect_active_notify(move |row| {
            private_shell.set_private_mode(row.is_active());
        });
        privacy_group.add(&private_mode);
        content.append(&privacy_group);
    }
    let view = scroller.upcast::<gtk::Widget>();
    if embedded {
        connect_view_with_close_controls(view)
    } else {
        view
    }
}

fn source_choice_selector(
    shell: &Rc<Shell>,
    context: &SetupViewContext,
    selected: &'static SourceRegistration,
    compact: bool,
) -> gtk::Box {
    let wrapper = gtk::Box::new(gtk::Orientation::Horizontal, if compact { 4 } else { 8 });
    wrapper.add_css_class("source-choice-list");
    if compact {
        wrapper.add_css_class("compact-source-choice-list");
    }
    wrapper.set_homogeneous(true);
    wrapper.set_hexpand(true);
    for registration in source_registrations() {
        let presentation = registration.picker;
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("source-choice-button");
        if compact {
            button.add_css_class("compact-source-choice-button");
        }
        set_source_choice_active(&button, same_registration(registration, selected));
        button.update_property(&[gtk::accessible::Property::Label(&tr(presentation.title))]);
        let child = gtk::Box::new(gtk::Orientation::Vertical, if compact { 2 } else { 4 });
        child.set_halign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name(presentation.icon_name);
        let icon_size = if compact { 24 } else { 34 };
        icon.set_pixel_size(icon_size);
        child.append(&icon);
        let label = gtk::Label::new(Some(&tr(presentation.title)));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(14);
        child.append(&label);
        button.set_child(Some(&child));
        if !same_registration(registration, selected) {
            let shell = Rc::clone(shell);
            let context = context.clone();
            let registration = *registration;
            button.connect_clicked(move |_| {
                *context.flow.borrow_mut() = (registration.new_setup_flow)(&shell);
                render_setup_flow(&shell, &context);
            });
        }
        wrapper.append(&button);
    }
    wrapper
}

fn set_source_choice_active(button: &gtk::Button, active: bool) {
    if active {
        button.add_css_class("active");
    } else {
        button.remove_css_class("active");
    }
}

fn same_registration(
    left: &'static SourceRegistration,
    right: &'static SourceRegistration,
) -> bool {
    std::ptr::eq(left, right)
}

fn render_setup_flow(shell: &Rc<Shell>, context: &SetupViewContext) {
    if shell.state.first_run_connection_pending.get() {
        replace_add_server_content(&context.content, shell.connection_progress_view());
        return;
    }
    let flow = context.flow.borrow().clone();
    let view = flow.view(shell, context);
    replace_add_server_content(&context.content, view);
}

fn credential_draft(preset: Option<CredentialHostPreset>) -> CredentialHostDraft {
    preset.map_or_else(
        || CredentialHostDraft {
            name: String::new(),
            url: "http://".to_string(),
            username: String::new(),
            password: String::new(),
            cert_verify: true,
        },
        |preset| CredentialHostDraft {
            name: preset.server_name,
            url: preset.server_url,
            username: preset.username,
            password: String::new(),
            cert_verify: !preset.trust_invalid_cert,
        },
    )
}

fn credential_host(draft: &Rc<RefCell<CredentialHostDraft>>, compact: bool) -> CredentialHost {
    let snapshot = draft.borrow().clone();
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let name = adw::EntryRow::builder()
        .title(tr("Name"))
        .text(&snapshot.name)
        .build();
    style_compact_field_row(&name);
    let url = adw::EntryRow::builder()
        .title(tr("Server Address"))
        .text(&snapshot.url)
        .build();
    style_compact_field_row(&url);
    let fields = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    fields.set_homogeneous(true);
    fields.set_hexpand(true);
    fields.append(&compact_field_row_group(&name));
    fields.append(&compact_field_row_group(&url));
    install_compact_field_row_responsiveness(&fields);
    let fields_group = if compact {
        adw::PreferencesGroup::new()
    } else {
        adw::PreferencesGroup::builder().title(tr("Server")).build()
    };
    fields_group.add(&fields);
    section.append(&fields_group);

    let username = adw::EntryRow::builder()
        .title(tr("Username"))
        .text(&snapshot.username)
        .build();
    style_compact_field_row(&username);
    let password = adw::PasswordEntryRow::builder()
        .title(tr("Password"))
        .build();
    password.set_text(&snapshot.password);
    style_compact_field_row(&password);
    let cert_verify = adw::SwitchRow::builder()
        .title(tr("Verify server certificate"))
        .subtitle(tr("Turn off only for a server you control"))
        .active(snapshot.cert_verify)
        .build();
    let rows = adw::PreferencesGroup::new();
    rows.add(&username);
    rows.add(&password);
    rows.add(&cert_verify);
    section.append(&rows);

    bind_credential_draft(draft, &name, &url, &username, &password, &cert_verify);
    CredentialHost {
        widget: section,
        name,
        url,
        username,
        password,
        cert_verify,
    }
}

fn bind_credential_draft(
    draft: &Rc<RefCell<CredentialHostDraft>>,
    name: &adw::EntryRow,
    url: &adw::EntryRow,
    username: &adw::EntryRow,
    password: &adw::PasswordEntryRow,
    cert_verify: &adw::SwitchRow,
) {
    let value = Rc::clone(draft);
    name.connect_text_notify(move |row| value.borrow_mut().name = row.text().to_string());
    let value = Rc::clone(draft);
    url.connect_text_notify(move |row| value.borrow_mut().url = row.text().to_string());
    let value = Rc::clone(draft);
    username.connect_text_notify(move |row| value.borrow_mut().username = row.text().to_string());
    let value = Rc::clone(draft);
    password.connect_text_notify(move |row| value.borrow_mut().password = row.text().to_string());
    let value = Rc::clone(draft);
    cert_verify.connect_active_notify(move |row| value.borrow_mut().cert_verify = row.is_active());
}

fn append_credential_connect(
    shell: &Rc<Shell>,
    context: &SetupViewContext,
    content: &gtk::Box,
    host: CredentialHost,
    submit: impl Fn(&AppController, CredentialHostInput) + 'static,
) {
    let status = setup_status_label(shell);
    let login = text_button("network-server-symbolic", "Connect");
    login.add_css_class("suggested-action");
    login.set_sensitive(host.ready());
    connect_entry_row_activation(&host.name, &login);
    connect_entry_row_activation(&host.url, &login);
    connect_entry_row_activation(&host.username, &login);
    connect_password_entry_row_activation(&host.password, &login);
    let refresh = Rc::new({
        let host = host_widgets(&host);
        let login = login.clone();
        move || login.set_sensitive(host.ready())
    }) as Rc<dyn Fn()>;
    host.url.connect_text_notify({
        let refresh = Rc::clone(&refresh);
        move |_| refresh()
    });
    host.username.connect_text_notify({
        let refresh = Rc::clone(&refresh);
        move |_| refresh()
    });
    host.password.connect_text_notify({
        let refresh = Rc::clone(&refresh);
        move |_| refresh()
    });

    let controller = shell.controller.clone();
    let shell_for_login = Rc::clone(shell);
    let status_for_login = status.clone();
    let login_for_click = login.clone();
    let host_for_click = host_widgets(&host);
    let attempt = Rc::new(Cell::new(false));
    let attempt_for_click = Rc::clone(&attempt);
    let on_connect_started = context.on_connect_started.clone();
    login.connect_clicked(move |_| {
        if !host_for_click.ready() {
            status_for_login.set_text(&tr("Enter a server address, username, and password"));
            status_for_login.set_visible(true);
            return;
        }
        let message = tr("Connecting to music server...");
        begin_connect_attempt(
            &status_for_login,
            &login_for_click,
            &attempt_for_click,
            on_connect_started.as_ref(),
            &message,
        );
        shell_for_login.begin_first_run_connection(&message);
        submit(&controller, host_for_click.input());
    });
    content.append(&setup_actions(&login));
    content.append(&status);
    connect_add_server_status_watcher(shell, &status, &login, refresh, attempt);
}

fn host_widgets(host: &CredentialHost) -> CredentialHost {
    CredentialHost {
        widget: host.widget.clone(),
        name: host.name.clone(),
        url: host.url.clone(),
        username: host.username.clone(),
        password: host.password.clone(),
        cert_verify: host.cert_verify.clone(),
    }
}

fn setup_actions(login: &gtk::Button) -> gtk::Box {
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    actions.set_halign(gtk::Align::End);
    actions.append(login);
    actions
}

fn setup_status_label(shell: &Rc<Shell>) -> gtk::Label {
    let status = gtk::Label::new(None);
    status.add_css_class("muted");
    status.set_wrap(true);
    status.set_xalign(0.0);
    if let Some(error) = &shell.state.library.borrow().last_error {
        status.set_text(error);
        status.add_css_class("error-text");
        status.set_visible(true);
    } else {
        status.set_visible(false);
    }
    status
}

fn begin_connect_attempt(
    status: &gtk::Label,
    login: &gtk::Button,
    attempt: &Cell<bool>,
    on_connect_started: Option<&Rc<dyn Fn()>>,
    message: &str,
) {
    attempt.set(true);
    status.remove_css_class("error-text");
    status.set_text(message);
    status.set_visible(true);
    login.set_sensitive(false);
    if let Some(on_connect_started) = on_connect_started {
        on_connect_started();
    }
}

fn connect_add_server_status_watcher(
    shell: &Rc<Shell>,
    status: &gtk::Label,
    login: &gtk::Button,
    refresh_ready: Rc<dyn Fn()>,
    connect_attempt_started: Rc<Cell<bool>>,
) {
    let shell = Rc::clone(shell);
    let status = status.clone();
    let login = login.clone();
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
            refresh_ready();
            return gtk::glib::ControlFlow::Continue;
        }
        if connect_attempt_started.get() {
            return gtk::glib::ControlFlow::Break;
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn discovered_servers_group(
    shell: &Rc<Shell>,
    host: &CredentialHost,
    draft: &Rc<RefCell<CredentialHostDraft>>,
) -> adw::PreferencesGroup {
    let status = shell.state.server_discovery_status.borrow().clone();
    let running = shell.state.server_discovery_running.get();
    let servers = shell.state.discovered_servers.borrow().clone();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Found Servers"))
        .description(discovery_status_label(&status))
        .build();
    if servers.is_empty() {
        let row = adw::ActionRow::builder()
            .title(if running {
                tr("Searching Local Network")
            } else {
                tr("No Servers Found")
            })
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
        if running {
            let spinner = gtk::Spinner::new();
            spinner.start();
            row.add_suffix(&spinner);
        }
        group.add(&row);
    } else {
        for server in servers {
            let row = adw::ActionRow::builder()
                .title(server.name.clone())
                .subtitle(format!("{} - {}", server.kind, server.address))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(
                "io.github.screwys.Rufin.source.jellyfin",
            ));
            row.set_activatable(true);
            let name = host.name.clone();
            let url = host.url.clone();
            let draft = Rc::clone(draft);
            row.connect_activated(move |_| {
                let mut draft = draft.borrow_mut();
                draft.name = server.name.clone();
                draft.url = server.address.clone();
                name.set_text(&server.name);
                url.set_text(&server.address);
            });
            group.add(&row);
        }
    }
    let search = adw::ButtonRow::builder()
        .title(if running {
            tr("Searching...")
        } else {
            tr("Search Again")
        })
        .start_icon_name("view-refresh-symbolic")
        .build();
    search.set_sensitive(!running);
    let shell = Rc::clone(shell);
    search.connect_activated(move |_| shell.refresh_server_discovery());
    group.add(&search);
    group
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

#[derive(Clone)]
struct LocalFolderSelectionRows {
    group: adw::PreferencesGroup,
    summary: adw::ActionRow,
    rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    folders: Rc<RefCell<Vec<PathBuf>>>,
    login: gtk::Button,
}

fn refresh_local_folder_selection_rows(selection: &LocalFolderSelectionRows) {
    let folders = selection.folders.borrow().clone();
    selection
        .summary
        .set_subtitle(&local_folders_subtitle(&folders));
    selection.login.set_sensitive(!folders.is_empty());
    for row in selection.rows.borrow_mut().drain(..) {
        selection.group.remove(&row);
    }
    for folder in folders {
        let row = adw::ActionRow::builder()
            .title(local_folder_title(&folder))
            .subtitle(path_subtitle(&folder))
            .subtitle_lines(2)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("rufin-route-folders-symbolic"));
        let remove = gtk::Button::from_icon_name("window-close-symbolic");
        remove.set_tooltip_text(Some(&tr("Remove folder")));
        remove.add_css_class("flat");
        remove.add_css_class("destructive-action");
        row.add_suffix(&remove);
        let selection_for_remove = selection.clone();
        let folder = folder.clone();
        remove.connect_clicked(move |_| {
            selection_for_remove
                .folders
                .borrow_mut()
                .retain(|candidate| candidate != &folder);
            refresh_local_folder_selection_rows(&selection_for_remove);
        });
        selection.group.add(&row);
        selection.rows.borrow_mut().push(row);
    }
}

fn source_enter_controller(login: &gtk::Button) {
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let login_for_key = login.clone();
    controller.connect_key_pressed(move |_, key, _, _| {
        let enter = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
        if enter && activate_connect_if_ready(&login_for_key) {
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    login.add_controller(controller);
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

fn activate_connect_if_ready(login: &gtk::Button) -> bool {
    if !login.is_sensitive() {
        return false;
    }
    login.emit_clicked();
    true
}

fn trimmed_optional_text(row: &adw::EntryRow) -> Option<String> {
    let text = row.text();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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

fn append_local_folder(folders: &Rc<RefCell<Vec<PathBuf>>>, path: PathBuf) {
    let mut folders = folders.borrow_mut();
    if !folders.iter().any(|folder| folder == &path) {
        folders.push(path);
    }
}

fn local_folder_title(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path_subtitle(path))
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
            *target.borrow_mut() = Some(path.clone());
            on_changed(path);
        });
    });
}

fn connect_add_local_folder_button(
    window: &adw::ApplicationWindow,
    button: &gtk::Button,
    folders: Rc<RefCell<Vec<PathBuf>>>,
    on_changed: impl Fn() + 'static,
) {
    let window = window.clone();
    let on_changed: Rc<dyn Fn()> = Rc::new(on_changed);
    button.connect_clicked(move |_| {
        let window = window.clone();
        let folders = Rc::clone(&folders);
        let on_changed = Rc::clone(&on_changed);
        gtk::glib::spawn_future_local(async move {
            let selected_folder = folders
                .borrow()
                .last()
                .cloned()
                .or_else(default_music_folder)
                .map(gtk::gio::File::for_path);
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
            on_changed();
        });
    });
}

fn replace_add_server_content(content: &gtk::Box, child: gtk::Widget) {
    while let Some(current) = content.first_child() {
        content.remove(&current);
    }
    content.append(&child);
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
