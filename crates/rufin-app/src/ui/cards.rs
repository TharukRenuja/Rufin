use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{Album, HomeSectionKind, Playlist, Route, Track};

use super::favorites::{album_favorite_key, track_favorite_key};
use super::layout::{
    HOME_ALBUM_CARD_LABEL_GAP, clamp_home_album_page_start, clipped_card_label_with_lines,
    constrain_single_line_card_label, home_album_card_height, home_album_card_size,
    home_album_content_width, home_album_page_size,
};
use super::{
    GRID_COVER_SIZE, HomeSectionState, Shell, add_card_label_link, add_link_hover,
    album_artist_route, favorite_button_is_active, favorite_icon_button, icon_button,
    set_favorite_button_active, stable_seed, track_artist_route,
};
use crate::controller::AppController;

impl Shell {
    fn album_card_with_size(self: &Rc<Self>, album: &Album, size: i32) -> gtk::Widget {
        album_card_widget_with_size(self, album, size, Some(&self.controller))
    }

    pub(super) fn responsive_card_grid_metrics(&self) -> (usize, i32) {
        let width = home_album_content_width(self);
        let current = nonzero_usize(self.state.card_grid_columns.get());
        let columns = home_album_page_size(width, current);
        self.state.card_grid_columns.set(columns);
        (columns, home_album_card_size(width, columns))
    }
}

pub(super) fn render_home_album_page(
    shell: &Rc<Shell>,
    row: &gtk::Box,
    previous: &gtk::Button,
    next: &gtk::Button,
    section_kind: HomeSectionKind,
    albums: &[Album],
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }

    if albums.is_empty() {
        previous.set_sensitive(false);
        next.set_sensitive(false);
        return;
    }

    let (page_start, page_size, card_size) = home_page_metrics(shell, section_kind, albums.len());
    let page_end = page_start.saturating_add(page_size).min(albums.len());

    previous.set_sensitive(page_start > 0);
    next.set_sensitive(page_end < albums.len());

    for album in &albums[page_start..page_end] {
        row.append(&shell.album_card_with_size(album, card_size));
    }
}

pub(super) fn render_home_track_page(
    shell: &Rc<Shell>,
    row: &gtk::Box,
    previous: &gtk::Button,
    next: &gtk::Button,
    section_kind: HomeSectionKind,
    tracks: &[Track],
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }

    if tracks.is_empty() {
        previous.set_sensitive(false);
        next.set_sensitive(false);
        return;
    }

    let (page_start, page_size, card_size) = home_page_metrics(shell, section_kind, tracks.len());
    let page_end = page_start.saturating_add(page_size).min(tracks.len());

    previous.set_sensitive(page_start > 0);
    next.set_sensitive(page_end < tracks.len());

    for track in &tracks[page_start..page_end] {
        row.append(&track_card_widget_with_size(shell, track, card_size));
    }
}

fn home_page_metrics(
    shell: &Rc<Shell>,
    section_kind: HomeSectionKind,
    item_count: usize,
) -> (usize, usize, i32) {
    let width = home_album_content_width(shell);
    let page_start = {
        let mut states = shell.state.home_section_state.borrow_mut();
        let existing_page_size = states.get(&section_kind).map(|state| state.page_size);
        let page_size = home_album_page_size(width, existing_page_size);
        let state = states.entry(section_kind).or_insert(HomeSectionState {
            page_start: 0,
            page_size,
        });
        if state.page_size != page_size {
            state.page_start -= state.page_start % page_size.max(1);
            state.page_size = page_size;
        }
        state.page_start = clamp_home_album_page_start(state.page_start, page_size, item_count);
        state.page_start
    };
    let page_size = shell
        .state
        .home_section_state
        .borrow()
        .get(&section_kind)
        .map(|state| state.page_size)
        .unwrap_or_else(|| home_album_page_size(width, None));
    (
        page_start,
        page_size,
        home_album_card_size(width, page_size),
    )
}

fn album_card_widget_with_size(
    shell: &Rc<Shell>,
    album: &Album,
    size: i32,
    controller: Option<&AppController>,
) -> gtk::Widget {
    let card = media_card(size);
    card.append(&album_cover_tile(shell, album, size, controller));

    let title = single_line_card_label(&album.title, size, &["album-title"]);
    let title_clip = label_clip(&title, size);
    add_link_hover(&title_clip, &title, &album.title);

    let artist = single_line_card_label(&album.artist, size, &["muted"]);
    let artist_clip = label_clip(&artist, size);
    add_card_label_link(
        shell,
        &artist_clip,
        &artist,
        &album.artist,
        album_artist_route(album),
    );

    let year = single_line_card_label(&album.year.to_string(), size, &["muted"]);
    let year_clip = label_clip(&year, size);

    card.append(&title_clip);
    card.append(&artist_clip);
    card.append(&year_clip);
    card.upcast()
}

pub(super) fn album_cover_tile(
    shell: &Rc<Shell>,
    album: &Album,
    size: i32,
    controller: Option<&AppController>,
) -> gtk::Widget {
    let overlay = cover_overlay(size);

    let album_button = gtk::Button::new();
    album_button.add_css_class("album-cover-button");
    album_button.add_css_class("flat");
    constrain_cover_widget(&album_button, size);
    album_button.set_child(Some(&shell.cover_tile_for(
        album.image_ref.as_ref(),
        album.color_seed,
        size,
        GRID_COVER_SIZE,
    )));
    let open_shell = Rc::clone(shell);
    let open_album_id = album.id.clone();
    album_button
        .connect_clicked(move |_| open_shell.navigate(Route::AlbumDetail(open_album_id.clone())));
    overlay.set_child(Some(&album_button));

    let (shade, play, favorite) = cover_hover_controls(size, "Play album", album.favorite);
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        play.connect_clicked(move |_| controller.play_album_now(album_id.clone()));
    }
    shell.register_favorite_button(album_favorite_key(&album.id), &favorite);
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_album_favorite(album_id.clone(), !favorite_button_is_active(button));
        });
    }
    overlay.add_overlay(&shade);
    overlay.add_overlay(&play);
    overlay.add_overlay(&favorite);
    connect_cover_hover(&overlay, &shade, &play, Some(&favorite));

    overlay.upcast()
}

fn track_card_widget_with_size(shell: &Rc<Shell>, track: &Track, size: i32) -> gtk::Widget {
    let card = media_card(size);
    card.append(&track_cover_tile(shell, track, size));

    let title = single_line_card_label(&track.title, size, &["album-title"]);
    let title_clip = clipped_card_label_with_lines(&title, size, 1);
    add_link_hover(&title_clip, &title, &track.title);

    let artist = single_line_card_label(&track.artist, size, &["muted"]);
    let artist_clip = clipped_card_label_with_lines(&artist, size, 1);
    add_card_label_link(
        shell,
        &artist_clip,
        &artist,
        &track.artist,
        track_artist_route(track),
    );

    let album = single_line_card_label(&track.album, size, &["muted"]);
    let album_clip = clipped_card_label_with_lines(&album, size, 1);

    card.append(&title_clip);
    card.append(&artist_clip);
    card.append(&album_clip);
    card.upcast()
}

pub(super) fn track_cover_tile(shell: &Rc<Shell>, track: &Track, size: i32) -> gtk::Widget {
    let overlay = cover_overlay(size);

    let cover_button = gtk::Button::new();
    cover_button.add_css_class("album-cover-button");
    cover_button.add_css_class("flat");
    constrain_cover_widget(&cover_button, size);
    cover_button.set_child(Some(&shell.cover_tile_for(
        track.image_ref.as_ref(),
        stable_seed(track.id.as_str()),
        size,
        GRID_COVER_SIZE,
    )));
    let controller = shell.controller.clone();
    let track_for_play = track.clone();
    cover_button.connect_clicked(move |_| controller.play_now(track_for_play.clone()));
    overlay.set_child(Some(&cover_button));

    let (shade, play, favorite) = cover_hover_controls(size, "Play track", track.favorite);
    let controller = shell.controller.clone();
    let track_for_play = track.clone();
    play.connect_clicked(move |_| controller.play_now(track_for_play.clone()));

    shell.register_favorite_button(track_favorite_key(&track.id), &favorite);
    let controller = shell.controller.clone();
    let track_id = track.id.clone();
    favorite.connect_clicked(move |button| {
        controller.set_track_favorite(track_id.clone(), !favorite_button_is_active(button));
    });
    overlay.add_overlay(&shade);
    overlay.add_overlay(&play);
    overlay.add_overlay(&favorite);
    connect_cover_hover(&overlay, &shade, &play, Some(&favorite));

    overlay.upcast()
}

pub(super) fn playlist_cover_tile(
    shell: &Rc<Shell>,
    playlist: &Playlist,
    size: i32,
) -> gtk::Widget {
    let overlay = cover_overlay(size);

    let playlist_button = gtk::Button::new();
    playlist_button.add_css_class("album-cover-button");
    playlist_button.add_css_class("flat");
    constrain_cover_widget(&playlist_button, size);
    playlist_button.set_child(Some(&shell.cover_tile_for(
        playlist.image_ref.as_ref(),
        stable_seed(playlist.id.as_str()),
        size,
        GRID_COVER_SIZE,
    )));
    let open_shell = Rc::clone(shell);
    let open_playlist_id = playlist.id.clone();
    playlist_button.connect_clicked(move |_| {
        open_shell.navigate(Route::PlaylistDetail(open_playlist_id.clone()))
    });
    overlay.set_child(Some(&playlist_button));

    let (shade, play) = cover_play_hover_controls(size, "Play playlist");
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    play.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id) {
            controller.play_tracks_now(detail.tracks);
        }
    });
    overlay.add_overlay(&shade);
    overlay.add_overlay(&play);
    connect_cover_hover(&overlay, &shade, &play, None);

    overlay.upcast()
}

fn media_card(size: i32) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, HOME_ALBUM_CARD_LABEL_GAP);
    card.add_css_class("album-card");
    card.set_width_request(size);
    card.set_size_request(size, home_album_card_height(size));
    card.set_hexpand(false);
    card.set_halign(gtk::Align::Start);
    card
}

fn single_line_card_label(text: &str, size: i32, css_classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    for css_class in css_classes {
        label.add_css_class(css_class);
    }
    label.set_xalign(0.0);
    constrain_single_line_card_label(&label, size);
    label
}

fn label_clip(label: &gtk::Label, size: i32) -> gtk::Widget {
    clipped_card_label_with_lines(label, size, 1)
}

pub(super) fn cover_overlay(size: i32) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    constrain_cover_widget(&overlay, size);
    overlay
}

pub(super) fn cover_hover_controls(
    size: i32,
    play_label: &str,
    favorite_active: bool,
) -> (gtk::Box, gtk::Button, gtk::Button) {
    let shade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shade.add_css_class("cover-hover-layer");
    constrain_cover_widget(&shade, size);
    shade.set_can_target(false);
    shade.set_visible(false);

    let play = icon_button("media-playback-start-symbolic", play_label);
    play.add_css_class("cover-hover-button");
    play.add_css_class("cover-play-button");
    play.set_halign(gtk::Align::Center);
    play.set_valign(gtk::Align::Center);
    play.set_visible(false);

    let favorite = favorite_icon_button("Favorite");
    favorite.add_css_class("cover-hover-button");
    favorite.add_css_class("cover-favorite-button");
    favorite.set_halign(gtk::Align::End);
    favorite.set_valign(gtk::Align::Start);
    favorite.set_margin_top(8);
    favorite.set_margin_end(8);
    favorite.set_visible(false);
    set_favorite_button_active(&favorite, favorite_active);

    (shade, play, favorite)
}

pub(super) fn cover_play_hover_controls(size: i32, play_label: &str) -> (gtk::Box, gtk::Button) {
    let shade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shade.add_css_class("cover-hover-layer");
    constrain_cover_widget(&shade, size);
    shade.set_can_target(false);
    shade.set_visible(false);

    let play = icon_button("media-playback-start-symbolic", play_label);
    play.add_css_class("cover-hover-button");
    play.add_css_class("cover-play-button");
    play.set_halign(gtk::Align::Center);
    play.set_valign(gtk::Align::Center);
    play.set_visible(false);

    (shade, play)
}

pub(super) fn connect_cover_hover(
    overlay: &gtk::Overlay,
    shade: &gtk::Box,
    play: &gtk::Button,
    favorite: Option<&gtk::Button>,
) {
    let motion = gtk::EventControllerMotion::new();
    let shade_for_enter = shade.clone();
    let play_for_enter = play.clone();
    let favorite_for_enter = favorite.cloned();
    motion.connect_enter(move |_, _, _| {
        shade_for_enter.set_visible(true);
        play_for_enter.set_visible(true);
        if let Some(favorite) = favorite_for_enter.as_ref() {
            favorite.set_visible(true);
        }
    });
    let shade_for_leave = shade.clone();
    let play_for_leave = play.clone();
    let favorite_for_leave = favorite.cloned();
    motion.connect_leave(move |_| {
        shade_for_leave.set_visible(false);
        play_for_leave.set_visible(false);
        if let Some(favorite) = favorite_for_leave.as_ref() {
            favorite.set_visible(false);
        }
    });
    overlay.add_controller(motion);
}

pub(super) fn constrain_cover_widget(widget: &impl IsA<gtk::Widget>, size: i32) {
    widget.set_width_request(size);
    widget.set_height_request(size);
    widget.set_size_request(size, size);
    widget.set_hexpand(false);
    widget.set_halign(gtk::Align::Start);
}

fn nonzero_usize(value: usize) -> Option<usize> {
    if value == 0 { None } else { Some(value) }
}
