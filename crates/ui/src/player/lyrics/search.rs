use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use lyrics::{ExternalLyricsProvider, LyricsQuery, LyricsSearchResult};
use tracing::debug;

use crate::format_duration;
use crate::player::state::{current_playback_media_id, current_playback_track_id};
use crate::shell::Shell;
use localization::tr;

#[derive(Clone)]
pub(crate) struct LyricsSearchDialog {
    pub(crate) dialog: adw::Dialog,
    pub(crate) media_id: playback::CurrentMediaId,
    pub(crate) artist_entry: gtk::Entry,
    pub(crate) title_entry: gtk::Entry,
    pub(crate) search_debounce_source: Rc<RefCell<Option<glib::SourceId>>>,
    pub(crate) list: gtk::ListBox,
    pub(crate) status: gtk::Label,
}

pub(crate) fn connect_lyrics_search_controls(shell: &Rc<Shell>) {
    super::settings::connect_lyrics_settings_controls(shell);

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
        .connect_offset_changed(move |value| offset_shell.apply_lyrics_offset_from_text(&value));
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
        .connect_offset_changed(move |value| offset_shell.apply_lyrics_offset_from_text(&value));
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
    if current_playback_media_id(&shell.playback.player.borrow()).as_ref() != Some(&dialog.media_id)
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
    shell.products.lyrics.search(
        dialog.media_id,
        LyricsQuery {
            artist_name,
            track_name,
        },
    );
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

#[cfg(test)]
mod tests {
    use lyrics::{ExternalLyricsProvider, LyricsSearchResult};

    use super::{
        lyrics_result_subtitle, lyrics_result_subtitle_markup, lyrics_result_title_markup,
        lyrics_search_response_matches_query, lyrics_search_result_has_content,
    };

    #[test]
    fn search_response_matches_the_complete_current_query() {
        assert!(lyrics_search_response_matches_query(
            "", "Opening", "", "Opening",
        ));
        assert!(lyrics_search_response_matches_query(
            "ATARASHII GAKKO",
            "Freaks",
            "atarashii gakko",
            "freaks",
        ));
        assert!(!lyrics_search_response_matches_query(
            "Earlier Artist",
            "Opening",
            "",
            "Opening",
        ));
        assert!(!lyrics_search_response_matches_query(
            "",
            "Opening Theme",
            "",
            "Opening",
        ));
    }

    #[test]
    fn search_result_labels_distinguish_content_and_deferred_providers() {
        let synced = result(
            ExternalLyricsProvider::Lrclib,
            Some("[00:01.00]line"),
            Some("line"),
        );
        assert!(lyrics_search_result_has_content(&synced));
        assert_eq!(
            lyrics_result_subtitle(&synced),
            "LRCLIB - Example Album - 1:35 - Synced lyrics"
        );

        let deferred = result(ExternalLyricsProvider::Netease, None, None);
        assert!(lyrics_search_result_has_content(&deferred));
        assert_eq!(
            lyrics_result_subtitle(&deferred),
            "NetEase - Example Album - 1:35 - Remote lyrics"
        );

        let empty = result(ExternalLyricsProvider::Lrclib, None, None);
        assert!(!lyrics_search_result_has_content(&empty));
        assert_eq!(
            lyrics_result_subtitle(&empty),
            "LRCLIB - Example Album - 1:35 - No lyrics"
        );
    }

    #[test]
    fn search_result_markup_escapes_visible_metadata() {
        let mut result = result(ExternalLyricsProvider::Lrclib, Some("[00:01.00]line"), None);
        result.artist_name = "Lady Gaga".to_string();
        result.track_name = "Poker Face (Piano & Voice Version) [Live]".to_string();
        result.album_name = "Hits & Rarities".to_string();

        assert_eq!(
            lyrics_result_title_markup(&result).as_str(),
            "Lady Gaga - Poker Face (Piano &amp; Voice Version) [Live]"
        );
        assert_eq!(
            lyrics_result_subtitle_markup(&result).as_str(),
            "LRCLIB - Hits &amp; Rarities - 1:35 - Synced lyrics"
        );
    }

    fn result(
        provider: ExternalLyricsProvider,
        synced_lyrics: Option<&str>,
        plain_lyrics: Option<&str>,
    ) -> LyricsSearchResult {
        LyricsSearchResult {
            provider,
            id: "12".to_string(),
            track_name: "Example Track".to_string(),
            artist_name: "Example Artist".to_string(),
            album_name: "Example Album".to_string(),
            duration_seconds: 95,
            synced_lyrics: synced_lyrics.map(str::to_string),
            plain_lyrics: plain_lyrics.map(str::to_string),
        }
    }
}
