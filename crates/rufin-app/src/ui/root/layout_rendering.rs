use super::library::{
    clear_list_item_child, configure_library_route_scroller, install_column_view_width_fit,
    item_at, item_at_from_item, play_count_column_width,
    route_column_view_initial_width_with_inset,
};
use super::*;

const PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH: i32 = 30;
const PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH: i32 = 320;
const PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH: i32 = 220;

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
pub(in crate::ui) fn playlist_entry_play_activation(
    playlist_id: PlaylistId,
    anchor_entry: &PlaylistEntry,
    anchor_index: usize,
    state: &PlaylistEntryListState,
) -> Option<PlayActivation> {
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
#[derive(Clone)]
pub(in crate::ui) struct PlaylistEntryTableRow {
    pub(in crate::ui) original_index: usize,
    pub(in crate::ui) display_index: usize,
    pub(in crate::ui) entry: PlaylistEntry,
}
pub(in crate::ui) fn playlist_entries_table_panel(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    state: Rc<RefCell<PlaylistEntryListState>>,
    playlist_id: PlaylistId,
    content_inset: i32,
) -> (gtk::Widget, gio::ListStore) {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);

    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.add_css_class("playlist-entry-table");
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_halign(gtk::Align::Fill);
    table.set_vexpand(true);

    let columns = vec![
        (
            playlist_entry_reorder_column(shell, Rc::clone(&entries), playlist_id.clone()),
            PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH,
        ),
        (
            playlist_entry_number_column(shell, Rc::clone(&entries), playlist_id.clone()),
            PLAYLIST_ENTRY_NUMBER_WIDTH,
        ),
        (
            playlist_entry_title_column(shell, Rc::clone(&entries), playlist_id.clone()),
            PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH,
        ),
        (
            playlist_entry_album_column(shell, Rc::clone(&entries), playlist_id.clone()),
            PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH,
        ),
        (
            playlist_entry_play_count_column(shell, Rc::clone(&entries), playlist_id.clone()),
            play_count_column_width(),
        ),
    ];
    for (column, _) in &columns {
        table.append_column(column);
    }
    install_column_view_width_fit(
        &table,
        columns,
        route_column_view_initial_width_with_inset(shell, content_inset),
    );

    let controller = shell.controller.clone();
    let playlist_id_for_activate = playlist_id.clone();
    let state_for_activate = Rc::clone(&state);
    let model_for_activate = model.clone();
    table.connect_activate(move |_, position| {
        let Some(row) = item_at::<PlaylistEntryTableRow>(&model_for_activate, position) else {
            return;
        };
        if let Some(activation) = playlist_entry_play_activation(
            playlist_id_for_activate.clone(),
            &row.entry,
            position as usize,
            &state_for_activate.borrow(),
        ) {
            controller.play_activation(activation);
        }
    });

    let scroller = gtk::ScrolledWindow::new();
    mark_route_scroll_owner(&scroller);
    configure_library_route_scroller(shell, &scroller);
    scroller.set_child(Some(&table));
    (scroller.upcast(), model)
}
pub(in crate::ui) fn rebuild_playlist_entries_model(
    model: &gio::ListStore,
    entries: &[PlaylistEntry],
    state: &PlaylistEntryListState,
) {
    let rows = playlist_entries_for_state(entries, state)
        .into_iter()
        .enumerate()
        .map(|(display_index, (original_index, entry))| {
            glib::BoxedAnyObject::new(PlaylistEntryTableRow {
                original_index,
                display_index,
                entry,
            })
        })
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &rows);
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
fn playlist_entry_reorder_column(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let drag = playlist_entry_drag_handle(&row.entry.entry_id);
        install_playlist_entry_cell_actions(
            &drag,
            &shell,
            Rc::clone(&entries),
            playlist_id.clone(),
            row,
        );
        item.set_child(Some(&drag));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(None::<&str>, Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_number_column(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };

        let label = gtk::Label::new(Some(&(row.display_index + 1).to_string()));
        label.add_css_class("muted");
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        install_playlist_entry_cell_actions(
            &label,
            &shell,
            Rc::clone(&entries),
            playlist_id.clone(),
            row,
        );
        item.set_child(Some(&label));
    });
    factory.connect_unbind(clear_list_item_child);

    let column = gtk::ColumnViewColumn::new(Some("#"), Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_NUMBER_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_album_column(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        "Album",
        PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH,
        entries,
        playlist_id,
        |row| row.entry.track.album.clone(),
    )
}
fn playlist_entry_play_count_column(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        "Plays",
        play_count_column_width(),
        entries,
        playlist_id,
        |row| playlist_entry_play_count_text(row.entry.track.play_count),
    )
}
fn playlist_entry_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&PlaylistEntryTableRow) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Fill);
        root.set_hexpand(true);

        let label = gtk::Label::new(Some(&value(&row)));
        label.add_css_class("table-link-label");
        label.add_css_class("muted");
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_width_chars(1);
        label.set_max_width_chars((width / 8).clamp(8, 32));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&label);

        install_playlist_entry_cell_actions(
            &root,
            &shell,
            Rc::clone(&entries),
            playlist_id.clone(),
            row,
        );
        item.set_child(Some(&root));
    });
    factory.connect_unbind(clear_list_item_child);

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}
fn playlist_entry_title_column(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let cover = shell.cover_tile_for(
            row.entry.track.image_ref.as_ref(),
            stable_seed(row.entry.track.id.as_str()),
            PLAYLIST_ENTRY_COVER_WIDTH,
            THUMB_COVER_SIZE,
        );

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_halign(gtk::Align::Fill);
        labels.set_width_request(1);
        labels.append(&playlist_entry_text_label(
            &row.entry.track.title,
            "",
            PLAYLIST_ENTRY_TITLE_MAX_CHARS,
        ));
        labels.append(&playlist_entry_text_label(
            &row.entry.track.artist,
            "muted",
            PLAYLIST_ENTRY_TITLE_MAX_CHARS,
        ));
        let title = playlist_title_cell(cover, labels.upcast());
        install_playlist_entry_cell_actions(
            &title,
            &shell,
            Rc::clone(&entries),
            playlist_id.clone(),
            row,
        );
        item.set_child(Some(&title));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr("Title")), Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_drag_handle(entry_id: &str) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let drag_source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let entry_id = entry_id.to_string();
    drag_source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&entry_id.to_value()))
    });
    drag.add_controller(drag_source);
    drag
}
fn install_playlist_entry_cell_actions(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
    row: PlaylistEntryTableRow,
) {
    install_playlist_entry_context_menu(
        target,
        shell,
        row.entry.track.clone(),
        playlist_id.clone(),
        row.entry.entry_id.clone(),
        row.entry.track.title.clone(),
    );

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let controller = shell.controller.clone();
    let target = target.as_ref().clone();
    let target_for_drop = target.clone();
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(entry_id) = value.get::<String>() else {
            return false;
        };
        let after = y > f64::from(target_for_drop.height()) / 2.0;
        let Some(new_index) = playlist_drop_index(&entries, &entry_id, row.original_index, after)
        else {
            return false;
        };
        controller.move_playlist_entry(playlist_id.clone(), entry_id, new_index);
        true
    });
    target.add_controller(drop_target);
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
pub(in crate::ui) fn playlist_title_cell(cover: gtk::Widget, labels: gtk::Widget) -> gtk::Widget {
    let title = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    title.append(&cover);
    title.append(&labels);
    title.upcast()
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
pub(in crate::ui) fn detail_showcase_frame(header: gtk::Widget) -> gtk::Widget {
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
