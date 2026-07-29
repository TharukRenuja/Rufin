use super::collection_context::{
    install_dynamic_album_context_menu, install_dynamic_track_context_menu,
};
use super::collection_context::{
    present_album_context_menu, present_artist_context_menu, present_genre_context_menu,
    present_playlist_context_menu, present_smart_playlist_context_menu, present_track_context_menu,
};
use crate::LibraryField;
use crate::favorites::{
    album_favorite_key, artist_favorite_key, favorite_button_is_active, set_favorite_button_active,
    track_favorite_key,
};
use crate::interactions::install_context_menu_openers;
use crate::shell::Shell;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::cover::{ArtworkTile, GRID_COVER_SIZE, THUMB_COVER_SIZE};
use crate::shell::route::{MountedRouteItemNavigation, item_navigation_entry_position};
use ::library::{
    AlbumSummary, ArtistSummary, GenreSummary, PlaylistSummary, SmartPlaylistId,
    SmartPlaylistSummary, Track,
};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use localization::msgid;
use localization::tr;
use playback::QueuePlacement;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use super::cards;
use super::collections::{
    PlaybackTarget, SMART_PLAYLIST_REORDER_WIDTH, collection_grid_card,
    collection_grid_field_label, track_grid_field_route,
};
use super::detail_links::album_artist_route;
use super::library_fields::{
    COLLECTION_GRID_CARD_MARGIN, COLLECTION_GRID_MIN_CARD_WIDTH, album_field, artist_field,
    grid_title_with_label, item_at, item_at_from_item, playlist_field, smart_playlist_display_name,
    smart_playlist_field, track_field,
};
use super::route::Route;
use super::route_layout::{HOME_ALBUM_GAP, HOME_ALBUM_MIN_SIZE};
use super::route_shell::restore_single_click_activation_on_primary_press;

pub(super) trait ReusableCollectionGridCell<T>: 'static {
    fn widget(&self) -> gtk::Widget;
    fn bind(&self, position: u32, value: T);
    fn clear(&self);
    fn apply_fields(&self, fields: &[LibraryField]);
}

pub(super) const COLLECTION_GRID_MAX_COLUMNS: u32 = 12;

#[derive(Clone)]
pub(crate) struct CollectionGridProjection {
    surface: gtk::Widget,
    navigation: MountedRouteItemNavigation,
    fields: Rc<RefCell<Vec<LibraryField>>>,
    apply_fields: Rc<dyn Fn(&[LibraryField])>,
    cache_bound: CollectionGridCacheBound,
}

#[derive(Clone)]
struct CollectionGridCacheBound {
    grid: glib::WeakRef<gtk::GridView>,
    maximum_columns: u32,
    minimum_card_width: i32,
}

impl CollectionGridCacheBound {
    fn fit_allocation(&self, allocation_width: i32) {
        let Some(grid) = self.grid.upgrade() else {
            return;
        };
        let maximum_columns = collection_grid_column_limit(
            allocation_width,
            grid.margin_start(),
            grid.margin_end(),
            self.minimum_card_width,
            self.maximum_columns,
        );
        if grid.max_columns() == maximum_columns {
            return;
        }
        grid.set_max_columns(maximum_columns);
    }
}

fn collection_grid_column_limit(
    allocation_width: i32,
    margin_start: i32,
    margin_end: i32,
    minimum_card_width: i32,
    maximum_columns: u32,
) -> u32 {
    let available_width = allocation_width
        .saturating_sub(margin_start)
        .saturating_sub(margin_end)
        .max(1);
    let minimum_slot_width = minimum_card_width
        .max(1)
        .saturating_add(COLLECTION_GRID_CARD_MARGIN.saturating_mul(2));
    (available_width / minimum_slot_width)
        .max(1)
        .min(maximum_columns.max(1) as i32) as u32
}

#[derive(Clone)]
pub(crate) struct FixedPageCollectionRow {
    row: gtk::FlowBox,
    page_size: Rc<Cell<usize>>,
}

impl FixedPageCollectionRow {
    pub(crate) fn widget(&self) -> gtk::FlowBox {
        self.row.clone()
    }

    pub(crate) fn set_page_size(&self, page_size: usize) {
        let page_size = page_size.max(1);
        if self.page_size.replace(page_size) == page_size {
            return;
        }
        self.row
            .set_max_children_per_line(page_size.min(u32::MAX as usize) as u32);
        self.row
            .set_min_children_per_line(page_size.min(u32::MAX as usize) as u32);
    }
}

#[derive(Clone)]
enum FixedPageSlot<T> {
    Item { position: u32, value: T },
    Empty,
}

fn refill_fixed_page_slots<T: Clone + 'static>(
    source: &gio::ListStore,
    presentation: &gio::ListStore,
    page_size: usize,
) {
    let additions = (0..page_size.max(1))
        .map(|index| {
            let slot = item_at::<T>(source, index as u32).map_or(FixedPageSlot::Empty, |value| {
                FixedPageSlot::Item {
                    position: index as u32,
                    value,
                }
            });
            glib::BoxedAnyObject::new(slot)
        })
        .collect::<Vec<_>>();
    presentation.splice(0, presentation.n_items(), &additions);
}

impl CollectionGridProjection {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.surface.clone()
    }

    pub(crate) fn apply_fields(&self, fields: &[LibraryField]) {
        if self.fields.borrow().as_slice() == fields {
            return;
        }
        *self.fields.borrow_mut() = fields.to_vec();
        (self.apply_fields)(fields);
    }

    pub(crate) fn fit_allocation(&self, width: i32) {
        self.cache_bound.fit_allocation(width);
    }

    pub(crate) fn navigate(&self, direction: gtk::DirectionType) -> glib::Propagation {
        (self.navigation)(direction)
    }
}

pub(super) fn collection_grid<T, Cell, Make, Activate, M>(
    model: M,
    fields: &[LibraryField],
    make_cell: Make,
    activate: Activate,
) -> CollectionGridProjection
where
    T: Clone + 'static,
    Cell: ReusableCollectionGridCell<T>,
    Make: Fn(&[LibraryField]) -> Cell + 'static,
    Activate: Fn(u32, T) + 'static,
    M: IsA<gio::ListModel> + Clone + 'static,
{
    collection_grid_with_column_bounds(
        model,
        1,
        COLLECTION_GRID_MAX_COLUMNS,
        COLLECTION_GRID_MIN_CARD_WIDTH,
        fields,
        make_cell,
        activate,
    )
}

pub(super) fn collection_grid_with_minimum_card_width<T, Cell, Make, Activate, M>(
    model: M,
    minimum_card_width: i32,
    fields: &[LibraryField],
    make_cell: Make,
    activate: Activate,
) -> CollectionGridProjection
where
    T: Clone + 'static,
    Cell: ReusableCollectionGridCell<T>,
    Make: Fn(&[LibraryField]) -> Cell + 'static,
    Activate: Fn(u32, T) + 'static,
    M: IsA<gio::ListModel> + Clone + 'static,
{
    collection_grid_with_column_bounds(
        model,
        1,
        COLLECTION_GRID_MAX_COLUMNS,
        minimum_card_width,
        fields,
        make_cell,
        activate,
    )
}

pub(super) fn fixed_page_collection_row<T, Make, Activate>(
    model: gio::ListStore,
    columns: usize,
    make_widget: Make,
    activate: Activate,
) -> FixedPageCollectionRow
where
    T: Clone + 'static,
    Make: Fn(u32, T) -> gtk::Widget + 'static,
    Activate: Fn(u32, T) + 'static,
{
    let columns = columns.max(1);
    let maximum_columns = columns.min(u32::MAX as usize) as u32;
    let row = gtk::FlowBox::new();
    row.add_css_class("album-grid");
    row.set_homogeneous(true);
    row.set_column_spacing(HOME_ALBUM_GAP as u32);
    row.set_min_children_per_line(maximum_columns);
    row.set_max_children_per_line(maximum_columns);
    row.set_selection_mode(gtk::SelectionMode::None);
    row.set_activate_on_single_click(true);
    row.set_hexpand(true);
    row.set_vexpand(false);
    row.set_halign(gtk::Align::Fill);
    row.set_valign(gtk::Align::Start);

    let page_size = Rc::new(std::cell::Cell::new(columns));
    let presentation = gio::ListStore::new::<glib::BoxedAnyObject>();
    row.bind_model(Some(&presentation), move |item| {
        let item = item
            .downcast_ref::<glib::BoxedAnyObject>()
            .expect("fixed Home row item type");
        match item.borrow::<FixedPageSlot<T>>().clone() {
            FixedPageSlot::Item { position, value } => {
                let widget = make_widget(position, value);
                widget.set_width_request(HOME_ALBUM_MIN_SIZE);
                widget
            }
            FixedPageSlot::Empty => {
                let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
                spacer.set_width_request(HOME_ALBUM_MIN_SIZE);
                spacer.set_can_target(false);
                spacer.set_focusable(false);
                spacer.set_sensitive(false);
                spacer.set_accessible_role(gtk::AccessibleRole::Presentation);
                spacer.upcast()
            }
        }
    });
    let activate_presentation = presentation.clone();
    row.connect_child_activated(move |_, child| {
        let position = child.index();
        if position < 0 {
            return;
        }
        let Some(item) = activate_presentation.item(position as u32) else {
            return;
        };
        let item = item
            .downcast_ref::<glib::BoxedAnyObject>()
            .expect("fixed Home activation item type");
        if let FixedPageSlot::Item { position, value } = item.borrow::<FixedPageSlot<T>>().clone() {
            activate(position, value);
        }
    });

    let refill = {
        let model = model.clone();
        let presentation = presentation.clone();
        let page_size = Rc::clone(&page_size);
        Rc::new(move || {
            refill_fixed_page_slots::<T>(&model, &presentation, page_size.get());
        }) as Rc<dyn Fn()>
    };
    let change_presentation = presentation.clone();
    let change_page_size = Rc::clone(&page_size);
    model.connect_items_changed(move |source, _, _, _| {
        refill_fixed_page_slots::<T>(source, &change_presentation, change_page_size.get());
    });
    refill();

    FixedPageCollectionRow { row, page_size }
}

fn collection_grid_with_column_bounds<T, Cell, Make, Activate, M>(
    model: M,
    min_columns: u32,
    max_columns: u32,
    minimum_card_width: i32,
    fields: &[LibraryField],
    make_cell: Make,
    activate: Activate,
) -> CollectionGridProjection
where
    T: Clone + 'static,
    Cell: ReusableCollectionGridCell<T>,
    Make: Fn(&[LibraryField]) -> Cell + 'static,
    Activate: Fn(u32, T) + 'static,
    M: IsA<gio::ListModel> + Clone + 'static,
{
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);
    let factory = gtk::SignalListItemFactory::new();
    let cells = Rc::new(RefCell::new(HashMap::<usize, Cell>::new()));
    let fields = Rc::new(RefCell::new(fields.to_vec()));
    let setup_cells = Rc::clone(&cells);
    let setup_fields = Rc::clone(&fields);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let cell = make_cell(&setup_fields.borrow());
        let child = cards::collection_grid_card_inset(&cell.widget(), minimum_card_width);
        item.set_child(Some(&child));
        let mut cells = setup_cells.borrow_mut();
        cells.insert(item.as_ptr() as usize, cell);
    });
    let bind_cells = Rc::clone(&cells);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(value) = item_at_from_item::<T>(item) else {
            return;
        };
        if let Some(cell) = bind_cells.borrow().get(&(item.as_ptr() as usize)) {
            cell.bind(item.position(), value);
        }
    });
    let unbind_cells = Rc::clone(&cells);
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(cell) = unbind_cells.borrow().get(&(item.as_ptr() as usize)) {
            cell.clear();
        }
    });
    let teardown_cells = Rc::clone(&cells);
    factory.connect_teardown(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        teardown_cells
            .borrow_mut()
            .remove(&(item.as_ptr() as usize));
        item.set_child(None::<&gtk::Widget>);
    });

    let grid = gtk::GridView::new(Some(selection.clone()), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(min_columns.max(1));
    // GtkGridView keeps up to 30 rows times max-columns alive. Start with the
    // smallest cache bound; the allocation owner raises this to the number of
    // minimum-width cards that can actually fit before the first allocation.
    grid.set_max_columns(min_columns.max(1));
    grid.set_single_click_activate(true);
    restore_single_click_activation_on_primary_press(&grid, |grid| {
        grid.set_single_click_activate(true);
    });
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    grid.connect_activate(move |_, position| {
        if let Some(value) = item_at::<T>(&model, position) {
            activate(position, value);
        }
    });
    let navigation_grid = grid.clone();
    let navigation_selection = selection;
    let navigation = Rc::new(move |direction| {
        navigation_grid.set_single_click_activate(false);
        if navigation_grid
            .state_flags()
            .contains(gtk::StateFlags::FOCUS_WITHIN)
        {
            return glib::Propagation::Proceed;
        }
        let Some(model) = navigation_grid.model() else {
            return glib::Propagation::Stop;
        };
        if let Some(position) = item_navigation_entry_position(
            navigation_selection.selected(),
            model.n_items(),
            direction,
        ) {
            navigation_selection.set_selected(position);
            navigation_grid.scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
            navigation_grid.grab_focus();
        }
        glib::Propagation::Stop
    }) as MountedRouteItemNavigation;
    let grid_weak = grid.downgrade();
    let cache_bound = CollectionGridCacheBound {
        grid: grid_weak.clone(),
        maximum_columns: max_columns,
        minimum_card_width,
    };
    let apply_cells = Rc::clone(&cells);
    CollectionGridProjection {
        surface: grid.upcast(),
        navigation,
        fields,
        apply_fields: Rc::new(move |fields| {
            for cell in apply_cells.borrow().values() {
                cell.apply_fields(fields);
            }
        }),
        cache_bound,
    }
}

fn play_loaded_tracks(
    shell: &Shell,
    target: PlaybackTarget,
    placement: QueuePlacement,
    shuffled_start: bool,
) {
    let Some(request) = target.play_request(shell, placement, shuffled_start) else {
        return;
    };
    shell.products.playback.queue.play_loaded(request);
}

fn play_one_track(shell: &Shell, track: Track, placement: QueuePlacement) {
    let Some(selected) = shell.library.selected.borrow().as_ref().cloned() else {
        return;
    };
    shell
        .products
        .playback
        .queue
        .play_loaded(selected.one_track(track, placement));
}

pub(super) struct TrackGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_tile: ArtworkTile,
    favorite: gtk::Button,
    current_track: Rc<RefCell<Option<Track>>>,
    current_position: Rc<Cell<u32>>,
    cover_size: i32,
}

impl TrackGridCell {
    pub(super) fn new(
        shell: Rc<Shell>,
        fields: &[LibraryField],
        play_from_collection: Rc<dyn Fn(u32)>,
        cover_size: i32,
    ) -> Self {
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let current_position = Rc::new(Cell::new(0));

        let overlay = cards::elastic_cover_overlay();
        let (cover_button, cover_tile) = collection_grid_cover_button();
        let button_position = Rc::clone(&current_position);
        let button_play = Rc::clone(&play_from_collection);
        cover_button.connect_clicked(move |_| {
            button_play(button_position.get());
        });
        overlay.set_child(Some(&cover_button));

        let (mut controls, favorite) =
            cards::cover_hover_controls_with_favorite(0, msgid("Play track"), false);
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_track = Rc::clone(&current_track);
        let menu_target = overlay.downgrade();
        menu.connect_clicked(move |_| {
            let Some(track) = menu_track.borrow().as_ref().cloned() else {
                return;
            };
            let Some(menu_target) = menu_target.upgrade() else {
                return;
            };
            present_track_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                track,
                cards::elastic_cover_context_point(&menu_target),
            );
        });

        let play_position = Rc::clone(&current_position);
        let play_action = Rc::clone(&play_from_collection);
        controls.play.connect_clicked(move |_| {
            play_action(play_position.get());
        });

        let next_shell = Rc::clone(&shell);
        let next_track = Rc::clone(&current_track);
        controls.play_next.connect_clicked(move |_| {
            if let Some(track) = next_track.borrow().as_ref().cloned() {
                play_one_track(&next_shell, track, QueuePlacement::Next);
            }
        });

        let last_shell = Rc::clone(&shell);
        let last_track = Rc::clone(&current_track);
        controls.play_last.connect_clicked(move |_| {
            if let Some(track) = last_track.borrow().as_ref().cloned() {
                play_one_track(&last_shell, track, QueuePlacement::Last);
            }
        });

        let favorite_key_track = Rc::clone(&current_track);
        shell.favorites.register_dynamic_button(
            Rc::new(move || {
                favorite_key_track
                    .borrow()
                    .as_ref()
                    .map(|track| track_favorite_key(&track.id))
            }),
            &favorite,
        );
        let favorite_shell = Rc::clone(&shell);
        let favorite_track = Rc::clone(&current_track);
        favorite.connect_clicked(move |button| {
            let Some(track) = favorite_track.borrow().as_ref().cloned() else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Track(track.id.clone()),
                favorite,
                Some(button),
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let cover = cards::square_cover_frame(&overlay);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_track = Rc::clone(&current_track);
        body.set_download_badge(shell.download_badge(false, move |selected| {
            downloaded_track
                .borrow()
                .as_ref()
                .is_some_and(|track| selected.loaded.is_downloaded(&track.id).unwrap_or(false))
        }));
        install_dynamic_track_context_menu(&body.card, &shell, Rc::clone(&current_track));

        Self {
            body,
            shell,
            cover_tile,
            favorite,
            current_track,
            current_position,
            cover_size,
        }
    }
}

impl ReusableCollectionGridCell<Track> for TrackGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, position: u32, track: Track) {
        let artwork = ArtworkBinding::track(&track);
        self.shell.bind_artwork_tile(
            &self.cover_tile,
            artwork,
            stable_seed(track.id.as_str()),
            self.cover_size,
            GRID_COVER_SIZE,
        );
        self.body.bind(&track.title, |field| {
            (
                track_field(&track, field),
                track_grid_field_route(&track, field),
            )
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .library
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selected| selected.loaded.is_downloaded(&track.id).unwrap_or(false)),
        );
        set_favorite_button_active(&self.favorite, track.favorite);
        *self.current_track.borrow_mut() = Some(track);
        self.current_position.set(position);
    }

    fn clear(&self) {
        self.shell.clear_artwork_tile(&self.cover_tile);
        self.body.clear();
        *self.current_track.borrow_mut() = None;
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        if let Some(track) = self.current_track.borrow().as_ref().cloned() {
            self.body.bind(&track.title, |field| {
                (
                    track_field(&track, field),
                    track_grid_field_route(&track, field),
                )
            });
        }
    }
}

pub(super) struct AlbumGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_tile: ArtworkTile,
    favorite: gtk::Button,
    current_album: Rc<RefCell<Option<AlbumSummary>>>,
    cover_size: i32,
}

impl AlbumGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField], cover_size: i32) -> Self {
        let current_album = Rc::new(RefCell::new(None::<AlbumSummary>));

        let overlay = cards::elastic_cover_overlay();
        let (album_button, cover_tile) = collection_grid_cover_button();
        let open_shell = Rc::clone(&shell);
        let open_album = Rc::clone(&current_album);
        album_button.connect_clicked(move |_| {
            let Some(album) = open_album.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::AlbumDetail(album.album.id.clone()));
        });
        overlay.set_child(Some(&album_button));

        let (mut controls, favorite) =
            cards::cover_hover_controls_with_favorite(0, msgid("Play album"), false);
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_album = Rc::clone(&current_album);
        let menu_target = overlay.downgrade();
        menu.connect_clicked(move |_| {
            let Some(album) = menu_album.borrow().as_ref().cloned() else {
                return;
            };
            let Some(menu_target) = menu_target.upgrade() else {
                return;
            };
            present_album_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                album,
                cards::elastic_cover_context_point(&menu_target),
            );
        });

        let play_shell = Rc::clone(&shell);
        let play_album = Rc::clone(&current_album);
        controls.play.connect_clicked(move |_| {
            let Some(album) = play_album.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &play_shell,
                PlaybackTarget::Album(album.album.id.clone()),
                QueuePlacement::Now,
                true,
            );
        });

        let next_shell = Rc::clone(&shell);
        let next_album = Rc::clone(&current_album);
        controls.play_next.connect_clicked(move |_| {
            let Some(album) = next_album.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &next_shell,
                PlaybackTarget::Album(album.album.id.clone()),
                QueuePlacement::Next,
                false,
            );
        });

        let last_shell = Rc::clone(&shell);
        let last_album = Rc::clone(&current_album);
        controls.play_last.connect_clicked(move |_| {
            let Some(album) = last_album.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &last_shell,
                PlaybackTarget::Album(album.album.id.clone()),
                QueuePlacement::Last,
                false,
            );
        });

        let favorite_key_album = Rc::clone(&current_album);
        shell.favorites.register_dynamic_button(
            Rc::new(move || {
                favorite_key_album
                    .borrow()
                    .as_ref()
                    .map(|album| album_favorite_key(&album.album.id))
            }),
            &favorite,
        );
        let favorite_shell = Rc::clone(&shell);
        let favorite_album = Rc::clone(&current_album);
        favorite.connect_clicked(move |button| {
            let Some(album) = favorite_album.borrow().as_ref().cloned() else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Album(album.album.id.clone()),
                favorite,
                Some(button),
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let cover = cards::square_cover_frame(&overlay);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_album = Rc::clone(&current_album);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_album.borrow().as_ref().is_some_and(|album| {
                selected
                    .loaded
                    .is_album_downloaded(&album.album.id, selected.music_folder_id.as_ref())
                    .unwrap_or(false)
            })
        }));
        install_dynamic_album_context_menu(&body.card, &shell, Rc::clone(&current_album));

        Self {
            body,
            shell,
            cover_tile,
            favorite,
            current_album,
            cover_size,
        }
    }
}

impl ReusableCollectionGridCell<AlbumSummary> for AlbumGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, album: AlbumSummary) {
        self.shell.bind_artwork_tile(
            &self.cover_tile,
            ArtworkBinding::album_artwork(&album.artwork),
            album.album.color_seed,
            self.cover_size,
            GRID_COVER_SIZE,
        );
        self.body.bind(&album.album.title, |field| {
            let value = album_field(&album, field);
            let route = if value.is_empty()
                || !matches!(field, LibraryField::Artist | LibraryField::AlbumArtist)
            {
                None
            } else {
                album_artist_route(&album.album)
            };
            (value, route)
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .library
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selected| {
                    selected
                        .loaded
                        .is_album_downloaded(&album.album.id, selected.music_folder_id.as_ref())
                        .unwrap_or(false)
                }),
        );
        set_favorite_button_active(&self.favorite, album.album.favorite);
        *self.current_album.borrow_mut() = Some(album);
    }

    fn clear(&self) {
        self.shell.clear_artwork_tile(&self.cover_tile);
        self.body.clear();
        *self.current_album.borrow_mut() = None;
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        if let Some(album) = self.current_album.borrow().as_ref().cloned() {
            self.body.bind(&album.album.title, |field| {
                let value = album_field(&album, field);
                let route = if value.is_empty()
                    || !matches!(field, LibraryField::Artist | LibraryField::AlbumArtist)
                {
                    None
                } else {
                    album_artist_route(&album.album)
                };
                (value, route)
            });
        }
    }
}

pub(super) struct ArtistGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_tile: ArtworkTile,
    favorite: gtk::Button,
    current_artist: Rc<RefCell<Option<ArtistSummary>>>,
}

impl ArtistGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField]) -> Self {
        let current_artist = Rc::new(RefCell::new(None::<ArtistSummary>));

        let overlay = cards::elastic_cover_overlay();
        let (artist_button, cover_tile) = collection_grid_cover_button();
        let open_shell = Rc::clone(&shell);
        let open_artist = Rc::clone(&current_artist);
        artist_button.connect_clicked(move |_| {
            let Some(artist) = open_artist.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::ArtistDetail(artist.artist.id.clone()));
        });
        overlay.set_child(Some(&artist_button));

        let (mut controls, favorite) =
            cards::cover_hover_controls_with_favorite(0, msgid("Play artist"), false);
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_artist = Rc::clone(&current_artist);
        let menu_target = overlay.downgrade();
        menu.connect_clicked(move |_| {
            let Some(artist) = menu_artist.borrow().as_ref().cloned() else {
                return;
            };
            let Some(menu_target) = menu_target.upgrade() else {
                return;
            };
            present_artist_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                artist,
                cards::elastic_cover_context_point(&menu_target),
            );
        });

        let play_shell = Rc::clone(&shell);
        let play_artist = Rc::clone(&current_artist);
        controls.play.connect_clicked(move |_| {
            let Some(artist) = play_artist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &play_shell,
                PlaybackTarget::Artist(artist.artist.id.clone()),
                QueuePlacement::Now,
                true,
            );
        });

        let next_shell = Rc::clone(&shell);
        let next_artist = Rc::clone(&current_artist);
        controls.play_next.connect_clicked(move |_| {
            let Some(artist) = next_artist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &next_shell,
                PlaybackTarget::Artist(artist.artist.id.clone()),
                QueuePlacement::Next,
                false,
            );
        });

        let last_shell = Rc::clone(&shell);
        let last_artist = Rc::clone(&current_artist);
        controls.play_last.connect_clicked(move |_| {
            let Some(artist) = last_artist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &last_shell,
                PlaybackTarget::Artist(artist.artist.id.clone()),
                QueuePlacement::Last,
                false,
            );
        });

        let favorite_key_artist = Rc::clone(&current_artist);
        shell.favorites.register_dynamic_button(
            Rc::new(move || {
                favorite_key_artist
                    .borrow()
                    .as_ref()
                    .map(|artist| artist_favorite_key(&artist.artist.id))
            }),
            &favorite,
        );
        let favorite_shell = Rc::clone(&shell);
        let favorite_artist = Rc::clone(&current_artist);
        favorite.connect_clicked(move |button| {
            let Some(artist) = favorite_artist.borrow().as_ref().cloned() else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Artist(artist.artist.id.clone()),
                favorite,
                Some(button),
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let cover = cards::square_cover_frame(&overlay);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_artist = Rc::clone(&current_artist);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_artist.borrow().as_ref().is_some_and(|artist| {
                selected
                    .loaded
                    .is_artist_downloaded(&artist.artist.id, selected.music_folder_id.as_ref())
                    .unwrap_or(false)
            })
        }));
        install_dynamic_artist_context_menu(&body.card, &shell, Rc::clone(&current_artist));

        Self {
            body,
            shell,
            cover_tile,
            favorite,
            current_artist,
        }
    }
}

impl ReusableCollectionGridCell<ArtistSummary> for ArtistGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, artist: ArtistSummary) {
        self.shell.bind_artwork_tile(
            &self.cover_tile,
            ArtworkBinding::artist(&artist.artist, &artist.representative_albums),
            stable_seed(artist.artist.id.as_str()),
            COLLECTION_GRID_MIN_CARD_WIDTH,
            GRID_COVER_SIZE,
        );
        self.body.bind(&artist.artist.name, |field| {
            (artist_field(&artist, field), None)
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .library
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selected| {
                    selected
                        .loaded
                        .is_artist_downloaded(&artist.artist.id, selected.music_folder_id.as_ref())
                        .unwrap_or(false)
                }),
        );
        set_favorite_button_active(&self.favorite, artist.artist.favorite);
        *self.current_artist.borrow_mut() = Some(artist);
    }

    fn clear(&self) {
        self.shell.clear_artwork_tile(&self.cover_tile);
        self.body.clear();
        *self.current_artist.borrow_mut() = None;
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        if let Some(artist) = self.current_artist.borrow().as_ref().cloned() {
            self.body.bind(&artist.artist.name, |field| {
                (artist_field(&artist, field), None)
            });
        }
    }
}

pub(super) struct PlaylistGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    current_playlist: Rc<RefCell<Option<PlaylistSummary>>>,
}

impl PlaylistGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField]) -> Self {
        let current_playlist = Rc::new(RefCell::new(None::<PlaylistSummary>));

        let overlay = cards::elastic_cover_overlay();
        let cover_button = collection_grid_cover_shell();
        let open_shell = Rc::clone(&shell);
        let open_playlist = Rc::clone(&current_playlist);
        cover_button.connect_clicked(move |_| {
            let Some(playlist) = open_playlist.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::PlaylistDetail(playlist.playlist.id.clone()));
        });
        overlay.set_child(Some(&cover_button));

        let mut controls = cards::cover_play_hover_controls(0, msgid("Play playlist"));
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_playlist = Rc::clone(&current_playlist);
        let menu_target = overlay.downgrade();
        menu.connect_clicked(move |_| {
            let Some(playlist) = menu_playlist.borrow().as_ref().cloned() else {
                return;
            };
            let Some(menu_target) = menu_target.upgrade() else {
                return;
            };
            present_playlist_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                playlist,
                cards::elastic_cover_context_point(&menu_target),
            );
        });

        let play_shell = Rc::clone(&shell);
        let play_playlist = Rc::clone(&current_playlist);
        controls.play.connect_clicked(move |_| {
            let Some(playlist) = play_playlist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &play_shell,
                PlaybackTarget::Playlist(playlist.playlist.id.clone()),
                QueuePlacement::Now,
                true,
            );
        });

        let next_shell = Rc::clone(&shell);
        let next_playlist = Rc::clone(&current_playlist);
        controls.play_next.connect_clicked(move |_| {
            let Some(playlist) = next_playlist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &next_shell,
                PlaybackTarget::Playlist(playlist.playlist.id.clone()),
                QueuePlacement::Next,
                false,
            );
        });

        let last_shell = Rc::clone(&shell);
        let last_playlist = Rc::clone(&current_playlist);
        controls.play_last.connect_clicked(move |_| {
            let Some(playlist) = last_playlist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &last_shell,
                PlaybackTarget::Playlist(playlist.playlist.id.clone()),
                QueuePlacement::Last,
                false,
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let cover = cards::square_cover_frame(&overlay);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_playlist = Rc::clone(&current_playlist);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_playlist
                .borrow()
                .as_ref()
                .is_some_and(|playlist| {
                    selected
                        .loaded
                        .is_playlist_downloaded(&playlist.playlist.id)
                        .unwrap_or(false)
                })
        }));
        install_dynamic_playlist_context_menu(&body.card, &shell, Rc::clone(&current_playlist));

        Self {
            body,
            shell,
            cover_button,
            current_playlist,
        }
    }
}

impl ReusableCollectionGridCell<PlaylistSummary> for PlaylistGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, playlist: PlaylistSummary) {
        let artwork = ArtworkBinding::playlist_slots(
            &playlist.playlist,
            &playlist.representative_albums,
            self.shell
                .settings
                .current
                .borrow()
                .prefer_server_playlist_covers,
        );
        self.cover_button
            .set_child(Some(&self.shell.elastic_cover_group_tile_for_artwork(
                &artwork,
                stable_seed(playlist.playlist.id.as_str()),
                THUMB_COVER_SIZE,
            )));
        self.body.bind(&playlist.playlist.name, |field| {
            (playlist_field(&playlist, field), None)
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .library
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selected| {
                    selected
                        .loaded
                        .is_playlist_downloaded(&playlist.playlist.id)
                        .unwrap_or(false)
                }),
        );
        *self.current_playlist.borrow_mut() = Some(playlist);
    }

    fn clear(&self) {
        self.cover_button.set_child(None::<&gtk::Widget>);
        self.body.clear();
        *self.current_playlist.borrow_mut() = None;
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        if let Some(playlist) = self.current_playlist.borrow().as_ref().cloned() {
            self.body.bind(&playlist.playlist.name, |field| {
                (playlist_field(&playlist, field), None)
            });
        }
    }
}

pub(super) struct SmartPlaylistGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    widget: gtk::Overlay,
    current_playlist: Rc<RefCell<Option<SmartPlaylistSummary>>>,
}

impl SmartPlaylistGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField]) -> Self {
        let current_playlist = Rc::new(RefCell::new(None::<SmartPlaylistSummary>));

        let overlay = cards::elastic_cover_overlay();
        let cover_button = collection_grid_cover_shell();
        let open_shell = Rc::clone(&shell);
        let open_playlist = Rc::clone(&current_playlist);
        cover_button.connect_clicked(move |_| {
            let Some(playlist) = open_playlist.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::SmartPlaylistDetail(
                playlist.smart_playlist.id.clone(),
            ));
        });
        overlay.set_child(Some(&cover_button));

        let mut controls = cards::cover_play_hover_controls(0, msgid("Play smart playlist"));
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_playlist = Rc::clone(&current_playlist);
        let menu_target = overlay.downgrade();
        menu.connect_clicked(move |_| {
            let Some(playlist) = menu_playlist.borrow().as_ref().cloned() else {
                return;
            };
            let Some(menu_target) = menu_target.upgrade() else {
                return;
            };
            present_smart_playlist_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                playlist,
                cards::elastic_cover_context_point(&menu_target),
            );
        });

        let play_shell = Rc::clone(&shell);
        let play_playlist = Rc::clone(&current_playlist);
        controls.play.connect_clicked(move |_| {
            let Some(playlist) = play_playlist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &play_shell,
                PlaybackTarget::SmartPlaylist(playlist.smart_playlist.id.clone()),
                QueuePlacement::Now,
                true,
            );
        });

        let next_shell = Rc::clone(&shell);
        let next_playlist = Rc::clone(&current_playlist);
        controls.play_next.connect_clicked(move |_| {
            let Some(playlist) = next_playlist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &next_shell,
                PlaybackTarget::SmartPlaylist(playlist.smart_playlist.id.clone()),
                QueuePlacement::Next,
                false,
            );
        });

        let last_shell = Rc::clone(&shell);
        let last_playlist = Rc::clone(&current_playlist);
        controls.play_last.connect_clicked(move |_| {
            let Some(playlist) = last_playlist.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &last_shell,
                PlaybackTarget::SmartPlaylist(playlist.smart_playlist.id.clone()),
                QueuePlacement::Last,
                false,
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let cover = cards::square_cover_frame(&overlay);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_playlist = Rc::clone(&current_playlist);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_playlist
                .borrow()
                .as_ref()
                .is_some_and(|playlist| {
                    selected
                        .loaded
                        .is_smart_playlist_downloaded(
                            &playlist.smart_playlist.id,
                            selected.music_folder_id.as_ref(),
                        )
                        .unwrap_or(false)
                })
        }));
        let widget = smart_playlist_grid_overlay(&body.card, Rc::clone(&current_playlist));
        install_dynamic_smart_playlist_drop_target(&widget, &shell, Rc::clone(&current_playlist));
        install_dynamic_smart_playlist_context_menu(&widget, &shell, Rc::clone(&current_playlist));

        Self {
            body,
            shell,
            cover_button,
            widget,
            current_playlist,
        }
    }
}

impl ReusableCollectionGridCell<SmartPlaylistSummary> for SmartPlaylistGridCell {
    fn widget(&self) -> gtk::Widget {
        self.widget.clone().upcast()
    }

    fn bind(&self, _: u32, playlist: SmartPlaylistSummary) {
        let artwork = ArtworkBinding::smart_playlist_slots(
            &playlist.smart_playlist,
            &playlist.representative_albums,
        );
        self.cover_button
            .set_child(Some(&self.shell.elastic_cover_group_tile_for_artwork(
                &artwork,
                stable_seed(playlist.smart_playlist.id.as_str()),
                THUMB_COVER_SIZE,
            )));
        self.body.bind(
            &smart_playlist_display_name(&playlist.smart_playlist),
            |field| (smart_playlist_field(&playlist, field), None),
        );
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .library
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selected| {
                    selected
                        .loaded
                        .is_smart_playlist_downloaded(
                            &playlist.smart_playlist.id,
                            selected.music_folder_id.as_ref(),
                        )
                        .unwrap_or(false)
                }),
        );
        *self.current_playlist.borrow_mut() = Some(playlist);
    }

    fn clear(&self) {
        self.cover_button.set_child(None::<&gtk::Widget>);
        self.body.clear();
        *self.current_playlist.borrow_mut() = None;
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        if let Some(playlist) = self.current_playlist.borrow().as_ref().cloned() {
            self.body.bind(
                &smart_playlist_display_name(&playlist.smart_playlist),
                |field| (smart_playlist_field(&playlist, field), None),
            );
        }
    }
}

fn install_dynamic_artist_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    artist: Rc<RefCell<Option<ArtistSummary>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(artist) = artist.borrow().clone() else {
                return;
            };
            present_artist_context_menu(target, &shell, artist, position);
        }),
    );
}

pub(super) fn install_dynamic_genre_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    genre: Rc<RefCell<Option<GenreSummary>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(genre) = genre.borrow().clone() else {
                return;
            };
            present_genre_context_menu(target, &shell, genre, position);
        }),
    );
}

fn install_dynamic_playlist_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    playlist: Rc<RefCell<Option<PlaylistSummary>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(playlist) = playlist.borrow().clone() else {
                return;
            };
            present_playlist_context_menu(target, &shell, playlist, position);
        }),
    );
}

fn install_dynamic_smart_playlist_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    playlist: Rc<RefCell<Option<SmartPlaylistSummary>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(playlist) = playlist.borrow().clone() else {
                return;
            };
            present_smart_playlist_context_menu(target, &shell, playlist, position);
        }),
    );
}

fn smart_playlist_grid_overlay(
    card: &gtk::Box,
    playlist: Rc<RefCell<Option<SmartPlaylistSummary>>>,
) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_halign(gtk::Align::Fill);
    overlay.set_child(Some(card));

    let drag = dynamic_smart_playlist_drag_handle(playlist);
    drag.set_margin_start(6);
    drag.set_margin_top(6);
    drag.set_halign(gtk::Align::Start);
    drag.set_valign(gtk::Align::Start);
    overlay.add_overlay(&drag);
    overlay
}

fn dynamic_smart_playlist_drag_handle(
    playlist: Rc<RefCell<Option<SmartPlaylistSummary>>>,
) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("rufin-list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(SMART_PLAYLIST_REORDER_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    source.connect_prepare(move |_, _, _| {
        let playlist_id = playlist
            .borrow()
            .as_ref()?
            .smart_playlist
            .id
            .as_str()
            .to_string();
        Some(gtk::gdk::ContentProvider::for_value(
            &playlist_id.to_value(),
        ))
    });
    drag.add_controller(source);
    drag
}

fn install_dynamic_smart_playlist_drop_target(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    playlist: Rc<RefCell<Option<SmartPlaylistSummary>>>,
) {
    let widget = target.as_ref().downgrade();
    let smart_playlists = shell.products.smart_playlists.clone();
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(dragged_id) = value.get::<String>() else {
            return false;
        };
        let Some(target_id) = playlist
            .borrow()
            .as_ref()
            .map(|playlist| playlist.smart_playlist.id.clone())
        else {
            return false;
        };
        let dragged_id = SmartPlaylistId::new(dragged_id);
        if dragged_id == target_id {
            return false;
        }
        let Some(widget) = widget.upgrade() else {
            return false;
        };
        let after = y > f64::from(widget.height()) / 2.0;
        smart_playlists.move_relative(dragged_id, target_id, after);
        true
    });
    target.add_controller(drop_target);
}

pub(super) struct CollectionGridCardCell {
    pub(super) card: gtk::Box,
    title: gtk::Label,
    title_row: gtk::Box,
    downloaded: RefCell<Option<gtk::Image>>,
    fields: RefCell<Vec<CollectionGridFieldCell>>,
}

impl CollectionGridCardCell {
    pub(super) fn new(shell: &Rc<Shell>, fields: &[LibraryField], cover: gtk::Widget) -> Self {
        let card = collection_grid_card();
        card.append(&cover);
        let (title_widget, title) = grid_title_with_label("", "track-title");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(false);
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        title_row.set_hexpand(true);
        title_row.append(&title_widget);
        card.append(&title_row);
        let field_cells = fields
            .iter()
            .copied()
            .map(|field| CollectionGridFieldCell::new(shell, field))
            .collect::<Vec<_>>();
        for field in &field_cells {
            card.append(&field.widget);
        }
        let cell = Self {
            card,
            title,
            title_row,
            downloaded: RefCell::new(None),
            fields: RefCell::new(field_cells),
        };
        cell
    }

    pub(super) fn set_download_badge(&self, badge: gtk::Image) {
        if let Some(previous) = self.downloaded.replace(Some(badge.clone())) {
            self.title_row.remove(&previous);
        }
        self.title_row.append(&badge);
    }

    pub(super) fn set_downloaded(&self, shell: &Shell, downloaded: bool) {
        if let Some(badge) = self.downloaded.borrow().as_ref() {
            shell.set_download_badge_visible(badge, downloaded);
        }
    }

    pub(super) fn widget(&self) -> gtk::Widget {
        self.card.clone().upcast()
    }

    pub(super) fn bind(
        &self,
        title: &str,
        mut field_value: impl FnMut(LibraryField) -> (String, Option<Route>),
    ) {
        self.title.set_text(title);
        self.title
            .set_tooltip_text((!title.is_empty()).then_some(title));
        for field in self.fields.borrow().iter() {
            let (value, route) = field_value(field.field);
            field.bind(value, route);
        }
    }

    pub(super) fn clear(&self) {
        self.title.set_text("");
        self.title.set_tooltip_text(None);
        if let Some(downloaded) = self.downloaded.borrow().as_ref() {
            downloaded.set_visible(false);
        }
        for field in self.fields.borrow().iter() {
            field.clear();
        }
    }

    pub(super) fn replace_fields(&self, shell: &Rc<Shell>, fields: &[LibraryField]) {
        if self
            .fields
            .borrow()
            .iter()
            .map(|field| field.field)
            .eq(fields.iter().copied())
        {
            return;
        }
        for field in self.fields.take() {
            self.card.remove(&field.widget);
        }
        let next = fields
            .iter()
            .copied()
            .map(|field| CollectionGridFieldCell::new(shell, field))
            .collect::<Vec<_>>();
        for field in &next {
            self.card.append(&field.widget);
        }
        self.fields.replace(next);
    }
}

pub(super) fn collection_grid_cover_shell() -> gtk::Button {
    let cover_button = gtk::Button::new();
    cover_button.add_css_class("album-cover-button");
    cover_button.add_css_class("flat");
    cards::clip_cover(&cover_button);
    cover_button.set_width_request(1);
    cover_button.set_height_request(1);
    cover_button.set_hexpand(true);
    cover_button.set_vexpand(true);
    cover_button.set_halign(gtk::Align::Fill);
    cover_button.set_valign(gtk::Align::Fill);
    cover_button
}

fn collection_grid_cover_button() -> (gtk::Button, ArtworkTile) {
    let cover_button = collection_grid_cover_shell();
    let cover_tile = ArtworkTile::new_elastic_square(0);
    cover_button.set_child(Some(&cover_tile.widget()));
    (cover_button, cover_tile)
}

struct CollectionGridFieldCell {
    field: LibraryField,
    widget: gtk::Widget,
    label: gtk::Label,
    route: Rc<RefCell<Option<Route>>>,
}

impl CollectionGridFieldCell {
    fn new(shell: &Rc<Shell>, field: LibraryField) -> Self {
        let (widget, label) = collection_grid_field_label("", field);
        let route = Rc::new(RefCell::new(None::<Route>));
        install_dynamic_grid_label_link(shell, &widget, &label, Rc::clone(&route));
        Self {
            field,
            widget,
            label,
            route,
        }
    }

    fn bind(&self, value: String, route: Option<Route>) {
        *self.route.borrow_mut() = route;
        self.label.set_text(&value);
        self.label
            .set_tooltip_text((!value.is_empty()).then_some(value.as_str()));
        let clickable = self.route.borrow().is_some();
        let cursor = clickable.then_some("pointer");
        self.widget.set_cursor_from_name(cursor);
        self.label.set_cursor_from_name(cursor);
    }

    fn clear(&self) {
        *self.route.borrow_mut() = None;
        self.label.set_text("");
        self.label.set_tooltip_text(None);
        self.widget.set_cursor_from_name(None);
        self.label.set_cursor_from_name(None);
    }
}

fn install_dynamic_grid_label_link(
    shell: &Rc<Shell>,
    target: &gtk::Widget,
    label: &gtk::Label,
    route: Rc<RefCell<Option<Route>>>,
) {
    let enter_label = label.downgrade();
    let enter_route = Rc::clone(&route);
    let leave_label = label.downgrade();
    let leave_route = Rc::clone(&route);
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        if enter_route.borrow().is_none() {
            return;
        }
        let Some(label) = enter_label.upgrade() else {
            return;
        };
        let text = label.text();
        if text.is_empty() {
            return;
        }
        let escaped_text = glib::markup_escape_text(text.as_str());
        label.add_css_class("hovered-link");
        label.set_markup(&format!("<u>{escaped_text}</u>"));
    });
    motion.connect_leave(move |_| {
        if leave_route.borrow().is_some() {
            let Some(label) = leave_label.upgrade() else {
                return;
            };
            let text = label.text().to_string();
            label.remove_css_class("hovered-link");
            label.set_text(&text);
        }
    });
    target.add_controller(motion);

    let click_shell = Rc::clone(shell);
    let click_route = route;
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |gesture, press_count, _, _| {
        if press_count != 1 {
            return;
        }
        let Some(route) = click_route.borrow().clone() else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        click_shell.navigate(route);
    });
    target.add_controller(click);
}

#[cfg(test)]
mod tests {
    use super::super::library_fields::ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH;
    use super::*;

    fn fixed_slot(store: &gio::ListStore, position: u32) -> FixedPageSlot<u8> {
        store
            .item(position)
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
            .expect("fixed page slot")
            .borrow::<FixedPageSlot<u8>>()
            .clone()
    }

    #[test]
    fn fixed_page_slots_keep_source_positions_and_fill_the_page() {
        let source = gio::ListStore::new::<glib::BoxedAnyObject>();
        source.append(&glib::BoxedAnyObject::new(7_u8));
        let presentation = gio::ListStore::new::<glib::BoxedAnyObject>();

        refill_fixed_page_slots::<u8>(&source, &presentation, 3);

        assert_eq!(presentation.n_items(), 3);
        assert!(matches!(
            fixed_slot(&presentation, 0),
            FixedPageSlot::Item {
                position: 0,
                value: 7
            }
        ));
        assert!(matches!(fixed_slot(&presentation, 1), FixedPageSlot::Empty));
        assert!(matches!(fixed_slot(&presentation, 2), FixedPageSlot::Empty));
    }

    #[test]
    fn collection_grid_column_limit_uses_the_card_minimum() {
        assert_eq!(
            collection_grid_column_limit(
                600,
                0,
                0,
                COLLECTION_GRID_MIN_CARD_WIDTH,
                COLLECTION_GRID_MAX_COLUMNS,
            ),
            3
        );
        assert_eq!(
            collection_grid_column_limit(
                600,
                0,
                0,
                ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH,
                COLLECTION_GRID_MAX_COLUMNS,
            ),
            2
        );
    }
}
