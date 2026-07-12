use crate::controller::LibraryCommitProjection;
use crate::i18n::tr_with;

use super::now_playing_notification::{
    now_playing_notification_can_send, now_playing_notification_should_withdraw,
};
use super::*;

const MOUSE_BACK_BUTTON: u32 = 8;
const MOUSE_FORWARD_BUTTON: u32 = 9;
const SLOW_EVENT_BATCH_MS: u64 = 100;
const SLOW_PLAYBACK_EVENT_POLL_MS: u64 = 100;
const TRANSLATOR_CREDITS: &str = include_str!(concat!(env!("OUT_DIR"), "/translator_credits.txt"));
const KEY_SEEK_SECONDS: i32 = 10;
const KEY_VOLUME_STEP: f64 = 0.05;
const CONTROL_TOAST_TIMEOUT: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum FullscreenPlaybackRefresh {
    None,
    Visualizer,
    Static,
}

pub(in crate::ui) fn fullscreen_playback_refresh(
    previous: Option<&PlaybackView>,
    next: &PlaybackView,
) -> FullscreenPlaybackRefresh {
    let Some(previous) = previous else {
        return FullscreenPlaybackRefresh::Static;
    };
    if previous.transport.source_id != next.transport.source_id
        || previous.transport.current != next.transport.current
    {
        FullscreenPlaybackRefresh::Static
    } else if previous.transport.state != next.transport.state {
        FullscreenPlaybackRefresh::Visualizer
    } else {
        FullscreenPlaybackRefresh::None
    }
}

pub(in crate::ui) fn connect_shell_actions(
    shell: &Rc<Shell>,
    normal_main_menu: gtk::Button,
    compact_main_menu: gtk::Button,
) {
    install_window_actions(shell);
    install_mouse_history_buttons(shell);
    install_main_menu_shortcut(shell, normal_main_menu, compact_main_menu);
    connect_layout_resize(shell);
}

fn install_mouse_history_buttons(shell: &Rc<Shell>) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);

    let history_shell = Rc::clone(shell);
    click.connect_pressed(move |click, _, _, _| match click.current_button() {
        MOUSE_BACK_BUTTON => {
            click.set_state(gtk::EventSequenceState::Claimed);
            history_shell.go_back();
        }
        MOUSE_FORWARD_BUTTON => {
            click.set_state(gtk::EventSequenceState::Claimed);
            history_shell.go_forward();
        }
        _ => {}
    });

    shell.window.add_controller(click);
}

pub(in crate::ui) fn connect_lyrics_search_controls(shell: &Rc<Shell>) {
    let lyrics_shell = Rc::clone(shell);
    shell.lyrics_pane.connect_search_clicked(move || {
        if current_playback_track_id(&lyrics_shell.state.player.borrow()).is_none() {
            return;
        }
        lyrics_shell.present_lyrics_search_dialog();
    });
    let lyrics_shell = Rc::clone(shell);
    shell
        .lyrics_pane
        .connect_clear_auto_search_clicked(move || lyrics_shell.suppress_auto_lyrics_for_current());

    let fullscreen_lyrics_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .lyrics_pane
        .connect_search_clicked(move || {
            if current_playback_track_id(&fullscreen_lyrics_shell.state.player.borrow()).is_none() {
                return;
            }
            fullscreen_lyrics_shell.present_lyrics_search_dialog();
        });
    let fullscreen_lyrics_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .lyrics_pane
        .connect_clear_auto_search_clicked(move || {
            fullscreen_lyrics_shell.suppress_auto_lyrics_for_current()
        });
}
pub(in crate::ui) fn submit_lyrics_search(shell: &Rc<Shell>) {
    let Some(dialog) = shell.state.lyrics_search_dialog.borrow().clone() else {
        return;
    };
    if let Some(source) = dialog.search_debounce_source.borrow_mut().take() {
        source.remove();
    }
    if current_playback_media_key(&shell.state.player.borrow()).as_ref() != Some(&dialog.media_key)
    {
        dialog.dialog.close();
        return;
    }
    let artist_name = dialog.artist_entry.text().trim().to_string();
    let track_name = dialog.title_entry.text().trim().to_string();
    if artist_name.is_empty() && track_name.is_empty() {
        dialog.status.set_text(&tr("Enter an artist or song."));
        return;
    }
    clear_list_box(&dialog.list);
    dialog.status.set_text(&tr("Searching..."));
    debug!(
        artist_name = %artist_name,
        track_name = %track_name,
        "submitted manual lyric search"
    );
    shell
        .controller
        .search_lyrics_for_current(artist_name, track_name);
}
pub(in crate::ui) fn lyrics_search_response_matches_query(
    received_artist_name: &str,
    received_track_name: &str,
    current_artist_name: &str,
    current_track_name: &str,
) -> bool {
    lyrics_search_text_matches(received_artist_name, current_artist_name)
        && lyrics_search_text_matches(received_track_name, current_track_name)
}
pub(in crate::ui) fn lyrics_search_text_matches(received: &str, current: &str) -> bool {
    received.trim().to_lowercase() == current.trim().to_lowercase()
}
fn source_notice_message(notice: &SourceNotice) -> String {
    match notice {
        SourceNotice::Checking { source_name } => tr_with(
            "Checking {provider} server...",
            &[("provider", source_name.as_str())],
        ),
        SourceNotice::Connected => tr("Connected. Loading cached library..."),
        SourceNotice::SettingsSaved => tr("Source settings saved."),
        SourceNotice::NoChanges => tr("No changes to save."),
        SourceNotice::CacheCleared => tr("Cached library cleared."),
    }
}
pub(in crate::ui) fn queue_source_waits_for_snapshot(
    player: Option<&PlaybackView>,
    active_source_id: Option<&::library::SourceId>,
) -> bool {
    player.is_some_and(|player| active_source_id != Some(&player.transport.source_id))
}
pub(in crate::ui) fn queue_ready_for_library(
    player: Option<&PlaybackView>,
    library: &LibrarySnapshot,
) -> bool {
    let Some(player) = player else {
        return true;
    };
    library
        .source
        .as_ref()
        .is_some_and(|server| server.id == player.transport.source_id)
}
pub(in crate::ui) fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}
pub(in crate::ui) fn lyrics_search_result_has_content(result: &LyricsSearchResult) -> bool {
    result.provider != ExternalLyricsProvider::Lrclib
        || result
            .synced_lyrics
            .as_deref()
            .is_some_and(|lyrics| !lyrics.trim().is_empty())
        || result
            .plain_lyrics
            .as_deref()
            .is_some_and(|lyrics| !lyrics.trim().is_empty())
}
pub(in crate::ui) fn lyrics_result_title(result: &LyricsSearchResult) -> String {
    format!("{} - {}", result.artist_name, result.track_name)
}
pub(in crate::ui) fn lyrics_result_title_markup(result: &LyricsSearchResult) -> glib::GString {
    glib::markup_escape_text(&lyrics_result_title(result))
}
pub(in crate::ui) fn lyrics_result_subtitle(result: &LyricsSearchResult) -> String {
    let mut subtitle = result.provider.title().to_string();
    if !result.album_name.trim().is_empty() {
        if !subtitle.is_empty() {
            subtitle.push_str(" - ");
        }
        subtitle.push_str(&result.album_name);
    }
    if result.duration_seconds > 0 {
        if !subtitle.is_empty() {
            subtitle.push_str(" - ");
        }
        subtitle.push_str(&format_duration(result.duration_seconds));
    }
    if !subtitle.is_empty() {
        subtitle.push_str(" - ");
    }
    if result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
    {
        subtitle.push_str(&tr("Synced lyrics"));
    } else if result
        .plain_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
    {
        subtitle.push_str(&tr("Plain lyrics"));
    } else if result.provider != ExternalLyricsProvider::Lrclib {
        subtitle.push_str(&tr("Remote lyrics"));
    } else {
        subtitle.push_str(&tr("No lyrics"));
    }
    subtitle
}
pub(in crate::ui) fn lyrics_result_subtitle_markup(result: &LyricsSearchResult) -> glib::GString {
    glib::markup_escape_text(&lyrics_result_subtitle(result))
}
pub(in crate::ui) fn initial_window_size(width: Option<i32>, height: Option<i32>) -> (i32, i32) {
    sanitized_window_size(width, height).unwrap_or((DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT))
}
pub(in crate::ui) fn install_window_state_persistence(shell: &Rc<Shell>) {
    let save_shell = Rc::clone(shell);
    shell.application.connect_shutdown(move |_| {
        save_shell.controller.shutdown_playback();
        save_shell.save_window_state();
    });
}
pub(in crate::ui) fn connect_layout_resize(shell: &Rc<Shell>) {
    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |_, _| {
            resize_shell.update_layout();
            resize_shell.queue_responsive_route_render();
        });
    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("height"), move |_, _| {
            resize_shell.update_layout();
            resize_shell.queue_responsive_route_render();
        });

    let window = shell.window.clone();
    let resize_shell = Rc::clone(shell);
    window.connect_realize(move |window| {
        if let Some(surface) = window.surface() {
            let surface_resize_shell = Rc::clone(&resize_shell);
            surface.connect_width_notify(move |_| {
                surface_resize_shell.update_layout();
                surface_resize_shell.queue_responsive_route_render();
            });
            let surface_resize_shell = Rc::clone(&resize_shell);
            surface.connect_height_notify(move |_| {
                surface_resize_shell.update_layout();
                surface_resize_shell.queue_responsive_route_render();
            });
        }
        resize_shell.update_layout();
        resize_shell.queue_responsive_route_render();
    });

    let route_shell = Rc::clone(shell);
    shell
        .route_host
        .connect_notify_local(Some("width"), move |_, _| {
            route_shell.queue_responsive_route_render();
        });
}
pub(in crate::ui) fn install_window_actions(shell: &Rc<Shell>) {
    let go_back = gio::SimpleAction::new("go-back", None);
    let go_back_shell = Rc::clone(shell);
    go_back.connect_activate(move |_, _| go_back_shell.go_back());
    shell.window.add_action(&go_back);

    let go_forward = gio::SimpleAction::new("go-forward", None);
    let go_forward_shell = Rc::clone(shell);
    go_forward.connect_activate(move |_, _| go_forward_shell.go_forward());
    shell.window.add_action(&go_forward);

    let preferences = gio::SimpleAction::new("preferences", None);
    let preferences_shell = Rc::clone(shell);
    preferences.connect_activate(move |_, _| present_preferences_dialog(&preferences_shell));
    shell.window.add_action(&preferences);

    let toggle_left_sidebar = gio::SimpleAction::new("toggle-left-sidebar", None);
    let toggle_left_sidebar_shell = Rc::clone(shell);
    toggle_left_sidebar.connect_activate(move |_, _| {
        toggle_left_sidebar_shell.toggle_active_left_sidebar_size();
    });
    shell.window.add_action(&toggle_left_sidebar);

    let toggle_private_mode = gio::SimpleAction::new("toggle-private-mode", None);
    let private_mode_shell = Rc::clone(shell);
    toggle_private_mode.connect_activate(move |_, _| {
        let enabled = !private_mode_shell.state.settings.borrow().private_mode;
        private_mode_shell.set_private_mode(enabled);
    });
    shell.window.add_action(&toggle_private_mode);

    let shortcuts = gio::SimpleAction::new("show-shortcuts", None);
    let shortcuts_shell = Rc::clone(shell);
    shortcuts.connect_activate(move |_, _| show_shortcuts_dialog(&shortcuts_shell));
    shell.window.add_action(&shortcuts);

    let fullscreen = gio::SimpleAction::new("toggle-fullscreen", None);
    let fullscreen_shell = Rc::clone(shell);
    fullscreen.connect_activate(move |_, _| {
        if fullscreen_shell.window.is_fullscreen() {
            fullscreen_shell.window.unfullscreen();
        } else {
            fullscreen_shell.window.fullscreen();
        }
    });
    shell.window.add_action(&fullscreen);

    add_window_action(shell, "play-pause", &["<Control>space"], {
        let controller = shell.controller.clone();
        move || controller.play_pause()
    });
    add_window_action(shell, "previous-track", &["<Control>b"], {
        let controller = shell.controller.clone();
        move || controller.previous_track()
    });
    add_window_action(shell, "next-track", &["<Control>n"], {
        let controller = shell.controller.clone();
        move || controller.next_track()
    });
    add_window_action(shell, "seek-backward", &["<Control>Left"], {
        let shell = Rc::clone(shell);
        move || seek_by(&shell, -KEY_SEEK_SECONDS)
    });
    add_window_action(shell, "seek-forward", &["<Control>Right"], {
        let shell = Rc::clone(shell);
        move || seek_by(&shell, KEY_SEEK_SECONDS)
    });
    add_window_action(shell, "toggle-shuffle", &["<Control>s"], {
        let shell = Rc::clone(shell);
        move || toggle_shuffle_shortcut(&shell)
    });
    add_window_action(shell, "cycle-repeat", &["<Control>r"], {
        let shell = Rc::clone(shell);
        move || cycle_repeat_shortcut(&shell)
    });
    add_window_action(shell, "toggle-favorite", &["<Control>f"], {
        let shell = Rc::clone(shell);
        move || toggle_favorite_shortcut(&shell)
    });
    add_window_action(shell, "toggle-auto-dj", &["<Control>d"], {
        let shell = Rc::clone(shell);
        move || toggle_auto_dj_shortcut(&shell)
    });
    add_window_action(shell, "mute", &["<Control>m"], {
        let shell = Rc::clone(shell);
        move || toggle_mute_shortcut(&shell)
    });
    add_window_action(shell, "volume-up", &["<Control>plus", "<Control>equal"], {
        let shell = Rc::clone(shell);
        move || adjust_volume(&shell, KEY_VOLUME_STEP)
    });
    add_window_action(shell, "volume-down", &["<Control>minus"], {
        let shell = Rc::clone(shell);
        move || adjust_volume(&shell, -KEY_VOLUME_STEP)
    });
    add_window_action(shell, "toggle-queue", &["F9"], {
        let shell = Rc::clone(shell);
        move || shell.toggle_right_panel()
    });
    add_window_action(shell, "toggle-lyrics", &["<Control>l"], {
        let shell = Rc::clone(shell);
        move || shell.toggle_lyrics_panel()
    });
    let about = gio::SimpleAction::new("about", None);
    let about_shell = Rc::clone(shell);
    about.connect_activate(move |_, _| show_about_dialog(&about_shell));
    shell.window.add_action(&about);

    let release_notes = gio::SimpleAction::new("show-release-notes", None);
    let release_notes_shell = Rc::clone(shell);
    release_notes.connect_activate(move |_, _| release_notes_shell.present_release_notes());
    shell.window.add_action(&release_notes);

    shell
        .application
        .set_accels_for_action("win.go-back", &["<Alt>Left"]);
    shell
        .application
        .set_accels_for_action("win.go-forward", &["<Alt>Right"]);
    shell
        .application
        .set_accels_for_action("win.preferences", &["<Control>comma"]);
    shell
        .application
        .set_accels_for_action("win.show-shortcuts", &["<Control>question"]);
    shell
        .application
        .set_accels_for_action("win.toggle-fullscreen", &["F11"]);
}

fn add_window_action(
    shell: &Rc<Shell>,
    name: &str,
    accels: &[&str],
    activate: impl Fn() + 'static,
) {
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| activate());
    shell.window.add_action(&action);
    if !accels.is_empty() {
        shell
            .application
            .set_accels_for_action(&format!("win.{name}"), accels);
    }
}

fn seek_by(shell: &Shell, delta_seconds: i32) {
    let Some(seconds) = ({
        let player = shell.state.player.borrow();
        let Some(player) = player.as_ref() else {
            return;
        };
        let duration_seconds =
            (player.transport.duration_millis / 1_000).min(u64::from(u32::MAX)) as u32;
        if player.transport.current.is_none() || duration_seconds == 0 {
            None
        } else {
            let position_seconds =
                (player.transport.position_millis / 1_000).min(u64::from(u32::MAX)) as u32;
            let target = position_seconds as i32 + delta_seconds;
            Some(target.clamp(0, duration_seconds as i32) as u32)
        }
    }) else {
        return;
    };
    shell.controller.seek(seconds);
}

fn adjust_volume(shell: &Rc<Shell>, delta: f64) {
    let Some(volume) = shell
        .state
        .player
        .borrow()
        .as_ref()
        .map(|player| (player.controls.volume + delta).clamp(0.0, 1.0))
    else {
        return;
    };
    shell.apply_user_volume(volume);
}

fn toggle_shuffle_shortcut(shell: &Shell) {
    let Some(enabled) = shell
        .state
        .player
        .borrow()
        .as_ref()
        .map(|player| !player.controls.shuffle_enabled)
    else {
        return;
    };
    shell.controller.toggle_shuffle();
    let title = if enabled {
        tr("Shuffle on")
    } else {
        tr("Shuffle off")
    };
    shell.show_control_feedback_toast(title);
}

fn cycle_repeat_shortcut(shell: &Shell) {
    let Some(repeat_mode) = shell
        .state
        .player
        .borrow()
        .as_ref()
        .map(|player| player.controls.repeat_mode)
    else {
        return;
    };
    let title = match repeat_mode {
        playback::RepeatMode::Off => tr("Repeat all"),
        playback::RepeatMode::All => tr("Repeat one"),
        playback::RepeatMode::One => tr("Repeat off"),
    };
    shell.controller.cycle_repeat();
    shell.show_control_feedback_toast(title);
}

fn toggle_auto_dj_shortcut(shell: &Shell) {
    let Some(enabled) = shell
        .state
        .player
        .borrow()
        .as_ref()
        .map(|player| !player.controls.auto_dj_enabled)
    else {
        return;
    };
    shell.controller.toggle_auto_dj();
    let title = if enabled {
        tr("Auto DJ on")
    } else {
        tr("Auto DJ off")
    };
    shell.show_control_feedback_toast(title);
}

fn toggle_mute_shortcut(shell: &Rc<Shell>) {
    let Some(muted) = shell
        .state
        .player
        .borrow()
        .as_ref()
        .map(|player| !player.controls.muted)
    else {
        return;
    };
    shell.apply_user_muted(muted);
    let title = if muted { tr("Muted") } else { tr("Unmuted") };
    shell.show_control_feedback_toast(title);
}

fn toggle_favorite_shortcut(shell: &Rc<Shell>) {
    let Some(entry) = shell
        .state
        .player
        .borrow()
        .as_ref()
        .and_then(|player| player.transport.current.clone())
    else {
        return;
    };
    shell.set_favorite_with_feedback(
        ::library::FavoriteItemId::Track(entry.track.id.clone()),
        !entry.track.favorite,
        Some(&shell.player_controls.favorite_button),
    );
}
pub(in crate::ui) fn install_main_menu_shortcut(
    shell: &Rc<Shell>,
    normal_main_menu: gtk::Button,
    compact_main_menu: gtk::Button,
) {
    let key_controller = gtk::EventControllerKey::new();
    let shortcut_shell = Rc::clone(shell);
    key_controller.connect_key_pressed(move |_, key, _, state| {
        if key == gtk::gdk::Key::F10 && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
            match shortcut_shell.state.resolved_left_sidebar.get() {
                ResolvedLeftSidebarMode::Compact => {
                    navigation::popup_primary_menu(&shortcut_shell.compact_main_menu_popover);
                    compact_main_menu.grab_focus();
                }
                _ => {
                    navigation::popup_primary_menu(&shortcut_shell.normal_main_menu_popover);
                    normal_main_menu.grab_focus();
                }
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    shell.window.add_controller(key_controller);
}
pub(in crate::ui) fn show_shortcuts_dialog(shell: &Shell) {
    let dialog = adw::ShortcutsDialog::builder()
        .title(tr("Keyboard Shortcuts"))
        .build();
    let section = adw::ShortcutsSection::new(Some(&tr("General")));
    section.add(adw::ShortcutsItem::new(&tr("Back"), "Back <Alt>Left"));
    section.add(adw::ShortcutsItem::new(
        &tr("Forward"),
        "Forward <Alt>Right",
    ));
    section.add(adw::ShortcutsItem::new(&tr("Menu"), "F10"));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Preferences"),
        "win.preferences",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Keyboard Shortcuts"),
        "win.show-shortcuts",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Toggle Fullscreen"),
        "win.toggle-fullscreen",
    ));
    dialog.add(section);

    let section = adw::ShortcutsSection::new(Some(&tr("Playback")));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Play/Pause"),
        "win.play-pause",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Previous"),
        "win.previous-track",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Next"),
        "win.next-track",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Seek Backward"),
        "win.seek-backward",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Seek Forward"),
        "win.seek-forward",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Shuffle"),
        "win.toggle-shuffle",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Repeat"),
        "win.cycle-repeat",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Favorite"),
        "win.toggle-favorite",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Auto DJ"),
        "win.toggle-auto-dj",
    ));
    section.add(adw::ShortcutsItem::from_action(&tr("Mute"), "win.mute"));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Volume Up"),
        "win.volume-up",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Volume Down"),
        "win.volume-down",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Show/hide right sidebar"),
        "win.toggle-queue",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Show/hide lyrics"),
        "win.toggle-lyrics",
    ));
    dialog.add(section);
    present_light_dismiss_dialog(&dialog, &shell.window);
}
pub(in crate::ui) fn show_about_dialog(shell: &Shell) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Rufin")
        .application_icon("io.github.screwys.Rufin")
        .developer_name("screwy")
        .developers(["screwy <screwygit@proton.me>"])
        .translator_credits(TRANSLATOR_CREDITS)
        .version(env!("CARGO_PKG_VERSION"))
        .website("https://github.com/screwys/Rufin")
        .issue_url("https://github.com/screwys/Rufin/issues")
        .copyright("© 2026 screwy")
        .license_type(gtk::License::Custom)
        .license(
            "This application comes with absolutely no warranty and is licensed under GNU General Public Licence, version 3 or later.",
        )
        .comments(tr(
            "Thank you for trying out Rufin! If you have problems or suggestions, please open an issue in Github.",
        ))
        .build();
    present_light_dismiss_dialog(&dialog, &shell.window);
}

fn invalidate_sync_snapshot_pages(library: &mut LibrarySnapshot, delta: &LibraryDelta) {
    if delta.is_empty() {
        return;
    }
    if delta.reset.is_some() {
        library.tracks.clear();
        library.albums.clear();
        library.artists.clear();
        library.album_artists.clear();
        library.genres.clear();
        library.playlists.clear();
        library.favorites.clear();
        library.search = ::library::SearchResults::default();
        return;
    }
    if !delta.tracks.is_empty() {
        library.tracks.clear();
        library.search = ::library::SearchResults::default();
    }
    if !delta.albums.is_empty() {
        library.albums.clear();
        library.search = ::library::SearchResults::default();
    }
    if !delta.artists.is_empty() {
        library.artists.clear();
        library.search = ::library::SearchResults::default();
    }
    if !delta.album_artists.is_empty() {
        library.album_artists.clear();
        library.search = ::library::SearchResults::default();
    }
    if !delta.genres.is_empty() {
        library.genres.clear();
        library.search = ::library::SearchResults::default();
    }
    if playlist_snapshot_changed(delta) {
        library.playlists.clear();
        library.search = ::library::SearchResults::default();
    }
}

fn playlist_snapshot_changed(delta: &LibraryDelta) -> bool {
    !delta.playlists.added.is_empty()
        || !delta.playlists.deleted.is_empty()
        || !delta.playlists.fields.is_empty()
        || !delta.playlists.entries.is_empty()
        || !delta.playlists.cover_refs.is_empty()
}

pub(in crate::ui) fn install_event_pump(shell: &Rc<Shell>, receiver: Receiver<ControllerEvent>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local(Duration::from_millis(33), move || {
        let batch_started = Instant::now();
        let playback_poll_started = Instant::now();
        shell.controller.poll_playback_events();
        let playback_poll_ms = playback_poll_started.elapsed().as_millis() as u64;
        if playback_poll_ms >= SLOW_PLAYBACK_EVENT_POLL_MS {
            warn!(playback_poll_ms, "slow playback event poll");
        }
        let mut event_count = 0_u64;
        while let Ok(event) = receiver.try_recv() {
            event_count += 1;
            match event {
                ControllerEvent::Snapshot(snapshot) => shell.apply_library_snapshot(*snapshot),
                ControllerEvent::SourceSelectionChanged { selected_source } => {
                    {
                        let mut library = shell.state.library.borrow_mut();
                        library.selected_source = Some(selected_source.clone());
                        library.music_folders.clear();
                        library.selected_music_folder_id = None;
                    }
                    *shell.state.library_load.borrow_mut() = LibraryLoad::Switching {
                        target: selected_source,
                    };
                    shell.state.startup_route_render_pending.set(false);
                    shell.state.startup_route_revealed.set(false);
                    shell.state.startup_route_content_prepared.set(false);
                    shell.update_source_selector();
                    shell.render_startup_loading_view();
                    continue;
                }
                ControllerEvent::SourceSyncChanged(change) => {
                    shell.apply_source_sync_changed(change);
                }
                ControllerEvent::LibraryCommitted(update) => {
                    shell.apply_library_committed(*update);
                }
                ControllerEvent::LibraryDelta(delta) => {
                    shell.apply_library_delta(*delta);
                }
                ControllerEvent::HomeSectionsUpdated {
                    snapshot,
                    include_explore,
                } => {
                    let previous_sections = shell.state.library.borrow().home_sections.clone();
                    let source_id = snapshot.source.as_ref().map(|server| server.id.clone());
                    let prefetched_explore = prefetched_explore_from_snapshot(&snapshot);
                    let snapshot = *snapshot;
                    let sections = snapshot.home_sections.clone();
                    shell.replace_library_snapshot(snapshot);
                    shell.update_prefetched_explore_from_snapshot(
                        source_id,
                        prefetched_explore,
                        &sections,
                    );
                    if !include_explore {
                        shell.promote_cached_prefetched_explore();
                    }
                    shell.update_source_selector();
                    if matches!(shell.state.routes.borrow().current(), Route::Home)
                        && !shell.state.startup_route_revealed.get()
                    {
                        shell.state.startup_route_content_prepared.set(false);
                        shell.prepare_startup_route_content();
                        return glib::ControlFlow::Continue;
                    }
                    shell.refresh_changed_visible_home_sections(
                        &previous_sections,
                        &sections,
                        include_explore,
                    );
                }
                ControllerEvent::HomeSectionPrefetched { source_id, section } => {
                    let active_source_id = shell
                        .state
                        .library
                        .borrow()
                        .source
                        .as_ref()
                        .map(|server| server.id.clone());
                    if active_source_id.as_ref() == Some(&source_id) {
                        let prefetched = PrefetchedHomeSection { source_id, section };
                        *shell.state.prefetched_explore.borrow_mut() = Some(prefetched);
                        if matches!(shell.state.routes.borrow().current(), Route::Home)
                            && !shell.state.startup_route_revealed.get()
                        {
                            shell.state.startup_route_content_prepared.set(false);
                            shell.prepare_startup_route_content();
                            return glib::ControlFlow::Continue;
                        }
                    }
                }
                ControllerEvent::PlaylistChanged {
                    playlist_id,
                    snapshot,
                } => {
                    shell.replace_library_snapshot(*snapshot);
                    shell.update_source_selector();
                    refresh_context_playlist_picker(&shell);
                    let route = shell.state.routes.borrow().current().clone();
                    let playlist_route_changed = matches!(route, Route::Playlists)
                        || matches!(route, Route::PlaylistDetail(id) if id == playlist_id);
                    if playlist_route_changed {
                        shell.render_current_route_preserving_scroll();
                    }
                }
                ControllerEvent::SmartPlaylistChanged {
                    smart_playlist_id,
                    snapshot,
                } => {
                    shell.replace_library_snapshot(*snapshot);
                    shell.state.smart_playlists.borrow_mut().clear();
                    shell.state.smart_playlists_loaded.set(false);
                    shell.update_source_selector();
                    let route = shell.state.routes.borrow().current().clone();
                    if matches!(route, Route::SmartPlaylists) {
                        shell.navigate(Route::SmartPlaylistDetail(smart_playlist_id));
                    } else if matches!(
                        route,
                        Route::SmartPlaylistDetail(id) if id == smart_playlist_id
                    ) {
                        shell.render_current_route_preserving_scroll();
                    }
                }
                ControllerEvent::FavoriteChanged {
                    item_id,
                    favorite,
                    snapshot,
                } => {
                    shell.apply_favorite_changed(item_id, favorite, *snapshot);
                }
                ControllerEvent::FavoriteChangeFailed {
                    item_id,
                    previous_favorite,
                    error,
                } => {
                    shell.restore_failed_favorite_change(&item_id, previous_favorite);
                    warn!(%error, "favorite change failed");
                }
                ControllerEvent::QueuePage(queue_page) => {
                    if shell.apply_queue_page_projection(queue_page) {
                        shell.schedule_queue_panel_render();
                    }
                }
                ControllerEvent::PlaybackProduct(projection) => {
                    let projection = *projection;
                    let previous_player = shell.state.player.borrow().clone();
                    let previous_media = current_playback_media_key(&previous_player);
                    let previous_track = previous_player
                        .as_ref()
                        .and_then(|player| player.transport.current.as_ref())
                        .map(|entry| entry.track.id.clone());
                    let next_player = projection.view;
                    let next_media =
                        next_player
                            .transport
                            .current
                            .as_ref()
                            .map(|entry| playback::MediaKey {
                                source_id: next_player.transport.source_id.clone(),
                                track_id: entry.track.id.clone(),
                            });
                    let next_track = next_player
                        .transport
                        .current
                        .as_ref()
                        .map(|entry| entry.track.id.clone());
                    let notification_became_sendable = !now_playing_notification_can_send(
                        &shell.state.settings.borrow(),
                        previous_player.as_ref(),
                    ) && now_playing_notification_can_send(
                        &shell.state.settings.borrow(),
                        Some(&next_player),
                    );
                    let lyrics_timing_changed = previous_media != next_media
                        || previous_player
                            .as_ref()
                            .map(|player| player.transport.state)
                            != Some(next_player.transport.state)
                        || previous_player
                            .as_ref()
                            .map(|player| player.transport.position_millis)
                            != Some(next_player.transport.position_millis);
                    let fullscreen_refresh =
                        fullscreen_playback_refresh(previous_player.as_ref(), &next_player);
                    if let Some(queue_page) = projection.queue_page {
                        shell.apply_queue_page_projection(queue_page);
                    }
                    if shell.state.queue_filter.borrow().trim().is_empty()
                        && let Some(current_index) = next_player.queue.current_index
                        && shell.state.queue.borrow().as_ref().is_none_or(|page| {
                            page.query.follows_current()
                                && (page.revision != next_player.queue.revision
                                    || !page
                                        .rows
                                        .iter()
                                        .any(|row| row.absolute_index == current_index))
                        })
                    {
                        shell.request_queue_page(QueuePageQuery::current());
                    }
                    *shell.state.player.borrow_mut() = Some(next_player.clone());
                    if previous_track != next_track {
                        shell.refresh_current_route_now_playing_selections();
                        shell.sync_bottom_player_favorite();
                    }
                    shell.maybe_clear_player_seek_preview(
                        &next_player,
                        previous_track != next_track,
                    );
                    shell.update_bottom_player();
                    #[cfg(unix)]
                    let mut mpris_discontinuity = None;
                    let mut notification_started_run = None;
                    let mut media_changed_key = None;
                    for notice in projection.notices {
                        match notice {
                            crate::controller::PlaybackNotice::MediaChanged(media) => {
                                media_changed_key = Some(media.key);
                                shell.controller.request_waveform_for_current();
                                shell.controller.warm_waveforms_for_queue();
                            }
                            crate::controller::PlaybackNotice::Visualizer { levels, .. } => {
                                shell.apply_fullscreen_visualizer_levels(levels);
                            }
                            crate::controller::PlaybackNotice::PositionDiscontinuity(
                                discontinuity,
                            ) => {
                                #[cfg(unix)]
                                {
                                    mpris_discontinuity = Some(discontinuity);
                                }
                                #[cfg(not(unix))]
                                {
                                    let _ = discontinuity;
                                }
                            }
                            crate::controller::PlaybackNotice::RunStarted(run) => {
                                notification_started_run = Some(run);
                            }
                        }
                    }
                    let lyrics_media_changed = match media_changed_key.as_ref() {
                        Some(media_key) => previous_media.as_ref() != Some(media_key),
                        None => previous_media.is_some() && next_media.is_none(),
                    };
                    if now_playing_notification_should_withdraw(
                        &shell.state.settings.borrow(),
                        Some(&next_player),
                    ) {
                        shell.withdraw_now_playing_notification();
                    }
                    let waits_for_source_snapshot = {
                        let library = shell.state.library.borrow();
                        queue_source_waits_for_snapshot(
                            Some(&next_player),
                            library.source.as_ref().map(|server| &server.id),
                        )
                    };
                    if waits_for_source_snapshot {
                        if let Some(target) = shell.state.library.borrow().selected_source.clone() {
                            *shell.state.library_load.borrow_mut() =
                                LibraryLoad::Switching { target };
                        }
                        shell.state.startup_route_render_pending.set(false);
                        shell.state.startup_route_revealed.set(false);
                        shell.state.startup_route_content_prepared.set(false);
                        shell.render_startup_loading_view();
                        continue;
                    }
                    let switch_ready = {
                        let load = shell.state.library_load.borrow();
                        let library = shell.state.library.borrow();
                        matches!(
                            &*load,
                            LibraryLoad::Switching { target }
                                if library.selected_source.as_ref() == Some(target)
                                    && library.cache.is_committed()
                                    && queue_ready_for_library(Some(&next_player), &library)
                        )
                    };
                    if switch_ready {
                        shell.finish_source_switch();
                        continue;
                    }
                    if matches!(
                        &*shell.state.library_load.borrow(),
                        LibraryLoad::Switching { .. }
                    ) {
                        if lyrics_media_changed {
                            *shell.state.lyrics.borrow_mut() = None;
                            *shell.state.lyrics_loading_media.borrow_mut() = None;
                            shell.lyrics_pane.clear_follow_scroll_pause();
                            shell
                                .fullscreen_player
                                .lyrics_pane
                                .clear_follow_scroll_pause();
                            shell.cancel_scheduled_lyrics_highlight();
                        }
                        continue;
                    }
                    if lyrics_media_changed {
                        *shell.state.lyrics.borrow_mut() = None;
                        *shell.state.lyrics_loading_media.borrow_mut() = None;
                        shell.lyrics_pane.clear_follow_scroll_pause();
                        shell
                            .fullscreen_player
                            .lyrics_pane
                            .clear_follow_scroll_pause();
                        shell.cancel_scheduled_lyrics_highlight();
                        shell.render_lyrics_panel();
                    }
                    if notification_started_run
                        .is_some_and(|run| next_player.transport.run == Some(run))
                        || notification_became_sendable
                    {
                        shell.notify_now_playing(Some(&next_player));
                    }
                    match fullscreen_refresh {
                        FullscreenPlaybackRefresh::Static => shell.update_fullscreen_player(),
                        FullscreenPlaybackRefresh::Visualizer => {
                            shell.sync_fullscreen_visualizer_state()
                        }
                        FullscreenPlaybackRefresh::None => {}
                    }
                    if lyrics_timing_changed {
                        shell.update_lyrics_highlight();
                    }
                    #[cfg(unix)]
                    shell.update_mpris_player_after(mpris_discontinuity);
                    shell.schedule_queue_panel_render();
                }
                ControllerEvent::Waveform(waveform) => {
                    *shell.state.waveform.borrow_mut() = waveform;
                    shell.update_bottom_player();
                }
                ControllerEvent::Lyrics {
                    media_key,
                    generation,
                    lyrics,
                } => {
                    if shell.controller.lyrics_result_is_current(generation) {
                        shell.apply_loaded_lyrics_for_media(media_key, *lyrics);
                    }
                }
                ControllerEvent::LyricsSearchResults {
                    media_key,
                    generation,
                    artist_name,
                    track_name,
                    results,
                } => {
                    if shell.controller.lyrics_result_is_current(generation) {
                        shell.apply_lyrics_search_results(
                            media_key,
                            artist_name,
                            track_name,
                            results,
                        );
                    }
                }
                ControllerEvent::LyricsSearchFailed {
                    media_key,
                    generation,
                    artist_name,
                    track_name,
                    error,
                } => {
                    if shell.controller.lyrics_result_is_current(generation) {
                        shell.apply_lyrics_search_failed(media_key, artist_name, track_name, error);
                    }
                }
                ControllerEvent::SearchLoaded { key, results } => {
                    shell.apply_search_loaded(key, results);
                }
                ControllerEvent::SearchFailed { key, error } => {
                    shell.apply_search_failed(key, error);
                }
                ControllerEvent::LyricsSaved {
                    media_key,
                    generation,
                    path,
                    lyrics,
                } => {
                    if shell.controller.lyrics_result_is_current(generation) {
                        shell.apply_lyrics_saved(media_key, path, lyrics);
                    }
                }
                ControllerEvent::FolderLoaded {
                    request_id,
                    path,
                    detail,
                } => {
                    shell.apply_folder_loaded(request_id, path, detail);
                }
                ControllerEvent::FolderLoadFailed {
                    request_id,
                    path,
                    error,
                } => {
                    shell.apply_folder_load_failed(request_id, path, error);
                }
                ControllerEvent::Artwork(event) => {
                    let ready_path = match &event {
                        artwork::ArtworkEvent::Changed(projection) => match &projection.readiness {
                            artwork::Readiness::Ready(image) => {
                                Some(image.cache_path().to_path_buf())
                            }
                            _ => None,
                        },
                        artwork::ArtworkEvent::Invalidated(_) => None,
                    };
                    let update_playback_art = ready_path.as_ref().is_some_and(|ready_path| {
                        shell
                            .state
                            .player
                            .borrow()
                            .as_ref()
                            .and_then(|player| {
                                player.transport.current.as_ref().and_then(|entry| {
                                    shell.current_playback_cached_artwork_path(
                                        &player.transport.source_id,
                                        entry,
                                        THUMB_COVER_SIZE,
                                    )
                                })
                            })
                            .is_some_and(|artwork| artwork.path == *ready_path)
                    });
                    shell.apply_artwork_event(event);
                    if update_playback_art {
                        let player = shell.state.player.borrow().clone();
                        shell.refresh_now_playing_notification(player.as_ref());
                    }
                    #[cfg(unix)]
                    if update_playback_art {
                        shell.update_mpris_player();
                    }
                }
                ControllerEvent::ServerDiscovery {
                    servers,
                    status,
                    running,
                } => {
                    *shell.state.discovered_servers.borrow_mut() = servers;
                    *shell.state.server_discovery_status.borrow_mut() = status;
                    shell.state.server_discovery_running.set(running);
                    if shell.state.library.borrow().first_run {
                        shell.render_current_route();
                    }
                    shell.refresh_add_server_dialog();
                }
                ControllerEvent::SourceNotice(notice) => shell.apply_source_notice(notice),
                ControllerEvent::SourceTransitionFailed { source_id, error } => {
                    shell.apply_source_transition_failed(source_id, error);
                }
                ControllerEvent::Error(error) => {
                    shell.clear_pending_playlist_entry_selection();
                    warn!(%error, "controller error");
                }
            }
        }
        let batch_ms = batch_started.elapsed().as_millis() as u64;
        if batch_ms >= SLOW_EVENT_BATCH_MS {
            warn!(
                event_count,
                playback_poll_ms, batch_ms, "slow controller event pump"
            );
        }
        glib::ControlFlow::Continue
    });
}

impl Shell {
    fn apply_source_notice(self: &Rc<Self>, notice: SourceNotice) {
        let message = source_notice_message(&notice);
        match notice {
            SourceNotice::Checking { .. } | SourceNotice::Connected
                if matches!(
                    &*self.state.library_load.borrow(),
                    LibraryLoad::Connecting { .. }
                ) =>
            {
                let first_run = match &*self.state.library_load.borrow() {
                    LibraryLoad::Connecting { first_run, .. } => *first_run,
                    _ => false,
                };
                *self.state.library_load.borrow_mut() = LibraryLoad::Connecting {
                    stage: message,
                    first_run,
                };
                self.render_current_route();
            }
            _ => self.show_notice_toast(&message),
        }
    }

    fn apply_source_transition_failed(self: &Rc<Self>, source_id: Option<SourceId>, error: String) {
        warn!(%error, "source transition failed");
        let load = self.state.library_load.borrow().clone();
        match load {
            LibraryLoad::Connecting {
                first_run: true, ..
            } => {
                *self.state.library_load.borrow_mut() = LibraryLoad::Failed {
                    source_id,
                    message: error,
                };
                self.render_current_route();
            }
            LibraryLoad::Connecting {
                first_run: false, ..
            } => {
                *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
                self.render_current_route();
            }
            LibraryLoad::Switching { .. } | LibraryLoad::WaitingForFirstCommit { .. } => {
                *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_startup_route_reveal();
            }
            LibraryLoad::Ready | LibraryLoad::Failed { .. } => {}
        }
    }

    fn apply_library_snapshot(self: &Rc<Self>, snapshot: LibrarySnapshot) {
        let (previous_first_run, previous_source) = {
            let current = self.state.library.borrow();
            (current.first_run, current.selected_source.clone())
        };
        let entered_first_run = snapshot.first_run && !previous_first_run;
        let source_changed = previous_source != snapshot.selected_source;
        let source_id = snapshot.source.as_ref().map(|source| source.id.clone());
        let selected_source = snapshot.selected_source.clone();
        let first_run = snapshot.first_run;
        let has_cache = snapshot.cache.is_committed();
        let prefetched_explore = prefetched_explore_from_snapshot(&snapshot);
        let sections = snapshot.home_sections.clone();
        self.replace_library_snapshot(snapshot);

        if entered_first_run {
            self.state.server_discovery_started.set(false);
            self.state.server_discovery_running.set(false);
            *self.state.discovered_servers.borrow_mut() = Vec::new();
            *self.state.server_discovery_status.borrow_mut() = ServerDiscoveryStatus::Idle;
        }
        self.update_prefetched_explore_from_snapshot(
            source_id.clone(),
            prefetched_explore,
            &sections,
        );
        refresh_context_playlist_picker(self);
        *self.state.folder_state.borrow_mut() = FolderRouteState::default();
        self.update_source_selector();

        let load = self.state.library_load.borrow().clone();
        let recovers_failed_projection =
            failed_projection_snapshot_recovers(&load, source_id.as_ref(), has_cache);
        match load {
            LibraryLoad::Connecting { .. } if has_cache && source_id.is_some() => {
                *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_first_run_app_reveal();
                return;
            }
            LibraryLoad::Connecting { .. } => {
                self.render_current_route();
                return;
            }
            LibraryLoad::Switching { target }
                if selected_source.as_ref() == Some(&target)
                    && first_run
                    && source_id.is_some() =>
            {
                *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
                self.state.startup_route_render_pending.set(false);
                self.state.startup_route_revealed.set(true);
                self.state.startup_route_content_prepared.set(true);
                self.render_current_route();
                self.show_reconnect_notice_if_needed();
                return;
            }
            LibraryLoad::Switching { target }
                if selected_source.as_ref() == Some(&target) && has_cache =>
            {
                if queue_ready_for_library(
                    self.state.player.borrow().as_ref(),
                    &self.state.library.borrow(),
                ) {
                    self.finish_source_switch();
                } else {
                    self.render_startup_loading_view();
                }
                return;
            }
            LibraryLoad::Switching { .. } | LibraryLoad::WaitingForFirstCommit { .. } => {
                self.render_startup_loading_view();
                return;
            }
            LibraryLoad::Ready if !has_cache && !first_run => {
                if let Some(source_id) = source_id {
                    *self.state.library_load.borrow_mut() =
                        LibraryLoad::WaitingForFirstCommit { source_id };
                    self.state.startup_route_render_pending.set(false);
                    self.state.startup_route_revealed.set(false);
                    self.state.startup_route_content_prepared.set(false);
                    self.render_startup_loading_view();
                    return;
                }
            }
            LibraryLoad::Failed { .. } if recovers_failed_projection => {
                *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_startup_route_reveal();
                return;
            }
            LibraryLoad::Failed { .. } => {
                self.render_current_route();
                return;
            }
            LibraryLoad::Ready => {}
        }

        if source_changed {
            self.reset_cover_pipeline();
            self.navigate(Route::Home);
        } else {
            self.render_current_route_preserving_scroll();
        }
    }

    fn finish_source_switch(self: &Rc<Self>) {
        *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
        self.prepare_home_route();
        self.render_queue_panel();
        self.render_lyrics_panel();
        self.update_bottom_player();
        self.update_fullscreen_player();
        #[cfg(unix)]
        self.update_mpris_player();
        self.schedule_startup_route_reveal();
    }

    fn apply_source_sync_changed(self: &Rc<Self>, change: library_sync::SourceSyncChanged) {
        apply_source_sync_presentation(&mut self.state.source_syncs.borrow_mut(), &change);
        if matches!(
            change.phase,
            library_sync::SyncPhase::Idle | library_sync::SyncPhase::Failed
        ) {
            self.dismiss_source_sync_toast(&change.source_id);
        }
        let active = self
            .state
            .library
            .borrow()
            .source
            .as_ref()
            .is_some_and(|source| source.id == change.source_id);
        match change.phase {
            library_sync::SyncPhase::Running => {
                if active && self.state.library_load.borrow().blocks_library() {
                    if self.login_screen_active() {
                        self.render_current_route();
                    } else {
                        self.render_startup_loading_view();
                    }
                }
                if change.manual && !self.library_sync_status_visible_fullscreen() {
                    let status = source_sync_progress_text(&change);
                    self.show_or_update_source_sync_toast(&change.source_id, change.epoch, &status);
                }
            }
            library_sync::SyncPhase::Failed => {
                if let Some(failure) = change.failure {
                    warn!(
                        source_id = %change.source_id,
                        error = %failure,
                        "source sync failed"
                    );
                    if active && !self.state.library.borrow().cache.is_committed() {
                        *self.state.library_load.borrow_mut() = LibraryLoad::Failed {
                            source_id: Some(change.source_id),
                            message: failure.clone(),
                        };
                        self.render_current_route();
                    }
                }
            }
            library_sync::SyncPhase::Idle => {}
        }
    }

    fn apply_library_committed(self: &Rc<Self>, update: LibraryCommitUpdate) {
        let LibraryCommitUpdate { commit, projection } = update;
        let tracks_changed = commit.delta.reset.is_some() || !commit.delta.tracks.is_empty();
        let projection_error = match projection {
            None => return,
            Some(Ok(LibraryCommitProjection::Initial(snapshot))) => {
                if !initial_snapshot_matches_commit(&snapshot, &commit)
                    || !self.commit_matches_selected_source(&commit)
                {
                    return;
                }
                let waiting_for_first_commit = matches!(
                    &*self.state.library_load.borrow(),
                    LibraryLoad::WaitingForFirstCommit { source_id }
                        if source_id == &commit.source_id
                );
                self.apply_library_snapshot(*snapshot);
                if waiting_for_first_commit {
                    *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
                    self.schedule_startup_route_reveal();
                }
                return;
            }
            Some(Ok(LibraryCommitProjection::Current { counts, home })) => {
                if !self.commit_matches_selected_source(&commit) {
                    return;
                }
                if failed_projection_matches_source(
                    &self.state.library_load.borrow(),
                    &commit.source_id,
                ) {
                    self.controller.reload_snapshot();
                    return;
                }
                let applied = {
                    let mut library = self.state.library.borrow_mut();
                    apply_library_commit_to_snapshot(&mut library, &commit, Some(counts), home)
                };
                if !applied {
                    return;
                }
                None
            }
            Some(Err(error)) => {
                if !self.commit_matches_selected_source(&commit) {
                    return;
                }
                let first_projection = !self.state.library.borrow().cache.is_committed();
                let applied = {
                    let mut library = self.state.library.borrow_mut();
                    apply_library_commit_to_snapshot(&mut library, &commit, None, None)
                };
                if !applied {
                    return;
                }
                Some((first_projection, error))
            }
        };
        if tracks_changed {
            self.rebuild_track_index();
        }
        self.update_source_selector();
        self.apply_library_delta(commit.delta);
        if let Some((first_projection, error)) = projection_error {
            warn!(%error, "failed to project committed library state");
            if first_projection {
                *self.state.library_load.borrow_mut() = LibraryLoad::Failed {
                    source_id: Some(commit.source_id.clone()),
                    message: error.clone(),
                };
                self.render_current_route();
                self.controller.reload_snapshot();
            }
            return;
        }
        let load = self.state.library_load.borrow().clone();
        match load {
            LibraryLoad::Connecting { .. } => self.schedule_first_run_app_reveal(),
            LibraryLoad::Switching { .. } => self.finish_source_switch(),
            LibraryLoad::WaitingForFirstCommit { source_id } if source_id == commit.source_id => {
                *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_startup_route_reveal();
            }
            LibraryLoad::Ready
            | LibraryLoad::WaitingForFirstCommit { .. }
            | LibraryLoad::Failed { .. } => {}
        }
    }

    fn commit_matches_selected_source(&self, commit: &library_sync::LibraryCommitted) -> bool {
        let library = self.state.library.borrow();
        let Some(source) = library.source.as_ref() else {
            return false;
        };
        if source.id != commit.source_id {
            return false;
        }
        match library.selected_source.as_ref() {
            Some(LibrarySourceSelection::Source(source_id)) => source_id == &commit.source_id,
            Some(LibrarySourceSelection::Local) => {
                commit.source_id.as_str() == crate::controller::LOCAL_SOURCE_IDENTITY_ID
            }
            None => false,
        }
    }
}

fn failed_projection_matches_source(load: &LibraryLoad, source_id: &SourceId) -> bool {
    matches!(
        load,
        LibraryLoad::Failed {
            source_id: Some(failed_source_id),
            ..
        } if failed_source_id == source_id
    )
}

fn failed_projection_snapshot_recovers(
    load: &LibraryLoad,
    source_id: Option<&SourceId>,
    has_cache: bool,
) -> bool {
    has_cache
        && source_id.is_some_and(|source_id| failed_projection_matches_source(load, source_id))
}

fn apply_source_sync_presentation(
    presentations: &mut HashMap<SourceId, library_sync::SourceSyncChanged>,
    change: &library_sync::SourceSyncChanged,
) {
    match change.phase {
        library_sync::SyncPhase::Running => {
            presentations.insert(change.source_id.clone(), change.clone());
        }
        library_sync::SyncPhase::Idle | library_sync::SyncPhase::Failed => {
            presentations.remove(&change.source_id);
        }
    }
}

fn initial_snapshot_matches_commit(
    snapshot: &LibrarySnapshot,
    commit: &library_sync::LibraryCommitted,
) -> bool {
    snapshot.cache.revision() == commit.revision
        && snapshot
            .source
            .as_ref()
            .is_some_and(|source| source.id == commit.source_id)
}

pub(in crate::ui) fn apply_library_commit_to_snapshot(
    library: &mut LibrarySnapshot,
    commit: &library_sync::LibraryCommitted,
    counts: Option<crate::controller::LibraryCounts>,
    home: Option<crate::controller::LibraryHomeUpdate>,
) -> bool {
    let active = library
        .source
        .as_ref()
        .is_some_and(|source| source.id == commit.source_id);
    if !active || commit.revision <= library.cache.revision() {
        return false;
    }

    library.cache = LibraryCacheState::Committed {
        revision: commit.revision,
    };
    invalidate_sync_snapshot_pages(library, &commit.delta);
    if let Some(counts) = counts {
        library.cached_album_count = counts.albums;
        library.cached_track_count = counts.tracks;
        library.cached_artist_count = counts.artists;
        library.cached_album_artist_count = counts.album_artists;
        library.cached_genre_count = counts.genres;
        library.cached_playlist_count = counts.playlists;
    }
    if let Some(home) = home {
        library.home_sections = home.sections;
        library.prefetched_explore = home.prefetched_explore;
    }
    true
}

pub(in crate::ui) fn source_sync_progress_text(change: &library_sync::SourceSyncChanged) -> String {
    match change.progress {
        None => "Syncing library...".to_string(),
        Some(library_sync::Progress::LocalScan(progress)) => match progress.stage {
            library_sync::LocalScanStage::Walking => "Scanning folders...".to_string(),
            library_sync::LocalScanStage::ReadingTags => "Reading track metadata...".to_string(),
            library_sync::LocalScanStage::BuildingLibrary => "Preparing local cache...".to_string(),
        },
        Some(library_sync::Progress::CollectionStarted(collection)) => {
            format!("Fetching {}...", sync_collection_name(collection))
        }
        Some(library_sync::Progress::PageFetching {
            collection,
            fetched,
            total,
        }) => match total {
            Some(total) => format!(
                "Fetching {}, {fetched}/{total} fetched...",
                sync_collection_name(collection)
            ),
            None => format!(
                "Fetching {}, {fetched} fetched...",
                sync_collection_name(collection)
            ),
        },
        Some(library_sync::Progress::PageStaged {
            collection,
            fetched,
        }) => format!(
            "Cached {}, {fetched} ready...",
            sync_collection_name(collection)
        ),
        Some(library_sync::Progress::Finalizing) => "Finalizing library cache...".to_string(),
        Some(library_sync::Progress::Finished) => tr("Cached library ready"),
    }
}

fn sync_collection_name(collection: library_sync::Collection) -> &'static str {
    match collection {
        library_sync::Collection::Albums => "albums",
        library_sync::Collection::Tracks => "tracks",
        library_sync::Collection::MusicFolders => "music folders",
        library_sync::Collection::Artists => "artists",
        library_sync::Collection::AlbumArtists => "album artists",
        library_sync::Collection::Genres => "genres",
        library_sync::Collection::Playlists => "playlists",
        library_sync::Collection::HomeSections => "home sections",
    }
}

impl Shell {
    pub(in crate::ui) fn show_control_feedback_toast(&self, title: String) {
        if !self.state.settings.borrow().control_notifications_enabled {
            return;
        }
        let generation = self.state.control_feedback_generation.get() + 1;
        self.state.control_feedback_generation.set(generation);
        self.control_feedback_label.set_text(&title);
        self.control_feedback_label.set_visible(true);
        let label = self.control_feedback_label.clone();
        let active_generation = Rc::clone(&self.state.control_feedback_generation);
        glib::timeout_add_local_once(
            Duration::from_secs(u64::from(CONTROL_TOAST_TIMEOUT)),
            move || {
                if active_generation.get() == generation {
                    label.set_visible(false);
                }
            },
        );
    }

    pub(in crate::ui) fn show_notice_toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn library_sync_status_visible_fullscreen(&self) -> bool {
        self.login_screen_active() || !self.state.startup_route_revealed.get()
    }

    fn show_or_update_source_sync_toast(
        self: &Rc<Self>,
        source_id: &SourceId,
        _epoch: u64,
        message: &str,
    ) {
        if let Some(toast) = self.state.source_sync_toasts.borrow().get(source_id) {
            toast.set_title(message);
            toast.set_timeout(0);
            return;
        }

        let toast = adw::Toast::new(message);
        toast.set_timeout(0);
        self.toast_overlay.add_toast(toast.clone());
        self.state
            .source_sync_toasts
            .borrow_mut()
            .insert(source_id.clone(), toast);
    }

    fn dismiss_source_sync_toast(&self, source_id: &SourceId) {
        let toast = self.state.source_sync_toasts.borrow_mut().remove(source_id);
        if let Some(toast) = toast {
            toast.dismiss();
        }
    }
}

#[cfg(test)]
mod source_sync_handoff_tests {
    use super::*;

    fn sync_change(
        source_id: &SourceId,
        epoch: u64,
        phase: library_sync::SyncPhase,
        manual: bool,
    ) -> library_sync::SourceSyncChanged {
        library_sync::SourceSyncChanged {
            source_id: source_id.clone(),
            epoch,
            phase,
            progress: None,
            failure: None,
            manual,
        }
    }

    #[test]
    fn failed_projection_retries_and_recovers_only_for_its_cached_source() {
        let failed_source = SourceId::new("source-a");
        let other_source = SourceId::new("source-b");
        let load = LibraryLoad::Failed {
            source_id: Some(failed_source.clone()),
            message: "snapshot read failed".to_string(),
        };

        assert!(failed_projection_matches_source(&load, &failed_source));
        assert!(!failed_projection_matches_source(&load, &other_source));
        assert!(failed_projection_snapshot_recovers(
            &load,
            Some(&failed_source),
            true,
        ));
        assert!(!failed_projection_snapshot_recovers(
            &load,
            Some(&failed_source),
            false,
        ));
        assert!(!failed_projection_snapshot_recovers(
            &load,
            Some(&other_source),
            true,
        ));
    }

    #[test]
    fn automatic_idle_removes_only_its_source_presentation() {
        let automatic_source = SourceId::new("source-a");
        let manual_source = SourceId::new("source-b");
        let mut presentations = HashMap::new();
        apply_source_sync_presentation(
            &mut presentations,
            &sync_change(
                &automatic_source,
                3,
                library_sync::SyncPhase::Running,
                false,
            ),
        );
        apply_source_sync_presentation(
            &mut presentations,
            &sync_change(&manual_source, 7, library_sync::SyncPhase::Running, true),
        );

        apply_source_sync_presentation(
            &mut presentations,
            &sync_change(&automatic_source, 3, library_sync::SyncPhase::Idle, false),
        );

        assert!(!presentations.contains_key(&automatic_source));
        assert!(
            presentations
                .get(&manual_source)
                .is_some_and(|change| change.manual && change.epoch == 7)
        );
    }
}
