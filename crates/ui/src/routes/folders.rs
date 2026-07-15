use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::Arc,
};

use super::collection_context::install_track_context_menu;
use super::route::{FolderPathItem, Route};
use crate::layout::{configure_fill_width_clip, width_allocation_owner};
use crate::localization::{
    bind_label_text, bind_search_placeholder, bind_widget_tooltip, localized_column,
    localized_label,
};
use crate::shell::Shell;
use crate::shell::cover::THUMB_COVER_SIZE;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::layout::route_content_width;
use crate::shell::route::{MountedRoute, MountedRouteDeltaApplier};
use crate::{LibraryListKey, LibraryListSettings, format_duration};
use ::library::{Folder, FolderDetail, LibraryDelta, Track};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use localization::{msgid, tr};
use playback::FolderWindowPlayRequest;
use tracing::warn;

use super::collection_context::present_track_context_menu;
use super::collection_routes::{MountedRefreshLoader, MountedRouteRefresh};
use super::collections::library_route_inset;
use super::library_fields::sort_tracks;
use super::models::track_matches_query;
use super::route_layout::{ROUTE_TOP_MARGIN, route_scroller_widget};

const FOLDER_TREE_WIDTH: i32 = 260;
const FOLDER_TREE_MIN_WIDTH: i32 = 132;
const FOLDER_TREE_HIDE_WIDTH: i32 = 550;
const FOLDER_ROW_ARTWORK_SIZE: i32 = 28;

#[derive(Clone)]
#[expect(clippy::large_enum_variant)]
enum FolderTableRow {
    Folder {
        name: String,
        path: Vec<FolderPathItem>,
    },
    Track {
        track: Track,
        tracks: Arc<Vec<Track>>,
        source: Rc<(Vec<FolderPathItem>, String, LibraryListSettings)>,
        position: usize,
    },
    Empty,
}

enum FolderCellText {
    Plain(String),
    Localized(&'static str),
}

impl FolderCellText {
    fn resolve(self) -> (String, Option<&'static str>) {
        match self {
            Self::Plain(text) => (text, None),
            Self::Localized(message) => (tr(message), Some(message)),
        }
    }
}

pub(crate) struct FolderRouteProjection {
    root: gtk::Widget,
    shell: Weak<Shell>,
    pub(crate) path: Vec<FolderPathItem>,
    status: gtk::Stack,
    error_body: gtk::Label,
    search: gtk::SearchEntry,
    tree: gtk::ListBox,
    table_model: gio::ListStore,
    pub(crate) detail: RefCell<Option<FolderDetail>>,
    pub(crate) error: RefCell<Option<String>>,
}

impl FolderRouteProjection {
    pub(crate) fn new(shell: &Rc<Shell>, path: Vec<FolderPathItem>) -> Rc<Self> {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("route-content");
        wrapper.add_css_class("folders-route");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(28);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);

        wrapper.append(&library_route_inset(
            folder_breadcrumbs(shell, &path).upcast(),
        ));

        let search = gtk::SearchEntry::new();
        search.add_css_class("folder-search");
        bind_search_placeholder(&search, "Search current folder");
        shell.set_route_search(Some(search.clone()));

        let route_width = route_content_width(shell);
        let tree_visible = folder_tree_visible(route_width);
        let tree_width = if tree_visible {
            folder_tree_width(route_width)
        } else {
            0
        };

        let table_model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let table = folder_table(shell, &table_model);

        let table_scroller = gtk::ScrolledWindow::new();
        configure_fill_width_clip(&table_scroller, gtk::PolicyType::Automatic);
        table_scroller.set_hexpand(true);
        table_scroller.set_vexpand(true);
        table_scroller.set_child(Some(&library_route_inset(table.clone().upcast())));
        let table_view = route_scroller_widget(table_scroller);

        let tree = gtk::ListBox::new();
        tree.add_css_class("folder-tree");
        tree.set_selection_mode(gtk::SelectionMode::None);
        tree.set_size_request(tree_width, -1);
        tree.set_hexpand(true);

        let tree_scroller = gtk::ScrolledWindow::new();
        tree_scroller.add_css_class("folders-tree-pane");
        tree_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        tree_scroller.set_min_content_width(tree_width);
        tree_scroller.set_size_request(tree_width, -1);
        tree_scroller.set_hexpand(false);
        tree_scroller.set_vexpand(true);
        tree_scroller.set_visible(tree_visible);
        tree_scroller.set_child(Some(&tree));

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.add_css_class("folders-split");
        paned.set_position(tree_width);
        paned.set_wide_handle(false);
        paned.set_hexpand(true);
        paned.set_vexpand(true);
        paned.set_start_child(Some(&tree_scroller));
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_end_child(Some(&table_view));
        paned.set_resize_end_child(true);
        paned.set_shrink_end_child(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.append(&library_route_inset(search.clone().upcast()));
        let resize_tree_scroller = tree_scroller.clone();
        let resize_tree = tree.clone();
        let resize_paned = paned.clone();
        let allocated_width = Cell::new(route_width);
        let paned_owner = width_allocation_owner(&paned, move |width| {
            if width <= 1 || allocated_width.replace(width) == width {
                return;
            }
            apply_folder_width(&resize_paned, &resize_tree_scroller, &resize_tree, width);
        });
        content.append(&paned_owner);

        let status = gtk::Stack::new();
        status.set_hexpand(true);
        status.set_vexpand(true);
        status.add_named(&content, Some("content"));
        status.add_named(
            &library_route_inset(shell.route_empty_view(msgid("Loading folders..."))),
            Some("loading"),
        );
        let (error_view, error_body) = folder_error_view();
        status.add_named(&library_route_inset(error_view), Some("error"));
        status.add_named(
            &library_route_inset(shell.route_empty_view(msgid("No folder contents found."))),
            Some("empty"),
        );
        status.set_visible_child_name("loading");
        wrapper.append(&status);

        let projection = Rc::new(Self {
            root: wrapper.upcast(),
            shell: Rc::downgrade(shell),
            path,
            status,
            error_body,
            search,
            tree,
            table_model,
            detail: RefCell::new(None),
            error: RefCell::new(None),
        });

        let search_projection = Rc::downgrade(&projection);
        projection.search.connect_search_changed(move |_| {
            if let Some(projection) = search_projection.upgrade() {
                projection.publish_table();
            }
        });
        projection
    }

    pub(crate) fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    pub(crate) fn publish(&self) {
        if let Some(error) = self.error.borrow().as_deref() {
            self.error_body.set_text(error);
        }
        if self.detail.borrow().is_some() {
            self.publish_tree();
            self.publish_table();
        }
        let page = if self.error.borrow().is_some() && self.detail.borrow().is_none() {
            "error"
        } else if self.detail.borrow().is_none() {
            "empty"
        } else {
            "content"
        };
        self.status.set_visible_child_name(page);
    }

    fn begin_refresh(&self) {
        *self.error.borrow_mut() = None;
        if self.detail.borrow().is_none() {
            self.status.set_visible_child_name("loading");
        }
    }

    fn apply_refresh(&self, result: Result<FolderDetail, String>) {
        match result {
            Ok(detail) => {
                *self.detail.borrow_mut() = Some(detail);
                *self.error.borrow_mut() = None;
            }
            Err(error) => {
                warn!(%error, path = ?self.path, "folder load failed");
                *self.error.borrow_mut() = Some(error);
            }
        }
        self.publish();
    }

    fn publish_tree(&self) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let detail = self.detail.borrow();
        let Some(detail) = detail.as_ref() else {
            return;
        };
        populate_folder_tree(&shell, &self.tree, &self.path, detail);
    }

    fn publish_table(&self) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let detail = self.detail.borrow();
        let Some(detail) = detail.as_ref() else {
            return;
        };
        populate_folder_table(
            &shell,
            &self.table_model,
            &self.path,
            detail,
            self.search.text().as_str(),
        );
    }
}

fn apply_folder_width(
    paned: &gtk::Paned,
    tree_scroller: &gtk::ScrolledWindow,
    tree: &gtk::ListBox,
    width: i32,
) {
    let tree_visible = folder_tree_visible(width);
    let tree_width = if tree_visible {
        folder_tree_width(width)
    } else {
        0
    };
    tree_scroller.set_visible(tree_visible);
    tree_scroller.set_min_content_width(tree_width);
    tree_scroller.set_size_request(tree_width, -1);
    tree.set_size_request(tree_width, -1);
    paned.set_position(tree_width);
}

fn folder_breadcrumbs(shell: &Rc<Shell>, path: &[FolderPathItem]) -> gtk::Box {
    let breadcrumbs = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    breadcrumbs.add_css_class("folder-breadcrumbs");
    breadcrumbs.set_hexpand(true);

    breadcrumbs.append(&breadcrumb_button(
        shell,
        "Folders",
        Vec::new(),
        path.is_empty(),
        true,
    ));
    for (index, entry) in path.iter().enumerate() {
        breadcrumbs.append(&gtk::Label::new(Some("/")));
        breadcrumbs.append(&breadcrumb_button(
            shell,
            &entry.name,
            path.iter().take(index + 1).cloned().collect(),
            index + 1 == path.len(),
            false,
        ));
    }

    breadcrumbs
}

fn breadcrumb_button(
    shell: &Rc<Shell>,
    label: &str,
    path: Vec<FolderPathItem>,
    current: bool,
    translate: bool,
) -> gtk::Button {
    let button = gtk::Button::with_label(&display_label(label, translate));
    button.add_css_class("flat");
    button.add_css_class("folder-breadcrumb");
    button.set_sensitive(!current);
    let shell = Rc::clone(shell);
    button.connect_clicked(move |_| {
        shell.navigate(Route::Folders { path: path.clone() });
    });
    button
}

fn populate_folder_tree(
    shell: &Rc<Shell>,
    tree: &gtk::ListBox,
    path: &[FolderPathItem],
    detail: &FolderDetail,
) {
    while let Some(child) = tree.first_child() {
        tree.remove(&child);
    }
    tree.append(&tree_row(
        shell,
        "Folders",
        Vec::new(),
        path.is_empty(),
        0,
        true,
    ));

    for (index, entry) in path.iter().enumerate() {
        tree.append(&tree_row(
            shell,
            &entry.name,
            path.iter().take(index + 1).cloned().collect(),
            index + 1 == path.len(),
            1,
            false,
        ));
    }

    let mut folders = detail.folders.clone();
    folders.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    for folder in folders {
        tree.append(&tree_row(
            shell,
            &folder.name,
            path_for_child(path, &folder),
            false,
            path.len().saturating_add(1).min(3),
            false,
        ));
    }
}

fn tree_row(
    shell: &Rc<Shell>,
    label: &str,
    path: Vec<FolderPathItem>,
    current: bool,
    depth: usize,
    translate: bool,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("folder-tree-row");
    button.set_hexpand(true);
    button.set_halign(gtk::Align::Fill);
    if current {
        button.add_css_class("active");
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_hexpand(true);
    content.set_margin_start((depth as i32) * 12);
    content.append(&gtk::Image::from_icon_name("rufin-route-folders-symbolic"));
    let text = gtk::Label::new(Some(&display_label(label, translate)));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&text);
    button.set_child(Some(&content));

    let shell = Rc::clone(shell);
    button.connect_clicked(move |_| {
        shell.navigate(Route::Folders { path: path.clone() });
    });
    button
}

fn populate_folder_table(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    path: &[FolderPathItem],
    detail: &FolderDetail,
    query: &str,
) {
    let query = query.trim().to_lowercase();
    let mut folders = detail
        .folders
        .iter()
        .filter(|folder| query.is_empty() || folder.name.to_lowercase().contains(&query))
        .cloned()
        .collect::<Vec<_>>();
    folders.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut tracks = detail
        .tracks
        .iter()
        .filter(|track| query.is_empty() || track_matches_query(track, &query))
        .cloned()
        .collect::<Vec<_>>();
    let settings = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::Tracks);
    sort_tracks(&mut tracks, &settings, false);

    let visible_tracks = Arc::new(tracks);
    let source = Rc::new((path.to_vec(), query.clone(), settings.clone()));
    let mut rows = folders
        .into_iter()
        .map(|folder| FolderTableRow::Folder {
            name: folder.name.clone(),
            path: path_for_child(path, &folder),
        })
        .collect::<Vec<_>>();
    rows.extend(
        visible_tracks
            .iter()
            .enumerate()
            .map(|(position, track)| FolderTableRow::Track {
                track: track.clone(),
                tracks: Arc::clone(&visible_tracks),
                source: Rc::clone(&source),
                position,
            }),
    );
    if rows.is_empty() {
        rows.push(FolderTableRow::Empty);
    }
    let rows = rows
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &rows);
}

fn folder_table(shell: &Rc<Shell>, model: &gio::ListStore) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);

    let table = gtk::ColumnView::new(Some(selection.clone()));
    table.add_css_class("folders-table");
    table.add_css_class("data-table");
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_halign(gtk::Align::Fill);
    table.set_vexpand(true);

    let name = folder_name_column(shell);
    name.set_expand(true);
    table.append_column(&name);

    let detail = folder_detail_column(shell);
    detail.set_expand(true);
    table.append_column(&detail);

    table.append_column(&folder_duration_column(shell));

    let row_factory = gtk::SignalListItemFactory::new();
    row_factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ColumnViewRow>() else {
            return;
        };
        let Some(row) = folder_table_row_from_object(item.item()) else {
            return;
        };
        let active = !matches!(&row, FolderTableRow::Empty);
        item.set_selectable(active);
        item.set_activatable(active);
        item.set_accessible_label(&row.accessible_label());
    });
    row_factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ColumnViewRow>() {
            item.set_selectable(true);
            item.set_activatable(true);
            item.set_accessible_label("");
        }
    });
    table.set_row_factory(Some(&row_factory));

    let activate_shell = Rc::clone(shell);
    let activate_model = model.clone();
    table.connect_activate(move |_, position| {
        let Some(row) = folder_table_row_at(&activate_model, position) else {
            return;
        };
        activate_folder_table_row(&activate_shell, &row);
    });

    let menu_shell = Rc::clone(shell);
    let menu_model = model.clone();
    let menu_table = table.downgrade();
    let menu_key = gtk::EventControllerKey::new();
    menu_key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        let Some(FolderTableRow::Track { track, .. }) =
            folder_table_row_at(&menu_model, selection.selected())
        else {
            return glib::Propagation::Proceed;
        };
        let Some(table) = menu_table.upgrade() else {
            return glib::Propagation::Proceed;
        };
        present_track_context_menu(&table.upcast(), &menu_shell, track, None);
        glib::Propagation::Stop
    });
    table.add_controller(menu_key);
    table
}

fn folder_name_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        item.set_child(Some(&folder_table_cell()));
    });
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = folder_table_row_from_object(item.item()) else {
            return;
        };
        let Some(cell) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        else {
            return;
        };
        clear_folder_table_cell(&cell);
        let content = folder_table_cell_content();

        match &row {
            FolderTableRow::Folder { name, .. } => {
                content.append(&gtk::Image::from_icon_name("rufin-route-folders-symbolic"));
                content.append(&folder_table_label(name, 20));
            }
            FolderTableRow::Track { track, .. } => {
                content.append(&shell.cover_tile_for_candidates(
                    ArtworkBinding::track(track),
                    stable_seed(track.id.as_str()),
                    FOLDER_ROW_ARTWORK_SIZE,
                    THUMB_COVER_SIZE,
                ));
                content.append(&folder_table_label(&track.title, 20));
            }
            FolderTableRow::Empty => {
                let label = folder_table_label("", 28);
                bind_label_text(&label, "No folder contents found.");
                bind_widget_tooltip(&label, "No folder contents found.");
                label.add_css_class("muted");
                content.append(&label);
            }
        }
        install_folder_table_cell_interactions(&content, &shell, &row);
        cell.append(&content);
    });
    factory.connect_unbind(|_, item| {
        if let Some(cell) = item
            .downcast_ref::<gtk::ListItem>()
            .and_then(gtk::ListItem::child)
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        {
            clear_folder_table_cell(&cell);
        }
    });
    localized_column("Name", &factory)
}

fn folder_detail_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    folder_text_column(shell, msgid("Artist / Album"), 18, |row| match row {
        FolderTableRow::Folder { .. } => FolderCellText::Localized(msgid("Folder")),
        FolderTableRow::Track { track, .. } => {
            FolderCellText::Plain(format!("{} / {}", track.artist, track.album))
        }
        FolderTableRow::Empty => FolderCellText::Plain(String::new()),
    })
}

fn folder_duration_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    folder_text_column(shell, "Duration", 8, |row| match row {
        FolderTableRow::Track { track, .. } => {
            FolderCellText::Plain(format_duration(track.duration_seconds))
        }
        FolderTableRow::Folder { .. } | FolderTableRow::Empty => {
            FolderCellText::Plain(String::new())
        }
    })
}

fn folder_text_column(
    shell: &Rc<Shell>,
    title: &str,
    max_width_chars: i32,
    text: impl Fn(&FolderTableRow) -> FolderCellText + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let text = Rc::new(text);
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        item.set_child(Some(&folder_table_cell()));
    });
    let shell = Rc::clone(shell);
    let text_for_bind = Rc::clone(&text);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = folder_table_row_from_object(item.item()) else {
            return;
        };
        let Some(cell) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        else {
            return;
        };
        clear_folder_table_cell(&cell);
        let content = folder_table_cell_content();
        let (text, localized_message) = text_for_bind(&row).resolve();
        let label = folder_table_label(&text, max_width_chars);
        if let Some(message) = localized_message {
            bind_label_text(&label, message);
            bind_widget_tooltip(&label, message);
        }
        content.append(&label);
        install_folder_table_cell_interactions(&content, &shell, &row);
        cell.append(&content);
    });
    factory.connect_unbind(|_, item| {
        if let Some(cell) = item
            .downcast_ref::<gtk::ListItem>()
            .and_then(gtk::ListItem::child)
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        {
            clear_folder_table_cell(&cell);
        }
    });
    localized_column(title, &factory)
}

fn folder_table_cell() -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    cell.add_css_class("folder-table-row");
    cell.set_width_request(1);
    cell.set_hexpand(true);
    cell.set_halign(gtk::Align::Fill);
    cell
}

fn folder_table_cell_content() -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_width_request(1);
    content.set_hexpand(true);
    content.set_halign(gtk::Align::Fill);
    content
}

fn folder_table_label(text: &str, max_width_chars: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_width_request(1);
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Fill);
    label.set_wrap(false);
    label.set_single_line_mode(true);
    label.set_width_chars(1);
    label.set_max_width_chars(max_width_chars);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if !text.is_empty() {
        label.set_tooltip_text(Some(text));
    }
    label
}

fn clear_folder_table_cell(cell: &gtk::Box) {
    while let Some(child) = cell.first_child() {
        cell.remove(&child);
    }
}

fn install_folder_table_cell_interactions(
    target: &gtk::Box,
    shell: &Rc<Shell>,
    row: &FolderTableRow,
) {
    match row {
        FolderTableRow::Folder { path, .. } => {
            let path = path.clone();
            let shell = Rc::clone(shell);
            let click = gtk::GestureClick::new();
            click.set_button(gtk::gdk::BUTTON_PRIMARY);
            click.connect_released(move |_, n_press, _, _| {
                if n_press == 1 {
                    shell.navigate(Route::Folders { path: path.clone() });
                }
            });
            target.add_controller(click);
        }
        FolderTableRow::Track { track, .. } => {
            install_track_context_menu(target, shell, track.clone());
        }
        FolderTableRow::Empty => {}
    }
}

fn activate_folder_table_row(shell: &Rc<Shell>, row: &FolderTableRow) {
    match row {
        FolderTableRow::Folder { path, .. } => {
            shell.navigate(Route::Folders { path: path.clone() });
        }
        FolderTableRow::Track {
            tracks,
            source,
            position,
            ..
        } => {
            let (path, query, settings) = source.as_ref();
            shell
                .products
                .playback
                .queue
                .play_folder_window(FolderWindowPlayRequest {
                    path: path.iter().map(|entry| entry.name.clone()).collect(),
                    query: query.clone(),
                    sort: settings.sort_key.track_sort(),
                    descending: settings.descending,
                    tracks: Arc::clone(tracks),
                    anchor_index: *position,
                });
        }
        FolderTableRow::Empty => {}
    }
}

fn folder_table_row_at(model: &gio::ListStore, position: u32) -> Option<FolderTableRow> {
    folder_table_row_from_object(model.item(position))
}

fn folder_table_row_from_object(item: Option<glib::Object>) -> Option<FolderTableRow> {
    item.and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        .map(|item| item.borrow::<FolderTableRow>().clone())
}

impl FolderTableRow {
    fn accessible_label(&self) -> String {
        match self {
            Self::Folder { name, .. } => name.clone(),
            Self::Track { track, .. } => {
                format!("{} — {} — {}", track.title, track.artist, track.album)
            }
            Self::Empty => tr("No folder contents found."),
        }
    }
}

fn folder_tree_width(route_width: i32) -> i32 {
    if route_width < 760 {
        (route_width / 3).clamp(FOLDER_TREE_MIN_WIDTH, FOLDER_TREE_WIDTH)
    } else {
        FOLDER_TREE_WIDTH
    }
}

fn folder_tree_visible(route_width: i32) -> bool {
    route_width >= FOLDER_TREE_HIDE_WIDTH
}

fn display_label(label: &str, translate: bool) -> String {
    if translate {
        tr(label)
    } else {
        label.to_string()
    }
}

fn folder_error_view() -> (gtk::Widget, gtk::Label) {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
    wrapper.add_css_class("empty-state");
    wrapper.set_vexpand(true);
    wrapper.set_hexpand(true);
    wrapper.set_valign(gtk::Align::Center);
    wrapper.set_halign(gtk::Align::Center);

    let heading = localized_label("Folder browsing failed");
    heading.add_css_class("section-heading");
    let body = gtk::Label::new(None);
    body.add_css_class("muted");
    body.set_wrap(true);
    body.set_justify(gtk::Justification::Center);
    wrapper.append(&heading);
    wrapper.append(&body);
    (wrapper.upcast(), body)
}

fn path_for_child(path: &[FolderPathItem], folder: &Folder) -> Vec<FolderPathItem> {
    let mut next = path.to_vec();
    next.push(FolderPathItem {
        id: folder.id.clone(),
        name: folder.name.clone(),
    });
    next
}

#[cfg(test)]
mod tests {
    #[test]
    fn folders_hide_tree_at_tiny_width() {
        assert!(!super::folder_tree_visible(450));
        assert!(!super::folder_tree_visible(549));
        assert!(super::folder_tree_visible(550));
    }
}

impl Shell {
    pub(crate) fn folders_route(self: &Rc<Self>, path: Vec<FolderPathItem>) -> MountedRoute {
        let load_path = path
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        let projection = FolderRouteProjection::new(self, path);
        let apply_loaded: Rc<dyn Fn(Result<FolderDetail, String>)> = {
            let projection = Rc::clone(&projection);
            Rc::new(move |result| projection.apply_refresh(result))
        };
        let load_library = self.products.library.clone();
        let load: MountedRefreshLoader<Result<FolderDetail, String>> = Arc::new(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                load_library.folder(&load_path)
            }))
            .unwrap_or_else(|_| {
                warn!("folder load task panicked");
                Err("Folder load task failed.".to_string())
            })
        });
        let refresh =
            MountedRouteRefresh::new(Rc::downgrade(&apply_loaded), load, "mounted Folders");
        projection.begin_refresh();
        refresh.request();

        let affected_by =
            Rc::new(|delta: &LibraryDelta| delta.reset.is_some() || delta.folders_changed);
        let apply_delta = {
            let projection = Rc::clone(&projection);
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &LibraryDelta| {
                let _ = &apply_loaded;
                projection.begin_refresh();
                refresh.request();
            }) as MountedRouteDeltaApplier
        };

        MountedRoute::new(
            projection.widget(),
            affected_by,
            apply_delta,
            Rc::new(|| {}),
        )
    }
}
