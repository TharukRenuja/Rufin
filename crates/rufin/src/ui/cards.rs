use std::rc::Rc;

use adw::prelude::*;
use domain::{
    Album, LibraryLayout, LibraryListKey, LibraryListSettings, Playlist, Route, SmartPlaylist,
    Track,
};
use source::FavoriteItemId;

use super::favorites::{album_favorite_key, track_favorite_key};
use super::layout::{
    album_grid_card_size, album_grid_page_size, home_album_card_size, home_album_content_width,
    home_album_page_size,
};
use super::{
    ActionButtonVariant, GRID_COVER_SIZE, MORE_ICON, PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON,
    Shell, THUMB_COVER_SIZE, configure_action_button, favorite_button_is_active,
    favorite_icon_button, icon_button, present_album_context_menu, present_playlist_context_menu,
    present_smart_playlist_context_menu, present_track_context_menu, set_favorite_button_active,
    stable_seed,
};
use crate::controller::AppController;

const COVER_CORNER_ACTION_INSET: i32 = 8;

impl Shell {
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
    clip_cover(&album_button);
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
    if let Some(favorite) = controls.favorite.as_ref() {
        shell.register_favorite_button(album_favorite_key(&album.id), favorite);
        if controller.is_some() {
            let shell = Rc::clone(shell);
            let album_id = album.id.clone();
            favorite.connect_clicked(move |button| {
                let favorite = !favorite_button_is_active(button);
                shell.set_favorite_with_feedback(
                    FavoriteItemId::Album(album_id.clone()),
                    favorite,
                    Some(button),
                );
            });
        }
    }
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

    overlay.upcast()
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
    clip_cover(&cover_button);
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

    if let Some(favorite) = controls.favorite.as_ref() {
        shell.register_favorite_button(track_favorite_key(&track.id), favorite);
        let shell = Rc::clone(shell);
        let track_id = track.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            shell.set_favorite_with_feedback(
                FavoriteItemId::Track(track_id.clone()),
                favorite,
                Some(button),
            );
        });
    }
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
    clip_cover(&playlist_button);
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
        controller.play_cached_playlist(playlist_id.clone());
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    controls.play_next.connect_clicked(move |_| {
        controller.play_cached_playlist_next(playlist_id.clone());
    });
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    controls.play_last.connect_clicked(move |_| {
        controller.play_cached_playlist_last(playlist_id.clone());
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
    clip_cover(&playlist_button);
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
    controls.play.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id) {
            controller.play_smart_playlist_detail(detail);
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

pub(super) fn cover_overlay(size: i32) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("cover-frame");
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
        let menu = icon_button(MORE_ICON, "More actions");
        configure_action_button(&menu, ActionButtonVariant::CoverCornerMenu, None);
        menu.set_halign(gtk::Align::Start);
        menu.set_valign(gtk::Align::End);
        menu.set_margin_start(COVER_CORNER_ACTION_INSET);
        menu.set_margin_bottom(COVER_CORNER_ACTION_INSET);
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
    configure_action_button(&favorite, ActionButtonVariant::CoverCornerFavorite, None);
    favorite.set_halign(gtk::Align::End);
    favorite.set_valign(gtk::Align::Start);
    favorite.set_margin_top(COVER_CORNER_ACTION_INSET);
    favorite.set_margin_end(COVER_CORNER_ACTION_INSET);
    favorite.set_visible(false);
    set_favorite_button_active(&favorite, favorite_active);
    controls.favorite = Some(favorite);
    controls
}

pub(super) fn cover_play_hover_controls(size: i32, play_label: &str) -> CoverHoverControls {
    let shade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shade.add_css_class("cover-hover-layer");
    constrain_cover_widget(&shade, size);
    shade.set_can_target(false);
    shade.set_visible(false);

    let play_next = icon_button(PLAY_NEXT_ICON, "Play Next");
    configure_action_button(
        &play_next,
        ActionButtonVariant::CoverSideTransport,
        Some(PLAY_NEXT_ICON),
    );
    play_next.set_visible(true);

    let play = icon_button(PLAY_ICON, play_label);
    configure_action_button(
        &play,
        ActionButtonVariant::CoverPrimaryTransport,
        Some(PLAY_ICON),
    );
    play.set_visible(true);

    let play_last = icon_button(PLAY_LATER_ICON, "Play Later");
    configure_action_button(
        &play_last,
        ActionButtonVariant::CoverSideTransport,
        Some(PLAY_LATER_ICON),
    );
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

pub(super) fn constrain_cover_widget(widget: &impl IsA<gtk::Widget>, size: i32) {
    widget.set_width_request(size);
    widget.set_height_request(size);
    widget.set_size_request(size, size);
    widget.set_hexpand(false);
    widget.set_halign(gtk::Align::Start);
}

pub(super) fn clip_cover(widget: &impl IsA<gtk::Widget>) {
    widget.set_overflow(gtk::Overflow::Hidden);
}

fn nonzero_usize(value: usize) -> Option<usize> {
    if value == 0 { None } else { Some(value) }
}
