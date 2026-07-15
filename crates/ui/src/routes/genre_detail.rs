use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use ::library::{ActiveLibraryQuery, play_context::PlayContextDescriptor};
use adw::prelude::*;
use artwork::ArtworkBinding;

use crate::LibraryListKey;
use crate::format_duration_units;
use crate::localization::localized_label;
use crate::shell::Shell;
use crate::shell::actions::PLAY_ICON;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::route::MountedRoute;
use localization::track_count_text;
use playback::{GenreWindowPlayRequest, RadioPlayRequest, RadioSeed};

use super::collection_routes::{MountedRefreshLoader, MountedRouteRefresh};
use super::collections::TrackTableSelectionHandle;
use super::detail_showcase::{
    append_track_query_batch_queue_actions, detail_action_row, detail_primary_action_button,
    detail_radio_button,
};
use super::grouped_detail::GroupedDetailData;
use super::play_context::selected_music_folder_id;

impl Shell {
    pub(crate) fn genre_detail_view_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        genre_id: ::library::GenreId,
        detail: Option<::library::CachedGenreDetail>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            return MountedRoute::static_widget(
                self.placeholder_view("Genre", "The selected cached genre was not found."),
            );
        };
        let seed = stable_seed(detail.genre.id.as_str());
        let mut summary_items = vec![(
            "rufin-route-tracks-symbolic",
            track_count_text(detail.genre.track_count.into()),
        )];
        if detail.genre.duration_seconds > 0 {
            summary_items.push((
                "appointment-soon-symbolic",
                format_duration_units(detail.genre.duration_seconds),
            ));
        }
        let genre = detail.genre;
        let artwork = ArtworkBinding::genre_slots(&genre);
        let kind_row = self.genre_detail_kind_row(&library_query, &genre_id);
        let track_selection: TrackTableSelectionHandle = Rc::new(RefCell::new(None));
        let actions = self.genre_detail_actions(&library_query, genre_id.clone());
        let grouped = self.grouped_detail_view(GroupedDetailData {
            key: LibraryListKey::GenreTracks,
            kind_row: Some(kind_row.upcast()),
            title: genre.name.clone(),
            artwork,
            seed,
            summary_items,
            actions: Some(actions.upcast()),
            selection_handle: Some(track_selection),
            tracks: detail.tracks,
            table_context: "genre-detail",
            source_descriptor: Some(PlayContextDescriptor::Genre {
                genre_id: genre_id.clone(),
                music_folder_id: selected_music_folder_id(self),
            }),
        });
        let mounted_track_count = Rc::new(Cell::new(u64::from(genre.track_count)));
        let track_count_for_locale = Rc::clone(&mounted_track_count);
        grouped.bind_summary_text_with(0, move || track_count_text(track_count_for_locale.get()));
        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&grouped.widget(), Some("detail"));
        route_stack.add_named(
            &self.placeholder_view("Genre", "The selected cached genre was not found."),
            Some("missing"),
        );
        route_stack.set_visible_child_name("detail");

        let shell = Rc::clone(self);
        let apply_stack = route_stack.clone();
        let delta_grouped = grouped.clone();
        let delta_track_count = Rc::clone(&mounted_track_count);
        let apply_loaded: Rc<dyn Fn(Result<Option<::library::CachedGenreDetail>, String>)> =
            Rc::new(move |result| {
                let detail = match result {
                    Ok(Some(detail)) => detail,
                    Ok(None) => {
                        apply_stack.set_visible_child_name("missing");
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to refresh Genre detail projection");
                        return;
                    }
                };
                let mut summary_items = vec![(
                    "rufin-route-tracks-symbolic",
                    track_count_text(detail.genre.track_count.into()),
                )];
                if detail.genre.duration_seconds > 0 {
                    summary_items.push((
                        "appointment-soon-symbolic",
                        format_duration_units(detail.genre.duration_seconds),
                    ));
                }
                let seed = stable_seed(detail.genre.id.as_str());
                let artwork = ArtworkBinding::genre_slots(&detail.genre);
                delta_track_count.set(u64::from(detail.genre.track_count));
                delta_grouped.replace(
                    &shell,
                    &detail.genre.name,
                    &summary_items,
                    &artwork,
                    seed,
                    detail.tracks,
                );
                apply_stack.set_visible_child_name("detail");
            });
        let load_query = library_query.clone();
        let load_genre_id = genre_id.clone();
        let load: MountedRefreshLoader<Result<Option<::library::CachedGenreDetail>, String>> =
            Arc::new(move || load_query.genre_detail(&load_genre_id));
        let refresh =
            MountedRouteRefresh::new(Rc::downgrade(&apply_loaded), load, "mounted Genre detail");
        let affected_by = Rc::new(|delta: &::library::LibraryDelta| {
            delta.reset.is_some()
                || !delta.genres.is_empty()
                || !delta.albums.is_empty()
                || !delta.tracks.is_empty()
        });
        let apply_delta = {
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &::library::LibraryDelta| {
                let _ = &apply_loaded;
                refresh.request();
            })
        };
        let resume = {
            let shell = Rc::clone(self);
            Rc::new(move || {
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::GenreTracks);
                grouped.apply_library_list_settings(LibraryListKey::GenreTracks, &settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    fn genre_detail_kind_row(
        self: &Rc<Self>,
        library_query: &ActiveLibraryQuery,
        genre_id: &::library::GenreId,
    ) -> gtk::Box {
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
        let library_query = library_query.clone();
        let genre_id = genre_id.clone();
        radio.connect_clicked(move |_| {
            if let Ok(Some(detail)) = library_query.genre_detail(&genre_id) {
                controller.play_radio(RadioPlayRequest::now(RadioSeed::Genre(detail.genre)));
            }
        });
        row.append(&radio);
        row
    }

    fn genre_detail_actions(
        self: &Rc<Self>,
        library_query: &ActiveLibraryQuery,
        genre_id: ::library::GenreId,
    ) -> gtk::Box {
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);

        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.products.playback.queue.clone();
        let play_query = library_query.clone();
        let play_genre_id = genre_id.clone();
        play.connect_clicked(move |_| {
            let tracks = play_query
                .genre_detail(&play_genre_id)
                .ok()
                .flatten()
                .map(|detail| detail.tracks)
                .unwrap_or_default();
            let total_items = tracks.len();
            controller.play_genre_window(GenreWindowPlayRequest {
                genre_id: play_genre_id.clone(),
                total_items,
                anchor_index: 0,
                track_at: Box::new(move |index| tracks.get(index).cloned()),
            });
        });
        actions.append(&play);

        let batch_query = library_query.clone();
        let batch_genre_id = genre_id;
        append_track_query_batch_queue_actions(
            &actions,
            &self.products.playback.queue,
            Rc::new(move || {
                batch_query
                    .genre_detail(&batch_genre_id)
                    .ok()
                    .flatten()
                    .map(|detail| detail.tracks)
                    .unwrap_or_default()
            }),
        );

        actions
    }
}
