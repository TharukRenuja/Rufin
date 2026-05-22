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
        LibraryField::Image => image_column::<Album, _, _>(
            shell,
            "Image",
            column_width(LibraryField::Image),
            |album| album.image_ref.clone(),
            |album| album.color_seed,
        ),
        LibraryField::TitleMerged => merged_column::<Album, _, _, _, _>(
            shell,
            "Title",
            column_width(LibraryField::TitleMerged),
            |album| album.title.clone(),
            |album| album.artist.clone(),
            |album| album.image_ref.clone(),
            |album| album.color_seed,
        ),
        LibraryField::Title => {
            expanding_text_column::<Album, _>("Title", 220, |album| album.title.clone())
        }
        LibraryField::Favorite => album_favorite_column(shell),
        _ => text_column::<Album, _>(field.title(), column_width(field), move |album| {
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
fn track_image_column(shell: &Rc<Shell>, title: &str, width: i32) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let cover = shell.cover_tile_for(
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );
        install_track_context_menu(&cover, &shell, track);
        item.set_child(Some(&cover));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column
}
fn track_text_column<F>(
    shell: &Rc<Shell>,
    title: &str,
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
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let label = gtk::Label::new(Some(&(value)(&track)));
        label.set_xalign(0.0);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_track_context_menu(&label, &shell, track);
        item.set_child(Some(&label));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(expand);
    column
}
fn merged_column<T, Title, Subtitle, Image, Seed>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    title_value: Title,
    subtitle_value: Subtitle,
    image_ref: Image,
    seed: Seed,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    Title: Fn(&T) -> String + 'static,
    Subtitle: Fn(&T) -> String + 'static,
    Image: Fn(&T) -> Option<rufin_core::ImageRef> + 'static,
    Seed: Fn(&T) -> u32 + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let title_value = Rc::new(title_value);
    let subtitle_value = Rc::new(subtitle_value);
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
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_valign(gtk::Align::Center);
        row.append(&shell.cover_tile_for(
            image_ref(&data).as_ref(),
            seed(&data),
            48,
            THUMB_COVER_SIZE,
        ));
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(&title_value(&data)));
        title.set_xalign(0.0);
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_single_line_mode(true);
        labels.append(&title);
        let subtitle = subtitle_value(&data);
        if !subtitle.trim().is_empty() {
            let subtitle = gtk::Label::new(Some(&subtitle));
            subtitle.add_css_class("muted");
            subtitle.set_xalign(0.0);
            subtitle.set_wrap(false);
            subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
            subtitle.set_single_line_mode(true);
            labels.append(&subtitle);
        }
        row.append(&labels);
        item.set_child(Some(&row));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(true);
    column
}
fn track_merged_column<Title, Subtitle, Image, Seed>(
    shell: &Rc<Shell>,
    title: &str,
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
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_valign(gtk::Align::Center);
        row.append(&shell.cover_tile_for(
            image_ref(&track).as_ref(),
            seed(&track),
            48,
            THUMB_COVER_SIZE,
        ));
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(&title_value(&track)));
        title.set_xalign(0.0);
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_single_line_mode(true);
        labels.append(&title);
        let subtitle = subtitle_value(&track);
        if !subtitle.trim().is_empty() {
            let subtitle = gtk::Label::new(Some(&subtitle));
            subtitle.add_css_class("muted");
            subtitle.set_xalign(0.0);
            subtitle.set_wrap(false);
            subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
            subtitle.set_single_line_mode(true);
            labels.append(&subtitle);
        }
        row.append(&labels);
        install_track_context_menu(&row, &shell, track);
        item.set_child(Some(&row));
    });
    factory.connect_unbind(clear_list_item_child);
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
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let button = favorite_icon_button("Favorite track");
        set_favorite_button_active(&button, track.favorite);
        install_track_context_menu(&button, &shell, track.clone());
        let controller = shell.controller.clone();
        button.connect_clicked(move |button| {
            controller.set_track_favorite(track.id.clone(), !favorite_button_is_active(button));
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
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
fn populate_track_model_for_settings_incremental(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    tracks: &[Track],
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
    generation: Rc<Cell<u64>>,
) -> usize {
    let mut values = tracks_for_settings(tracks, settings, query, favorite_first);
    let visible_count = values.len();
    if values.len() <= TRACK_INITIAL_COMPLETE_ROWS {
        replace_tracks_in_model(model, values);
        return visible_count;
    }

    let remaining = values.split_off(TRACK_INITIAL_COMPLETE_ROWS);
    replace_tracks_in_model(model, values);
    append_tracks_incrementally(Rc::clone(shell), model.clone(), remaining, generation);
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
fn append_tracks_incrementally(
    shell: Rc<Shell>,
    model: gio::ListStore,
    mut tracks: Vec<Track>,
    generation: Rc<Cell<u64>>,
) {
    if tracks.is_empty() {
        return;
    }

    let generation_id = generation.get();
    tracks.reverse();
    let tracks = Rc::new(RefCell::new(tracks));
    glib::idle_add_local(move || {
        if generation.get() != generation_id
            || !matches!(shell.state.routes.borrow().current(), &Route::Tracks)
        {
            return glib::ControlFlow::Break;
        }

        let mut chunk = Vec::with_capacity(TRACK_COMPLETE_APPEND_ROWS);
        {
            let mut tracks = tracks.borrow_mut();
            for _ in 0..TRACK_COMPLETE_APPEND_ROWS {
                let Some(track) = tracks.pop() else {
                    break;
                };
                chunk.push(track);
            }
        }
        append_tracks_to_model(&model, chunk);

        if tracks.borrow().is_empty() {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
fn sort_albums(albums: &mut [Album], settings: &LibraryListSettings) {
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
fn sort_artists(artists: &mut [Artist], settings: &LibraryListSettings) {
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
