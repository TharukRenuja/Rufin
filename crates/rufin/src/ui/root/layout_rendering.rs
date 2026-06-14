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
}
pub(in crate::ui) fn playlist_entries_table_panel(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    state: Rc<RefCell<PlaylistEntryListState>>,
    playlist_id: PlaylistId,
    content_inset: i32,
) -> (gtk::Widget, gio::ListStore) {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::NoSelection::new(Some(model.clone()));

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
        shell,
        &table,
        columns,
        route_column_view_initial_width_with_inset(shell, content_inset),
    );

    let controller = shell.controller.clone();
    let playlist_id_for_activate = playlist_id.clone();
    let entries_for_activate = Rc::clone(&entries);
    let state_for_activate = Rc::clone(&state);
    let model_for_activate = model.clone();
    table.connect_activate(move |_, position| {
        let Some(row) = item_at::<PlaylistEntryTableRow>(&model_for_activate, position) else {
            return;
        };
        let Some(entry) = entries_for_activate.get(row.original_index) else {
            return;
        };
        if let Some(activation) = playlist_entry_play_activation(
            playlist_id_for_activate.clone(),
            entry,
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
        .map(|(display_index, original_index)| {
            glib::BoxedAnyObject::new(PlaylistEntryTableRow {
                original_index,
                display_index,
            })
        })
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &rows);
}
pub(in crate::ui) fn playlist_entries_for_state(
    entries: &[PlaylistEntry],
    state: &PlaylistEntryListState,
) -> Vec<usize> {
    let query = state.query.trim().to_lowercase();
    let mut rows = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || playlist_entry_matches_query(entry, &query))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        let ordering = compare_playlist_entry(entries, *left, *right, state.sort);
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
fn playlist_entry_for_row<'a>(
    entries: &'a [PlaylistEntry],
    row: &PlaylistEntryTableRow,
) -> Option<&'a PlaylistEntry> {
    entries.get(row.original_index)
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
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let drag = playlist_entry_drag_handle(&entry.entry_id);
        install_playlist_entry_cell_actions(
            &drag,
            &shell,
            Rc::clone(&entries),
            playlist_id.clone(),
            row,
            entry,
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
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
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
            entry,
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
        |entry| entry.track.album.clone(),
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
        |entry| playlist_entry_play_count_text(entry.track.play_count),
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
    F: Fn(&PlaylistEntry) -> String + 'static,
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
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Fill);
        root.set_hexpand(true);

        let label = gtk::Label::new(Some(&value(entry)));
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
            entry,
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
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
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
        let title = playlist_title_cell(cover, labels.upcast());
        install_playlist_entry_cell_actions(
            &title,
            &shell,
            Rc::clone(&entries),
            playlist_id.clone(),
            row,
            entry,
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
    entry: &PlaylistEntry,
) {
    install_playlist_entry_context_menu(
        target,
        shell,
        entry.track.clone(),
        playlist_id.clone(),
        entry.entry_id.clone(),
        entry.track.title.clone(),
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
    entries: &[PlaylistEntry],
    left: usize,
    right: usize,
    sort: PlaylistEntrySort,
) -> std::cmp::Ordering {
    let Some(left_entry) = entries.get(left) else {
        return std::cmp::Ordering::Equal;
    };
    let Some(right_entry) = entries.get(right) else {
        return std::cmp::Ordering::Equal;
    };
    match sort {
        PlaylistEntrySort::Order => left.cmp(&right),
        PlaylistEntrySort::Title => cmp_text(&left_entry.track.title, &right_entry.track.title),
        PlaylistEntrySort::Artist => cmp_text(&left_entry.track.artist, &right_entry.track.artist),
        PlaylistEntrySort::Album => cmp_text(&left_entry.track.album, &right_entry.track.album),
    }
    .then_with(|| left.cmp(&right))
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
    button.set_valign(gtk::Align::Center);
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
    button.set_valign(gtk::Align::Center);
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
    button.set_valign(gtk::Align::Center);
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
pub(in crate::ui) fn detail_cover_button(
    shell: &Rc<Shell>,
    image_ref: Option<&ImageRef>,
    seed: u32,
    size: i32,
    fetch_size: u32,
    cover_class: &str,
) -> gtk::Button {
    shell.prime_cached_cover(image_ref, fetch_size, size);
    let cover = shell.cover_tile_for(image_ref, seed, size, fetch_size);
    cover.add_css_class("detail-showcase-cover");
    cover.add_css_class(cover_class);

    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("detail-cover-button");
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Start);
    button.set_cursor_from_name(Some("pointer"));
    button.set_child(Some(&cover));

    let shell = Rc::clone(shell);
    let image_ref = image_ref.cloned();
    button.connect_clicked(move |_| {
        shell.present_full_artwork(image_ref.as_ref(), seed);
    });
    button
}
impl Shell {
    fn present_full_artwork(self: &Rc<Self>, image_ref: Option<&ImageRef>, seed: u32) {
        let size = full_artwork_size(self.window.width(), self.window.height());
        let fetch_size = cover_fetch_size_for_display(size);
        let tile = ArtworkTile::new_sized(size, size, seed);
        let cover = tile.widget();
        self.bind_cover_tile_for_dimensions(
            &tile,
            image_ref,
            seed,
            GRID_COVER_SIZE as i32,
            GRID_COVER_SIZE,
        );
        self.bind_cover_tile_for_dimensions(&tile, image_ref, seed, size, fetch_size);
        cover.add_css_class("full-artwork-cover");
        cover.set_halign(gtk::Align::Center);
        cover.set_valign(gtk::Align::Center);

        let root = gtk::Overlay::new();
        root.add_css_class("full-artwork-window");
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&cover));

        self.app_root_overlay.add_overlay(&root);
        self.app_root_overlay.set_measure_overlay(&root, false);

        let overlay = self.app_root_overlay.clone();
        let root_for_close = root.clone();
        add_widget_click(root.upcast_ref(), move || {
            overlay.remove_overlay(&root_for_close)
        });
    }
}
fn full_artwork_size(width: i32, height: i32) -> i32 {
    (width.min(height) - 80).clamp(240, 720)
}
pub(in crate::ui) fn detail_showcase_frame(header: gtk::Widget) -> gtk::Widget {
    header.set_hexpand(true);
    header.set_halign(gtk::Align::Fill);
    header
}

pub(in crate::ui) fn fit_detail_text(label: &gtk::Label, text: &str) {
    let count = text.chars().count();
    if count >= 42 {
        label.add_css_class("detail-text-very-long");
    } else if count >= 24 {
        label.add_css_class("detail-text-long");
    }
}

pub(in crate::ui) fn album_external_links(shell: &Rc<Shell>, album: &Album) -> Option<gtk::Widget> {
    let settings = shell.state.settings.borrow();
    let link_settings = &settings.external_site_links;
    if settings.private_mode || !link_settings.enabled {
        return None;
    }

    let row = detail_external_link_row();
    if link_settings.lastfm
        && let Some(url) = lastfm_album_url(&album.artist, &album.title)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.lastfm",
            "Open on Last.fm",
            url,
        ));
    }
    if link_settings.musicbrainz
        && let Some(url) = musicbrainz_album_url(album)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.musicbrainz",
            "Open on MusicBrainz",
            url,
        ));
    }
    if link_settings.server
        && let Some(link) = server_entity_url(shell, album.id.as_str())
    {
        row.append(&detail_external_link_button(
            shell,
            link.icon_name,
            link.label,
            link.url,
        ));
    }

    row.first_child().is_some().then(|| row.upcast())
}

pub(in crate::ui) fn artist_external_links(
    shell: &Rc<Shell>,
    artist: &Artist,
    tracks: &[Track],
) -> Option<gtk::Widget> {
    let settings = shell.state.settings.borrow();
    let link_settings = &settings.external_site_links;
    if settings.private_mode || !link_settings.enabled {
        return None;
    }

    let row = detail_external_link_row();
    if link_settings.lastfm
        && let Some(url) = lastfm_artist_url(&artist.name)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.lastfm",
            "Open on Last.fm",
            url,
        ));
    }
    if link_settings.musicbrainz
        && let Some(url) = musicbrainz_artist_url(artist, tracks)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.musicbrainz",
            "Open on MusicBrainz",
            url,
        ));
    }
    if link_settings.server
        && let Some(link) = server_entity_url(shell, artist.id.as_str())
    {
        row.append(&detail_external_link_button(
            shell,
            link.icon_name,
            link.label,
            link.url,
        ));
    }

    row.first_child().is_some().then(|| row.upcast())
}

fn detail_external_link_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("detail-external-link-row");
    row
}

fn detail_external_link_button(
    shell: &Rc<Shell>,
    icon_name: &str,
    label: &str,
    url: String,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.add_css_class("detail-external-link-button");
    button.set_tooltip_text(Some(&tr(label)));
    let image = gtk::Image::from_icon_name(icon_name);
    image.set_pixel_size(18);
    button.set_child(Some(&image));
    let window = shell.window.clone();
    button.connect_clicked(move |_| {
        let launcher = gtk::UriLauncher::new(&url);
        let window = window.clone();
        gtk::glib::spawn_future_local(async move {
            if let Err(error) = launcher.launch_future(Some(&window)).await {
                warn!(%error, "failed to open external detail link");
            }
        });
    });
    button
}

fn lastfm_album_url(artist: &str, album: &str) -> Option<String> {
    let artist = clean_url_label(artist)?;
    let album = clean_url_label(album)?;
    Some(format!(
        "https://www.last.fm/music/{}/{}",
        percent_encode_path_segment(artist),
        percent_encode_path_segment(album)
    ))
}

fn lastfm_artist_url(artist: &str) -> Option<String> {
    let artist = clean_url_label(artist)?;
    Some(format!(
        "https://www.last.fm/music/{}",
        percent_encode_path_segment(artist)
    ))
}

fn musicbrainz_album_url(album: &Album) -> Option<String> {
    if let Some(group_id) = album
        .musicbrainz_release_group_id
        .as_deref()
        .and_then(clean_url_label)
    {
        return Some(format!("https://musicbrainz.org/release-group/{group_id}"));
    }
    let release_id = album
        .musicbrainz_album_id
        .as_deref()
        .and_then(clean_url_label)?;
    Some(format!("https://musicbrainz.org/release/{release_id}"))
}

fn musicbrainz_artist_url(artist: &Artist, tracks: &[Track]) -> Option<String> {
    let artist_id = tracks
        .iter()
        .flat_map(|track| {
            track
                .artist_credits
                .iter()
                .chain(track.album_artist_credits.iter())
        })
        .find(|credit| {
            credit.id == artist.id || credit.name.eq_ignore_ascii_case(artist.name.as_str())
        })
        .and_then(|credit| credit.musicbrainz_artist_id.as_deref())
        .and_then(clean_url_label)?;
    Some(format!("https://musicbrainz.org/artist/{artist_id}"))
}

struct DetailExternalLink {
    label: &'static str,
    icon_name: &'static str,
    url: String,
}

fn server_entity_url(shell: &Shell, entity_id: &str) -> Option<DetailExternalLink> {
    let library = shell.state.library.borrow();
    let server = library.server.as_ref()?;
    if server.provider != "jellyfin" {
        return None;
    }
    let item_id = entity_id
        .strip_prefix("jellyfin:album:")
        .or_else(|| entity_id.strip_prefix("jellyfin:artist:"))?;
    let base_url = server.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() || item_id.trim().is_empty() {
        return None;
    }
    Some(DetailExternalLink {
        label: "Open on Jellyfin",
        icon_name: server_external_icon_name(&server.provider),
        url: format!("{base_url}/web/index.html#!/details?id={item_id}"),
    })
}

fn server_external_icon_name(provider: &str) -> &'static str {
    match provider {
        "jellyfin" => "io.github.screwys.Rufin.provider.jellyfin",
        "navidrome" => "io.github.screwys.Rufin.provider.navidrome",
        "subsonic" | "opensubsonic" => "io.github.screwys.Rufin.provider.opensubsonic",
        _ => "network-server-symbolic",
    }
}

fn clean_url_label(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }
    encoded
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

#[cfg(test)]
mod external_link_tests {
    use super::*;

    #[test]
    fn full_artwork_size_fits_window() {
        assert_eq!(full_artwork_size(1440, 900), 720);
        assert_eq!(full_artwork_size(640, 480), 400);
        assert_eq!(full_artwork_size(300, 260), 240);
    }

    #[test]
    fn lastfm_urls_escape_path_segments() {
        assert_eq!(
            lastfm_album_url("Test Artist", "A/B").as_deref(),
            Some("https://www.last.fm/music/Test%20Artist/A%2FB")
        );
        assert_eq!(
            lastfm_artist_url("青葉市子").as_deref(),
            Some("https://www.last.fm/music/%E9%9D%92%E8%91%89%E5%B8%82%E5%AD%90")
        );
    }

    #[test]
    fn musicbrainz_album_url_prefers_release_group() {
        let mut album = Album {
            id: AlbumId::fake(1),
            title: "Album".to_string(),
            artist: "Artist".to_string(),
            artist_id: None,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 60,
            favorite: false,
            color_seed: 1,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: Some("release-one".to_string()),
            musicbrainz_release_group_id: Some("group-one".to_string()),
        };

        assert_eq!(
            musicbrainz_album_url(&album).as_deref(),
            Some("https://musicbrainz.org/release-group/group-one")
        );

        album.musicbrainz_release_group_id = None;
        assert_eq!(
            musicbrainz_album_url(&album).as_deref(),
            Some("https://musicbrainz.org/release/release-one")
        );
    }
}
