use std::{cell::Cell, rc::Rc};

use super::route::Route;
use ::library::{AlbumSummary, FavoriteItemId, Track};
use adw::prelude::*;
use artwork::ArtworkBinding;
use playback::QueuePlacement;

use super::collection_context::{present_album_context_menu, present_track_context_menu};
use super::collections::PlaybackTarget;
use super::home_layout::home_showcase_cover_size;
#[cfg(test)]
use super::library_fields::COLLECTION_GRID_MAX_CARD_WIDTH;
use super::library_fields::{COLLECTION_GRID_CARD_MARGIN, COLLECTION_GRID_MIN_CARD_WIDTH};
use crate::favorites::{
    album_favorite_key, favorite_button_is_active, favorite_icon_button,
    set_favorite_button_active, track_favorite_key,
};
use crate::interactions::install_context_menu_openers;
use crate::shell::Shell;
use crate::shell::actions::{
    ActionButtonVariant, COVER_PRIMARY_ACTION_SIZE, COVER_SIDE_ACTION_SIZE, MORE_ICON, PLAY_ICON,
    PLAY_LATER_ICON, PLAY_NEXT_ICON, configure_action_button, icon_button,
    icon_button_without_tooltip,
};
use crate::shell::cover::presentation::stable_seed;
use crate::shell::cover::{ArtworkTile, cover_fetch_size_for_display};

const COVER_CORNER_HORIZONTAL_INSET: i32 = 4;
const COVER_CORNER_VERTICAL_INSET: i32 = 8;
const COVER_TRANSPORT_COMPACT_GAP: i32 = 3;
const COVER_TRANSPORT_REGULAR_GAP: i32 = 8;

fn cover_hover_transport_width(spacing: i32) -> i32 {
    COVER_SIDE_ACTION_SIZE * 2 + COVER_PRIMARY_ACTION_SIZE + spacing * 2
}

fn cover_hover_transport_spacing(cover_width: i32) -> i32 {
    let available_spacing = cover_width
        .saturating_sub(COVER_CORNER_HORIZONTAL_INSET * 2)
        .saturating_sub(cover_hover_transport_width(0))
        / 2;
    available_spacing.clamp(COVER_TRANSPORT_COMPACT_GAP, COVER_TRANSPORT_REGULAR_GAP)
}

#[derive(Clone)]
pub(crate) struct ShowcaseCoverOverlay {
    root: gtk::Overlay,
    button: gtk::Button,
    tile: ArtworkTile,
    size: Rc<Cell<i32>>,
}

impl ShowcaseCoverOverlay {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub(crate) fn resize(&self, size: i32) {
        let size = size.max(1);
        if self.size.replace(size) == size {
            return;
        }
        constrain_cover_widget(&self.root, size);
        constrain_cover_widget(&self.button, size);
        self.tile.set_square_size(size);
    }
}

pub(crate) fn album_cover_overlay(
    shell: &Rc<Shell>,
    album: &AlbumSummary,
    size: i32,
) -> ShowcaseCoverOverlay {
    let overlay = cover_overlay(size);
    let album_value = &album.album;

    let album_button = gtk::Button::new();
    album_button.add_css_class("album-cover-button");
    album_button.add_css_class("flat");
    constrain_cover_widget(&album_button, size);
    clip_cover(&album_button);
    let tile = ArtworkTile::new_sized(size, size, album_value.color_seed);
    shell.bind_artwork_tile(
        &tile,
        ArtworkBinding::album_artwork(&album.artwork),
        album_value.color_seed,
        home_showcase_render_size(),
        home_showcase_fetch_size(),
    );
    album_button.set_child(Some(&tile.widget()));
    let open_shell = Rc::clone(shell);
    let open_album_id = album_value.id.clone();
    album_button
        .connect_clicked(move |_| open_shell.navigate(Route::AlbumDetail(open_album_id.clone())));
    overlay.set_child(Some(&album_button));

    let mut controls = cover_hover_controls(0, "Play album", album_value.favorite);
    let menu = controls.add_context_button();
    let menu_shell = Rc::clone(shell);
    let menu_album = album.clone();
    let open_menu: Rc<dyn Fn(&gtk::Widget, Option<(f64, f64)>)> =
        Rc::new(move |target, position| {
            present_album_context_menu(
                target,
                &menu_shell,
                menu_album.clone(),
                None,
                None,
                position,
            );
        });
    install_context_menu_openers(&overlay, Rc::clone(&open_menu));
    let menu_target = overlay.downgrade();
    menu.connect_clicked(move |_| {
        let Some(menu_target) = menu_target.upgrade() else {
            return;
        };
        open_menu(
            menu_target.upcast_ref(),
            elastic_cover_context_point(&menu_target),
        );
    });
    let play_shell = Rc::clone(shell);
    let play_album = album.clone();
    controls.play.connect_clicked(move |_| {
        play_loaded_album(&play_shell, &play_album, QueuePlacement::Now, true);
    });

    let next_shell = Rc::clone(shell);
    let next_album = album.clone();
    controls.play_next.connect_clicked(move |_| {
        play_loaded_album(&next_shell, &next_album, QueuePlacement::Next, false);
    });

    let last_shell = Rc::clone(shell);
    let last_album = album.clone();
    controls.play_last.connect_clicked(move |_| {
        play_loaded_album(&last_shell, &last_album, QueuePlacement::Last, false);
    });

    if let Some(favorite) = controls.favorite.as_ref() {
        shell
            .favorites
            .register_button(album_favorite_key(&album_value.id), favorite);
        let shell = Rc::clone(shell);
        let album_id = album_value.id.clone();
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

    ShowcaseCoverOverlay {
        root: overlay,
        button: album_button,
        tile,
        size: Rc::new(Cell::new(size)),
    }
}

pub(crate) fn track_cover_overlay(
    shell: &Rc<Shell>,
    track: Track,
    size: i32,
) -> ShowcaseCoverOverlay {
    let overlay = cover_overlay(size);
    let cover_button = gtk::Button::new();
    cover_button.add_css_class("album-cover-button");
    cover_button.add_css_class("flat");
    constrain_cover_widget(&cover_button, size);
    clip_cover(&cover_button);
    let seed = stable_seed(track.id.as_str());
    let tile = ArtworkTile::new_sized(size, size, seed);
    shell.bind_artwork_tile(
        &tile,
        ArtworkBinding::track(&track),
        seed,
        home_showcase_render_size(),
        home_showcase_fetch_size(),
    );
    cover_button.set_child(Some(&tile.widget()));
    let activate_shell = Rc::clone(shell);
    let activate_track = track.clone();
    cover_button.connect_clicked(move |_| {
        play_loaded_track(&activate_shell, activate_track.clone(), QueuePlacement::Now);
    });
    overlay.set_child(Some(&cover_button));

    let mut controls = cover_hover_controls(0, "Play track", track.favorite);
    let menu = controls.add_context_button();
    let menu_shell = Rc::clone(shell);
    let menu_track = track.clone();
    let open_menu: Rc<dyn Fn(&gtk::Widget, Option<(f64, f64)>)> =
        Rc::new(move |target, position| {
            present_track_context_menu(target, &menu_shell, menu_track.clone(), position);
        });
    install_context_menu_openers(&overlay, Rc::clone(&open_menu));
    let menu_target = overlay.downgrade();
    menu.connect_clicked(move |_| {
        let Some(menu_target) = menu_target.upgrade() else {
            return;
        };
        open_menu(
            menu_target.upcast_ref(),
            elastic_cover_context_point(&menu_target),
        );
    });
    let play_shell = Rc::clone(shell);
    let play_track = track.clone();
    controls.play.connect_clicked(move |_| {
        play_loaded_track(&play_shell, play_track.clone(), QueuePlacement::Now);
    });
    let next_shell = Rc::clone(shell);
    let next_track = track.clone();
    controls.play_next.connect_clicked(move |_| {
        play_loaded_track(&next_shell, next_track.clone(), QueuePlacement::Next);
    });
    let last_shell = Rc::clone(shell);
    let last_track = track.clone();
    controls.play_last.connect_clicked(move |_| {
        play_loaded_track(&last_shell, last_track.clone(), QueuePlacement::Last);
    });
    if let Some(favorite) = controls.favorite.as_ref() {
        shell
            .favorites
            .register_button(track_favorite_key(&track.id), favorite);
        let favorite_shell = Rc::clone(shell);
        let favorite_id = track.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                FavoriteItemId::Track(favorite_id.clone()),
                favorite,
                Some(button),
            );
        });
    }
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

    ShowcaseCoverOverlay {
        root: overlay,
        button: cover_button,
        tile,
        size: Rc::new(Cell::new(size)),
    }
}

fn home_showcase_render_size() -> i32 {
    home_showcase_cover_size(i32::MAX)
}

fn home_showcase_fetch_size() -> u32 {
    cover_fetch_size_for_display(home_showcase_render_size())
}

fn play_loaded_album(
    shell: &Shell,
    album: &AlbumSummary,
    placement: QueuePlacement,
    shuffled_start: bool,
) {
    let Some(request) = PlaybackTarget::Album(album.album.id.clone()).play_request(
        shell,
        placement,
        shuffled_start,
    ) else {
        return;
    };
    shell.products.playback.queue.play_loaded(request);
}

fn play_loaded_track(shell: &Shell, track: Track, placement: QueuePlacement) {
    let Some(selected) = shell.library.selected.borrow().as_ref().cloned() else {
        return;
    };
    shell
        .products
        .playback
        .queue
        .play_loaded(selected.one_track(track, placement));
}

pub(super) fn cover_overlay(size: i32) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("cover-frame");
    constrain_cover_widget(&overlay, size);
    overlay
}

pub(super) fn elastic_cover_overlay() -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("cover-frame");
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.set_halign(gtk::Align::Fill);
    overlay.set_valign(gtk::Align::Fill);
    overlay
}

mod collection_grid_card_inset_imp {
    use std::cell::Cell;

    use gtk::{glib, prelude::*, subclass::prelude::*};

    #[derive(Default)]
    pub struct CollectionGridCardInset {
        pub(super) minimum_content_width: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CollectionGridCardInset {
        const NAME: &'static str = "RufinCollectionGridCardInset";
        type Type = super::CollectionGridCardInset;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CollectionGridCardInset {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for CollectionGridCardInset {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.obj()
                .first_child()
                .map(|child| child.request_mode())
                .unwrap_or(gtk::SizeRequestMode::ConstantSize)
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };
            let total_inset = super::COLLECTION_GRID_CARD_MARGIN * 2;
            if orientation == gtk::Orientation::Horizontal {
                return super::collection_grid_card_horizontal_measure(
                    self.minimum_content_width.get(),
                );
            }
            let child_for_size = if for_size < 0 {
                -1
            } else {
                for_size.saturating_sub(total_inset).max(0)
            };
            let (minimum, natural, minimum_baseline, natural_baseline) =
                child.measure(orientation, child_for_size);
            let add_inset = |size: i32| size.saturating_add(total_inset);
            let add_baseline = |baseline: i32| {
                if baseline < 0 {
                    -1
                } else {
                    baseline.saturating_add(super::COLLECTION_GRID_CARD_MARGIN)
                }
            };
            (
                add_inset(minimum),
                add_inset(natural),
                add_baseline(minimum_baseline),
                add_baseline(natural_baseline),
            )
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.obj().first_child() else {
                return;
            };
            let (x, child_width) = super::collection_grid_card_inner_extent(width);
            let (y, child_height) = super::collection_grid_card_inner_extent(height);
            let child_baseline = if baseline < 0 {
                -1
            } else {
                baseline.saturating_sub(y).max(0)
            };
            let transform = gtk::gsk::Transform::new()
                .translate(&gtk::graphene::Point::new(x as f32, y as f32));
            child.allocate(child_width, child_height, child_baseline, Some(transform));
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(child) = self.obj().first_child() {
                self.obj().snapshot_child(&child, snapshot);
            }
        }
    }
}

fn collection_grid_card_horizontal_measure(minimum_content_width: i32) -> (i32, i32, i32, i32) {
    let slot_width = minimum_content_width
        .max(1)
        .saturating_add(COLLECTION_GRID_CARD_MARGIN.saturating_mul(2));
    (slot_width, slot_width, -1, -1)
}

gtk::glib::wrapper! {
    pub struct CollectionGridCardInset(ObjectSubclass<collection_grid_card_inset_imp::CollectionGridCardInset>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

fn collection_grid_card_inner_extent(allocation: i32) -> (i32, i32) {
    let allocation = allocation.max(0);
    let leading = COLLECTION_GRID_CARD_MARGIN.min(allocation / 2);
    (leading, allocation.saturating_sub(leading * 2))
}

#[cfg(test)]
mod collection_grid_card_inset_tests {
    use super::*;

    #[test]
    fn responsive_home_artwork_covers_every_presented_size() {
        let render_size = home_showcase_render_size();
        for width in 1..=1_200 {
            assert!(home_showcase_cover_size(width) <= render_size);
        }
    }

    #[test]
    fn preliminary_allocations_never_produce_a_negative_card_extent() {
        for allocation in 0..=COLLECTION_GRID_CARD_MARGIN * 2 {
            let (leading, inner) = collection_grid_card_inner_extent(allocation);
            assert!(leading >= 0);
            assert!(leading <= COLLECTION_GRID_CARD_MARGIN);
            assert!(inner >= 0);
            assert_eq!(leading * 2 + inner, allocation);
        }
    }

    #[test]
    fn grid_slot_width_uses_the_configured_card_width() {
        let expected = COLLECTION_GRID_MIN_CARD_WIDTH + COLLECTION_GRID_CARD_MARGIN * 2;
        assert_eq!(
            collection_grid_card_horizontal_measure(COLLECTION_GRID_MIN_CARD_WIDTH),
            (expected, expected, -1, -1)
        );
    }

    #[test]
    fn square_cover_height_follows_the_allocated_width() {
        assert_eq!(square_cover_vertical_measure(360), (360, 360, -1, -1));
        assert_eq!(square_cover_vertical_measure(180), (180, 180, -1, -1));
        assert_eq!(
            square_cover_vertical_measure(-1),
            (
                COLLECTION_GRID_MIN_CARD_WIDTH,
                COLLECTION_GRID_MIN_CARD_WIDTH,
                -1,
                -1
            )
        );
    }

    #[test]
    fn cover_hover_transport_spacing_uses_available_grid_width() {
        assert_eq!(
            cover_hover_transport_width(COVER_TRANSPORT_COMPACT_GAP),
            COLLECTION_GRID_MIN_CARD_WIDTH
        );

        let regular_width = cover_hover_transport_width(COVER_TRANSPORT_REGULAR_GAP)
            + COVER_CORNER_HORIZONTAL_INSET * 2;
        assert_eq!(cover_hover_transport_spacing(regular_width - 1), 7);
        assert_eq!(
            cover_hover_transport_spacing(regular_width),
            COVER_TRANSPORT_REGULAR_GAP
        );

        let mut previous_spacing = COVER_TRANSPORT_COMPACT_GAP;
        for cover_width in COLLECTION_GRID_MIN_CARD_WIDTH..=COLLECTION_GRID_MAX_CARD_WIDTH {
            let spacing = cover_hover_transport_spacing(cover_width);
            assert!(spacing >= previous_spacing);
            assert!(cover_hover_transport_width(spacing) <= cover_width);
            if spacing > COVER_TRANSPORT_COMPACT_GAP {
                assert!(
                    cover_hover_transport_width(spacing) + COVER_CORNER_HORIZONTAL_INSET * 2
                        <= cover_width
                );
            }
            previous_spacing = spacing;
        }
    }
}

pub(super) fn collection_grid_card_inset(
    child: &impl IsA<gtk::Widget>,
    minimum_content_width: i32,
) -> CollectionGridCardInset {
    use gtk::subclass::prelude::ObjectSubclassIsExt;

    let minimum_content_width = minimum_content_width.max(1);
    let inset: CollectionGridCardInset = gtk::glib::Object::new();
    inset.imp().minimum_content_width.set(minimum_content_width);
    inset.set_hexpand(true);
    inset.set_halign(gtk::Align::Fill);
    inset.set_valign(gtk::Align::Start);
    inset.set_accessible_role(gtk::AccessibleRole::Presentation);
    child.set_width_request(minimum_content_width);
    child.set_parent(&inset);
    inset
}

mod square_cover_frame_imp {
    use gtk::{glib, prelude::*, subclass::prelude::*};

    #[derive(Default)]
    pub struct SquareCoverFrame {
        pub(super) transport: glib::WeakRef<gtk::Box>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SquareCoverFrame {
        const NAME: &'static str = "RufinSquareCoverFrame";
        type Type = super::SquareCoverFrame;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SquareCoverFrame {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SquareCoverFrame {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Vertical {
                super::square_cover_vertical_measure(for_size)
            } else {
                self.obj()
                    .first_child()
                    .map(|child| child.measure(orientation, for_size))
                    .unwrap_or((0, 0, -1, -1))
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(transport) = self.transport.upgrade() {
                let spacing = super::cover_hover_transport_spacing(width);
                if transport.spacing() != spacing {
                    transport.set_spacing(spacing);
                }
            }
            if let Some(child) = self.obj().first_child() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(child) = self.obj().first_child() {
                self.obj().snapshot_child(&child, snapshot);
            }
        }
    }
}

fn square_cover_vertical_measure(for_size: i32) -> (i32, i32, i32, i32) {
    let size = if for_size >= 0 {
        for_size
    } else {
        COLLECTION_GRID_MIN_CARD_WIDTH
    };
    (size, size, -1, -1)
}

gtk::glib::wrapper! {
    pub struct SquareCoverFrame(ObjectSubclass<square_cover_frame_imp::SquareCoverFrame>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

pub(super) fn square_cover_frame(
    child: &impl IsA<gtk::Widget>,
    transport: &gtk::Box,
) -> SquareCoverFrame {
    use gtk::subclass::prelude::ObjectSubclassIsExt;

    let frame: SquareCoverFrame = gtk::glib::Object::new();
    frame.imp().transport.set(Some(transport));
    frame.set_hexpand(true);
    frame.set_halign(gtk::Align::Fill);
    frame.set_valign(gtk::Align::Start);
    frame.set_accessible_role(gtk::AccessibleRole::Presentation);
    child.set_parent(&frame);
    frame
}

pub(crate) struct CoverHoverControls {
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
        let menu = icon_button_without_tooltip(MORE_ICON, "More actions");
        configure_action_button(&menu, ActionButtonVariant::CoverCornerMenu, None);
        menu.set_halign(gtk::Align::Start);
        menu.set_valign(gtk::Align::End);
        menu.set_margin_start(COVER_CORNER_HORIZONTAL_INSET);
        menu.set_margin_bottom(COVER_CORNER_VERTICAL_INSET);
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
    favorite.set_margin_top(COVER_CORNER_VERTICAL_INSET);
    favorite.set_margin_end(COVER_CORNER_HORIZONTAL_INSET);
    favorite.set_visible(false);
    set_favorite_button_active(&favorite, favorite_active);
    controls.favorite = Some(favorite.clone());
    (controls, favorite)
}

pub(super) fn cover_play_hover_controls(size: i32, play_label: &str) -> CoverHoverControls {
    let shade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shade.add_css_class("cover-hover-layer");
    if size > 0 {
        constrain_cover_widget(&shade, size);
    } else {
        shade.set_hexpand(true);
        shade.set_vexpand(true);
        shade.set_halign(gtk::Align::Fill);
        shade.set_valign(gtk::Align::Fill);
    }
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

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, COVER_TRANSPORT_REGULAR_GAP);
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

pub(super) fn elastic_cover_context_point(widget: &impl IsA<gtk::Widget>) -> Option<(f64, f64)> {
    Some((20.0, f64::from(widget.height().saturating_sub(20))))
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
