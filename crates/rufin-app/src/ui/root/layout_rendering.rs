impl PlaylistEntrySort {
    fn title(self) -> &'static str {
        match self {
            Self::Order => "Playlist order",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Duration => "Duration",
        }
    }
}
const PLAYLIST_ENTRY_SORTS: [PlaylistEntrySort; 5] = [
    PlaylistEntrySort::Order,
    PlaylistEntrySort::Title,
    PlaylistEntrySort::Artist,
    PlaylistEntrySort::Album,
    PlaylistEntrySort::Duration,
];
#[derive(Clone, Debug)]
struct PlaylistEntryListState {
    query: String,
    sort: PlaylistEntrySort,
    descending: bool,
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
fn rebuild_playlist_entries_list(
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

    for (display_index, (original_index, entry)) in rows.into_iter().enumerate() {
        list.append(&playlist_entry_row(
            shell,
            Rc::clone(entries),
            playlist_id,
            original_index,
            display_index,
            &entry,
        ));
    }
}
fn playlist_entries_for_state(
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
fn playlist_entry_matches_query(entry: &PlaylistEntry, query: &str) -> bool {
    entry.track.title.to_lowercase().contains(query)
        || entry.track.artist.to_lowercase().contains(query)
        || entry.track.album.to_lowercase().contains(query)
}
fn compare_playlist_entry(
    left: &(usize, PlaylistEntry),
    right: &(usize, PlaylistEntry),
    sort: PlaylistEntrySort,
) -> std::cmp::Ordering {
    match sort {
        PlaylistEntrySort::Order => left.0.cmp(&right.0),
        PlaylistEntrySort::Title => cmp_text(&left.1.track.title, &right.1.track.title),
        PlaylistEntrySort::Artist => cmp_text(&left.1.track.artist, &right.1.track.artist),
        PlaylistEntrySort::Album => cmp_text(&left.1.track.album, &right.1.track.album),
        PlaylistEntrySort::Duration => left
            .1
            .track
            .duration_seconds
            .cmp(&right.1.track.duration_seconds),
    }
    .then_with(|| left.0.cmp(&right.0))
}
fn cmp_text(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}
fn playlist_entries_header_row() -> gtk::Widget {
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
        "Duration",
        PLAYLIST_ENTRY_DURATION_WIDTH,
        false,
        0.5,
    ));
    row.append(&fixed_spacer(PLAYLIST_ENTRY_REMOVE_WIDTH));
    row.upcast()
}
fn playlist_header_label(text: &str, width: i32, expand: bool, xalign: f32) -> gtk::Label {
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
fn playlist_header_text_label(text: &str, max_width_chars: i32) -> gtk::Label {
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
fn playlist_header_album_label(text: &str, max_width_chars: i32) -> gtk::Label {
    let label = playlist_header_text_label(text, max_width_chars);
    label.set_xalign(0.5);
    label
}
fn playlist_text_columns(title: gtk::Widget, album: gtk::Widget) -> gtk::Widget {
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_TEXT_COLUMN_GAP);
    columns.set_homogeneous(false);
    columns.set_hexpand(true);
    columns.set_halign(gtk::Align::Fill);
    columns.set_width_request(1);

    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    columns.append(&title);

    album.set_hexpand(false);
    album.set_halign(gtk::Align::Fill);
    album.set_width_request(PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH);
    columns.append(&album);

    columns.upcast()
}
fn playlist_title_cell(cover: gtk::Widget, labels: gtk::Widget) -> gtk::Widget {
    let title = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    title.append(&cover);
    title.append(&labels);
    title.upcast()
}
fn playlist_entry_row(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: &PlaylistId,
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

    let duration = gtk::Label::new(Some(&format_duration(entry.track.duration_seconds)));
    duration.add_css_class("muted");
    duration.set_xalign(0.5);
    duration.set_width_request(PLAYLIST_ENTRY_DURATION_WIDTH);
    row.append(&duration);

    let remove = gtk::Button::with_label("x");
    remove.add_css_class("icon-button");
    remove.add_css_class("flat");
    remove.add_css_class("circular");
    remove.set_tooltip_text(Some(&tr("Remove from playlist")));
    remove.set_width_request(PLAYLIST_ENTRY_REMOVE_WIDTH);
    let remove_shell = Rc::clone(shell);
    let remove_playlist_id = playlist_id.clone();
    let remove_entry_id = entry.entry_id.clone();
    let remove_title = entry.track.title.clone();
    remove.connect_clicked(move |_| {
        confirm_remove_playlist_entry(
            &remove_shell,
            remove_playlist_id.clone(),
            remove_entry_id.clone(),
            remove_title.clone(),
        );
    });
    row.append(&remove);

    let controller = shell.controller.clone();
    let track = entry.track.clone();
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |gesture, n_press, _, _| {
        if n_press == 2 {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            controller.play_now(track.clone());
        }
    });
    row.add_controller(click);
    install_track_context_menu(&row, shell, entry.track.clone());

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
fn playlist_drop_index(
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
fn playlist_entry_text_label(text: &str, css_class: &str, max_width_chars: i32) -> gtk::Label {
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
fn fixed_spacer(width: i32) -> gtk::Widget {
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_width_request(width);
    spacer.upcast()
}
fn confirm_remove_playlist_entry(
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
fn seekbar_target_seconds(value: f64, duration_seconds: u32) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(0.0, f64::from(duration_seconds)) as u32
}
fn set_active_class(widget: &impl IsA<gtk::Widget>, active: bool) {
    if active {
        widget.add_css_class("active-toggle");
    } else {
        widget.remove_css_class("active-toggle");
    }
}
fn favorite_icon_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(FAVORITE_EMPTY_GLYPH);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.add_css_class("favorite-toggle");
    button.set_tooltip_text(Some(&tr(label)));
    button
}
fn set_favorite_button_active(button: &gtk::Button, active: bool) {
    set_active_class(button, active);
    button.set_label(if active {
        FAVORITE_FILLED_GLYPH
    } else {
        FAVORITE_EMPTY_GLYPH
    });
}
fn favorite_button_is_active(button: &gtk::Button) -> bool {
    button.label().as_deref() == Some(FAVORITE_FILLED_GLYPH)
}
fn icon_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon_name);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));
    button
}
fn icon_button_with_image(icon_name: &str, label: &str) -> (gtk::Button, gtk::Image) {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));
    let image = gtk::Image::from_icon_name(icon_name);
    button.set_child(Some(&image));
    (button, image)
}
fn text_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("pill-button");
    button.add_css_class("pill");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
    button
}
fn install_css() {
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
