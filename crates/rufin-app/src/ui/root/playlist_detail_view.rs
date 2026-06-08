use super::*;

const PLAYLIST_DETAIL_WIDE_ROUTE_MARGIN: i32 = 24;
const PLAYLIST_DETAIL_COMPACT_ROUTE_MARGIN: i32 = 16;
const PLAYLIST_DETAIL_COMPACT_WIDTH: i32 = 760;
const PLAYLIST_DETAIL_WIDE_COVER_SIZE: i32 = 208;
const PLAYLIST_DETAIL_COMPACT_COVER_SIZE: i32 = 182;
const PLAYLIST_DETAIL_COVER_FETCH_SIZE: u32 = GRID_COVER_SIZE;

pub(in crate::ui) fn playlist_detail_compact_for_width(width: i32) -> bool {
    width < PLAYLIST_DETAIL_COMPACT_WIDTH
}

pub(in crate::ui) fn playlist_route_margin(width: i32) -> i32 {
    if playlist_detail_compact_for_width(width) {
        PLAYLIST_DETAIL_COMPACT_ROUTE_MARGIN
    } else {
        PLAYLIST_DETAIL_WIDE_ROUTE_MARGIN
    }
}
pub(in crate::ui) fn playlist_header_orientation(_width: i32) -> gtk::Orientation {
    gtk::Orientation::Horizontal
}
pub(in crate::ui) fn playlist_toolbar_orientation(_width: i32) -> gtk::Orientation {
    gtk::Orientation::Horizontal
}
pub(in crate::ui) fn playlist_sort_width(width: i32) -> i32 {
    if playlist_detail_compact_for_width(width) {
        (width / 3).clamp(112, 150)
    } else {
        170
    }
}
pub(in crate::ui) fn playlist_detail_cover_fetch_size() -> u32 {
    PLAYLIST_DETAIL_COVER_FETCH_SIZE
}
pub(in crate::ui) fn playlist_cover_size(width: i32) -> i32 {
    if playlist_detail_compact_for_width(width) {
        PLAYLIST_DETAIL_COMPACT_COVER_SIZE
    } else {
        PLAYLIST_DETAIL_WIDE_COVER_SIZE
    }
}

fn playlist_detail_action_button(icon: &str, label: &str, primary: bool) -> gtk::Button {
    let button = icon_button(icon, label);
    button.add_css_class("playlist-detail-action-button");
    if primary {
        button.add_css_class("playlist-detail-play-button");
    }
    button
}

impl Shell {
    pub(in crate::ui) fn smart_playlist_detail_view(
        self: &Rc<Self>,
        smart_playlist_id: SmartPlaylistId,
    ) -> gtk::Widget {
        let detail = self
            .controller
            .cached_smart_playlist_detail(&smart_playlist_id)
            .ok()
            .flatten();
        let Some(detail) = detail else {
            return self.placeholder_view(
                "Smart Playlist",
                "The selected smart playlist was not found.",
            );
        };
        let seed = stable_seed(detail.smart_playlist.id.as_str());
        let cover_refs = if detail.smart_playlist.image_refs.is_empty() {
            track_cover_refs_for_items(&detail.tracks)
        } else {
            detail.smart_playlist.image_refs.clone()
        };
        let summary = format!(
            "{} {} • {}",
            detail.smart_playlist.track_count,
            tr("tracks"),
            format_duration(detail.smart_playlist.duration_seconds)
        );
        let content_width = route_content_width(self);
        let compact = playlist_detail_compact_for_width(content_width);
        let route_margin = playlist_route_margin(content_width);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);

        let header = gtk::Box::new(
            playlist_header_orientation(content_width),
            if compact { 20 } else { 28 },
        );
        header.add_css_class("playlist-detail-showcase");
        add_album_seed_gradient_class(&header, seed);
        header.set_hexpand(true);
        header.set_halign(gtk::Align::Fill);
        header.set_width_request(1);
        header.set_margin_start(route_margin);
        header.set_margin_end(route_margin);
        let cover = self.cover_group_tile_for(
            cover_refs,
            detail.smart_playlist.image_ref.as_ref(),
            seed,
            cover_size,
            playlist_detail_cover_fetch_size(),
        );
        cover.add_css_class("playlist-detail-cover");
        header.append(&cover);
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        metadata.set_hexpand(true);
        metadata.set_width_request(1);
        let title = gtk::Label::new(Some(&detail.smart_playlist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        let summary = gtk::Label::new(Some(&summary));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("playlist-detail-actions");
        let play = playlist_detail_action_button("media-playback-start-symbolic", "Play", true);
        let controller = self.controller.clone();
        let source_key =
            smart_playlist_play_source_key(&detail.smart_playlist, selected_music_folder_id(self));
        let tracks = detail.tracks.clone();
        play.connect_clicked(move |_| {
            if let Some(activation) =
                loaded_tracks_window_play_activation(source_key.clone(), tracks.len(), 0, |index| {
                    tracks.get(index).cloned()
                })
            {
                controller.play_activation(activation);
            }
        });
        actions.append(&play);
        let edit = playlist_detail_action_button("document-edit-symbolic", "Edit", false);
        let shell = Rc::clone(self);
        let playlist_for_edit = detail.smart_playlist.clone();
        edit.connect_clicked(move |_| shell.edit_smart_playlist_dialog(playlist_for_edit.clone()));
        actions.append(&edit);
        let delete = playlist_detail_action_button("user-trash-symbolic", "Delete", false);
        let controller = self.controller.clone();
        let delete_id = detail.smart_playlist.id.clone();
        delete.connect_clicked(move |_| controller.delete_smart_playlist(delete_id.clone()));
        actions.append(&delete);
        metadata.append(&title);
        metadata.append(&summary);
        metadata.append(&actions);
        header.append(&metadata);
        wrapper.append(&header);

        if detail.tracks.is_empty() {
            let empty = self.placeholder_view("Tracks", "No tracks match this smart playlist.");
            empty.set_margin_start(route_margin);
            empty.set_margin_end(route_margin);
            wrapper.append(&empty);
        } else {
            wrapper.append(&self.library_tracks_scrolling_panel(
                detail.tracks,
                LibraryListKey::SmartPlaylistTracks,
                "smart-playlist-detail",
                route_margin,
                Some(PlaySourceDescriptor::SmartPlaylist {
                    smart_playlist_id: detail.smart_playlist.id.clone(),
                    definition_fingerprint: smart_playlist_definition_fingerprint(
                        &detail.smart_playlist.definition,
                    ),
                    selected_music_folder_id: selected_music_folder_id(self),
                }),
            ));
        }
        wrapper.upcast()
    }

    pub(in crate::ui) fn playlist_detail_view(
        self: &Rc<Self>,
        playlist_id: PlaylistId,
    ) -> gtk::Widget {
        let detail = self
            .controller
            .cached_playlist_detail(&playlist_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let playlist = library
                    .playlists
                    .iter()
                    .find(|playlist| playlist.id.as_str() == playlist_id.as_str())
                    .cloned()?;
                Some(rufin_provider::PlaylistDetail {
                    playlist,
                    tracks: Vec::new(),
                    entries: Vec::new(),
                })
            });
        let Some(detail) = detail else {
            return self
                .placeholder_view("Playlist", "The selected cached playlist was not found.");
        };
        let seed = stable_seed(detail.playlist.id.as_str());
        let cover_refs = if detail.playlist.image_refs.is_empty() {
            track_cover_refs_for_items(&detail.tracks)
        } else {
            detail.playlist.image_refs.clone()
        };
        let summary = format!(
            "{} {} • {}",
            detail.playlist.track_count,
            tr("tracks"),
            format_duration(detail.playlist.duration_seconds)
        );
        let content_width = route_content_width(self);
        let compact = playlist_detail_compact_for_width(content_width);
        let route_margin = playlist_route_margin(content_width);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(route_margin);
        wrapper.set_margin_end(route_margin);

        let header = gtk::Box::new(
            playlist_header_orientation(content_width),
            if compact { 20 } else { 28 },
        );
        header.add_css_class("playlist-detail-showcase");
        add_album_seed_gradient_class(&header, seed);
        header.set_hexpand(true);
        header.set_halign(gtk::Align::Fill);
        header.set_width_request(1);
        let cover = self.cover_group_tile_for(
            cover_refs,
            detail.playlist.image_ref.as_ref(),
            seed,
            cover_size,
            playlist_detail_cover_fetch_size(),
        );
        cover.add_css_class("playlist-detail-cover");
        header.append(&cover);
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        metadata.set_hexpand(true);
        metadata.set_width_request(1);
        let title = gtk::Label::new(Some(&detail.playlist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        let summary = gtk::Label::new(Some(&summary));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("playlist-detail-actions");
        let play = playlist_detail_action_button("media-playback-start-symbolic", "Play", true);
        let controller = self.controller.clone();
        let playlist_id_for_play = detail.playlist.id.clone();
        let entries_for_play = detail.entries.clone();
        play.connect_clicked(move |_| {
            if let Some(activation) = playlist_play_activation(
                playlist_id_for_play.clone(),
                entries_for_play.clone(),
                0,
                &PlaylistEntryListState::default(),
            ) {
                controller.play_activation(activation);
            }
        });
        actions.append(&play);
        let rename = playlist_detail_action_button("document-edit-symbolic", "Rename", false);
        let shell = Rc::clone(self);
        let playlist_id_for_rename = detail.playlist.id.clone();
        let current_name = detail.playlist.name.clone();
        rename.connect_clicked(move |_| {
            shell.rename_playlist_dialog(playlist_id_for_rename.clone(), current_name.clone())
        });
        actions.append(&rename);
        let add_current = playlist_detail_action_button("list-add-symbolic", "Add current", false);
        let current_track = self
            .state
            .player
            .borrow()
            .current
            .as_ref()
            .and_then(|entry| {
                self.state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .find(|track| track.id == entry.track_id)
                    .cloned()
            });
        add_current.set_sensitive(current_track.is_some());
        let controller = self.controller.clone();
        let playlist_id_for_add = detail.playlist.id.clone();
        add_current.connect_clicked(move |_| {
            if let Some(track) = current_track.clone() {
                controller.add_tracks_to_playlist(playlist_id_for_add.clone(), vec![track]);
            }
        });
        actions.append(&add_current);
        let delete = playlist_detail_action_button("user-trash-symbolic", "Delete", false);
        let controller = self.controller.clone();
        let window = self.window.clone();
        let playlist_id_for_delete = detail.playlist.id.clone();
        let playlist_name = detail.playlist.name.clone();
        delete.connect_clicked(move |_| {
            let dialog = adw::AlertDialog::builder()
                .heading(tr("Delete Playlist"))
                .body(format!("Delete \"{playlist_name}\"?"))
                .build();
            dialog.add_response("cancel", &tr("Cancel"));
            dialog.add_response("delete", &tr("Delete"));
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            let controller = controller.clone();
            let playlist_id = playlist_id_for_delete.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "delete" {
                    controller.delete_playlist(playlist_id.clone());
                }
            });
            dialog.present(Some(&window));
        });
        actions.append(&delete);
        metadata.append(&title);
        metadata.append(&summary);
        metadata.append(&actions);
        header.append(&metadata);
        wrapper.append(&header);

        if detail.entries.is_empty() {
            wrapper
                .append(&self.placeholder_view("Tracks", "No cached tracks are linked here yet."));
        } else {
            wrapper.append(&self.playlist_entries_view(&detail));
        }
        wrapper.upcast()
    }
    pub(in crate::ui) fn playlist_entries_view(
        self: &Rc<Self>,
        detail: &rufin_provider::PlaylistDetail,
    ) -> gtk::Widget {
        let entries = Rc::new(detail.entries.clone());
        let state = Rc::new(RefCell::new(PlaylistEntryListState::default()));
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);

        let content_width = route_content_width(self);
        let toolbar = gtk::Box::new(playlist_toolbar_orientation(content_width), 8);
        toolbar.add_css_class("track-toolbar");
        toolbar.set_hexpand(true);
        toolbar.set_halign(gtk::Align::Fill);
        toolbar.set_width_request(1);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        search.set_width_request(1);
        toolbar.append(&search);
        self.install_type_to_search(&search);

        let sort_titles = PLAYLIST_ENTRY_SORTS
            .iter()
            .map(|sort| tr(sort.title()))
            .collect::<Vec<_>>();
        let sort_refs = sort_titles.iter().map(String::as_str).collect::<Vec<_>>();
        let sort_options = gtk::StringList::new(&sort_refs);
        let sort_dropdown = gtk::DropDown::new(Some(sort_options), None::<gtk::Expression>);
        sort_dropdown.set_hexpand(false);
        sort_dropdown.set_halign(gtk::Align::End);
        sort_dropdown.set_width_request(playlist_sort_width(content_width));

        let direction = gtk::Button::from_icon_name("view-sort-ascending-symbolic");
        direction.add_css_class("flat");
        direction.set_tooltip_text(Some(&tr("Change sort order")));
        toolbar.append(&sort_dropdown);
        toolbar.append(&direction);
        wrapper.append(&toolbar);

        let (table, model) = playlist_entries_table_panel(
            self,
            Rc::clone(&entries),
            Rc::clone(&state),
            detail.playlist.id.clone(),
        );
        rebuild_playlist_entries_model(&model, &entries, &state.borrow());

        {
            let model = model.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            search.connect_search_changed(move |entry| {
                state.borrow_mut().query = entry.text().trim().to_string();
                rebuild_playlist_entries_model(&model, &entries, &state.borrow());
            });
        }
        {
            let model = model.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            sort_dropdown.connect_selected_notify(move |dropdown| {
                let selected = PLAYLIST_ENTRY_SORTS
                    .get(dropdown.selected() as usize)
                    .copied()
                    .unwrap_or(PlaylistEntrySort::Order);
                state.borrow_mut().sort = selected;
                rebuild_playlist_entries_model(&model, &entries, &state.borrow());
            });
        }
        {
            let model = model.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            direction.connect_clicked(move |button| {
                let descending = {
                    let mut state = state.borrow_mut();
                    state.descending = !state.descending;
                    state.descending
                };
                button.set_icon_name(if descending {
                    "view-sort-descending-symbolic"
                } else {
                    "view-sort-ascending-symbolic"
                });
                rebuild_playlist_entries_model(&model, &entries, &state.borrow());
            });
        }
        wrapper.append(&table);
        wrapper.upcast()
    }
}
