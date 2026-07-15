use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use ::library::{ActiveLibraryQuery, Playlist, PlaylistId, Track};
use adw::prelude::*;
use artwork::ArtworkBinding;
use sources::SourcePlaylistOperation;

use crate::format_duration_units;
use crate::interactions::{close_context_surface, context_menu_button, context_menu_scroll_page};
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use crate::shell::cover::THUMB_COVER_SIZE;
use crate::shell::cover::presentation::stable_seed;
use localization::track_count_text;
use localization::{tr, tr_with};

use super::collection_routes::load_complete_cached_items;
use super::playlist_entries::playlist_operation_supported;

const CONTEXT_PLAYLIST_ROW_COVER_SIZE: i32 = 48;
const ADD_TO_PLAYLIST_DIALOG_WIDTH: i32 = 700;
const ADD_TO_PLAYLIST_DIALOG_HEIGHT: i32 = 510;

#[derive(Clone)]
struct PlaylistPickerRow {
    playlist: Playlist,
    row: gtk::Widget,
    check: gtk::CheckButton,
    haystack: String,
}
#[derive(Clone)]
pub(crate) struct PlaylistPickerHandle {
    list: gtk::Box,
    rows: Rc<RefCell<Vec<PlaylistPickerRow>>>,
    create: gtk::Button,
    search: gtk::SearchEntry,
    add_button: gtk::Button,
    can_create: bool,
}

pub(crate) struct PlaylistPickerState {
    pub(crate) active: RefCell<Option<PlaylistPickerHandle>>,
}
fn present_context_playlist_picker_dialog(
    shell: &Rc<Shell>,
    track_source: Rc<dyn Fn() -> Vec<Track>>,
) {
    let content = context_playlist_picker(shell, track_source);
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&tr("Add to Playlist"), "")));
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .title(tr("Add to Playlist"))
        .content_width(ADD_TO_PLAYLIST_DIALOG_WIDTH)
        .content_height(ADD_TO_PLAYLIST_DIALOG_HEIGHT)
        .child(&toolbar)
        .build();
    let shell_for_close = Rc::clone(shell);
    dialog.connect_closed(move |_| {
        *shell_for_close.playlist_picker.active.borrow_mut() = None;
    });
    present_light_dismiss_dialog(&dialog, &shell.chrome.window);
}
fn context_playlist_picker(
    shell: &Rc<Shell>,
    track_source: Rc<dyn Fn() -> Vec<Track>>,
) -> gtk::Box {
    let library_query = shell.library.query.borrow().clone();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("context-playlist-picker");
    root.set_margin_top(12);
    root.set_margin_bottom(14);
    root.set_margin_start(18);
    root.set_margin_end(18);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Type to search or create a new playlist")));
    root.append(&search);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let rows = Rc::new(RefCell::new(Vec::<PlaylistPickerRow>::new()));
    let create = playlist_create_row("");
    create.set_visible(false);
    list.append(&create);
    let add_button = gtk::Button::with_label(&tr("Add"));
    add_button.add_css_class("suggested-action");
    add_button.set_sensitive(false);
    let scroller = context_menu_scroll_page(&list);
    root.append(&scroller);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let skip = gtk::CheckButton::with_label(&tr("Don't duplicate"));
    skip.set_active(true);
    footer.append(&skip);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.connect_clicked(close_context_surface);
    footer.append(&cancel);
    footer.append(&add_button);
    root.append(&footer);

    let handle = PlaylistPickerHandle {
        list: list.clone(),
        rows: Rc::clone(&rows),
        create: create.clone(),
        search: search.clone(),
        add_button: add_button.clone(),
        can_create: shell.products.library.playlist_creation_supported(),
    };
    refresh_playlist_picker_rows(shell, &handle, &context_menu_playlists(shell));
    *shell.playlist_picker.active.borrow_mut() = Some(handle.clone());

    let handle_for_search = handle.clone();
    search.connect_search_changed(move |entry| {
        let text = entry.text();
        let label = create_playlist_label(text.trim());
        let query = text.trim().to_lowercase();
        handle_for_search.create.set_label(&label);
        sync_playlist_picker_filter(&handle_for_search, &query);
    });

    let library = shell.products.library.clone();
    let library_query_for_add = library_query.clone();
    let track_source_for_create = Rc::clone(&track_source);
    create.connect_clicked(move |_| {
        let name = search.text().trim().to_string();
        if !name.is_empty() {
            library.create_playlist(name, track_source_for_create());
            search.set_text("");
        }
    });

    let rows_for_add = Rc::clone(&rows);
    let library = shell.products.library.clone();
    let toast_overlay = shell.chrome.quick_toast_overlay.clone();
    add_button.connect_clicked(move |button| {
        let tracks = track_source();
        if tracks.is_empty() {
            close_context_surface(button);
            return;
        }
        let mut added_tracks = 0;
        let mut changed_playlists = 0;
        for row in rows_for_add
            .borrow()
            .iter()
            .filter(|row| row.check.is_active())
        {
            let tracks = playlist_tracks_to_add(
                library_query_for_add.as_ref(),
                &row.playlist.id,
                &tracks,
                skip.is_active(),
            );
            if !tracks.is_empty() {
                added_tracks += tracks.len();
                changed_playlists += 1;
                library.add_tracks_to_playlist(row.playlist.id.clone(), tracks);
            }
        }
        let toast = adw::Toast::new(&playlist_add_toast(added_tracks, changed_playlists));
        toast.set_timeout(2);
        toast_overlay.add_toast(toast);
        close_context_surface(button);
    });

    root
}
pub(crate) fn refresh_context_playlist_picker(shell: &Rc<Shell>) {
    let Some(handle) = shell.playlist_picker.active.borrow().clone() else {
        return;
    };
    refresh_playlist_picker_rows(shell, &handle, &context_menu_playlists(shell));
}
fn refresh_playlist_picker_rows(
    shell: &Rc<Shell>,
    handle: &PlaylistPickerHandle,
    playlists: &[Playlist],
) {
    while let Some(child) = handle.list.first_child() {
        handle.list.remove(&child);
    }
    handle.list.append(&handle.create);
    handle.rows.borrow_mut().clear();
    for playlist in playlists {
        let (row, check, haystack) = playlist_picker_row(shell, playlist);
        handle.list.append(&row);
        handle.rows.borrow_mut().push(PlaylistPickerRow {
            playlist: playlist.clone(),
            row: row.upcast::<gtk::Widget>(),
            check: check.clone(),
            haystack,
        });
        let rows_for_check = Rc::clone(&handle.rows);
        let add_for_check = handle.add_button.clone();
        check.connect_toggled(move |_| {
            update_playlist_picker_add_button(&rows_for_check, &add_for_check)
        });
    }
    let query = handle.search.text().trim().to_lowercase();
    sync_playlist_picker_filter(handle, &query);
}
fn sync_playlist_picker_filter(handle: &PlaylistPickerHandle, query: &str) {
    handle
        .create
        .set_visible(show_create_playlist_row(query, handle.can_create));
    for row in handle.rows.borrow().iter() {
        row.row
            .set_visible(query.is_empty() || row.haystack.contains(query));
    }
    update_playlist_picker_add_button(&handle.rows, &handle.add_button);
}
fn playlist_create_row(name: &str) -> gtk::Button {
    let button = gtk::Button::with_label(&create_playlist_label(name));
    button.add_css_class("flat");
    button.add_css_class("context-playlist-row");
    button.add_css_class("context-playlist-create-row");
    button.set_halign(gtk::Align::Fill);
    button
}
fn create_playlist_label(name: &str) -> String {
    format!("+ {} {}", tr("Create"), name)
}
fn show_create_playlist_row(query: &str, can_create: bool) -> bool {
    can_create && !query.trim().is_empty()
}
fn playlist_picker_row(
    shell: &Rc<Shell>,
    playlist: &Playlist,
) -> (gtk::Box, gtk::CheckButton, String) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("context-playlist-row");
    row.set_margin_top(4);
    row.set_margin_bottom(4);

    let check = gtk::CheckButton::new();
    row.append(&check);
    row.append(&playlist_picker_cover(shell, playlist));

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&playlist.name));
    title.add_css_class("context-playlist-title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&title);

    let meta = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    meta.add_css_class("context-playlist-meta");
    meta.append(&playlist_picker_meta(
        "rufin-route-tracks-symbolic",
        &track_count_text(playlist.track_count.into()),
    ));
    meta.append(&playlist_picker_meta(
        "appointment-soon-symbolic",
        &format_duration_units(playlist.duration_seconds),
    ));
    let genres = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    genres.add_css_class("context-playlist-genres");
    for genre in playlist.top_genres.iter().take(2) {
        genres.append(&playlist_genre_pill(genre));
    }
    genres.set_visible(genres.first_child().is_some());
    meta.append(&genres);
    text.append(&meta);
    row.append(&text);

    let haystack = format!(
        "{} {} {}",
        playlist.name,
        playlist.track_count,
        format_duration_units(playlist.duration_seconds)
    )
    .to_lowercase();
    (row, check, haystack)
}
fn playlist_genre_pill(name: &str) -> gtk::Label {
    let pill = gtk::Label::new(Some(name));
    pill.add_css_class("album-detail-genre-pill");
    pill
}
fn playlist_picker_cover(shell: &Rc<Shell>, playlist: &Playlist) -> gtk::Widget {
    let settings = shell.settings.current.borrow();
    let cover = shell.cover_tile_for_candidates(
        ArtworkBinding::playlist(playlist, settings.prefer_server_playlist_covers),
        stable_seed(playlist.id.as_str()),
        CONTEXT_PLAYLIST_ROW_COVER_SIZE,
        THUMB_COVER_SIZE,
    );
    cover.add_css_class("context-playlist-cover");
    cover
}
fn playlist_picker_meta(icon_name: &str, text: &str) -> gtk::Box {
    let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("muted");
    icon.set_pixel_size(13);
    item.append(&icon);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    item.append(&label);
    item
}
fn playlist_add_toast(added_tracks: usize, playlist_count: usize) -> String {
    if added_tracks == 0 {
        return tr("No songs added");
    }
    let track_count = added_tracks.to_string();
    let playlist_count_text = playlist_count.to_string();
    let args = [
        ("track_count", track_count.as_str()),
        ("playlist_count", playlist_count_text.as_str()),
    ];
    match (added_tracks == 1, playlist_count == 1) {
        (true, true) => tr_with(
            "{track_count} song added to {playlist_count} playlist",
            &args,
        ),
        (true, false) => tr_with(
            "{track_count} song added to {playlist_count} playlists",
            &args,
        ),
        (false, true) => tr_with(
            "{track_count} songs added to {playlist_count} playlist",
            &args,
        ),
        (false, false) => tr_with(
            "{track_count} songs added to {playlist_count} playlists",
            &args,
        ),
    }
}
fn playlist_tracks_to_add(
    query: Option<&ActiveLibraryQuery>,
    playlist_id: &PlaylistId,
    tracks: &[Track],
    skip_duplicates: bool,
) -> Vec<Track> {
    if !skip_duplicates {
        return tracks.to_vec();
    }
    let Some(detail) = query.and_then(|query| query.playlist_detail(playlist_id).ok().flatten())
    else {
        return tracks.to_vec();
    };
    if detail.entries.is_empty() {
        filter_existing_tracks(tracks, &detail.tracks)
    } else {
        filter_duplicate_tracks(tracks, &detail.entries)
    }
}
fn filter_duplicate_tracks(tracks: &[Track], entries: &[::library::PlaylistEntry]) -> Vec<Track> {
    filter_existing_track_ids(tracks, entries.iter().map(|entry| &entry.track.id))
}
fn filter_existing_tracks(tracks: &[Track], existing: &[Track]) -> Vec<Track> {
    filter_existing_track_ids(tracks, existing.iter().map(|track| &track.id))
}
fn filter_existing_track_ids<'a>(
    tracks: &[Track],
    existing: impl IntoIterator<Item = &'a ::library::TrackId>,
) -> Vec<Track> {
    let existing = existing.into_iter().collect::<HashSet<_>>();
    tracks
        .iter()
        .filter(|track| !existing.contains(&track.id))
        .cloned()
        .collect()
}
fn update_playlist_picker_add_button(
    rows: &Rc<RefCell<Vec<PlaylistPickerRow>>>,
    button: &gtk::Button,
) {
    button.set_sensitive(rows.borrow().iter().any(|row| row.check.is_active()));
}
pub(crate) fn context_menu_picker_button(
    label: &str,
    icon_name: &str,
    shell: &Rc<Shell>,
    track_source: Rc<dyn Fn() -> Vec<Track>>,
) -> gtk::Button {
    let button = context_menu_button(&tr(label), icon_name);
    let shell = Rc::clone(shell);
    button.connect_clicked(move |button| {
        close_context_surface(button);
        present_context_playlist_picker_dialog(&shell, Rc::clone(&track_source));
    });
    button
}
pub(crate) fn context_menu_can_add_to_playlist(shell: &Rc<Shell>) -> bool {
    shell.products.library.playlist_creation_supported()
        || !context_menu_playlists(shell).is_empty()
}
fn context_menu_playlists(shell: &Rc<Shell>) -> Vec<Playlist> {
    let Some(query) = shell.library.query.borrow().clone() else {
        return Vec::new();
    };
    let Ok(playlists) = load_complete_cached_items(|limit| query.playlists_page(0, limit)) else {
        return Vec::new();
    };
    playlists
        .into_iter()
        .filter(|playlist| {
            playlist_operation_supported(shell, playlist, SourcePlaylistOperation::AddTracks)
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use ::library::{AlbumId, Track, TrackId};

    use super::{filter_duplicate_tracks, playlist_add_toast};

    #[test]
    fn filter_duplicate_tracks_skips_existing_playlist_entries() {
        let tracks = vec![test_track(1, &[]), test_track(2, &[])];
        let entries = vec![::library::PlaylistEntry {
            entry_id: "entry-1".to_string(),
            track: test_track(1, &[]),
        }];

        let filtered = filter_duplicate_tracks(&tracks, &entries);

        assert_eq!(filtered, vec![test_track(2, &[])]);
    }

    #[test]
    fn playlist_add_toast_summarizes_added_tracks_and_playlists() {
        assert_eq!(playlist_add_toast(24, 3), "24 songs added to 3 playlists");
        assert_eq!(playlist_add_toast(1, 1), "1 song added to 1 playlist");
        assert_eq!(playlist_add_toast(0, 0), "No songs added");
    }

    fn test_track(index: usize, genres: &[&str]) -> Track {
        Track {
            id: TrackId::fake(index),
            album_id: AlbumId::fake(1),
            title: format!("Track {index}"),
            artist: "Artist".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2024,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: index as u16,
            image_ref: None,
            album_artwork: None,
            genres: genres.iter().map(|genre| genre.to_string()).collect(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        }
    }
}
