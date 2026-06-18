use super::*;

pub(in crate::ui) fn album_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => {
            album_image_column(shell, "Image", column_width(LibraryField::Image))
        }
        LibraryField::TitleMerged => {
            album_merged_column(shell, "Title", column_width(LibraryField::TitleMerged))
        }
        LibraryField::Title => {
            album_text_column(shell, "Title", 220, true, |album| album.title.clone())
        }
        LibraryField::Favorite => album_favorite_column(shell),
        _ => album_text_column(
            shell,
            field.title(),
            column_width(field),
            false,
            move |album| album_field(album, field),
        ),
    }
}
pub(in crate::ui) fn artist_column(
    shell: &Rc<Shell>,
    field: LibraryField,
) -> gtk::ColumnViewColumn {
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
pub(in crate::ui) fn genre_column(field: LibraryField) -> gtk::ColumnViewColumn {
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
pub(in crate::ui) fn playlist_column(
    shell: &Rc<Shell>,
    field: LibraryField,
) -> gtk::ColumnViewColumn {
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
pub(in crate::ui) fn smart_playlist_column(
    shell: &Rc<Shell>,
    field: LibraryField,
) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => image_column::<SmartPlaylist, _, _>(
            shell,
            "Image",
            column_width(LibraryField::Image),
            |playlist| playlist.image_ref.clone(),
            |playlist| stable_seed(playlist.id.as_str()),
        ),
        LibraryField::Title | LibraryField::TitleMerged => {
            expanding_text_column::<SmartPlaylist, _>("Title", 220, |playlist| {
                smart_playlist_display_name(playlist)
            })
        }
        _ => text_column::<SmartPlaylist, _>(field.title(), column_width(field), move |playlist| {
            smart_playlist_field(playlist, field)
        }),
    }
}
pub(in crate::ui) fn track_column_for_key(
    shell: &Rc<Shell>,
    key: LibraryListKey,
    field: LibraryField,
) -> gtk::ColumnViewColumn {
    let width = track_column_width(key, field);
    match field {
        LibraryField::RowIndex => row_index_column_with_width(width),
        LibraryField::Image => track_image_column(shell, "Image", width),
        LibraryField::TitleMerged => track_merged_column(
            shell,
            "Title",
            width,
            |track| track.title.clone(),
            |track| track.artist.clone(),
            |track| track.image_ref.clone(),
            |track| stable_seed(track.id.as_str()),
        ),
        LibraryField::Title => track_text_column(shell, "Title", width, true, 0.0, |track| {
            track.title.clone()
        }),
        LibraryField::Favorite => track_favorite_column(shell),
        LibraryField::Artist => track_link_column(shell, "Artist", width, |track| {
            (track.artist.clone(), track_artist_route(track))
        }),
        LibraryField::AlbumArtist => {
            track_link_column(shell, LibraryField::AlbumArtist.title(), width, |track| {
                (
                    track_field(track, LibraryField::AlbumArtist),
                    track_album_artist_route(track),
                )
            })
        }
        LibraryField::Album => track_link_column(shell, "Album", width, |track| {
            (
                track.album.clone(),
                Some(Route::AlbumDetail(track.album_id.clone())),
            )
        }),
        LibraryField::Duration => track_text_column(shell, "◷", width, false, 0.0, |track| {
            track_field(track, LibraryField::Duration)
        }),
        _ => track_text_column(shell, field.title(), width, false, 0.0, move |track| {
            track_field(track, field)
        }),
    }
}
pub(in crate::ui) fn track_column_fit_width(key: LibraryListKey, field: LibraryField) -> i32 {
    column_fit_width(field, track_column_width(key, field))
}
pub(in crate::ui) fn track_column_width(key: LibraryListKey, field: LibraryField) -> i32 {
    match key {
        LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => return track_list_column_width(field),
        LibraryListKey::SmartPlaylistTracks => {}
        _ => return column_width(field),
    }

    match field {
        LibraryField::RowIndex => 44,
        LibraryField::Title | LibraryField::TitleMerged => 212,
        LibraryField::Album
        | LibraryField::Artist
        | LibraryField::AlbumArtist
        | LibraryField::Genre => 180,
        LibraryField::PlayCount => play_count_column_width(),
        LibraryField::UserRating | LibraryField::SongCount | LibraryField::AlbumCount => 82,
        LibraryField::ReleaseDate | LibraryField::DateAdded | LibraryField::LastPlayed => 108,
        LibraryField::Year | LibraryField::DiscNumber | LibraryField::TrackNumber => 62,
        LibraryField::Duration => 70,
        LibraryField::Image => column_width(LibraryField::Image),
        LibraryField::Favorite => 48,
    }
}
pub(in crate::ui) fn column_fit_width(field: LibraryField, width: i32) -> i32 {
    if field == LibraryField::TitleMerged {
        width.saturating_add(72)
    } else {
        width
    }
}
fn track_album_artist_route(track: &Track) -> Option<Route> {
    if let Some(credit) = track.album_artist_credits.first() {
        Some(Route::ArtistDetail(credit.id.clone()))
    } else {
        let album_artist = track_field(track, LibraryField::AlbumArtist);
        (!album_artist.trim().is_empty()).then_some(Route::Search {
            query: album_artist,
            kind: SearchKind::Artists,
        })
    }
}
fn track_list_column_width(field: LibraryField) -> i32 {
    match field {
        LibraryField::RowIndex => 54,
        LibraryField::Title | LibraryField::TitleMerged => 320,
        LibraryField::Album => 260,
        LibraryField::Artist | LibraryField::AlbumArtist | LibraryField::Genre => 220,
        LibraryField::Year | LibraryField::DiscNumber | LibraryField::TrackNumber => 70,
        LibraryField::Duration => 90,
        LibraryField::Favorite => 76,
        _ => column_width(field),
    }
}
pub(in crate::ui) fn text_column<T, F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> String + 'static,
{
    text_column_with_expand(title, width, false, value)
}
pub(in crate::ui) fn expanding_text_column<T, F>(
    title: &str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> String + 'static,
{
    text_column_with_expand(title, width, true, value)
}
pub(in crate::ui) fn text_column_with_expand<T, F>(
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
            label.set_halign(gtk::Align::Fill);
            label.set_hexpand(true);
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
pub(in crate::ui) fn row_index_column() -> gtk::ColumnViewColumn {
    row_index_column_with_width(column_width(LibraryField::RowIndex))
}
pub(in crate::ui) fn row_index_column_with_width(width: i32) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.add_css_class("muted");
            label.set_xalign(0.0);
            label.set_halign(gtk::Align::Fill);
            label.set_hexpand(true);
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
    column.set_fixed_width(width);
    column
}
#[derive(Clone)]
pub(in crate::ui) struct LibraryAlbumImageCell {
    pub(in crate::ui) cover: ArtworkTile,
    pub(in crate::ui) current_album: Rc<RefCell<Option<Album>>>,
}

#[derive(Clone)]
pub(in crate::ui) struct LibraryAlbumTextCell {
    pub(in crate::ui) label: gtk::Label,
    pub(in crate::ui) current_album: Rc<RefCell<Option<Album>>>,
}

#[derive(Clone)]
pub(in crate::ui) struct LibraryAlbumMergedCell {
    pub(in crate::ui) cover: ArtworkTile,
    pub(in crate::ui) title: gtk::Label,
    pub(in crate::ui) subtitle: gtk::Label,
    pub(in crate::ui) subtitle_route: Rc<RefCell<Option<Route>>>,
    pub(in crate::ui) current_album: Rc<RefCell<Option<Album>>>,
}

thread_local! {
    static LIBRARY_ALBUM_IMAGE_CELLS: RefCell<HashMap<usize, LibraryAlbumImageCell>> = RefCell::new(HashMap::new());
    static LIBRARY_ALBUM_TEXT_CELLS: RefCell<HashMap<usize, LibraryAlbumTextCell>> = RefCell::new(HashMap::new());
    static LIBRARY_ALBUM_MERGED_CELLS: RefCell<HashMap<usize, LibraryAlbumMergedCell>> = RefCell::new(HashMap::new());
}

pub(in crate::ui) fn album_image_cell(item: &gtk::ListItem) -> Option<LibraryAlbumImageCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn album_text_cell(item: &gtk::ListItem) -> Option<LibraryAlbumTextCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_TEXT_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn album_merged_cell(item: &gtk::ListItem) -> Option<LibraryAlbumMergedCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_MERGED_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn album_image_column(
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

pub(in crate::ui) fn album_text_column<F>(
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
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_dynamic_album_context_menu(&label, &setup_shell, Rc::clone(&current_album));
        item.set_child(Some(&label));
        let key = library_list_item_storage_key(item);
        LIBRARY_ALBUM_TEXT_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryAlbumTextCell {
                    label,
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

pub(in crate::ui) fn album_merged_column(
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
        let subtitle_route = Rc::new(RefCell::new(None::<Route>));
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
        subtitle.add_css_class("artist-label");
        subtitle.set_xalign(0.0);
        subtitle.set_halign(gtk::Align::Start);
        subtitle.set_hexpand(false);
        subtitle.set_wrap(false);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.set_single_line_mode(true);
        subtitle.set_visible(false);
        subtitle.set_cursor_from_name(Some("pointer"));
        add_dynamic_link_hover(subtitle.upcast_ref(), &subtitle);
        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&subtitle_route);
        add_label_click(&subtitle, move || {
            let route = route_for_click.borrow().clone();
            if let Some(route) = route {
                click_shell.navigate(route);
            }
        });
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
                    subtitle_route,
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
        let route = album_artist_route(&album);
        *cell.subtitle_route.borrow_mut() = route;
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
            *cell.subtitle_route.borrow_mut() = None;
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

pub(in crate::ui) fn image_column<T, F, S>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    image_ref: F,
    seed: S,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> Option<domain::ImageRef> + 'static,
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
pub(in crate::ui) fn artist_image_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
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
pub(in crate::ui) fn artist_text_column<F>(
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
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
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
pub(in crate::ui) struct LibraryTrackImageCell {
    pub(in crate::ui) cover: ArtworkTile,
    pub(in crate::ui) current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
pub(in crate::ui) struct LibraryTrackTextCell {
    pub(in crate::ui) label: gtk::Label,
    pub(in crate::ui) current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
pub(in crate::ui) struct LibraryTrackMergedCell {
    pub(in crate::ui) cover: ArtworkTile,
    pub(in crate::ui) title: gtk::Label,
    pub(in crate::ui) subtitle: gtk::Label,
    pub(in crate::ui) subtitle_route: Rc<RefCell<Option<Route>>>,
    pub(in crate::ui) current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
pub(in crate::ui) struct LibraryTrackFavoriteCell {
    pub(in crate::ui) button: gtk::Button,
    pub(in crate::ui) current_track: Rc<RefCell<Option<Track>>>,
}

thread_local! {
    static LIBRARY_TRACK_IMAGE_CELLS: RefCell<HashMap<usize, LibraryTrackImageCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_TEXT_CELLS: RefCell<HashMap<usize, LibraryTrackTextCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_MERGED_CELLS: RefCell<HashMap<usize, LibraryTrackMergedCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_FAVORITE_CELLS: RefCell<HashMap<usize, LibraryTrackFavoriteCell>> = RefCell::new(HashMap::new());
}

pub(in crate::ui) fn library_list_item_storage_key(list_item: &gtk::ListItem) -> usize {
    list_item.as_ptr() as usize
}

pub(in crate::ui) fn track_image_cell(item: &gtk::ListItem) -> Option<LibraryTrackImageCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn track_text_cell(item: &gtk::ListItem) -> Option<LibraryTrackTextCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_TEXT_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn track_merged_cell(item: &gtk::ListItem) -> Option<LibraryTrackMergedCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_MERGED_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn track_favorite_cell(item: &gtk::ListItem) -> Option<LibraryTrackFavoriteCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_FAVORITE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn track_image_column(
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
pub(in crate::ui) fn track_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    expand: bool,
    xalign: f32,
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
        label.set_xalign(xalign);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_dynamic_track_context_menu(&label, &setup_shell, Rc::clone(&current_track));
        item.set_child(Some(&label));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_TEXT_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackTextCell {
                    label,
                    current_track,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
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

pub(in crate::ui) fn track_merged_column<Title, Subtitle, Image, Seed>(
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
    Image: Fn(&Track) -> Option<domain::ImageRef> + 'static,
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
        let subtitle_route = Rc::new(RefCell::new(None::<Route>));
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
        subtitle.add_css_class("artist-label");
        subtitle.add_css_class("table-link-label");
        subtitle.set_xalign(0.0);
        subtitle.set_halign(gtk::Align::Start);
        subtitle.set_hexpand(false);
        subtitle.set_wrap(false);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.set_single_line_mode(true);
        subtitle.set_width_chars(1);
        subtitle.set_max_width_chars(28);
        subtitle.set_visible(false);
        subtitle.set_cursor_from_name(Some("pointer"));
        add_dynamic_link_hover(subtitle.upcast_ref(), &subtitle);
        labels.append(&subtitle);

        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&subtitle_route);
        add_label_click(&subtitle, move || {
            let route = route_for_click.borrow().clone();
            if let Some(route) = route {
                click_shell.navigate(route);
            }
        });

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
                    subtitle_route,
                    current_track,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
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
        let subtitle_route = track_artist_route(&track);
        *cell.current_track.borrow_mut() = Some(track);
        if subtitle.trim().is_empty() {
            *cell.subtitle_route.borrow_mut() = None;
            cell.subtitle.set_visible(false);
        } else if let Some(route) = subtitle_route {
            *cell.subtitle_route.borrow_mut() = Some(route);
            cell.subtitle.set_text(&subtitle);
            cell.subtitle.set_visible(true);
        } else {
            *cell.subtitle_route.borrow_mut() = None;
            cell.subtitle.set_text(&subtitle);
            cell.subtitle.set_visible(true);
        }
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_merged_cell(item)
        {
            cell.title.set_text("");
            cell.subtitle.set_text("");
            cell.subtitle.set_visible(false);
            *cell.subtitle_route.borrow_mut() = None;
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
pub(in crate::ui) fn album_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
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
        let favorite_shell = Rc::clone(&shell);
        button.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                source::FavoriteItemId::Album(album.id.clone()),
                favorite,
                Some(button),
            );
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}
pub(in crate::ui) fn artist_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
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
        let favorite_shell = Rc::clone(&shell);
        button.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                source::FavoriteItemId::Artist(artist.id.clone()),
                favorite,
                Some(button),
            );
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}
pub(in crate::ui) fn track_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
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
        let favorite_shell = Rc::clone(&setup_shell);
        let click_track = Rc::clone(&current_track);
        button.connect_clicked(move |button| {
            let Some(track) = click_track.borrow().as_ref().cloned() else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                source::FavoriteItemId::Track(track.id),
                favorite,
                Some(button),
            );
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
