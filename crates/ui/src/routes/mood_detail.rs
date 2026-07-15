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
use playback::MoodWindowPlayRequest;

use super::collection_routes::{MountedRefreshLoader, MountedRouteRefresh};
use super::collections::TrackTableSelectionHandle;
use super::detail_showcase::{
    append_track_query_batch_queue_actions, detail_action_row, detail_primary_action_button,
};
use super::grouped_detail::GroupedDetailData;
use super::play_context::selected_music_folder_id;

impl Shell {
    pub(crate) fn mood_detail_view_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        mood_id: ::library::MoodId,
        detail: Option<::library::CachedMoodDetail>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            return MountedRoute::static_widget(self.placeholder_view(
                "Mood",
                "Files need Mood/BPM tags written on them. Not supported for Jellyfin",
            ));
        };
        let seed = stable_seed(detail.mood.id.as_str());
        let mut summary_items = vec![(
            "rufin-route-tracks-symbolic",
            track_count_text(detail.mood.track_count.into()),
        )];
        if detail.mood.duration_seconds > 0 {
            summary_items.push((
                "appointment-soon-symbolic",
                format_duration_units(detail.mood.duration_seconds),
            ));
        }
        let mood = detail.mood;
        let artwork = ArtworkBinding::mood_slots(&mood);
        let kind_row = self.mood_detail_kind_row();
        let track_selection: TrackTableSelectionHandle = Rc::new(RefCell::new(None));
        let actions = self.mood_detail_actions(&library_query, mood_id.clone());
        let grouped = self.grouped_detail_view(GroupedDetailData {
            key: LibraryListKey::MoodTracks,
            kind_row: Some(kind_row.upcast()),
            title: mood.name.clone(),
            artwork,
            seed,
            summary_items,
            actions: Some(actions.upcast()),
            selection_handle: Some(track_selection),
            tracks: detail.tracks,
            table_context: "mood-detail",
            source_descriptor: Some(PlayContextDescriptor::Mood {
                mood_id: mood_id.clone(),
                music_folder_id: selected_music_folder_id(self),
            }),
        });
        let mounted_track_count = Rc::new(Cell::new(u64::from(mood.track_count)));
        let track_count_for_locale = Rc::clone(&mounted_track_count);
        grouped.bind_summary_text_with(0, move || track_count_text(track_count_for_locale.get()));
        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&grouped.widget(), Some("detail"));
        route_stack.add_named(
            &self.placeholder_view(
                "Mood",
                "Files need Mood/BPM tags written on them. Not supported for Jellyfin",
            ),
            Some("missing"),
        );
        route_stack.set_visible_child_name("detail");

        let shell = Rc::clone(self);
        let apply_stack = route_stack.clone();
        let delta_grouped = grouped.clone();
        let delta_track_count = Rc::clone(&mounted_track_count);
        let apply_loaded: Rc<dyn Fn(Result<Option<::library::CachedMoodDetail>, String>)> =
            Rc::new(move |result| {
                let detail = match result {
                    Ok(Some(detail)) => detail,
                    Ok(None) => {
                        apply_stack.set_visible_child_name("missing");
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to refresh Mood detail projection");
                        return;
                    }
                };
                let mut summary_items = vec![(
                    "rufin-route-tracks-symbolic",
                    track_count_text(detail.mood.track_count.into()),
                )];
                if detail.mood.duration_seconds > 0 {
                    summary_items.push((
                        "appointment-soon-symbolic",
                        format_duration_units(detail.mood.duration_seconds),
                    ));
                }
                let seed = stable_seed(detail.mood.id.as_str());
                let artwork = ArtworkBinding::mood_slots(&detail.mood);
                delta_track_count.set(u64::from(detail.mood.track_count));
                delta_grouped.replace(
                    &shell,
                    &detail.mood.name,
                    &summary_items,
                    &artwork,
                    seed,
                    detail.tracks,
                );
                apply_stack.set_visible_child_name("detail");
            });
        let load_query = library_query.clone();
        let load_mood_id = mood_id.clone();
        let load: MountedRefreshLoader<Result<Option<::library::CachedMoodDetail>, String>> =
            Arc::new(move || load_query.mood_detail(&load_mood_id));
        let refresh =
            MountedRouteRefresh::new(Rc::downgrade(&apply_loaded), load, "mounted Mood detail");
        let affected_by = Rc::new(|delta: &::library::LibraryDelta| {
            delta.reset.is_some() || !delta.tracks.is_empty()
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
                    .library_list(LibraryListKey::MoodTracks);
                grouped.apply_library_list_settings(LibraryListKey::MoodTracks, &settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    fn mood_detail_kind_row(self: &Rc<Self>) -> gtk::Box {
        let kind = localized_label("Mood");
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        kind.set_halign(gtk::Align::Start);
        kind.set_valign(gtk::Align::Center);
        kind.set_margin_end(6);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.add_css_class("album-detail-kind-row");
        row.set_valign(gtk::Align::Center);
        row.set_halign(gtk::Align::Start);
        row.append(&kind);
        row
    }

    fn mood_detail_actions(
        self: &Rc<Self>,
        library_query: &ActiveLibraryQuery,
        mood_id: ::library::MoodId,
    ) -> gtk::Box {
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);

        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.products.playback.queue.clone();
        let play_query = library_query.clone();
        let play_mood_id = mood_id.clone();
        play.connect_clicked(move |_| {
            let tracks = play_query
                .mood_detail(&play_mood_id)
                .ok()
                .flatten()
                .map(|detail| detail.tracks)
                .unwrap_or_default();
            let total_items = tracks.len();
            controller.play_mood_window(MoodWindowPlayRequest {
                mood_id: play_mood_id.clone(),
                total_items,
                anchor_index: 0,
                track_at: Box::new(move |index| tracks.get(index).cloned()),
            });
        });
        actions.append(&play);

        let batch_query = library_query.clone();
        let batch_mood_id = mood_id;
        append_track_query_batch_queue_actions(
            &actions,
            &self.products.playback.queue,
            Rc::new(move || {
                batch_query
                    .mood_detail(&batch_mood_id)
                    .ok()
                    .flatten()
                    .map(|detail| detail.tracks)
                    .unwrap_or_default()
            }),
        );

        actions
    }
}
