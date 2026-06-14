use std::rc::Rc;

use adw::prelude::*;
use domain::{
    Album, HomeSectionKind, LibraryLayout, LibraryListKey, LibraryListSettings, Playlist, Route,
    SmartPlaylist, Track,
};

use super::favorites::{album_favorite_key, track_favorite_key};
use super::layout::{
    HOME_ALBUM_CARD_LABEL_GAP, album_grid_card_size, album_grid_page_size,
    clamp_home_album_page_start, clipped_card_label_with_lines, constrain_single_line_card_label,
    home_album_card_height, home_album_card_size, home_album_content_width, home_album_page_size,
};
use super::{
    GRID_COVER_SIZE, HomeSectionState, PLAY_LATER_ICON, PLAY_NEXT_ICON, PlaylistEntryListState,
    Shell, THUMB_COVER_SIZE, add_card_label_link, add_link_hover, album_artist_route,
    favorite_button_is_active, favorite_icon_button, icon_button, install_album_context_menu,
    install_track_context_menu, loaded_tracks_window_play_activation,
    playlist_entry_play_activation, playlist_play_source_key, present_album_context_menu,
    present_playlist_context_menu, present_smart_playlist_context_menu, present_track_context_menu,
    selected_music_folder_id, set_favorite_button_active, smart_playlist_play_source_key,
    stable_seed, track_artist_route,
};
use crate::controller::AppController;

impl Shell {
    fn album_card_with_size(self: &Rc<Self>, album: &Album, size: i32) -> gtk::Widget {
        album_card_widget_with_size(self, album, size, Some(&self.controller))
    }

    pub(super) fn collection_card_grid_metrics(&self) -> (usize, i32) {
        let width = home_album_content_width(self);
        let current = nonzero_usize(self.state.collection_grid_columns.get());
        let columns = home_album_page_size(width, current);
        self.state.collection_grid_columns.set(columns);
        (columns, home_album_card_size(width, columns))
    }

    pub(super) fn collection_card_grid_metrics_for(
        &self,
        key: LibraryListKey,
        settings: &LibraryListSettings,
    ) -> (usize, i32) {
        if key == LibraryListKey::Albums && settings.layout == LibraryLayout::Grid {
            return self.album_collection_card_grid_metrics();
        }
        self.collection_card_grid_metrics()
    }

    fn album_collection_card_grid_metrics(&self) -> (usize, i32) {
        let width = home_album_content_width(self);
        let current = nonzero_usize(self.state.collection_grid_columns.get());
        let columns = album_grid_page_size(width, current);
        self.state.collection_grid_columns.set(columns);
        (columns, album_grid_card_size(width, columns))
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

    let artist = single_line_card_label(&album.artist, size, &["artist-label"]);
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
    install_album_context_menu(&card, shell, album.clone());
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

    let mut controls = cover_hover_controls(size, "Play album", album.favorite);
    let menu = controls.add_context_button();
    let menu_target = overlay.clone();
    let menu_shell = Rc::clone(shell);
    let menu_album = album.clone();
    menu.connect_clicked(move |_| {
        present_album_context_menu(
            menu_target.upcast_ref(),
            &menu_shell,
            super::context_album(&menu_shell, &menu_album),
            cover_context_point(size),
        );
    });
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        controls
            .play
            .connect_clicked(move |_| controller.play_album_now(album_id.clone()));
    }
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        controls.play_next.connect_clicked(move |_| {
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                for track in tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        });
    }
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        controls.play_last.connect_clicked(move |_| {
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                controller.play_last(tracks);
            }
        });
    }
    let favorite = controls.favorite.as_ref().expect("favorite button");
    shell.register_favorite_button(album_favorite_key(&album.id), favorite);
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_album_favorite(album_id.clone(), !favorite_button_is_active(button));
        });
    }
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

    overlay.upcast()
}

fn track_card_widget_with_size(shell: &Rc<Shell>, track: &Track, size: i32) -> gtk::Widget {
    let card = media_card(size);
    card.append(&track_cover_tile(shell, track, size));

    let title = single_line_card_label(&track.title, size, &["album-title"]);
    let title_clip = clipped_card_label_with_lines(&title, size, 1);
    add_link_hover(&title_clip, &title, &track.title);

    let artist = single_line_card_label(&track.artist, size, &["artist-label"]);
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
    install_track_context_menu(&card, shell, track.clone());
    card.upcast()
}

pub(super) fn track_cover_tile(shell: &Rc<Shell>, track: &Track, size: i32) -> gtk::Widget {
    track_play_tile(shell, track, size, None)
}

pub(super) fn track_play_tile(
    shell: &Rc<Shell>,
    track: &Track,
    size: i32,
    play_action: Option<Rc<dyn Fn()>>,
) -> gtk::Widget {
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
    let button_play_action = play_action.clone();
    cover_button.connect_clicked(move |_| {
        if let Some(play_action) = button_play_action.as_ref() {
            play_action();
        } else {
            controller.play_now(track_for_play.clone());
        }
    });
    overlay.set_child(Some(&cover_button));

    let mut controls = cover_hover_controls(size, "Play track", track.favorite);
    let menu = controls.add_context_button();
    let menu_target = overlay.clone();
    let menu_shell = Rc::clone(shell);
    let menu_track = track.clone();
    menu.connect_clicked(move |_| {
        present_track_context_menu(
            menu_target.upcast_ref(),
            &menu_shell,
            super::context_track(&menu_shell, &menu_track),
            cover_context_point(size),
        );
    });
    let controller = shell.controller.clone();
    let track_for_play = track.clone();
    let hover_play_action = play_action.clone();
    controls.play.connect_clicked(move |_| {
        if let Some(play_action) = hover_play_action.as_ref() {
            play_action();
        } else {
            controller.play_now(track_for_play.clone());
        }
    });

    let controller = shell.controller.clone();
    let track_for_play_next = track.clone();
    controls
        .play_next
        .connect_clicked(move |_| controller.play_next(track_for_play_next.clone()));

    let controller = shell.controller.clone();
    let track_for_play_last = track.clone();
    controls
        .play_last
        .connect_clicked(move |_| controller.play_last(vec![track_for_play_last.clone()]));

    let favorite = controls.favorite.as_ref().expect("favorite button");
    shell.register_favorite_button(track_favorite_key(&track.id), favorite);
    let controller = shell.controller.clone();
    let track_id = track.id.clone();
    favorite.connect_clicked(move |button| {
        controller.set_track_favorite(track_id.clone(), !favorite_button_is_active(button));
    });
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

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
    let artwork = crate::cover_art_policy::selected_playlist_artwork(
        playlist,
        &shell.state.settings.borrow(),
    );
    playlist_button.set_child(Some(&shell.cover_group_tile_for_artwork(
        &artwork,
        stable_seed(playlist.id.as_str()),
        size,
        THUMB_COVER_SIZE,
    )));
    let open_shell = Rc::clone(shell);
    let open_playlist_id = playlist.id.clone();
    playlist_button.connect_clicked(move |_| {
        open_shell.navigate(Route::PlaylistDetail(open_playlist_id.clone()))
    });
    overlay.set_child(Some(&playlist_button));

    let mut controls = cover_play_hover_controls(size, "Play playlist");
    let menu = controls.add_context_button();
    let menu_target = overlay.clone();
    let menu_shell = Rc::clone(shell);
    let menu_playlist = playlist.clone();
    menu.connect_clicked(move |_| {
        present_playlist_context_menu(
            menu_target.upcast_ref(),
            &menu_shell,
            menu_playlist.clone(),
            cover_context_point(size),
        );
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    controls.play.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id) {
            let state = PlaylistEntryListState::default();
            let activation = if detail.entries.is_empty() {
                loaded_tracks_window_play_activation(
                    playlist_play_source_key(playlist_id.clone(), &state),
                    detail.tracks.len(),
                    0,
                    |index| detail.tracks.get(index).cloned(),
                )
            } else {
                playlist_entry_play_activation(playlist_id.clone(), &detail.entries[0], 0, &state)
            };
            if let Some(activation) = activation {
                controller.play_activation(activation);
            }
        }
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id) {
            for track in detail.tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id) {
            controller.play_last(detail.tracks);
        }
    });
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

    overlay.upcast()
}

pub(super) fn smart_playlist_cover_tile(
    shell: &Rc<Shell>,
    playlist: &SmartPlaylist,
    size: i32,
) -> gtk::Widget {
    let overlay = cover_overlay(size);

    let playlist_button = gtk::Button::new();
    playlist_button.add_css_class("album-cover-button");
    playlist_button.add_css_class("flat");
    constrain_cover_widget(&playlist_button, size);
    let artwork = crate::cover_art_policy::selected_smart_playlist_artwork(playlist);
    playlist_button.set_child(Some(&shell.cover_group_tile_for_artwork(
        &artwork,
        stable_seed(playlist.id.as_str()),
        size,
        THUMB_COVER_SIZE,
    )));
    let open_shell = Rc::clone(shell);
    let open_playlist_id = playlist.id.clone();
    playlist_button.connect_clicked(move |_| {
        open_shell.navigate(Route::SmartPlaylistDetail(open_playlist_id.clone()))
    });
    overlay.set_child(Some(&playlist_button));

    let mut controls = cover_play_hover_controls(size, "Play smart playlist");
    let menu = controls.add_context_button();
    let menu_target = overlay.clone();
    let menu_shell = Rc::clone(shell);
    let menu_playlist = playlist.clone();
    menu.connect_clicked(move |_| {
        present_smart_playlist_context_menu(
            menu_target.upcast_ref(),
            &menu_shell,
            menu_playlist.clone(),
            cover_context_point(size),
        );
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    let selected_music_folder_id = selected_music_folder_id(shell);
    controls.play.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id)
            && let Some(activation) = loaded_tracks_window_play_activation(
                smart_playlist_play_source_key(
                    &detail.smart_playlist,
                    selected_music_folder_id.clone(),
                ),
                detail.tracks.len(),
                0,
                |index| detail.tracks.get(index).cloned(),
            )
        {
            controller.play_activation(activation);
        }
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id) {
            for track in detail.tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id) {
            controller.play_last(detail.tracks);
        }
    });
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

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

pub(super) struct CoverHoverControls {
    pub(super) shade: gtk::Box,
    pub(super) transport: gtk::Box,
    pub(super) play_next: gtk::Button,
    pub(super) play: gtk::Button,
    pub(super) play_last: gtk::Button,
    pub(super) favorite: Option<gtk::Button>,
    pub(super) menu: Option<gtk::Button>,
}

impl CoverHoverControls {
    pub(super) fn add_context_button(&mut self) -> gtk::Button {
        let menu = icon_button("view-more-symbolic", "More actions");
        menu.add_css_class("cover-hover-button");
        menu.add_css_class("cover-menu-button");
        menu.set_halign(gtk::Align::Start);
        menu.set_valign(gtk::Align::End);
        menu.set_margin_start(6);
        menu.set_margin_bottom(6);
        menu.set_visible(false);
        self.menu = Some(menu.clone());
        menu
    }

    pub(super) fn add_to_overlay(&self, overlay: &gtk::Overlay) {
        overlay.add_overlay(&self.shade);
        overlay.add_overlay(&self.transport);
        if let Some(menu) = self.menu.as_ref() {
            overlay.add_overlay(menu);
        }
        if let Some(favorite) = self.favorite.as_ref() {
            overlay.add_overlay(favorite);
        }
    }

    pub(super) fn connect_hover(&self, overlay: &gtk::Overlay) {
        let motion = gtk::EventControllerMotion::new();
        let shade_for_enter = self.shade.clone();
        let transport_for_enter = self.transport.clone();
        let favorite_for_enter = self.favorite.clone();
        let menu_for_enter = self.menu.clone();
        motion.connect_enter(move |_, _, _| {
            shade_for_enter.set_visible(true);
            transport_for_enter.set_visible(true);
            if let Some(favorite) = favorite_for_enter.as_ref() {
                favorite.set_visible(true);
            }
            if let Some(menu) = menu_for_enter.as_ref() {
                menu.set_visible(true);
            }
        });
        let shade_for_leave = self.shade.clone();
        let transport_for_leave = self.transport.clone();
        let favorite_for_leave = self.favorite.clone();
        let menu_for_leave = self.menu.clone();
        motion.connect_leave(move |_| {
            shade_for_leave.set_visible(false);
            transport_for_leave.set_visible(false);
            if let Some(favorite) = favorite_for_leave.as_ref() {
                favorite.set_visible(false);
            }
            if let Some(menu) = menu_for_leave.as_ref() {
                menu.set_visible(false);
            }
        });
        overlay.add_controller(motion);
    }
}

pub(super) fn cover_hover_controls(
    size: i32,
    play_label: &str,
    favorite_active: bool,
) -> CoverHoverControls {
    let mut controls = cover_play_hover_controls(size, play_label);
    let favorite = favorite_icon_button("Favorite");
    favorite.add_css_class("cover-hover-button");
    favorite.add_css_class("cover-favorite-button");
    favorite.set_halign(gtk::Align::End);
    favorite.set_valign(gtk::Align::Start);
    favorite.set_margin_top(6);
    favorite.set_margin_end(6);
    favorite.set_visible(false);
    set_favorite_button_active(&favorite, favorite_active);
    controls.favorite = Some(favorite);
    controls
}

pub(super) fn cover_play_hover_controls(size: i32, play_label: &str) -> CoverHoverControls {
    const SIDE_BUTTON_SIZE: i32 = 34;
    const PLAY_BUTTON_SIZE: i32 = 54;

    let shade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shade.add_css_class("cover-hover-layer");
    constrain_cover_widget(&shade, size);
    shade.set_can_target(false);
    shade.set_visible(false);

    let play_next = icon_button(PLAY_NEXT_ICON, "Play Next");
    play_next.add_css_class("cover-hover-button");
    play_next.add_css_class("cover-side-button");
    pin_cover_hover_button(&play_next, SIDE_BUTTON_SIZE);
    play_next.set_visible(true);

    let play = icon_button("media-playback-start-symbolic", play_label);
    play.add_css_class("cover-hover-button");
    play.add_css_class("cover-play-button");
    pin_cover_hover_button(&play, PLAY_BUTTON_SIZE);
    nudge_cover_play_icon(&play);
    play.set_visible(true);

    let play_last = icon_button(PLAY_LATER_ICON, "Play Later");
    play_last.add_css_class("cover-hover-button");
    play_last.add_css_class("cover-side-button");
    pin_cover_hover_button(&play_last, SIDE_BUTTON_SIZE);
    play_last.set_visible(true);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    transport.add_css_class("cover-hover-transport");
    transport.set_halign(gtk::Align::Center);
    transport.set_valign(gtk::Align::Center);
    transport.set_visible(false);
    transport.append(&play_next);
    transport.append(&play);
    transport.append(&play_last);

    CoverHoverControls {
        shade,
        transport,
        play_next,
        play,
        play_last,
        favorite: None,
        menu: None,
    }
}

pub(super) fn cover_context_point(size: i32) -> Option<(f64, f64)> {
    Some((20.0, f64::from(size.saturating_sub(20))))
}

fn pin_cover_hover_button(button: &gtk::Button, size: i32) {
    button.set_size_request(size, size);
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    button.set_hexpand(false);
    button.set_vexpand(false);
}

fn nudge_cover_play_icon(button: &gtk::Button) {
    let Some(child) = button.child() else {
        return;
    };
    if let Ok(image) = child.downcast::<gtk::Image>() {
        image.set_margin_start(2);
    }
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
