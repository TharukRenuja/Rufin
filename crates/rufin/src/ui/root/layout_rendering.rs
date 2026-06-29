use super::library::{
    configure_library_route_scroller, install_column_view_width_fit, item_at, item_at_from_item,
    play_count_column_width, route_column_view_initial_width_with_inset,
};
use super::*;
use crate::i18n::msgid;

const PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH: i32 = 30;
const PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH: i32 = 320;
const PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH: i32 = 220;
pub(in crate::ui) type PlaylistEntrySelectionHandle = Rc<RefCell<Option<Rc<dyn Fn(&str)>>>>;

impl PlaylistEntrySort {
    pub(in crate::ui) fn title(self) -> &'static str {
        match self {
            Self::Order => msgid("Playlist order"),
            Self::Title => msgid("Title"),
            Self::Artist => msgid("Artist"),
            Self::Album => msgid("Album"),
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
fn playlist_entry_id_at(
    entries: &[PlaylistEntry],
    model: &gio::ListStore,
    position: u32,
) -> Option<String> {
    let row = item_at::<PlaylistEntryTableRow>(model, position)?;
    entries
        .get(row.original_index)
        .map(|entry| entry.entry_id.clone())
}

fn playlist_entry_selection_position(
    entries: &[PlaylistEntry],
    model: &gio::ListStore,
    selected_entry_id: &RefCell<Option<String>>,
) -> u32 {
    let Some(entry_id) = selected_entry_id.borrow().clone() else {
        return gtk::INVALID_LIST_POSITION;
    };
    (0..model.n_items())
        .find(|position| {
            playlist_entry_id_at(entries, model, *position).as_deref() == Some(&entry_id)
        })
        .unwrap_or(gtk::INVALID_LIST_POSITION)
}

fn sync_playlist_entry_selection(
    selection: &gtk::SingleSelection,
    entries: &[PlaylistEntry],
    model: &gio::ListStore,
    selected_entry_id: &RefCell<Option<String>>,
) {
    let selected = playlist_entry_selection_position(entries, model, selected_entry_id);
    if selection.selected() != selected {
        selection.set_selected(selected);
    }
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
    selection_handle: Option<PlaylistEntrySelectionHandle>,
) -> (gtk::Widget, gio::ListStore) {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);
    let selected_entry_id = Rc::new(RefCell::new(None::<String>));
    let select_entry_id: Rc<dyn Fn(&str)> = Rc::new({
        let entries = Rc::clone(&entries);
        let model = model.clone();
        let selection = selection.clone();
        let selected_entry_id = Rc::clone(&selected_entry_id);
        move |entry_id| {
            *selected_entry_id.borrow_mut() = Some(entry_id.to_string());
            sync_playlist_entry_selection(&selection, entries.as_ref(), &model, &selected_entry_id);
        }
    });
    if let Some(selection_handle) = selection_handle.as_ref() {
        *selection_handle.borrow_mut() = Some(Rc::clone(&select_entry_id));
    }

    let table = gtk::ColumnView::new(Some(selection.clone()));
    table.add_css_class("track-table");
    table.add_css_class("playlist-entry-table");
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_halign(gtk::Align::Fill);
    table.set_vexpand(true);

    let columns = vec![
        (
            playlist_entry_reorder_column(
                shell,
                Rc::clone(&entries),
                playlist_id.clone(),
                Rc::clone(&select_entry_id),
            ),
            PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH,
        ),
        (
            playlist_entry_number_column(
                shell,
                Rc::clone(&entries),
                playlist_id.clone(),
                Rc::clone(&select_entry_id),
            ),
            PLAYLIST_ENTRY_NUMBER_WIDTH,
        ),
        (
            playlist_entry_title_column(
                shell,
                Rc::clone(&entries),
                playlist_id.clone(),
                Rc::clone(&select_entry_id),
            ),
            PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH,
        ),
        (
            playlist_entry_album_column(
                shell,
                Rc::clone(&entries),
                playlist_id.clone(),
                Rc::clone(&select_entry_id),
            ),
            PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH,
        ),
        (
            playlist_entry_play_count_column(
                shell,
                Rc::clone(&entries),
                playlist_id.clone(),
                Rc::clone(&select_entry_id),
            ),
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
    let select_entry_for_activate = Rc::clone(&select_entry_id);
    table.connect_activate(move |_, position| {
        let Some(row) = item_at::<PlaylistEntryTableRow>(&model_for_activate, position) else {
            return;
        };
        let Some(entry) = entries_for_activate.get(row.original_index) else {
            return;
        };
        select_entry_for_activate(&entry.entry_id);
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
    {
        let entries = Rc::clone(&entries);
        let model = model.clone();
        let selected_entry_id = Rc::clone(&selected_entry_id);
        selection.connect_selection_changed(move |selection, _, _| {
            sync_playlist_entry_selection(selection, entries.as_ref(), &model, &selected_entry_id);
        });
    }
    {
        let entries = Rc::clone(&entries);
        let selection = selection.clone();
        let selected_entry_id = Rc::clone(&selected_entry_id);
        model.connect_items_changed(move |model, _, _, _| {
            sync_playlist_entry_selection(&selection, entries.as_ref(), model, &selected_entry_id);
        });
    }
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
    select_entry_id: Rc<dyn Fn(&str)>,
) {
    let select_entry: Rc<dyn Fn(&PlaylistEntryContextMenuState)> =
        Rc::new(move |state| select_entry_id(&state.remove_action.entry_id));
    install_dynamic_playlist_entry_context_menu_with_play_handler(
        target,
        shell,
        Rc::clone(&state.menu),
        select_entry,
    );

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
    select_entry_id: Rc<dyn Fn(&str)>,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    let setup_select_entry_id = Rc::clone(&select_entry_id);
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
            Rc::clone(&setup_select_entry_id),
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
    select_entry_id: Rc<dyn Fn(&str)>,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    let setup_select_entry_id = Rc::clone(&select_entry_id);
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
            Rc::clone(&setup_select_entry_id),
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
    select_entry_id: Rc<dyn Fn(&str)>,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        "Album",
        PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH,
        entries,
        playlist_id,
        select_entry_id,
        |entry| entry.track.album.clone(),
    )
}
fn playlist_entry_play_count_column(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        msgid("Plays"),
        play_count_column_width(),
        entries,
        playlist_id,
        select_entry_id,
        |entry| playlist_entry_play_count_text(entry.track.play_count),
    )
}
fn playlist_entry_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
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
    let setup_select_entry_id = Rc::clone(&select_entry_id);
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
            Rc::clone(&setup_select_entry_id),
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
    select_entry_id: Rc<dyn Fn(&str)>,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    let setup_select_entry_id = Rc::clone(&select_entry_id);
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
        title.add_css_class("playlist-entry-title");
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
            Rc::clone(&setup_select_entry_id),
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
    present_light_dismiss_dialog(&dialog, &shell.window);
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
    let glyph = if active {
        FAVORITE_FILLED_GLYPH
    } else {
        FAVORITE_EMPTY_GLYPH
    };
    if let Some(label) = favorite_button_glyph(button) {
        label.set_label(glyph);
    } else {
        button.set_label(glyph);
    }
}
pub(in crate::ui) fn favorite_button_is_active(button: &gtk::Button) -> bool {
    button.has_css_class("active-toggle")
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

#[derive(Clone, Copy)]
pub(in crate::ui) enum ActionButtonVariant {
    CoverSideTransport,
    CoverPrimaryTransport,
    CoverCornerMenu,
    CoverCornerFavorite,
    DetailAction,
    DetailPrimary,
    DetailFavorite,
}

pub(in crate::ui) fn configure_action_button(
    button: &gtk::Button,
    variant: ActionButtonVariant,
    icon_name: Option<&str>,
) {
    let is_cover = matches!(
        variant,
        ActionButtonVariant::CoverSideTransport
            | ActionButtonVariant::CoverPrimaryTransport
            | ActionButtonVariant::CoverCornerMenu
            | ActionButtonVariant::CoverCornerFavorite
    );
    if is_cover {
        button.add_css_class("cover-hover-button");
        button.add_css_class("cover-hover-animated");
    } else {
        button.add_css_class("detail-showcase-action-button");
    }

    let nudge_icon = match variant {
        ActionButtonVariant::CoverSideTransport => {
            button.add_css_class("cover-side-button");
            pin_action_button(button, 34);
            true
        }
        ActionButtonVariant::CoverPrimaryTransport => {
            button.add_css_class("cover-play-button");
            pin_action_button(button, 54);
            true
        }
        ActionButtonVariant::CoverCornerMenu => {
            button.add_css_class("cover-menu-button");
            pin_action_button(button, 34);
            false
        }
        ActionButtonVariant::CoverCornerFavorite => {
            button.add_css_class("cover-favorite-button");
            pin_action_button(button, 34);
            false
        }
        ActionButtonVariant::DetailAction => true,
        ActionButtonVariant::DetailPrimary => {
            button.add_css_class("detail-showcase-play-button");
            true
        }
        ActionButtonVariant::DetailFavorite => false,
    };

    if let (true, Some(icon_name)) = (nudge_icon, icon_name) {
        nudge_transport_action_icon(button, icon_name);
    }
    let face_class = if is_cover {
        "cover-hover-face"
    } else {
        "detail-showcase-action-face"
    };
    wrap_button_child_in_action_layers(button, face_class);
    if matches!(
        variant,
        ActionButtonVariant::CoverCornerFavorite | ActionButtonVariant::DetailFavorite
    ) && let Some(label) = favorite_button_glyph(button)
    {
        label.set_margin_start(1);
    }
}

fn pin_action_button(button: &gtk::Button, size: i32) {
    button.set_size_request(size, size);
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
}

fn nudge_transport_action_icon(button: &gtk::Button, icon_name: &str) {
    let start_margin = if icon_name == "media-playback-start-symbolic" {
        4
    } else if icon_name == PLAY_NEXT_ICON || icon_name == PLAY_LATER_ICON {
        2
    } else {
        return;
    };
    let Some(child) = button.child() else {
        return;
    };
    if let Ok(image) = child.downcast::<gtk::Image>() {
        image.set_margin_start(start_margin);
    }
}
fn wrap_button_child_in_action_layers(button: &gtk::Button, face_class: &str) {
    let Some(child) = button.child() else {
        return;
    };
    button.set_child(None::<&gtk::Widget>);
    child.set_halign(gtk::Align::Center);
    child.set_valign(gtk::Align::Center);

    let shadow = gtk::CenterBox::new();
    shadow.add_css_class("action-button-shadow");
    shadow.set_can_target(false);

    let face = gtk::CenterBox::new();
    face.add_css_class(face_class);
    face.set_can_target(false);
    face.set_center_widget(Some(&child));
    shadow.set_center_widget(Some(&face));
    button.set_child(Some(&shadow));
}
fn favorite_button_glyph(button: &gtk::Button) -> Option<gtk::Label> {
    button.child().and_then(favorite_glyph_from_widget)
}
fn favorite_glyph_from_widget(widget: gtk::Widget) -> Option<gtk::Label> {
    let widget = match widget.downcast::<gtk::Label>() {
        Ok(label) => return Some(label),
        Err(widget) => widget,
    };
    widget
        .downcast::<gtk::CenterBox>()
        .ok()
        .and_then(|face| face.center_widget())
        .and_then(favorite_glyph_from_widget)
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
