use super::*;

const ALBUM_DETAIL_ROW_HORIZONTAL_INSET: i32 = 8;
const ALBUM_DETAIL_TRACK_COLUMN_GAP: i32 = 8;
const ALBUM_DETAIL_MAX_COVER: i32 = 168;
const ALBUM_DETAIL_MIN_COVER: i32 = 102;
const ALBUM_DETAIL_NARROW_WIDTH: i32 = 360;
const ALBUM_DETAIL_SEPARATOR_WIDTH: i32 = 1;

pub(in crate::ui) fn populate_album_collection_model(
    model: &gio::ListStore,
    albums: &[Album],
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) {
    if settings.layout == LibraryLayout::Detail {
        replace_album_items(
            model,
            album_detail_items_for(albums, settings, album_tracks),
        );
    } else {
        populate_album_model(model, albums, settings);
    }
}

pub(in crate::ui) fn append_album_collection_model(
    model: &gio::ListStore,
    albums: Vec<Album>,
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) {
    if settings.layout == LibraryLayout::Detail {
        append_album_items(
            model,
            album_detail_items_for(&albums, settings, album_tracks),
        );
    } else {
        append_albums_to_model(model, albums);
    }
}

pub(in crate::ui) fn album_detail_items_for(
    albums: &[Album],
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) -> Vec<AlbumDetailItem> {
    let mut albums = albums.to_vec();
    sort_albums(&mut albums, settings);
    let mut rows = Vec::new();

    for album in albums {
        let mut tracks = album_tracks.get(&album.id).cloned().unwrap_or_default();
        sort_album_detail_tracks(&mut tracks);
        let inline_count = tracks.len().min(ALBUM_DETAIL_INLINE_TRACK_ROWS);
        let remaining_tracks = tracks.split_off(inline_count);
        rows.push(AlbumDetailItem::Lead {
            album,
            inline_tracks: tracks,
            last_in_album: remaining_tracks.is_empty(),
        });
        let remaining_count = remaining_tracks.len();
        for (offset, track) in remaining_tracks.into_iter().enumerate() {
            rows.push(AlbumDetailItem::Track {
                track,
                index: inline_count + offset,
                last_in_album: offset + 1 == remaining_count,
            });
        }
    }

    rows
}

pub(in crate::ui) fn album_detail_list(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::ListView {
    let track_selection = AlbumDetailTrackSelection::default();
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
    let selection_for_factory = track_selection.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<AlbumDetailItem>(item) else {
            return;
        };
        let content = album_detail_item_row(&shell_for_factory, row, key, &selection_for_factory);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        item.set_child(Some(&content));
    });
    factory.connect_unbind(clear_list_item_child);

    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("track-table");
    list.add_css_class("album-detail-list");
    list.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    list.set_hexpand(true);
    list.set_halign(gtk::Align::Fill);
    list.set_vexpand(true);
    let controller = shell.controller.clone();
    list.connect_activate(move |_, position| {
        let Some(AlbumDetailItem::Track { track, .. }) =
            item_at::<AlbumDetailItem>(&model, position)
        else {
            return;
        };
        play_album_track_from_cache(&controller, track);
    });
    list
}
#[derive(Clone)]
pub(in crate::ui) struct AlbumDetailVirtualList {
    pub(in crate::ui) widget: gtk::Box,
    pub(in crate::ui) top_spacer: gtk::Box,
    pub(in crate::ui) rows: gtk::Box,
    pub(in crate::ui) bottom_spacer: gtk::Box,
    pub(in crate::ui) selection: AlbumDetailTrackSelection,
}
#[derive(Clone)]
pub(in crate::ui) struct AlbumDetailVirtualRow {
    pub(in crate::ui) item: AlbumDetailItem,
    pub(in crate::ui) top: i32,
    pub(in crate::ui) height: i32,
}
impl AlbumDetailVirtualRow {
    pub(in crate::ui) fn bottom(&self) -> i32 {
        self.top.saturating_add(self.height)
    }
}
pub(in crate::ui) fn album_detail_virtual_list() -> AlbumDetailVirtualList {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
    widget.add_css_class("track-table");
    widget.add_css_class("album-detail-list");
    widget.set_hexpand(true);
    widget.set_halign(gtk::Align::Fill);
    widget.set_vexpand(false);

    let top_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.set_hexpand(true);
    rows.set_halign(gtk::Align::Fill);
    let bottom_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    widget.append(&top_spacer);
    widget.append(&rows);
    widget.append(&bottom_spacer);

    AlbumDetailVirtualList {
        widget,
        top_spacer,
        rows,
        bottom_spacer,
        selection: AlbumDetailTrackSelection::default(),
    }
}
pub(in crate::ui) fn connect_album_detail_virtual_list(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
    model: &gio::ListStore,
    key: LibraryListKey,
    list: &AlbumDetailVirtualList,
) {
    let rows = Rc::new(RefCell::new(album_detail_virtual_rows(shell, key, model)));
    let rendered = Rc::new(RefCell::new(None::<(usize, usize)>));
    let adjustment = scroller.vadjustment();

    let render = {
        let shell = Rc::clone(shell);
        let list = list.clone();
        let rows = Rc::clone(&rows);
        let rendered = Rc::clone(&rendered);
        Rc::new(move |adjustment: &gtk::Adjustment| {
            render_album_detail_virtual_rows(&shell, key, &list, &rows, &rendered, adjustment);
        })
    };

    render(&adjustment);
    {
        let adjustment = adjustment.clone();
        let render = Rc::clone(&render);
        glib::idle_add_local_once(move || render(&adjustment));
    }

    {
        let shell = Rc::clone(shell);
        let rows = Rc::clone(&rows);
        let rendered = Rc::clone(&rendered);
        let adjustment = adjustment.clone();
        let render = Rc::clone(&render);
        model.connect_items_changed(move |model, _, _, _| {
            *rows.borrow_mut() = album_detail_virtual_rows(&shell, key, model);
            *rendered.borrow_mut() = None;
            render(&adjustment);
        });
    }

    let render_serial = Rc::new(Cell::new(0_u64));
    let last_scroll_value = Rc::new(Cell::new(adjustment.value()));
    adjustment.connect_value_changed(move |adjustment| {
        let previous_value = last_scroll_value.replace(adjustment.value());
        let delta = (adjustment.value() - previous_value).abs();
        let serial = render_serial.get().saturating_add(1);
        render_serial.set(serial);
        let adjustment = adjustment.clone();
        let render = Rc::clone(&render);
        let render_serial_for_callback = Rc::clone(&render_serial);
        if delta >= f64::from(fast_scroll_delta()) {
            glib::timeout_add_local_once(Duration::from_millis(FAST_SCROLL_DELAY), move || {
                if render_serial_for_callback.get() == serial {
                    render(&adjustment);
                }
            });
        } else {
            glib::idle_add_local_once(move || {
                if render_serial_for_callback.get() == serial {
                    render(&adjustment);
                }
            });
        }
    });
}
pub(in crate::ui) fn album_detail_virtual_rows(
    shell: &Shell,
    key: LibraryListKey,
    model: &gio::ListStore,
) -> Vec<AlbumDetailVirtualRow> {
    let cover_size = album_detail_row_metrics(shell, key).cover_size;
    let mut rows = Vec::with_capacity(model.n_items() as usize);
    let mut top = 0_i32;
    for index in 0..model.n_items() {
        let Some(item) = item_at::<AlbumDetailItem>(model, index) else {
            continue;
        };
        let height = album_detail_item_total_height(&item, cover_size);
        rows.push(AlbumDetailVirtualRow { item, top, height });
        top = top.saturating_add(height);
    }
    rows
}
pub(in crate::ui) fn render_album_detail_virtual_rows(
    shell: &Rc<Shell>,
    key: LibraryListKey,
    list: &AlbumDetailVirtualList,
    rows: &Rc<RefCell<Vec<AlbumDetailVirtualRow>>>,
    rendered: &Rc<RefCell<Option<(usize, usize)>>>,
    adjustment: &gtk::Adjustment,
) {
    let rows_ref = rows.borrow();
    let total_height = rows_ref
        .last()
        .map(AlbumDetailVirtualRow::bottom)
        .unwrap_or(1)
        .max(1);
    list.widget.set_height_request(total_height);

    let overscan = f64::from(album_detail_virtual_overscan_height());
    let visible_top = (adjustment.value() - overscan).max(0.0);
    let visible_bottom = (adjustment.value() + adjustment.page_size() + overscan)
        .min(f64::from(total_height))
        .max(visible_top);
    let (start, end) = album_detail_virtual_range(&rows_ref, visible_top, visible_bottom);
    if rendered.borrow().as_ref() == Some(&(start, end)) {
        return;
    }
    *rendered.borrow_mut() = Some((start, end));

    while let Some(child) = list.rows.first_child() {
        list.rows.remove(&child);
    }

    let top_height = rows_ref.get(start).map(|row| row.top).unwrap_or(0).max(0);
    let bottom_start = end
        .checked_sub(1)
        .and_then(|index| rows_ref.get(index))
        .map(AlbumDetailVirtualRow::bottom)
        .unwrap_or(top_height);
    list.top_spacer.set_height_request(top_height);
    list.bottom_spacer
        .set_height_request(total_height.saturating_sub(bottom_start).max(0));

    for row in &rows_ref[start..end] {
        list.rows.append(&album_detail_item_row(
            shell,
            row.item.clone(),
            key,
            &list.selection,
        ));
    }
}
pub(in crate::ui) fn album_detail_virtual_range(
    rows: &[AlbumDetailVirtualRow],
    visible_top: f64,
    visible_bottom: f64,
) -> (usize, usize) {
    let start = rows
        .iter()
        .position(|row| f64::from(row.bottom()) >= visible_top)
        .unwrap_or(rows.len());
    let end = rows
        .iter()
        .position(|row| f64::from(row.top) > visible_bottom)
        .unwrap_or(rows.len())
        .max(start);
    (start, end)
}
pub(in crate::ui) fn album_detail_virtual_overscan_height() -> i32 {
    ALBUM_DETAIL_TRACK_ROW_HEIGHT * 8
}
pub(in crate::ui) const FAST_SCROLL_DELAY: u64 = 90;
pub(in crate::ui) fn fast_scroll_delta() -> i32 {
    album_detail_virtual_overscan_height() / 2
}
#[derive(Clone, Default)]
pub(in crate::ui) struct AlbumDetailTrackSelection {
    pub(in crate::ui) selected_track_id: Rc<RefCell<Option<TrackId>>>,
    pub(in crate::ui) selected_row: Rc<RefCell<Option<gtk::Widget>>>,
}
impl AlbumDetailTrackSelection {
    pub(in crate::ui) fn bind_row(&self, row: &gtk::Widget, track_id: &TrackId) {
        if self.selected_track_id.borrow().as_ref() == Some(track_id) {
            row.add_css_class("album-detail-track-selected");
            *self.selected_row.borrow_mut() = Some(row.clone());
        }
    }

    pub(in crate::ui) fn select_row(&self, row: &gtk::Widget, track_id: TrackId) {
        if let Some(previous) = self.selected_row.borrow_mut().take() {
            previous.remove_css_class("album-detail-track-selected");
        }
        row.add_css_class("album-detail-track-selected");
        *self.selected_track_id.borrow_mut() = Some(track_id);
        *self.selected_row.borrow_mut() = Some(row.clone());
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) enum AlbumDetailItem {
    Lead {
        album: Album,
        inline_tracks: Vec<Track>,
        last_in_album: bool,
    },
    Track {
        track: Track,
        index: usize,
        last_in_album: bool,
    },
}
pub(in crate::ui) fn album_detail_item_row(
    shell: &Rc<Shell>,
    row: AlbumDetailItem,
    key: LibraryListKey,
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    match row {
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album,
        } => album_detail_lead_row(shell, &album, &inline_tracks, last_in_album, key, selection),
        AlbumDetailItem::Track {
            track,
            index,
            last_in_album,
        } => album_detail_track_row(shell, &track, index, last_in_album, key, selection),
    }
}
pub(in crate::ui) fn album_detail_lead_row(
    shell: &Rc<Shell>,
    album: &Album,
    inline_tracks: &[Track],
    last_in_album: bool,
    key: LibraryListKey,
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    let metrics = album_detail_row_metrics(shell, key);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, metrics.spacing);
    let content_height =
        album_detail_lead_content_height(album, metrics.cover_size, inline_tracks.len());
    row.add_css_class("album-detail-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_margin_top(12);
    row.set_margin_bottom(if last_in_album { 16 } else { 0 });
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_height_request(content_height);

    row.append(&album_detail_meta(
        shell,
        album,
        metrics.cover_size,
        metrics.meta_width,
    ));

    let track_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    track_area.set_hexpand(true);
    track_area.set_halign(gtk::Align::Fill);
    if !inline_tracks.is_empty() {
        let fields = shell.library_settings(key).detail_track_fields;
        let track_width =
            album_detail_track_area_width(shell, key, metrics.meta_width, metrics.spacing, &fields);
        let field_widths = album_detail_track_field_widths(key, &fields, track_width);
        track_area.set_width_request(track_width);
        track_area.set_height_request(album_detail_track_area_height(inline_tracks.len()));
        track_area.append(&album_detail_track_header(&field_widths));
        for (index, track) in inline_tracks.iter().enumerate() {
            track_area.append(&album_detail_track_cells(
                shell,
                track,
                index,
                &field_widths,
                selection,
            ));
        }
        row.append(&album_detail_separator(content_height));
    }
    row.append(&track_area);
    row.upcast()
}
pub(in crate::ui) fn album_detail_track_row(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    last_in_album: bool,
    key: LibraryListKey,
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    let metrics = album_detail_row_metrics(shell, key);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, metrics.spacing);
    row.add_css_class("album-detail-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_margin_bottom(if last_in_album { 16 } else { 0 });
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_height_request(ALBUM_DETAIL_TRACK_ROW_HEIGHT);

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_width_request(metrics.meta_width);
    spacer.set_hexpand(false);
    row.append(&spacer);
    row.append(&album_detail_separator(ALBUM_DETAIL_TRACK_ROW_HEIGHT));

    let fields = shell.library_settings(key).detail_track_fields;
    let track_width =
        album_detail_track_area_width(shell, key, metrics.meta_width, metrics.spacing, &fields);
    let field_widths = album_detail_track_field_widths(key, &fields, track_width);
    row.append(&album_detail_track_cells(
        shell,
        track,
        index,
        &field_widths,
        selection,
    ));
    row.upcast()
}
pub(in crate::ui) fn album_detail_separator(height: i32) -> gtk::Widget {
    let separator = gtk::Separator::new(gtk::Orientation::Vertical);
    separator.add_css_class("album-detail-separator");
    separator.set_width_request(ALBUM_DETAIL_SEPARATOR_WIDTH);
    separator.set_height_request(height.max(1));
    separator.set_valign(gtk::Align::Fill);
    separator.upcast()
}
pub(in crate::ui) fn album_detail_meta(
    shell: &Rc<Shell>,
    album: &Album,
    cover_size: i32,
    meta_width: i32,
) -> gtk::Widget {
    let meta = gtk::Box::new(gtk::Orientation::Vertical, ALBUM_DETAIL_META_SPACING);
    meta.set_width_request(meta_width);
    meta.set_height_request(album_detail_meta_height(album, cover_size));
    meta.set_hexpand(false);
    meta.append(&album_detail_cover_tile(shell, album, cover_size));
    meta.append(&album_detail_meta_label(
        &album.title,
        "track-title",
        meta_width,
    ));
    meta.append(&album_detail_meta_label(&album.artist, "muted", meta_width));
    meta.append(&album_detail_meta_label(
        &album_fact_text(album),
        "muted",
        meta_width,
    ));
    if !album.genres.is_empty() {
        meta.append(&album_detail_meta_label(
            &album.genres.join(", "),
            "muted",
            meta_width,
        ));
    }
    meta.upcast()
}
pub(in crate::ui) fn album_detail_cover_tile(
    shell: &Rc<Shell>,
    album: &Album,
    cover_size: i32,
) -> gtk::Widget {
    let overlay = cards::cover_overlay(cover_size);
    let cover_button = gtk::Button::new();
    cover_button.add_css_class("flat");
    cover_button.add_css_class("album-cover-button");
    cards::constrain_cover_widget(&cover_button, cover_size);
    cards::clip_cover(&cover_button);
    cover_button.set_child(Some(&shell.cover_tile_for(
        album.image_ref.as_ref(),
        album.color_seed,
        cover_size,
        GRID_COVER_SIZE,
    )));
    let shell_for_open = Rc::clone(shell);
    let album_id = album.id.clone();
    cover_button.connect_clicked(move |_| {
        shell_for_open.navigate(Route::AlbumDetail(album_id.clone()));
    });
    overlay.set_child(Some(&cover_button));

    let controls = cards::cover_hover_controls(cover_size, "Play album", album.favorite);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    controls
        .play
        .connect_clicked(move |_| controller.play_album_now(album_id.clone()));

    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });

    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            controller.play_last(tracks);
        }
    });

    let favorite = controls.favorite.as_ref().expect("favorite button");
    shell.register_favorite_button(album_favorite_key(&album.id), favorite);
    let favorite_shell = Rc::clone(shell);
    let album_id = album.id.clone();
    favorite.connect_clicked(move |button| {
        let favorite = !favorite_button_is_active(button);
        favorite_shell.set_favorite_with_feedback(
            source::FavoriteItemId::Album(album_id.clone()),
            favorite,
            Some(button),
        );
    });

    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);
    install_album_context_menu(&overlay, shell, album.clone());
    overlay.upcast()
}
pub(in crate::ui) fn album_detail_track_header(
    field_widths: &[(LibraryField, i32)],
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, ALBUM_DETAIL_TRACK_COLUMN_GAP);
    row.add_css_class("album-detail-track-cells");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_width_request(album_detail_track_cells_width(field_widths));
    row.set_height_request(ALBUM_DETAIL_TRACK_HEADER_HEIGHT);
    row.set_margin_end(album_detail_track_trailing_inset(field_widths));
    for (field, width) in field_widths {
        row.append(&album_detail_track_cell_box(
            *field,
            *width,
            ALBUM_DETAIL_TRACK_HEADER_HEIGHT,
            album_detail_track_header_cell(*field, *width),
        ));
    }
    row.upcast()
}
pub(in crate::ui) fn album_detail_track_header_cell(
    field: LibraryField,
    width: i32,
) -> gtk::Widget {
    if field == LibraryField::Duration {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_width_request(width);
        row.set_halign(gtk::Align::Fill);
        let image = gtk::Image::from_icon_name("appointment-soon-symbolic");
        let label = tr("Duration");
        image.add_css_class("muted");
        image.set_halign(gtk::Align::Start);
        image.set_tooltip_text(Some(&label));
        image.update_property(&[gtk::accessible::Property::Label(&label)]);
        row.append(&image);
        return row.upcast();
    }

    let label = album_detail_track_label(&tr(field.title()), field, width, false);
    label.add_css_class("muted");
    label.upcast()
}
pub(in crate::ui) fn album_detail_track_cells(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    field_widths: &[(LibraryField, i32)],
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, ALBUM_DETAIL_TRACK_COLUMN_GAP);
    row.add_css_class("album-detail-track-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_width_request(album_detail_track_cells_width(field_widths));
    row.set_height_request(ALBUM_DETAIL_TRACK_ROW_HEIGHT);
    row.set_valign(gtk::Align::Center);
    row.set_margin_end(album_detail_track_trailing_inset(field_widths));
    row.set_focusable(false);
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "{} {}",
        track.title, track.artist
    ))]);
    selection.bind_row(row.upcast_ref(), &track.id);
    for (field, width) in field_widths {
        row.append(&album_detail_track_cell(
            shell, track, index, *field, *width,
        ));
    }
    install_track_context_menu(&row, shell, track.clone());

    let controller = shell.controller.clone();
    let track = track.clone();
    let selection = selection.clone();
    let row_for_click = row.clone();
    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    gesture.set_button(1);
    gesture.connect_pressed(move |gesture, n_press, _, _| {
        if album_detail_play_click(n_press) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let controller = controller.clone();
            let track = track.clone();
            glib::idle_add_local_once(move || play_album_track_from_cache(&controller, track));
        } else if n_press == 1 {
            selection.select_row(row_for_click.upcast_ref(), track.id.clone());
        }
    });
    row.add_controller(gesture);
    row.upcast()
}
fn album_detail_play_click(n_press: i32) -> bool {
    n_press >= 2 && n_press % 2 == 0
}

fn play_album_track_from_cache(controller: &crate::controller::AppController, track: Track) {
    let Ok(Some((album, tracks))) = controller.cached_album_detail(&track.album_id) else {
        controller.play_now(track);
        return;
    };
    let Some(anchor_index) = tracks.iter().position(|candidate| candidate.id == track.id) else {
        controller.play_now(track);
        return;
    };
    controller.play_album_tracks(album.id, tracks, anchor_index, false);
}
pub(in crate::ui) fn album_detail_track_cell(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    field: LibraryField,
    width: i32,
) -> gtk::Widget {
    match field {
        LibraryField::Favorite => {
            let button = favorite_icon_button("Favorite track");
            set_favorite_button_active(&button, track.favorite);
            let favorite_shell = Rc::clone(shell);
            let track_id = track.id.clone();
            button.connect_clicked(move |button| {
                let favorite = !favorite_button_is_active(button);
                favorite_shell.set_favorite_with_feedback(
                    source::FavoriteItemId::Track(track_id.clone()),
                    favorite,
                    Some(button),
                );
            });
            button.set_halign(gtk::Align::Center);
            album_detail_fixed_cell(width, button.upcast())
        }
        LibraryField::Image => {
            let cover = shell.cover_tile_for(
                track.image_ref.as_ref(),
                stable_seed(track.id.as_str()),
                48,
                THUMB_COVER_SIZE,
            );
            cover.set_halign(gtk::Align::Center);
            album_detail_fixed_cell(width, cover)
        }
        _ => album_detail_track_cell_box(
            field,
            width,
            ALBUM_DETAIL_TRACK_ROW_HEIGHT,
            album_detail_track_label(
                &album_detail_track_text(track, index, field),
                field,
                width,
                true,
            )
            .upcast(),
        ),
    }
}
pub(in crate::ui) fn album_detail_fixed_cell(width: i32, child: gtk::Widget) -> gtk::Widget {
    album_detail_fixed_cell_height(width, ALBUM_DETAIL_TRACK_ROW_HEIGHT, child)
}
pub(in crate::ui) fn album_detail_track_cell_box(
    field: LibraryField,
    width: i32,
    height: i32,
    child: gtk::Widget,
) -> gtk::Widget {
    if field == LibraryField::Title {
        return album_detail_expanding_cell_height(width, height, child);
    }

    album_detail_fixed_cell_height(width, height, child)
}
pub(in crate::ui) fn album_detail_expanding_cell_height(
    width: i32,
    height: i32,
    child: gtk::Widget,
) -> gtk::Widget {
    let width = width.max(1);
    let height = height.max(1);
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    cell.set_width_request(width);
    cell.set_height_request(height);
    cell.set_size_request(width, height);
    cell.set_hexpand(true);
    cell.set_halign(gtk::Align::Fill);
    cell.set_valign(gtk::Align::Fill);
    cell.set_overflow(gtk::Overflow::Hidden);
    child.set_hexpand(true);
    child.set_halign(gtk::Align::Fill);
    cell.append(&child);
    cell.upcast()
}
pub(in crate::ui) fn album_detail_fixed_cell_height(
    width: i32,
    height: i32,
    child: gtk::Widget,
) -> gtk::Widget {
    let width = width.max(1);
    let height = height.max(1);
    let clip = gtk::ScrolledWindow::new();
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_overflow(gtk::Overflow::Hidden);
    clip.set_min_content_width(width);
    clip.set_max_content_width(width);
    clip.set_propagate_natural_width(false);
    clip.set_propagate_natural_height(false);
    clip.set_width_request(width);
    clip.set_size_request(width, height);
    clip.set_height_request(height);
    clip.set_min_content_height(height);
    clip.set_max_content_height(height);
    clip.set_hexpand(false);
    clip.set_halign(gtk::Align::Fill);
    clip.set_child(Some(&child));
    clip.upcast()
}
pub(in crate::ui) fn album_detail_track_label(
    text: &str,
    _field: LibraryField,
    width: i32,
    ellipsize: bool,
) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Fill);
    label.set_valign(gtk::Align::Center);
    label.set_wrap(false);
    label.set_lines(1);
    label.set_single_line_mode(true);
    label.set_width_request(width);
    label.set_size_request(width, -1);
    label.set_width_chars(1);
    label.set_max_width_chars(1);
    label.set_hexpand(false);
    if ellipsize {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    label
}
pub(in crate::ui) fn album_detail_track_trailing_inset(
    field_widths: &[(LibraryField, i32)],
) -> i32 {
    let _ = field_widths;
    0
}
pub(in crate::ui) fn album_detail_track_field_widths(
    key: LibraryListKey,
    fields: &[LibraryField],
    available_width: i32,
) -> Vec<(LibraryField, i32)> {
    let gap_count = fields.len().saturating_sub(1).min(i32::MAX as usize) as i32;
    let gap_total = ALBUM_DETAIL_TRACK_COLUMN_GAP.saturating_mul(gap_count);
    let available_width = available_width
        .saturating_sub(gap_total)
        .max(fields.len() as i32);
    fields
        .iter()
        .copied()
        .zip(album_detail_text_column_widths(
            key,
            fields,
            available_width,
        ))
        .collect()
}
fn album_detail_text_column_widths(
    key: LibraryListKey,
    fields: &[LibraryField],
    available_width: i32,
) -> Vec<i32> {
    if fields.is_empty() {
        return Vec::new();
    }

    let base_widths = fields
        .iter()
        .map(|field| album_detail_track_column_width(key, *field))
        .collect::<Vec<_>>();
    let fixed_total = fields
        .iter()
        .zip(base_widths.iter())
        .filter_map(|(field, width)| (*field != LibraryField::Title).then_some(*width))
        .sum::<i32>();
    let title_count = fields
        .iter()
        .filter(|field| **field == LibraryField::Title)
        .count()
        .min(i32::MAX as usize) as i32;
    if title_count == 0 {
        return fitted_column_widths(&base_widths, available_width);
    }
    if fixed_total.saturating_add(title_count) > available_width {
        return fitted_column_widths(&base_widths, available_width);
    }

    let remaining_for_title = available_width.saturating_sub(fixed_total).max(title_count);
    fields
        .iter()
        .zip(base_widths.iter())
        .map(|(field, width)| {
            if *field == LibraryField::Title {
                remaining_for_title / title_count
            } else {
                *width
            }
        })
        .collect()
}
pub(in crate::ui) fn album_detail_track_column_width(
    key: LibraryListKey,
    field: LibraryField,
) -> i32 {
    match field {
        LibraryField::RowIndex => 40,
        LibraryField::TrackNumber => 52,
        LibraryField::DiscNumber => 44,
        LibraryField::Duration => 48,
        LibraryField::Year => 52,
        LibraryField::PlayCount => play_count_column_width().min(56),
        LibraryField::UserRating | LibraryField::SongCount | LibraryField::AlbumCount => 64,
        LibraryField::Favorite => 40,
        LibraryField::Image => 56,
        LibraryField::Title | LibraryField::TitleMerged => track_column_width(key, field),
        LibraryField::Album
        | LibraryField::Artist
        | LibraryField::AlbumArtist
        | LibraryField::Genre => 160,
        LibraryField::ReleaseDate | LibraryField::DateAdded | LibraryField::LastPlayed => 96,
    }
}
pub(in crate::ui) fn album_detail_track_cells_width(field_widths: &[(LibraryField, i32)]) -> i32 {
    let fields_width = field_widths.iter().map(|(_, width)| *width).sum::<i32>();
    let gap_count = field_widths.len().saturating_sub(1).min(i32::MAX as usize) as i32;
    let gap_total = ALBUM_DETAIL_TRACK_COLUMN_GAP.saturating_mul(gap_count);
    fields_width.saturating_add(gap_total).max(1)
}
pub(in crate::ui) fn album_detail_track_area_width(
    shell: &Shell,
    key: LibraryListKey,
    meta_width: i32,
    spacing: i32,
    fields: &[LibraryField],
) -> i32 {
    album_detail_track_area_width_for(key, route_content_width(shell), meta_width, spacing, fields)
}

fn album_detail_track_area_width_for(
    key: LibraryListKey,
    route_width: i32,
    meta_width: i32,
    spacing: i32,
    fields: &[LibraryField],
) -> i32 {
    album_detail_row_content_width(key, route_width)
        .saturating_sub(meta_width)
        .saturating_sub(ALBUM_DETAIL_SEPARATOR_WIDTH)
        .saturating_sub(spacing.saturating_mul(2))
        .max(album_detail_min_track_area_width(fields))
}
pub(in crate::ui) fn album_detail_track_text(
    track: &Track,
    index: usize,
    field: LibraryField,
) -> String {
    if field == LibraryField::RowIndex {
        (index + 1).to_string()
    } else {
        track_field(track, field)
    }
}
pub(in crate::ui) fn album_detail_lead_content_height(
    album: &Album,
    cover_size: i32,
    inline_count: usize,
) -> i32 {
    album_detail_meta_height(album, cover_size).max(album_detail_track_area_height(inline_count))
}
pub(in crate::ui) fn album_detail_item_total_height(row: &AlbumDetailItem, cover_size: i32) -> i32 {
    match row {
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album,
        } => {
            album_detail_lead_content_height(album, cover_size, inline_tracks.len())
                + 12
                + if *last_in_album { 16 } else { 0 }
        }
        AlbumDetailItem::Track { last_in_album, .. } => {
            ALBUM_DETAIL_TRACK_ROW_HEIGHT + if *last_in_album { 16 } else { 0 }
        }
    }
}
pub(in crate::ui) fn album_detail_meta_height(album: &Album, cover_size: i32) -> i32 {
    cover_size
        + album_detail_meta_label_count(album) as i32
            * (ALBUM_DETAIL_META_LABEL_HEIGHT + ALBUM_DETAIL_META_SPACING)
        + ALBUM_DETAIL_META_LABEL_HEIGHT
}
pub(in crate::ui) fn album_detail_meta_label_count(album: &Album) -> usize {
    3 + usize::from(!album.genres.is_empty())
}
pub(in crate::ui) fn album_detail_track_area_height(inline_count: usize) -> i32 {
    if inline_count == 0 {
        0
    } else {
        ALBUM_DETAIL_TRACK_HEADER_HEIGHT
            + inline_count.min(ALBUM_DETAIL_INLINE_TRACK_ROWS) as i32
                * ALBUM_DETAIL_TRACK_ROW_HEIGHT
    }
}
pub(in crate::ui) fn sort_album_detail_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|left, right| compare_track(left, right, LibraryField::TrackNumber));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AlbumDetailRowMetrics {
    cover_size: i32,
    meta_width: i32,
    spacing: i32,
}

fn album_detail_row_metrics(shell: &Shell, key: LibraryListKey) -> AlbumDetailRowMetrics {
    album_detail_row_metrics_for_width(key, route_content_width(shell))
}

fn album_detail_row_metrics_for_width(
    key: LibraryListKey,
    route_width: i32,
) -> AlbumDetailRowMetrics {
    let row_width = album_detail_row_content_width(key, route_width);
    let spacing = if row_width < ALBUM_DETAIL_NARROW_WIDTH {
        8
    } else {
        12
    };
    let target_cover = if row_width < ALBUM_DETAIL_NARROW_WIDTH {
        row_width * 38 / 100
    } else {
        row_width * 42 / 100
    };
    let cover_size = if row_width < ALBUM_DETAIL_MIN_COVER {
        row_width
    } else {
        target_cover.clamp(ALBUM_DETAIL_MIN_COVER, ALBUM_DETAIL_MAX_COVER)
    }
    .min(row_width)
    .max(1);

    AlbumDetailRowMetrics {
        cover_size,
        meta_width: cover_size,
        spacing,
    }
}

fn album_detail_row_content_width(key: LibraryListKey, route_width: i32) -> i32 {
    route_width
        .saturating_sub(album_detail_route_inset(key))
        .saturating_sub(ALBUM_DETAIL_ROW_HORIZONTAL_INSET)
        .max(1)
}

fn album_detail_route_inset(key: LibraryListKey) -> i32 {
    let _ = key;
    PRIMARY_ROUTE_MARGIN_START.saturating_add(DETAIL_ROUTE_SCROLL_GUTTER)
}

fn album_detail_min_track_area_width(fields: &[LibraryField]) -> i32 {
    let field_count = fields.len().min(i32::MAX as usize) as i32;
    let gap_count = fields.len().saturating_sub(1).min(i32::MAX as usize) as i32;
    field_count + ALBUM_DETAIL_TRACK_COLUMN_GAP.saturating_mul(gap_count)
}

#[cfg(test)]
mod album_detail_width_tests {
    use super::*;

    #[test]
    fn album_detail_meta_shrinks_with_route() {
        let metrics = album_detail_row_metrics_for_width(LibraryListKey::Albums, 260);

        assert_eq!(metrics.spacing, 8);
        assert!(metrics.meta_width < ALBUM_DETAIL_MAX_COVER);
        assert_eq!(metrics.cover_size, metrics.meta_width);
    }

    #[test]
    fn album_detail_track_width_stays_in_narrow_budget() {
        let fields = [
            LibraryField::RowIndex,
            LibraryField::Title,
            LibraryField::Duration,
        ];
        let metrics = album_detail_row_metrics_for_width(LibraryListKey::Albums, 260);
        let track_width = album_detail_track_area_width_for(
            LibraryListKey::Albums,
            260,
            metrics.meta_width,
            metrics.spacing,
            &fields,
        );

        let row_width = album_detail_rendered_width(
            metrics,
            album_detail_track_cells_width(&album_detail_track_field_widths(
                LibraryListKey::Albums,
                &fields,
                track_width,
            )),
        );
        assert!(row_width <= album_detail_row_content_width(LibraryListKey::Albums, 260));
    }

    #[test]
    fn album_detail_width_leaves_scroll_gutter() {
        let fields = [
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::Year,
            LibraryField::Duration,
        ];

        for route_width in [360, 562, 1145] {
            let metrics = album_detail_row_metrics_for_width(LibraryListKey::Albums, route_width);
            let track_width = album_detail_track_area_width_for(
                LibraryListKey::Albums,
                route_width,
                metrics.meta_width,
                metrics.spacing,
                &fields,
            );
            let row_width = album_detail_rendered_width(
                metrics,
                album_detail_track_cells_width(&album_detail_track_field_widths(
                    LibraryListKey::Albums,
                    &fields,
                    track_width,
                )),
            );
            let route_budget = route_width
                .saturating_sub(PRIMARY_ROUTE_MARGIN_START)
                .saturating_sub(DETAIL_ROUTE_SCROLL_GUTTER)
                .saturating_sub(ALBUM_DETAIL_ROW_HORIZONTAL_INSET)
                .max(1);
            assert!(row_width <= route_budget);
        }
    }

    fn album_detail_rendered_width(metrics: AlbumDetailRowMetrics, track_width: i32) -> i32 {
        metrics
            .meta_width
            .saturating_add(ALBUM_DETAIL_SEPARATOR_WIDTH)
            .saturating_add(metrics.spacing.saturating_mul(2))
            .saturating_add(track_width)
    }

    #[test]
    fn album_detail_repeated_double_clicks_activate() {
        assert!(!album_detail_play_click(1));
        assert!(album_detail_play_click(2));
        assert!(!album_detail_play_click(3));
        assert!(album_detail_play_click(4));
    }
}
