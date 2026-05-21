use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{Folder, FolderPathItem, Route, Track, format_duration};
use rufin_provider::FolderDetail;

use super::{
    PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START, Shell, THUMB_COVER_SIZE,
    install_track_context_menu, sort_tracks_with_options, stable_seed, track_matches_query,
};
use crate::i18n::tr;

const FOLDER_TREE_WIDTH: i32 = 260;
const FOLDER_DURATION_COLUMN_WIDTH: i32 = 72;
const FOLDER_NAME_COLUMN_MIN_WIDTH: i32 = 260;
const FOLDER_NAME_COLUMN_MAX_WIDTH: i32 = 520;
const FOLDER_NAME_TEXT_AVERAGE_WIDTH: i32 = 7;
const FOLDER_ROW_ARTWORK_SIZE: i32 = 28;

impl Shell {
    pub(super) fn folders_view(self: &Rc<Self>, path: Vec<FolderPathItem>) -> gtk::Widget {
        let mut state = self.state.folder_state.borrow().clone();
        if state.path != path || (!state.loading && state.detail.is_none() && state.error.is_none())
        {
            self.start_folder_load(path.clone());
            state = self.state.folder_state.borrow().clone();
        }

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("route-content");
        wrapper.add_css_class("folders-route");
        wrapper.set_margin_top(18);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);

        wrapper.append(&folder_breadcrumbs(self, &path));

        if state.loading {
            wrapper.append(&self.route_empty_view("Loading folders..."));
            return wrapper.upcast();
        }

        if let Some(error) = state.error.as_deref() {
            wrapper.append(&folder_error_view(error));
            return wrapper.upcast();
        }

        let Some(detail) = state.detail else {
            wrapper.append(&self.route_empty_view("No folder contents found."));
            return wrapper.upcast();
        };

        let search = gtk::SearchEntry::new();
        search.add_css_class("folder-search");
        search.set_placeholder_text(Some(&tr("Search current folder")));
        wrapper.append(&search);

        let content = gtk::Paned::new(gtk::Orientation::Horizontal);
        content.add_css_class("folders-split");
        content.set_position(FOLDER_TREE_WIDTH);
        content.set_wide_handle(false);
        content.set_hexpand(true);
        content.set_vexpand(true);

        let tree = folder_tree(self, &path, &detail);
        let tree_scroller = gtk::ScrolledWindow::new();
        tree_scroller.add_css_class("folders-tree-pane");
        tree_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        tree_scroller.set_min_content_width(FOLDER_TREE_WIDTH);
        tree_scroller.set_size_request(FOLDER_TREE_WIDTH, -1);
        tree_scroller.set_hexpand(false);
        tree_scroller.set_vexpand(true);
        tree_scroller.set_child(Some(&tree));
        content.set_start_child(Some(&tree_scroller));
        content.set_resize_start_child(false);
        content.set_shrink_start_child(false);

        let table = gtk::ListBox::new();
        table.add_css_class("folders-table");
        table.set_selection_mode(gtk::SelectionMode::Single);
        table.set_hexpand(true);
        table.set_vexpand(true);
        populate_folder_table(self, &table, &path, &detail, "");

        let table_scroller = gtk::ScrolledWindow::new();
        table_scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        table_scroller.set_min_content_width(0);
        table_scroller.set_hexpand(true);
        table_scroller.set_vexpand(true);
        table_scroller.set_child(Some(&table));
        content.set_end_child(Some(&table_scroller));
        content.set_resize_end_child(true);
        content.set_shrink_end_child(true);

        let table_for_search = table.clone();
        let detail_for_search = detail.clone();
        let path_for_search = path.clone();
        let shell_for_search = Rc::clone(self);
        search.connect_search_changed(move |entry| {
            populate_folder_table(
                &shell_for_search,
                &table_for_search,
                &path_for_search,
                &detail_for_search,
                entry.text().as_str(),
            );
        });

        wrapper.append(&content);
        wrapper.upcast()
    }
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

fn folder_tree(shell: &Rc<Shell>, path: &[FolderPathItem], detail: &FolderDetail) -> gtk::ListBox {
    let tree = gtk::ListBox::new();
    tree.add_css_class("folder-tree");
    tree.set_selection_mode(gtk::SelectionMode::None);
    tree.set_size_request(FOLDER_TREE_WIDTH, -1);
    tree.set_hexpand(true);
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

    tree
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
    content.set_halign(gtk::Align::Fill);
    content.set_margin_start((depth as i32) * 12);
    content.append(&gtk::Image::from_icon_name("folder-symbolic"));
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
    table: &gtk::ListBox,
    path: &[FolderPathItem],
    detail: &FolderDetail,
    query: &str,
) {
    while let Some(child) = table.first_child() {
        table.remove(&child);
    }

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
    let settings = shell.state.settings.borrow().track_table.clone();
    sort_tracks_with_options(&mut tracks, &settings, false);

    let name_column_width = name_column_width(&folders, &tracks);
    table.append(&folder_table_header(name_column_width));

    for folder in folders {
        table.append(&folder_table_folder_row(
            shell,
            path,
            &folder,
            name_column_width,
        ));
    }

    let visible_tracks = Rc::new(tracks);
    for (position, track) in visible_tracks.iter().enumerate() {
        table.append(&folder_table_track_row(
            shell,
            table,
            Rc::clone(&visible_tracks),
            position,
            track,
            name_column_width,
        ));
    }

    if table
        .first_child()
        .and_then(|child| child.next_sibling())
        .is_none()
    {
        table.append(&folder_table_empty_row());
    }
}

fn folder_table_header(name_column_width: i32) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.add_css_class("folder-table-header");

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&name_header_label(name_column_width));
    content.append(&table_header_label("Artist / Album"));
    content.append(&duration_header_label());
    row.set_child(Some(&content));
    row
}

fn name_header_label(name_column_width: i32) -> gtk::Label {
    let text = gtk::Label::new(Some(&tr("Name")));
    text.add_css_class("muted");
    text.set_xalign(0.0);
    text.set_hexpand(false);
    text.set_width_request(name_column_width);
    text
}

fn table_header_label(label: &str) -> gtk::Label {
    let text = gtk::Label::new(Some(&tr(label)));
    text.add_css_class("muted");
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_width_chars(18);
    text
}

fn duration_header_label() -> gtk::Label {
    let text = gtk::Label::new(Some(&tr("Duration")));
    text.add_css_class("muted");
    text.set_xalign(1.0);
    text.set_hexpand(false);
    text.set_width_request(FOLDER_DURATION_COLUMN_WIDTH);
    text
}

fn folder_table_folder_row(
    shell: &Rc<Shell>,
    path: &[FolderPathItem],
    folder: &Folder,
    name_column_width: i32,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("folder-table-row");
    button.add_css_class("folder-table-folder-row");

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&folder_name_cell(&folder.name, name_column_width));
    content.append(&detail_cell(&tr("Folder")));
    content.append(&duration_cell(""));
    button.set_child(Some(&content));

    let shell = Rc::clone(shell);
    let next_path = path_for_child(path, folder);
    button.connect_clicked(move |_| {
        shell.navigate(Route::Folders {
            path: next_path.clone(),
        });
    });
    button
}

fn folder_table_track_row(
    shell: &Rc<Shell>,
    table: &gtk::ListBox,
    visible_tracks: Rc<Vec<Track>>,
    position: usize,
    track: &Track,
    name_column_width: i32,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("folder-table-row");
    row.set_selectable(true);
    row.set_activatable(true);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&track_name_cell(shell, track, name_column_width));
    content.append(&detail_cell(&format!("{} / {}", track.artist, track.album)));
    content.append(&duration_cell(&format_duration(track.duration_seconds)));
    row.set_child(Some(&content));

    let row_for_click = row.clone();
    let table_for_click = table.clone();
    let controller = shell.controller.clone();
    let tracks_for_click = Rc::clone(&visible_tracks);
    let gesture = gtk::GestureClick::new();
    gesture.connect_released(move |_, n_press, _, _| {
        table_for_click.select_row(Some(&row_for_click));
        row_for_click.grab_focus();
        if n_press == 2 {
            controller.play_tracks_now(rotated_from_position(&tracks_for_click, position));
        }
    });
    row.add_controller(gesture);
    install_track_context_menu(&row, shell, track.clone());

    row
}

fn folder_name_cell(text: &str, width: i32) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    cell.set_hexpand(false);
    cell.set_width_request(width);
    cell.append(&gtk::Image::from_icon_name("folder-symbolic"));
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    cell.append(&label);
    cell
}

fn track_name_cell(shell: &Rc<Shell>, track: &Track, width: i32) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    cell.set_hexpand(false);
    cell.set_width_request(width);
    cell.append(&shell.cover_tile_for(
        track.image_ref.as_ref(),
        stable_seed(track.id.as_str()),
        FOLDER_ROW_ARTWORK_SIZE,
        THUMB_COVER_SIZE,
    ));
    let label = gtk::Label::new(Some(&track.title));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    cell.append(&label);
    cell
}

fn detail_cell(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_width_chars(16);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn duration_cell(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(1.0);
    label.set_hexpand(false);
    label.set_width_request(FOLDER_DURATION_COLUMN_WIDTH);
    label
}

fn name_column_width(folders: &[Folder], tracks: &[Track]) -> i32 {
    let longest = folders
        .iter()
        .map(|folder| folder.name.chars().count())
        .chain(tracks.iter().map(|track| track.title.chars().count()))
        .max()
        .unwrap_or(0);
    (longest as i32 * FOLDER_NAME_TEXT_AVERAGE_WIDTH + FOLDER_ROW_ARTWORK_SIZE + 24)
        .clamp(FOLDER_NAME_COLUMN_MIN_WIDTH, FOLDER_NAME_COLUMN_MAX_WIDTH)
}

fn display_label(label: &str, translate: bool) -> String {
    if translate {
        tr(label)
    } else {
        label.to_string()
    }
}

fn folder_table_empty_row() -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.add_css_class("folder-table-empty-row");
    row.set_child(Some(&gtk::Label::new(Some(&tr(
        "No folder contents found.",
    )))));
    row
}

fn folder_error_view(error: &str) -> gtk::Widget {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
    wrapper.add_css_class("empty-state");
    wrapper.set_vexpand(true);
    wrapper.set_hexpand(true);
    wrapper.set_valign(gtk::Align::Center);
    wrapper.set_halign(gtk::Align::Center);

    let heading = gtk::Label::new(Some(&tr("Folder browsing failed")));
    heading.add_css_class("section-heading");
    let body = gtk::Label::new(Some(error));
    body.add_css_class("muted");
    body.set_wrap(true);
    body.set_justify(gtk::Justification::Center);
    wrapper.append(&heading);
    wrapper.append(&body);
    wrapper.upcast()
}

fn path_for_child(path: &[FolderPathItem], folder: &Folder) -> Vec<FolderPathItem> {
    let mut next = path.to_vec();
    next.push(FolderPathItem {
        id: folder.id.clone(),
        name: folder.name.clone(),
    });
    next
}

fn rotated_from_position<T: Clone>(items: &[T], position: usize) -> Vec<T> {
    if items.is_empty() || position >= items.len() {
        return Vec::new();
    }
    items[position..]
        .iter()
        .chain(items[..position].iter())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn rotated_from_position_queues_clicked_item_first() {
        assert_eq!(
            super::rotated_from_position(&[1, 2, 3, 4], 2),
            vec![3, 4, 1, 2]
        );
    }

    #[test]
    fn name_column_width_uses_content_without_unbounded_growth() {
        let folders = Vec::new();
        let tracks = vec![
            rufin_core::Track {
                id: rufin_core::TrackId::new("track-short"),
                album_id: rufin_core::AlbumId::new("album-one"),
                title: "Short".to_string(),
                artist: String::new(),
                artist_id: None,
                artist_credits: Vec::new(),
                album_artist_credits: Vec::new(),
                album: String::new(),
                year: 0,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                duration_seconds: 0,
                favorite: false,
                disc_number: 1,
                track_number: 1,
                image_ref: None,
                genres: Vec::new(),
                local_path: None,
            },
            rufin_core::Track {
                id: rufin_core::TrackId::new("track-long"),
                album_id: rufin_core::AlbumId::new("album-one"),
                title: "A Very Long Track Title That Still Should Not Own The Whole Row"
                    .to_string(),
                artist: String::new(),
                artist_id: None,
                artist_credits: Vec::new(),
                album_artist_credits: Vec::new(),
                album: String::new(),
                year: 0,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                duration_seconds: 0,
                favorite: false,
                disc_number: 1,
                track_number: 2,
                image_ref: None,
                genres: Vec::new(),
                local_path: None,
            },
        ];

        let width = super::name_column_width(&folders, &tracks);

        assert!(width > super::FOLDER_NAME_COLUMN_MIN_WIDTH);
        assert!(width <= super::FOLDER_NAME_COLUMN_MAX_WIDTH);
    }
}
