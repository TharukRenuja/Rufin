use std::rc::Rc;

use ::library::{Album, FavoriteItemId};
use adw::prelude::*;
use artwork::CandidateSet;
use domain::{LibraryLayout, LibraryListKey, LibraryListSettings, Route};

use super::favorites::album_favorite_key;
use super::layout::{
    album_grid_card_size, album_grid_page_size, home_album_card_size, home_album_content_width,
    home_album_page_size,
};
use super::{
    ActionButtonVariant, GRID_COVER_SIZE, MORE_ICON, PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON,
    Shell, configure_action_button, favorite_button_is_active, favorite_icon_button, icon_button,
    present_album_context_menu, set_favorite_button_active,
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

pub(super) fn album_cover_overlay(
    shell: &Rc<Shell>,
    album: &Album,
    size: i32,
    controller: &AppController,
) -> gtk::Widget {
    let overlay = cover_overlay(size);

    let album_button = gtk::Button::new();
    album_button.add_css_class("album-cover-button");
    album_button.add_css_class("flat");
    constrain_cover_widget(&album_button, size);
    clip_cover(&album_button);
    album_button.set_child(Some(&shell.cover_tile_for_candidates(
        CandidateSet::album(album),
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
    let play_controller = controller.clone();
    let album_id = album.id.clone();
    controls
        .play
        .connect_clicked(move |_| play_controller.play_album_now(album_id.clone()));

    let next_controller = controller.clone();
    let album_id = album.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = next_controller.cached_album_detail(&album_id) {
            for track in tracks.iter().rev() {
                next_controller.play_next(track.clone());
            }
        }
    });

    let last_controller = controller.clone();
    let album_id = album.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = last_controller.cached_album_detail(&album_id) {
            last_controller.play_last(tracks);
        }
    });

    if let Some(favorite) = controls.favorite.as_ref() {
        shell.register_favorite_button(album_favorite_key(&album.id), favorite);
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
    cover_hover_controls_with_favorite(size, play_label, favorite_active).0
}

pub(super) fn cover_hover_controls_with_favorite(
    size: i32,
    play_label: &str,
    favorite_active: bool,
) -> (CoverHoverControls, gtk::Button) {
    let mut controls = cover_play_hover_controls(size, play_label);
    let favorite = favorite_icon_button("Favorite");
    configure_action_button(&favorite, ActionButtonVariant::CoverCornerFavorite, None);
    favorite.set_halign(gtk::Align::End);
    favorite.set_valign(gtk::Align::Start);
    favorite.set_margin_top(COVER_CORNER_ACTION_INSET);
    favorite.set_margin_end(COVER_CORNER_ACTION_INSET);
    favorite.set_visible(false);
    set_favorite_button_active(&favorite, favorite_active);
    controls.favorite = Some(favorite.clone());
    (controls, favorite)
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
