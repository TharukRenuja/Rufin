use std::rc::Rc;

use adw::prelude::*;
use rufin_core::DensityMode;

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
