fn ui_perf_next_scroll_value(
    scenario: UiPerfScenario,
    adjustment: &gtk::Adjustment,
    max_value: f64,
    direction: &Cell<f64>,
    jump_index: &Cell<usize>,
) -> f64 {
    match scenario {
        UiPerfScenario::HumanScroll => {
            let step = (adjustment.page_size() * 0.20).clamp(80.0, 180.0);
            bounce_scroll_value(adjustment.value(), step, max_value, direction)
        }
        UiPerfScenario::FastScroll => {
            let step = (adjustment.page_size() * 0.95).max(260.0);
            bounce_scroll_value(adjustment.value(), step, max_value, direction)
        }
        UiPerfScenario::Jump => {
            let points = [0.0, 0.25, 0.85, 0.45, 1.0, 0.10, 0.65, 0.0];
            let index = jump_index.get();
            jump_index.set(index.saturating_add(1));
            max_value * points[index % points.len()]
        }
        UiPerfScenario::DragSweep => {
            let index = jump_index.get();
            jump_index.set(index.saturating_add(1));
            let phase = (index % 64) as f64 / 63.0;
            let fraction = if (index / 64).is_multiple_of(2) {
                phase
            } else {
                1.0 - phase
            };
            max_value * fraction
        }
    }
}
fn bounce_scroll_value(current: f64, step: f64, max_value: f64, direction: &Cell<f64>) -> f64 {
    let mut next = current + direction.get() * step;
    if next >= max_value {
        next = max_value;
        direction.set(-1.0);
    } else if next <= 0.0 {
        next = 0.0;
        direction.set(1.0);
    }
    next
}
fn find_largest_scrolled_window(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut best = None;
    collect_largest_scrolled_window(widget, &mut best);
    best.map(|(scroller, _)| scroller)
}
fn restore_scrolled_window_value(widget: &gtk::Widget, value: f64) {
    let Some(scroller) = find_largest_scrolled_window(widget) else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let max_value = (adjustment.upper() - adjustment.page_size()).max(0.0);
    adjustment.set_value(value.clamp(0.0, max_value));
}
fn collect_largest_scrolled_window(
    widget: &gtk::Widget,
    best: &mut Option<(gtk::ScrolledWindow, f64)>,
) {
    if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
        let adjustment = scroller.vadjustment();
        let score = (adjustment.upper() - adjustment.page_size()).max(0.0);
        if best
            .as_ref()
            .is_none_or(|(_, best_score)| score > *best_score)
        {
            *best = Some((scroller, score));
        }
    }

    let mut child = widget.first_child();
    while let Some(widget) = child {
        collect_largest_scrolled_window(&widget, best);
        child = widget.next_sibling();
    }
}
fn replace_albums_in_model(model: &gio::ListStore, albums: impl IntoIterator<Item = Album>) {
    let additions = albums
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
fn append_albums_to_model(model: &gio::ListStore, albums: impl IntoIterator<Item = Album>) {
    append_boxed_items_to_model(model, albums);
}
fn replace_artists_in_model(model: &gio::ListStore, artists: impl IntoIterator<Item = Artist>) {
    let additions = artists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
fn append_artists_to_model(model: &gio::ListStore, artists: impl IntoIterator<Item = Artist>) {
    append_boxed_items_to_model(model, artists);
}
fn replace_genres_in_model(model: &gio::ListStore, genres: impl IntoIterator<Item = Genre>) {
    let additions = genres
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
fn append_genres_to_model(model: &gio::ListStore, genres: impl IntoIterator<Item = Genre>) {
    append_boxed_items_to_model(model, genres);
}
fn replace_playlists_in_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    let additions = playlists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
fn append_playlists_to_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    append_boxed_items_to_model(model, playlists);
}
fn set_track_sort_button_content(button: &gtk::Button, settings: &TrackTableSettings) {
    let sort_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    sort_content.append(&gtk::Label::new(Some(&tr(settings.sort_key.title()))));
    sort_content.append(&gtk::Image::from_icon_name(if settings.descending {
        "view-sort-descending-symbolic"
    } else {
        "view-sort-ascending-symbolic"
    }));
    button.set_child(Some(&sort_content));
}
fn set_track_table_columns(
    shell: &Rc<Shell>,
    table: &gtk::ColumnView,
    settings: &TrackTableSettings,
) {
    let columns = table.columns();
    while columns.n_items() > 0 {
        let Some(column) = columns
            .item(0)
            .and_then(|item| item.downcast::<gtk::ColumnViewColumn>().ok())
        else {
            break;
        };
        table.remove_column(&column);
    }

    for column in &settings.visible_columns {
        table.append_column(&track_table_column(shell, *column));
    }
}
fn track_table_column(shell: &Rc<Shell>, column: TrackTableColumn) -> gtk::ColumnViewColumn {
    match column {
        TrackTableColumn::TrackNumber => track_row_index_column(),
        TrackTableColumn::Title => track_identity_column(shell),
        TrackTableColumn::Artist => track_link_column(shell, "Artist", 180, |track| {
            (track.artist.clone(), track_artist_route(track))
        }),
        TrackTableColumn::Album => track_link_column(shell, "Album", 220, |track| {
            (
                track.album.clone(),
                Some(Route::AlbumDetail(track.album_id.clone())),
            )
        }),
        TrackTableColumn::Year => track_column("Year", 70, |track| track.year.to_string()),
        TrackTableColumn::Duration => track_column("Duration", 90, |track| {
            format_duration(track.duration_seconds)
        }),
        TrackTableColumn::Favorite => track_favorite_column(shell),
    }
}
fn populate_track_model_with_options(
    model: &gio::ListStore,
    tracks: &[Track],
    settings: &TrackTableSettings,
    query: &str,
    favorite_first: bool,
) {
    let query = query.trim().to_lowercase();
    let mut filtered = tracks
        .iter()
        .filter(|track| query.is_empty() || track_matches_query(track, &query))
        .cloned()
        .collect::<Vec<_>>();
    sort_tracks_with_options(&mut filtered, settings, favorite_first);
    let additions = filtered
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
fn append_tracks_to_model(model: &gio::ListStore, tracks: impl IntoIterator<Item = Track>) {
    append_boxed_items_to_model(model, tracks);
}
fn append_boxed_items_to_model<T: 'static>(
    model: &gio::ListStore,
    items: impl IntoIterator<Item = T>,
) {
    let additions = items
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    if !additions.is_empty() {
        model.splice(model.n_items(), 0, &additions);
    }
}
fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || track.album.to_lowercase().contains(query)
        || track.year.to_string().contains(query)
}
fn sort_tracks_with_options(
    tracks: &mut [Track],
    settings: &TrackTableSettings,
    favorite_first: bool,
) {
    tracks.sort_by(|left, right| {
        let mut ordering = match settings.sort_key {
            TrackSortKey::TrackNumber => left
                .disc_number
                .cmp(&right.disc_number)
                .then(left.track_number.cmp(&right.track_number))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase())),
            TrackSortKey::Title => left.title.to_lowercase().cmp(&right.title.to_lowercase()),
            TrackSortKey::Artist => left
                .artist
                .to_lowercase()
                .cmp(&right.artist.to_lowercase())
                .then_with(|| left.album.to_lowercase().cmp(&right.album.to_lowercase()))
                .then(left.track_number.cmp(&right.track_number)),
            TrackSortKey::Album => left
                .album
                .to_lowercase()
                .cmp(&right.album.to_lowercase())
                .then(left.disc_number.cmp(&right.disc_number))
                .then(left.track_number.cmp(&right.track_number)),
            TrackSortKey::Year => left
                .year
                .cmp(&right.year)
                .then_with(|| left.album.to_lowercase().cmp(&right.album.to_lowercase())),
            TrackSortKey::Duration => left.duration_seconds.cmp(&right.duration_seconds),
            TrackSortKey::Favorite => left.favorite.cmp(&right.favorite),
        };
        if settings.descending {
            ordering = ordering.reverse();
        }

        if favorite_first {
            right.favorite.cmp(&left.favorite).then(ordering)
        } else {
            ordering
        }
    });
}
fn track_sort_index(sort_key: TrackSortKey) -> u32 {
    TrackSortKey::all()
        .iter()
        .position(|candidate| *candidate == sort_key)
        .unwrap_or(0) as u32
}
fn track_sort_from_index(index: u32) -> TrackSortKey {
    TrackSortKey::all()
        .get(index as usize)
        .copied()
        .unwrap_or(TrackSortKey::TrackNumber)
}
fn track_table_column_config_title(column: TrackTableColumn) -> &'static str {
    match column {
        TrackTableColumn::Title => "Title (merged)",
        _ => column.title(),
    }
}
fn sync_track_column_checks(
    checks: &Rc<RefCell<Vec<(TrackTableColumn, gtk::CheckButton)>>>,
    settings: &TrackTableSettings,
    syncing: &Cell<bool>,
) {
    syncing.set(true);
    for (column, check) in checks.borrow().iter() {
        check.set_active(settings.visible_columns.contains(column));
    }
    syncing.set(false);
}
fn route_uses_responsive_cards(route: &Route) -> bool {
    matches!(
        route,
        Route::Home
            | Route::Albums
            | Route::Artists
            | Route::AlbumArtists
            | Route::Favorites
            | Route::ArtistDetail(_)
            | Route::ArtistDiscography(_)
            | Route::Genres
            | Route::GenreDetail(_)
            | Route::Playlists
            | Route::PlaylistDetail(_)
            | Route::Search { .. }
    )
}
fn route_boundary(view: gtk::Widget) -> gtk::Widget {
    let spec = route_boundary_spec();
    let scroller = gtk::ScrolledWindow::new();
    // this is necessary because route pages can contain tables, grids, and
    // toolbars wider than the visible pane. they may scroll inside the pane,
    // but they must never draw under the right sidebar.
    scroller.set_policy(spec.horizontal_policy, spec.vertical_policy);
    scroller.set_overflow(spec.overflow);
    scroller.set_min_content_width(spec.min_content_width);
    scroller.set_propagate_natural_width(spec.propagate_natural_width);
    scroller.set_hexpand(spec.hexpand);
    scroller.set_vexpand(spec.vexpand);
    scroller.set_child(Some(&view));
    scroller.upcast()
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RouteBoundarySpec {
    horizontal_policy: gtk::PolicyType,
    vertical_policy: gtk::PolicyType,
    overflow: gtk::Overflow,
    min_content_width: i32,
    propagate_natural_width: bool,
    hexpand: bool,
    vexpand: bool,
}
fn route_boundary_spec() -> RouteBoundarySpec {
    RouteBoundarySpec {
        horizontal_policy: gtk::PolicyType::Automatic,
        vertical_policy: gtk::PolicyType::Never,
        overflow: gtk::Overflow::Hidden,
        min_content_width: 0,
        propagate_natural_width: false,
        hexpand: true,
        vexpand: true,
    }
}
fn route_displays_sync_status(_route: &Route, first_run: bool) -> bool {
    first_run
}
fn stable_seed(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}
fn next_home_showcase_seed() -> u64 {
    let counter = HOME_SHOWCASE_COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    let time_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_else(|_| stable_seed("home-showcase") as u64);
    time_seed.rotate_left(17) ^ counter.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}
fn add_album_seed_gradient_class(widget: &impl IsA<gtk::Widget>, seed: u32) {
    let class_name = format!("album-seed-gradient-{:08x}", seed);
    widget.add_css_class(&class_name);

    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let (red, green, blue) = showcase_seed_rgb(seed);
    let (red_two, green_two, blue_two) = showcase_seed_rgb(seed.rotate_left(11) ^ 0x5bd1_e995);
    let (red_three, green_three, blue_three) =
        showcase_seed_rgb(seed.rotate_right(7) ^ 0x9e37_79b9);
    let css = format!(
        ".{class_name} {{
            background: linear-gradient(135deg,
                color-mix(in srgb, rgba({red}, {green}, {blue}, 0.78) 58%, @window_bg_color),
                color-mix(in srgb, rgba({red_two}, {green_two}, {blue_two}, 0.64) 44%, @card_bg_color) 58%,
                color-mix(in srgb, @window_bg_color 62%, rgba({red_three}, {green_three}, {blue_three}, 0.56)));
        }}"
    );
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
fn showcase_seed_rgb(seed: u32) -> (u8, u8, u8) {
    (
        showcase_color_component(seed, 0),
        showcase_color_component(seed, 8),
        showcase_color_component(seed, 16),
    )
}
fn showcase_color_component(seed: u32, shift: u8) -> u8 {
    let value = ((seed >> shift) & 0xff) as f64;
    (value * 0.72 + 48.0).round().clamp(0.0, 232.0) as u8
}
fn track_column<F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        list_item.set_child(Some(&label));
    });

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(child) = list_item.child() else {
            return;
        };
        let Ok(label) = child.downcast::<gtk::Label>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let track = boxed.borrow::<Track>();
        label.set_text(&value(&track));
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}
fn track_row_index_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        list_item.set_child(Some(&label));
    });

    factory.connect_bind(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(child) = list_item.child() else {
            return;
        };
        let Ok(label) = child.downcast::<gtk::Label>() else {
            return;
        };
        label.set_text(&(list_item.position() + 1).to_string());
    });

    let column = gtk::ColumnViewColumn::new(Some("#"), Some(factory));
    column.set_fixed_width(54);
    column.set_resizable(false);
    column
}
fn track_identity_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let track = boxed.borrow::<Track>().clone();
        let artist_text = track.artist.clone();
        let artist_route = track_artist_route(&track);
        let cover = shell.cover_tile_for(
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("track-identity");
        row.set_valign(gtk::Align::Center);
        row.set_hexpand(true);
        row.append(&cover);

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_valign(gtk::Align::Center);
        labels.set_hexpand(true);

        let title = gtk::Label::new(Some(&track.title));
        title.add_css_class("track-title");
        title.set_xalign(0.0);
        title.set_halign(gtk::Align::Fill);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&title);

        if !artist_text.trim().is_empty() {
            let artist = gtk::Label::new(Some(&artist_text));
            artist.add_css_class("muted");
            artist.add_css_class("table-link-label");
            artist.set_xalign(0.0);
            artist.set_halign(gtk::Align::Start);
            artist.set_hexpand(false);
            artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
            artist.set_width_chars(1);
            artist.set_max_width_chars(28);

            if let Some(route) = artist_route {
                let button = gtk::Button::new();
                button.add_css_class("flat");
                button.add_css_class("table-link");
                button.add_css_class("track-artist-link");
                button.set_halign(gtk::Align::Start);
                button.set_hexpand(false);
                button.set_cursor_from_name(Some("pointer"));
                add_link_hover(button.upcast_ref(), &artist, &artist_text);
                button.set_child(Some(&artist));

                let shell = Rc::clone(&shell);
                button.connect_clicked(move |_| shell.navigate(route.clone()));
                labels.append(&button);
            } else {
                labels.append(&artist);
            }
        }

        row.append(&labels);
        install_track_context_menu(&row, &shell, track);
        list_item.set_child(Some(&row));
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(None::<&gtk::Widget>);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr("Title")), Some(factory));
    column.set_fixed_width(320);
    column.set_resizable(false);
    column
}
fn track_link_column<F>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> (String, Option<Route>) + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    let shell = Rc::clone(shell);

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let track = boxed.borrow::<Track>().clone();
        let (text, route) = value(&track);
        let label = gtk::Label::new(Some(&text));
        label.add_css_class("table-link-label");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(false);
        label.set_width_chars(1);
        label.set_max_width_chars((width / 8).clamp(8, 32));

        let Some(route) = route else {
            install_track_context_menu(&label, &shell, track);
            list_item.set_child(Some(&label));
            return;
        };

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("table-link");
        button.set_halign(gtk::Align::Start);
        button.set_hexpand(false);
        button.set_cursor_from_name(Some("pointer"));

        add_link_hover(button.upcast_ref(), &label, &text);

        button.set_child(Some(&label));
        install_track_context_menu(&button, &shell, track);

        let shell = Rc::clone(&shell);
        button.connect_clicked(move |_| shell.navigate(route.clone()));
        list_item.set_child(Some(&button));
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(None::<&gtk::Widget>);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}
fn track_artist_route(track: &Track) -> Option<Route> {
    if let Some(artist_id) = track.artist_id.clone() {
        Some(Route::ArtistDetail(artist_id))
    } else if !track.artist.trim().is_empty() {
        Some(Route::Search {
            query: track.artist.clone(),
            kind: SearchKind::Artists,
        })
    } else {
        None
    }
}
fn album_artist_route(album: &Album) -> Option<Route> {
    if let Some(artist_id) = album.artist_id.clone() {
        Some(Route::ArtistDetail(artist_id))
    } else if !album.artist.trim().is_empty() {
        Some(Route::Search {
            query: album.artist.clone(),
            kind: SearchKind::Artists,
        })
    } else {
        None
    }
}
fn install_track_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>, track: Track) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_track = track.clone();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        present_track_context_menu(
            &target,
            &click_shell,
            context_track(&click_shell, &click_track),
            Some((x, y)),
        );
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key_track = track;
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            present_track_context_menu(
                &target,
                &key_shell,
                context_track(&key_shell, &key_track),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
fn install_album_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>, album: Album) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_album = album.clone();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        present_album_context_menu(
            &target,
            &click_shell,
            context_album(&click_shell, &click_album),
            Some((x, y)),
        );
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key_album = album;
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            present_album_context_menu(
                &target,
                &key_shell,
                context_album(&key_shell, &key_album),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
fn install_artist_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>, artist: Artist) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_artist = artist.clone();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        present_artist_context_menu(
            &target,
            &click_shell,
            context_artist(&click_shell, &click_artist),
            Some((x, y)),
        );
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key_artist = artist;
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            present_artist_context_menu(
                &target,
                &key_shell,
                context_artist(&key_shell, &key_artist),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
fn install_current_track_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        if let Some(track) = current_player_track(&click_shell) {
            present_track_context_menu(&target, &click_shell, track, Some((x, y)));
        }
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade()
            && let Some(track) = current_player_track(&key_shell)
        {
            present_track_context_menu(&target, &key_shell, track, None);
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
