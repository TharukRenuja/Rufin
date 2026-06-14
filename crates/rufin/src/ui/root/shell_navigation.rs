use super::*;
use crate::ui::root::library::configure_library_route_scroller;

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
pub(in crate::ui) fn route_boundary_for_route(
    route: &Route,
    view: gtk::Widget,
    content_width: i32,
) -> gtk::Widget {
    route_boundary_from_spec(view, route_boundary_spec_for_route(route), content_width)
}
pub(in crate::ui) fn apply_route_boundary_width(boundary: &gtk::Widget, content_width: i32) {
    let width = content_width.max(1);
    boundary.set_width_request(width);
    if let Some(scroller) = boundary.downcast_ref::<gtk::ScrolledWindow>() {
        scroller.set_max_content_width(-1);
        scroller.set_min_content_width(width);
        scroller.set_max_content_width(width);
    }
}
fn route_boundary_from_spec(
    view: gtk::Widget,
    spec: RouteBoundarySpec,
    content_width: i32,
) -> gtk::Widget {
    let scroller = gtk::ScrolledWindow::new();
    if spec.vertical_policy != gtk::PolicyType::Never {
        mark_route_scroll_owner(&scroller);
    }
    scroller.set_policy(spec.horizontal_policy, spec.vertical_policy);
    scroller.set_overflow(spec.overflow);
    scroller.set_min_content_width(spec.min_content_width);
    scroller.set_max_content_width(1);
    scroller.set_width_request(1);
    scroller.set_propagate_natural_width(spec.propagate_natural_width);
    scroller.set_propagate_natural_height(false);
    scroller.set_hexpand(spec.hexpand);
    scroller.set_vexpand(spec.vexpand);
    scroller.set_child(Some(&view));
    let boundary = scroller.upcast::<gtk::Widget>();
    apply_route_boundary_width(&boundary, content_width);
    boundary
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
        horizontal_policy: gtk::PolicyType::Never,
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

pub(in crate::ui) fn detail_route_scroller(shell: &Rc<Shell>, content: gtk::Widget) -> gtk::Widget {
    let scroller = gtk::ScrolledWindow::new();
    mark_route_scroll_owner(&scroller);
    configure_library_route_scroller(shell, &scroller);
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_overlay_scrolling(true);
    scroller.set_child(Some(&content));
    scroller.upcast()
}

pub(in crate::ui) fn detail_route_wrapper(spacing: i32) -> gtk::Box {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    wrapper.add_css_class("route-content");
    wrapper.set_hexpand(true);
    wrapper.set_halign(gtk::Align::Fill);
    wrapper.set_width_request(1);
    wrapper.set_vexpand(true);
    wrapper
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
pub(in crate::ui) struct TrackLinkCell {
    button: gtk::Button,
    button_label: gtk::Label,
    label: gtk::Label,
    route: Rc<RefCell<Option<Route>>>,
    hover_text: Rc<RefCell<String>>,
    current_track: Rc<RefCell<Option<Track>>>,
}

thread_local! {
    static TRACK_LINK_CELLS: RefCell<HashMap<usize, TrackLinkCell>> = RefCell::new(HashMap::new());
}

pub(in crate::ui) fn list_item_storage_key(list_item: &gtk::ListItem) -> usize {
    list_item.as_ptr() as usize
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
        button_label.set_xalign(0.0);
        button_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        button_label.set_halign(gtk::Align::Start);
        button_label.set_hexpand(false);
        button_label.set_width_chars(1);
        button_label.set_max_width_chars((width / 8).clamp(8, 32));

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("table-link");
        button.set_halign(gtk::Align::Start);
        button.set_hexpand(false);
        button.set_cursor_from_name(Some("pointer"));
        add_stateful_link_hover(button.upcast_ref(), &button_label, Rc::clone(&hover_text));
        button.set_child(Some(&button_label));
        button.set_visible(false);
        root.append(&button);

        let label = gtk::Label::new(None);
        label.add_css_class("table-link-label");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(false);
        label.set_width_chars(1);
        label.set_max_width_chars((width / 8).clamp(8, 32));
        label.set_visible(false);
        root.append(&label);

        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&route);
        button.connect_clicked(move |_| {
            let route = route_for_click.borrow().clone();
            if let Some(route) = route {
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
pub(in crate::ui) fn present_current_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
) {
    let target = target.as_ref();
    if let Some(track) = current_player_track(shell) {
        present_track_context_menu(target, shell, track, None);
    }
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
