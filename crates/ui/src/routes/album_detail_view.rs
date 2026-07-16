use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use ::library::{
    ActiveLibraryQuery, AlbumDetail, AlbumDetailProjection, AlbumId, FavoriteItemId, GenreLink,
    play_context::PlayContextDescriptor,
};
use adw::prelude::*;
use artwork::ArtworkBinding;

use crate::LibraryListKey;
use crate::favorites::{
    album_favorite_key, favorite_button_is_active, favorite_icon_button, set_favorite_button_active,
};
use crate::format_duration_units;
use crate::interactions::{add_dynamic_link_hover, add_label_click};
use crate::localization::bind_label_text_with;
use crate::shell::Shell;
use crate::shell::actions::PLAY_ICON;
use crate::shell::actions::{ActionButtonVariant, configure_action_button};
use crate::shell::cover::cover_fetch_size_for_display;
use crate::shell::route::MountedRoute;
use localization::tr;
use localization::track_count_text;
use playback::{AlbumPlayRequest, RadioPlayRequest, RadioSeed};
use tracing::warn;

use super::collection_routes::{MountedRefreshLoader, MountedRouteRefresh};
use super::collections::{library_route_inset, set_library_table_content_height};
use super::detail_links::album_artist_route;
use super::detail_showcase::{
    DetailSummaryProjection, MediaDetailShowcase, album_external_links,
    append_track_query_batch_queue_actions, detail_action_row, detail_cover_projection,
    detail_genre_pill_button, detail_primary_action_button, detail_radio_button, fit_detail_text,
    fitted_detail_title_label, media_detail_showcase,
};
use super::play_context::selected_music_folder_id;
use super::release_kind::album_release_kind_label;
use super::route::Route;
use super::route_layout::{
    PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START, ROUTE_TOP_MARGIN,
    detail_route_inner_width, detail_showcase_cover_size,
};
use super::route_layout::{detail_route_scroller, detail_route_wrapper};
use super::routes::SearchableTrackOptions;

const ALBUM_DETAIL_ROUTE_INSET: i32 = PRIMARY_ROUTE_MARGIN_START + PRIMARY_ROUTE_MARGIN_END;

fn load_album_detail_refresh(
    query: &ActiveLibraryQuery,
    album_id: &AlbumId,
) -> Result<Option<AlbumDetailProjection>, String> {
    query.album_detail_projection(album_id)
}

pub(crate) fn load_album_detail_for_revision(
    query: &ActiveLibraryQuery,
    revision: i64,
    album_id: &AlbumId,
) -> Result<Option<AlbumDetailProjection>, String> {
    query.album_detail_projection_for_revision(revision, album_id)
}

impl Shell {
    pub(crate) fn album_detail_view_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        album_id: AlbumId,
        loaded: Option<AlbumDetailProjection>,
    ) -> MountedRoute {
        let Some(AlbumDetailProjection {
            detail: AlbumDetail { album, tracks },
            genre_links,
        }) = loaded
        else {
            let active_source_id = library_query.source_id().to_string();
            let player_source_id = self
                .playback
                .player
                .borrow()
                .as_ref()
                .map(|player| player.transport.source_id.to_string());
            warn!(
                album_id = album_id.as_str(),
                active_source_id, player_source_id, "album route missing"
            );
            return MountedRoute::static_widget(
                self.placeholder_view("Album", "The selected cached album was not found."),
            );
        };
        let current_album = Rc::new(RefCell::new(album.clone()));
        let applied_external_link_policy = {
            let settings = self.settings.current.borrow();
            Rc::new(RefCell::new((
                settings.private_mode,
                settings.external_site_links.clone(),
            )))
        };

        let wrapper = detail_route_wrapper(0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
        content.set_margin_top(ROUTE_TOP_MARGIN);
        content.set_margin_bottom(36);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);

        let inner_content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let cover_size = detail_showcase_cover_size(inner_content_width);
        let cover_fetch_size = cover_fetch_size_for_display(cover_size);
        let cover = detail_cover_projection(
            self,
            ArtworkBinding::album(&album),
            album.color_seed,
            cover_size,
            cover_fetch_size,
            "album-detail-cover",
        );
        let facts = DetailSummaryProjection::new(&[
            ("x-office-calendar-symbolic", album.year.to_string()),
            (
                "rufin-route-tracks-symbolic",
                track_count_text(album.track_count.into()),
            ),
            (
                "appointment-soon-symbolic",
                format_duration_units(album.duration_seconds),
            ),
        ]);
        let fact_track_count = Rc::new(Cell::new(u64::from(album.track_count)));
        let fact_track_count_for_locale = Rc::clone(&fact_track_count);
        facts.bind_text_with(1, move || {
            track_count_text(fact_track_count_for_locale.get())
        });
        let text_stack = gtk::Box::new(gtk::Orientation::Vertical, 8);
        text_stack.set_hexpand(true);
        text_stack.set_halign(gtk::Align::Fill);
        text_stack.set_width_request(1);
        let kind_message = Rc::new(RefCell::new(album_release_kind_label(&album).to_string()));
        let kind = gtk::Label::new(None);
        let kind_message_for_locale = Rc::clone(&kind_message);
        bind_label_text_with(&kind, move || tr(&kind_message_for_locale.borrow()));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        kind.set_halign(gtk::Align::Start);
        kind.set_valign(gtk::Align::Center);
        kind.set_margin_end(6);
        let kind_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        kind_row.add_css_class("album-detail-kind-row");
        kind_row.add_css_class("album-detail-genre-row");
        kind_row.set_valign(gtk::Align::Center);
        kind_row.set_halign(gtk::Align::Start);
        kind_row.append(&kind);
        let radio = detail_radio_button();
        let controller = self.products.playback.radio.clone();
        let radio_query = library_query.clone();
        let radio_album_id = album.id.clone();
        radio.connect_clicked(move |_| {
            if let Ok(Some((album, _))) = radio_query.album_detail(&radio_album_id) {
                controller.play_radio(RadioPlayRequest::now(RadioSeed::Album(album)));
            }
        });
        kind_row.append(&radio);
        let genres = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        kind_row.append(&genres);
        self.replace_album_genre_buttons(&genres, &genre_links);
        let title = fitted_detail_title_label(&album.title);
        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("detail-artist");
        artist.set_xalign(0.0);
        artist.set_halign(gtk::Align::Start);
        artist.set_wrap(true);
        artist.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        artist.set_width_request(1);
        artist.set_width_chars(1);
        artist.set_max_width_chars(32);
        fit_detail_text(&artist, &album.artist);
        if album_artist_route(&album).is_some() {
            artist.set_cursor_from_name(Some("pointer"));
            add_dynamic_link_hover(artist.upcast_ref(), &artist);
            let shell = Rc::clone(self);
            let click_query = library_query.clone();
            let click_album_id = album.id.clone();
            add_label_click(&artist, move || {
                if let Ok(Some((album, _))) = click_query.album_detail(&click_album_id)
                    && let Some(route) = album_artist_route(&album)
                {
                    shell.navigate(route);
                }
            });
        }
        text_stack.append(&kind_row);
        text_stack.append(&title);
        text_stack.append(&artist);
        text_stack.append(&facts.widget());
        let actions = detail_action_row();
        actions.add_css_class("album-detail-actions");
        actions.set_halign(gtk::Align::Start);
        let play_album = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.products.playback.queue.clone();
        let album_id_for_play = album.id.clone();
        let play_query = library_query.clone();
        play_album.connect_clicked(move |_| {
            if let Ok(Some((_, tracks))) = play_query.album_detail(&album_id_for_play) {
                controller.play_album(AlbumPlayRequest {
                    album_id: album_id_for_play.clone(),
                    tracks,
                    anchor_index: 0,
                    shuffled_start: true,
                });
            }
        });
        actions.append(&play_album);

        let batch_query = library_query.clone();
        let batch_album_id = album.id.clone();
        append_track_query_batch_queue_actions(
            &actions,
            &self.products.playback.queue,
            Rc::new(move || {
                batch_query
                    .album_detail(&batch_album_id)
                    .ok()
                    .flatten()
                    .map(|(_, tracks)| tracks)
                    .unwrap_or_default()
            }),
        );

        let favorite = favorite_icon_button("Favorite");
        configure_action_button(&favorite, ActionButtonVariant::DetailFavorite, None);
        set_favorite_button_active(&favorite, album.favorite);
        self.favorites
            .register_button(album_favorite_key(&album.id), &favorite);
        let shell = Rc::clone(self);
        let favorite_album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            shell.set_favorite_with_feedback(
                FavoriteItemId::Album(favorite_album_id.clone()),
                favorite,
                Some(button),
            );
        });
        actions.append(&favorite);

        let external_links = gtk::Box::new(gtk::Orientation::Vertical, 0);
        if let Some(links) = album_external_links(self, &album) {
            external_links.append(&links);
        }

        let showcase = media_detail_showcase(
            self,
            MediaDetailShowcase {
                route_class: "album-detail-showcase",
                seed: album.color_seed,
                initial_width: inner_content_width,
                cover: cover.clone(),
                external_links: Some(external_links.clone().upcast()),
                external_links_class: Some("album-detail-link-stack"),
                text_stack: text_stack.upcast(),
                actions: actions.upcast(),
            },
        );
        content.append(&showcase);

        let table_scroller = gtk::ScrolledWindow::new();
        table_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        table_scroller.set_width_request(1);
        table_scroller.set_min_content_width(0);
        table_scroller.set_max_content_width(1);
        table_scroller.set_propagate_natural_width(false);
        table_scroller.set_propagate_natural_height(false);
        table_scroller.set_hexpand(true);
        table_scroller.set_halign(gtk::Align::Fill);
        let resize_scroller = table_scroller.clone();
        let resize_tracks: Rc<dyn Fn(usize)> = Rc::new(move |row_count| {
            set_library_table_content_height(&resize_scroller, row_count, None);
        });
        let track_projection = self.searchable_track_collection(
            tracks,
            LibraryListKey::AlbumDetailTracks,
            SearchableTrackOptions {
                on_visible_count_changed: Some(resize_tracks),
                source_descriptor: Some(PlayContextDescriptor::Album {
                    album_id: album.id.clone(),
                    music_folder_id: selected_music_folder_id(self),
                }),
                favorites_only: false,
                content_inset: ALBUM_DETAIL_ROUTE_INSET,
                fixed_layout: None,
            },
        );
        let table = gtk::Box::new(gtk::Orientation::Vertical, 10);
        table.set_widget_name("album-detail");
        table.set_hexpand(true);
        table.set_halign(gtk::Align::Fill);
        table.set_width_request(1);
        let track_toolbar = self.library_toolbar_projection(
            LibraryListKey::AlbumDetailTracks,
            track_projection.search(),
        );
        table.append(&track_toolbar.widget());
        self.set_route_search(Some(track_projection.search()));
        let table_surface = track_projection.mount_in_scroller(&table_scroller);
        table.append(&table_surface);
        content.append(&table);

        wrapper.append(&detail_route_scroller(
            self,
            library_route_inset(content.upcast()),
        ));
        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&wrapper, Some("detail"));
        route_stack.add_named(
            &self.placeholder_view("Album", "The selected cached album was not found."),
            Some("missing"),
        );
        route_stack.set_visible_child_name("detail");

        let shell = Rc::clone(self);
        let apply_stack = route_stack.clone();
        let apply_kind_message = Rc::clone(&kind_message);
        let apply_fact_track_count = Rc::clone(&fact_track_count);
        let apply_external_links = external_links.clone();
        let apply_current_album = Rc::clone(&current_album);
        let apply_external_link_policy = Rc::clone(&applied_external_link_policy);
        let delta_track_projection = track_projection.clone();
        let apply_loaded: Rc<dyn Fn(Result<Option<AlbumDetailProjection>, String>)> =
            Rc::new(move |result| {
                let AlbumDetailProjection {
                    detail: AlbumDetail { album, tracks },
                    genre_links,
                } = match result {
                    Ok(Some(loaded)) => loaded,
                    Ok(None) => {
                        apply_stack.set_visible_child_name("missing");
                        return;
                    }
                    Err(error) => {
                        warn!(%error, "failed to refresh Album detail projection");
                        return;
                    }
                };
                let album_kind = album_release_kind_label(&album);
                apply_kind_message.replace(album_kind.to_string());
                kind.set_text(&tr(album_kind));
                title.set_text(&album.title);
                title.remove_css_class("detail-text-long");
                title.remove_css_class("detail-text-very-long");
                fit_detail_text(&title, &album.title);
                artist.set_text(&album.artist);
                artist.remove_css_class("detail-text-long");
                artist.remove_css_class("detail-text-very-long");
                fit_detail_text(&artist, &album.artist);
                facts.replace(&[
                    ("x-office-calendar-symbolic", album.year.to_string()),
                    (
                        "rufin-route-tracks-symbolic",
                        track_count_text(album.track_count.into()),
                    ),
                    (
                        "appointment-soon-symbolic",
                        format_duration_units(album.duration_seconds),
                    ),
                ]);
                apply_fact_track_count.set(u64::from(album.track_count));
                shell.replace_album_genre_buttons(&genres, &genre_links);
                while let Some(child) = apply_external_links.first_child() {
                    apply_external_links.remove(&child);
                }
                if let Some(links) = album_external_links(&shell, &album) {
                    apply_external_links.append(&links);
                }
                {
                    let settings = shell.settings.current.borrow();
                    apply_external_link_policy
                        .replace((settings.private_mode, settings.external_site_links.clone()));
                }
                set_favorite_button_active(&favorite, album.favorite);
                cover.replace(&shell, ArtworkBinding::album(&album), album.color_seed);
                delta_track_projection.replace(tracks);
                apply_current_album.replace(album);
                apply_stack.set_visible_child_name("detail");
            });
        let load_query = library_query.clone();
        let load_album_id = album_id;
        let load: MountedRefreshLoader<Result<Option<AlbumDetailProjection>, String>> =
            Arc::new(move || load_album_detail_refresh(&load_query, &load_album_id));
        let refresh =
            MountedRouteRefresh::new(Rc::downgrade(&apply_loaded), load, "mounted Album detail");
        let affected_by = Rc::new(|delta: &::library::LibraryDelta| {
            delta.reset.is_some() || !delta.albums.is_empty() || !delta.tracks.is_empty()
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
            let current_album = Rc::clone(&current_album);
            let external_links = external_links.clone();
            let applied_external_link_policy = Rc::clone(&applied_external_link_policy);
            Rc::new(move || {
                let external_link_policy = {
                    let settings = shell.settings.current.borrow();
                    (settings.private_mode, settings.external_site_links.clone())
                };
                if *applied_external_link_policy.borrow() != external_link_policy {
                    while let Some(child) = external_links.first_child() {
                        external_links.remove(&child);
                    }
                    if let Some(links) = album_external_links(&shell, &current_album.borrow()) {
                        external_links.append(&links);
                    }
                    applied_external_link_policy.replace(external_link_policy);
                }
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::AlbumDetailTracks);
                track_projection
                    .apply_library_list_settings(LibraryListKey::AlbumDetailTracks, &settings);
                track_toolbar.apply(LibraryListKey::AlbumDetailTracks, &settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    fn replace_album_genre_buttons(self: &Rc<Self>, row: &gtk::Box, genre_links: &[GenreLink]) {
        while let Some(child) = row.first_child() {
            row.remove(&child);
        }
        for link in genre_links
            .iter()
            .filter(|link| !link.name.trim().is_empty())
        {
            let genre_name = link.name.trim();
            let button = detail_genre_pill_button(genre_name);
            if let Some(genre_id) = link.id.clone() {
                let shell = Rc::clone(self);
                button
                    .connect_clicked(move |_| shell.navigate(Route::GenreDetail(genre_id.clone())));
            } else {
                button.set_sensitive(false);
            }
            row.append(&button);
        }
    }
}
