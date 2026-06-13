use super::*;

pub(in crate::ui) fn sort_genres(genres: &mut [Genre], settings: &LibraryListSettings) {
    genres.sort_by(|left, right| {
        apply_desc(
            compare_genre(left, right, settings.sort_key),
            settings.descending,
        )
    });
}
pub(in crate::ui) fn sort_playlists(playlists: &mut [Playlist], settings: &LibraryListSettings) {
    playlists.sort_by(|left, right| {
        apply_desc(
            compare_playlist(left, right, settings.sort_key),
            settings.descending,
        )
    });
}
pub(in crate::ui) fn sort_smart_playlists(
    playlists: &mut [SmartPlaylist],
    settings: &LibraryListSettings,
) {
    playlists.sort_by(|left, right| {
        apply_desc(
            compare_smart_playlist(left, right, settings.sort_key),
            settings.descending,
        )
    });
}
pub(in crate::ui) fn sort_tracks(
    tracks: &mut [Track],
    settings: &LibraryListSettings,
    favorite_first: bool,
) {
    tracks.sort_by(|left, right| {
        if favorite_first {
            let favorite = right.favorite.cmp(&left.favorite);
            if favorite != Ordering::Equal {
                return favorite;
            }
        }
        let missing = track_field_missing(left, settings.sort_key)
            .cmp(&track_field_missing(right, settings.sort_key));
        if missing != Ordering::Equal {
            return missing;
        }
        apply_desc(
            compare_track(left, right, settings.sort_key),
            settings.descending,
        )
    });
}
pub(in crate::ui) fn compare_album(left: &Album, right: &Album, field: LibraryField) -> Ordering {
    match field {
        LibraryField::AlbumArtist => cmp_string(&left.artist, &right.artist),
        LibraryField::Year => left.year.cmp(&right.year),
        LibraryField::ReleaseDate => cmp_option_string(&left.release_date, &right.release_date),
        LibraryField::DateAdded => cmp_option_string(&left.date_added, &right.date_added),
        LibraryField::LastPlayed => cmp_option_string(&left.last_played, &right.last_played),
        LibraryField::PlayCount => cmp_option_u32(left.play_count, right.play_count),
        LibraryField::UserRating => cmp_option_u8(left.user_rating, right.user_rating),
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        LibraryField::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => cmp_string(&left.title, &right.title),
    }
    .then_with(|| cmp_string(&left.title, &right.title))
}
pub(in crate::ui) fn compare_artist(
    left: &Artist,
    right: &Artist,
    field: LibraryField,
) -> Ordering {
    match field {
        LibraryField::AlbumCount => left.album_count.cmp(&right.album_count),
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        LibraryField::LastPlayed => cmp_option_string(&left.last_played, &right.last_played),
        LibraryField::PlayCount => cmp_option_u32(left.play_count, right.play_count),
        LibraryField::UserRating => cmp_option_u8(left.user_rating, right.user_rating),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => cmp_string(&left.name, &right.name),
    }
    .then_with(|| cmp_string(&left.name, &right.name))
}
pub(in crate::ui) fn compare_genre(left: &Genre, right: &Genre, field: LibraryField) -> Ordering {
    match field {
        LibraryField::AlbumCount => left.album_count.cmp(&right.album_count),
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        _ => cmp_string(&left.name, &right.name),
    }
    .then_with(|| cmp_string(&left.name, &right.name))
}
pub(in crate::ui) fn compare_playlist(
    left: &Playlist,
    right: &Playlist,
    field: LibraryField,
) -> Ordering {
    match field {
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        LibraryField::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        _ => cmp_string(&left.name, &right.name),
    }
    .then_with(|| cmp_string(&left.name, &right.name))
}
pub(in crate::ui) fn compare_smart_playlist(
    left: &SmartPlaylist,
    right: &SmartPlaylist,
    field: LibraryField,
) -> Ordering {
    match field {
        LibraryField::RowIndex => left.position.cmp(&right.position),
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        LibraryField::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        _ => cmp_string(&left.name, &right.name),
    }
    .then_with(|| cmp_string(&left.name, &right.name))
}
pub(in crate::ui) fn compare_track(left: &Track, right: &Track, field: LibraryField) -> Ordering {
    match field {
        LibraryField::TrackNumber => left
            .disc_number
            .cmp(&right.disc_number)
            .then(left.track_number.cmp(&right.track_number)),
        LibraryField::Artist => cmp_string(&left.artist, &right.artist),
        LibraryField::AlbumArtist => cmp_string(
            &joined_credits(&left.album_artist_credits),
            &joined_credits(&right.album_artist_credits),
        ),
        LibraryField::Album => cmp_string(&left.album, &right.album),
        LibraryField::Year => left.year.cmp(&right.year),
        LibraryField::ReleaseDate => cmp_option_string(&left.release_date, &right.release_date),
        LibraryField::DateAdded => cmp_option_string(&left.date_added, &right.date_added),
        LibraryField::LastPlayed => cmp_option_string(&left.last_played, &right.last_played),
        LibraryField::PlayCount => cmp_option_u32(left.play_count, right.play_count),
        LibraryField::UserRating => cmp_option_u8(left.user_rating, right.user_rating),
        LibraryField::Genre => cmp_string(&left.genres.join(", "), &right.genres.join(", ")),
        LibraryField::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => cmp_string(&left.title, &right.title),
    }
    .then_with(|| cmp_string(&left.album, &right.album))
    .then(left.disc_number.cmp(&right.disc_number))
    .then(left.track_number.cmp(&right.track_number))
    .then_with(|| cmp_string(&left.title, &right.title))
}
pub(in crate::ui) fn album_field_missing(album: &Album, field: LibraryField) -> bool {
    match field {
        LibraryField::ReleaseDate => album.release_date.is_none(),
        LibraryField::DateAdded => album.date_added.is_none(),
        LibraryField::LastPlayed => album.last_played.is_none(),
        LibraryField::PlayCount => album.play_count.is_none(),
        LibraryField::UserRating => album.user_rating.is_none(),
        _ => false,
    }
}
pub(in crate::ui) fn artist_field_missing(artist: &Artist, field: LibraryField) -> bool {
    match field {
        LibraryField::LastPlayed => artist.last_played.is_none(),
        LibraryField::PlayCount => artist.play_count.is_none(),
        LibraryField::UserRating => artist.user_rating.is_none(),
        _ => false,
    }
}
pub(in crate::ui) fn track_field_missing(track: &Track, field: LibraryField) -> bool {
    match field {
        LibraryField::ReleaseDate => track.release_date.is_none(),
        LibraryField::DateAdded => track.date_added.is_none(),
        LibraryField::LastPlayed => track.last_played.is_none(),
        LibraryField::PlayCount => track.play_count.is_none(),
        LibraryField::UserRating => track.user_rating.is_none(),
        _ => false,
    }
}
pub(in crate::ui) fn album_field(album: &Album, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => album.title.clone(),
        LibraryField::AlbumArtist | LibraryField::Artist => album.artist.clone(),
        LibraryField::Year => nonzero_year(album.year),
        LibraryField::ReleaseDate => album.release_date.clone().unwrap_or_default(),
        LibraryField::DateAdded => album.date_added.clone().unwrap_or_default(),
        LibraryField::LastPlayed => album.last_played.clone().unwrap_or_default(),
        LibraryField::PlayCount => option_count(album.play_count),
        LibraryField::UserRating => option_rating(album.user_rating),
        LibraryField::Genre => album.genres.join(", "),
        LibraryField::SongCount => format!("{} {}", album.track_count, tr("tracks")),
        LibraryField::Duration => format_duration(album.duration_seconds),
        LibraryField::Favorite => favorite_text(album.favorite),
        _ => String::new(),
    }
}
pub(in crate::ui) fn artist_field(artist: &Artist, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => artist.name.clone(),
        LibraryField::AlbumCount => format!("{} {}", artist.album_count, tr("albums")),
        LibraryField::SongCount => format!("{} {}", artist.track_count, tr("tracks")),
        LibraryField::LastPlayed => artist.last_played.clone().unwrap_or_default(),
        LibraryField::PlayCount => option_count(artist.play_count),
        LibraryField::UserRating => option_rating(artist.user_rating),
        LibraryField::Favorite => favorite_text(artist.favorite),
        _ => String::new(),
    }
}
pub(in crate::ui) fn genre_field(genre: &Genre, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => genre.name.clone(),
        LibraryField::AlbumCount => format!("{} {}", genre.album_count, tr("albums")),
        LibraryField::SongCount => format!("{} {}", genre.track_count, tr("tracks")),
        _ => String::new(),
    }
}
pub(in crate::ui) fn playlist_field(playlist: &Playlist, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => playlist.name.clone(),
        LibraryField::SongCount => format!("{} {}", playlist.track_count, tr("tracks")),
        LibraryField::Duration => format_duration(playlist.duration_seconds),
        _ => String::new(),
    }
}
pub(in crate::ui) fn smart_playlist_field(playlist: &SmartPlaylist, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => playlist.name.clone(),
        LibraryField::SongCount if playlist.track_count > 0 => {
            format!("{} {}", playlist.track_count, tr("tracks"))
        }
        LibraryField::Duration if playlist.duration_seconds > 0 => {
            format_duration(playlist.duration_seconds)
        }
        _ => String::new(),
    }
}
pub(in crate::ui) fn track_field(track: &Track, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => track.title.clone(),
        LibraryField::Artist => track.artist.clone(),
        LibraryField::AlbumArtist => joined_credits(&track.album_artist_credits),
        LibraryField::Album => track.album.clone(),
        LibraryField::Year => nonzero_year(track.year),
        LibraryField::ReleaseDate => track.release_date.clone().unwrap_or_default(),
        LibraryField::DateAdded => track.date_added.clone().unwrap_or_default(),
        LibraryField::LastPlayed => track.last_played.clone().unwrap_or_default(),
        LibraryField::PlayCount => option_count(track.play_count),
        LibraryField::UserRating => option_rating(track.user_rating),
        LibraryField::Genre => track.genres.join(", "),
        LibraryField::DiscNumber => track.disc_number.to_string(),
        LibraryField::TrackNumber => format!("{}-{:02}", track.disc_number, track.track_number),
        LibraryField::Duration => format_duration(track.duration_seconds),
        LibraryField::Favorite => favorite_text(track.favorite),
        _ => String::new(),
    }
}
pub(in crate::ui) fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || joined_credits(&track.album_artist_credits)
            .to_lowercase()
            .contains(query)
        || track.album.to_lowercase().contains(query)
        || track.genres.join(" ").to_lowercase().contains(query)
        || track.year.to_string().contains(query)
}
pub(in crate::ui) fn album_matches_query(album: &Album, query: &str) -> bool {
    album.title.to_lowercase().contains(query)
        || album.artist.to_lowercase().contains(query)
        || album.genres.join(" ").to_lowercase().contains(query)
        || album.year.to_string().contains(query)
}
pub(in crate::ui) fn artist_matches_query(artist: &Artist, query: &str) -> bool {
    artist.name.to_lowercase().contains(query)
}
pub(in crate::ui) fn genre_matches_query(genre: &Genre, query: &str) -> bool {
    genre.name.to_lowercase().contains(query)
}
pub(in crate::ui) fn playlist_matches_query(playlist: &Playlist, query: &str) -> bool {
    playlist.name.to_lowercase().contains(query)
}
pub(in crate::ui) fn smart_playlist_matches_query(playlist: &SmartPlaylist, query: &str) -> bool {
    playlist.name.to_lowercase().contains(query)
}
pub(in crate::ui) fn item_at<T: Clone + 'static>(
    model: &gio::ListStore,
    position: u32,
) -> Option<T> {
    model
        .item(position)
        .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        .map(|boxed| boxed.borrow::<T>().clone())
}
pub(in crate::ui) fn item_at_from_item<T: Clone + 'static>(item: &gtk::ListItem) -> Option<T> {
    item.item()
        .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        .map(|boxed| boxed.borrow::<T>().clone())
}
pub(in crate::ui) fn clear_list_item_child(_: &gtk::SignalListItemFactory, item: &glib::Object) {
    if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
        item.set_child(None::<&gtk::Widget>);
    }
}
pub(in crate::ui) fn replace_tracks_in_model(model: &gio::ListStore, tracks: Vec<Track>) {
    let additions = tracks
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
pub(in crate::ui) fn replace_album_items(model: &gio::ListStore, rows: Vec<AlbumDetailItem>) {
    let additions = rows
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}
pub(in crate::ui) fn append_album_items(model: &gio::ListStore, rows: Vec<AlbumDetailItem>) {
    let additions = rows
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(model.n_items(), 0, &additions);
}
pub(in crate::ui) const COLLECTION_GRID_CARD_GAP: i32 = 6;
pub(in crate::ui) const COLLECTION_GRID_TITLE_LINES: i32 = 2;
pub(in crate::ui) const COLLECTION_GRID_FIELD_LINES: i32 = 1;
const COLLECTION_GRID_LABEL_LINE_HEIGHT: i32 = 20;

fn collection_grid_label_height(lines: i32) -> i32 {
    lines
        .max(1)
        .saturating_mul(COLLECTION_GRID_LABEL_LINE_HEIGHT)
}

pub(in crate::ui) fn collection_grid_card_height(size: i32, field_count: usize) -> i32 {
    let size = size.max(1);
    let label_count = field_count.saturating_add(1);
    let label_count = label_count.min(i32::MAX as usize) as i32;
    let field_count = field_count.min(i32::MAX as usize) as i32;
    size.saturating_add(label_count.saturating_mul(COLLECTION_GRID_CARD_GAP))
        .saturating_add(collection_grid_label_height(COLLECTION_GRID_TITLE_LINES))
        .saturating_add(
            field_count.saturating_mul(collection_grid_label_height(COLLECTION_GRID_FIELD_LINES)),
        )
}

pub(in crate::ui) fn center_label(
    text: &str,
    css_class: &str,
    width: i32,
    lines: i32,
) -> gtk::Widget {
    let width = width.max(1);
    let lines = lines.max(1);
    let height = collection_grid_label_height(lines);
    let label = gtk::Label::new(Some(text));
    if !css_class.is_empty() {
        label.add_css_class(css_class);
    }
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_lines(lines);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(1);
    label.set_max_width_chars((width / 8).clamp(8, 32));
    label.set_size_request(width, height);
    if !text.is_empty() {
        label.set_tooltip_text(Some(text));
    }

    let clip = gtk::Box::new(gtk::Orientation::Vertical, 0);
    clip.add_css_class("card-label-clip");
    clip.set_overflow(gtk::Overflow::Hidden);
    clip.set_size_request(width, height);
    clip.set_width_request(width);
    clip.set_height_request(height);
    clip.set_hexpand(false);
    clip.set_halign(gtk::Align::Center);
    clip.append(&label);
    clip.upcast()
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct AlbumDetailMetaLabelSpec {
    pub(in crate::ui) width: i32,
    pub(in crate::ui) height: i32,
    pub(in crate::ui) horizontal_policy: gtk::PolicyType,
    pub(in crate::ui) vertical_policy: gtk::PolicyType,
    pub(in crate::ui) overflow: gtk::Overflow,
    pub(in crate::ui) propagate_natural_width: bool,
    pub(in crate::ui) propagate_natural_height: bool,
    pub(in crate::ui) wrap: bool,
}
pub(in crate::ui) fn album_detail_meta_label_spec(
    width: i32,
    title: bool,
) -> AlbumDetailMetaLabelSpec {
    AlbumDetailMetaLabelSpec {
        width,
        height: if title {
            ALBUM_DETAIL_META_LABEL_HEIGHT * 2
        } else {
            ALBUM_DETAIL_META_LABEL_HEIGHT
        },
        horizontal_policy: gtk::PolicyType::Never,
        vertical_policy: gtk::PolicyType::Never,
        overflow: gtk::Overflow::Hidden,
        propagate_natural_width: false,
        propagate_natural_height: false,
        wrap: title,
    }
}
pub(in crate::ui) fn album_detail_meta_label(
    text: &str,
    css_class: &str,
    width: i32,
) -> gtk::Widget {
    let spec = album_detail_meta_label_spec(width, css_class == "track-title");
    let label = gtk::Label::new(Some(text));
    if !css_class.is_empty() {
        label.add_css_class(css_class);
    }
    label.set_xalign(0.5);
    label.set_wrap(spec.wrap);
    if spec.wrap {
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_lines(2);
        label.set_single_line_mode(false);
    } else {
        label.set_single_line_mode(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    label.set_width_chars(1);
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(false);

    let clip = gtk::ScrolledWindow::new();
    clip.add_css_class("card-label-clip");
    clip.set_policy(spec.horizontal_policy, spec.vertical_policy);
    clip.set_overflow(spec.overflow);
    clip.set_width_request(spec.width);
    clip.set_height_request(spec.height);
    clip.set_size_request(spec.width, spec.height);
    clip.set_min_content_width(spec.width);
    clip.set_max_content_width(spec.width);
    clip.set_min_content_height(spec.height);
    clip.set_max_content_height(spec.height);
    clip.set_propagate_natural_width(spec.propagate_natural_width);
    clip.set_propagate_natural_height(spec.propagate_natural_height);
    clip.set_hexpand(false);
    clip.set_child(Some(&label));
    clip.upcast()
}
pub(in crate::ui) fn album_fact_text(album: &Album) -> String {
    format!(
        "{} • {} {} • {}",
        nonzero_year(album.year),
        album.track_count,
        tr("tracks"),
        format_duration(album.duration_seconds)
    )
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::ui) enum LibraryFieldSet {
    Row,
    Grid,
    Detail,
}
pub(in crate::ui) fn populate_library_field_rows(
    shell: &Rc<Shell>,
    key: LibraryListKey,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }

    let settings = shell.library_settings(key);
    let field_set = field_set_for_layout(settings.layout);
    group.set_title(&tr(field_group_title(field_set)));

    let active = active_fields_for_set(&settings, field_set).to_vec();
    let mut order = active.clone();
    for field in available_fields_for_set(key, field_set) {
        if !order.contains(field) {
            order.push(*field);
        }
    }
    for field in order {
        let row = library_field_config_row(shell, key, field_set, field, &active, group, rows);
        group.add(&row);
        rows.borrow_mut().push(row);
    }
}
pub(in crate::ui) fn library_field_config_row(
    shell: &Rc<Shell>,
    key: LibraryListKey,
    field_set: LibraryFieldSet,
    field: LibraryField,
    active: &[LibraryField],
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
) -> adw::ActionRow {
    let enabled = active.contains(&field);
    let row = adw::ActionRow::builder()
        .title(tr(field.title()))
        .subtitle(if enabled { tr("Visible") } else { tr("Hidden") })
        .build();

    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    row.add_prefix(&drag);

    let check = gtk::CheckButton::new();
    check.set_active(enabled);
    check.set_sensitive(can_toggle_field(active, field_set, field));
    check.set_valign(gtk::Align::Center);
    row.add_prefix(&check);
    row.set_activatable_widget(Some(&check));

    let up = gtk::Button::from_icon_name("go-up-symbolic");
    up.add_css_class("flat");
    up.set_tooltip_text(Some(&tr("Move up")));
    up.set_valign(gtk::Align::Center);
    up.set_sensitive(enabled);
    row.add_suffix(&up);

    let down = gtk::Button::from_icon_name("go-down-symbolic");
    down.add_css_class("flat");
    down.set_tooltip_text(Some(&tr("Move down")));
    down.set_valign(gtk::Align::Center);
    down.set_sensitive(enabled);
    row.add_suffix(&down);

    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        check.connect_toggled(move |check| {
            shell.update_library_list_settings(key, |settings| {
                set_field_enabled(settings, key, field_set, field, check.is_active());
            });
            populate_library_field_rows(&shell, key, &group, &rows);
            shell.render_current_route_preserving_scroll();
        });
    }
    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        up.connect_clicked(move |_| {
            shell.update_library_list_settings(key, |settings| {
                move_visible_field(settings, field_set, field, -1);
            });
            populate_library_field_rows(&shell, key, &group, &rows);
            shell.render_current_route_preserving_scroll();
        });
    }
    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        down.connect_clicked(move |_| {
            shell.update_library_list_settings(key, |settings| {
                move_visible_field(settings, field_set, field, 1);
            });
            populate_library_field_rows(&shell, key, &group, &rows);
            shell.render_current_route_preserving_scroll();
        });
    }

    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let field_id = library_field_drag_id(field).to_string();
    source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&field_id.to_value()))
    });
    drag.add_controller(source);

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let shell = Rc::clone(shell);
    let group = group.clone();
    let rows = Rc::clone(rows);
    let row_for_drop = row.clone();
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(source_id) = value.get::<String>() else {
            return false;
        };
        let Some(source_field) = library_field_from_drag_id(&source_id) else {
            return false;
        };
        if source_field == field {
            return false;
        }
        let after = y > f64::from(row_for_drop.height()) / 2.0;
        shell.update_library_list_settings(key, |settings| {
            reorder_visible_field(settings, field_set, source_field, field, after);
        });
        populate_library_field_rows(&shell, key, &group, &rows);
        shell.render_current_route_preserving_scroll();
        true
    });
    row.add_controller(drop_target);

    row
}
pub(in crate::ui) fn layout_button_content(layout: LibraryLayout) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(10);
    content.set_margin_end(10);
    content.append(&gtk::Image::from_icon_name(layout_icon(layout)));
    content.append(&gtk::Label::new(Some(&tr(layout_title(layout)))));
    content.upcast()
}
pub(in crate::ui) fn sync_layout_buttons(
    buttons: &Rc<RefCell<Vec<(LibraryLayout, gtk::ToggleButton)>>>,
    active_layout: LibraryLayout,
) {
    for (layout, button) in buttons.borrow().iter() {
        button.set_active(*layout == active_layout);
    }
}
pub(in crate::ui) fn supported_layouts(key: LibraryListKey) -> Vec<LibraryLayout> {
    let mut layouts = vec![LibraryLayout::Row, LibraryLayout::Grid];
    if key.supports_layout(LibraryLayout::Detail) {
        layouts.push(LibraryLayout::Detail);
    }
    layouts
}
pub(in crate::ui) fn field_group_title(field_set: LibraryFieldSet) -> &'static str {
    match field_set {
        LibraryFieldSet::Row => "Columns",
        LibraryFieldSet::Grid => "Grid labels",
        LibraryFieldSet::Detail => "Detail track columns",
    }
}
pub(in crate::ui) fn field_set_for_layout(layout: LibraryLayout) -> LibraryFieldSet {
    match layout {
        LibraryLayout::Grid => LibraryFieldSet::Grid,
        LibraryLayout::Detail => LibraryFieldSet::Detail,
        LibraryLayout::Row => LibraryFieldSet::Row,
    }
}
pub(in crate::ui) fn active_fields_for_set(
    settings: &LibraryListSettings,
    field_set: LibraryFieldSet,
) -> &[LibraryField] {
    match field_set {
        LibraryFieldSet::Grid => &settings.grid_fields,
        LibraryFieldSet::Detail => &settings.detail_track_fields,
        LibraryFieldSet::Row => &settings.row_fields,
    }
}
pub(in crate::ui) fn active_fields_for_set_mut(
    settings: &mut LibraryListSettings,
    field_set: LibraryFieldSet,
) -> &mut Vec<LibraryField> {
    match field_set {
        LibraryFieldSet::Grid => &mut settings.grid_fields,
        LibraryFieldSet::Detail => &mut settings.detail_track_fields,
        LibraryFieldSet::Row => &mut settings.row_fields,
    }
}
pub(in crate::ui) fn available_fields_for_set(
    key: LibraryListKey,
    field_set: LibraryFieldSet,
) -> &'static [LibraryField] {
    match field_set {
        LibraryFieldSet::Grid => domain::available_grid_fields(key),
        LibraryFieldSet::Detail => domain::available_detail_track_fields(),
        LibraryFieldSet::Row => domain::available_row_fields(key),
    }
}
pub(in crate::ui) fn set_field_enabled(
    settings: &mut LibraryListSettings,
    key: LibraryListKey,
    field_set: LibraryFieldSet,
    field: LibraryField,
    enabled: bool,
) {
    let order = available_fields_for_set(key, field_set).to_vec();
    let fields = active_fields_for_set_mut(settings, field_set);
    if enabled {
        insert_field_in_order(fields, field, &order);
    } else {
        fields.retain(|candidate| *candidate != field);
    }
}
pub(in crate::ui) fn insert_field_in_order(
    fields: &mut Vec<LibraryField>,
    field: LibraryField,
    order: &[LibraryField],
) {
    if fields.contains(&field) {
        return;
    }
    let target_order = order
        .iter()
        .position(|candidate| *candidate == field)
        .unwrap_or(usize::MAX);
    let insert_at = fields
        .iter()
        .position(|candidate| {
            order
                .iter()
                .position(|ordered| ordered == candidate)
                .unwrap_or(usize::MAX)
                > target_order
        })
        .unwrap_or(fields.len());
    fields.insert(insert_at, field);
}
pub(in crate::ui) fn move_visible_field(
    settings: &mut LibraryListSettings,
    field_set: LibraryFieldSet,
    field: LibraryField,
    delta: isize,
) {
    let fields = active_fields_for_set_mut(settings, field_set);
    let Some(index) = fields.iter().position(|candidate| *candidate == field) else {
        return;
    };
    let new_index = if delta < 0 {
        index.saturating_sub(1)
    } else {
        (index + 1).min(fields.len().saturating_sub(1))
    };
    fields.swap(index, new_index);
}
pub(in crate::ui) fn reorder_visible_field(
    settings: &mut LibraryListSettings,
    field_set: LibraryFieldSet,
    source: LibraryField,
    target: LibraryField,
    after: bool,
) {
    let fields = active_fields_for_set_mut(settings, field_set);
    let Some(source_index) = fields.iter().position(|field| *field == source) else {
        return;
    };
    let field = fields.remove(source_index);
    let Some(mut target_index) = fields.iter().position(|field| *field == target) else {
        fields.insert(source_index.min(fields.len()), field);
        return;
    };
    if after {
        target_index += 1;
    }
    fields.insert(target_index.min(fields.len()), field);
}
pub(in crate::ui) fn can_toggle_field(
    active: &[LibraryField],
    field_set: LibraryFieldSet,
    field: LibraryField,
) -> bool {
    if !active.contains(&field) {
        return true;
    }
    if field_set == LibraryFieldSet::Grid {
        return true;
    }
    !row_field_is_usable(field)
        || active
            .iter()
            .filter(|field| row_field_is_usable(**field))
            .count()
            > 1
}
pub(in crate::ui) fn row_field_is_usable(field: LibraryField) -> bool {
    !matches!(
        field,
        LibraryField::RowIndex
            | LibraryField::Image
            | LibraryField::TrackNumber
            | LibraryField::DiscNumber
            | LibraryField::Favorite
    )
}
pub(in crate::ui) fn library_field_drag_id(field: LibraryField) -> &'static str {
    match field {
        LibraryField::RowIndex => "RowIndex",
        LibraryField::Image => "Image",
        LibraryField::Title => "Title",
        LibraryField::TitleMerged => "TitleMerged",
        LibraryField::Artist => "Artist",
        LibraryField::AlbumArtist => "AlbumArtist",
        LibraryField::Album => "Album",
        LibraryField::Year => "Year",
        LibraryField::ReleaseDate => "ReleaseDate",
        LibraryField::DateAdded => "DateAdded",
        LibraryField::LastPlayed => "LastPlayed",
        LibraryField::PlayCount => "PlayCount",
        LibraryField::UserRating => "UserRating",
        LibraryField::Genre => "Genre",
        LibraryField::TrackNumber => "TrackNumber",
        LibraryField::DiscNumber => "DiscNumber",
        LibraryField::SongCount => "SongCount",
        LibraryField::AlbumCount => "AlbumCount",
        LibraryField::Duration => "Duration",
        LibraryField::Favorite => "Favorite",
    }
}
pub(in crate::ui) fn library_field_from_drag_id(id: &str) -> Option<LibraryField> {
    [
        LibraryField::RowIndex,
        LibraryField::Image,
        LibraryField::Title,
        LibraryField::TitleMerged,
        LibraryField::Artist,
        LibraryField::AlbumArtist,
        LibraryField::Album,
        LibraryField::Year,
        LibraryField::ReleaseDate,
        LibraryField::DateAdded,
        LibraryField::LastPlayed,
        LibraryField::PlayCount,
        LibraryField::UserRating,
        LibraryField::Genre,
        LibraryField::TrackNumber,
        LibraryField::DiscNumber,
        LibraryField::SongCount,
        LibraryField::AlbumCount,
        LibraryField::Duration,
        LibraryField::Favorite,
    ]
    .into_iter()
    .find(|field| library_field_drag_id(*field) == id)
}
pub(in crate::ui) fn next_layout(key: LibraryListKey, layout: LibraryLayout) -> LibraryLayout {
    if key.supports_layout(LibraryLayout::Detail) {
        match layout {
            LibraryLayout::Grid => LibraryLayout::Detail,
            LibraryLayout::Detail => LibraryLayout::Row,
            LibraryLayout::Row => LibraryLayout::Grid,
        }
    } else {
        match layout {
            LibraryLayout::Grid => LibraryLayout::Row,
            LibraryLayout::Row | LibraryLayout::Detail => LibraryLayout::Grid,
        }
    }
}
pub(in crate::ui) fn layout_icon(layout: LibraryLayout) -> &'static str {
    match layout {
        LibraryLayout::Grid => "view-grid-symbolic",
        LibraryLayout::Row => "view-list-symbolic",
        LibraryLayout::Detail => "view-list-details-symbolic",
    }
}
pub(in crate::ui) fn layout_title(layout: LibraryLayout) -> &'static str {
    match layout {
        LibraryLayout::Grid => "Grid",
        LibraryLayout::Row => "Rows",
        LibraryLayout::Detail => "Detail",
    }
}
pub(in crate::ui) fn column_width(field: LibraryField) -> i32 {
    match field {
        LibraryField::RowIndex => 48,
        LibraryField::Image | LibraryField::Favorite => 56,
        LibraryField::Title | LibraryField::TitleMerged => 220,
        LibraryField::Album
        | LibraryField::Artist
        | LibraryField::AlbumArtist
        | LibraryField::Genre => 220,
        LibraryField::ReleaseDate | LibraryField::DateAdded | LibraryField::LastPlayed => 118,
        LibraryField::PlayCount => play_count_column_width(),
        LibraryField::UserRating | LibraryField::SongCount | LibraryField::AlbumCount => 96,
        LibraryField::Year | LibraryField::DiscNumber | LibraryField::TrackNumber => 68,
        LibraryField::Duration => 76,
    }
}
pub(in crate::ui) fn play_count_column_width() -> i32 {
    compact_header_column_width("Plays", 56)
}
pub(in crate::ui) fn compact_header_column_width(header: &str, min_width: i32) -> i32 {
    let width = tr(header).chars().count().min(i32::MAX as usize / 8) as i32 * 8 + 20;
    width.max(min_width)
}
pub(in crate::ui) fn apply_desc(ordering: Ordering, descending: bool) -> Ordering {
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}
pub(in crate::ui) fn cmp_string(left: &str, right: &str) -> Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}
pub(in crate::ui) fn cmp_option_string(left: &Option<String>, right: &Option<String>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => cmp_string(left, right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
pub(in crate::ui) fn cmp_option_u32(left: Option<u32>, right: Option<u32>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
pub(in crate::ui) fn cmp_option_u8(left: Option<u8>, right: Option<u8>) -> Ordering {
    cmp_option_u32(left.map(u32::from), right.map(u32::from))
}
pub(in crate::ui) fn option_count(value: Option<u32>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
pub(in crate::ui) fn option_rating(value: Option<u8>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
pub(in crate::ui) fn favorite_text(favorite: bool) -> String {
    if favorite { "♥" } else { "" }.to_string()
}
pub(in crate::ui) fn nonzero_year(year: u16) -> String {
    if year == 0 {
        String::new()
    } else {
        year.to_string()
    }
}
pub(in crate::ui) fn joined_credits(credits: &[domain::ArtistCredit]) -> String {
    credits
        .iter()
        .map(|credit| credit.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smart_playlist_with_stats(track_count: u32, duration_seconds: u32) -> SmartPlaylist {
        SmartPlaylist {
            id: domain::SmartPlaylistId::new("smart:test"),
            name: "Smart Mix".to_string(),
            position: 0,
            builtin: None,
            definition: domain::SmartPlaylistDefinition {
                root: domain::SmartPlaylistRuleGroup {
                    mode: domain::SmartPlaylistMatchMode::All,
                    rules: Vec::new(),
                },
                sort_field: domain::SmartPlaylistSortField::Title,
                descending: false,
                limit: None,
            },
            track_count,
            duration_seconds,
            image_refs: Vec::new(),
            image_ref: None,
        }
    }

    #[test]
    fn cards_smart_zeroes() {
        let unresolved = smart_playlist_with_stats(0, 0);
        assert!(smart_playlist_field(&unresolved, LibraryField::SongCount).is_empty());
        assert!(smart_playlist_field(&unresolved, LibraryField::Duration).is_empty());

        let resolved = smart_playlist_with_stats(2, 120);
        assert_eq!(
            smart_playlist_field(&resolved, LibraryField::SongCount),
            "2 tracks"
        );
        assert_eq!(
            smart_playlist_field(&resolved, LibraryField::Duration),
            "2:00"
        );
    }

    #[test]
    fn collection_grid_card_height_reserves_field_slots() {
        let title_only = collection_grid_card_height(180, 0);
        let with_fields = collection_grid_card_height(180, 2);

        assert_eq!(title_only, 226);
        assert_eq!(with_fields, 278);
        assert_eq!(
            with_fields - title_only,
            2 * (COLLECTION_GRID_LABEL_LINE_HEIGHT + COLLECTION_GRID_CARD_GAP)
        );
    }
}
