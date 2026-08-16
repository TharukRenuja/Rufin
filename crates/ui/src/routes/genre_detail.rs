use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use ::library::{GenreDetail, GenreSummary, Library, MusicFolderId, RadioSeed};
use adw::prelude::*;
use artwork::ArtworkBinding;

use crate::format_duration_units;
use crate::localization::localized_label;
use crate::shell::Shell;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::route::{LatestMountedRouteRead, MountedRoute, SelectedRouteIdentity};
use crate::{LibraryListKey, LibraryListSettings};
use localization::{msgid, track_count_text};
use playback::RadioPlayRequest;

use super::collection_context::present_genre_context_menu;
use super::collections::CollectionPlay;
use super::detail_showcase::detail_radio_button;
use super::grouped_detail::GroupedDetailData;
use super::route::Route;
use super::track_model::{
    PreparedTrackProjection, TrackProjectionRequest, prepare_track_projection,
};

#[derive(Clone)]
struct GenreDetailReadRequest {
    identity: SelectedRouteIdentity,
    tracks: TrackProjectionRequest,
}

struct PreparedGenreDetail {
    summary: GenreSummary,
    tracks: PreparedTrackProjection,
}

impl Shell {
    pub(crate) fn genre_detail_view(
        self: &Rc<Self>,
        genre_id: ::library::GenreId,
        detail: Option<GenreDetail>,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            return MountedRoute::static_widget(
                self.placeholder_view("Genre", msgid("This isn't available")),
            );
        };
        let genre = detail.summary;
        let tracks = detail.tracks;
        let seed = stable_seed(genre.genre.id.as_str());
        let summary_items = genre_summary_items(&genre);
        let artwork = ArtworkBinding::genre_slots(&genre.genre, &genre.representative_albums);
        let current_genre = Rc::new(RefCell::new(genre.clone()));
        let kind_row = self.genre_detail_kind_row(Rc::clone(&current_genre));
        let context_id = format!("genre:{}", genre_id.as_str());
        let menu_shell = Rc::clone(self);
        let menu_genre = Rc::clone(&current_genre);
        let context_menu = Rc::new(
            move |target: &gtk::Widget, position: Option<(f64, f64)>, play: CollectionPlay| {
                let genre = menu_genre.borrow().clone();
                present_genre_context_menu(target, &menu_shell, genre, Some(play), position);
            },
        );
        let grouped = self.grouped_detail_view(GroupedDetailData {
            key: LibraryListKey::GenreTracks,
            kind_row: Some(kind_row.upcast()),
            title: genre.genre.name.clone(),
            artwork,
            seed,
            summary_items,
            context_menu: Some(context_menu),
            tracks,
            table_context: "genre-detail",
            playback_context: context_id.clone(),
            play_label: msgid("Play genre"),
        });
        let track_count = Rc::new(Cell::new(genre.track_count));
        let localized_track_count = Rc::clone(&track_count);
        grouped.bind_summary_text_with(0, move || {
            track_count_text(u64::from(localized_track_count.get()))
        });
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&grouped.widget(), Some("content"));
        stack.add_named(
            &self.placeholder_view("Genre", msgid("This isn't available")),
            Some("missing"),
        );
        stack.set_visible_child_name("content");
        let identity = self.mounted_route_read_identity(
            Route::GenreDetail(genre_id.clone()),
            &loaded,
            music_folder_id.clone(),
        );
        let apply = {
            let shell = Rc::clone(self);
            let grouped = grouped.clone();
            let stack = stack.clone();
            let current_genre = Rc::clone(&current_genre);
            let track_count = Rc::clone(&track_count);
            Rc::new(
                move |request: GenreDetailReadRequest,
                      result: Result<Option<PreparedGenreDetail>, String>| {
                    if !shell.mounted_route_read_is_current(&request.identity) {
                        return;
                    }
                    let next = match result {
                        Ok(next) => next,
                        Err(error) => {
                            tracing::warn!(%error, "failed to refresh the mounted Genre route");
                            return;
                        }
                    };
                    let Some(next) = next else {
                        stack.set_visible_child_name("missing");
                        return;
                    };
                    let artwork = ArtworkBinding::genre_slots(
                        &next.summary.genre,
                        &next.summary.representative_albums,
                    );
                    if !grouped.replace_prepared(
                        &shell,
                        &next.summary.genre.name,
                        &artwork,
                        stable_seed(next.summary.genre.id.as_str()),
                        &genre_summary_items(&next.summary),
                        next.tracks,
                    ) {
                        return;
                    }
                    track_count.set(next.summary.track_count);
                    current_genre.replace(next.summary);
                    stack.set_visible_child_name("content");
                },
            )
        };
        let load = {
            let loaded = Arc::clone(&loaded);
            let genre_id = genre_id.clone();
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |request: &GenreDetailReadRequest| {
                load_genre_detail(
                    &loaded,
                    &genre_id,
                    music_folder_id.as_ref(),
                    &request.tracks.settings,
                )
                .and_then(|detail| {
                    detail
                        .map(|GenreDetail { summary, tracks }| {
                            prepare_track_projection(tracks, request.tracks.clone())
                                .map(|tracks| PreparedGenreDetail { summary, tracks })
                                .map_err(|error| error.to_string())
                        })
                        .transpose()
                })
            })
        };
        let read = LatestMountedRouteRead::new_with_request(apply, load, "mounted Genre route");
        {
            let read = Rc::downgrade(&read);
            let identity = identity.clone();
            grouped.tracks().connect_search_request(move |tracks| {
                let Some(read) = read.upgrade() else {
                    return;
                };
                read.request_with_if_running(GenreDetailReadRequest {
                    identity: identity.clone(),
                    tracks,
                });
            });
        }
        let resume = {
            let shell = Rc::clone(self);
            let grouped = grouped.clone();
            let read = Rc::clone(&read);
            let identity = identity.clone();
            Rc::new(move || {
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::GenreTracks);
                grouped.apply_library_list_settings(LibraryListKey::GenreTracks, &settings);
                read.request_with_if_running(GenreDetailReadRequest {
                    identity: identity.clone(),
                    tracks: grouped.tracks().projection_request(),
                });
            })
        };
        let update = {
            let genre_id = genre_id.clone();
            let grouped = grouped.clone();
            let read = Rc::clone(&read);
            let identity = identity.clone();
            let music_folder_id = music_folder_id.clone();
            Rc::new(move |update: &crate::runtime::SelectedLibraryUpdate| {
                let replacements = update.change.tracks.as_slice();
                if !update.change.genres.contains(&genre_id) {
                    if replacements.is_empty() {
                        return;
                    }
                    if grouped
                        .tracks()
                        .apply_track_replacement(replacements, |track| {
                            track
                                .relations
                                .genres
                                .iter()
                                .any(|genre| genre.id == genre_id)
                                && music_folder_id.as_ref().is_none_or(|folder_id| {
                                    track.relations.music_folders.contains(folder_id)
                                })
                        })
                    {
                        return;
                    }
                }
                read.request_with(GenreDetailReadRequest {
                    identity: identity.clone(),
                    tracks: grouped.tracks().projection_request(),
                });
            })
        };
        MountedRoute::new(stack.upcast(), resume)
            .with_item_navigation(grouped.item_navigation())
            .with_library_update(update)
    }

    fn genre_detail_kind_row(self: &Rc<Self>, genre: Rc<RefCell<GenreSummary>>) -> gtk::Box {
        let kind = localized_label("Genre");
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

        let radio = detail_radio_button();
        let controller = self.products.playback.radio.clone();
        radio.connect_clicked(move |_| {
            let genre = genre.borrow();
            controller.play_radio(RadioPlayRequest::now(RadioSeed::Genre {
                id: genre.genre.id.clone(),
                name: genre.genre.name.clone(),
            }));
        });
        row.append(&radio);
        row
    }
}

pub(crate) fn load_genre_detail(
    loaded: &Arc<Library>,
    genre_id: &::library::GenreId,
    music_folder_id: Option<&MusicFolderId>,
    settings: &LibraryListSettings,
) -> Result<Option<GenreDetail>, String> {
    let mut detail = loaded
        .genre_detail(genre_id, music_folder_id)
        .map_err(|error| error.to_string())?;
    if let Some(detail) = detail.as_mut() {
        detail.tracks = detail
            .tracks
            .sorted(settings.sort_key.track_sort(), settings.descending)
            .map_err(|error| error.to_string())?;
    }
    Ok(detail)
}

fn genre_summary_items(genre: &GenreSummary) -> Vec<(&'static str, String)> {
    let mut items = vec![(
        "rufin-route-tracks-symbolic",
        track_count_text(genre.track_count.into()),
    )];
    if genre.duration_seconds > 0 {
        items.push((
            "preferences-system-time-bundled-symbolic",
            format_duration_units(genre.duration_seconds),
        ));
    }
    items
}
