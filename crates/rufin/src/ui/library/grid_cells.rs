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

pub(super) fn collection_grid<T, Cell, Make, Activate>(
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
    cover_tile: ArtworkTile,
    size: i32,
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

        let body = CollectionGridCardCell::new(&shell, fields, size, overlay.upcast());
        install_dynamic_track_context_menu(&body.card, &shell, Rc::clone(&current_track));

        Self {
            body,
            shell,
            model,
            play_context,
            cover_tile,
            size,
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
        self.shell.bind_cover_tile_for(
            &self.cover_tile,
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            self.size,
            GRID_COVER_SIZE,
        );
        self.body.bind(&track.title, |field| {
            (
                track_field(&track, field),
                track_grid_field_route(&track, field),
            )
        });
        set_favorite_button_active(&self.favorite, track.favorite);
        *self.current_track.borrow_mut() = Some(track);
        *self.current_play_action.borrow_mut() = play_action;
    }

    fn clear(&self) {
        self.cover_tile.clear_image();
        self.body.clear();
        *self.current_track.borrow_mut() = None;
        *self.current_play_action.borrow_mut() = None;
    }
}

pub(super) struct AlbumGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_tile: ArtworkTile,
    size: i32,
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

        let body = CollectionGridCardCell::new(&shell, fields, size, overlay.upcast());
        install_dynamic_album_context_menu(&body.card, &shell, Rc::clone(&current_album));

        Self {
            body,
            shell,
            cover_tile,
            size,
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
        self.shell.bind_cover_tile_for(
            &self.cover_tile,
            album.image_ref.as_ref(),
            album.color_seed,
            self.size,
            GRID_COVER_SIZE,
        );
        self.body.bind(&album.title, |field| {
            let value = album_field(&album, field);
            let route = if value.is_empty()
                || !matches!(field, LibraryField::Artist | LibraryField::AlbumArtist)
            {
                None
            } else {
                album_artist_route(&album)
            };
            (value, route)
        });
        set_favorite_button_active(&self.favorite, album.favorite);
        *self.current_album.borrow_mut() = Some(album);
    }

    fn clear(&self) {
        self.cover_tile.clear_image();
        self.body.clear();
        *self.current_album.borrow_mut() = None;
    }
}

pub(super) struct ArtistGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_tile: ArtworkTile,
    size: i32,
    favorite: gtk::Button,
    current_artist: Rc<RefCell<Option<Artist>>>,
}

impl ArtistGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField], size: i32) -> Self {
        let current_artist = Rc::new(RefCell::new(None::<Artist>));

        let overlay = cards::cover_overlay(size);
        let (artist_button, cover_tile) = collection_grid_cover_button(size);
        let open_shell = Rc::clone(&shell);
        let open_artist = Rc::clone(&current_artist);
        artist_button.connect_clicked(move |_| {
            let Some(artist) = open_artist.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::ArtistDetail(artist.id));
        });
        overlay.set_child(Some(&artist_button));

        let (mut controls, favorite) =
            cards::cover_hover_controls_with_favorite(size, msgid("Play artist"), false);
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_artist = Rc::clone(&current_artist);
        let menu_target = overlay.clone();
        menu.connect_clicked(move |_| {
            let Some(artist) = menu_artist.borrow().as_ref().cloned() else {
                return;
            };
            present_artist_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                context_artist(&menu_shell, &artist),
                cards::cover_context_point(size),
            );
        });

        let controller = shell.controller.clone();
        let play_artist = Rc::clone(&current_artist);
        controls.play.connect_clicked(move |_| {
            let Some(artist_id) = play_artist
                .borrow()
                .as_ref()
                .map(|artist| artist.id.clone())
            else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_artist_detail(&artist_id) {
                controller.play_artist_tracks_window(
                    artist_id,
                    domain::ArtistTrackScope::AllCredits,
                    detail.tracks.len(),
                    0,
                    |index| detail.tracks.get(index).cloned(),
                );
            }
        });

        let controller = shell.controller.clone();
        let next_artist = Rc::clone(&current_artist);
        controls.play_next.connect_clicked(move |_| {
            let Some(artist_id) = next_artist
                .borrow()
                .as_ref()
                .map(|artist| artist.id.clone())
            else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_artist_detail(&artist_id) {
                for track in detail.tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        });

        let controller = shell.controller.clone();
        let last_artist = Rc::clone(&current_artist);
        controls.play_last.connect_clicked(move |_| {
            let Some(artist_id) = last_artist
                .borrow()
                .as_ref()
                .map(|artist| artist.id.clone())
            else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_artist_detail(&artist_id) {
                controller.play_last(detail.tracks);
            }
        });

        let favorite_key_artist = Rc::clone(&current_artist);
        shell.register_dynamic_favorite_button(
            Rc::new(move || {
                favorite_key_artist
                    .borrow()
                    .as_ref()
                    .map(|artist| artist_favorite_key(&artist.id))
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
                source::FavoriteItemId::Artist(artist.id),
                favorite,
                Some(button),
            );
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let body = CollectionGridCardCell::new(&shell, fields, size, overlay.upcast());
        install_dynamic_artist_context_menu(&body.card, &shell, Rc::clone(&current_artist));

        Self {
            body,
            shell,
            cover_tile,
            size,
            favorite,
            current_artist,
        }
    }
}

impl ReusableCollectionGridCell<Artist> for ArtistGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, artist: Artist) {
        let image_ref = artist_cover_image_ref(&self.shell, &artist);
        self.shell.bind_cover_tile_for(
            &self.cover_tile,
            image_ref.as_ref(),
            stable_seed(artist.id.as_str()),
            self.size,
            GRID_COVER_SIZE,
        );
        self.body
            .bind(&artist.name, |field| (artist_field(&artist, field), None));
        set_favorite_button_active(&self.favorite, artist.favorite);
        *self.current_artist.borrow_mut() = Some(artist);
    }

    fn clear(&self) {
        self.cover_tile.clear_image();
        self.body.clear();
        *self.current_artist.borrow_mut() = None;
    }
}

pub(super) struct GenreGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    size: i32,
    current_genre: Rc<RefCell<Option<Genre>>>,
}

impl GenreGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField], size: i32) -> Self {
        let current_genre = Rc::new(RefCell::new(None::<Genre>));

        let overlay = cards::cover_overlay(size);
        let cover_button = collection_grid_cover_shell(size);
        let open_shell = Rc::clone(&shell);
        let open_genre = Rc::clone(&current_genre);
        cover_button.connect_clicked(move |_| {
            let Some(genre) = open_genre.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::GenreDetail(genre.id));
        });
        overlay.set_child(Some(&cover_button));

        let mut controls = cards::cover_play_hover_controls(size, msgid("Play genre"));
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_genre = Rc::clone(&current_genre);
        let menu_target = overlay.clone();
        menu.connect_clicked(move |_| {
            let Some(genre) = menu_genre.borrow().as_ref().cloned() else {
                return;
            };
            present_genre_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                genre,
                cards::cover_context_point(size),
            );
        });

        let controller = shell.controller.clone();
        let play_genre = Rc::clone(&current_genre);
        controls.play.connect_clicked(move |_| {
            let Some(genre_id) = play_genre.borrow().as_ref().map(|genre| genre.id.clone()) else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
                let tracks = detail.tracks;
                controller.play_genre_tracks_window(genre_id, tracks.len(), 0, |index| {
                    tracks.get(index).cloned()
                });
            }
        });

        let controller = shell.controller.clone();
        let next_genre = Rc::clone(&current_genre);
        controls.play_next.connect_clicked(move |_| {
            let Some(genre_id) = next_genre.borrow().as_ref().map(|genre| genre.id.clone()) else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
                for track in detail.tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        });

        let controller = shell.controller.clone();
        let last_genre = Rc::clone(&current_genre);
        controls.play_last.connect_clicked(move |_| {
            let Some(genre_id) = last_genre.borrow().as_ref().map(|genre| genre.id.clone()) else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
                controller.play_last(detail.tracks);
            }
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let body = CollectionGridCardCell::new(&shell, fields, size, overlay.upcast());
        install_dynamic_genre_context_menu(&body.card, &shell, Rc::clone(&current_genre));

        Self {
            body,
            shell,
            cover_button,
            size,
            current_genre,
        }
    }
}

impl ReusableCollectionGridCell<Genre> for GenreGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, genre: Genre) {
        let artwork = crate::cover_art_policy::selected_genre_artwork(&genre);
        self.cover_button
            .set_child(Some(&self.shell.cover_group_tile_for_artwork(
                &artwork,
                stable_seed(genre.id.as_str()),
                self.size,
                THUMB_COVER_SIZE,
            )));
        self.body
            .bind(&genre.name, |field| (genre_field(&genre, field), None));
        *self.current_genre.borrow_mut() = Some(genre);
    }

    fn clear(&self) {
        self.cover_button.set_child(None::<&gtk::Widget>);
        self.body.clear();
        *self.current_genre.borrow_mut() = None;
    }
}

pub(super) struct PlaylistGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    size: i32,
    current_playlist: Rc<RefCell<Option<Playlist>>>,
}

impl PlaylistGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField], size: i32) -> Self {
        let current_playlist = Rc::new(RefCell::new(None::<Playlist>));

        let overlay = cards::cover_overlay(size);
        let cover_button = collection_grid_cover_shell(size);
        let open_shell = Rc::clone(&shell);
        let open_playlist = Rc::clone(&current_playlist);
        cover_button.connect_clicked(move |_| {
            let Some(playlist) = open_playlist.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::PlaylistDetail(playlist.id));
        });
        overlay.set_child(Some(&cover_button));

        let mut controls = cards::cover_play_hover_controls(size, msgid("Play playlist"));
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_playlist = Rc::clone(&current_playlist);
        let menu_target = overlay.clone();
        menu.connect_clicked(move |_| {
            let Some(playlist) = menu_playlist.borrow().as_ref().cloned() else {
                return;
            };
            present_playlist_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                playlist,
                cards::cover_context_point(size),
            );
        });

        let controller = shell.controller.clone();
        let play_playlist = Rc::clone(&current_playlist);
        controls.play.connect_clicked(move |_| {
            let Some(playlist_id) = play_playlist
                .borrow()
                .as_ref()
                .map(|playlist| playlist.id.clone())
            else {
                return;
            };
            controller.play_cached_playlist(playlist_id);
        });

        let controller = shell.controller.clone();
        let next_playlist = Rc::clone(&current_playlist);
        controls.play_next.connect_clicked(move |_| {
            let Some(playlist_id) = next_playlist
                .borrow()
                .as_ref()
                .map(|playlist| playlist.id.clone())
            else {
                return;
            };
            controller.play_cached_playlist_next(playlist_id);
        });

        let controller = shell.controller.clone();
        let last_playlist = Rc::clone(&current_playlist);
        controls.play_last.connect_clicked(move |_| {
            let Some(playlist_id) = last_playlist
                .borrow()
                .as_ref()
                .map(|playlist| playlist.id.clone())
            else {
                return;
            };
            controller.play_cached_playlist_last(playlist_id);
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let body = CollectionGridCardCell::new(&shell, fields, size, overlay.upcast());
        install_dynamic_playlist_context_menu(&body.card, &shell, Rc::clone(&current_playlist));

        Self {
            body,
            shell,
            cover_button,
            size,
            current_playlist,
        }
    }
}

impl ReusableCollectionGridCell<Playlist> for PlaylistGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, playlist: Playlist) {
        let artwork = crate::cover_art_policy::selected_playlist_artwork(
            &playlist,
            &self.shell.state.settings.borrow(),
        );
        self.cover_button
            .set_child(Some(&self.shell.cover_group_tile_for_artwork(
                &artwork,
                stable_seed(playlist.id.as_str()),
                self.size,
                THUMB_COVER_SIZE,
            )));
        self.body.bind(&playlist.name, |field| {
            (playlist_field(&playlist, field), None)
        });
        *self.current_playlist.borrow_mut() = Some(playlist);
    }

    fn clear(&self) {
        self.cover_button.set_child(None::<&gtk::Widget>);
        self.body.clear();
        *self.current_playlist.borrow_mut() = None;
    }
}

pub(super) struct SmartPlaylistGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    widget: gtk::Overlay,
    size: i32,
    current_playlist: Rc<RefCell<Option<SmartPlaylist>>>,
}

impl SmartPlaylistGridCell {
    pub(super) fn new(shell: Rc<Shell>, fields: &[LibraryField], size: i32) -> Self {
        let current_playlist = Rc::new(RefCell::new(None::<SmartPlaylist>));

        let overlay = cards::cover_overlay(size);
        let cover_button = collection_grid_cover_shell(size);
        let open_shell = Rc::clone(&shell);
        let open_playlist = Rc::clone(&current_playlist);
        cover_button.connect_clicked(move |_| {
            let Some(playlist) = open_playlist.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(Route::SmartPlaylistDetail(playlist.id));
        });
        overlay.set_child(Some(&cover_button));

        let mut controls = cards::cover_play_hover_controls(size, msgid("Play smart playlist"));
        let menu = controls.add_context_button();
        let menu_shell = Rc::clone(&shell);
        let menu_playlist = Rc::clone(&current_playlist);
        let menu_target = overlay.clone();
        menu.connect_clicked(move |_| {
            let Some(playlist) = menu_playlist.borrow().as_ref().cloned() else {
                return;
            };
            present_smart_playlist_context_menu(
                menu_target.upcast_ref(),
                &menu_shell,
                playlist,
                cards::cover_context_point(size),
            );
        });

        let controller = shell.controller.clone();
        let play_playlist = Rc::clone(&current_playlist);
        controls.play.connect_clicked(move |_| {
            let Some(playlist_id) = play_playlist
                .borrow()
                .as_ref()
                .map(|playlist| playlist.id.clone())
            else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id) {
                controller.play_smart_playlist_detail(detail);
            }
        });

        let controller = shell.controller.clone();
        let next_playlist = Rc::clone(&current_playlist);
        controls.play_next.connect_clicked(move |_| {
            let Some(playlist_id) = next_playlist
                .borrow()
                .as_ref()
                .map(|playlist| playlist.id.clone())
            else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id) {
                for track in detail.tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        });

        let controller = shell.controller.clone();
        let last_playlist = Rc::clone(&current_playlist);
        controls.play_last.connect_clicked(move |_| {
            let Some(playlist_id) = last_playlist
                .borrow()
                .as_ref()
                .map(|playlist| playlist.id.clone())
            else {
                return;
            };
            if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id) {
                controller.play_last(detail.tracks);
            }
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let body = CollectionGridCardCell::new(&shell, fields, size, overlay.upcast());
        let widget = smart_playlist_grid_overlay(
            size,
            fields.len(),
            &body.card,
            Rc::clone(&current_playlist),
        );
        install_dynamic_smart_playlist_drop_target(&widget, &shell, Rc::clone(&current_playlist));
        install_dynamic_smart_playlist_context_menu(&widget, &shell, Rc::clone(&current_playlist));

        Self {
            body,
            shell,
            cover_button,
            widget,
            size,
            current_playlist,
        }
    }
}

impl ReusableCollectionGridCell<SmartPlaylist> for SmartPlaylistGridCell {
    fn widget(&self) -> gtk::Widget {
        self.widget.clone().upcast()
    }

    fn bind(&self, _: u32, playlist: SmartPlaylist) {
        let artwork = crate::cover_art_policy::selected_smart_playlist_artwork(&playlist);
        self.cover_button
            .set_child(Some(&self.shell.cover_group_tile_for_artwork(
                &artwork,
                stable_seed(playlist.id.as_str()),
                self.size,
                THUMB_COVER_SIZE,
            )));
        self.body
            .bind(&smart_playlist_display_name(&playlist), |field| {
                (smart_playlist_field(&playlist, field), None)
            });
        *self.current_playlist.borrow_mut() = Some(playlist);
    }

    fn clear(&self) {
        self.cover_button.set_child(None::<&gtk::Widget>);
        self.body.clear();
        *self.current_playlist.borrow_mut() = None;
    }
}

fn install_dynamic_artist_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    artist: Rc<RefCell<Option<Artist>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(artist) = artist.borrow().clone() else {
                return;
            };
            present_artist_context_menu(target, &shell, context_artist(&shell, &artist), position);
        }),
    );
}

fn install_dynamic_genre_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    genre: Rc<RefCell<Option<Genre>>>,
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
    playlist: Rc<RefCell<Option<Playlist>>>,
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
    playlist: Rc<RefCell<Option<SmartPlaylist>>>,
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
    size: i32,
    field_count: usize,
    card: &gtk::Box,
    playlist: Rc<RefCell<Option<SmartPlaylist>>>,
) -> gtk::Overlay {
    let card_height = collection_grid_card_height(size, field_count);
    let overlay = gtk::Overlay::new();
    overlay.set_size_request(size, card_height);
    overlay.set_hexpand(false);
    overlay.set_halign(gtk::Align::Center);
    overlay.set_child(Some(card));

    let drag = dynamic_smart_playlist_drag_handle(playlist);
    drag.set_margin_start(6);
    drag.set_margin_top(6);
    drag.set_halign(gtk::Align::Start);
    drag.set_valign(gtk::Align::Start);
    overlay.add_overlay(&drag);
    overlay
}

fn dynamic_smart_playlist_drag_handle(playlist: Rc<RefCell<Option<SmartPlaylist>>>) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("rufin-list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(SMART_PLAYLIST_REORDER_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    source.connect_prepare(move |_, _, _| {
        let playlist_id = playlist.borrow().as_ref()?.id.as_str().to_string();
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
    playlist: Rc<RefCell<Option<SmartPlaylist>>>,
) {
    let widget = target.as_ref().clone();
    let controller = shell.controller.clone();
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(dragged_id) = value.get::<String>() else {
            return false;
        };
        let Some(target_id) = playlist
            .borrow()
            .as_ref()
            .map(|playlist| playlist.id.clone())
        else {
            return false;
        };
        let dragged_id = SmartPlaylistId::new(dragged_id);
        if dragged_id == target_id {
            return false;
        }
        let after = y > f64::from(widget.height()) / 2.0;
        controller.move_smart_playlist(dragged_id, target_id, after);
        true
    });
    target.add_controller(drop_target);
}

struct CollectionGridCardCell {
    card: gtk::Box,
    title: gtk::Label,
    fields: Vec<CollectionGridFieldCell>,
}

impl CollectionGridCardCell {
    fn new(shell: &Rc<Shell>, fields: &[LibraryField], size: i32, cover: gtk::Widget) -> Self {
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
            title,
            fields,
        }
    }

    fn widget(&self) -> gtk::Widget {
        self.card.clone().upcast()
    }

    fn bind(
        &self,
        title: &str,
        mut field_value: impl FnMut(LibraryField) -> (String, Option<Route>),
    ) {
        self.title.set_text(title);
        self.title
            .set_tooltip_text((!title.is_empty()).then_some(title));
        for field in &self.fields {
            let (value, route) = field_value(field.field);
            field.bind(value, route);
        }
    }

    fn clear(&self) {
        self.title.set_text("");
        self.title.set_tooltip_text(None);
        for field in &self.fields {
            field.clear();
        }
    }
}

fn collection_grid_cover_shell(size: i32) -> gtk::Button {
    let cover_button = gtk::Button::new();
    cover_button.add_css_class("album-cover-button");
    cover_button.add_css_class("flat");
    cards::constrain_cover_widget(&cover_button, size);
    cards::clip_cover(&cover_button);
    cover_button
}

fn collection_grid_cover_button(size: i32) -> (gtk::Button, ArtworkTile) {
    let cover_button = collection_grid_cover_shell(size);
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
