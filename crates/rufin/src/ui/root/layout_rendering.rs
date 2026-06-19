use super::library::{
    configure_library_route_scroller, install_column_view_width_fit, item_at, item_at_from_item,
    play_count_column_width, route_column_view_initial_width_with_inset,
};
use super::*;

const PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH: i32 = 30;
const PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH: i32 = 320;
const PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH: i32 = 220;
const DETAIL_TRASH_ICON_SIZE: i32 = 18;

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
    descriptor: PlaySourceDescriptor,
    settings: Rc<dyn Fn() -> LibraryListSettings>,
    query: Rc<RefCell<String>>,
    favorite_first: bool,
}
impl LoadedTrackPlayContext {
    pub(in crate::ui) fn play_window(
        &self,
        controller: &AppController,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        controller.play_library_source_window(
            self.descriptor.clone(),
            (
                (self.settings)(),
                self.query.borrow().to_string(),
                self.favorite_first,
            ),
            total_items,
            anchor_index,
            track_at,
        )
    }
}
pub(in crate::ui) fn selected_music_folder_id(shell: &Rc<Shell>) -> Option<MusicFolderId> {
    shell
        .state
        .library
        .borrow()
        .selected_music_folder_id
        .clone()
}
pub(in crate::ui) fn track_collection_play_context(
    shell: &Rc<Shell>,
    descriptor: PlaySourceDescriptor,
    key: LibraryListKey,
    query: Rc<RefCell<String>>,
    favorite_first: bool,
) -> LoadedTrackPlayContext {
    let shell = Rc::clone(shell);
    LoadedTrackPlayContext {
        descriptor,
        settings: Rc::new(move || shell.library_settings(key)),
        query,
        favorite_first,
    }
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
#[derive(Clone)]
struct PlaylistEntryCellState {
    menu: Rc<RefCell<Option<PlaylistEntryContextMenuState>>>,
    row: Rc<Cell<Option<usize>>>,
}
#[derive(Clone)]
struct PlaylistEntryTitleCell {
    cover: ArtworkTile,
    title: gtk::Label,
    artist: gtk::Label,
}
thread_local! {
    static PLAYLIST_ENTRY_CELL_STATES: RefCell<HashMap<usize, PlaylistEntryCellState>> = RefCell::new(HashMap::new());
    static PLAYLIST_ENTRY_TITLE_CELLS: RefCell<HashMap<usize, PlaylistEntryTitleCell>> = RefCell::new(HashMap::new());
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
        let state = state_for_activate.borrow();
        controller.play_playlist_entry(
            playlist_id_for_activate.clone(),
            entry.clone(),
            position as usize,
            source_query(&state.query),
            (playlist_entry_sort_descriptor(state.sort), state.descending),
            false,
        );
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
fn playlist_entry_cell_state() -> PlaylistEntryCellState {
    PlaylistEntryCellState {
        menu: Rc::new(RefCell::new(None)),
        row: Rc::new(Cell::new(None)),
    }
}
fn store_playlist_entry_cell_state(item: &gtk::ListItem, state: PlaylistEntryCellState) {
    let key = list_item_storage_key(item);
    PLAYLIST_ENTRY_CELL_STATES.with(|states| {
        states.borrow_mut().insert(key, state);
    });
}
fn playlist_entry_cell_state_for_item(item: &gtk::ListItem) -> Option<PlaylistEntryCellState> {
    let key = list_item_storage_key(item);
    PLAYLIST_ENTRY_CELL_STATES.with(|states| states.borrow().get(&key).cloned())
}
fn remove_playlist_entry_cell_state(item: &gtk::ListItem) {
    let key = list_item_storage_key(item);
    PLAYLIST_ENTRY_CELL_STATES.with(|states| {
        states.borrow_mut().remove(&key);
    });
}
fn bind_playlist_entry_cell_state(
    state: &PlaylistEntryCellState,
    row: PlaylistEntryTableRow,
    entry: &PlaylistEntry,
    playlist_id: &PlaylistId,
) {
    state.row.set(Some(row.original_index));
    *state.menu.borrow_mut() = Some(PlaylistEntryContextMenuState {
        track: entry.track.clone(),
        remove_action: PlaylistEntryContextMenuAction {
            playlist_id: playlist_id.clone(),
            entry_id: entry.entry_id.clone(),
            title: entry.track.title.clone(),
        },
    });
}
fn clear_playlist_entry_cell_state(state: &PlaylistEntryCellState) {
    state.row.set(None);
    state.menu.borrow_mut().take();
}
fn setup_playlist_entry_cell_actions(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
    state: &PlaylistEntryCellState,
) {
    install_dynamic_playlist_entry_context_menu(target, shell, Rc::clone(&state.menu));

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let controller = shell.controller.clone();
    let target = target.as_ref().clone();
    let target_for_drop = target.clone();
    let row_state = Rc::clone(&state.row);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(entry_id) = value.get::<String>() else {
            return false;
        };
        let Some(target_index) = row_state.get() else {
            return false;
        };
        let after = y > f64::from(target_for_drop.height()) / 2.0;
        let Some(new_index) = playlist_drop_index(&entries, &entry_id, target_index, after) else {
            return false;
        };
        controller.move_playlist_entry(playlist_id.clone(), entry_id, new_index);
        true
    });
    target.add_controller(drop_target);
}
fn playlist_entry_reorder_column(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let drag = playlist_entry_drag_handle(&state);
        setup_playlist_entry_cell_actions(
            &drag,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&drag));
        store_playlist_entry_cell_state(item, state);
    });
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
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(state) = playlist_entry_cell_state_for_item(item)
        {
            clear_playlist_entry_cell_state(&state);
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            remove_playlist_entry_cell_state(item);
        }
    });
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
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let label = gtk::Label::new(None);
        label.add_css_class("muted");
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        setup_playlist_entry_cell_actions(
            &label,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&label));
        store_playlist_entry_cell_state(item, state);
    });
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
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        label.set_text(&(row.display_index + 1).to_string());
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            if let Some(label) = item
                .child()
                .and_then(|child| child.downcast::<gtk::Label>().ok())
            {
                label.set_text("");
            }
            if let Some(state) = playlist_entry_cell_state_for_item(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            remove_playlist_entry_cell_state(item);
        }
    });

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
    let value = Rc::new(value);
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Fill);
        root.set_hexpand(true);

        let label = gtk::Label::new(None);
        label.add_css_class("table-link-label");
        label.add_css_class("muted");
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_width_chars(1);
        label.set_max_width_chars((width / 8).clamp(8, 32));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&label);

        setup_playlist_entry_cell_actions(
            &root,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&root));
        store_playlist_entry_cell_state(item, state);
    });
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
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
            .and_then(|root| root.first_child())
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        label.set_text(&(value)(entry));
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            if let Some(label) = item
                .child()
                .and_then(|child| child.downcast::<gtk::Box>().ok())
                .and_then(|root| root.first_child())
                .and_then(|child| child.downcast::<gtk::Label>().ok())
            {
                label.set_text("");
            }
            if let Some(state) = playlist_entry_cell_state_for_item(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            remove_playlist_entry_cell_state(item);
        }
    });

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
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let cover = ArtworkTile::new(PLAYLIST_ENTRY_COVER_WIDTH, 0);
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_halign(gtk::Align::Fill);
        labels.set_width_request(1);
        let title = playlist_entry_text_label("", "", PLAYLIST_ENTRY_TITLE_MAX_CHARS);
        let artist = playlist_entry_text_label("", "muted", PLAYLIST_ENTRY_TITLE_MAX_CHARS);
        labels.append(&title);
        labels.append(&artist);
        let cell = playlist_title_cell(cover.widget(), labels.upcast());
        setup_playlist_entry_cell_actions(
            &cell,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&cell));
        store_playlist_entry_cell_state(item, state);
        let key = list_item_storage_key(item);
        PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                PlaylistEntryTitleCell {
                    cover,
                    title,
                    artist,
                },
            );
        });
    });
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
        let key = list_item_storage_key(item);
        let Some(cell) = PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
        else {
            return;
        };
        shell.bind_cover_tile_for(
            &cell.cover,
            entry.track.image_ref.as_ref(),
            stable_seed(entry.track.id.as_str()),
            PLAYLIST_ENTRY_COVER_WIDTH,
            THUMB_COVER_SIZE,
        );
        cell.title.set_text(&entry.track.title);
        cell.artist.set_text(&entry.track.artist);
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = list_item_storage_key(item);
            if let Some(cell) =
                PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
            {
                cell.cover.bind_image(0, None);
                cell.title.set_text("");
                cell.artist.set_text("");
            }
            if let Some(state) = playlist_entry_cell_state_for_item(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = list_item_storage_key(item);
            PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
            remove_playlist_entry_cell_state(item);
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(&tr("Title")), Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_drag_handle(state: &PlaylistEntryCellState) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let drag_source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let menu_state = Rc::clone(&state.menu);
    drag_source.connect_prepare(move |_, _, _| {
        let entry_id = menu_state
            .borrow()
            .as_ref()
            .map(|state| state.remove_action.entry_id.clone())?;
        Some(gtk::gdk::ContentProvider::for_value(&entry_id.to_value()))
    });
    drag.add_controller(drag_source);
    drag
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

pub(in crate::ui) fn format_duration_units(seconds: u32) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        return format!("{hours}h {minutes}m {seconds}s");
    }
    if minutes > 0 {
        return format!("{minutes}m {seconds}s");
    }
    format!("{seconds}s")
}

pub(in crate::ui) fn detail_summary_row(items: &[(&str, String)]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("detail-summary-row");
    row.set_halign(gtk::Align::Start);
    for (icon_name, text) in items {
        let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.add_css_class("muted");
        icon.set_pixel_size(14);
        item.append(&icon);

        let label = gtk::Label::new(Some(text));
        label.add_css_class("muted");
        label.set_xalign(0.0);
        item.append(&label);
        row.append(&item);
    }
    row
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
pub(in crate::ui) fn detail_genre_pill_button(label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("album-detail-genre-pill");
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    button.set_hexpand(false);
    button.set_tooltip_text(Some(label));
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_halign(gtk::Align::Start);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_width_chars(1);
    text.set_max_width_chars(28);
    button.set_child(Some(&text));
    button
}
pub(in crate::ui) fn detail_delete_button(label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.add_css_class("detail-showcase-action-button");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some(&tr(label)));
    button.set_child(Some(&detail_trash_icon()));
    button
}
fn detail_trash_icon() -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(DETAIL_TRASH_ICON_SIZE);
    icon.set_content_height(DETAIL_TRASH_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(|area, context, width, height| {
        let color = area.color();
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        context.set_line_width((f64::from(width.min(height)) * 0.095).clamp(1.5, 1.9));
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let left = width * 0.29;
        let right = width * 0.71;
        let top = height * 0.36;
        let bottom = height * 0.78;
        context.rectangle(left, top, right - left, bottom - top);
        let _ = context.stroke();

        let lid_y = height * 0.28;
        context.move_to(width * 0.23, lid_y);
        context.line_to(width * 0.77, lid_y);
        let _ = context.stroke();

        context.move_to(width * 0.42, height * 0.20);
        context.line_to(width * 0.58, height * 0.20);
        context.line_to(width * 0.62, lid_y);
        context.move_to(width * 0.42, height * 0.20);
        context.line_to(width * 0.38, lid_y);
        let _ = context.stroke();
    });
    icon
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
    header.set_width_request(1);
    header
}

pub(in crate::ui) fn detail_showcase_frame_with_back(
    shell: &Rc<Shell>,
    header: gtk::Widget,
) -> gtk::Widget {
    let frame = detail_showcase_frame(header);
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_halign(gtk::Align::Fill);
    overlay.set_width_request(1);
    overlay.set_child(Some(&frame));

    let back = icon_button("go-previous-symbolic", "Back");
    back.add_css_class("detail-back-button");
    back.set_halign(gtk::Align::Start);
    back.set_valign(gtk::Align::Start);
    back.set_margin_top(1);
    back.set_margin_start(4);
    back.set_sensitive(shell.state.routes.borrow().can_back());
    {
        let shell = Rc::clone(shell);
        back.connect_clicked(move |_| shell.go_back());
    }
    overlay.add_overlay(&back);
    overlay.set_measure_overlay(&back, false);
    overlay.upcast()
}

pub(in crate::ui) fn mark_tiny_detail_showcase(widget: &impl IsA<gtk::Widget>, width: i32) {
    if width < 520 {
        widget.add_css_class("detail-showcase-tiny");
    }
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
    if !crate::external_activity::external_site_links(&settings) {
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
    if !crate::external_activity::external_site_links(&settings) {
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
