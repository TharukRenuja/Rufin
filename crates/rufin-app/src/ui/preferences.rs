use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{DensityMode, DiscordDisplayType, DiscordLinkType};

use crate::i18n::tr;

use super::Shell;

pub(super) fn present_preferences_dialog(shell: &Rc<Shell>) {
    let dialog = adw::PreferencesDialog::builder()
        .title(tr("Preferences"))
        .search_enabled(true)
        .content_width(560)
        .content_height(640)
        .build();

    let general_page = general_page(shell);
    let library_page = library_page(shell, &dialog);
    dialog.add(&general_page);
    dialog.add(&library_page);
    dialog.present(Some(&shell.window));
}

fn general_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("General"))
        .icon_name("preferences-system-symbolic")
        .build();

    let interface_group = adw::PreferencesGroup::builder()
        .title(tr("Interface"))
        .build();

    let density_titles = [tr("Auto"), tr("Normal"), tr("Compact")];
    let density_refs = density_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let density_options = gtk::StringList::new(&density_refs);
    let density_row = adw::ComboRow::builder()
        .title(tr("Left sidebar density"))
        .subtitle(tr("Choose when the left sidebar uses compact navigation."))
        .model(&density_options)
        .selected(density_index(shell.state.density_mode.get()))
        .build();
    let density_shell = Rc::clone(shell);
    density_row.connect_selected_notify(move |row| {
        density_shell.set_density_mode(density_from_index(row.selected()));
    });
    interface_group.add(&density_row);

    let settings = shell.state.settings.borrow().clone();
    let sidebar_row = adw::SwitchRow::builder()
        .title(tr("Show sidebar"))
        .subtitle(tr("Keep the queue sidebar visible in the main window."))
        .active(settings.right_panel_visible)
        .build();
    let sidebar_shell = Rc::clone(shell);
    sidebar_row.connect_active_notify(move |row| {
        sidebar_shell.set_right_panel_visible(row.is_active());
    });
    interface_group.add(&sidebar_row);

    let lyrics_panel_row = adw::SwitchRow::builder()
        .title(tr("Show Lyrics Panel"))
        .subtitle(tr("Keep the lyrics section visible below the queue."))
        .active(settings.lyrics_panel_visible)
        .build();
    let lyrics_panel_shell = Rc::clone(shell);
    lyrics_panel_row.connect_active_notify(move |row| {
        lyrics_panel_shell.set_lyrics_panel_visible(row.is_active());
    });
    interface_group.add(&lyrics_panel_row);

    page.add(&interface_group);

    let privacy_group = adw::PreferencesGroup::builder()
        .title(tr("Privacy"))
        .build();
    let private_row = adw::SwitchRow::builder()
        .title(tr("Private mode"))
        .subtitle(tr(
            "Stop playback reporting, external lyrics, notifications, and presence.",
        ))
        .active(settings.private_mode)
        .build();
    let private_shell = Rc::clone(shell);
    private_row.connect_active_notify(move |row| {
        private_shell.set_private_mode(row.is_active());
    });
    privacy_group.add(&private_row);

    let notifications_row = adw::SwitchRow::builder()
        .title(tr("Now playing notifications"))
        .subtitle(tr("Show a desktop notification when the track changes."))
        .active(settings.notifications_enabled)
        .build();
    let notifications_shell = Rc::clone(shell);
    notifications_row.connect_active_notify(move |row| {
        notifications_shell.set_notifications_enabled(row.is_active());
    });
    privacy_group.add(&notifications_row);
    page.add(&privacy_group);

    let lyrics_group = adw::PreferencesGroup::builder().title(tr("Lyrics")).build();
    let external_row = adw::SwitchRow::builder()
        .title(tr("External lyric lookup"))
        .subtitle(tr(
            "Use Jellyfin remote lyric providers when server lyrics are unavailable.",
        ))
        .active(settings.external_lyrics_enabled)
        .build();
    let external_shell = Rc::clone(shell);
    external_row.connect_active_notify(move |row| {
        external_shell.set_external_lyrics_enabled(row.is_active());
    });
    lyrics_group.add(&external_row);
    page.add(&lyrics_group);

    let discord_group = adw::PreferencesGroup::builder()
        .title(tr("Discord"))
        .description(tr(
            "Rich presence uses Discord IPC. Last.fm and MusicBrainz covers are public metadata lookups.",
        ))
        .build();
    let presence_row = adw::SwitchRow::builder()
        .title(tr("Rich presence"))
        .subtitle(tr("Show the current track in Discord."))
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
        .subtitle(tr("Choose which line Discord emphasizes."))
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
        tr("MusicBrainz and Last.fm"),
    ];
    let link_refs = link_titles.iter().map(String::as_str).collect::<Vec<_>>();
    let link_options = gtk::StringList::new(&link_refs);
    let link_row = adw::ComboRow::builder()
        .title(tr("Activity links and MusicBrainz covers"))
        .subtitle(tr(
            "Add external links and enable MusicBrainz cover fallback.",
        ))
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
        .subtitle(tr("Keep rich presence visible while playback is paused."))
        .active(settings.discord_show_paused)
        .build();
    let paused_shell = Rc::clone(shell);
    paused_row.connect_active_notify(move |row| {
        paused_shell.set_discord_show_paused(row.is_active());
    });
    discord_group.add(&paused_row);

    let listening_row = adw::SwitchRow::builder()
        .title(tr("Use listening activity"))
        .subtitle(tr("Set the Discord activity type to Listening."))
        .active(settings.discord_show_as_listening)
        .build();
    let listening_shell = Rc::clone(shell);
    listening_row.connect_active_notify(move |row| {
        listening_shell.set_discord_show_as_listening(row.is_active());
    });
    discord_group.add(&listening_row);

    let state_icon_row = adw::SwitchRow::builder()
        .title(tr("Show playback icon"))
        .subtitle(tr(
            "Show playing or paused icons when the Discord app assets exist.",
        ))
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

    page
}

fn library_page(shell: &Rc<Shell>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Library"))
        .icon_name("network-server-symbolic")
        .build();

    let library = shell.state.library.borrow();
    let username = library
        .username
        .as_deref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| tr("no account"));
    let server_name = library
        .server
        .as_ref()
        .map(|server| server.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| tr("No server"));
    let server_url = library
        .server
        .as_ref()
        .map(|server| server.base_url.clone())
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| tr("No active server"));

    let server_group = adw::PreferencesGroup::builder()
        .title(tr("Jellyfin Server"))
        .build();
    let server_row = adw::ActionRow::builder()
        .title(server_name)
        .subtitle(format!(
            "{}\n{}: {}\n{}: {} {} / {} {}",
            server_url,
            tr("User"),
            username,
            tr("Cached"),
            library.albums.len(),
            tr("albums"),
            library.tracks.len(),
            tr("tracks")
        ))
        .subtitle_lines(3)
        .build();
    server_group.add(&server_row);

    let status_row = adw::ActionRow::builder()
        .title(tr("Sync Status"))
        .subtitle(library.sync_status.clone())
        .build();
    server_group.add(&status_row);
    page.add(&server_group);
    drop(library);

    let actions_group = adw::PreferencesGroup::builder()
        .title(tr("Actions"))
        .build();

    let resync = button_row("Resync Library", "view-refresh-symbolic");
    let controller = shell.controller.clone();
    resync.connect_activated(move |_| controller.resync_active_server());
    actions_group.add(&resync);

    let clear_cache = button_row("Clear Cached Library", "edit-clear-symbolic");
    let clear_dialog = dialog.clone();
    let clear_shell = Rc::clone(shell);
    clear_cache.connect_activated(move |_| {
        clear_dialog.close();
        clear_shell.confirm_clear_cache();
    });
    actions_group.add(&clear_cache);

    let forget = button_row("Forget Server", "user-trash-symbolic");
    forget.add_css_class("destructive-action");
    let forget_dialog = dialog.clone();
    let forget_shell = Rc::clone(shell);
    forget.connect_activated(move |_| {
        forget_dialog.close();
        forget_shell.confirm_forget_server();
    });
    actions_group.add(&forget);

    page.add(&actions_group);
    page
}

fn button_row(title: &str, icon_name: &str) -> adw::ButtonRow {
    adw::ButtonRow::builder()
        .title(tr(title))
        .start_icon_name(icon_name)
        .end_icon_name("go-next-symbolic")
        .build()
}

fn density_index(density: DensityMode) -> u32 {
    match density {
        DensityMode::Auto => 0,
        DensityMode::Normal => 1,
        DensityMode::Compact => 2,
    }
}

fn density_from_index(index: u32) -> DensityMode {
    match index {
        1 => DensityMode::Normal,
        2 => DensityMode::Compact,
        _ => DensityMode::Auto,
    }
}

fn discord_display_index(display_type: DiscordDisplayType) -> u32 {
    match display_type {
        DiscordDisplayType::Application => 0,
        DiscordDisplayType::Song => 1,
        DiscordDisplayType::Artist => 2,
    }
}

fn discord_display_from_index(index: u32) -> DiscordDisplayType {
    match index {
        1 => DiscordDisplayType::Song,
        2 => DiscordDisplayType::Artist,
        _ => DiscordDisplayType::Application,
    }
}

fn discord_link_index(link_type: DiscordLinkType) -> u32 {
    match link_type {
        DiscordLinkType::None => 0,
        DiscordLinkType::LastFm => 1,
        DiscordLinkType::MusicBrainz => 2,
        DiscordLinkType::MusicBrainzLastFm => 3,
    }
}

fn discord_link_from_index(index: u32) -> DiscordLinkType {
    match index {
        1 => DiscordLinkType::LastFm,
        2 => DiscordLinkType::MusicBrainz,
        3 => DiscordLinkType::MusicBrainzLastFm,
        _ => DiscordLinkType::None,
    }
}
