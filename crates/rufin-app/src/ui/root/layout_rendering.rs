use super::*;

impl PlaylistEntrySort {
    pub(in crate::ui) fn title(self) -> &'static str {
        match self {
            Self::Order => "Playlist order",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
        }
    }
}
pub(in crate::ui) const PLAYLIST_ENTRY_SORTS: [PlaylistEntrySort; 4] = [
    PlaylistEntrySort::Order,
    PlaylistEntrySort::Title,
    PlaylistEntrySort::Artist,
    PlaylistEntrySort::Album,
];
#[derive(Clone)]
pub(in crate::ui) struct LoadedTrackPlayContext {
    source_key: Rc<dyn Fn() -> PlaySourceKey>,
}
impl LoadedTrackPlayContext {
    pub(in crate::ui) fn new(source_key: impl Fn() -> PlaySourceKey + 'static) -> Self {
        Self {
            source_key: Rc::new(source_key),
        }
    }

    pub(in crate::ui) fn source_key(&self) -> PlaySourceKey {
        (self.source_key)()
    }
}
pub(in crate::ui) fn loaded_tracks_play_activation(
    source_key: PlaySourceKey,
    tracks: Vec<Track>,
    anchor_index: usize,
) -> Option<PlayActivation> {
    loaded_track_items_play_activation(
        source_key,
        tracks.into_iter().map(|track| (track, None)).collect(),
        anchor_index,
    )
}
pub(in crate::ui) fn loaded_track_items_play_activation(
    source_key: PlaySourceKey,
    items: Vec<(Track, Option<String>)>,
    anchor_index: usize,
) -> Option<PlayActivation> {
    let (anchor_track, anchor_source_item_id) = items.get(anchor_index).cloned()?;
    Some(PlayActivation {
        action: PlayAction::ReplaceNow,
        target: PlayTarget::LoadedSource {
            source_key,
            completeness: LoadedCompleteness::Complete,
            items: items
                .into_iter()
                .enumerate()
                .map(|(source_index, (track, source_item_id))| PlaySourceItem {
                    track,
                    source_index,
                    source_item_id,
                })
                .collect(),
            anchor: PlayAnchor {
                track_id: anchor_track.id,
                source_index: anchor_index,
                source_item_id: anchor_source_item_id,
            },
        },
    })
}
pub(in crate::ui) fn loaded_tracks_window_play_activation(
    source_key: PlaySourceKey,
    total_items: usize,
    anchor_index: usize,
    mut track_at: impl FnMut(usize) -> Option<Track>,
) -> Option<PlayActivation> {
    if total_items == 0 || anchor_index >= total_items {
        return None;
    }
    let (start, end, completeness) = if total_items <= FULL_LOADED_LIMIT {
        (0, total_items, LoadedCompleteness::Complete)
    } else {
        let (start, end) = bounded_loaded_window(total_items, anchor_index);
        (
            start,
            end,
            LoadedCompleteness::Window {
                start,
                total: Some(total_items),
            },
        )
    };
    let items = (start..end)
        .map(|source_index| {
            track_at(source_index).map(|track| PlaySourceItem {
                track,
                source_index,
                source_item_id: None,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let anchor_track = items
        .get(anchor_index.saturating_sub(start))
        .map(|item| item.track.clone())?;
    Some(PlayActivation {
        action: PlayAction::ReplaceNow,
        target: PlayTarget::LoadedSource {
            source_key,
            completeness,
            items,
            anchor: PlayAnchor {
                track_id: anchor_track.id,
                source_index: anchor_index,
                source_item_id: None,
            },
        },
    })
}
fn bounded_loaded_window(total_items: usize, anchor_index: usize) -> (usize, usize) {
    let before = anchor_index.min(MATERIALIZED_WINDOW_BEFORE_ANCHOR);
    let start = anchor_index - before;
    let after = MATERIALIZED_WINDOW_LIMIT.saturating_sub(before + 1);
    let mut end = (anchor_index + 1).saturating_add(after).min(total_items);
    let mut start = start;
    let len = end.saturating_sub(start);
    if len < MATERIALIZED_WINDOW_LIMIT {
        start = start.saturating_sub(MATERIALIZED_WINDOW_LIMIT - len);
    }
    end = end.max(anchor_index + 1).min(total_items);
    (start, end)
}
pub(in crate::ui) fn selected_music_folder_id(shell: &Rc<Shell>) -> Option<MusicFolderId> {
    shell
        .state
        .library
        .borrow()
        .selected_music_folder_id
        .clone()
}
pub(in crate::ui) fn album_play_source_key(
    album_id: AlbumId,
    selected_music_folder_id: Option<MusicFolderId>,
) -> PlaySourceKey {
    PlaySourceKey {
        descriptor: PlaySourceDescriptor::Album {
            album_id,
            selected_music_folder_id,
        },
        order: SourceOrder::Canonical,
    }
}
pub(in crate::ui) fn album_play_activation(
    album_id: AlbumId,
    tracks: Vec<Track>,
    anchor_index: usize,
    selected_music_folder_id: Option<MusicFolderId>,
) -> Option<PlayActivation> {
    loaded_tracks_play_activation(
        album_play_source_key(album_id, selected_music_folder_id),
        tracks,
        anchor_index,
    )
}
pub(in crate::ui) fn playlist_play_activation(
    playlist_id: PlaylistId,
    entries: Vec<PlaylistEntry>,
    anchor_index: usize,
    state: &PlaylistEntryListState,
) -> Option<PlayActivation> {
    let anchor_entry = entries.get(anchor_index)?;
    Some(PlayActivation {
        action: PlayAction::ReplaceNow,
        target: PlayTarget::StoreBackedSource {
            source_key: playlist_play_source_key(playlist_id, state),
            anchor: PlayAnchor {
                track_id: anchor_entry.track.id.clone(),
                source_index: anchor_index,
                source_item_id: Some(anchor_entry.entry_id.clone()),
            },
        },
    })
}
pub(in crate::ui) fn playlist_play_source_key(
    playlist_id: PlaylistId,
    state: &PlaylistEntryListState,
) -> PlaySourceKey {
    PlaySourceKey {
        descriptor: PlaySourceDescriptor::Playlist { playlist_id },
        order: SourceOrder::PlaylistDisplayed {
            query: source_query(&state.query),
            sort: playlist_entry_sort_descriptor(state.sort),
            descending: state.descending,
        },
    }
}
pub(in crate::ui) fn track_collection_play_context(
    shell: &Rc<Shell>,
    descriptor: PlaySourceDescriptor,
    key: LibraryListKey,
    query: Rc<RefCell<String>>,
    favorite_first: bool,
) -> LoadedTrackPlayContext {
    let shell = Rc::clone(shell);
    LoadedTrackPlayContext::new(move || PlaySourceKey {
        descriptor: descriptor.clone(),
        order: library_displayed_source_order(
            &shell.library_settings(key),
            query.borrow().as_str(),
            favorite_first,
        ),
    })
}
pub(in crate::ui) fn track_table_play_context(
    shell: &Rc<Shell>,
    descriptor: PlaySourceDescriptor,
    query: Rc<RefCell<String>>,
    favorite_first: bool,
) -> LoadedTrackPlayContext {
    let shell = Rc::clone(shell);
    LoadedTrackPlayContext::new(move || {
        let settings = shell.state.settings.borrow().track_table.clone();
        PlaySourceKey {
            descriptor: descriptor.clone(),
            order: SourceOrder::LibraryDisplayed {
                filter_key: track_table_filter_key(
                    &settings,
                    query.borrow().as_str(),
                    favorite_first,
                ),
                sort: track_sort_descriptor(settings.sort_key),
            },
        }
    })
}
pub(in crate::ui) fn library_displayed_source_order(
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) -> SourceOrder {
    SourceOrder::LibraryDisplayed {
        filter_key: library_filter_key(settings, query, favorite_first),
        sort: library_sort_descriptor(settings.sort_key),
    }
}
pub(in crate::ui) fn folder_play_source_key(
    path: &[FolderPathItem],
    query: &str,
    settings: &TrackTableSettings,
    selected_music_folder_id: Option<MusicFolderId>,
) -> PlaySourceKey {
    PlaySourceKey {
        descriptor: PlaySourceDescriptor::FolderLoaded {
            path: path.iter().map(|entry| entry.name.clone()).collect(),
            selected_music_folder_id,
        },
        order: SourceOrder::FolderDisplayed {
            query: source_query(query),
            filter_key: track_table_filter_key(settings, query, false),
            sort: track_sort_descriptor(settings.sort_key),
        },
    }
}
pub(in crate::ui) fn smart_playlist_play_source_key(
    playlist: &SmartPlaylist,
    selected_music_folder_id: Option<MusicFolderId>,
) -> PlaySourceKey {
    PlaySourceKey {
        descriptor: PlaySourceDescriptor::SmartPlaylist {
            smart_playlist_id: playlist.id.clone(),
            definition_fingerprint: smart_playlist_definition_fingerprint(&playlist.definition),
            selected_music_folder_id,
        },
        order: SourceOrder::SmartPlaylistDefinition {
            sort: SmartPlaylistSortDescriptor::Definition,
            limit: playlist.definition.limit,
            skip_count: 0,
        },
    }
}
pub(in crate::ui) fn smart_playlist_definition_fingerprint(
    definition: &SmartPlaylistDefinition,
) -> String {
    serde_json::to_string(definition).unwrap_or_else(|_| "unavailable".to_string())
}
fn library_filter_key(
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) -> Option<String> {
    let query = query.trim();
    Some(format!(
        "query={};sort={};descending={};favorite-first={}",
        query,
        library_field_key(settings.sort_key),
        settings.descending,
        favorite_first
    ))
}
fn track_table_filter_key(
    settings: &TrackTableSettings,
    query: &str,
    favorite_first: bool,
) -> Option<String> {
    let query = query.trim();
    Some(format!(
        "query={};sort={};descending={};favorite-first={}",
        query,
        track_sort_key(settings.sort_key),
        settings.descending,
        favorite_first
    ))
}
fn source_query(query: &str) -> Option<String> {
    let query = query.trim();
    (!query.is_empty()).then(|| query.to_string())
}
fn playlist_entry_sort_descriptor(sort: PlaylistEntrySort) -> PlaylistEntrySortDescriptor {
    match sort {
        PlaylistEntrySort::Order => PlaylistEntrySortDescriptor::Position,
        PlaylistEntrySort::Title => PlaylistEntrySortDescriptor::Title,
        PlaylistEntrySort::Artist => PlaylistEntrySortDescriptor::Artist,
        PlaylistEntrySort::Album => PlaylistEntrySortDescriptor::Album,
    }
}
fn track_sort_key(sort_key: TrackSortKey) -> &'static str {
    match sort_key {
        TrackSortKey::TrackNumber => "track-number",
        TrackSortKey::Title => "title",
        TrackSortKey::Artist => "artist",
        TrackSortKey::Album => "album",
        TrackSortKey::Year => "year",
        TrackSortKey::Duration => "duration",
        TrackSortKey::Favorite => "favorite",
    }
}
fn track_sort_descriptor(sort_key: TrackSortKey) -> TrackSortDescriptor {
    match sort_key {
        TrackSortKey::TrackNumber => TrackSortDescriptor::TrackNumber,
        TrackSortKey::Title => TrackSortDescriptor::Title,
        TrackSortKey::Artist => TrackSortDescriptor::Artist,
        TrackSortKey::Album => TrackSortDescriptor::Album,
        TrackSortKey::Year | TrackSortKey::Duration | TrackSortKey::Favorite => {
            TrackSortDescriptor::Title
        }
    }
}
fn library_sort_descriptor(field: LibraryField) -> TrackSortDescriptor {
    match field {
        LibraryField::TrackNumber | LibraryField::DiscNumber => TrackSortDescriptor::TrackNumber,
        LibraryField::Title | LibraryField::TitleMerged => TrackSortDescriptor::Title,
        LibraryField::Artist | LibraryField::AlbumArtist => TrackSortDescriptor::Artist,
        LibraryField::Album => TrackSortDescriptor::Album,
        LibraryField::DateAdded => TrackSortDescriptor::DateAdded,
        _ => TrackSortDescriptor::Title,
    }
}
fn library_field_key(field: LibraryField) -> &'static str {
    match field {
        LibraryField::RowIndex => "row-index",
        LibraryField::Image => "image",
        LibraryField::Title => "title",
        LibraryField::TitleMerged => "title-merged",
        LibraryField::Artist => "artist",
        LibraryField::AlbumArtist => "album-artist",
        LibraryField::Album => "album",
        LibraryField::Year => "year",
        LibraryField::ReleaseDate => "release-date",
        LibraryField::DateAdded => "date-added",
        LibraryField::LastPlayed => "last-played",
        LibraryField::PlayCount => "play-count",
        LibraryField::UserRating => "user-rating",
        LibraryField::Genre => "genre",
        LibraryField::TrackNumber => "track-number",
        LibraryField::DiscNumber => "disc-number",
        LibraryField::SongCount => "song-count",
        LibraryField::AlbumCount => "album-count",
        LibraryField::Duration => "duration",
        LibraryField::Favorite => "favorite",
    }
}
#[derive(Clone, Debug)]
pub(in crate::ui) struct PlaylistEntryListState {
    pub(in crate::ui) query: String,
    pub(in crate::ui) sort: PlaylistEntrySort,
    pub(in crate::ui) descending: bool,
}
impl Default for PlaylistEntryListState {
    fn default() -> Self {
        Self {
            query: String::new(),
            sort: PlaylistEntrySort::Order,
            descending: false,
        }
    }
}
pub(in crate::ui) fn rebuild_playlist_entries_list(
    shell: &Rc<Shell>,
    list: &gtk::ListBox,
    entries: &Rc<Vec<PlaylistEntry>>,
    state: &PlaylistEntryListState,
    playlist_id: &PlaylistId,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let rows = playlist_entries_for_state(entries, state);
    if rows.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No tracks match the search.")));
        empty.add_css_class("muted");
        empty.set_margin_top(16);
        empty.set_margin_bottom(16);
        list.append(&empty);
        return;
    }

    let visible_entries = Rc::new(
        rows.iter()
            .map(|(_, entry)| entry.clone())
            .collect::<Vec<_>>(),
    );
    for (display_index, (original_index, entry)) in rows.into_iter().enumerate() {
        list.append(&playlist_entry_row(
            shell,
            Rc::clone(entries),
            Rc::clone(&visible_entries),
            playlist_id,
            state.clone(),
            original_index,
            display_index,
            &entry,
        ));
    }
}
pub(in crate::ui) fn playlist_entries_for_state(
    entries: &[PlaylistEntry],
    state: &PlaylistEntryListState,
) -> Vec<(usize, PlaylistEntry)> {
    let query = state.query.trim().to_lowercase();
    let mut rows = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || playlist_entry_matches_query(entry, &query))
        .map(|(index, entry)| (index, entry.clone()))
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        let ordering = compare_playlist_entry(left, right, state.sort);
        if state.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}
pub(in crate::ui) fn playlist_entry_matches_query(entry: &PlaylistEntry, query: &str) -> bool {
    entry.track.title.to_lowercase().contains(query)
        || entry.track.artist.to_lowercase().contains(query)
        || entry.track.album.to_lowercase().contains(query)
}
pub(in crate::ui) fn compare_playlist_entry(
    left: &(usize, PlaylistEntry),
    right: &(usize, PlaylistEntry),
    sort: PlaylistEntrySort,
) -> std::cmp::Ordering {
    match sort {
        PlaylistEntrySort::Order => left.0.cmp(&right.0),
        PlaylistEntrySort::Title => cmp_text(&left.1.track.title, &right.1.track.title),
        PlaylistEntrySort::Artist => cmp_text(&left.1.track.artist, &right.1.track.artist),
        PlaylistEntrySort::Album => cmp_text(&left.1.track.album, &right.1.track.album),
    }
    .then_with(|| left.0.cmp(&right.0))
}
pub(in crate::ui) fn cmp_text(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}
pub(in crate::ui) fn playlist_entries_header_row() -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    row.add_css_class("playlist-entry-header");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_valign(gtk::Align::Center);
    row.append(&fixed_spacer(PLAYLIST_ENTRY_DRAG_WIDTH));
    row.append(&playlist_header_label(
        "#",
        PLAYLIST_ENTRY_NUMBER_WIDTH,
        false,
        PLAYLIST_ENTRY_NUMBER_XALIGN,
    ));
    row.append(&playlist_text_columns(
        playlist_header_text_label("Title", PLAYLIST_ENTRY_TITLE_MAX_CHARS).upcast(),
        playlist_header_album_label("Album", PLAYLIST_ENTRY_ALBUM_MAX_CHARS).upcast(),
    ));
    row.append(&playlist_header_label(
        "Plays",
        PLAYLIST_ENTRY_PLAY_COUNT_WIDTH,
        false,
        1.0,
    ));
    row.upcast()
}
pub(in crate::ui) fn playlist_header_label(
    text: &str,
    width: i32,
    expand: bool,
    xalign: f32,
) -> gtk::Label {
    let label = gtk::Label::new(Some(&tr(text)));
    label.add_css_class("muted");
    label.set_xalign(xalign);
    label.set_width_request(width);
    label.set_hexpand(expand);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if expand {
        label.set_width_chars(1);
        label.set_max_width_chars(PLAYLIST_ENTRY_TITLE_MAX_CHARS);
    }
    label
}
pub(in crate::ui) fn playlist_header_text_label(text: &str, max_width_chars: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(&tr(text)));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Fill);
    label.set_width_chars(1);
    label.set_max_width_chars(max_width_chars);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}
pub(in crate::ui) fn playlist_header_album_label(text: &str, max_width_chars: i32) -> gtk::Label {
    let label = playlist_header_text_label(text, max_width_chars);
    label.set_xalign(0.5);
    label
}
pub(in crate::ui) fn playlist_text_columns(title: gtk::Widget, album: gtk::Widget) -> gtk::Widget {
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_TEXT_COLUMN_GAP);
    columns.set_homogeneous(false);
    columns.set_hexpand(true);
    columns.set_halign(gtk::Align::Fill);
    columns.set_width_request(1);

    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    columns.append(&title);

    album.set_hexpand(true);
    album.set_halign(gtk::Align::Fill);
    album.set_width_request(1);
    columns.append(&album);

    columns.upcast()
}
pub(in crate::ui) fn playlist_title_cell(cover: gtk::Widget, labels: gtk::Widget) -> gtk::Widget {
    let title = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    title.append(&cover);
    title.append(&labels);
    title.upcast()
}
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn playlist_entry_row(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    visible_entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: &PlaylistId,
    state: PlaylistEntryListState,
    original_index: usize,
    display_index: usize,
    entry: &PlaylistEntry,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    row.add_css_class("playlist-entry-row");
    row.set_focusable(true);
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_valign(gtk::Align::Center);

    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(PLAYLIST_ENTRY_DRAG_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let drag_source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let drag_entry_id = entry.entry_id.clone();
    drag_source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(
            &drag_entry_id.to_value(),
        ))
    });
    drag.add_controller(drag_source);
    row.append(&drag);

    let number = gtk::Label::new(Some(&(display_index + 1).to_string()));
    number.add_css_class("muted");
    number.set_xalign(PLAYLIST_ENTRY_NUMBER_XALIGN);
    number.set_width_request(PLAYLIST_ENTRY_NUMBER_WIDTH);
    row.append(&number);

    let cover = shell.cover_tile_for(
        entry.track.image_ref.as_ref(),
        stable_seed(entry.track.id.as_str()),
        PLAYLIST_ENTRY_COVER_WIDTH,
        THUMB_COVER_SIZE,
    );

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    labels.set_halign(gtk::Align::Fill);
    labels.set_width_request(1);
    labels.append(&playlist_entry_text_label(
        &entry.track.title,
        "",
        PLAYLIST_ENTRY_TITLE_MAX_CHARS,
    ));
    labels.append(&playlist_entry_text_label(
        &entry.track.artist,
        "muted",
        PLAYLIST_ENTRY_TITLE_MAX_CHARS,
    ));

    let album =
        playlist_entry_text_label(&entry.track.album, "muted", PLAYLIST_ENTRY_ALBUM_MAX_CHARS);
    album.set_xalign(0.5);
    album.set_valign(gtk::Align::Center);
    row.append(&playlist_text_columns(
        playlist_title_cell(cover, labels.upcast()),
        album.upcast(),
    ));

    let play_count = gtk::Label::new(Some(&playlist_entry_play_count_text(
        entry.track.play_count,
    )));
    play_count.add_css_class("muted");
    play_count.set_xalign(1.0);
    play_count.set_valign(gtk::Align::Center);
    play_count.set_width_request(PLAYLIST_ENTRY_PLAY_COUNT_WIDTH);
    row.append(&play_count);

    let controller = shell.controller.clone();
    let entries_for_play = Rc::clone(&visible_entries);
    let playlist_id_for_play = playlist_id.clone();
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |gesture, n_press, _, _| {
        if n_press == 2 {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(activation) = playlist_play_activation(
                playlist_id_for_play.clone(),
                entries_for_play.as_ref().clone(),
                display_index,
                &state,
            ) {
                controller.play_activation(activation);
            }
        }
    });
    row.add_controller(click);
    install_playlist_entry_context_menu(
        &row,
        shell,
        entry.track.clone(),
        playlist_id.clone(),
        entry.entry_id.clone(),
        entry.track.title.clone(),
    );

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let controller = shell.controller.clone();
    let playlist_id = playlist_id.clone();
    let entries_for_drop = Rc::clone(&entries);
    let row_for_drop = row.clone();
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(entry_id) = value.get::<String>() else {
            return false;
        };
        let after = y > f64::from(row_for_drop.height()) / 2.0;
        let Some(new_index) =
            playlist_drop_index(&entries_for_drop, &entry_id, original_index, after)
        else {
            return false;
        };
        controller.move_playlist_entry(playlist_id.clone(), entry_id, new_index);
        true
    });
    row.add_controller(drop_target);

    row.upcast()
}
pub(in crate::ui) fn playlist_drop_index(
    entries: &[PlaylistEntry],
    dragged_entry_id: &str,
    target_index: usize,
    after: bool,
) -> Option<usize> {
    let source_index = entries
        .iter()
        .position(|entry| entry.entry_id == dragged_entry_id)?;
    let mut new_index = if after {
        target_index.saturating_add(1)
    } else {
        target_index
    };
    if source_index < new_index {
        new_index = new_index.saturating_sub(1);
    }
    (source_index != new_index).then_some(new_index)
}
pub(in crate::ui) fn playlist_entry_text_label(
    text: &str,
    css_class: &str,
    max_width_chars: i32,
) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    if !css_class.is_empty() {
        label.add_css_class(css_class);
    }
    label.set_xalign(0.0);
    label.set_width_chars(1);
    label.set_max_width_chars(max_width_chars);
    label.set_wrap(false);
    label.set_single_line_mode(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}
pub(in crate::ui) fn playlist_entry_play_count_text(value: Option<u32>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
pub(in crate::ui) fn fixed_spacer(width: i32) -> gtk::Widget {
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_width_request(width);
    spacer.upcast()
}
pub(in crate::ui) fn confirm_remove_playlist_entry(
    shell: &Rc<Shell>,
    playlist_id: PlaylistId,
    entry_id: String,
    title: String,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Remove from Playlist"))
        .body(format!("Remove \"{title}\" from this playlist?"))
        .build();
    dialog.add_response("cancel", &tr("Cancel"));
    dialog.add_response("remove", &tr("Remove"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    let controller = shell.controller.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "remove" {
            controller.remove_playlist_entry(playlist_id.clone(), entry_id.clone());
        }
    });
    dialog.present(Some(&shell.window));
}
pub(in crate::ui) fn seekbar_target_seconds(value: f64, duration_seconds: u32) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(0.0, f64::from(duration_seconds)) as u32
}
pub(in crate::ui) fn set_active_class(widget: &impl IsA<gtk::Widget>, active: bool) {
    if active {
        widget.add_css_class("active-toggle");
    } else {
        widget.remove_css_class("active-toggle");
    }
}
pub(in crate::ui) fn favorite_icon_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(FAVORITE_EMPTY_GLYPH);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.add_css_class("favorite-toggle");
    button.set_tooltip_text(Some(&tr(label)));
    button
}
pub(in crate::ui) fn set_favorite_button_active(button: &gtk::Button, active: bool) {
    set_active_class(button, active);
    button.set_label(if active {
        FAVORITE_FILLED_GLYPH
    } else {
        FAVORITE_EMPTY_GLYPH
    });
}
pub(in crate::ui) fn favorite_button_is_active(button: &gtk::Button) -> bool {
    button.label().as_deref() == Some(FAVORITE_FILLED_GLYPH)
}
pub(in crate::ui) fn icon_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon_name);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));
    button
}
pub(in crate::ui) fn icon_button_with_image(
    icon_name: &str,
    label: &str,
) -> (gtk::Button, gtk::Image) {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));
    let image = gtk::Image::from_icon_name(icon_name);
    button.set_child(Some(&image));
    (button, image)
}
pub(in crate::ui) fn text_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("pill-button");
    button.add_css_class("pill");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
    button
}
pub(in crate::ui) fn detail_action_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = icon_button(icon_name, label);
    button.add_css_class("detail-showcase-action-button");
    button
}
pub(in crate::ui) fn detail_action_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("detail-showcase-actions");
    row.set_halign(gtk::Align::Center);
    row
}
pub(in crate::ui) fn detail_showcase_frame(
    header: gtk::Widget,
    _content_width: i32,
) -> gtk::Widget {
    header.set_hexpand(true);
    header.set_halign(gtk::Align::Fill);
    header
}
pub(in crate::ui) fn detail_link_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("detail-showcase-link-button");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
    button
}
pub(in crate::ui) fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../../style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
