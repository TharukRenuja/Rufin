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
use crate::shell::cover::{ArtworkTile, LARGE_COVER_SIZE, THUMB_COVER_SIZE};
use crate::shell::route::{MountedRouteItemNavigation, item_navigation_entry_position};
use ::library::{
    AlbumId, AlbumSummary, ArtistSummary, GenreSummary, PlaylistSummary, SmartPlaylistId,
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
    collection_grid_field_label, track_grid_field_links,
};
use super::detail_links::{DetailLinkBinding, DetailLinks, album_artist_links};
use super::library_fields::{
    COLLECTION_GRID_CARD_MARGIN, COLLECTION_GRID_MAX_CARD_WIDTH, COLLECTION_GRID_MIN_CARD_WIDTH,
    album_field, artist_field, grid_title_with_label, item_at, item_at_from_item, playlist_field,
    smart_playlist_display_name, smart_playlist_field,
};
use super::route::Route;
use super::route_shell::restore_single_click_activation_on_primary_press;

pub(super) trait ReusableCollectionGridCell<T>: 'static {
    fn widget(&self) -> gtk::Widget;
    fn activatable(&self, _: &T) -> bool {
        true
    }
    fn bind(&self, position: u32, value: T);
    fn clear(&self);
    fn apply_fields(&self, fields: &[LibraryField]);
}

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
}

impl CollectionGridCacheBound {
    fn fit_allocation(&self, allocation_width: i32) {
        let Some(grid) = self.grid.upgrade() else {
            return;
        };
        let maximum_columns =
            collection_grid_column_limit(allocation_width, grid.margin_start(), grid.margin_end());
        if grid.max_columns() == maximum_columns {
            return;
        }
        grid.set_max_columns(maximum_columns);
    }
}

fn collection_grid_column_limit(allocation_width: i32, margin_start: i32, margin_end: i32) -> u32 {
    let available_width = allocation_width
        .saturating_sub(margin_start)
        .saturating_sub(margin_end)
        .max(1);
    collection_grid_column_count(available_width).min(u32::MAX as usize) as u32
}

pub(super) fn collection_grid_column_count(available_width: i32) -> usize {
    let minimum_slot_width = COLLECTION_GRID_MIN_CARD_WIDTH
        .max(1)
        .saturating_add(COLLECTION_GRID_CARD_MARGIN.saturating_mul(2));
    let maximum_slot_width = COLLECTION_GRID_MAX_CARD_WIDTH
        .max(COLLECTION_GRID_MIN_CARD_WIDTH)
        .max(1)
        .saturating_add(COLLECTION_GRID_CARD_MARGIN.saturating_mul(2));
    let available_width = available_width.max(1);
    let maximum_fitting_columns = (available_width / minimum_slot_width).max(1);
    let minimum_needed_columns =
        available_width.saturating_add(maximum_slot_width - 1) / maximum_slot_width;
    minimum_needed_columns.max(1).min(maximum_fitting_columns) as usize
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
    row.set_column_spacing(0);
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
                cards::collection_grid_card_inset(&widget, COLLECTION_GRID_MIN_CARD_WIDTH).upcast()
            }
            FixedPageSlot::Empty => {
                let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
                spacer.set_can_target(false);
                spacer.set_focusable(false);
                spacer.set_sensitive(false);
                spacer.set_accessible_role(gtk::AccessibleRole::Presentation);
                cards::collection_grid_card_inset(&spacer, COLLECTION_GRID_MIN_CARD_WIDTH).upcast()
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
        let child =
            cards::collection_grid_card_inset(&cell.widget(), COLLECTION_GRID_MIN_CARD_WIDTH);
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
            let activatable = cell.activatable(&value);
            item.set_activatable(activatable);
            item.set_selectable(activatable);
            cell.bind(item.position(), value);
        }
    });
    let unbind_cells = Rc::clone(&cells);
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        item.set_activatable(false);
        item.set_selectable(false);
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
    grid.set_min_columns(1);
    // GtkGridView keeps up to 30 rows times max-columns alive. Start with the
    // smallest cache bound; the allocation owner raises this to the number of
    // cards in the allocated width before the first allocation.
    grid.set_max_columns(1);
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
    let cache_bound = CollectionGridCacheBound {
        grid: grid.downgrade(),
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

fn album_playback_target(album_id: AlbumId, context: Option<&str>) -> PlaybackTarget {
    let target = PlaybackTarget::Album(album_id);
    context
        .map(|context| target.clone().in_context(context))
        .unwrap_or(target)
}

fn play_one_track(shell: &Shell, track: Track, placement: QueuePlacement) {
    let Some(selected) = shell.selected_library().as_deref().cloned() else {
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
    field_value: Rc<dyn Fn(u32, &Track, LibraryField) -> DetailLinks>,
}

impl TrackGridCell {
    pub(super) fn new(
        shell: Rc<Shell>,
        fields: &[LibraryField],
        play_from_collection: Rc<dyn Fn(u32)>,
    ) -> Self {
        Self::new_with_field_value(
            shell,
            fields,
            play_from_collection,
            Rc::new(|_, track, field| track_grid_field_links(track, field)),
        )
    }

    pub(super) fn new_with_field_value(
        shell: Rc<Shell>,
        fields: &[LibraryField],
        play_from_collection: Rc<dyn Fn(u32)>,
        field_value: Rc<dyn Fn(u32, &Track, LibraryField) -> DetailLinks>,
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
        shell.register_dynamic_favorite_button(
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

        let cover = cards::square_cover_frame(&overlay, &controls.transport);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_track = Rc::clone(&current_track);
        body.set_download_badge(shell.download_badge(false, move |selected| {
            downloaded_track
                .borrow()
                .as_ref()
                .is_some_and(|track| selected.library.is_downloaded(&track.id).unwrap_or(false))
        }));
        install_dynamic_track_context_menu(&body.card, &shell, Rc::clone(&current_track));

        Self {
            body,
            shell,
            cover_tile,
            favorite,
            current_track,
            current_position,
            field_value,
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
            COLLECTION_GRID_MAX_CARD_WIDTH,
            LARGE_COVER_SIZE,
        );
        self.body.bind(&track.title, |field| {
            (self.field_value)(position, &track, field)
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .selected_library()
                .as_deref()
                .is_some_and(|selected| selected.library.is_downloaded(&track.id).unwrap_or(false)),
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
            let position = self.current_position.get();
            self.body.bind(&track.title, |field| {
                (self.field_value)(position, &track, field)
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
}

impl AlbumGridCell {
    pub(super) fn new(
        shell: Rc<Shell>,
        fields: &[LibraryField],
        playback_context: Option<String>,
    ) -> Self {
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
        let menu_playback_context = playback_context.clone();
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
                menu_playback_context.clone(),
                None,
                cards::elastic_cover_context_point(&menu_target),
            );
        });

        let play_shell = Rc::clone(&shell);
        let play_album = Rc::clone(&current_album);
        let play_context = playback_context.clone();
        controls.play.connect_clicked(move |_| {
            let Some(album) = play_album.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &play_shell,
                album_playback_target(album.album.id.clone(), play_context.as_deref()),
                QueuePlacement::Now,
                true,
            );
        });

        let next_shell = Rc::clone(&shell);
        let next_album = Rc::clone(&current_album);
        let next_context = playback_context.clone();
        controls.play_next.connect_clicked(move |_| {
            let Some(album) = next_album.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &next_shell,
                album_playback_target(album.album.id.clone(), next_context.as_deref()),
                QueuePlacement::Next,
                false,
            );
        });

        let last_shell = Rc::clone(&shell);
        let last_album = Rc::clone(&current_album);
        let last_context = playback_context.clone();
        controls.play_last.connect_clicked(move |_| {
            let Some(album) = last_album.borrow().as_ref().cloned() else {
                return;
            };
            play_loaded_tracks(
                &last_shell,
                album_playback_target(album.album.id.clone(), last_context.as_deref()),
                QueuePlacement::Last,
                false,
            );
        });

        let favorite_key_album = Rc::clone(&current_album);
        shell.register_dynamic_favorite_button(
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

        let cover = cards::square_cover_frame(&overlay, &controls.transport);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_album = Rc::clone(&current_album);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_album.borrow().as_ref().is_some_and(|album| {
                selected
                    .library
                    .is_album_downloaded(&album.album.id, selected.music_folder_id.as_ref())
                    .unwrap_or(false)
            })
        }));
        install_dynamic_album_context_menu(
            &body.card,
            &shell,
            Rc::clone(&current_album),
            playback_context,
        );

        Self {
            body,
            shell,
            cover_tile,
            favorite,
            current_album,
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
            COLLECTION_GRID_MAX_CARD_WIDTH,
            LARGE_COVER_SIZE,
        );
        self.body.bind(&album.album.title, |field| {
            let value = album_field(&album, field);
            if value.is_empty()
                || !matches!(field, LibraryField::Artist | LibraryField::AlbumArtist)
            {
                DetailLinks::text(&value)
            } else {
                album_artist_links(&album.album)
            }
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .selected_library()
                .as_deref()
                .is_some_and(|selected| {
                    selected
                        .library
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
                if value.is_empty()
                    || !matches!(field, LibraryField::Artist | LibraryField::AlbumArtist)
                {
                    DetailLinks::text(&value)
                } else {
                    album_artist_links(&album.album)
                }
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
                None,
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
        shell.register_dynamic_favorite_button(
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

        let cover = cards::square_cover_frame(&overlay, &controls.transport);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_artist = Rc::clone(&current_artist);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_artist.borrow().as_ref().is_some_and(|artist| {
                selected
                    .library
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
            ArtworkBinding::artist(&artist.artwork),
            stable_seed(artist.artist.id.as_str()),
            COLLECTION_GRID_MAX_CARD_WIDTH,
            LARGE_COVER_SIZE,
        );
        self.body.bind(&artist.artist.name, |field| {
            DetailLinks::text(&artist_field(&artist, field))
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .selected_library()
                .as_deref()
                .is_some_and(|selected| {
                    selected
                        .library
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
                DetailLinks::text(&artist_field(&artist, field))
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
                None,
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

        let cover = cards::square_cover_frame(&overlay, &controls.transport);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_playlist = Rc::clone(&current_playlist);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_playlist
                .borrow()
                .as_ref()
                .is_some_and(|playlist| {
                    selected
                        .library
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
            DetailLinks::text(&playlist_field(&playlist, field))
        });
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .selected_library()
                .as_deref()
                .is_some_and(|selected| {
                    selected
                        .library
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
                DetailLinks::text(&playlist_field(&playlist, field))
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
                None,
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

        let cover = cards::square_cover_frame(&overlay, &controls.transport);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_playlist = Rc::clone(&current_playlist);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_playlist
                .borrow()
                .as_ref()
                .is_some_and(|playlist| {
                    selected
                        .library
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
            |field| DetailLinks::text(&smart_playlist_field(&playlist, field)),
        );
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .selected_library()
                .as_deref()
                .is_some_and(|selected| {
                    selected
                        .library
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
                |field| DetailLinks::text(&smart_playlist_field(&playlist, field)),
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
            present_artist_context_menu(target, &shell, artist, None, position);
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
            present_genre_context_menu(target, &shell, genre, None, position);
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
            present_playlist_context_menu(target, &shell, playlist, None, position);
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
            present_smart_playlist_context_menu(target, &shell, playlist, None, position);
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
    let source = shell.selected_source_operations();
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
        let Some(source) = source.as_ref() else {
            return false;
        };
        let Some(widget) = widget.upgrade() else {
            return false;
        };
        let after = y > f64::from(widget.height()) / 2.0;
        source.move_smart_playlist(dragged_id, target_id, after);
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
        mut field_value: impl FnMut(LibraryField) -> DetailLinks,
    ) {
        self.title.set_text(title);
        self.title
            .set_tooltip_text((!title.is_empty()).then_some(title));
        for field in self.fields.borrow().iter() {
            field.bind(field_value(field.field));
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
    links: DetailLinkBinding,
}

impl CollectionGridFieldCell {
    fn new(shell: &Rc<Shell>, field: LibraryField) -> Self {
        let (widget, label) = collection_grid_field_label("", field);
        let links = DetailLinkBinding::new(&label, shell);
        Self {
            field,
            widget,
            label,
            links,
        }
    }

    fn bind(&self, links: DetailLinks) {
        self.links.bind(links);
        let value = self.label.text();
        self.label
            .set_tooltip_text((!value.is_empty()).then_some(value.as_str()));
    }

    fn clear(&self) {
        self.links.clear();
        self.label.set_tooltip_text(None);
        self.widget.set_cursor_from_name(None);
    }
}

#[cfg(test)]
mod tests {
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
    fn collection_grid_columns_follow_the_shared_card_width_band() {
        let minimum_slot = COLLECTION_GRID_MIN_CARD_WIDTH + COLLECTION_GRID_CARD_MARGIN * 2;
        let maximum_slot = COLLECTION_GRID_MAX_CARD_WIDTH + COLLECTION_GRID_CARD_MARGIN * 2;
        assert_eq!(collection_grid_column_count(435), 3);
        assert_eq!(collection_grid_column_count(450), 3);
        assert_eq!(collection_grid_column_count(496), 3);
        assert_eq!(collection_grid_column_count(minimum_slot * 3 - 1), 2);
        assert_eq!(collection_grid_column_count(maximum_slot * 2 + 1), 3);
        assert_eq!(collection_grid_column_limit(600, 0, 0), 3);
        assert_eq!(collection_grid_column_count(1_600), 8);
        assert_eq!(collection_grid_column_count(3_200), 16);
    }
}
