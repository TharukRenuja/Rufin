use super::super::{
    Shell,
    layout::{large_popup_content_height, large_popup_content_width},
    playback_outputs::{present_audio_output_popover, selected_audio_output_title},
    present_light_dismiss_dialog,
};
use super::library;
use crate::{
    external_scrobbling::{self, AudioscrobblerSession},
    i18n::{self, tr, tr_with},
};
use adw::prelude::*;
use domain::ExternalLyricsProvider;
use domain::{
    AudioscrobblerScrobbleSettings, DiscordDisplayType, DiscordLinkType, EQUALIZER_BAND_COUNT,
    HomeBlockKind, LeftSidebarMode, MAX_AUTO_DJ_REFILL_THRESHOLD, MAX_CROSSFADE_SECONDS,
    MAX_NARROW_LAYOUT_THRESHOLD, MIN_AUTO_DJ_REFILL_THRESHOLD, MIN_CROSSFADE_SECONDS,
    MIN_NARROW_LAYOUT_THRESHOLD, PlaybackTransitionMode, ReplayGainMode, RightSidebarMode,
    SecretStorageMode, SidebarRouteItem, SidebarRouteItemSettings, StreamQuality,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

mod general;
mod layout;

use general::*;
pub(in crate::ui) use layout::button_row;
use layout::*;

#[cfg(test)]
mod reorder_tests;

const PREFERENCES_DIALOG_WIDTH: i32 = 700;
const PREFERENCES_DIALOG_HEIGHT: i32 = 640;
const LASTFM_API_CREATE_URL: &str = "https://www.last.fm/api/account/create";
const LISTENBRAINZ_TOKEN_URL: &str = "https://listenbrainz.org/settings/";
const SCROBBLING_ICON_NAME: &str = "io.github.screwys.Rufin.scrobbling-symbolic";

fn selection_row<F>(
    title: &str,
    option_titles: &[String],
    selected: u32,
    on_selected: F,
) -> adw::ActionRow
where
    F: Fn(u32) + 'static,
{
    let row = adw::ActionRow::builder().title(title).build();
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    buttons.add_css_class("linked");
    buttons.add_css_class("preference-selection-buttons");
    buttons.set_valign(gtk::Align::Center);
    let on_selected = Rc::new(on_selected);
    let mut first_button: Option<gtk::ToggleButton> = None;

    for (index, title) in option_titles.iter().enumerate() {
        let button = gtk::ToggleButton::with_label(title);
        button.add_css_class("preference-selection-button");
        button.set_tooltip_text(Some(title));
        if let Some(first) = &first_button {
            button.set_group(Some(first));
        } else {
            first_button = Some(button.clone());
        }
        button.set_active(index as u32 == selected);
        let on_selected = Rc::clone(&on_selected);
        button.connect_toggled(move |button| {
            if button.is_active() {
                on_selected(index as u32);
            }
        });
        buttons.append(&button);
    }

    row.add_suffix(&buttons);
    row
}

pub(in crate::ui) fn present_preferences_dialog(shell: &Rc<Shell>) {
    present_preferences_dialog_with_page(shell, PreferencesPageKind::General, false);
}
pub(in crate::ui) fn present_library_preferences_dialog(shell: &Rc<Shell>) {
    present_preferences_dialog_with_page(shell, PreferencesPageKind::Library, false);
}
pub(in crate::ui) fn present_add_server_preferences_dialog(shell: &Rc<Shell>) {
    present_preferences_dialog_with_page(shell, PreferencesPageKind::Library, true);
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreferencesPageKind {
    General,
    Layout,
    Scrobbling,
    Playback,
    Library,
}
impl PreferencesPageKind {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Layout,
        Self::Scrobbling,
        Self::Playback,
        Self::Library,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Layout => "layout",
            Self::Scrobbling => "scrobbling",
            Self::Playback => "playback",
            Self::Library => "library",
        }
    }

    fn title(self) -> String {
        match self {
            Self::General => tr("General"),
            Self::Layout => tr("Layout"),
            Self::Scrobbling => tr("Scrobbling"),
            Self::Playback => tr("Playback"),
            Self::Library => tr("Library"),
        }
    }

    fn icon_name(self) -> &'static str {
        match self {
            Self::General => "preferences-system-symbolic",
            Self::Layout => "preferences-desktop-display-symbolic",
            Self::Scrobbling => SCROBBLING_ICON_NAME,
            Self::Playback => "media-playback-start-symbolic",
            Self::Library => "rufin-route-tracks-symbolic",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[derive(Clone)]
pub(in crate::ui) struct PreferencesNavigationControls {
    back: gtk::Button,
    navigation: Rc<RefCell<Option<adw::NavigationView>>>,
    page_allows_back: Rc<Cell<bool>>,
    nested_page_visible: Rc<Cell<bool>>,
}

impl PreferencesNavigationControls {
    fn new() -> Self {
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.add_css_class("flat");
        back.add_css_class("preferences-nested-back");
        back.update_property(&[gtk::accessible::Property::Label(&tr("Back"))]);
        back.set_visible(false);

        let controls = Self {
            back,
            navigation: Rc::new(RefCell::new(None)),
            page_allows_back: Rc::new(Cell::new(false)),
            nested_page_visible: Rc::new(Cell::new(false)),
        };
        let navigation = Rc::clone(&controls.navigation);
        let page_allows_back = Rc::clone(&controls.page_allows_back);
        let nested_page_visible = Rc::clone(&controls.nested_page_visible);
        controls.back.connect_clicked(move |button| {
            if let Some(navigation) = navigation.borrow().as_ref() {
                navigation.pop();
            }
            nested_page_visible.set(false);
            button.set_visible(page_allows_back.get() && nested_page_visible.get());
        });
        controls
    }

    fn set_page_allows_back(&self, allowed: bool) {
        self.page_allows_back.set(allowed);
        self.update_visibility();
    }

    pub(in crate::ui) fn set_navigation(&self, navigation: &adw::NavigationView) {
        *self.navigation.borrow_mut() = Some(navigation.clone());
    }

    pub(in crate::ui) fn set_nested_page_visible(&self, visible: bool) {
        self.nested_page_visible.set(visible);
        self.update_visibility();
    }

    fn update_visibility(&self) {
        self.back
            .set_visible(self.page_allows_back.get() && self.nested_page_visible.get());
    }
}

fn present_preferences_dialog_with_page(
    shell: &Rc<Shell>,
    initial_page: PreferencesPageKind,
    open_add_server: bool,
) {
    if let Some(dialog) = shell.state.preferences_dialog.borrow().as_ref().cloned() {
        rebuild_preferences_dialog(shell, &dialog, initial_page, open_add_server);
        present_light_dismiss_dialog(&dialog, &shell.window);
        return;
    }

    let dialog = adw::Dialog::builder()
        .title(tr("Preferences"))
        .content_width(large_popup_content_width(PREFERENCES_DIALOG_WIDTH))
        .content_height(large_popup_content_height(
            shell.window.height(),
            PREFERENCES_DIALOG_HEIGHT,
        ))
        .build();
    dialog.add_css_class("preferences");
    *shell.state.preferences_dialog.borrow_mut() = Some(dialog.clone());
    rebuild_preferences_dialog(shell, &dialog, initial_page, open_add_server);

    let shell_for_close = Rc::clone(shell);
    let dialog_for_close = dialog.clone();
    dialog.connect_closed(move |_| {
        let mut active_dialog = shell_for_close.state.preferences_dialog.borrow_mut();
        if active_dialog.as_ref() == Some(&dialog_for_close) {
            *active_dialog = None;
        }
    });

    present_light_dismiss_dialog(&dialog, &shell.window);
}

fn rebuild_preferences_dialog(
    shell: &Rc<Shell>,
    dialog: &adw::Dialog,
    initial_page: PreferencesPageKind,
    open_add_server: bool,
) {
    dialog.set_title(&tr("Preferences"));

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&tr("Preferences"), "")));
    toolbar.add_top_bar(&header);

    let stack = adw::ViewStack::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    let switcher = adw::ViewSwitcher::builder()
        .policy(adw::ViewSwitcherPolicy::Wide)
        .stack(&stack)
        .build();
    let navigation_controls = PreferencesNavigationControls::new();
    let switcher_bar = gtk::CenterBox::new();
    switcher_bar.add_css_class("preferences-tab-bar");
    switcher_bar.set_hexpand(true);
    switcher_bar.set_start_widget(Some(&navigation_controls.back));
    switcher_bar.set_center_widget(Some(&switcher));
    toolbar.add_top_bar(&switcher_bar);
    toolbar.set_content(Some(&stack));

    let page_slots = Rc::new(
        PreferencesPageKind::ALL
            .into_iter()
            .map(|kind| {
                let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
                slot.set_hexpand(true);
                slot.set_vexpand(true);
                let title = kind.title();
                stack.add_titled_with_icon(&slot, Some(kind.name()), &title, kind.icon_name());
                (kind, slot)
            })
            .collect::<Vec<_>>(),
    );
    navigation_controls.set_page_allows_back(initial_page == PreferencesPageKind::Library);
    ensure_preferences_page(
        shell,
        dialog,
        &page_slots,
        &navigation_controls,
        initial_page,
        open_add_server,
    );
    stack.set_visible_child_name(initial_page.name());
    let page_shell = Rc::clone(shell);
    let page_dialog = dialog.clone();
    let page_slots_for_switch = Rc::clone(&page_slots);
    let navigation_controls_for_switch = navigation_controls.clone();
    stack.connect_visible_child_name_notify(move |stack| {
        let Some(name) = stack.visible_child_name() else {
            return;
        };
        let Some(kind) = PreferencesPageKind::from_name(name.as_str()) else {
            return;
        };
        navigation_controls_for_switch.set_page_allows_back(kind == PreferencesPageKind::Library);
        ensure_preferences_page(
            &page_shell,
            &page_dialog,
            &page_slots_for_switch,
            &navigation_controls_for_switch,
            kind,
            false,
        );
    });

    dialog.set_child(Some(&toolbar));
}
fn ensure_preferences_page(
    shell: &Rc<Shell>,
    dialog: &adw::Dialog,
    page_slots: &[(PreferencesPageKind, gtk::Box)],
    navigation_controls: &PreferencesNavigationControls,
    kind: PreferencesPageKind,
    open_add_server: bool,
) {
    let Some((_, slot)) = page_slots.iter().find(|(slot_kind, _)| *slot_kind == kind) else {
        return;
    };
    if slot.first_child().is_some() {
        return;
    }
    slot.append(&build_preferences_page(
        kind,
        shell,
        dialog,
        navigation_controls,
        open_add_server,
    ));
}
fn build_preferences_page(
    kind: PreferencesPageKind,
    shell: &Rc<Shell>,
    dialog: &adw::Dialog,
    navigation_controls: &PreferencesNavigationControls,
    open_add_server: bool,
) -> gtk::Widget {
    match kind {
        PreferencesPageKind::General => general_page(shell, dialog).upcast(),
        PreferencesPageKind::Layout => layout_page(shell).upcast(),
        PreferencesPageKind::Scrobbling => scrobbling_page(shell).upcast(),
        PreferencesPageKind::Playback => playback_page(shell).upcast(),
        PreferencesPageKind::Library => {
            library::library_page(shell, dialog, navigation_controls, open_add_server)
        }
    }
}
fn general_page(shell: &Rc<Shell>, dialog: &adw::Dialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("General"))
        .icon_name("preferences-system-symbolic")
        .build();

    let settings = shell.state.settings.borrow().clone();

    let language_group = adw::PreferencesGroup::builder()
        .title(tr("Language"))
        .build();
    let language_options = Rc::new(i18n::language_options());
    let language_titles = language_options
        .iter()
        .map(|option| option.title.as_str())
        .collect::<Vec<_>>();
    let language_model = gtk::StringList::new(&language_titles);
    let language_row = adw::ComboRow::builder()
        .title(tr("Language"))
        .model(&language_model)
        .selected(i18n::language_option_index(
            language_options.as_ref(),
            &settings.language,
        ))
        .build();
    let language_shell = Rc::clone(shell);
    let language_options_for_row = Rc::clone(&language_options);
    let dialog_for_language = dialog.clone();
    language_row.connect_selected_notify(move |row| {
        let Some(option) = language_options_for_row.get(row.selected() as usize) else {
            return;
        };
        let language = option.id.clone();
        if language_shell.set_language_preference(language) {
            rebuild_preferences_dialog(
                &language_shell,
                &dialog_for_language,
                PreferencesPageKind::General,
                false,
            );
        }
    });
    language_group.add(&language_row);
    page.add(&language_group);

    #[cfg(unix)]
    {
        let window_group = adw::PreferencesGroup::builder()
            .title(tr("App window"))
            .build();
        let tray_row = adw::SwitchRow::builder()
            .title(tr("Show tray icon"))
            .active(settings.tray_enabled)
            .build();
        let exit_to_tray_row = adw::SwitchRow::builder()
            .title(tr("Exit to tray"))
            .active(settings.tray_enabled && settings.exit_to_tray)
            .build();
        let start_minimized_row = adw::SwitchRow::builder()
            .title(tr("Start minimized"))
            .active(settings.tray_enabled && settings.start_minimized)
            .build();
        let type_to_search_row = adw::SwitchRow::builder()
            .title(tr("Type to search"))
            .active(settings.type_to_search_enabled)
            .build();
        exit_to_tray_row.set_visible(settings.tray_enabled);
        start_minimized_row.set_visible(settings.tray_enabled);
        let tray_shell = Rc::clone(shell);
        let tray_exit_row = exit_to_tray_row.clone();
        let start_minimized_row_for_tray = start_minimized_row.clone();
        tray_row.connect_active_notify(move |row| {
            let enabled = row.is_active();
            tray_exit_row.set_visible(enabled);
            start_minimized_row_for_tray.set_visible(enabled);
            if !enabled {
                tray_exit_row.set_active(false);
                start_minimized_row_for_tray.set_active(false);
            }
            tray_shell.set_tray_enabled(enabled);
        });
        let exit_to_tray_shell = Rc::clone(shell);
        exit_to_tray_row.connect_active_notify(move |row| {
            exit_to_tray_shell.set_exit_to_tray_enabled(row.is_active());
        });
        let start_minimized_shell = Rc::clone(shell);
        start_minimized_row.connect_active_notify(move |row| {
            start_minimized_shell.set_start_minimized_enabled(row.is_active());
        });
        let type_to_search_shell = Rc::clone(shell);
        type_to_search_row.connect_active_notify(move |row| {
            type_to_search_shell.set_type_to_search_enabled(row.is_active());
        });
        window_group.add(&tray_row);
        window_group.add(&exit_to_tray_row);
        window_group.add(&start_minimized_row);
        window_group.add(&type_to_search_row);
        page.add(&window_group);
    }

    let notifications_group = adw::PreferencesGroup::builder()
        .title(tr("Notifications"))
        .build();
    let control_notifications_row = adw::SwitchRow::builder()
        .title(tr("Control notifications"))
        .active(settings.control_notifications_enabled)
        .build();
    let control_notifications_shell = Rc::clone(shell);
    control_notifications_row.connect_active_notify(move |row| {
        control_notifications_shell.set_control_notifications_enabled(row.is_active());
    });
    notifications_group.add(&control_notifications_row);

    let notifications_row = adw::SwitchRow::builder()
        .title(tr("Now playing notifications"))
        .active(settings.notifications_enabled)
        .build();
    let notifications_shell = Rc::clone(shell);
    notifications_row.connect_active_notify(move |row| {
        notifications_shell.set_notifications_enabled(row.is_active());
    });
    notifications_group.add(&notifications_row);

    let release_notifications_row = adw::SwitchRow::builder()
        .title(tr("Release notifications"))
        .active(settings.release_notifications_enabled)
        .build();
    let release_notifications_shell = Rc::clone(shell);
    release_notifications_row.connect_active_notify(move |row| {
        release_notifications_shell.set_release_notifications_enabled(row.is_active());
    });
    notifications_group.add(&release_notifications_row);
    page.add(&notifications_group);

    let metadata_group = adw::PreferencesGroup::builder()
        .title(tr("Metadata"))
        .build();
    let prefer_server_playlist_row = adw::SwitchRow::builder()
        .title(tr("Prefer server playlist covers"))
        .active(settings.prefer_server_playlist_covers)
        .build();
    let prefer_server_playlist_shell = Rc::clone(shell);
    prefer_server_playlist_row.connect_active_notify(move |row| {
        prefer_server_playlist_shell.set_prefer_server_playlist_covers(row.is_active());
    });

    let prefer_server_row = adw::SwitchRow::builder()
        .title(tr("Prefer server lyrics"))
        .active(settings.prefer_server_lyrics)
        .sensitive(settings.external_lyrics_enabled)
        .build();
    let prefer_server_shell = Rc::clone(shell);
    prefer_server_row.connect_active_notify(move |row| {
        prefer_server_shell.set_prefer_server_lyrics(row.is_active());
    });

    let external_metadata_row = adw::SwitchRow::builder()
        .title(tr("External metadata lookup"))
        .active(settings.external_metadata_enabled)
        .build();
    let metadata_shell = Rc::clone(shell);
    external_metadata_row.connect_active_notify(move |row| {
        metadata_shell.set_external_metadata_enabled(row.is_active());
    });

    let external_row = adw::SwitchRow::builder()
        .title(tr("External lyric lookup"))
        .active(settings.external_lyrics_enabled)
        .build();

    let provider_rows = ExternalLyricsProvider::all()
        .into_iter()
        .map(|provider| {
            let row = adw::SwitchRow::builder()
                .title(provider.title())
                .active(settings.external_lyrics_providers.contains(&provider))
                .sensitive(settings.external_lyrics_enabled)
                .build();
            let provider_shell = Rc::clone(shell);
            row.connect_active_notify(move |row| {
                provider_shell.set_external_lyrics_provider_enabled(provider, row.is_active());
            });
            (provider, row)
        })
        .collect::<Vec<_>>();

    let external_shell = Rc::clone(shell);
    let prefer_server_row_for_external = prefer_server_row.clone();
    let provider_rows_for_external = provider_rows
        .iter()
        .map(|(_provider, row)| row.clone())
        .collect::<Vec<_>>();
    external_row.connect_active_notify(move |row| {
        let enabled = row.is_active();
        prefer_server_row_for_external.set_sensitive(enabled);
        for provider_row in &provider_rows_for_external {
            provider_row.set_sensitive(enabled);
        }
        external_shell.set_external_lyrics_enabled(enabled);
    });
    metadata_group.add(&prefer_server_playlist_row);
    metadata_group.add(&prefer_server_row);
    metadata_group.add(&external_metadata_row);
    metadata_group.add(&external_row);
    for (_provider, row) in provider_rows {
        metadata_group.add(&row);
    }
    page.add(&metadata_group);

    let external_links = settings.external_site_links.clone();
    let external_links_group = adw::PreferencesGroup::builder()
        .title(tr("External site links"))
        .build();
    let external_links_row = adw::SwitchRow::builder()
        .title(tr("Show external site links"))
        .subtitle(tr("Show external service icons on album and artist pages"))
        .active(external_links.enabled)
        .build();
    let lastfm_links_row = adw::SwitchRow::builder()
        .title(tr("Last.fm"))
        .active(external_links.lastfm)
        .sensitive(external_links.enabled)
        .build();
    let musicbrainz_links_row = adw::SwitchRow::builder()
        .title(tr("MusicBrainz"))
        .active(external_links.musicbrainz)
        .sensitive(external_links.enabled)
        .build();
    let server_links_row = adw::SwitchRow::builder()
        .title(tr("Server"))
        .active(external_links.server)
        .sensitive(external_links.enabled)
        .build();
    let external_links_shell = Rc::clone(shell);
    let lastfm_links_for_master = lastfm_links_row.clone();
    let musicbrainz_links_for_master = musicbrainz_links_row.clone();
    let server_links_for_master = server_links_row.clone();
    external_links_row.connect_active_notify(move |row| {
        let enabled = row.is_active();
        lastfm_links_for_master.set_sensitive(enabled);
        musicbrainz_links_for_master.set_sensitive(enabled);
        server_links_for_master.set_sensitive(enabled);
        external_links_shell.set_external_site_links_enabled(enabled);
    });
    let lastfm_links_shell = Rc::clone(shell);
    lastfm_links_row.connect_active_notify(move |row| {
        lastfm_links_shell.set_lastfm_site_links_enabled(row.is_active());
    });
    let musicbrainz_links_shell = Rc::clone(shell);
    musicbrainz_links_row.connect_active_notify(move |row| {
        musicbrainz_links_shell.set_musicbrainz_site_links_enabled(row.is_active());
    });
    let server_links_shell = Rc::clone(shell);
    server_links_row.connect_active_notify(move |row| {
        server_links_shell.set_server_site_links_enabled(row.is_active());
    });
    external_links_group.add(&external_links_row);
    external_links_group.add(&lastfm_links_row);
    external_links_group.add(&musicbrainz_links_row);
    external_links_group.add(&server_links_row);
    page.add(&external_links_group);

    let discord_group = adw::PreferencesGroup::builder()
        .title(tr("Discord"))
        .build();
    let presence_row = adw::SwitchRow::builder()
        .title(tr("Rich presence"))
        .active(settings.discord_presence_enabled)
        .build();
    let presence_shell = Rc::clone(shell);
    presence_row.connect_active_notify(move |row| {
        presence_shell.set_discord_presence_enabled(row.is_active());
    });
    discord_group.add(&presence_row);

    let display_titles = [tr("Application name"), tr("Song title"), tr("Artist name")];
    let display_refs = display_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let display_options = gtk::StringList::new(&display_refs);
    let display_row = adw::ComboRow::builder()
        .title(tr("Status display"))
        .model(&display_options)
        .selected(discord_display_index(settings.discord_display_type))
        .build();
    let display_shell = Rc::clone(shell);
    display_row.connect_selected_notify(move |row| {
        display_shell.set_discord_display_type(discord_display_from_index(row.selected()));
    });
    discord_group.add(&display_row);

    let link_titles = [
        tr("None"),
        tr("Last.fm"),
        tr("MusicBrainz"),
        tr("MusicBrainz + Last.fm"),
    ];
    let link_refs = link_titles.iter().map(String::as_str).collect::<Vec<_>>();
    let link_options = gtk::StringList::new(&link_refs);
    let link_row = adw::ComboRow::builder()
        .title(tr("Metadata source"))
        .subtitle(tr("Source to use for cover images and song/artist links"))
        .model(&link_options)
        .selected(discord_link_index(settings.discord_link_type))
        .build();
    let link_shell = Rc::clone(shell);
    link_row.connect_selected_notify(move |row| {
        link_shell.set_discord_link_type(discord_link_from_index(row.selected()));
    });
    discord_group.add(&link_row);

    let paused_row = adw::SwitchRow::builder()
        .title(tr("Show paused status"))
        .active(settings.discord_show_paused)
        .build();
    let paused_shell = Rc::clone(shell);
    paused_row.connect_active_notify(move |row| {
        paused_shell.set_discord_show_paused(row.is_active());
    });
    discord_group.add(&paused_row);

    let listening_row = adw::SwitchRow::builder()
        .title(tr("Use listening activity"))
        .subtitle(tr("Set the Discord activity type to Listening"))
        .active(settings.discord_show_as_listening)
        .build();
    let listening_shell = Rc::clone(shell);
    listening_row.connect_active_notify(move |row| {
        listening_shell.set_discord_show_as_listening(row.is_active());
    });
    discord_group.add(&listening_row);

    let state_icon_row = adw::SwitchRow::builder()
        .title(tr("Show playback icon"))
        .active(settings.discord_show_state_icon)
        .build();
    let state_icon_shell = Rc::clone(shell);
    state_icon_row.connect_active_notify(move |row| {
        state_icon_shell.set_discord_show_state_icon(row.is_active());
    });
    discord_group.add(&state_icon_row);

    let lastfm_row = adw::PasswordEntryRow::builder()
        .title(tr("Last.fm API key"))
        .show_apply_button(true)
        .build();
    lastfm_row.set_text(&settings.lastfm_api_key);
    let lastfm_shell = Rc::clone(shell);
    lastfm_row.connect_apply(move |row| {
        lastfm_shell.set_lastfm_api_key(row.text().to_string());
    });
    discord_group.add(&lastfm_row);

    page.add(&discord_group);

    let privacy_group = adw::PreferencesGroup::builder()
        .title(tr("Privacy and Security"))
        .build();
    let private_row = adw::SwitchRow::builder()
        .title(tr("Private mode"))
        .active(settings.private_mode)
        .build();
    let private_shell = Rc::clone(shell);
    private_row.connect_active_notify(move |row| {
        private_shell.set_private_mode(row.is_active());
    });
    privacy_group.add(&private_row);

    let secret_storage_titles = [tr("Legacy"), tr("Secure storage")];
    let secret_storage_refs = secret_storage_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let secret_storage_row = adw::ComboRow::builder()
        .title(tr("Secret storage"))
        .model(&gtk::StringList::new(&secret_storage_refs))
        .selected(secret_storage_mode_index(settings.secret_storage_mode))
        .build();
    let secret_storage_shell = Rc::clone(shell);
    let secret_storage_guard = Rc::new(Cell::new(false));
    let secret_storage_guard_for_row = Rc::clone(&secret_storage_guard);
    let preferences_dialog_for_secret_storage = dialog.clone();
    secret_storage_row.connect_selected_notify(move |row| {
        if secret_storage_guard_for_row.get() {
            return;
        }
        let mode = secret_storage_mode_from_index(row.selected());
        let previous_mode = secret_storage_shell
            .state
            .settings
            .borrow()
            .secret_storage_mode;
        if mode == previous_mode {
            return;
        }

        let confirm = adw::AlertDialog::builder()
            .heading(tr("Change Secret Storage"))
            .body(tr(
                "Changing secret backend removes legacy config secrets and signs you out everywhere, including API secrets. App cache is not affected.",
            ))
            .build();
        let cancel = tr("Cancel");
        let change = tr("Change");
        confirm.add_responses(&[("cancel", cancel.as_str()), ("change", change.as_str())]);
        confirm.set_default_response(Some("cancel"));
        confirm.set_close_response("cancel");
        confirm.set_response_appearance("change", adw::ResponseAppearance::Destructive);
        let row = row.clone();
        let shell = Rc::clone(&secret_storage_shell);
        let guard = Rc::clone(&secret_storage_guard_for_row);
        let preferences_dialog = preferences_dialog_for_secret_storage.clone();
        let window = shell.window.clone();
        confirm.choose(
            Some(&window),
            None::<&gtk::gio::Cancellable>,
            move |response| {
                if response.as_str() == "change" && shell.set_secret_storage_mode(mode) {
                    preferences_dialog.close();
                    return;
                }
                guard.set(true);
                row.set_selected(secret_storage_mode_index(previous_mode));
                guard.set(false);
            },
        );
    });
    privacy_group.add(&secret_storage_row);

    page.add(&privacy_group);

    page
}
fn secret_storage_mode_index(mode: SecretStorageMode) -> u32 {
    match mode {
        SecretStorageMode::ConfigFile => 0,
        SecretStorageMode::SystemKeyring => 1,
    }
}
fn secret_storage_mode_from_index(index: u32) -> SecretStorageMode {
    match index {
        1 => SecretStorageMode::SystemKeyring,
        _ => SecretStorageMode::ConfigFile,
    }
}
fn interface_group(shell: &Rc<Shell>) -> adw::PreferencesGroup {
    let settings = shell.state.settings.borrow().clone();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Interface"))
        .build();

    let default_left_shell = Rc::clone(shell);
    let default_left_row = left_sidebar_row(
        &tr("Default left sidebar"),
        settings.layout.default_profile.left_sidebar,
        move |selected| {
            let mode = left_sidebar_mode_from_index(selected);
            default_left_shell.update_app_settings("layout setting", |settings| {
                if settings.layout.default_profile.left_sidebar == mode {
                    return false;
                }
                settings.layout.default_profile.left_sidebar = mode;
                true
            });
            default_left_shell.update_layout();
        },
    );
    group.add(&default_left_row);

    let default_right_shell = Rc::clone(shell);
    let default_right_row = right_sidebar_row(
        &tr("Default right sidebar"),
        settings.layout.default_profile.right_sidebar,
        move |selected| {
            let mode = right_sidebar_mode_from_index(selected);
            default_right_shell.update_app_settings("layout setting", |settings| {
                if settings.layout.default_profile.right_sidebar == mode {
                    return false;
                }
                settings.layout.default_profile.right_sidebar = mode;
                if mode.is_visible() {
                    settings.layout.default_profile.last_visible_right_sidebar = mode;
                }
                settings.layout.sanitize();
                true
            });
            default_right_shell.update_layout();
        },
    );
    group.add(&default_right_row);

    let lyrics_panel_row = adw::SwitchRow::builder()
        .title(tr("Show Lyrics Panel"))
        .active(settings.lyrics_panel_visible)
        .build();
    let lyrics_panel_shell = Rc::clone(shell);
    lyrics_panel_row.connect_active_notify(move |row| {
        lyrics_panel_shell.set_lyrics_panel_visible(row.is_active());
    });
    group.add(&lyrics_panel_row);

    let narrow_row = adw::SwitchRow::builder()
        .title(tr("Use different layout below a threshold width"))
        .active(settings.layout.narrow_enabled)
        .build();
    group.add(&narrow_row);

    let threshold_adjustment = gtk::Adjustment::new(
        f64::from(settings.layout.narrow_threshold),
        f64::from(MIN_NARROW_LAYOUT_THRESHOLD),
        f64::from(MAX_NARROW_LAYOUT_THRESHOLD),
        10.0,
        100.0,
        0.0,
    );
    let threshold_row = adw::SpinRow::builder()
        .title(tr("Narrow layout threshold"))
        .adjustment(&threshold_adjustment)
        .digits(0)
        .numeric(true)
        .sensitive(settings.layout.narrow_enabled)
        .build();
    let threshold_unit = gtk::Label::new(Some("px"));
    threshold_unit.add_css_class("dim-label");
    threshold_row.add_suffix(&threshold_unit);
    let threshold_shell = Rc::clone(shell);
    threshold_row.connect_value_notify(move |row| {
        let threshold = row.value().round() as i32;
        threshold_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.narrow_threshold == threshold {
                return false;
            }
            settings.layout.narrow_threshold = threshold;
            settings.layout.sanitize();
            true
        });
        threshold_shell.update_layout();
    });
    group.add(&threshold_row);

    let narrow_left_shell = Rc::clone(shell);
    let narrow_left_row = left_sidebar_row(
        &tr("Narrow left sidebar"),
        settings.layout.narrow_profile.left_sidebar,
        move |selected| {
            let mode = left_sidebar_mode_from_index(selected);
            narrow_left_shell.update_app_settings("layout setting", |settings| {
                if settings.layout.narrow_profile.left_sidebar == mode {
                    return false;
                }
                settings.layout.narrow_profile.left_sidebar = mode;
                true
            });
            narrow_left_shell.update_layout();
        },
    );
    narrow_left_row.set_sensitive(settings.layout.narrow_enabled);
    group.add(&narrow_left_row);

    let narrow_right_shell = Rc::clone(shell);
    let narrow_right_row = right_sidebar_row(
        &tr("Narrow right sidebar"),
        settings.layout.narrow_profile.right_sidebar,
        move |selected| {
            let mode = right_sidebar_mode_from_index(selected);
            narrow_right_shell.update_app_settings("layout setting", |settings| {
                if settings.layout.narrow_profile.right_sidebar == mode {
                    return false;
                }
                settings.layout.narrow_profile.right_sidebar = mode;
                if mode.is_visible() {
                    settings.layout.narrow_profile.last_visible_right_sidebar = mode;
                }
                settings.layout.sanitize();
                true
            });
            narrow_right_shell.update_layout();
        },
    );
    narrow_right_row.set_sensitive(settings.layout.narrow_enabled);
    group.add(&narrow_right_row);

    let threshold_row_for_toggle = threshold_row.clone();
    let narrow_left_row_for_toggle = narrow_left_row.clone();
    let narrow_right_row_for_toggle = narrow_right_row.clone();
    let narrow_shell = Rc::clone(shell);
    narrow_row.connect_active_notify(move |row| {
        let enabled = row.is_active();
        threshold_row_for_toggle.set_sensitive(enabled);
        narrow_left_row_for_toggle.set_sensitive(enabled);
        narrow_right_row_for_toggle.set_sensitive(enabled);
        narrow_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.narrow_enabled == enabled {
                return false;
            }
            settings.layout.narrow_enabled = enabled;
            true
        });
        narrow_shell.update_layout();
    });

    group
}
fn sidebar_items_group(shell: &Rc<Shell>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Sidebar Items"))
        .build();
    let rows = Rc::new(RefCell::new(Vec::<adw::ActionRow>::new()));
    populate_sidebar_item_rows(shell, &group, &rows);
    group
}
fn populate_sidebar_item_rows(
    shell: &Rc<Shell>,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }

    let items = shell.state.settings.borrow().sidebar.route_items.clone();
    for entry in items {
        let row = sidebar_item_row(shell, group, rows, entry);
        group.add(&row);
        rows.borrow_mut().push(row);
    }
}
fn sidebar_item_row(
    shell: &Rc<Shell>,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    entry: SidebarRouteItemSettings,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(tr(sidebar_route_item_title(entry.item)))
        .subtitle(if entry.visible {
            tr("Visible")
        } else {
            tr("Hidden")
        })
        .build();

    let drag = gtk::Image::from_icon_name("rufin-list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    row.add_prefix(&drag);

    let visible = gtk::Switch::new();
    visible.set_active(entry.visible);
    visible.set_valign(gtk::Align::Center);

    let up = gtk::Button::from_icon_name("go-up-symbolic");
    up.add_css_class("flat");
    up.set_tooltip_text(Some(&tr("Move up")));
    up.set_valign(gtk::Align::Center);
    row.add_suffix(&up);

    let down = gtk::Button::from_icon_name("go-down-symbolic");
    down.add_css_class("flat");
    down.set_tooltip_text(Some(&tr("Move down")));
    down.set_valign(gtk::Align::Center);
    row.add_suffix(&down);

    row.add_suffix(&visible);
    row.set_activatable_widget(Some(&visible));

    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        visible.connect_active_notify(move |switch| {
            let item = entry.item;
            let is_visible = switch.is_active();
            shell.update_app_settings("sidebar setting", |settings| {
                if let Some(stored) = settings
                    .sidebar
                    .route_items
                    .iter_mut()
                    .find(|stored| stored.item == item)
                {
                    if stored.visible == is_visible {
                        return false;
                    }
                    stored.visible = is_visible;
                }
                settings.sidebar.sanitize();
                true
            });
            shell.rebuild_sidebar_navigation();
            populate_sidebar_item_rows(&shell, &group, &rows);
        });
    }
    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        up.connect_clicked(move |_| {
            move_sidebar_item(&shell, entry.item, -1);
            populate_sidebar_item_rows(&shell, &group, &rows);
        });
    }
    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        down.connect_clicked(move |_| {
            move_sidebar_item(&shell, entry.item, 1);
            populate_sidebar_item_rows(&shell, &group, &rows);
        });
    }

    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let item_id = sidebar_route_item_drag_id(entry.item).to_string();
    source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&item_id.to_value()))
    });
    drag.add_controller(source);

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let shell = Rc::clone(shell);
    let group = group.clone();
    let rows = Rc::clone(rows);
    let row_for_drop = row.clone();
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(source_id) = value.get::<String>() else {
            return false;
        };
        let Some(source_item) = sidebar_drag_route(&source_id) else {
            return false;
        };
        if source_item == entry.item {
            return false;
        }
        let after = y > f64::from(row_for_drop.height()) / 2.0;
        let changed = shell
            .update_app_settings("sidebar setting", |settings| {
                let changed = reorder_sidebar_item_settings(
                    &mut settings.sidebar.route_items,
                    source_item,
                    entry.item,
                    after,
                );
                if changed {
                    settings.sidebar.sanitize();
                }
                changed
            })
            .is_some();
        if changed {
            shell.rebuild_sidebar_navigation();
            populate_sidebar_item_rows(&shell, &group, &rows);
        }
        changed
    });
    row.add_controller(drop_target);

    row
}
fn move_sidebar_item(shell: &Rc<Shell>, item: SidebarRouteItem, delta: isize) {
    shell.update_app_settings("sidebar setting", |settings| {
        let Some(index) = settings
            .sidebar
            .route_items
            .iter()
            .position(|entry| entry.item == item)
        else {
            return false;
        };
        let new_index = if delta < 0 {
            index.saturating_sub(1)
        } else {
            (index + 1).min(settings.sidebar.route_items.len().saturating_sub(1))
        };
        if index == new_index {
            return false;
        }
        settings.sidebar.route_items.swap(index, new_index);
        settings.sidebar.sanitize();
        true
    });
    shell.rebuild_sidebar_navigation();
}
fn reorder_sidebar_item_settings(
    items: &mut Vec<SidebarRouteItemSettings>,
    source: SidebarRouteItem,
    target: SidebarRouteItem,
    after: bool,
) -> bool {
    if source == target {
        return false;
    }
    let before = items.clone();
    let Some(source_index) = items.iter().position(|entry| entry.item == source) else {
        return false;
    };
    let entry = items.remove(source_index);
    let Some(mut target_index) = items.iter().position(|entry| entry.item == target) else {
        items.insert(source_index.min(items.len()), entry);
        return false;
    };
    if after {
        target_index += 1;
    }
    items.insert(target_index.min(items.len()), entry);
    *items != before
}
fn sidebar_route_item_drag_id(item: SidebarRouteItem) -> &'static str {
    match item {
        SidebarRouteItem::Home => "Home",
        SidebarRouteItem::Favorites => "Favorites",
        SidebarRouteItem::Albums => "Albums",
        SidebarRouteItem::Tracks => "Tracks",
        SidebarRouteItem::Artists => "Artists",
        SidebarRouteItem::AlbumArtists => "AlbumArtists",
        SidebarRouteItem::Genres => "Genres",
        SidebarRouteItem::Folders => "Folders",
        SidebarRouteItem::Playlists => "Playlists",
        SidebarRouteItem::SmartPlaylists => "SmartPlaylists",
    }
}
fn sidebar_drag_route(id: &str) -> Option<SidebarRouteItem> {
    SidebarRouteItem::all()
        .into_iter()
        .find(|item| sidebar_route_item_drag_id(*item) == id)
}
fn sidebar_route_item_title(item: SidebarRouteItem) -> &'static str {
    match item {
        SidebarRouteItem::Home => "Home",
        SidebarRouteItem::Favorites => "Favorites",
        SidebarRouteItem::Albums => "Albums",
        SidebarRouteItem::Tracks => "Tracks",
        SidebarRouteItem::Artists => "Artists",
        SidebarRouteItem::AlbumArtists => "Album Artists",
        SidebarRouteItem::Genres => "Genres",
        SidebarRouteItem::Folders => "Folders",
        SidebarRouteItem::Playlists => "Playlists",
        SidebarRouteItem::SmartPlaylists => "Smart Playlists",
    }
}
