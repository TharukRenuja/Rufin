use std::rc::Rc;

use adw::prelude::*;
use rufin_core::DensityMode;

use crate::i18n::tr;

use super::{Shell, text_button};

pub(super) fn present_preferences_dialog(shell: &Rc<Shell>) {
    let window = gtk::Window::builder()
        .title(tr("Preferences"))
        .default_width(520)
        .default_height(560)
        .modal(true)
        .transient_for(&shell.window)
        .build();

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_min_content_width(0);

    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
    wrapper.add_css_class("route-content");
    wrapper.set_margin_top(24);
    wrapper.set_margin_bottom(24);
    wrapper.set_margin_start(24);
    wrapper.set_margin_end(24);

    append_density_group(shell, &wrapper);
    append_lyrics_group(shell, &wrapper);
    append_server_group(shell, &wrapper);

    scroller.set_child(Some(&wrapper));
    window.set_child(Some(&scroller));
    window.present();
}

fn append_density_group(shell: &Rc<Shell>, wrapper: &gtk::Box) {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 12);
    group.add_css_class("settings-group");

    let heading = gtk::Label::new(Some(&tr("Layout density")));
    heading.add_css_class("section-heading");
    heading.set_xalign(0.0);

    let options = gtk::StringList::new(&[&tr("Auto"), &tr("Normal"), &tr("Compact")]);
    let dropdown = gtk::DropDown::new(Some(options), None::<gtk::Expression>);
    dropdown.set_selected(match shell.state.density_mode.get() {
        DensityMode::Auto => 0,
        DensityMode::Normal => 1,
        DensityMode::Compact => 2,
    });

    let shell = Rc::clone(shell);
    dropdown.connect_selected_notify(move |dropdown| {
        let density = match dropdown.selected() {
            1 => DensityMode::Normal,
            2 => DensityMode::Compact,
            _ => DensityMode::Auto,
        };
        shell.set_density_mode(density);
    });

    let note = gtk::Label::new(Some(&tr("Saved locally for the next launch.")));
    note.add_css_class("muted");
    note.set_wrap(true);
    note.set_xalign(0.0);

    group.append(&heading);
    group.append(&dropdown);
    group.append(&note);
    wrapper.append(&group);
}

fn append_lyrics_group(shell: &Rc<Shell>, wrapper: &gtk::Box) {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 12);
    group.add_css_class("settings-group");
    let heading = gtk::Label::new(Some(&tr("Lyrics")));
    heading.add_css_class("section-heading");
    heading.set_xalign(0.0);
    group.append(&heading);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_valign(gtk::Align::Center);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&tr("External lyric lookup")));
    title.set_xalign(0.0);
    let note = gtk::Label::new(Some(&tr(
        "Use Jellyfin remote lyric providers when server lyrics are unavailable.",
    )));
    note.add_css_class("muted");
    note.set_wrap(true);
    note.set_xalign(0.0);
    text.append(&title);
    text.append(&note);

    let external_switch = gtk::Switch::new();
    external_switch.set_active(shell.state.settings.borrow().external_lyrics_enabled);
    let shell = Rc::clone(shell);
    external_switch.connect_active_notify(move |switch| {
        shell.set_external_lyrics_enabled(switch.is_active());
    });

    row.append(&text);
    row.append(&external_switch);
    group.append(&row);
    wrapper.append(&group);
}

fn append_server_group(shell: &Rc<Shell>, wrapper: &gtk::Box) {
    let library = shell.state.library.borrow();
    let server_name = library
        .server
        .as_ref()
        .map(|server| server.name.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| tr("No server"));
    let username = library
        .username
        .as_deref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| tr("no account"));
    let server_url = library
        .server
        .as_ref()
        .map(|server| server.base_url.clone())
        .unwrap_or_else(|| tr("No active server"));
    let album_count = library.albums.len();
    let track_count = library.tracks.len();
    let sync_status = library.sync_status.clone();
    drop(library);

    let status = gtk::Label::new(Some(&format!(
        "{} ({username}): {} {} / {} {} - {}",
        server_name,
        album_count,
        tr("albums"),
        track_count,
        tr("tracks"),
        sync_status
    )));
    status.add_css_class("muted");
    status.set_xalign(0.0);
    status.set_wrap(true);
    wrapper.append(&status);

    let group = gtk::Box::new(gtk::Orientation::Vertical, 12);
    group.add_css_class("settings-group");
    let heading = gtk::Label::new(Some(&tr("Jellyfin Server")));
    heading.add_css_class("section-heading");
    heading.set_xalign(0.0);
    group.append(&heading);

    let details = gtk::Label::new(Some(&format!(
        "{}\n{}: {}\n{}: {} {} / {} {}",
        server_url,
        tr("User"),
        username,
        tr("Cached"),
        album_count,
        tr("albums"),
        track_count,
        tr("tracks")
    )));
    details.add_css_class("muted");
    details.set_wrap(true);
    details.set_xalign(0.0);
    group.append(&details);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let resync = text_button("view-refresh-symbolic", "Resync Library");
    let clear_cache = text_button("edit-clear-symbolic", "Clear Cached Library");
    let forget = text_button("user-trash-symbolic", "Forget Server");
    forget.add_css_class("destructive-action");

    let controller = shell.controller.clone();
    resync.connect_clicked(move |_| controller.resync_active_server());

    let clear_shell = Rc::clone(shell);
    clear_cache.connect_clicked(move |_| clear_shell.confirm_clear_cache());

    let forget_shell = Rc::clone(shell);
    forget.connect_clicked(move |_| forget_shell.confirm_forget_server());

    actions.append(&resync);
    actions.append(&clear_cache);
    actions.append(&forget);
    group.append(&actions);
    wrapper.append(&group);
}
