#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LibraryRouteInsetSpec {
    margin_start: i32,
    margin_end: i32,
    hexpand: bool,
}
fn library_route_inset_spec() -> LibraryRouteInsetSpec {
    LibraryRouteInsetSpec {
        margin_start: PRIMARY_ROUTE_MARGIN_START,
        margin_end: 0,
        hexpand: true,
    }
}
fn library_route_inset(child: gtk::Widget) -> gtk::Widget {
    let spec = library_route_inset_spec();
    // this keeps the scrollbar at the pane edge while the actual
    // library content keeps the same visual inset.
    child.set_margin_start(spec.margin_start);
    child.set_margin_end(spec.margin_end);
    child.set_hexpand(spec.hexpand);
    child.set_halign(gtk::Align::Fill);
    child
}
fn album_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Row => album_table(shell, model, key).upcast(),
        LibraryLayout::Detail if key.supports_layout(LibraryLayout::Detail) => {
            album_detail_list(shell, model, key).upcast()
        }
        LibraryLayout::Grid | LibraryLayout::Detail => album_grid(shell, model, key).upcast(),
    }
}
fn artist_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Row => artist_table(shell, model, key).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => artist_grid(shell, model, key).upcast(),
    }
}
fn genre_collection_widget(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::Widget {
    match shell.library_settings(LibraryListKey::Genres).layout {
        LibraryLayout::Row => genre_table(shell, model).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => genre_grid(shell, model).upcast(),
    }
}
fn playlist_collection_widget(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::Widget {
    match shell.library_settings(LibraryListKey::Playlists).layout {
        LibraryLayout::Row => playlist_table(shell, model).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => playlist_grid(shell, model).upcast(),
    }
}
fn track_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Grid => track_grid(shell, model, key).upcast(),
        LibraryLayout::Row | LibraryLayout::Detail => {
            track_table(shell, model, key, false).upcast()
        }
    }
}
fn album_grid(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
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
        let album = boxed.borrow::<Album>();
        item.set_child(Some(&album_card(
            &shell_for_factory,
            &album,
            key,
            card_size,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let shell = Rc::clone(shell);
    grid.connect_activate(move |_, position| {
        if let Some(album) = item_at::<Album>(&model, position) {
            shell.navigate(Route::AlbumDetail(album.id));
        }
    });
    grid
}
fn artist_grid(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
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
        let artist = boxed.borrow::<Artist>();
        item.set_child(Some(&artist_card(
            &shell_for_factory,
            &artist,
            key,
            card_size,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let shell = Rc::clone(shell);
    grid.connect_activate(move |_, position| {
        if let Some(artist) = item_at::<Artist>(&model, position) {
            shell.navigate(Route::ArtistDetail(artist.id));
        }
    });
    grid
}
fn genre_grid(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
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
        let genre = boxed.borrow::<Genre>();
        item.set_child(Some(&genre_card(&shell_for_factory, &genre, card_size)));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let shell = Rc::clone(shell);
    grid.connect_activate(move |_, position| {
        if let Some(genre) = item_at::<Genre>(&model, position) {
            shell.navigate(Route::GenreDetail(genre.id));
        }
    });
    grid
}
fn playlist_grid(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
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
        let playlist = boxed.borrow::<Playlist>();
        item.set_child(Some(&playlist_card(
            &shell_for_factory,
            &playlist,
            card_size,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let shell = Rc::clone(shell);
    grid.connect_activate(move |_, position| {
        if let Some(playlist) = item_at::<Playlist>(&model, position) {
            shell.navigate(Route::PlaylistDetail(playlist.id));
        }
    });
    grid
}
fn track_grid(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
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
        let track = boxed.borrow::<Track>();
        item.set_child(Some(&track_card(
            &shell_for_factory,
            &track,
            key,
            card_size,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let controller = shell.controller.clone();
    grid.connect_activate(move |_, position| {
        if let Some(track) = item_at::<Track>(&model, position) {
            controller.play_now(track);
        }
    });
    grid
}
fn album_table(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    for field in shell.library_settings(key).row_fields {
        table.append_column(&album_column(shell, field));
    }
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(album) = item_at::<Album>(&model, position) {
            shell.navigate(Route::AlbumDetail(album.id));
        }
    });
    table
}
fn artist_table(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    for field in shell.library_settings(key).row_fields {
        table.append_column(&artist_column(shell, field));
    }
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(artist) = item_at::<Artist>(&model, position) {
            shell.navigate(Route::ArtistDetail(artist.id));
        }
    });
    table
}
fn genre_table(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    for field in shell.library_settings(LibraryListKey::Genres).row_fields {
        table.append_column(&genre_column(field));
    }
    table
}
fn playlist_table(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_single_click_activate(true);
    table.set_hexpand(true);
    table.set_vexpand(true);
    for field in shell.library_settings(LibraryListKey::Playlists).row_fields {
        table.append_column(&playlist_column(shell, field));
    }
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(playlist) = item_at::<Playlist>(&model, position) {
            shell.navigate(Route::PlaylistDetail(playlist.id));
        }
    });
    table
}
fn track_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    detail: bool,
) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = if detail {
        shell.library_settings(key).detail_track_fields
    } else {
        shell.library_settings(key).row_fields
    };
    for field in fields {
        table.append_column(&track_column(shell, field));
    }
    let controller = shell.controller.clone();
    table.connect_activate(move |_, position| {
        if let Some(track) = item_at::<Track>(&model, position) {
            controller.play_now(track);
        }
    });
    table
}
fn album_detail_list(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::ListView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<AlbumDetailItem>(item) else {
            return;
        };
        let content = album_detail_item_row(&shell_for_factory, row, key);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        item.set_child(Some(&content));
    });
    factory.connect_unbind(clear_list_item_child);

    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("track-table");
    list.add_css_class("album-detail-list");
    list.set_hexpand(true);
    list.set_halign(gtk::Align::Fill);
    list.set_vexpand(true);
    let controller = shell.controller.clone();
    list.connect_activate(move |_, position| {
        let Some(AlbumDetailItem::Track { track, .. }) =
            item_at::<AlbumDetailItem>(&model, position)
        else {
            return;
        };
        controller.play_now(track);
    });
    list
}
#[derive(Clone, Debug, PartialEq)]
enum AlbumDetailItem {
    Lead {
        album: Album,
        inline_tracks: Vec<Track>,
        last_in_album: bool,
    },
    Track {
        track: Track,
        index: usize,
        last_in_album: bool,
    },
}
fn album_detail_item_row(
    shell: &Rc<Shell>,
    row: AlbumDetailItem,
    key: LibraryListKey,
) -> gtk::Widget {
    match row {
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album,
        } => album_detail_lead_row(shell, &album, &inline_tracks, last_in_album, key),
        AlbumDetailItem::Track {
            track,
            index,
            last_in_album,
        } => album_detail_track_row(shell, &track, index, last_in_album, key),
    }
}
fn album_detail_lead_row(
    shell: &Rc<Shell>,
    album: &Album,
    inline_tracks: &[Track],
    last_in_album: bool,
    key: LibraryListKey,
) -> gtk::Widget {
    let compact = compact_detail_layout(shell);
    let (cover_size, meta_width, spacing) = if compact {
        (148, 168, 14)
    } else {
        (220, 240, 24)
    };
    let row = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
    row.add_css_class("album-detail-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_margin_top(12);
    row.set_margin_bottom(if last_in_album { 16 } else { 0 });
    row.set_margin_start(4);
    row.set_margin_end(4);

    row.append(&album_detail_meta(shell, album, cover_size, meta_width));

    let track_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    track_area.set_hexpand(true);
    track_area.set_halign(gtk::Align::Fill);
    if !inline_tracks.is_empty() {
        let fields = shell.library_settings(key).detail_track_fields;
        track_area.append(&album_detail_track_header(&fields));
        for (index, track) in inline_tracks.iter().enumerate() {
            track_area.append(&album_detail_track_cells(shell, track, index, &fields));
        }
    }
    row.append(&track_area);
    row.upcast()
}
fn album_detail_track_row(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    last_in_album: bool,
    key: LibraryListKey,
) -> gtk::Widget {
    let compact = compact_detail_layout(shell);
    let (_cover_size, meta_width, spacing) = if compact {
        (148, 168, 14)
    } else {
        (220, 240, 24)
    };
    let row = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
    row.add_css_class("album-detail-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_margin_bottom(if last_in_album { 16 } else { 0 });
    row.set_margin_start(4);
    row.set_margin_end(4);

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_width_request(meta_width);
    spacer.set_hexpand(false);
    row.append(&spacer);

    let fields = shell.library_settings(key).detail_track_fields;
    row.append(&album_detail_track_cells(shell, track, index, &fields));
    row.upcast()
}
fn album_detail_meta(
    shell: &Rc<Shell>,
    album: &Album,
    cover_size: i32,
    meta_width: i32,
) -> gtk::Widget {
    let meta = gtk::Box::new(gtk::Orientation::Vertical, 6);
    meta.set_width_request(meta_width);
    meta.set_hexpand(false);
    meta.append(&album_detail_cover_tile(shell, album, cover_size));
    meta.append(&album_detail_meta_label(
        &album.title,
        "track-title",
        meta_width,
    ));
    meta.append(&album_detail_meta_label(&album.artist, "muted", meta_width));
    meta.append(&album_detail_meta_label(
        &album_fact_text(album),
        "muted",
        meta_width,
    ));
    if !album.genres.is_empty() {
        meta.append(&album_detail_meta_label(
            &album.genres.join(", "),
            "muted",
            meta_width,
        ));
    }
    meta.upcast()
}
fn album_detail_cover_tile(shell: &Rc<Shell>, album: &Album, cover_size: i32) -> gtk::Widget {
    let overlay = cards::cover_overlay(cover_size);
    overlay.set_child(Some(&shell.cover_tile_for(
        album.image_ref.as_ref(),
        album.color_seed,
        cover_size,
        GRID_COVER_SIZE,
    )));

    let controls = cards::cover_hover_controls(cover_size, "Play album", album.favorite);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    controls
        .play
        .connect_clicked(move |_| controller.play_album_now(album_id.clone()));

    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });

    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            controller.play_last(tracks);
        }
    });

    let favorite = controls.favorite.as_ref().expect("favorite button");
    shell.register_favorite_button(album_favorite_key(&album.id), favorite);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    favorite.connect_clicked(move |button| {
        controller.set_album_favorite(album_id.clone(), !favorite_button_is_active(button));
    });

    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);
    install_album_context_menu(&overlay, shell, album.clone());
    overlay.upcast()
}
fn album_detail_track_header(fields: &[LibraryField]) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("album-detail-track-cells");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_height_request(34);
    row.set_margin_end(album_detail_track_trailing_inset(fields));
    for field in fields {
        let label = album_detail_track_label(&tr(field.title()), *field, false);
        label.add_css_class("muted");
        row.append(&label);
    }
    row.upcast()
}
fn album_detail_track_cells(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    fields: &[LibraryField],
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_height_request(LIBRARY_TABLE_ROW_HEIGHT);
    row.set_valign(gtk::Align::Center);
    row.set_margin_end(album_detail_track_trailing_inset(fields));
    for field in fields {
        row.append(&album_detail_track_cell(shell, track, index, *field));
    }
    install_track_context_menu(&row, shell, track.clone());

    let controller = shell.controller.clone();
    let track = track.clone();
    let gesture = gtk::GestureClick::new();
    gesture.connect_released(move |_, n_press, _, _| {
        if n_press == 2 {
            controller.play_now(track.clone());
        }
    });
    row.add_controller(gesture);
    row.upcast()
}
fn album_detail_track_cell(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    field: LibraryField,
) -> gtk::Widget {
    match field {
        LibraryField::Favorite => {
            let button = favorite_icon_button("Favorite track");
            set_favorite_button_active(&button, track.favorite);
            let controller = shell.controller.clone();
            let track_id = track.id.clone();
            button.connect_clicked(move |button| {
                controller.set_track_favorite(track_id.clone(), !favorite_button_is_active(button));
            });
            button.set_width_request(column_width(field));
            button.upcast()
        }
        LibraryField::Image => shell.cover_tile_for(
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        ),
        _ => album_detail_track_label(&album_detail_track_text(track, index, field), field, true)
            .upcast(),
    }
}
fn album_detail_track_label(text: &str, field: LibraryField, ellipsize: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(if field == LibraryField::Duration {
        1.0
    } else {
        0.0
    });
    label.set_wrap(false);
    label.set_single_line_mode(true);
    label.set_hexpand(album_detail_track_field_expands(field));
    if !album_detail_track_field_expands(field) {
        label.set_width_request(column_width(field));
    }
    if ellipsize {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    label
}
fn album_detail_track_field_expands(field: LibraryField) -> bool {
    matches!(
        field,
        LibraryField::Title
            | LibraryField::TitleMerged
            | LibraryField::Album
            | LibraryField::Artist
            | LibraryField::AlbumArtist
            | LibraryField::Genre
    )
}
fn album_detail_track_trailing_inset(fields: &[LibraryField]) -> i32 {
    if fields
        .last()
        .is_some_and(|field| !album_detail_track_field_expands(*field))
    {
        ALBUM_DETAIL_FIXED_TRAILING_INSET
    } else {
        0
    }
}
fn album_detail_track_text(track: &Track, index: usize, field: LibraryField) -> String {
    if field == LibraryField::RowIndex {
        (index + 1).to_string()
    } else {
        track_field(track, field)
    }
}
fn sort_album_detail_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|left, right| compare_track(left, right, LibraryField::TrackNumber));
}
fn set_library_table_content_height(scroller: &gtk::ScrolledWindow, row_count: usize) {
    let height = library_table_content_height(row_count);
    scroller.set_min_content_height(height);
    scroller.set_max_content_height(height);
}
fn library_table_content_height(row_count: usize) -> i32 {
    let max_rows = ((i32::MAX - LIBRARY_TABLE_HEADER_HEIGHT) / LIBRARY_TABLE_ROW_HEIGHT) as usize;
    let visible_rows = row_count.max(1).min(max_rows);
    LIBRARY_TABLE_HEADER_HEIGHT + visible_rows as i32 * LIBRARY_TABLE_ROW_HEIGHT
}
fn compact_detail_layout(shell: &Shell) -> bool {
    route_content_width(shell) < 760
}
fn album_card(shell: &Rc<Shell>, album: &Album, key: LibraryListKey, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&cards::album_cover_tile(
        shell,
        album,
        size,
        Some(&shell.controller),
    ));
    card.append(&center_label(&album.title, "track-title"));
    for field in shell.library_settings(key).grid_fields {
        let value = album_field(album, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    install_album_context_menu(&card, shell, album.clone());
    card.upcast()
}
fn artist_card(shell: &Rc<Shell>, artist: &Artist, key: LibraryListKey, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&artist_cover_tile(shell, artist, size));
    card.append(&center_label(&artist.name, "track-title"));
    for field in shell.library_settings(key).grid_fields {
        let value = artist_field(artist, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    install_artist_context_menu(&card, shell, artist.clone());
    card.upcast()
}
fn genre_card(shell: &Rc<Shell>, genre: &Genre, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&genre_cover_tile(shell, genre, size));
    card.append(&center_label(&genre.name, "track-title"));
    for field in shell.library_settings(LibraryListKey::Genres).grid_fields {
        let value = genre_field(genre, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    card.upcast()
}
fn playlist_card(shell: &Rc<Shell>, playlist: &Playlist, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&cards::playlist_cover_tile(shell, playlist, size));
    card.append(&center_label(&playlist.name, "track-title"));
    for field in shell
        .library_settings(LibraryListKey::Playlists)
        .grid_fields
    {
        let value = playlist_field(playlist, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    card.upcast()
}
fn track_card(shell: &Rc<Shell>, track: &Track, key: LibraryListKey, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&cards::track_cover_tile(shell, track, size));
    card.append(&center_label(&track.title, "track-title"));
    for field in shell.library_settings(key).grid_fields {
        let value = track_field(track, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    install_track_context_menu(&card, shell, track.clone());
    card.upcast()
}
fn artist_cover_tile(shell: &Rc<Shell>, artist: &Artist, size: i32) -> gtk::Widget {
    let overlay = cards::cover_overlay(size);

    let artist_button = gtk::Button::new();
    artist_button.add_css_class("album-cover-button");
    artist_button.add_css_class("flat");
    cards::constrain_cover_widget(&artist_button, size);
    let image_ref = artist_cover_image_ref(shell, artist);
    artist_button.set_child(Some(&shell.cover_tile_for(
        image_ref.as_ref(),
        stable_seed(artist.id.as_str()),
        size,
        GRID_COVER_SIZE,
    )));
    let open_shell = Rc::clone(shell);
    let open_artist_id = artist.id.clone();
    artist_button
        .connect_clicked(move |_| open_shell.navigate(Route::ArtistDetail(open_artist_id.clone())));
    overlay.set_child(Some(&artist_button));

    let controls = cards::cover_hover_controls(size, "Play artist", artist.favorite);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    controls.play.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_artist_detail(&artist_id) {
            controller.play_tracks_now(detail.tracks);
        }
    });
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_artist_detail(&artist_id) {
            for track in detail.tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_artist_detail(&artist_id) {
            controller.play_last(detail.tracks);
        }
    });
    let favorite = controls.favorite.as_ref().expect("favorite button");
    shell.register_favorite_button(artist_favorite_key(&artist.id), favorite);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    favorite.connect_clicked(move |button| {
        controller.set_artist_favorite(artist_id.clone(), !favorite_button_is_active(button));
    });
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

    overlay.upcast()
}
fn artist_cover_image_ref(shell: &Rc<Shell>, artist: &Artist) -> Option<ImageRef> {
    artist.image_ref.clone().or_else(|| {
        shell
            .controller
            .cached_artist_detail(&artist.id)
            .ok()
            .flatten()
            .and_then(|detail| detail.artist.image_ref)
    })
}
