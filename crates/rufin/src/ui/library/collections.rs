use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct LibraryRouteInsetSpec {
    pub(in crate::ui) margin_start: i32,
    pub(in crate::ui) margin_end: i32,
    pub(in crate::ui) hexpand: bool,
}
const SMART_PLAYLIST_REORDER_WIDTH: i32 = 30;

pub(in crate::ui) fn library_route_inset_spec() -> LibraryRouteInsetSpec {
    LibraryRouteInsetSpec {
        margin_start: PRIMARY_ROUTE_MARGIN_START,
        margin_end: 0,
        hexpand: true,
    }
}
pub(in crate::ui) fn library_route_inset(child: gtk::Widget) -> gtk::Widget {
    let spec = library_route_inset_spec();
    // this keeps the scrollbar at the pane edge while the actual
    // library content keeps the same visual inset.
    child.set_margin_start(spec.margin_start);
    child.set_margin_end(spec.margin_end);
    child.set_hexpand(spec.hexpand);
    child.set_halign(gtk::Align::Fill);
    child
}
pub(in crate::ui) fn configure_library_route_scroller(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
) {
    scroller.add_css_class("library-route-scroller");
    configure_fill_width_clip(scroller, gtk::PolicyType::Always);
    scroller.set_propagate_natural_height(false);
    scroller.set_overlay_scrolling(false);
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);

    let adjustment_shell = Rc::clone(shell);
    scroller.vadjustment().connect_value_changed(move |_| {
        adjustment_shell.pause_cover_warm_for_interaction();
    });
}

pub(in crate::ui) fn album_collection_widget(
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
pub(in crate::ui) fn artist_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Row => artist_table(shell, model, key).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => artist_grid(shell, model, key).upcast(),
    }
}
pub(in crate::ui) fn genre_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> gtk::Widget {
    match shell.library_settings(LibraryListKey::Genres).layout {
        LibraryLayout::Row => genre_table(shell, model).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => genre_grid(shell, model).upcast(),
    }
}
pub(in crate::ui) fn playlist_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> gtk::Widget {
    match shell.library_settings(LibraryListKey::Playlists).layout {
        LibraryLayout::Row => playlist_table(shell, model).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => playlist_grid(shell, model).upcast(),
    }
}
pub(in crate::ui) fn smart_playlist_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> gtk::Widget {
    match shell
        .library_settings(LibraryListKey::SmartPlaylists)
        .layout
    {
        LibraryLayout::Row => smart_playlist_table(shell, model).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => smart_playlist_grid(shell, model).upcast(),
    }
}
pub(in crate::ui) fn track_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    play_context: Option<LoadedTrackPlayContext>,
    content_inset: i32,
    width_mode: ColumnViewWidthMode,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Grid => track_grid(shell, model, key, play_context).upcast(),
        LibraryLayout::Row | LibraryLayout::Detail => track_table(
            shell,
            model,
            key,
            false,
            play_context,
            content_inset,
            width_mode,
        )
        .upcast(),
    }
}
fn track_model_play_action(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    play_context: LoadedTrackPlayContext,
    preferred_position: Option<u32>,
    fallback_track: Track,
) -> Rc<dyn Fn()> {
    let controller = shell.controller.clone();
    let model = model.clone();
    Rc::new(move || {
        play_track_from_model(
            &controller,
            &model,
            Some(&play_context),
            preferred_position,
            fallback_track.clone(),
        );
    })
}
fn play_track_from_model(
    controller: &crate::controller::AppController,
    model: &gio::ListStore,
    play_context: Option<&LoadedTrackPlayContext>,
    preferred_position: Option<u32>,
    fallback_track: Track,
) {
    let Some(play_context) = play_context else {
        controller.play_now(fallback_track);
        return;
    };
    let anchor_index = preferred_position
        .and_then(|position| {
            let position = position as usize;
            item_at::<Track>(model, position as u32)
                .is_some_and(|track| track.id == fallback_track.id)
                .then_some(position)
        })
        .or_else(|| {
            (0..model.n_items()).find_map(|position| {
                item_at::<Track>(model, position)
                    .is_some_and(|track| track.id == fallback_track.id)
                    .then_some(position as usize)
            })
        });
    let Some(anchor_index) = anchor_index else {
        controller.play_now(fallback_track);
        return;
    };
    let played = play_context.play_window(
        controller,
        model.n_items() as usize,
        anchor_index,
        |index| item_at::<Track>(model, index as u32),
    );
    if !played {
        controller.play_now(fallback_track);
    }
}

pub(in crate::ui) fn album_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::GridView {
    let settings = shell.library_settings(key);
    let (columns, card_size) = shell.collection_card_grid_metrics_for(key, &settings);
    let selection = gtk::NoSelection::new(Some(model.clone()));
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
pub(in crate::ui) fn artist_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::GridView {
    let (columns, card_size) = shell.collection_card_grid_metrics();
    let selection = gtk::NoSelection::new(Some(model.clone()));
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
pub(in crate::ui) fn genre_grid(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::GridView {
    let (columns, card_size) = shell.collection_card_grid_metrics();
    let selection = gtk::NoSelection::new(Some(model.clone()));
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
pub(in crate::ui) fn playlist_grid(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::GridView {
    let (columns, card_size) = shell.collection_card_grid_metrics();
    let selection = gtk::NoSelection::new(Some(model.clone()));
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
pub(in crate::ui) fn smart_playlist_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> gtk::GridView {
    let (columns, card_size) = shell.collection_card_grid_metrics();
    let selection = gtk::NoSelection::new(Some(model.clone()));
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
        let playlist = boxed.borrow::<SmartPlaylist>();
        item.set_child(Some(&smart_playlist_card(
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
        if let Some(playlist) = item_at::<SmartPlaylist>(&model, position) {
            shell.navigate(Route::SmartPlaylistDetail(playlist.id));
        }
    });
    grid
}
pub(in crate::ui) fn track_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    play_context: Option<LoadedTrackPlayContext>,
) -> gtk::GridView {
    let (columns, card_size) = shell.collection_card_grid_metrics();
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
    let model_for_factory = model.clone();
    let play_context_for_factory = play_context.clone();
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
        let play_action = play_context_for_factory.as_ref().map(|context| {
            track_model_play_action(
                &shell_for_factory,
                &model_for_factory,
                context.clone(),
                None,
                track.clone(),
            )
        });
        item.set_child(Some(&track_card(
            &shell_for_factory,
            &track,
            key,
            card_size,
            play_action,
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
    let play_context = play_context.clone();
    grid.connect_activate(move |_, position| {
        if let Some(track) = item_at::<Track>(&model, position) {
            play_track_from_model(
                &controller,
                &model,
                play_context.as_ref(),
                Some(position),
                track,
            );
        }
    });
    grid
}
pub(in crate::ui) fn album_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::ColumnView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    let initial_width = route_column_view_initial_width(shell);
    table.add_css_class("track-table");
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = shell.library_settings(key).row_fields;
    let mut columns = Vec::with_capacity(fields.len());
    for field in fields {
        let column = album_column(shell, field);
        table.append_column(&column);
        columns.push((column, column_fit_width(field, column_width(field))));
    }
    install_column_view_width_fit(shell, &table, columns, initial_width);
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(album) = item_at::<Album>(&model, position) {
            shell.navigate(Route::AlbumDetail(album.id));
        }
    });
    table
}
pub(in crate::ui) fn artist_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::ColumnView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    let initial_width = route_column_view_initial_width(shell);
    table.add_css_class("track-table");
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = shell.library_settings(key).row_fields;
    let mut columns = Vec::with_capacity(fields.len());
    for field in fields {
        let column = artist_column(shell, field);
        table.append_column(&column);
        columns.push((column, column_fit_width(field, column_width(field))));
    }
    install_column_view_width_fit(shell, &table, columns, initial_width);
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(artist) = item_at::<Artist>(&model, position) {
            shell.navigate(Route::ArtistDetail(artist.id));
        }
    });
    table
}
pub(in crate::ui) fn genre_table(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::ColumnView {
    let selection = gtk::NoSelection::new(Some(model));
    let table = gtk::ColumnView::new(Some(selection));
    let initial_width = route_column_view_initial_width(shell);
    table.add_css_class("track-table");
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = shell.library_settings(LibraryListKey::Genres).row_fields;
    let mut columns = Vec::with_capacity(fields.len());
    for field in fields {
        let column = genre_column(field);
        table.append_column(&column);
        let width = if matches!(field, LibraryField::Title | LibraryField::TitleMerged) {
            180
        } else {
            column_width(field)
        };
        columns.push((column, column_fit_width(field, width)));
    }
    install_column_view_width_fit(shell, &table, columns, initial_width);
    table
}
pub(in crate::ui) fn playlist_table(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::ColumnView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    let initial_width = route_column_view_initial_width(shell);
    table.add_css_class("track-table");
    table.set_single_click_activate(true);
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = shell.library_settings(LibraryListKey::Playlists).row_fields;
    let mut columns = Vec::with_capacity(fields.len());
    for field in fields {
        let column = playlist_column(shell, field);
        table.append_column(&column);
        let width = if matches!(field, LibraryField::Title | LibraryField::TitleMerged) {
            220
        } else {
            column_width(field)
        };
        columns.push((column, column_fit_width(field, width)));
    }
    install_column_view_width_fit(shell, &table, columns, initial_width);
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(playlist) = item_at::<Playlist>(&model, position) {
            shell.navigate(Route::PlaylistDetail(playlist.id));
        }
    });
    table
}
pub(in crate::ui) fn smart_playlist_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> gtk::ColumnView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    let initial_width = route_column_view_initial_width(shell);
    table.add_css_class("track-table");
    table.set_single_click_activate(true);
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = shell
        .library_settings(LibraryListKey::SmartPlaylists)
        .row_fields;
    let reorder_column = smart_playlist_reorder_column(shell);
    table.append_column(&reorder_column);
    let mut columns = Vec::with_capacity(fields.len() + 1);
    columns.push((reorder_column, SMART_PLAYLIST_REORDER_WIDTH));
    for field in fields {
        let column = smart_playlist_column(shell, field);
        table.append_column(&column);
        let width = if matches!(field, LibraryField::Title | LibraryField::TitleMerged) {
            220
        } else {
            column_width(field)
        };
        columns.push((column, column_fit_width(field, width)));
    }
    install_column_view_width_fit(shell, &table, columns, initial_width);
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(playlist) = item_at::<SmartPlaylist>(&model, position) {
            shell.navigate(Route::SmartPlaylistDetail(playlist.id));
        }
    });
    table
}

pub(in crate::ui) fn smart_playlist_reorder_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(playlist) = item_at_from_item::<SmartPlaylist>(item) else {
            return;
        };
        let handle = smart_playlist_drag_handle(&playlist.id);
        install_smart_playlist_drop_target(&handle, &shell, &playlist.id);
        item.set_child(Some(&handle));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(None::<&str>, Some(factory));
    column.set_fixed_width(SMART_PLAYLIST_REORDER_WIDTH);
    column
}
pub(in crate::ui) fn track_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    detail: bool,
    play_context: Option<LoadedTrackPlayContext>,
    content_inset: i32,
    width_mode: ColumnViewWidthMode,
) -> gtk::ColumnView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    let initial_width = column_view_initial_width(shell, content_inset, width_mode);
    table.add_css_class("track-table");
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = if detail {
        shell.library_settings(key).detail_track_fields
    } else {
        shell.library_settings(key).row_fields
    };
    let mut columns = Vec::with_capacity(fields.len());
    for field in fields {
        let column = track_column_for_key(shell, key, field);
        table.append_column(&column);
        columns.push((column, track_column_fit_width(key, field)));
    }
    install_column_view_width_fit(shell, &table, columns, initial_width);
    let controller = shell.controller.clone();
    let play_context = play_context.clone();
    table.connect_activate(move |_, position| {
        if let Some(track) = item_at::<Track>(&model, position) {
            play_track_from_model(
                &controller,
                &model,
                play_context.as_ref(),
                Some(position),
                track,
            );
        }
    });
    table
}
pub(in crate::ui) fn set_library_table_content_height(
    scroller: &gtk::ScrolledWindow,
    row_count: usize,
    max_visible_rows: Option<usize>,
) {
    let height = max_visible_rows.map_or_else(
        || library_table_content_height(row_count),
        |max_visible_rows| capped_library_table_content_height(row_count, Some(max_visible_rows)),
    );
    scroller.set_min_content_height(height);
    scroller.set_max_content_height(height);
}
pub(in crate::ui) fn library_table_content_height(row_count: usize) -> i32 {
    capped_library_table_content_height(row_count, None)
}
pub(in crate::ui) fn capped_library_table_content_height(
    row_count: usize,
    max_visible_rows: Option<usize>,
) -> i32 {
    let max_rows = ((i32::MAX - LIBRARY_TABLE_HEADER_HEIGHT) / LIBRARY_TABLE_ROW_HEIGHT) as usize;
    let visible_rows = row_count.max(1).min(max_visible_rows.unwrap_or(max_rows));
    LIBRARY_TABLE_HEADER_HEIGHT + visible_rows as i32 * LIBRARY_TABLE_ROW_HEIGHT
}
pub(in crate::ui) fn compact_detail_layout(shell: &Shell) -> bool {
    route_content_width(shell) < 760
}

pub(in crate::ui) fn album_card(
    shell: &Rc<Shell>,
    album: &Album,
    key: LibraryListKey,
    size: i32,
) -> gtk::Widget {
    let fields = shell.library_settings(key).grid_fields;
    let card = collection_grid_card(size, fields.len());
    card.append(&cards::album_cover_tile(
        shell,
        album,
        size,
        Some(&shell.controller),
    ));
    card.append(&left_grid_title(&album.title, "track-title", size));
    for field in fields {
        let value = album_field(album, field);
        if !value.is_empty() {
            let label = left_collection_grid_field_label(&value, field, size);
            if matches!(field, LibraryField::Artist | LibraryField::AlbumArtist) {
                add_card_label_link(shell, &label.0, &label.1, &value, album_artist_route(album));
            }
            card.append(&label.0);
        }
    }
    install_album_context_menu(&card, shell, album.clone());
    card.upcast()
}
pub(in crate::ui) fn artist_card(
    shell: &Rc<Shell>,
    artist: &Artist,
    key: LibraryListKey,
    size: i32,
) -> gtk::Widget {
    let fields = shell.library_settings(key).grid_fields;
    let card = collection_grid_card(size, fields.len());
    card.append(&artist_cover_tile(shell, artist, size));
    card.append(&center_grid_title(&artist.name, "track-title", size));
    for field in fields {
        let value = artist_field(artist, field);
        if !value.is_empty() {
            card.append(&center_label(
                &value,
                collection_grid_field_class(field),
                size,
                COLLECTION_GRID_FIELD_LINES,
            ));
        }
    }
    install_artist_context_menu(&card, shell, artist.clone());
    card.upcast()
}
pub(in crate::ui) fn genre_cover_tile(shell: &Rc<Shell>, genre: &Genre, size: i32) -> gtk::Widget {
    let overlay = cards::cover_overlay(size);

    let genre_button = gtk::Button::new();
    genre_button.add_css_class("album-cover-button");
    genre_button.add_css_class("flat");
    cards::constrain_cover_widget(&genre_button, size);
    cards::clip_cover(&genre_button);
    let artwork = crate::cover_art_policy::selected_genre_artwork(genre);
    genre_button.set_child(Some(&shell.cover_group_tile_for_artwork(
        &artwork,
        stable_seed(genre.id.as_str()),
        size,
        THUMB_COVER_SIZE,
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
            let tracks = detail.tracks;
            controller.play_genre_tracks_window(genre_id.clone(), tracks.len(), 0, |index| {
                tracks.get(index).cloned()
            });
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
pub(in crate::ui) fn genre_card(shell: &Rc<Shell>, genre: &Genre, size: i32) -> gtk::Widget {
    let fields = shell.library_settings(LibraryListKey::Genres).grid_fields;
    let card = collection_grid_card(size, fields.len());
    card.append(&genre_cover_tile(shell, genre, size));
    card.append(&center_grid_title(&genre.name, "track-title", size));
    for field in fields {
        let value = genre_field(genre, field);
        if !value.is_empty() {
            card.append(&center_label(
                &value,
                "muted",
                size,
                COLLECTION_GRID_FIELD_LINES,
            ));
        }
    }
    card.upcast()
}
pub(in crate::ui) fn playlist_card(
    shell: &Rc<Shell>,
    playlist: &Playlist,
    size: i32,
) -> gtk::Widget {
    let fields = shell
        .library_settings(LibraryListKey::Playlists)
        .grid_fields;
    let card = collection_grid_card(size, fields.len());
    card.append(&cards::playlist_cover_tile(shell, playlist, size));
    card.append(&center_grid_title(&playlist.name, "track-title", size));
    for field in fields {
        let value = playlist_field(playlist, field);
        if !value.is_empty() {
            card.append(&center_label(
                &value,
                "muted",
                size,
                COLLECTION_GRID_FIELD_LINES,
            ));
        }
    }
    install_playlist_context_menu(&card, shell, playlist.clone());
    card.upcast()
}
pub(in crate::ui) fn smart_playlist_card(
    shell: &Rc<Shell>,
    playlist: &SmartPlaylist,
    size: i32,
) -> gtk::Widget {
    let fields = shell
        .library_settings(LibraryListKey::SmartPlaylists)
        .grid_fields;
    let card_height = collection_grid_card_height(size, fields.len());
    let card = collection_grid_card(size, fields.len());
    card.append(&cards::smart_playlist_cover_tile(shell, playlist, size));
    card.append(&center_grid_title(
        &smart_playlist_display_name(playlist),
        "track-title",
        size,
    ));
    for field in fields {
        let value = smart_playlist_field(playlist, field);
        if !value.is_empty() {
            card.append(&center_label(
                &value,
                "muted",
                size,
                COLLECTION_GRID_FIELD_LINES,
            ));
        }
    }
    let overlay = gtk::Overlay::new();
    overlay.set_size_request(size, card_height);
    overlay.set_hexpand(false);
    overlay.set_halign(gtk::Align::Center);
    overlay.set_child(Some(&card));
    let drag = smart_playlist_drag_handle(&playlist.id);
    drag.set_margin_start(6);
    drag.set_margin_top(6);
    drag.set_halign(gtk::Align::Start);
    drag.set_valign(gtk::Align::Start);
    overlay.add_overlay(&drag);
    install_smart_playlist_drop_target(&overlay, shell, &playlist.id);
    install_smart_playlist_context_menu(&overlay, shell, playlist.clone());
    overlay.upcast()
}

pub(in crate::ui) fn smart_playlist_drag_handle(playlist_id: &SmartPlaylistId) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(SMART_PLAYLIST_REORDER_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let drag_id = playlist_id.as_str().to_string();
    source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&drag_id.to_value()))
    });
    drag.add_controller(source);
    drag
}

pub(in crate::ui) fn install_smart_playlist_drop_target(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    target_id: &SmartPlaylistId,
) {
    let widget = target.as_ref().clone();
    let controller = shell.controller.clone();
    let target_id = target_id.clone();
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(dragged_id) = value.get::<String>() else {
            return false;
        };
        let dragged_id = SmartPlaylistId::new(dragged_id);
        if dragged_id == target_id {
            return false;
        }
        let after = y > f64::from(widget.height()) / 2.0;
        controller.move_smart_playlist(dragged_id, target_id.clone(), after);
        true
    });
    target.add_controller(drop_target);
}
pub(in crate::ui) fn track_card(
    shell: &Rc<Shell>,
    track: &Track,
    key: LibraryListKey,
    size: i32,
    play_action: Option<Rc<dyn Fn()>>,
) -> gtk::Widget {
    let fields = shell.library_settings(key).grid_fields;
    let card = collection_grid_card(size, fields.len());
    card.append(&cards::track_play_tile(shell, track, size, play_action));
    card.append(&center_grid_title(&track.title, "track-title", size));
    for field in fields {
        let value = track_field(track, field);
        if !value.is_empty() {
            let label = collection_grid_field_label(&value, field, size);
            if let Some(route) = track_grid_artist_route(track, field) {
                add_card_label_link(shell, &label.0, &label.1, &value, Some(route));
            }
            card.append(&label.0);
        }
    }
    install_track_context_menu(&card, shell, track.clone());
    card.upcast()
}

fn collection_grid_field_class(field: LibraryField) -> &'static str {
    match field {
        LibraryField::Artist | LibraryField::AlbumArtist => "artist-label",
        _ => "muted",
    }
}

fn collection_grid_field_label(
    value: &str,
    field: LibraryField,
    size: i32,
) -> (gtk::Widget, gtk::Label) {
    center_label_with_label(
        value,
        collection_grid_field_class(field),
        size,
        COLLECTION_GRID_FIELD_LINES,
    )
}

fn left_collection_grid_field_label(
    value: &str,
    field: LibraryField,
    size: i32,
) -> (gtk::Widget, gtk::Label) {
    let label = collection_grid_field_label(value, field, size);
    label.1.set_xalign(0.0);
    label.1.set_justify(gtk::Justification::Left);
    label
}

fn track_grid_artist_route(track: &Track, field: LibraryField) -> Option<Route> {
    match field {
        LibraryField::Artist => track_artist_route(track),
        LibraryField::AlbumArtist => track_album_artist_route(track),
        _ => None,
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

fn collection_grid_card(size: i32, field_count: usize) -> gtk::Box {
    let height = collection_grid_card_height(size, field_count);
    let card = gtk::Box::new(gtk::Orientation::Vertical, COLLECTION_GRID_CARD_GAP);
    card.add_css_class("album-card");
    card.set_size_request(size, height);
    card.set_hexpand(false);
    card.set_vexpand(false);
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Start);
    card
}
pub(in crate::ui) fn artist_cover_tile(
    shell: &Rc<Shell>,
    artist: &Artist,
    size: i32,
) -> gtk::Widget {
    let overlay = cards::cover_overlay(size);

    let artist_button = gtk::Button::new();
    artist_button.add_css_class("album-cover-button");
    artist_button.add_css_class("flat");
    cards::constrain_cover_widget(&artist_button, size);
    cards::clip_cover(&artist_button);
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

    let mut controls = cards::cover_hover_controls(size, "Play artist", artist.favorite);
    let menu = controls.add_context_button();
    let menu_target = overlay.clone();
    let menu_shell = Rc::clone(shell);
    let menu_artist = artist.clone();
    menu.connect_clicked(move |_| {
        present_artist_context_menu(
            menu_target.upcast_ref(),
            &menu_shell,
            context_artist(&menu_shell, &menu_artist),
            cards::cover_context_point(size),
        );
    });
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    controls.play.connect_clicked(move |_| {
        if let Ok(Some(detail)) = controller.cached_artist_detail(&artist_id) {
            controller.play_artist_tracks_window(
                artist_id.clone(),
                domain::ArtistTrackScope::AllCredits,
                detail.tracks.len(),
                0,
                |index| detail.tracks.get(index).cloned(),
            );
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
    if let Some(favorite) = controls.favorite.as_ref() {
        shell.register_favorite_button(artist_favorite_key(&artist.id), favorite);
        let favorite_shell = Rc::clone(shell);
        let artist_id = artist.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                source::FavoriteItemId::Artist(artist_id.clone()),
                favorite,
                Some(button),
            );
        });
    }
    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);

    overlay.upcast()
}
pub(in crate::ui) fn artist_cover_image_ref(
    _shell: &Rc<Shell>,
    artist: &Artist,
) -> Option<ImageRef> {
    artist.image_ref.clone()
}
