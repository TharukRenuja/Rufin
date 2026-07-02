use super::*;
use crate::i18n::msgid;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(super) trait ReusableCollectionGridCell<T>: 'static {
    fn widget(&self) -> gtk::Widget;
    fn bind(&self, position: u32, value: T);
    fn clear(&self);
}

pub(super) fn reusable_collection_grid<T, Cell, Make, Activate>(
    model: gio::ListStore,
    columns: usize,
    make_cell: Make,
    activate: Activate,
) -> gtk::GridView
where
    T: Clone + 'static,
    Cell: ReusableCollectionGridCell<T>,
    Make: Fn() -> Cell + 'static,
    Activate: Fn(u32, T) + 'static,
{
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let cells = Rc::new(RefCell::new(HashMap::<usize, Cell>::new()));
    let setup_cells = Rc::clone(&cells);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let cell = make_cell();
        item.set_child(Some(&cell.widget()));
        setup_cells
            .borrow_mut()
            .insert(item.as_ptr() as usize, cell);
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
    let teardown_cells = cells;
    factory.connect_teardown(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        teardown_cells
            .borrow_mut()
            .remove(&(item.as_ptr() as usize));
        item.set_child(None::<&gtk::Widget>);
    });

    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    grid.connect_activate(move |_, position| {
        if let Some(value) = item_at::<T>(&model, position) {
            activate(position, value);
        }
    });
    grid
}

pub(super) struct TrackGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    model: gio::ListStore,
    play_context: Option<LoadedTrackPlayContext>,
    favorite: gtk::Button,
    current_track: Rc<RefCell<Option<Track>>>,
    current_play_action: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
}

impl TrackGridCell {
    pub(super) fn new(
        shell: Rc<Shell>,
        fields: &[LibraryField],
        size: i32,
        model: gio::ListStore,
        play_context: Option<LoadedTrackPlayContext>,
    ) -> Self {
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let current_play_action = Rc::new(RefCell::new(None::<Rc<dyn Fn()>>));

        let overlay = cards::cover_overlay(size);
        let (cover_button, cover_tile) = collection_grid_cover_button(size);
        let controller = shell.controller.clone();
        let button_track = Rc::clone(&current_track);
        let button_play_action = Rc::clone(&current_play_action);
        cover_button.connect_clicked(move |_| {
            if let Some(play_action) = button_play_action.borrow().as_ref() {
                play_action();
            } else if let Some(track) = button_track.borrow().as_ref() {
                controller.play_now(track.clone());
            }
        });
        overlay.set_child(Some(&cover_button));

        let (mut controls, favorite) =
            cards::cover_hover_controls_with_favorite(size, msgid("Play track"), false);
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_track = Rc::clone(&current_track);
        let menu_target = overlay.clone();
        menu.connect_clicked(move |_| {
            let Some(track) = menu_track.borrow().as_ref().cloned() else {
                return;
            };
            let track = context_track(&menu_shell, &track);
            present_track_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                track,
                cards::cover_context_point(size),
            );
        });

        let controller = shell.controller.clone();
        let play_track = Rc::clone(&current_track);
        let play_action = Rc::clone(&current_play_action);
        controls.play.connect_clicked(move |_| {
            if let Some(play_action) = play_action.borrow().as_ref() {
                play_action();
            } else if let Some(track) = play_track.borrow().as_ref() {
                controller.play_now(track.clone());
            }
        });

        let controller = shell.controller.clone();
        let next_track = Rc::clone(&current_track);
        controls.play_next.connect_clicked(move |_| {
            if let Some(track) = next_track.borrow().as_ref() {
                controller.play_next(track.clone());
            }
        });

        let controller = shell.controller.clone();
        let last_track = Rc::clone(&current_track);
        controls.play_last.connect_clicked(move |_| {
            if let Some(track) = last_track.borrow().as_ref() {
                controller.play_last(vec![track.clone()]);
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
                source::FavoriteItemId::Track(track.id),
                favorite,
                Some(button),
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let body = CollectionGridCardCell::new(&shell, fields, size, cover_tile, overlay.upcast());
        install_dynamic_track_context_menu(&body.card, &shell, Rc::clone(&current_track));

        Self {
            body,
            shell,
            model,
            play_context,
            favorite,
            current_track,
            current_play_action,
        }
    }
}

impl ReusableCollectionGridCell<Track> for TrackGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, position: u32, track: Track) {
        let play_action = self.play_context.as_ref().map(|context| {
            track_model_play_action(
                &self.shell,
                &self.model,
                context.clone(),
                Some(position),
                track.clone(),
            )
        });
        self.body.bind(
            &self.shell,
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            &track.title,
            |field| {
                (
                    track_field(&track, field),
                    track_grid_field_route(&track, field),
                )
            },
        );
        set_favorite_button_active(&self.favorite, track.favorite);
        *self.current_track.borrow_mut() = Some(track);
        *self.current_play_action.borrow_mut() = play_action;
    }

    fn clear(&self) {
        self.body.clear();
        *self.current_track.borrow_mut() = None;
        *self.current_play_action.borrow_mut() = None;
    }
}

pub(super) struct AlbumGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    favorite: gtk::Button,
    current_album: Rc<RefCell<Option<Album>>>,
}

impl AlbumGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField], size: i32) -> Self {
        let current_album = Rc::new(RefCell::new(None::<Album>));

        let overlay = cards::cover_overlay(size);
        let (album_button, cover_tile) = collection_grid_cover_button(size);
        let open_shell = Rc::clone(&shell);
        let open_album = Rc::clone(&current_album);
        album_button.connect_clicked(move |_| {
            let Some(album) = open_album.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::AlbumDetail(album.id));
        });
        overlay.set_child(Some(&album_button));

        let (mut controls, favorite) =
            cards::cover_hover_controls_with_favorite(size, msgid("Play album"), false);
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_album = Rc::clone(&current_album);
        let menu_target = overlay.clone();
        menu.connect_clicked(move |_| {
            let Some(album) = menu_album.borrow().as_ref().cloned() else {
                return;
            };
            present_album_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                context_album(&menu_shell, &album),
                cards::cover_context_point(size),
            );
        });

        let controller = shell.controller.clone();
        let play_album = Rc::clone(&current_album);
        controls.play.connect_clicked(move |_| {
            if let Some(album) = play_album.borrow().as_ref() {
                controller.play_album_now(album.id.clone());
            }
        });

        let controller = shell.controller.clone();
        let next_album = Rc::clone(&current_album);
        controls.play_next.connect_clicked(move |_| {
            let Some(album_id) = next_album.borrow().as_ref().map(|album| album.id.clone()) else {
                return;
            };
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                for track in tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        });

        let controller = shell.controller.clone();
        let last_album = Rc::clone(&current_album);
        controls.play_last.connect_clicked(move |_| {
            let Some(album_id) = last_album.borrow().as_ref().map(|album| album.id.clone()) else {
                return;
            };
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                controller.play_last(tracks);
            }
        });

        let favorite_key_album = Rc::clone(&current_album);
        shell.register_dynamic_favorite_button(
            Rc::new(move || {
                favorite_key_album
                    .borrow()
                    .as_ref()
                    .map(|album| album_favorite_key(&album.id))
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
                source::FavoriteItemId::Album(album.id),
                favorite,
                Some(button),
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let body = CollectionGridCardCell::new(&shell, fields, size, cover_tile, overlay.upcast());
        install_dynamic_album_context_menu(&body.card, &shell, Rc::clone(&current_album));

        Self {
            body,
            shell,
            favorite,
            current_album,
        }
    }
}

impl ReusableCollectionGridCell<Album> for AlbumGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, album: Album) {
        self.body.bind(
            &self.shell,
            album.image_ref.as_ref(),
            album.color_seed,
            &album.title,
            |field| {
                let value = album_field(&album, field);
                let route = if value.is_empty()
                    || !matches!(field, LibraryField::Artist | LibraryField::AlbumArtist)
                {
                    None
                } else {
                    album_artist_route(&album)
                };
                (value, route)
            },
        );
        set_favorite_button_active(&self.favorite, album.favorite);
        *self.current_album.borrow_mut() = Some(album);
    }

    fn clear(&self) {
        self.body.clear();
        *self.current_album.borrow_mut() = None;
    }
}

struct CollectionGridCardCell {
    card: gtk::Box,
    cover_tile: ArtworkTile,
    size: i32,
    title: gtk::Label,
    fields: Vec<CollectionGridFieldCell>,
}

impl CollectionGridCardCell {
    fn new(
        shell: &Rc<Shell>,
        fields: &[LibraryField],
        size: i32,
        cover_tile: ArtworkTile,
        cover: gtk::Widget,
    ) -> Self {
        let card = collection_grid_card(size, fields.len());
        card.append(&cover);
        let (title_widget, title) = grid_title_with_label("", "track-title", size);
        card.append(&title_widget);
        let fields = fields
            .iter()
            .copied()
            .map(|field| CollectionGridFieldCell::new(shell, field, size))
            .collect::<Vec<_>>();
        for field in &fields {
            card.append(&field.widget);
        }
        Self {
            card,
            cover_tile,
            size,
            title,
            fields,
        }
    }

    fn widget(&self) -> gtk::Widget {
        self.card.clone().upcast()
    }

    fn bind(
        &self,
        shell: &Rc<Shell>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        title: &str,
        mut field_value: impl FnMut(LibraryField) -> (String, Option<Route>),
    ) {
        shell.bind_cover_tile_for(
            &self.cover_tile,
            image_ref,
            seed,
            self.size,
            GRID_COVER_SIZE,
        );
        self.title.set_text(title);
        self.title
            .set_tooltip_text((!title.is_empty()).then_some(title));
        for field in &self.fields {
            let (value, route) = field_value(field.field);
            field.bind(value, route);
        }
    }

    fn clear(&self) {
        self.cover_tile.clear_image();
        self.title.set_text("");
        self.title.set_tooltip_text(None);
        for field in &self.fields {
            field.clear();
        }
    }
}

fn collection_grid_cover_button(size: i32) -> (gtk::Button, ArtworkTile) {
    let cover_button = gtk::Button::new();
    cover_button.add_css_class("album-cover-button");
    cover_button.add_css_class("flat");
    cards::constrain_cover_widget(&cover_button, size);
    cards::clip_cover(&cover_button);
    let cover_tile = ArtworkTile::new(size, 0);
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
    fn new(shell: &Rc<Shell>, field: LibraryField, size: i32) -> Self {
        let (widget, label) = collection_grid_field_label("", field, size);
        widget.set_visible(false);
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
        self.widget.set_visible(!value.is_empty());
    }

    fn clear(&self) {
        *self.route.borrow_mut() = None;
        self.label.set_text("");
        self.label.set_tooltip_text(None);
        self.widget.set_cursor_from_name(None);
        self.label.set_cursor_from_name(None);
        self.widget.set_visible(false);
    }
}

fn install_dynamic_grid_label_link(
    shell: &Rc<Shell>,
    target: &gtk::Widget,
    label: &gtk::Label,
    route: Rc<RefCell<Option<Route>>>,
) {
    let enter_label = label.clone();
    let enter_route = Rc::clone(&route);
    let leave_label = label.clone();
    let leave_route = Rc::clone(&route);
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        if enter_route.borrow().is_none() {
            return;
        }
        let text = enter_label.text();
        if text.is_empty() {
            return;
        }
        let escaped_text = glib::markup_escape_text(text.as_str());
        enter_label.add_css_class("hovered-link");
        enter_label.set_markup(&format!("<u>{escaped_text}</u>"));
    });
    motion.connect_leave(move |_| {
        if leave_route.borrow().is_some() {
            let text = leave_label.text().to_string();
            leave_label.remove_css_class("hovered-link");
            leave_label.set_text(&text);
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
