use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

use ::library::play_context::{
    PlayContextDescriptor, PlaylistSort, smart_playlist_definition_fingerprint,
};
use ::library::{
    ActiveLibraryQuery, LibraryDelta, Playlist, PlaylistDetail, PlaylistEntry, PlaylistId,
    SmartPlaylistDetail, SmartPlaylistId,
};
use adw::prelude::*;
use artwork::ArtworkBinding;
use sources::SourcePlaylistOperation;

use crate::LibraryListKey;
use crate::format_duration_units;
use crate::localization::{bind_label_text_with, bind_search_placeholder, localized_label};
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use crate::shell::actions::{ADD_ICON, EDIT_ICON, PLAY_ICON};
use crate::shell::cover::GRID_COVER_SIZE;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::route::{MountedRoute, MountedRouteDeltaApplier};
use localization::{msgid, tr, track_count_text};
use playback::{PlaylistEntryPlayRequest, RadioPlayRequest, RadioSeed, SmartPlaylistPlayRequest};

use super::collection_routes::{
    MountedRefreshLoader, MountedRouteRefresh, smart_playlist_detail_affected,
};
use super::collections::{
    LibraryCollectionProjection, TrackTableSelectionHandle, library_route_inset,
};
use super::detail_showcase::{
    PlaylistDetailShowcase, detail_action_button, detail_action_row, detail_delete_button,
    detail_genre_pill_button, detail_primary_action_button, detail_radio_button,
    detail_title_label, playlist_detail_showcase,
};
use super::library_fields::smart_playlist_display_name;
use super::play_context::selected_music_folder_id;
use super::playlist_entries::{
    PlaylistEntryListState, PlaylistEntrySelectionHandle, playlist_entries_collection_projection,
    playlist_operation_supported, rebuild_playlist_entries_model,
};
use super::route::Route;
use super::route_layout::{
    PRIMARY_ROUTE_HORIZONTAL_INSET, PRIMARY_ROUTE_MARGIN_START, ROUTE_TOP_MARGIN,
    detail_route_inner_width,
};
use super::route_shell::LibraryToolbarProjection;

const PLAYLIST_DETAIL_COMPACT_WIDTH: i32 = 760;
const PLAYLIST_DETAIL_COVER_ONLY_WIDTH: i32 = 420;
const PLAYLIST_DETAIL_TINY_COVER_SIZE: i32 = 150;
const PLAYLIST_DETAIL_WIDE_COVER_SIZE: i32 = 208;
const PLAYLIST_DETAIL_COVER_FETCH_SIZE: u32 = GRID_COVER_SIZE;

pub(crate) struct PlaylistDetailRefresh {
    detail: Option<PlaylistDetail>,
    genre_ids: HashMap<String, ::library::GenreId>,
}

pub(crate) fn load_playlist_detail_refresh(
    query: &ActiveLibraryQuery,
    playlist_id: &PlaylistId,
) -> Result<PlaylistDetailRefresh, String> {
    let detail = query.playlist_detail(playlist_id)?;
    let genre_ids = detail
        .as_ref()
        .map(|detail| playlist_genre_ids(query, &detail.playlist.top_genres))
        .unwrap_or_default();
    Ok(PlaylistDetailRefresh { detail, genre_ids })
}

fn playlist_genre_ids(
    query: &ActiveLibraryQuery,
    genres: &[String],
) -> HashMap<String, ::library::GenreId> {
    query.genre_ids_by_name(genres).unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to resolve Playlist genre links");
        HashMap::new()
    })
}

#[derive(Clone)]
pub(crate) struct PlaylistEntryProjection {
    widget: gtk::Widget,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    replace_entries: Rc<dyn Fn(Vec<PlaylistEntry>)>,
    collection: LibraryCollectionProjection,
    toolbar: LibraryToolbarProjection,
    state: Rc<RefCell<PlaylistEntryListState>>,
    model: gtk::gio::ListStore,
    applied_settings: Rc<RefCell<crate::LibraryListSettings>>,
    refresh_selection: Rc<dyn Fn()>,
}

impl PlaylistEntryProjection {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub(crate) fn entries(&self) -> Rc<RefCell<Vec<PlaylistEntry>>> {
        Rc::clone(&self.entries)
    }

    pub(crate) fn replace(&self, entries: Vec<PlaylistEntry>) {
        (self.replace_entries)(entries);
    }

    pub(crate) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &crate::LibraryListSettings,
    ) {
        if key != LibraryListKey::PlaylistTracks {
            return;
        }
        let previous = self.applied_settings.borrow().clone();
        self.state.borrow_mut().apply_settings(settings);
        if previous.sort_key != settings.sort_key || previous.descending != settings.descending {
            rebuild_playlist_entries_model(
                &self.model,
                &self.entries.borrow(),
                &self.state.borrow(),
            );
            (self.refresh_selection)();
        }
        self.collection.apply_settings(settings);
        self.toolbar.apply(key, settings);
        *self.applied_settings.borrow_mut() = settings.clone();
    }
}

pub(crate) fn playlist_detail_compact_for_width(width: i32) -> bool {
    width < PLAYLIST_DETAIL_COMPACT_WIDTH
}

pub(crate) fn playlist_detail_cover_fetch_size() -> u32 {
    PLAYLIST_DETAIL_COVER_FETCH_SIZE
}
pub(crate) fn playlist_cover_size(width: i32) -> i32 {
    if width < PLAYLIST_DETAIL_COVER_ONLY_WIDTH {
        width.clamp(96, PLAYLIST_DETAIL_TINY_COVER_SIZE)
    } else if playlist_detail_compact_for_width(width) {
        PLAYLIST_DETAIL_TINY_COVER_SIZE
            + ((width - PLAYLIST_DETAIL_COVER_ONLY_WIDTH)
                * (PLAYLIST_DETAIL_WIDE_COVER_SIZE - PLAYLIST_DETAIL_TINY_COVER_SIZE)
                / (PLAYLIST_DETAIL_COMPACT_WIDTH - PLAYLIST_DETAIL_COVER_ONLY_WIDTH))
    } else {
        PLAYLIST_DETAIL_WIDE_COVER_SIZE
    }
}

impl Shell {
    fn playlist_detail_kind_row(
        self: &Rc<Self>,
        library_query: &ActiveLibraryQuery,
        genres: &[String],
        radio_playlist: Option<Playlist>,
    ) -> gtk::Box {
        let genre_ids = playlist_genre_ids(library_query, genres);
        self.playlist_detail_kind_row_with_ids(genres, radio_playlist, &genre_ids)
    }

    fn playlist_detail_kind_row_with_ids(
        self: &Rc<Self>,
        genres: &[String],
        radio_playlist: Option<Playlist>,
        genre_ids: &HashMap<String, ::library::GenreId>,
    ) -> gtk::Box {
        let kind = localized_label("Playlist");
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
        if let Some(playlist) = radio_playlist.filter(|_| {
            self.products
                .playback
                .radio
                .manual_radio_supported(sources::GeneratedTrackSeedKind::Playlist)
        }) {
            let radio = detail_radio_button();
            let controller = self.products.playback.radio.clone();
            radio.connect_clicked(move |_| {
                controller.play_radio(RadioPlayRequest::now(RadioSeed::Playlist(playlist.clone())));
            });
            row.append(&radio);
        }

        for genre_name in genres {
            let button = detail_genre_pill_button(genre_name);
            if let Some(genre_id) = genre_ids.get(&genre_name.to_lowercase()).cloned() {
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

    pub(crate) fn smart_playlist_detail_route_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        smart_playlist_id: SmartPlaylistId,
        detail: Option<SmartPlaylistDetail>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            return MountedRoute::static_widget(self.placeholder_view(
                msgid("Smart Playlist"),
                "The selected smart playlist was not found.",
            ));
        };
        let SmartPlaylistDetail {
            smart_playlist,
            tracks: initial_tracks,
        } = detail;
        let initial_tracks = Arc::new(initial_tracks);
        // Actions retain only lightweight playlist metadata and the queue anchor. The track
        // projection below owns the full track vector, so this route does not keep a second
        // SmartPlaylistDetail copy alive beside its GTK model.
        let anchor_track_id = Rc::new(RefCell::new(
            initial_tracks.first().map(|track| track.id.clone()),
        ));
        let seed = stable_seed(smart_playlist.id.as_str());
        let artwork = ArtworkBinding::smart_playlist_slots(&smart_playlist);
        let current_smart_playlist = Rc::new(RefCell::new(smart_playlist));
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        let track_selection: TrackTableSelectionHandle = Rc::new(RefCell::new(None));

        let cover = self.cover_group_projection_for_artwork(
            &artwork,
            seed,
            cover_size,
            playlist_detail_cover_fetch_size(),
        );
        cover.widget().add_css_class("playlist-detail-cover");
        let title = detail_title_label(&smart_playlist_display_name(
            &current_smart_playlist.borrow(),
        ));
        let kind_row = self.playlist_detail_kind_row(&library_query, &[], None);
        let summary = PlaylistDetailSummary::new(
            current_smart_playlist.borrow().track_count,
            current_smart_playlist.borrow().duration_seconds,
        );
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);
        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.products.playback.queue.clone();
        let play_smart_playlist = Rc::clone(&current_smart_playlist);
        let play_anchor_track_id = Rc::clone(&anchor_track_id);
        let play_music_folder_id = selected_music_folder_id(self);
        play.connect_clicked(move |_| {
            controller.play_smart_playlist(SmartPlaylistPlayRequest {
                playlist: play_smart_playlist.borrow().clone(),
                anchor_track_id: play_anchor_track_id.borrow().clone(),
                music_folder_id: play_music_folder_id.clone(),
            });
        });
        actions.append(&play);
        let edit = detail_action_button(EDIT_ICON, "Edit");
        let shell = Rc::clone(self);
        let edit_smart_playlist = Rc::clone(&current_smart_playlist);
        edit.connect_clicked(move |_| {
            shell.edit_smart_playlist_dialog(edit_smart_playlist.borrow().clone())
        });
        actions.append(&edit);
        let delete = detail_delete_button("Delete");
        let library = self.products.library.clone();
        let delete_smart_playlist = Rc::clone(&current_smart_playlist);
        delete.connect_clicked(move |_| {
            library.delete_smart_playlist(delete_smart_playlist.borrow().id.clone())
        });
        actions.append(&delete);
        let showcase = playlist_detail_showcase(
            self,
            PlaylistDetailShowcase {
                seed,
                initial_width: content_width,
                cover: cover.clone(),
                kind_row: kind_row.upcast(),
                title: title.clone().upcast(),
                summary: summary.widget(),
                actions: actions.upcast(),
            },
        );
        wrapper.append(&library_route_inset(showcase));

        let initial_descriptor = {
            let smart_playlist = current_smart_playlist.borrow();
            PlayContextDescriptor::SmartPlaylist {
                smart_playlist_id: smart_playlist.id.clone(),
                definition_fingerprint: smart_playlist_definition_fingerprint(
                    &smart_playlist.definition,
                ),
                music_folder_id: selected_music_folder_id(self),
            }
        };
        let (tracks_widget, tracks, tracks_toolbar) = self
            .scrolling_track_projection_with_selection(
                initial_tracks,
                LibraryListKey::SmartPlaylistTracks,
                "smart-playlist-detail",
                Some(initial_descriptor),
                Some(track_selection),
            );
        let tracks_stack = gtk::Stack::new();
        tracks_stack.set_hexpand(true);
        tracks_stack.set_vexpand(true);
        tracks_stack.add_named(&tracks_widget, Some("tracks"));
        tracks_stack.add_named(
            &library_route_inset(
                self.placeholder_view("Tracks", "No tracks match this smart playlist."),
            ),
            Some("empty"),
        );
        tracks_stack.set_visible_child_name(if tracks.source_is_empty() {
            "empty"
        } else {
            "tracks"
        });
        wrapper.append(&tracks_stack);

        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&wrapper, Some("detail"));
        route_stack.add_named(
            &self.placeholder_view(
                msgid("Smart Playlist"),
                "The selected smart playlist was not found.",
            ),
            Some("missing"),
        );
        route_stack.set_visible_child_name("detail");

        let apply_loaded: Rc<dyn Fn(Result<Option<SmartPlaylistDetail>, String>)> = {
            let shell = Rc::clone(self);
            let current_smart_playlist = Rc::clone(&current_smart_playlist);
            let anchor_track_id = Rc::clone(&anchor_track_id);
            let route_stack = route_stack.clone();
            let title = title.clone();
            let summary = summary.clone();
            let cover = cover.clone();
            let tracks_stack = tracks_stack.clone();
            let tracks = tracks.clone();
            Rc::new(move |result| {
                let next = match result {
                    Ok(next) => next,
                    Err(error) => {
                        tracing::warn!(%error, "failed to refresh mounted smart playlist detail");
                        return;
                    }
                };
                let Some(next) = next else {
                    route_stack.set_visible_child_name("missing");
                    return;
                };
                let SmartPlaylistDetail {
                    smart_playlist: next_smart_playlist,
                    tracks: next_tracks,
                } = next;
                let next_tracks = Arc::new(next_tracks);
                let next_anchor_track_id = next_tracks.first().map(|track| track.id.clone());
                title.set_text(&smart_playlist_display_name(&next_smart_playlist));
                summary.set(
                    next_smart_playlist.track_count,
                    next_smart_playlist.duration_seconds,
                );
                let artwork = ArtworkBinding::smart_playlist_slots(&next_smart_playlist);
                cover.replace(&shell, &artwork, seed);
                tracks.set_source_descriptor(PlayContextDescriptor::SmartPlaylist {
                    smart_playlist_id: next_smart_playlist.id.clone(),
                    definition_fingerprint: smart_playlist_definition_fingerprint(
                        &next_smart_playlist.definition,
                    ),
                    music_folder_id: selected_music_folder_id(&shell),
                });
                tracks.replace_shared(next_tracks);
                tracks_stack.set_visible_child_name(if tracks.source_is_empty() {
                    "empty"
                } else {
                    "tracks"
                });
                anchor_track_id.replace(next_anchor_track_id);
                current_smart_playlist.replace(next_smart_playlist);
                route_stack.set_visible_child_name("detail");
            })
        };

        let load_query = library_query.clone();
        let load_smart_playlist_id = smart_playlist_id.clone();
        let load: MountedRefreshLoader<Result<Option<SmartPlaylistDetail>, String>> =
            Arc::new(move || load_query.smart_playlist_detail(&load_smart_playlist_id));
        let refresh = MountedRouteRefresh::new(
            Rc::downgrade(&apply_loaded),
            load,
            "mounted smart playlist detail",
        );

        let affected_by = {
            let mounted_id = smart_playlist_id.clone();
            Rc::new(move |delta: &LibraryDelta| smart_playlist_detail_affected(delta, &mounted_id))
        };
        let apply_delta = {
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &LibraryDelta| {
                let _ = &apply_loaded;
                refresh.request();
            }) as MountedRouteDeltaApplier
        };
        let resume = {
            let shell = Rc::clone(self);
            Rc::new(move || {
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::SmartPlaylistTracks);
                tracks.apply_library_list_settings(LibraryListKey::SmartPlaylistTracks, &settings);
                tracks_toolbar.apply(LibraryListKey::SmartPlaylistTracks, &settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    pub(crate) fn playlist_detail_route_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        playlist_id: PlaylistId,
        loaded: Option<PlaylistDetailRefresh>,
    ) -> MountedRoute {
        let settings = self.settings.current.borrow().clone();
        let Some(PlaylistDetailRefresh {
            detail: Some(detail),
            genre_ids,
        }) = loaded
        else {
            return MountedRoute::static_widget(
                self.placeholder_view("Playlist", "The selected cached playlist was not found."),
            );
        };
        let current_playlist = Rc::new(RefCell::new(detail.playlist.clone()));
        let applied_playlist_artwork = Rc::new(Cell::new(settings.prefer_server_playlist_covers));
        let seed = stable_seed(detail.playlist.id.as_str());
        let artwork = ArtworkBinding::playlist_slots(
            &detail.playlist,
            settings.prefer_server_playlist_covers,
        );
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        let entry_selection: PlaylistEntrySelectionHandle = Rc::new(RefCell::new(None));

        let cover = self.cover_group_projection_for_artwork(
            &artwork,
            seed,
            cover_size,
            playlist_detail_cover_fetch_size(),
        );
        cover.widget().add_css_class("playlist-detail-cover");
        let title = detail_title_label(&detail.playlist.name);
        let kind_row = self.playlist_detail_kind_row_with_ids(
            &detail.playlist.top_genres,
            Some(detail.playlist.clone()),
            &genre_ids,
        );
        let kind_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        kind_slot.append(&kind_row);
        let summary = PlaylistDetailSummary::new(
            detail.playlist.track_count,
            detail.playlist.duration_seconds,
        );
        let can_rename =
            playlist_operation_supported(self, &detail.playlist, SourcePlaylistOperation::Rename);
        let can_delete =
            playlist_operation_supported(self, &detail.playlist, SourcePlaylistOperation::Delete);
        let can_add_tracks = playlist_operation_supported(
            self,
            &detail.playlist,
            SourcePlaylistOperation::AddTracks,
        );
        let can_remove_entries = playlist_operation_supported(
            self,
            &detail.playlist,
            SourcePlaylistOperation::RemoveEntries,
        );
        let can_reorder_entries = playlist_operation_supported(
            self,
            &detail.playlist,
            SourcePlaylistOperation::ReorderEntries,
        );
        let entry_projection = self.playlist_entries_projection(
            &detail,
            Some(Rc::clone(&entry_selection)),
            can_remove_entries,
            can_reorder_entries,
        );
        let current_entries = entry_projection.entries();
        let current_name = Rc::new(RefCell::new(detail.playlist.name.clone()));
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);
        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.products.playback.queue.clone();
        let shell = Rc::clone(self);
        let playlist_id_for_play = detail.playlist.id.clone();
        let entries_for_play = Rc::clone(&current_entries);
        let play_selection = Rc::clone(&entry_selection);
        play.connect_clicked(move |_| {
            if let Some(entry) = entries_for_play.borrow().first().cloned() {
                let entries_for_selection = Rc::clone(&entries_for_play);
                let play_selection = Rc::clone(&play_selection);
                shell.arm_playlist_entry_selection(Rc::new(move |queue| {
                    let Some(current_index) = queue.current_absolute_index else {
                        return;
                    };
                    let Some(queue_entry) = queue
                        .rows
                        .iter()
                        .find(|row| row.absolute_index == current_index)
                    else {
                        return;
                    };
                    let entries = entries_for_selection.borrow();
                    let Some(entry) = entries.get(current_index) else {
                        return;
                    };
                    if entry.track.id != queue_entry.entry.track.id {
                        return;
                    }
                    if let Some(select_entry) = play_selection.borrow().as_ref() {
                        select_entry(&entry.entry_id);
                    }
                }));
                controller.play_playlist_entry(PlaylistEntryPlayRequest {
                    playlist_id: playlist_id_for_play.clone(),
                    entry,
                    source_index: 0,
                    query: None,
                    sort: PlaylistSort::Position,
                    descending: false,
                    shuffled_start: true,
                });
            }
        });
        actions.append(&play);
        if can_rename {
            let rename = detail_action_button(EDIT_ICON, "Rename");
            let shell = Rc::clone(self);
            let playlist_id_for_rename = detail.playlist.id.clone();
            let current_name = Rc::clone(&current_name);
            rename.connect_clicked(move |_| {
                shell.rename_playlist_dialog(
                    playlist_id_for_rename.clone(),
                    current_name.borrow().clone(),
                )
            });
            actions.append(&rename);
        }
        if can_add_tracks {
            let add_current = detail_action_button(ADD_ICON, "Add current");
            let current_track = self
                .playback
                .player
                .borrow()
                .as_ref()
                .and_then(|player| player.transport.current.as_ref())
                .map(|entry| entry.track.clone());
            add_current.set_sensitive(current_track.is_some());
            let library = self.products.library.clone();
            let playlist_id_for_add = detail.playlist.id.clone();
            add_current.connect_clicked(move |_| {
                if let Some(track) = current_track.clone() {
                    library.add_tracks_to_playlist(playlist_id_for_add.clone(), vec![track]);
                }
            });
            actions.append(&add_current);
        }
        if can_delete {
            let delete = detail_delete_button("Delete");
            let library = self.products.library.clone();
            let window = self.chrome.window.clone();
            let playlist_id_for_delete = detail.playlist.id.clone();
            let playlist_name = Rc::clone(&current_name);
            delete.connect_clicked(move |_| {
                let playlist_name = playlist_name.borrow().clone();
                let dialog = adw::AlertDialog::builder()
                    .heading(tr("Delete Playlist"))
                    .body(format!("Delete \"{playlist_name}\"?"))
                    .build();
                dialog.add_response("cancel", &tr("Cancel"));
                dialog.add_response("delete", &tr("Delete"));
                dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                let library = library.clone();
                let playlist_id = playlist_id_for_delete.clone();
                dialog.connect_response(None, move |_, response| {
                    if response == "delete" {
                        library.delete_playlist(playlist_id.clone());
                    }
                });
                present_light_dismiss_dialog(&dialog, &window);
            });
            actions.append(&delete);
        }
        let showcase = playlist_detail_showcase(
            self,
            PlaylistDetailShowcase {
                seed,
                initial_width: content_width,
                cover: cover.clone(),
                kind_row: kind_slot.clone().upcast(),
                title: title.clone().upcast(),
                summary: summary.widget(),
                actions: actions.upcast(),
            },
        );
        wrapper.append(&library_route_inset(showcase));

        wrapper.append(&entry_projection.widget());

        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&wrapper, Some("content"));
        route_stack.add_named(
            &self.placeholder_view("Playlist", "The selected cached playlist was not found."),
            Some("missing"),
        );
        route_stack.set_visible_child_name("content");

        let apply_loaded: Rc<dyn Fn(Result<PlaylistDetailRefresh, String>)> = {
            let shell = Rc::clone(self);
            let route_stack = route_stack.clone();
            let entry_projection = entry_projection.clone();
            let title = title.clone();
            let summary = summary.clone();
            let current_name = Rc::clone(&current_name);
            let cover = cover.clone();
            let kind_slot = kind_slot.clone();
            let current_playlist = Rc::clone(&current_playlist);
            let applied_playlist_artwork = Rc::clone(&applied_playlist_artwork);
            Rc::new(move |result| {
                let PlaylistDetailRefresh { detail, genre_ids } = match result {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        tracing::warn!(%error, "failed to refresh Playlist detail projection");
                        return;
                    }
                };
                let Some(detail) = detail else {
                    route_stack.set_visible_child_name("missing");
                    return;
                };
                title.set_text(&detail.playlist.name);
                *current_name.borrow_mut() = detail.playlist.name.clone();
                summary.set(
                    detail.playlist.track_count,
                    detail.playlist.duration_seconds,
                );
                entry_projection.replace(detail.entries);

                let prefer_server_playlist_covers = shell
                    .settings
                    .current
                    .borrow()
                    .prefer_server_playlist_covers;
                let artwork =
                    ArtworkBinding::playlist_slots(&detail.playlist, prefer_server_playlist_covers);
                cover.replace(&shell, &artwork, seed);
                current_playlist.replace(detail.playlist.clone());
                applied_playlist_artwork.set(prefer_server_playlist_covers);

                while let Some(child) = kind_slot.first_child() {
                    kind_slot.remove(&child);
                }
                kind_slot.append(&shell.playlist_detail_kind_row_with_ids(
                    &detail.playlist.top_genres,
                    Some(detail.playlist.clone()),
                    &genre_ids,
                ));
                route_stack.set_visible_child_name("content");
            })
        };
        let load_query = library_query.clone();
        let load_playlist_id = playlist_id.clone();
        let load: MountedRefreshLoader<Result<PlaylistDetailRefresh, String>> =
            Arc::new(move || load_playlist_detail_refresh(&load_query, &load_playlist_id));
        let refresh = MountedRouteRefresh::new(
            Rc::downgrade(&apply_loaded),
            load,
            "mounted Playlist detail",
        );
        let affected_by = {
            let playlist_id = playlist_id.clone();
            Rc::new(move |delta: &::library::LibraryDelta| {
                delta.reset.is_some()
                    || delta.playlists.added.contains(&playlist_id)
                    || delta.playlists.deleted.contains(&playlist_id)
                    || delta.playlists.fields.contains(&playlist_id)
                    || delta.playlists.entries.contains(&playlist_id)
                    || delta.playlists.cover_refs.contains(&playlist_id)
                    || !delta.tracks.is_empty()
            })
        };
        let apply_delta = {
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &::library::LibraryDelta| {
                let _ = &apply_loaded;
                refresh.request();
            }) as MountedRouteDeltaApplier
        };
        let resume = {
            let shell = Rc::clone(self);
            let current_playlist = Rc::clone(&current_playlist);
            let applied_playlist_artwork = Rc::clone(&applied_playlist_artwork);
            let cover = cover.clone();
            Rc::new(move || {
                let prefer_server_playlist_covers = shell
                    .settings
                    .current
                    .borrow()
                    .prefer_server_playlist_covers;
                if applied_playlist_artwork.get() != prefer_server_playlist_covers {
                    let artwork = ArtworkBinding::playlist_slots(
                        &current_playlist.borrow(),
                        prefer_server_playlist_covers,
                    );
                    cover.replace(&shell, &artwork, seed);
                    applied_playlist_artwork.set(prefer_server_playlist_covers);
                }
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::PlaylistTracks);
                entry_projection
                    .apply_library_list_settings(LibraryListKey::PlaylistTracks, &settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    pub(crate) fn playlist_entries_projection(
        self: &Rc<Self>,
        detail: &::library::PlaylistDetail,
        selection_handle: Option<PlaylistEntrySelectionHandle>,
        can_remove_entries: bool,
        can_reorder_entries: bool,
    ) -> PlaylistEntryProjection {
        let entries = Rc::new(RefCell::new(detail.entries.clone()));
        let settings = self
            .settings
            .current
            .borrow()
            .library_list(LibraryListKey::PlaylistTracks);
        let state = Rc::new(RefCell::new(PlaylistEntryListState::for_settings(
            &settings,
        )));
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);

        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        search.set_width_request(1);
        self.set_route_search(Some(search.clone()));
        let toolbar =
            self.library_toolbar_projection(LibraryListKey::PlaylistTracks, search.clone());
        let toolbar_widget = library_route_inset(toolbar.widget());
        toolbar_widget.set_visible(!entries.borrow().is_empty());
        wrapper.append(&toolbar_widget);

        let (collection, model) = playlist_entries_collection_projection(
            self,
            Rc::clone(&entries),
            Rc::clone(&state),
            detail.playlist.id.clone(),
            PRIMARY_ROUTE_HORIZONTAL_INSET,
            selection_handle,
            can_remove_entries,
            can_reorder_entries,
        );
        rebuild_playlist_entries_model(&model, &entries.borrow(), &state.borrow());
        self.refresh_current_route_now_playing_selections();

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            search.connect_search_changed(move |entry| {
                state.borrow_mut().query = entry.text().trim().to_string();
                rebuild_playlist_entries_model(&model, &entries.borrow(), &state.borrow());
                shell.refresh_current_route_now_playing_selections();
            });
        }
        let content = collection.scrolling_widget();
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(
            &library_route_inset(
                self.placeholder_view("Tracks", "No cached tracks are linked here yet."),
            ),
            Some("empty"),
        );
        stack.add_named(&content, Some("content"));
        stack.set_visible_child_name(if entries.borrow().is_empty() {
            "empty"
        } else {
            "content"
        });
        wrapper.append(&stack);

        let replace_entries = {
            let shell = Rc::clone(self);
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            let model = model.clone();
            let stack = stack.clone();
            let toolbar_widget = toolbar_widget.clone();
            Rc::new(move |next: Vec<PlaylistEntry>| {
                let empty = next.is_empty();
                *entries.borrow_mut() = next;
                rebuild_playlist_entries_model(&model, &entries.borrow(), &state.borrow());
                shell.refresh_current_route_now_playing_selections();
                toolbar_widget.set_visible(!empty);
                stack.set_visible_child_name(if empty { "empty" } else { "content" });
            }) as Rc<dyn Fn(Vec<PlaylistEntry>)>
        };
        PlaylistEntryProjection {
            widget: wrapper.upcast(),
            entries,
            replace_entries,
            collection,
            toolbar,
            state,
            model,
            applied_settings: Rc::new(RefCell::new(settings)),
            refresh_selection: {
                let shell = Rc::clone(self);
                Rc::new(move || shell.refresh_current_route_now_playing_selections())
            },
        }
    }
}

#[derive(Clone)]
struct PlaylistDetailSummary {
    row: gtk::Box,
    track_count: gtk::Label,
    track_count_value: Rc<Cell<u32>>,
    duration: gtk::Label,
}

impl PlaylistDetailSummary {
    fn new(track_count: u32, duration_seconds: u32) -> Self {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_halign(gtk::Align::Start);
        let (track_count_item, track_count_label) = playlist_detail_summary_item(
            "rufin-route-tracks-symbolic",
            &track_count_text(track_count.into()),
        );
        let track_count_value = Rc::new(Cell::new(track_count));
        let track_count_for_locale = Rc::clone(&track_count_value);
        bind_label_text_with(&track_count_label, move || {
            track_count_text(u64::from(track_count_for_locale.get()))
        });
        let (duration_item, duration_label) = playlist_detail_summary_item(
            "appointment-soon-symbolic",
            &format_duration_units(duration_seconds),
        );
        row.append(&track_count_item);
        row.append(&duration_item);
        Self {
            row,
            track_count: track_count_label,
            track_count_value,
            duration: duration_label,
        }
    }

    fn widget(&self) -> gtk::Widget {
        self.row.clone().upcast()
    }

    fn set(&self, track_count: u32, duration_seconds: u32) {
        self.track_count_value.set(track_count);
        self.track_count
            .set_text(&track_count_text(track_count.into()));
        self.duration
            .set_text(&format_duration_units(duration_seconds));
    }
}

fn playlist_detail_summary_item(icon_name: &str, text: &str) -> (gtk::Box, gtk::Label) {
    let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("muted");
    icon.set_pixel_size(14);
    item.append(&icon);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    item.append(&label);
    (item, label)
}

#[cfg(test)]
mod tests {
    use crate::format_duration_units;
    use crate::routes::route_layout::{detail_showcase_cover_only, detail_showcase_cover_size};

    use super::playlist_cover_size;

    #[test]
    fn playlist_detail_duration_uses_units() {
        assert_eq!(format_duration_units(57), "57s");
        assert_eq!(format_duration_units(4_497), "1h 14m 57s");
    }

    #[test]
    fn mounted_detail_covers_track_width_without_resize_jumps() {
        let mut previous_media = detail_showcase_cover_size(96);
        let mut previous_collection = playlist_cover_size(96);
        for width in 97..=900 {
            let media = detail_showcase_cover_size(width);
            let collection = playlist_cover_size(width);
            assert!(media >= previous_media && media - previous_media <= 1);
            assert!(collection >= previous_collection && collection - previous_collection <= 1);
            if detail_showcase_cover_only(width) {
                assert!(media <= width);
                assert!(collection <= width);
            }
            previous_media = media;
            previous_collection = collection;
        }
    }
}
