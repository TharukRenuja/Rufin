use std::{cell::Cell, rc::Rc, sync::Arc};

use ::library::{Library, MoodDetail, MoodSummary, MusicFolderId};
use adw::prelude::*;
use artwork::ArtworkBinding;

use crate::format_duration_units;
use crate::localization::localized_label;
use crate::shell::Shell;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::route::{LatestMountedRouteRead, MountedRoute, SelectedRouteIdentity};
use crate::{LibraryListKey, LibraryListSettings};
use localization::{msgid, track_count_text};

use super::grouped_detail::GroupedDetailData;
use super::route::Route;
use super::track_model::{
    PreparedTrackProjection, TrackProjectionRequest, prepare_track_projection,
};

#[derive(Clone)]
struct MoodDetailReadRequest {
    identity: SelectedRouteIdentity,
    tracks: TrackProjectionRequest,
}

struct PreparedMoodDetail {
    summary: MoodSummary,
    tracks: PreparedTrackProjection,
}

impl Shell {
    pub(crate) fn mood_detail_view(
        self: &Rc<Self>,
        mood_id: ::library::MoodId,
        detail: Option<MoodDetail>,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            return MountedRoute::static_widget(self.placeholder_view(
                "Mood",
                "Files need Mood/BPM tags written on them. Not supported for Jellyfin",
            ));
        };
        let mood = detail.summary;
        let tracks = detail.tracks;
        let seed = stable_seed(mood.mood.id.as_str());
        let summary_items = mood_summary_items(&mood);
        let artwork = ArtworkBinding::mood_slots(&mood.mood, &mood.representative_albums);
        let context_id = format!("mood:{}", mood_id.as_str());
        let grouped = self.grouped_detail_view(GroupedDetailData {
            key: LibraryListKey::MoodTracks,
            kind_row: Some(self.mood_detail_kind_row().upcast()),
            title: mood.mood.name.clone(),
            artwork,
            seed,
            summary_items,
            context_menu: None,
            tracks,
            table_context: "mood-detail",
            playback_context: context_id.clone(),
            play_label: msgid("Play mood"),
        });
        let track_count = Rc::new(Cell::new(mood.track_count));
        let localized_track_count = Rc::clone(&track_count);
        grouped.bind_summary_text_with(0, move || {
            track_count_text(u64::from(localized_track_count.get()))
        });
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&grouped.widget(), Some("content"));
        stack.add_named(
            &self.placeholder_view("Mood", msgid("This isn't available")),
            Some("missing"),
        );
        stack.set_visible_child_name("content");
        let identity = self.mounted_route_read_identity(
            Route::MoodDetail(mood_id.clone()),
            &loaded,
            music_folder_id.clone(),
        );
        let apply = {
            let shell = Rc::clone(self);
            let grouped = grouped.clone();
            let stack = stack.clone();
            let track_count = Rc::clone(&track_count);
            Rc::new(
                move |request: MoodDetailReadRequest,
                      result: Result<Option<PreparedMoodDetail>, String>| {
                    if !shell.mounted_route_read_is_current(&request.identity) {
                        return;
                    }
                    let next = match result {
                        Ok(next) => next,
                        Err(error) => {
                            tracing::warn!(%error, "failed to refresh the mounted Mood route");
                            return;
                        }
                    };
                    let Some(next) = next else {
                        stack.set_visible_child_name("missing");
                        return;
                    };
                    let artwork = ArtworkBinding::mood_slots(
                        &next.summary.mood,
                        &next.summary.representative_albums,
                    );
                    if !grouped.replace_prepared(
                        &shell,
                        &next.summary.mood.name,
                        &artwork,
                        stable_seed(next.summary.mood.id.as_str()),
                        &mood_summary_items(&next.summary),
                        next.tracks,
                    ) {
                        return;
                    }
                    track_count.set(next.summary.track_count);
                    stack.set_visible_child_name("content");
                },
            )
        };
        let load = {
            let loaded = Arc::clone(&loaded);
            let mood_id = mood_id.clone();
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |request: &MoodDetailReadRequest| {
                load_mood_detail(
                    &loaded,
                    &mood_id,
                    music_folder_id.as_ref(),
                    &request.tracks.settings,
                )
                .and_then(|detail| {
                    detail
                        .map(|MoodDetail { summary, tracks }| {
                            prepare_track_projection(tracks, request.tracks.clone())
                                .map(|tracks| PreparedMoodDetail { summary, tracks })
                                .map_err(|error| error.to_string())
                        })
                        .transpose()
                })
            })
        };
        let read = LatestMountedRouteRead::new_with_request(apply, load, "mounted Mood route");
        {
            let read = Rc::downgrade(&read);
            let identity = identity.clone();
            grouped.tracks().connect_search_request(move |tracks| {
                let Some(read) = read.upgrade() else {
                    return;
                };
                read.request_with_if_running(MoodDetailReadRequest {
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
                    .library_list(LibraryListKey::MoodTracks);
                grouped.apply_library_list_settings(LibraryListKey::MoodTracks, &settings);
                read.request_with_if_running(MoodDetailReadRequest {
                    identity: identity.clone(),
                    tracks: grouped.tracks().projection_request(),
                });
            })
        };
        let update = {
            let mood_id = mood_id.clone();
            let grouped = grouped.clone();
            let read = Rc::clone(&read);
            let identity = identity.clone();
            let music_folder_id = music_folder_id.clone();
            Rc::new(move |update: &crate::runtime::SelectedLibraryUpdate| {
                let replacements = update.change.tracks.as_slice();
                if !update.change.moods.contains(&mood_id) {
                    if replacements.is_empty() {
                        return;
                    }
                    if grouped
                        .tracks()
                        .apply_track_replacement(replacements, |track| {
                            track.relations.moods.iter().any(|mood| mood.id == mood_id)
                                && music_folder_id.as_ref().is_none_or(|folder_id| {
                                    track.relations.music_folders.contains(folder_id)
                                })
                        })
                    {
                        return;
                    }
                }
                read.request_with(MoodDetailReadRequest {
                    identity: identity.clone(),
                    tracks: grouped.tracks().projection_request(),
                });
            })
        };
        MountedRoute::new(stack.upcast(), resume)
            .with_item_navigation(grouped.item_navigation())
            .with_library_update(update)
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
}

pub(crate) fn load_mood_detail(
    loaded: &Arc<Library>,
    mood_id: &::library::MoodId,
    music_folder_id: Option<&MusicFolderId>,
    settings: &LibraryListSettings,
) -> Result<Option<MoodDetail>, String> {
    let mut detail = loaded
        .mood_detail(mood_id, music_folder_id)
        .map_err(|error| error.to_string())?;
    if let Some(detail) = detail.as_mut() {
        detail.tracks = detail
            .tracks
            .sorted(settings.sort_key.track_sort(), settings.descending)
            .map_err(|error| error.to_string())?;
    }
    Ok(detail)
}

fn mood_summary_items(mood: &MoodSummary) -> Vec<(&'static str, String)> {
    let mut items = vec![(
        "rufin-route-tracks-symbolic",
        track_count_text(mood.track_count.into()),
    )];
    if mood.duration_seconds > 0 {
        items.push((
            "appointment-soon-symbolic",
            format_duration_units(mood.duration_seconds),
        ));
    }
    items
}
