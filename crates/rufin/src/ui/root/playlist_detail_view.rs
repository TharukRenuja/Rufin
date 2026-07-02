use super::library::library_route_inset;
use super::*;

const PLAYLIST_DETAIL_COMPACT_WIDTH: i32 = 760;
const PLAYLIST_DETAIL_TINY_WIDTH: i32 = 520;
const PLAYLIST_DETAIL_COVER_ONLY_WIDTH: i32 = 420;
const PLAYLIST_DETAIL_TINY_COVER_SIZE: i32 = 150;
const PLAYLIST_DETAIL_WIDE_COVER_SIZE: i32 = 208;
const PLAYLIST_DETAIL_COMPACT_COVER_SIZE: i32 = 182;
const PLAYLIST_DETAIL_COVER_FETCH_SIZE: u32 = GRID_COVER_SIZE;

pub(in crate::ui) fn playlist_detail_compact_for_width(width: i32) -> bool {
    width < PLAYLIST_DETAIL_COMPACT_WIDTH
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
    if width < PLAYLIST_DETAIL_COVER_ONLY_WIDTH {
        width.clamp(96, PLAYLIST_DETAIL_TINY_COVER_SIZE)
    } else if width < PLAYLIST_DETAIL_TINY_WIDTH {
        PLAYLIST_DETAIL_TINY_COVER_SIZE
            + ((width - PLAYLIST_DETAIL_COVER_ONLY_WIDTH)
                * (PLAYLIST_DETAIL_COMPACT_COVER_SIZE - PLAYLIST_DETAIL_TINY_COVER_SIZE)
                / (PLAYLIST_DETAIL_TINY_WIDTH - PLAYLIST_DETAIL_COVER_ONLY_WIDTH))
    } else if playlist_detail_compact_for_width(width) {
        PLAYLIST_DETAIL_COMPACT_COVER_SIZE
    } else {
        PLAYLIST_DETAIL_WIDE_COVER_SIZE
    }
}

fn playlist_detail_from_loaded_tracks(
    playlist: Playlist,
    tracks: &[Track],
    cached_track_count: usize,
    tracks_by_id: &HashMap<TrackId, usize>,
    entry_keys: Vec<(String, TrackId)>,
) -> Option<source::PlaylistDetail> {
    if cached_track_count > tracks.len() {
        return None;
    }
    let mut detail_tracks = Vec::with_capacity(entry_keys.len());
    let mut entries = Vec::with_capacity(entry_keys.len());
    for (entry_id, track_id) in entry_keys {
        let track = tracks.get(*tracks_by_id.get(&track_id)?)?;
        if track.id != track_id {
            return None;
        }
        let track = track.clone();
        detail_tracks.push(track.clone());
        entries.push(PlaylistEntry { entry_id, track });
    }
    Some(source::PlaylistDetail {
        playlist,
        tracks: detail_tracks,
        entries,
    })
}

impl Shell {
    fn playlist_detail_kind_row(
        self: &Rc<Self>,
        genres: &[String],
        radio_playlist: Option<Playlist>,
    ) -> gtk::Box {
        let kind = gtk::Label::new(Some(&tr("Playlist")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        kind.set_halign(gtk::Align::Start);
        kind.set_valign(gtk::Align::Center);
        kind.set_margin_end(6);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.add_css_class("album-detail-kind-row");
        row.add_css_class("album-detail-genre-row");
        row.set_valign(gtk::Align::Center);
        row.set_halign(gtk::Align::Start);
        row.append(&kind);
        if let Some(playlist) = radio_playlist {
            let radio = detail_radio_button();
            let controller = self.controller.clone();
            radio.connect_clicked(move |_| {
                controller.play_playlist_radio(playlist.clone());
            });
            row.append(&radio);
        }

        for genre_name in genres {
            let button = detail_genre_pill_button(genre_name);
            if let Some(genre_id) = self.playlist_detail_genre_id(genre_name) {
                let shell = Rc::clone(self);
                button
                    .connect_clicked(move |_| shell.navigate(Route::GenreDetail(genre_id.clone())));
            } else {
                button.set_sensitive(false);
            }
            row.append(&button);
        }

        row
    }

    fn playlist_detail_genre_id(&self, name: &str) -> Option<domain::GenreId> {
        let library = self.state.library.borrow();
        if let Some(genre) = library
            .genres
            .iter()
            .find(|genre| genre.name.eq_ignore_ascii_case(name))
        {
            return Some(genre.id.clone());
        }
        drop(library);

        self.controller
            .cached_genres_page_matching(name, 0, 8)
            .ok()
            .into_iter()
            .flat_map(|page| page.items)
            .find(|genre| genre.name.eq_ignore_ascii_case(name))
            .map(|genre| genre.id)
    }

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
        let mut smart_playlist = detail.smart_playlist.clone();
        smart_playlist.image_refs = cover_refs;
        let artwork = crate::cover_art_policy::selected_smart_playlist_artwork(&smart_playlist);
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let compact = playlist_detail_compact_for_width(content_width);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        let track_selection: TrackTableSelectionHandle = Rc::new(RefCell::new(None));

        let cover = self.cover_group_tile_for_artwork(
            &artwork,
            seed,
            cover_size,
            playlist_detail_cover_fetch_size(),
        );
        cover.add_css_class("playlist-detail-cover");
        let title = detail_title_label(&smart_playlist_display_name(&detail.smart_playlist));
        let kind_row = self.playlist_detail_kind_row(&[], None);
        let summary = playlist_detail_summary(
            detail.smart_playlist.track_count,
            detail.smart_playlist.duration_seconds,
        );
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);
        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.controller.clone();
        let detail_for_play = detail.clone();
        play.connect_clicked(move |_| {
            controller.play_smart_playlist_detail(detail_for_play.clone());
        });
        actions.append(&play);
        let edit = detail_action_button(EDIT_ICON, "Edit");
        let shell = Rc::clone(self);
        let playlist_for_edit = detail.smart_playlist.clone();
        edit.connect_clicked(move |_| shell.edit_smart_playlist_dialog(playlist_for_edit.clone()));
        actions.append(&edit);
        let delete = detail_delete_button("Delete");
        let controller = self.controller.clone();
        let delete_id = detail.smart_playlist.id.clone();
        delete.connect_clicked(move |_| controller.delete_smart_playlist(delete_id.clone()));
        actions.append(&delete);
        let showcase = playlist_detail_showcase(
            self,
            PlaylistDetailShowcase {
                seed,
                content_width,
                compact,
                cover,
                kind_row: kind_row.upcast(),
                title: title.upcast(),
                summary: summary.upcast(),
                actions: actions.upcast(),
            },
        );
        wrapper.append(&library_route_inset(showcase));

        if detail.tracks.is_empty() {
            let empty = self.placeholder_view("Tracks", "No tracks match this smart playlist.");
            wrapper.append(&library_route_inset(empty));
        } else {
            wrapper.append(&self.library_tracks_scrolling_panel_with_selection(
                detail.tracks,
                LibraryListKey::SmartPlaylistTracks,
                "smart-playlist-detail",
                Some(PlaySourceDescriptor::SmartPlaylist {
                    smart_playlist_id: detail.smart_playlist.id.clone(),
                    definition_fingerprint: smart_playlist_definition_fingerprint(
                        &detail.smart_playlist.definition,
                    ),
                    selected_music_folder_id: selected_music_folder_id(self),
                }),
                Some(track_selection),
            ));
        }
        wrapper.upcast()
    }

    pub(in crate::ui) fn playlist_detail_view(
        self: &Rc<Self>,
        playlist_id: PlaylistId,
    ) -> gtk::Widget {
        let settings = self.state.settings.borrow().clone();
        let server = self.state.library.borrow().server.clone();
        let detail = server
            .as_ref()
            .and_then(|_| self.playlist_detail_from_loaded_tracks(&playlist_id))
            .or_else(|| {
                server.as_ref().and_then(|server| {
                    self.controller
                        .cached_playlist_detail_for_server(&playlist_id, server, &settings)
                        .ok()
                        .flatten()
                })
            })
            .or_else(|| {
                (server.is_none())
                    .then(|| {
                        self.controller
                            .cached_playlist_detail(&playlist_id)
                            .ok()
                            .flatten()
                    })
                    .flatten()
            })
            .or_else(|| {
                let library = self.state.library.borrow();
                let playlist = library
                    .playlists
                    .iter()
                    .find(|playlist| playlist.id.as_str() == playlist_id.as_str())
                    .cloned()?;
                Some(source::PlaylistDetail {
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
        let mut playlist = detail.playlist.clone();
        playlist.image_refs = cover_refs;
        let artwork = crate::cover_art_policy::selected_playlist_artwork(&playlist, &settings);
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let compact = playlist_detail_compact_for_width(content_width);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        let entry_selection: PlaylistEntrySelectionHandle = Rc::new(RefCell::new(None));

        let cover = self.cover_group_tile_for_artwork(
            &artwork,
            seed,
            cover_size,
            playlist_detail_cover_fetch_size(),
        );
        cover.add_css_class("playlist-detail-cover");
        let title = detail_title_label(&detail.playlist.name);
        let kind_row = self
            .playlist_detail_kind_row(&detail.playlist.top_genres, Some(detail.playlist.clone()));
        let summary = playlist_detail_summary(
            detail.playlist.track_count,
            detail.playlist.duration_seconds,
        );
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);
        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.controller.clone();
        let shell = Rc::clone(self);
        let playlist_id_for_play = detail.playlist.id.clone();
        let entry_for_play = detail.entries.first().cloned();
        let entries_for_selection = Rc::new(detail.entries.clone());
        let play_selection = Rc::clone(&entry_selection);
        play.connect_clicked(move |_| {
            if let Some(entry) = entry_for_play.clone() {
                let entries_for_selection = Rc::clone(&entries_for_selection);
                let play_selection = Rc::clone(&play_selection);
                shell.arm_playlist_entry_selection(Rc::new(move |queue| {
                    let Some(current_index) = queue.current_index else {
                        return;
                    };
                    let Some(queue_entry) = queue.entries.get(current_index) else {
                        return;
                    };
                    let Some(entry) = entries_for_selection.get(current_index) else {
                        return;
                    };
                    if entry.track.id != queue_entry.track_id {
                        return;
                    }
                    if let Some(select_entry) = play_selection.borrow().as_ref() {
                        select_entry(&entry.entry_id);
                    }
                }));
                controller.play_playlist_entry(
                    playlist_id_for_play.clone(),
                    entry,
                    0,
                    None,
                    (PlaylistEntrySortDescriptor::Position, false),
                    true,
                );
            }
        });
        actions.append(&play);
        let rename = detail_action_button(EDIT_ICON, "Rename");
        let shell = Rc::clone(self);
        let playlist_id_for_rename = detail.playlist.id.clone();
        let current_name = detail.playlist.name.clone();
        rename.connect_clicked(move |_| {
            shell.rename_playlist_dialog(playlist_id_for_rename.clone(), current_name.clone())
        });
        actions.append(&rename);
        let add_current = detail_action_button(ADD_ICON, "Add current");
        let current_track = self
            .state
            .player
            .borrow()
            .current
            .as_ref()
            .and_then(|entry| {
                let track_id = entry.track_id.clone();
                let index = self.state.track_index.borrow().get(&track_id).copied()?;
                let library = self.state.library.borrow();
                let track = library.tracks.get(index)?;
                (track.id == track_id).then(|| track.clone())
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
        let delete = detail_delete_button("Delete");
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
            present_light_dismiss_dialog(&dialog, &window);
        });
        actions.append(&delete);
        let showcase = playlist_detail_showcase(
            self,
            PlaylistDetailShowcase {
                seed,
                content_width,
                compact,
                cover,
                kind_row: kind_row.upcast(),
                title: title.upcast(),
                summary: summary.upcast(),
                actions: actions.upcast(),
            },
        );
        wrapper.append(&library_route_inset(showcase));

        if detail.entries.is_empty() {
            let placeholder =
                self.placeholder_view("Tracks", "No cached tracks are linked here yet.");
            wrapper.append(&library_route_inset(placeholder));
        } else {
            let entries = self.playlist_entries_view(&detail, Some(entry_selection));
            wrapper.append(&entries);
        }
        wrapper.upcast()
    }

    fn playlist_detail_from_loaded_tracks(
        self: &Rc<Self>,
        playlist_id: &PlaylistId,
    ) -> Option<source::PlaylistDetail> {
        let library = self.state.library.borrow();
        let playlist = library
            .playlists
            .iter()
            .find(|playlist| playlist.id == *playlist_id)
            .cloned()?;
        let entry_keys = library.playlist_entry_keys.get(playlist_id).cloned()?;
        let track_index = self.state.track_index.borrow();
        playlist_detail_from_loaded_tracks(
            playlist,
            &library.tracks,
            library.cached_track_count,
            &track_index,
            entry_keys,
        )
    }

    pub(in crate::ui) fn playlist_entries_view(
        self: &Rc<Self>,
        detail: &source::PlaylistDetail,
        selection_handle: Option<PlaylistEntrySelectionHandle>,
    ) -> gtk::Widget {
        let entries = Rc::new(detail.entries.clone());
        let state = Rc::new(RefCell::new(PlaylistEntryListState::default()));
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);

        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
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

        let direction = gtk::Button::from_icon_name(sort_order_icon(state.borrow().descending));
        direction.add_css_class("flat");
        direction.set_tooltip_text(Some(&tr("Change sort order")));
        toolbar.append(&sort_dropdown);
        toolbar.append(&direction);
        wrapper.append(&library_route_inset(toolbar.upcast()));

        let (table, model) = playlist_entries_table_panel(
            self,
            Rc::clone(&entries),
            Rc::clone(&state),
            detail.playlist.id.clone(),
            PRIMARY_ROUTE_HORIZONTAL_INSET,
            selection_handle,
        );
        rebuild_playlist_entries_model(&model, &entries, &state.borrow());
        self.refresh_current_route_now_playing_selections();

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            search.connect_search_changed(move |entry| {
                state.borrow_mut().query = entry.text().trim().to_string();
                rebuild_playlist_entries_model(&model, &entries, &state.borrow());
                shell.refresh_current_route_now_playing_selections();
            });
        }
        {
            let shell = Rc::clone(self);
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
                shell.refresh_current_route_now_playing_selections();
            });
        }
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            direction.connect_clicked(move |direction| {
                let descending = {
                    let mut state = state.borrow_mut();
                    state.descending = !state.descending;
                    state.descending
                };
                direction.set_icon_name(sort_order_icon(descending));
                rebuild_playlist_entries_model(&model, &entries, &state.borrow());
                shell.refresh_current_route_now_playing_selections();
            });
        }
        wrapper.append(&table);
        wrapper.upcast()
    }
}

fn playlist_detail_summary(track_count: u32, duration_seconds: u32) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_halign(gtk::Align::Start);
    row.append(&playlist_detail_summary_item(
        "rufin-route-tracks-symbolic",
        &track_count_text(track_count.into()),
    ));
    row.append(&playlist_detail_summary_item(
        "appointment-soon-symbolic",
        &format_duration_units(duration_seconds),
    ));
    row
}

fn playlist_detail_summary_item(icon_name: &str, text: &str) -> gtk::Box {
    let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("muted");
    icon.set_pixel_size(14);
    item.append(&icon);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    item.append(&label);
    item
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_detail_from_loaded_tracks_preserves_entry_order() {
        let first = test_track(1, &["Rock"]);
        let second = test_track(2, &["Folk"]);
        let playlist = test_playlist(1);
        let detail = playlist_detail_from_loaded_tracks(
            playlist,
            &[first.clone(), second.clone()],
            2,
            &track_index_for(&[first.clone(), second.clone()]),
            vec![
                ("entry-two".to_string(), second.id.clone()),
                ("entry-one".to_string(), first.id.clone()),
            ],
        )
        .expect("playlist detail");

        assert_eq!(
            detail
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<Vec<_>>(),
            vec!["entry-two", "entry-one"]
        );
        assert_eq!(detail.tracks, vec![second, first]);
    }

    #[test]
    fn playlist_detail_from_loaded_tracks_rejects_partial_snapshot() {
        let track = test_track(1, &["Rock"]);

        assert!(
            playlist_detail_from_loaded_tracks(
                test_playlist(1),
                std::slice::from_ref(&track),
                2,
                &track_index_for(std::slice::from_ref(&track)),
                vec![("entry-one".to_string(), track.id.clone())],
            )
            .is_none()
        );
    }

    #[test]
    fn playlist_detail_from_loaded_tracks_rejects_stale_index() {
        let first = test_track(1, &["Rock"]);
        let second = test_track(2, &["Folk"]);
        let playlist = test_playlist(1);

        assert!(
            playlist_detail_from_loaded_tracks(
                playlist,
                &[first.clone(), second.clone()],
                2,
                &track_index_for(&[second.clone(), first.clone()]),
                vec![("entry-one".to_string(), first.id.clone())],
            )
            .is_none()
        );
    }

    #[test]
    fn playlist_detail_duration_uses_units() {
        assert_eq!(format_duration_units(57), "57s");
        assert_eq!(format_duration_units(4_497), "1h 14m 57s");
    }

    fn test_track(index: usize, genres: &[&str]) -> Track {
        Track {
            id: TrackId::fake(index),
            album_id: AlbumId::fake(1),
            title: format!("Track {index}"),
            artist: "Artist".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2024,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: index as u16,
            image_ref: None,
            genres: genres.iter().map(|genre| genre.to_string()).collect(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
        }
    }

    fn test_playlist(index: usize) -> Playlist {
        Playlist {
            id: PlaylistId::fake(index),
            name: format!("Playlist {index}"),
            track_count: 0,
            duration_seconds: 0,
            top_genres: Vec::new(),
            image_refs: Vec::new(),
            image_ref: None,
        }
    }
}
