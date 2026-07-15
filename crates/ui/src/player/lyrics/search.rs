use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use metadata::{ExternalLyricsProvider, LyricsSearchResult};
use tracing::debug;

use crate::format_duration;
use crate::player::state::{current_playback_media_key, current_playback_track_id};
use crate::shell::Shell;
use localization::tr;

use super::view::install_offset_focus_dismissal;

#[derive(Clone)]
pub(crate) struct LyricsSearchDialog {
    pub(crate) dialog: adw::Dialog,
    pub(crate) media_key: playback::MediaKey,
    pub(crate) artist_entry: gtk::Entry,
    pub(crate) title_entry: gtk::Entry,
    pub(crate) search_debounce_source: Rc<RefCell<Option<glib::SourceId>>>,
    pub(crate) list: gtk::ListBox,
    pub(crate) status: gtk::Label,
}

pub(crate) fn connect_lyrics_search_controls(shell: &Rc<Shell>) {
    install_offset_focus_dismissal(
        &shell.chrome.window,
        &[
            &shell.right_panel.lyrics_pane,
            &shell.player_view.fullscreen_player.lyrics_pane,
        ],
    );

    let save_shell = Rc::clone(shell);
    shell
        .right_panel
        .lyrics_pane
        .connect_save_clicked(move || save_shell.present_current_lyrics_save_dialog());
    let lyrics_shell = Rc::clone(shell);
    shell
        .right_panel
        .lyrics_pane
        .connect_search_clicked(move || {
            if current_playback_track_id(&lyrics_shell.playback.player.borrow()).is_none() {
                return;
            }
            lyrics_shell.present_lyrics_search_dialog();
        });
    let lyrics_shell = Rc::clone(shell);
    shell
        .right_panel
        .lyrics_pane
        .connect_clear_auto_search_clicked(move || lyrics_shell.suppress_auto_lyrics_for_current());
    let offset_shell = Rc::clone(shell);
    shell
        .right_panel
        .lyrics_pane
        .connect_offset_decrease_clicked(move || offset_shell.adjust_lyrics_offset(-50));
    let offset_shell = Rc::clone(shell);
    shell
        .right_panel
        .lyrics_pane
        .connect_offset_increase_clicked(move || offset_shell.adjust_lyrics_offset(50));
    let offset_shell = Rc::clone(shell);
    shell
        .right_panel
        .lyrics_pane
        .connect_offset_committed(move |value| offset_shell.set_lyrics_offset_from_text(&value));

    let fullscreen_save_shell = Rc::clone(shell);
    shell
        .player_view
        .fullscreen_player
        .lyrics_pane
        .connect_save_clicked(move || fullscreen_save_shell.present_current_lyrics_save_dialog());
    let fullscreen_lyrics_shell = Rc::clone(shell);
    shell
        .player_view
        .fullscreen_player
        .lyrics_pane
        .connect_search_clicked(move || {
            if current_playback_track_id(&fullscreen_lyrics_shell.playback.player.borrow())
                .is_none()
            {
                return;
            }
            fullscreen_lyrics_shell.present_lyrics_search_dialog();
        });
    let fullscreen_lyrics_shell = Rc::clone(shell);
    shell
        .player_view
        .fullscreen_player
        .lyrics_pane
        .connect_clear_auto_search_clicked(move || {
            fullscreen_lyrics_shell.suppress_auto_lyrics_for_current()
        });
    let offset_shell = Rc::clone(shell);
    shell
        .player_view
        .fullscreen_player
        .lyrics_pane
        .connect_offset_decrease_clicked(move || offset_shell.adjust_lyrics_offset(-50));
    let offset_shell = Rc::clone(shell);
    shell
        .player_view
        .fullscreen_player
        .lyrics_pane
        .connect_offset_increase_clicked(move || offset_shell.adjust_lyrics_offset(50));
    let offset_shell = Rc::clone(shell);
    shell
        .player_view
        .fullscreen_player
        .lyrics_pane
        .connect_offset_committed(move |value| offset_shell.set_lyrics_offset_from_text(&value));
}

pub(crate) fn submit_lyrics_search(shell: &Rc<Shell>) {
    let Some(dialog) = shell.lyrics.search_dialog.borrow().clone() else {
        return;
    };
    if let Some(source) = dialog.search_debounce_source.borrow_mut().take() {
        source.remove();
    }
    if current_playback_media_key(&shell.playback.player.borrow()).as_ref()
        != Some(&dialog.media_key)
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
        .products
        .lyrics
        .search_current(artist_name, track_name);
}

pub(crate) fn lyrics_search_response_matches_query(
    received_artist_name: &str,
    received_track_name: &str,
    current_artist_name: &str,
    current_track_name: &str,
) -> bool {
    lyrics_search_text_matches(received_artist_name, current_artist_name)
        && lyrics_search_text_matches(received_track_name, current_track_name)
}

fn lyrics_search_text_matches(received: &str, current: &str) -> bool {
    received.trim().to_lowercase() == current.trim().to_lowercase()
}

pub(crate) fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

pub(crate) fn lyrics_search_result_has_content(result: &LyricsSearchResult) -> bool {
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

fn lyrics_result_title(result: &LyricsSearchResult) -> String {
    format!("{} - {}", result.artist_name, result.track_name)
}

pub(crate) fn lyrics_result_title_markup(result: &LyricsSearchResult) -> glib::GString {
    glib::markup_escape_text(&lyrics_result_title(result))
}

pub(crate) fn lyrics_result_subtitle(result: &LyricsSearchResult) -> String {
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

pub(crate) fn lyrics_result_subtitle_markup(result: &LyricsSearchResult) -> glib::GString {
    glib::markup_escape_text(&lyrics_result_subtitle(result))
}
