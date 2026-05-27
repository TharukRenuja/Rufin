fn genre_cover_refs(shell: &Rc<Shell>, genre: &Genre) -> Vec<ImageRef> {
    let library = shell.state.library.borrow();
    let mut refs = Vec::new();
    for album in &library.albums {
        if album.genres.iter().any(|name| name == &genre.name) {
            push_unique_image_ref(&mut refs, album.image_ref.as_ref());
            if refs.len() >= 4 {
                return refs;
            }
        }
    }
    if !refs.is_empty() {
        return refs;
    }

    let mut seen_albums = HashSet::new();
    for track in &library.tracks {
        if track.genres.iter().any(|name| name == &genre.name)
            && !seen_albums.contains(&track.album_id)
        {
            let before = refs.len();
            push_unique_image_ref(&mut refs, track.image_ref.as_ref());
            if refs.len() > before {
                seen_albums.insert(track.album_id.clone());
            }
            if refs.len() >= 4 {
                return refs;
            }
        }
    }
    refs
}
fn push_unique_image_ref(refs: &mut Vec<ImageRef>, image_ref: Option<&ImageRef>) {
    if refs.len() >= 4 {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    if !refs.iter().any(|existing| existing == image_ref) {
        refs.push(image_ref.clone());
    }
}
fn genre_cover_tile(shell: &Rc<Shell>, genre: &Genre, size: i32) -> gtk::Widget {
    let overlay = cards::cover_overlay(size);

    let genre_button = gtk::Button::new();
    genre_button.add_css_class("album-cover-button");
    genre_button.add_css_class("flat");
    cards::constrain_cover_widget(&genre_button, size);
    let cover_refs = genre_cover_refs(shell, genre);
    genre_button.set_child(Some(&shell.cover_group_tile_for(
        cover_refs,
        genre.image_ref.as_ref(),
        stable_seed(genre.id.as_str()),
        size,
        GRID_COVER_SIZE,
    )));
    let open_shell = Rc::clone(shell);
    let open_genre_id = genre.id.clone();
    genre_button
        .connect_clicked(move |_| open_shell.navigate(Route::GenreDetail(open_genre_id.clone())));
    overlay.set_child(Some(&genre_button));

    let controls = cards::cover_play_hover_controls(size, "Play genre");
    let controller = shell.controller.clone();
    let genre_id = genre.id.clone();
    controls.play.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
            controller.play_tracks_now(detail.tracks);
        }
    });
    let controller = shell.controller.clone();
    let genre_id = genre.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
            for track in detail.tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    let controller = shell.controller.clone();
    let genre_id = genre.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
            controller.play_last(detail.tracks);
        }
    });
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

    overlay.upcast()
}
fn album_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => album_image_column(
            shell,
            "Image",
            column_width(LibraryField::Image),
        ),
        LibraryField::TitleMerged => album_merged_column(
            shell,
            "Title",
            column_width(LibraryField::TitleMerged),
        ),
        LibraryField::Title => album_text_column(shell, "Title", 220, true, |album| {
            album.title.clone()
        }),
        LibraryField::Favorite => album_favorite_column(shell),
        _ => album_text_column(shell, field.title(), column_width(field), false, move |album| {
            album_field(album, field)
        }),
    }
}
fn artist_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => artist_image_column(shell),
        LibraryField::TitleMerged | LibraryField::Title => {
            artist_text_column(shell, "Title", 220, true, |artist| artist.name.clone())
        }
        LibraryField::Favorite => artist_favorite_column(shell),
        _ => artist_text_column(
            shell,
            field.title(),
            column_width(field),
            false,
            move |artist| artist_field(artist, field),
        ),
    }
}
fn genre_column(field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Title | LibraryField::TitleMerged => {
            expanding_text_column::<Genre, _>("Title", 180, |genre| genre.name.clone())
        }
        _ => text_column::<Genre, _>(field.title(), column_width(field), move |genre| {
            genre_field(genre, field)
        }),
    }
}
fn playlist_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => image_column::<Playlist, _, _>(
            shell,
            "Image",
            column_width(LibraryField::Image),
            |playlist| playlist.image_ref.clone(),
            |playlist| stable_seed(playlist.id.as_str()),
        ),
        LibraryField::Title | LibraryField::TitleMerged => {
            expanding_text_column::<Playlist, _>("Title", 220, |playlist| playlist.name.clone())
        }
        _ => text_column::<Playlist, _>(field.title(), column_width(field), move |playlist| {
            playlist_field(playlist, field)
        }),
    }
}
fn track_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => {
            track_image_column(shell, "Image", column_width(LibraryField::Image))
        }
        LibraryField::TitleMerged => track_merged_column(
            shell,
            "Title",
            column_width(LibraryField::TitleMerged),
            |track| track.title.clone(),
            |track| track.artist.clone(),
            |track| track.image_ref.clone(),
            |track| stable_seed(track.id.as_str()),
        ),
        LibraryField::Title => {
            track_text_column(shell, "Title", 180, true, |track| track.title.clone())
        }
        LibraryField::Favorite => track_favorite_column(shell),
        _ => track_text_column(
            shell,
            field.title(),
            column_width(field),
            false,
            move |track| track_field(track, field),
        ),
    }
}
fn text_column<T, F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> String + 'static,
{
    text_column_with_expand(title, width, false, value)
}
fn expanding_text_column<T, F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> String + 'static,
{
    text_column_with_expand(title, width, true, value)
}
fn text_column_with_expand<T, F>(
    title: &str,
    width: i32,
    expand: bool,
    value: F,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.set_xalign(0.0);
            label.set_wrap(false);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_single_line_mode(true);
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let data = boxed.borrow::<T>();
        label.set_text(&(value)(&data));
    });
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(expand);
    column
}
fn row_index_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.add_css_class("muted");
            label.set_xalign(0.0);
            label.set_wrap(false);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_single_line_mode(true);
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        label.set_text(&(item.position() + 1).to_string());
    });
    let column = gtk::ColumnViewColumn::new(Some("#"), Some(factory));
    column.set_fixed_width(column_width(LibraryField::RowIndex));
    column
}
#[derive(Clone)]
struct LibraryAlbumImageCell {
    cover: ArtworkTile,
    current_album: Rc<RefCell<Option<Album>>>,
}

#[derive(Clone)]
struct LibraryAlbumTextCell {
    label: gtk::Label,
    current_album: Rc<RefCell<Option<Album>>>,
}

#[derive(Clone)]
struct LibraryAlbumMergedCell {
    cover: ArtworkTile,
    title: gtk::Label,
    subtitle: gtk::Label,
    current_album: Rc<RefCell<Option<Album>>>,
}

thread_local! {
    static LIBRARY_ALBUM_IMAGE_CELLS: RefCell<HashMap<usize, LibraryAlbumImageCell>> = RefCell::new(HashMap::new());
    static LIBRARY_ALBUM_TEXT_CELLS: RefCell<HashMap<usize, LibraryAlbumTextCell>> = RefCell::new(HashMap::new());
    static LIBRARY_ALBUM_MERGED_CELLS: RefCell<HashMap<usize, LibraryAlbumMergedCell>> = RefCell::new(HashMap::new());
}

fn album_image_cell(item: &gtk::ListItem) -> Option<LibraryAlbumImageCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn album_text_cell(item: &gtk::ListItem) -> Option<LibraryAlbumTextCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_TEXT_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn album_merged_cell(item: &gtk::ListItem) -> Option<LibraryAlbumMergedCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_MERGED_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn album_image_column(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_album = Rc::new(RefCell::new(None::<Album>));
        let cover = ArtworkTile::new(48, 0);
        let widget = cover.widget();
        install_dynamic_album_context_menu(&widget, &setup_shell, Rc::clone(&current_album));
        item.set_child(Some(&widget));
        let key = library_list_item_storage_key(item);
        LIBRARY_ALBUM_IMAGE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryAlbumImageCell {
                    cover,
                    current_album,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<Album>(item) else {
            return;
        };
        let Some(cell) = album_image_cell(item) else {
            return;
        };
        shell.bind_cover_tile_for(
            &cell.cover,
            album.image_ref.as_ref(),
            album.color_seed,
            48,
            THUMB_COVER_SIZE,
        );
        *cell.current_album.borrow_mut() = Some(album);
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = album_image_cell(item)
        {
            cell.cover.bind_image(0, None);
            *cell.current_album.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ALBUM_IMAGE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column
}

fn album_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    expand: bool,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Album) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_album = Rc::new(RefCell::new(None::<Album>));
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_dynamic_album_context_menu(&label, &setup_shell, Rc::clone(&current_album));
        item.set_child(Some(&label));
        let key = library_list_item_storage_key(item);
        LIBRARY_ALBUM_TEXT_CELLS.with(|cells| {
            cells
                .borrow_mut()
                .insert(key, LibraryAlbumTextCell { label, current_album });
        });
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<Album>(item) else {
            return;
        };
        let Some(cell) = album_text_cell(item) else {
            return;
        };
        cell.label.set_text(&(value)(&album));
        *cell.current_album.borrow_mut() = Some(album);
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = album_text_cell(item)
        {
            cell.label.set_text("");
            *cell.current_album.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ALBUM_TEXT_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(expand);
    column
}

fn album_merged_column(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_album = Rc::new(RefCell::new(None::<Album>));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_valign(gtk::Align::Center);

        let cover = ArtworkTile::new(48, 0);
        row.append(&cover.widget());

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_single_line_mode(true);
        labels.append(&title);

        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("muted");
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(false);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.set_single_line_mode(true);
        subtitle.set_visible(false);
        labels.append(&subtitle);

        row.append(&labels);
        install_dynamic_album_context_menu(&row, &setup_shell, Rc::clone(&current_album));
        item.set_child(Some(&row));
        let key = library_list_item_storage_key(item);
        LIBRARY_ALBUM_MERGED_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryAlbumMergedCell {
                    cover,
                    title,
                    subtitle,
                    current_album,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<Album>(item) else {
            return;
        };
        let Some(cell) = album_merged_cell(item) else {
            return;
        };
        shell.bind_cover_tile_for(
            &cell.cover,
            album.image_ref.as_ref(),
            album.color_seed,
            48,
            THUMB_COVER_SIZE,
        );
        cell.title.set_text(&album.title);
        cell.subtitle.set_text(&album.artist);
        cell.subtitle.set_visible(!album.artist.trim().is_empty());
        *cell.current_album.borrow_mut() = Some(album);
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = album_merged_cell(item)
        {
            cell.title.set_text("");
            cell.subtitle.set_text("");
            cell.subtitle.set_visible(false);
            cell.cover.bind_image(0, None);
            *cell.current_album.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ALBUM_MERGED_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(true);
    column
}

fn image_column<T, F, S>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    image_ref: F,
    seed: S,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> Option<rufin_core::ImageRef> + 'static,
    S: Fn(&T) -> u32 + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let image_ref = Rc::new(image_ref);
    let seed = Rc::new(seed);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let data = boxed.borrow::<T>();
        item.set_child(Some(&shell.cover_tile_for(
            image_ref(&data).as_ref(),
            seed(&data),
            48,
            THUMB_COVER_SIZE,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column
}
fn artist_image_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(artist) = item_at_from_item::<Artist>(item) else {
            return;
        };
        let image_ref = artist_cover_image_ref(&shell, &artist);
        let cover = shell.cover_tile_for(
            image_ref.as_ref(),
            stable_seed(artist.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );
        install_artist_context_menu(&cover, &shell, artist);
        item.set_child(Some(&cover));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr("Image")), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Image));
    column
}
fn artist_text_column<F>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    expand: bool,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Artist) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(artist) = item_at_from_item::<Artist>(item) else {
            return;
        };
        let label = gtk::Label::new(Some(&(value)(&artist)));
        label.set_xalign(0.0);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_artist_context_menu(&label, &shell, artist);
        item.set_child(Some(&label));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(expand);
    column
}
#[derive(Clone)]
struct LibraryTrackImageCell {
    cover: ArtworkTile,
    current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
struct LibraryTrackTextCell {
    label: gtk::Label,
    current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
struct LibraryTrackMergedCell {
    cover: ArtworkTile,
    title: gtk::Label,
    subtitle: gtk::Label,
    current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
struct LibraryTrackFavoriteCell {
    button: gtk::Button,
    current_track: Rc<RefCell<Option<Track>>>,
}

thread_local! {
    static LIBRARY_TRACK_IMAGE_CELLS: RefCell<HashMap<usize, LibraryTrackImageCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_TEXT_CELLS: RefCell<HashMap<usize, LibraryTrackTextCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_MERGED_CELLS: RefCell<HashMap<usize, LibraryTrackMergedCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_FAVORITE_CELLS: RefCell<HashMap<usize, LibraryTrackFavoriteCell>> = RefCell::new(HashMap::new());
}

fn library_list_item_storage_key(list_item: &gtk::ListItem) -> usize {
    list_item.as_ptr() as usize
}

fn track_image_cell(item: &gtk::ListItem) -> Option<LibraryTrackImageCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn track_text_cell(item: &gtk::ListItem) -> Option<LibraryTrackTextCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_TEXT_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn track_merged_cell(item: &gtk::ListItem) -> Option<LibraryTrackMergedCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_MERGED_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn track_favorite_cell(item: &gtk::ListItem) -> Option<LibraryTrackFavoriteCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_FAVORITE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn track_image_column(shell: &Rc<Shell>, title: &'static str, width: i32) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let cover = ArtworkTile::new(48, 0);
        let widget = cover.widget();
        install_dynamic_track_context_menu(&widget, &setup_shell, Rc::clone(&current_track));
        item.set_child(Some(&widget));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_IMAGE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackImageCell {
                    cover,
                    current_track,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let bind_started = shell.state.perf.as_ref().map(|_| Instant::now());
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(cell) = track_image_cell(item) else {
            return;
        };
        shell.bind_cover_tile_for(
            &cell.cover,
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );
        *cell.current_track.borrow_mut() = Some(track);
        if let Some(bind_started) = bind_started {
            shell.record_perf_track_row_bind(title, bind_started.elapsed());
        }
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_image_cell(item)
        {
            *cell.current_track.borrow_mut() = None;
            cell.cover.bind_image(0, None);
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_IMAGE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column
}
fn track_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    expand: bool,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_dynamic_track_context_menu(&label, &setup_shell, Rc::clone(&current_track));
        item.set_child(Some(&label));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_TEXT_CELLS.with(|cells| {
            cells
                .borrow_mut()
                .insert(key, LibraryTrackTextCell { label, current_track });
        });
    });

    factory.connect_bind(move |_, item| {
        let bind_started = shell.state.perf.as_ref().map(|_| Instant::now());
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(cell) = track_text_cell(item) else {
            return;
        };
        cell.label.set_text(&(value)(&track));
        *cell.current_track.borrow_mut() = Some(track);
        if let Some(bind_started) = bind_started {
            shell.record_perf_track_row_bind(title, bind_started.elapsed());
        }
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_text_cell(item)
        {
            cell.label.set_text("");
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_TEXT_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(expand);
    column
}
fn track_merged_column<Title, Subtitle, Image, Seed>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    title_value: Title,
    subtitle_value: Subtitle,
    image_ref: Image,
    seed: Seed,
) -> gtk::ColumnViewColumn
where
    Title: Fn(&Track) -> String + 'static,
    Subtitle: Fn(&Track) -> String + 'static,
    Image: Fn(&Track) -> Option<rufin_core::ImageRef> + 'static,
    Seed: Fn(&Track) -> u32 + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let title_value = Rc::new(title_value);
    let subtitle_value = Rc::new(subtitle_value);
    let image_ref = Rc::new(image_ref);
    let seed = Rc::new(seed);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_valign(gtk::Align::Center);

        let cover = ArtworkTile::new(48, 0);
        row.append(&cover.widget());

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_single_line_mode(true);
        labels.append(&title);

        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("muted");
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(false);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.set_single_line_mode(true);
        subtitle.set_visible(false);
        labels.append(&subtitle);

        row.append(&labels);
        install_dynamic_track_context_menu(&row, &setup_shell, Rc::clone(&current_track));
        item.set_child(Some(&row));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_MERGED_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackMergedCell {
                    cover,
                    title,
                    subtitle,
                    current_track,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let bind_started = shell.state.perf.as_ref().map(|_| Instant::now());
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(cell) = track_merged_cell(item) else {
            return;
        };
        shell.bind_cover_tile_for(
            &cell.cover,
            image_ref(&track).as_ref(),
            seed(&track),
            48,
            THUMB_COVER_SIZE,
        );
        cell.title.set_text(&title_value(&track));
        let subtitle = subtitle_value(&track);
        cell.subtitle.set_text(&subtitle);
        cell.subtitle.set_visible(!subtitle.trim().is_empty());
        *cell.current_track.borrow_mut() = Some(track);
        if let Some(bind_started) = bind_started {
            shell.record_perf_track_row_bind(title, bind_started.elapsed());
        }
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_merged_cell(item)
        {
            cell.title.set_text("");
            cell.subtitle.set_text("");
            cell.subtitle.set_visible(false);
            cell.cover.bind_image(0, None);
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_MERGED_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(true);
    column
}
fn album_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<Album>(item) else {
            return;
        };
        let button = favorite_icon_button("Favorite album");
        set_favorite_button_active(&button, album.favorite);
        install_album_context_menu(&button, &shell, album.clone());
        let controller = shell.controller.clone();
        button.connect_clicked(move |button| {
            controller.set_album_favorite(album.id.clone(), !favorite_button_is_active(button));
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}
fn artist_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(artist) = item_at_from_item::<Artist>(item) else {
            return;
        };
        let button = favorite_icon_button("Favorite artist");
        set_favorite_button_active(&button, artist.favorite);
        install_artist_context_menu(&button, &shell, artist.clone());
        let controller = shell.controller.clone();
        button.connect_clicked(move |button| {
            controller.set_artist_favorite(artist.id.clone(), !favorite_button_is_active(button));
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}
fn track_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let button = favorite_icon_button("Favorite track");
        install_dynamic_track_context_menu(&button, &setup_shell, Rc::clone(&current_track));
        let controller = setup_shell.controller.clone();
        let click_track = Rc::clone(&current_track);
        button.connect_clicked(move |button| {
            let Some(track) = click_track.borrow().as_ref().cloned() else {
                return;
            };
            controller.set_track_favorite(track.id, !favorite_button_is_active(button));
        });
        item.set_child(Some(&button));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_FAVORITE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackFavoriteCell {
                    button,
                    current_track,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let bind_started = shell.state.perf.as_ref().map(|_| Instant::now());
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(cell) = track_favorite_cell(item) else {
            return;
        };
        set_favorite_button_active(&cell.button, track.favorite);
        *cell.current_track.borrow_mut() = Some(track);
        if let Some(bind_started) = bind_started {
            shell.record_perf_track_row_bind("Favorite", bind_started.elapsed());
        }
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_favorite_cell(item)
        {
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_FAVORITE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}
fn populate_album_collection_model(
    model: &gio::ListStore,
    albums: &[Album],
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) {
    if settings.layout == LibraryLayout::Detail {
        replace_album_detail_items_in_model(
            model,
            album_detail_items_for(albums, settings, album_tracks),
        );
    } else {
        populate_album_model(model, albums, settings);
    }
}
fn append_album_collection_model(
    model: &gio::ListStore,
    albums: Vec<Album>,
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) {
    if settings.layout == LibraryLayout::Detail {
        append_album_detail_items_to_model(
            model,
            album_detail_items_for(&albums, settings, album_tracks),
        );
    } else {
        append_albums_to_model(model, albums);
    }
}
fn album_detail_items_for(
    albums: &[Album],
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) -> Vec<AlbumDetailItem> {
    let mut albums = albums.to_vec();
    sort_albums(&mut albums, settings);
    let mut rows = Vec::new();

    for album in albums {
        let mut tracks = album_tracks.get(&album.id).cloned().unwrap_or_default();
        sort_album_detail_tracks(&mut tracks);
        let inline_count = tracks.len().min(ALBUM_DETAIL_INLINE_TRACK_ROWS);
        let remaining_tracks = tracks.split_off(inline_count);
        rows.push(AlbumDetailItem::Lead {
            album,
            inline_tracks: tracks,
            last_in_album: remaining_tracks.is_empty(),
        });
        let remaining_count = remaining_tracks.len();
        for (offset, track) in remaining_tracks.into_iter().enumerate() {
            rows.push(AlbumDetailItem::Track {
                track,
                index: inline_count + offset,
                last_in_album: offset + 1 == remaining_count,
            });
        }
    }

    rows
}
fn populate_album_model(model: &gio::ListStore, albums: &[Album], settings: &LibraryListSettings) {
    let mut values = albums.to_vec();
    sort_albums(&mut values, settings);
    replace_albums_in_model(model, values);
}
fn populate_artist_model(
    model: &gio::ListStore,
    artists: &[Artist],
    settings: &LibraryListSettings,
) {
    let mut values = artists.to_vec();
    sort_artists(&mut values, settings);
    replace_artists_in_model(model, values);
}
fn populate_genre_model(model: &gio::ListStore, genres: &[Genre], settings: &LibraryListSettings) {
    let mut values = genres.to_vec();
    sort_genres(&mut values, settings);
    replace_genres_in_model(model, values);
}
fn populate_playlist_model(
    model: &gio::ListStore,
    playlists: &[Playlist],
    settings: &LibraryListSettings,
) {
    let mut values = playlists.to_vec();
    sort_playlists(&mut values, settings);
    replace_playlists_in_model(model, values);
}
fn populate_track_model_for_settings(
    model: &gio::ListStore,
    tracks: &[Track],
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) -> usize {
    let values = tracks_for_settings(tracks, settings, query, favorite_first);
    let visible_count = values.len();
    replace_tracks_in_model(model, values);
    visible_count
}
fn tracks_for_settings(
    tracks: &[Track],
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) -> Vec<Track> {
    let query = query.trim().to_lowercase();
    let mut values = tracks
        .iter()
        .filter(|track| query.is_empty() || track_matches_query(track, &query))
        .cloned()
        .collect::<Vec<_>>();
    sort_tracks(&mut values, settings, favorite_first);
    values
}
pub(super) fn sort_albums(albums: &mut [Album], settings: &LibraryListSettings) {
    albums.sort_by(|left, right| {
        let missing = album_field_missing(left, settings.sort_key)
            .cmp(&album_field_missing(right, settings.sort_key));
        if missing != Ordering::Equal {
            return missing;
        }
        apply_desc(
            compare_album(left, right, settings.sort_key),
            settings.descending,
        )
    });
}
pub(super) fn sort_artists(artists: &mut [Artist], settings: &LibraryListSettings) {
    artists.sort_by(|left, right| {
        let missing = artist_field_missing(left, settings.sort_key)
            .cmp(&artist_field_missing(right, settings.sort_key));
        if missing != Ordering::Equal {
            return missing;
        }
        apply_desc(
            compare_artist(left, right, settings.sort_key),
            settings.descending,
        )
    });
}
