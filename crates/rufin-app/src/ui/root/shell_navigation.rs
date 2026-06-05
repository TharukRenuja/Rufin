use super::*;

const ROUTE_SCROLL_OWNER_CLASS: &str = "route-scroll-owner";

pub(in crate::ui) fn mark_route_scroll_owner(scroller: &gtk::ScrolledWindow) {
    scroller.add_css_class(ROUTE_SCROLL_OWNER_CLASS);
}

pub(in crate::ui) fn find_largest_scrolled_window(
    widget: &gtk::Widget,
) -> Option<gtk::ScrolledWindow> {
    let mut best = None;
    collect_scrolled_window(widget, 0, &mut best);
    best.map(|(scroller, _)| scroller)
}
fn collect_scrolled_window(
    widget: &gtk::Widget,
    depth: usize,
    best: &mut Option<(gtk::ScrolledWindow, ScrollerScore)>,
) {
    if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
        let adjustment = scroller.vadjustment();
        let score = ScrollerScore {
            owner: scroller.has_css_class(ROUTE_SCROLL_OWNER_CLASS),
            vertical: scroller.vscrollbar_policy() != gtk::PolicyType::Never,
            range: (adjustment.upper() - adjustment.page_size()).max(0.0),
            depth,
        };
        if best
            .as_ref()
            .is_none_or(|(_, best_score)| scroller_score_is_better(score, *best_score))
        {
            *best = Some((scroller, score));
        }
    }

    let mut child = widget.first_child();
    while let Some(widget) = child {
        collect_scrolled_window(&widget, depth.saturating_add(1), best);
        child = widget.next_sibling();
    }
}

#[derive(Clone, Copy)]
struct ScrollerScore {
    owner: bool,
    vertical: bool,
    range: f64,
    depth: usize,
}

fn scroller_score_is_better(candidate: ScrollerScore, current: ScrollerScore) -> bool {
    (
        candidate.owner,
        candidate.vertical,
        candidate.range,
        candidate.depth,
    ) > (
        current.owner,
        current.vertical,
        current.range,
        current.depth,
    )
}
pub(in crate::ui) fn replace_albums_in_model(
    model: &gio::ListStore,
    albums: impl IntoIterator<Item = Album>,
) {
    let additions = albums
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
pub(in crate::ui) fn append_albums_to_model(
    model: &gio::ListStore,
    albums: impl IntoIterator<Item = Album>,
) {
    append_boxed_items_to_model(model, albums);
}
pub(in crate::ui) fn replace_artists_in_model(
    model: &gio::ListStore,
    artists: impl IntoIterator<Item = Artist>,
) {
    let additions = artists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
pub(in crate::ui) fn append_artists_to_model(
    model: &gio::ListStore,
    artists: impl IntoIterator<Item = Artist>,
) {
    append_boxed_items_to_model(model, artists);
}
pub(in crate::ui) fn replace_genres_in_model(
    model: &gio::ListStore,
    genres: impl IntoIterator<Item = Genre>,
) {
    let additions = genres
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
pub(in crate::ui) fn append_genres_to_model(
    model: &gio::ListStore,
    genres: impl IntoIterator<Item = Genre>,
) {
    append_boxed_items_to_model(model, genres);
}
pub(in crate::ui) fn replace_playlists_in_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    let additions = playlists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
pub(in crate::ui) fn append_playlists_to_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    append_boxed_items_to_model(model, playlists);
}
pub(in crate::ui) fn set_track_sort_button_content(
    button: &gtk::Button,
    settings: &TrackTableSettings,
) {
    let sort_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    sort_content.append(&gtk::Label::new(Some(&tr(settings.sort_key.title()))));
    sort_content.append(&gtk::Image::from_icon_name(if settings.descending {
        "view-sort-descending-symbolic"
    } else {
        "view-sort-ascending-symbolic"
    }));
    button.set_child(Some(&sort_content));
}
pub(in crate::ui) fn set_track_table_columns(
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
    fit_track_columns(
        table,
        &settings.visible_columns,
        super::library::route_column_view_initial_width(shell.as_ref()),
    );
}
pub(in crate::ui) fn fit_track_table_columns(
    table: &gtk::ColumnView,
    visible_columns: &[TrackTableColumn],
) {
    let available_width = table.width().saturating_sub(2);
    if available_width <= 1 || visible_columns.is_empty() {
        return;
    }

    fit_track_columns(table, visible_columns, available_width);
}
pub(in crate::ui) fn fit_track_columns(
    table: &gtk::ColumnView,
    visible_columns: &[TrackTableColumn],
    available_width: i32,
) {
    if available_width <= 1 || visible_columns.is_empty() {
        return;
    }

    let base_widths = visible_columns
        .iter()
        .map(|column| track_table_column_width(*column))
        .collect::<Vec<_>>();
    let fitted_widths = super::library::fitted_column_widths(&base_widths, available_width);
    let columns = table.columns();
    for (index, width) in fitted_widths.into_iter().enumerate() {
        let Some(column) = columns
            .item(index as u32)
            .and_then(|item| item.downcast::<gtk::ColumnViewColumn>().ok())
        else {
            continue;
        };
        column.set_fixed_width(width);
    }
}
fn track_table_column_width(column: TrackTableColumn) -> i32 {
    match column {
        TrackTableColumn::TrackNumber => 54,
        TrackTableColumn::Title => 320,
        TrackTableColumn::Artist => 180,
        TrackTableColumn::Album => 220,
        TrackTableColumn::Year => 70,
        TrackTableColumn::Duration => 90,
        TrackTableColumn::Favorite => 76,
    }
}
pub(in crate::ui) fn track_table_column(
    shell: &Rc<Shell>,
    column: TrackTableColumn,
) -> gtk::ColumnViewColumn {
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
        TrackTableColumn::Year => track_column(shell, "Year", 70, |track| track.year.to_string()),
        TrackTableColumn::Duration => track_column(shell, "◷", 90, |track| {
            format_duration(track.duration_seconds)
        }),
        TrackTableColumn::Favorite => track_favorite_column(shell),
    }
}
pub(in crate::ui) fn populate_track_model_with_options(
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
pub(in crate::ui) fn append_tracks_to_model(
    model: &gio::ListStore,
    tracks: impl IntoIterator<Item = Track>,
) {
    append_boxed_items_to_model(model, tracks);
}
pub(in crate::ui) fn append_boxed_items_to_model<T: 'static>(
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
pub(in crate::ui) fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || track.album.to_lowercase().contains(query)
        || track.year.to_string().contains(query)
}
pub(in crate::ui) fn sort_tracks_with_options(
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
pub(in crate::ui) fn track_sort_index(sort_key: TrackSortKey) -> u32 {
    TrackSortKey::all()
        .iter()
        .position(|candidate| *candidate == sort_key)
        .unwrap_or(0) as u32
}
pub(in crate::ui) fn track_sort_from_index(index: u32) -> TrackSortKey {
    TrackSortKey::all()
        .get(index as usize)
        .copied()
        .unwrap_or(TrackSortKey::TrackNumber)
}
pub(in crate::ui) fn track_table_column_config_title(column: TrackTableColumn) -> &'static str {
    match column {
        TrackTableColumn::Title => "Title (merged)",
        _ => column.title(),
    }
}
pub(in crate::ui) fn sync_track_column_checks(
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
pub(in crate::ui) fn route_uses_responsive_cards(route: &Route) -> bool {
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
            | Route::SmartPlaylists
            | Route::PlaylistDetail(_)
            | Route::SmartPlaylistDetail(_)
            | Route::Search { .. }
    )
}
pub(in crate::ui) fn route_boundary_for_route(route: &Route, view: gtk::Widget) -> gtk::Widget {
    route_boundary_from_spec(view, route_boundary_spec_for_route(route))
}
fn route_boundary_from_spec(view: gtk::Widget, spec: RouteBoundarySpec) -> gtk::Widget {
    let scroller = gtk::ScrolledWindow::new();
    // this is necessary because route pages can contain tables, grids, and
    // toolbars wider than the visible pane. they may scroll inside the pane,
    // but they must never draw under the right sidebar.
    scroller.set_policy(spec.horizontal_policy, spec.vertical_policy);
    scroller.set_overflow(spec.overflow);
    scroller.set_min_content_width(spec.min_content_width);
    scroller.set_propagate_natural_width(spec.propagate_natural_width);
    scroller.set_propagate_natural_height(false);
    scroller.set_hexpand(spec.hexpand);
    scroller.set_vexpand(spec.vexpand);
    scroller.set_child(Some(&view));
    scroller.upcast()
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct RouteBoundarySpec {
    pub(in crate::ui) horizontal_policy: gtk::PolicyType,
    pub(in crate::ui) vertical_policy: gtk::PolicyType,
    pub(in crate::ui) overflow: gtk::Overflow,
    pub(in crate::ui) min_content_width: i32,
    pub(in crate::ui) propagate_natural_width: bool,
    pub(in crate::ui) hexpand: bool,
    pub(in crate::ui) vexpand: bool,
}
pub(in crate::ui) fn route_boundary_spec() -> RouteBoundarySpec {
    RouteBoundarySpec {
        horizontal_policy: gtk::PolicyType::External,
        vertical_policy: gtk::PolicyType::Never,
        overflow: gtk::Overflow::Hidden,
        min_content_width: 0,
        propagate_natural_width: false,
        hexpand: true,
        vexpand: true,
    }
}
pub(in crate::ui) fn route_boundary_spec_for_route(_route: &Route) -> RouteBoundarySpec {
    route_boundary_spec()
}
pub(in crate::ui) fn route_displays_sync_status(_route: &Route, first_run: bool) -> bool {
    first_run
}
pub(in crate::ui) fn stable_seed(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}
pub(in crate::ui) fn next_home_showcase_seed() -> u64 {
    let counter = HOME_SHOWCASE_COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    let time_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_else(|_| stable_seed("home-showcase") as u64);
    time_seed.rotate_left(17) ^ counter.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}
pub(in crate::ui) fn add_album_seed_gradient_class(widget: &impl IsA<gtk::Widget>, seed: u32) {
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
pub(in crate::ui) fn showcase_seed_rgb(seed: u32) -> (u8, u8, u8) {
    (
        showcase_color_component(seed, 0),
        showcase_color_component(seed, 8),
        showcase_color_component(seed, 16),
    )
}
pub(in crate::ui) fn showcase_color_component(seed: u32, shift: u8) -> u8 {
    let value = ((seed >> shift) & 0xff) as f64;
    (value * 0.72 + 48.0).round().clamp(0.0, 232.0) as u8
}
#[derive(Clone)]
pub(in crate::ui) struct TrackIdentityCell {
    cover: ArtworkTile,
    title: gtk::Label,
    artist_button: gtk::Button,
    artist_button_label: gtk::Label,
    artist_label: gtk::Label,
    artist_route: Rc<RefCell<Option<Route>>>,
    artist_hover_text: Rc<RefCell<String>>,
    current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
pub(in crate::ui) struct TrackLinkCell {
    button: gtk::Button,
    button_label: gtk::Label,
    label: gtk::Label,
    route: Rc<RefCell<Option<Route>>>,
    hover_text: Rc<RefCell<String>>,
    current_track: Rc<RefCell<Option<Track>>>,
}

thread_local! {
    static TRACK_IDENTITY_CELLS: RefCell<HashMap<usize, TrackIdentityCell>> = RefCell::new(HashMap::new());
    static TRACK_LINK_CELLS: RefCell<HashMap<usize, TrackLinkCell>> = RefCell::new(HashMap::new());
}

pub(in crate::ui) fn list_item_storage_key(list_item: &gtk::ListItem) -> usize {
    list_item.as_ptr() as usize
}

pub(in crate::ui) fn store_track_identity_cell(list_item: &gtk::ListItem, cell: TrackIdentityCell) {
    let key = list_item_storage_key(list_item);
    TRACK_IDENTITY_CELLS.with(|cells| {
        cells.borrow_mut().insert(key, cell);
    });
}

pub(in crate::ui) fn track_identity_cell(list_item: &gtk::ListItem) -> Option<TrackIdentityCell> {
    let key = list_item_storage_key(list_item);
    TRACK_IDENTITY_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn remove_track_identity_cell(list_item: &gtk::ListItem) {
    let key = list_item_storage_key(list_item);
    TRACK_IDENTITY_CELLS.with(|cells| {
        cells.borrow_mut().remove(&key);
    });
}

pub(in crate::ui) fn store_track_link_cell(list_item: &gtk::ListItem, cell: TrackLinkCell) {
    let key = list_item_storage_key(list_item);
    TRACK_LINK_CELLS.with(|cells| {
        cells.borrow_mut().insert(key, cell);
    });
}

pub(in crate::ui) fn track_link_cell(list_item: &gtk::ListItem) -> Option<TrackLinkCell> {
    let key = list_item_storage_key(list_item);
    TRACK_LINK_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn remove_track_link_cell(list_item: &gtk::ListItem) {
    let key = list_item_storage_key(list_item);
    TRACK_LINK_CELLS.with(|cells| {
        cells.borrow_mut().remove(&key);
    });
}

pub(in crate::ui) fn track_column<F>(
    _shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
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
        label.set_xalign(0.5);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
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
pub(in crate::ui) fn track_row_index_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.5);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
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
pub(in crate::ui) fn track_identity_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };

        let current_track = Rc::new(RefCell::new(None::<Track>));
        let artist_route = Rc::new(RefCell::new(None::<Route>));
        let artist_hover_text = Rc::new(RefCell::new(String::new()));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("track-identity");
        row.set_valign(gtk::Align::Center);
        row.set_hexpand(true);

        let cover = ArtworkTile::new(48, 0);
        row.append(&cover.widget());

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_valign(gtk::Align::Center);
        labels.set_hexpand(true);

        let title = gtk::Label::new(None);
        title.add_css_class("track-title");
        title.set_xalign(0.0);
        title.set_halign(gtk::Align::Fill);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&title);

        let artist_button_label = gtk::Label::new(None);
        artist_button_label.add_css_class("muted");
        artist_button_label.add_css_class("table-link-label");
        artist_button_label.set_xalign(0.0);
        artist_button_label.set_halign(gtk::Align::Start);
        artist_button_label.set_hexpand(false);
        artist_button_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        artist_button_label.set_width_chars(1);
        artist_button_label.set_max_width_chars(28);

        let artist_button = gtk::Button::new();
        artist_button.add_css_class("flat");
        artist_button.add_css_class("table-link");
        artist_button.add_css_class("track-artist-link");
        artist_button.set_halign(gtk::Align::Start);
        artist_button.set_hexpand(false);
        artist_button.set_cursor_from_name(Some("pointer"));
        add_stateful_link_hover(
            artist_button.upcast_ref(),
            &artist_button_label,
            Rc::clone(&artist_hover_text),
        );
        artist_button.set_child(Some(&artist_button_label));
        artist_button.set_visible(false);
        labels.append(&artist_button);

        let artist_label = gtk::Label::new(None);
        artist_label.add_css_class("muted");
        artist_label.add_css_class("table-link-label");
        artist_label.set_xalign(0.0);
        artist_label.set_halign(gtk::Align::Start);
        artist_label.set_hexpand(false);
        artist_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        artist_label.set_width_chars(1);
        artist_label.set_max_width_chars(28);
        artist_label.set_visible(false);
        labels.append(&artist_label);

        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&artist_route);
        artist_button.connect_clicked(move |_| {
            if let Some(route) = route_for_click.borrow().clone() {
                click_shell.navigate(route);
            }
        });

        row.append(&labels);
        install_dynamic_track_context_menu(&row, &setup_shell, Rc::clone(&current_track));
        list_item.set_child(Some(&row));
        store_track_identity_cell(
            list_item,
            TrackIdentityCell {
                cover,
                title,
                artist_button,
                artist_button_label,
                artist_label,
                artist_route,
                artist_hover_text,
                current_track,
            },
        );
    });

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
        let Some(cell) = track_identity_cell(list_item) else {
            return;
        };
        let track = boxed.borrow::<Track>().clone();
        let artist_text = track.artist.clone();
        let artist_route = track_artist_route(&track);
        shell.bind_cover_tile_for(
            &cell.cover,
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );
        cell.title.set_text(&track.title);
        *cell.current_track.borrow_mut() = Some(track);

        if artist_text.trim().is_empty() {
            *cell.artist_route.borrow_mut() = None;
            cell.artist_hover_text.borrow_mut().clear();
            cell.artist_button.set_visible(false);
            cell.artist_label.set_visible(false);
        } else if let Some(route) = artist_route {
            *cell.artist_route.borrow_mut() = Some(route);
            *cell.artist_hover_text.borrow_mut() = artist_text.clone();
            cell.artist_button_label.set_text(&artist_text);
            cell.artist_button.set_visible(true);
            cell.artist_label.set_visible(false);
        } else {
            *cell.artist_route.borrow_mut() = None;
            cell.artist_hover_text.borrow_mut().clear();
            cell.artist_label.set_text(&artist_text);
            cell.artist_button.set_visible(false);
            cell.artist_label.set_visible(true);
        }
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_identity_cell(list_item)
        {
            cell.title.set_text("");
            cell.artist_button_label.set_text("");
            cell.artist_label.set_text("");
            cell.artist_button.set_visible(false);
            cell.artist_label.set_visible(false);
            cell.artist_hover_text.borrow_mut().clear();
            *cell.artist_route.borrow_mut() = None;
            *cell.current_track.borrow_mut() = None;
            cell.cover.bind_image(0, None);
        }
    });

    factory.connect_teardown(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            remove_track_identity_cell(list_item);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr("Title")), Some(factory));
    column.set_fixed_width(320);
    column.set_resizable(false);
    column
}
pub(in crate::ui) fn track_link_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> (String, Option<Route>) + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let route = Rc::new(RefCell::new(None::<Route>));
        let hover_text = Rc::new(RefCell::new(String::new()));

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Fill);
        root.set_hexpand(true);

        let button_label = gtk::Label::new(None);
        button_label.add_css_class("table-link-label");
        button_label.set_xalign(0.5);
        button_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        button_label.set_halign(gtk::Align::Fill);
        button_label.set_hexpand(true);
        button_label.set_width_chars(1);
        button_label.set_max_width_chars((width / 8).clamp(8, 32));

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("table-link");
        button.set_halign(gtk::Align::Fill);
        button.set_hexpand(true);
        button.set_cursor_from_name(Some("pointer"));
        add_stateful_link_hover(button.upcast_ref(), &button_label, Rc::clone(&hover_text));
        button.set_child(Some(&button_label));
        button.set_visible(false);
        root.append(&button);

        let label = gtk::Label::new(None);
        label.add_css_class("table-link-label");
        label.set_xalign(0.5);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_width_chars(1);
        label.set_max_width_chars((width / 8).clamp(8, 32));
        label.set_visible(false);
        root.append(&label);

        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&route);
        button.connect_clicked(move |_| {
            if let Some(route) = route_for_click.borrow().clone() {
                click_shell.navigate(route);
            }
        });

        install_dynamic_track_context_menu(&root, &setup_shell, Rc::clone(&current_track));
        list_item.set_child(Some(&root));
        store_track_link_cell(
            list_item,
            TrackLinkCell {
                button,
                button_label,
                label,
                route,
                hover_text,
                current_track,
            },
        );
    });

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
        let Some(cell) = track_link_cell(list_item) else {
            return;
        };
        let track = boxed.borrow::<Track>().clone();
        let (text, route) = value(&track);
        *cell.current_track.borrow_mut() = Some(track);
        if let Some(route) = route {
            *cell.route.borrow_mut() = Some(route);
            *cell.hover_text.borrow_mut() = text.clone();
            cell.button_label.set_text(&text);
            cell.button.set_visible(true);
            cell.label.set_visible(false);
        } else {
            *cell.route.borrow_mut() = None;
            cell.hover_text.borrow_mut().clear();
            cell.label.set_text(&text);
            cell.button.set_visible(false);
            cell.label.set_visible(true);
        }
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_link_cell(list_item)
        {
            cell.button_label.set_text("");
            cell.label.set_text("");
            cell.button.set_visible(false);
            cell.label.set_visible(false);
            cell.hover_text.borrow_mut().clear();
            *cell.route.borrow_mut() = None;
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            remove_track_link_cell(list_item);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}
pub(in crate::ui) fn track_artist_route(track: &Track) -> Option<Route> {
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
pub(in crate::ui) fn album_artist_route(album: &Album) -> Option<Route> {
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
pub(in crate::ui) fn install_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    track: Track,
) {
    install_dynamic_track_context_menu(target, shell, Rc::new(RefCell::new(Some(track))));
}
pub(in crate::ui) fn install_playlist_entry_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    track: Track,
    playlist_id: PlaylistId,
    entry_id: String,
    title: String,
) {
    let remove_action = PlaylistEntryContextMenuAction {
        playlist_id,
        entry_id,
        title,
    };
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_track = track.clone();
    let click_remove_action = remove_action.clone();
    let click = context_click_gesture();
    click.connect_pressed(move |click, _, x, y| {
        claim_context_click(click);
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        present_track_menu(
            &target,
            &click_shell,
            context_track(&click_shell, &click_track),
            click_remove_action.clone(),
            Some((x, y)),
        );
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key_track = track;
    let key_remove_action = remove_action;
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            present_track_menu(
                &target,
                &key_shell,
                context_track(&key_shell, &key_track),
                key_remove_action.clone(),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
pub(in crate::ui) fn install_dynamic_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    track: Rc<RefCell<Option<Track>>>,
) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_track = Rc::clone(&track);
    let click = context_click_gesture();
    click.connect_pressed(move |click, _, x, y| {
        claim_context_click(click);
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        let Some(track) = click_track.borrow().clone() else {
            return;
        };
        present_track_context_menu(
            &target,
            &click_shell,
            context_track(&click_shell, &track),
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
        if let Some(target) = target_weak.upgrade()
            && let Some(track) = key_track.borrow().clone()
        {
            present_track_context_menu(
                &target,
                &key_shell,
                context_track(&key_shell, &track),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
pub(in crate::ui) fn install_album_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    album: Album,
) {
    install_dynamic_album_context_menu(target, shell, Rc::new(RefCell::new(Some(album))));
}
pub(in crate::ui) fn install_dynamic_album_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    album: Rc<RefCell<Option<Album>>>,
) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_album = Rc::clone(&album);
    let click = context_click_gesture();
    click.connect_pressed(move |click, _, x, y| {
        claim_context_click(click);
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        let Some(album) = click_album.borrow().clone() else {
            return;
        };
        present_album_context_menu(
            &target,
            &click_shell,
            context_album(&click_shell, &album),
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
        if let Some(target) = target_weak.upgrade()
            && let Some(album) = key_album.borrow().clone()
        {
            present_album_context_menu(
                &target,
                &key_shell,
                context_album(&key_shell, &album),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
pub(in crate::ui) fn install_playlist_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    playlist: Playlist,
) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_playlist = playlist.clone();
    let click = context_click_gesture();
    click.connect_pressed(move |click, _, x, y| {
        claim_context_click(click);
        if let Some(target) = target_weak.upgrade() {
            present_playlist_context_menu(
                &target,
                &click_shell,
                click_playlist.clone(),
                Some((x, y)),
            );
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
        if let Some(target) = target_weak.upgrade() {
            present_playlist_context_menu(&target, &key_shell, playlist.clone(), None);
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
pub(in crate::ui) fn install_smart_playlist_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    playlist: SmartPlaylist,
) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_playlist = playlist.clone();
    let click = context_click_gesture();
    click.connect_pressed(move |click, _, x, y| {
        claim_context_click(click);
        if let Some(target) = target_weak.upgrade() {
            present_smart_playlist_context_menu(
                &target,
                &click_shell,
                click_playlist.clone(),
                Some((x, y)),
            );
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
        if let Some(target) = target_weak.upgrade() {
            present_smart_playlist_context_menu(&target, &key_shell, playlist.clone(), None);
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}
pub(in crate::ui) fn install_artist_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    artist: Artist,
) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_artist = artist.clone();
    let click = context_click_gesture();
    click.connect_pressed(move |click, _, x, y| {
        claim_context_click(click);
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
pub(in crate::ui) fn install_current_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click = context_click_gesture();
    click.connect_pressed(move |click, _, x, y| {
        claim_context_click(click);
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

fn context_click_gesture() -> gtk::GestureClick {
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click
}

fn claim_context_click(click: &gtk::GestureClick) {
    click.set_state(gtk::EventSequenceState::Claimed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_scroller_wins_before_bounds() {
        let wrapper = ScrollerScore {
            owner: false,
            vertical: false,
            range: 0.0,
            depth: 1,
        };
        let route = ScrollerScore {
            owner: false,
            vertical: true,
            range: 0.0,
            depth: 2,
        };

        assert!(scroller_score_is_better(route, wrapper));
    }

    #[test]
    fn route_owner_wins_over_nested_scroller() {
        let nested = ScrollerScore {
            owner: false,
            vertical: true,
            range: 2000.0,
            depth: 5,
        };
        let route = ScrollerScore {
            owner: true,
            vertical: true,
            range: 0.0,
            depth: 2,
        };

        assert!(scroller_score_is_better(route, nested));
    }
}
