use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use ::library::play_context::{ArtistTrackScope, PlayContextDescriptor};
use ::library::{ActiveLibraryQuery, CachedArtistDetail, FavoriteItemId};
use ::library::{Artist, ArtistId, Track};
use adw::prelude::*;
use artwork::ArtworkBinding;
use tracing::warn;

use crate::LibraryListKey;
use crate::favorites::{
    artist_favorite_key, favorite_button_is_active, favorite_icon_button,
    set_favorite_button_active,
};
use crate::layout::{configure_fill_width_clip, width_allocation_owner};
use crate::localization::{bind_label_text_with, localized_label};
use crate::shell::Shell;
use crate::shell::actions::PLAY_ICON;
use crate::shell::actions::{ActionButtonVariant, configure_action_button};
use crate::shell::cover::cover_fetch_size_for_display;
use crate::shell::cover::presentation::{add_album_seed_gradient_class, stable_seed};
use crate::shell::route::MountedRoute;
use localization::{album_count_text, msgid, track_count_text};
use playback::{ArtistWindowPlayRequest, RadioPlayRequest, RadioSeed};

use super::artist_releases::{
    ArtistReleaseProjections, ArtistReleaseRoutePreamble, ArtistRouteSearchTarget,
};
use super::collection_routes::{MountedRefreshLoader, MountedRouteRefresh};
use super::collections::{
    COMPACT_TRACK_TABLE_HEADER_HEIGHT, configure_compact_track_table_scroller, library_route_inset,
};
use super::detail_showcase::{
    DetailCoverProjection, MediaDetailShowcase, append_track_query_batch_queue_actions,
    artist_external_links, detail_action_row, detail_cover_projection,
    detail_primary_action_button, detail_radio_button, detail_showcase_frame_with_back,
    fit_detail_text, fitted_detail_title_label, mark_tiny_detail_showcase, media_detail_showcase,
};
use super::play_context::selected_music_folder_id;
use super::route::Route;
use super::route_layout::detail_route_wrapper;
use super::route_layout::{
    PRIMARY_ROUTE_HORIZONTAL_INSET, PRIMARY_ROUTE_MARGIN_START, ROUTE_TOP_MARGIN,
    detail_route_inner_width, detail_showcase_cover_size,
};
use super::routes::{SearchableTrackOptions, install_embedded_track_scroll_latch};

const ARTIST_COUNT_ICON_SIZE: i32 = 16;

fn artist_detail_affected(delta: &::library::LibraryDelta) -> bool {
    delta.reset.is_some()
        || !delta.artists.is_empty()
        || !delta.album_artists.is_empty()
        || !delta.albums.is_empty()
        || !delta.tracks.is_empty()
}

fn mounted_artist_refresh(
    query: &ActiveLibraryQuery,
    artist_id: &ArtistId,
    apply: std::rc::Weak<dyn Fn(Result<Option<CachedArtistDetail>, String>)>,
    context: &'static str,
) -> Rc<MountedRouteRefresh<Result<Option<CachedArtistDetail>, String>>> {
    let load_query = query.clone();
    let load_artist_id = artist_id.clone();
    let load: MountedRefreshLoader<Result<Option<CachedArtistDetail>, String>> =
        Arc::new(move || load_query.artist_detail(&load_artist_id));
    MountedRouteRefresh::new(apply, load, context)
}

#[derive(Clone)]
struct ArtistDetailHeaderProjection {
    root: gtk::Widget,
    title: gtk::Label,
    album_count: gtk::Label,
    track_count: gtk::Label,
    cover: DetailCoverProjection,
    external_links: gtk::Box,
    favorite: gtk::Button,
    summary_facts: ArtistSummaryFacts,
    artist: Rc<RefCell<Artist>>,
    tracks: Rc<RefCell<Arc<Vec<Track>>>>,
}

#[derive(Clone)]
struct ArtistSubrouteHeaderProjection {
    root: gtk::Widget,
    title: gtk::Label,
    summary: gtk::Label,
}

#[derive(Clone, Copy)]
struct ArtistSummaryCounts {
    albums: usize,
    appears_on: usize,
    tracks: u32,
}

#[derive(Clone)]
struct ArtistSummaryFacts(Rc<Cell<ArtistSummaryCounts>>);

impl ArtistSummaryFacts {
    fn new(counts: ArtistSummaryCounts) -> Self {
        Self(Rc::new(Cell::new(counts)))
    }

    fn replace(&self, counts: ArtistSummaryCounts) {
        self.0.set(counts);
    }

    fn album_text(&self) -> String {
        let counts = self.0.get();
        album_count_text((counts.albums + counts.appears_on) as u64)
    }

    fn track_text(&self) -> String {
        track_count_text(self.0.get().tracks.into())
    }

    fn summary_text(&self) -> String {
        let counts = self.0.get();
        artist_summary_text(counts.albums, counts.appears_on, counts.tracks)
    }

    fn counts(detail: &CachedArtistDetail) -> ArtistSummaryCounts {
        ArtistSummaryCounts {
            albums: detail.albums.len(),
            appears_on: detail.appears_on.len(),
            tracks: detail
                .artist
                .track_count
                .max(detail.tracks.len().min(u32::MAX as usize) as u32),
        }
    }
}

impl ArtistSubrouteHeaderProjection {
    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn replace(&self, artist: &Artist, summary: &str) {
        self.title.set_text(&artist.name);
        self.title.remove_css_class("detail-text-long");
        self.title.remove_css_class("detail-text-very-long");
        fit_detail_text(&self.title, &artist.name);
        self.summary.set_text(summary);
    }
}

impl ArtistDetailHeaderProjection {
    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn replace(
        &self,
        shell: &Rc<Shell>,
        artist: Artist,
        tracks: Arc<Vec<Track>>,
        summary_counts: ArtistSummaryCounts,
    ) {
        self.summary_facts.replace(summary_counts);
        self.title.set_text(&artist.name);
        self.title.remove_css_class("detail-text-long");
        self.title.remove_css_class("detail-text-very-long");
        fit_detail_text(&self.title, &artist.name);
        self.album_count.set_text(&self.summary_facts.album_text());
        self.track_count.set_text(&self.summary_facts.track_text());
        self.cover.replace(
            shell,
            ArtworkBinding::artist(&artist),
            stable_seed(artist.id.as_str()),
        );
        self.replace_external_links(shell, &artist, &tracks);
        set_favorite_button_active(&self.favorite, artist.favorite);
        self.artist.replace(artist);
        self.tracks.replace(tracks);
    }

    fn apply_external_link_settings(&self, shell: &Rc<Shell>) {
        let artist = self.artist.borrow();
        let tracks = self.tracks.borrow();
        self.replace_external_links(shell, &artist, &tracks);
    }

    fn replace_external_links(&self, shell: &Rc<Shell>, artist: &Artist, tracks: &[Track]) {
        while let Some(child) = self.external_links.first_child() {
            self.external_links.remove(&child);
        }
        if let Some(links) = artist_external_links(shell, artist, tracks) {
            self.external_links.append(&links);
        }
    }
}

impl Shell {
    pub(crate) fn artist_detail_view_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        artist_id: ArtistId,
        detail: Option<CachedArtistDetail>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            let active_source_id = library_query.source_id().to_string();
            let player_source_id = self
                .playback
                .player
                .borrow()
                .as_ref()
                .map(|player| player.transport.source_id.to_string());
            warn!(
                artist_id = artist_id.as_str(),
                active_source_id, player_source_id, "artist route missing"
            );
            return MountedRoute::static_widget(
                self.placeholder_view("Artist", "The selected cached artist was not found."),
            );
        };
        let summary_facts = ArtistSummaryFacts::new(ArtistSummaryFacts::counts(&detail));
        let CachedArtistDetail {
            artist,
            albums,
            appears_on,
            tracks,
        } = detail;
        let tracks = Arc::new(tracks);
        let applied_external_link_policy = {
            let settings = self.settings.current.borrow();
            Rc::new(RefCell::new((
                settings.private_mode,
                settings.external_site_links.clone(),
            )))
        };
        let favorite_tracks = favorite_artist_tracks(&tracks);
        let wrapper = detail_route_wrapper(0);
        let header = self.artist_detail_header(artist, Arc::clone(&tracks), summary_facts);
        let favorite_section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        favorite_section.append(&section_heading(msgid("Favorite tracks")));
        let favorite_scroller = gtk::ScrolledWindow::new();
        let resize_favorite_scroller = favorite_scroller.clone();
        let resize_favorite_tracks: Rc<dyn Fn(usize)> = Rc::new(move |row_count| {
            configure_compact_track_table_scroller(&resize_favorite_scroller, row_count);
        });
        let favorite_projection = self.searchable_track_collection(
            favorite_tracks,
            LibraryListKey::ArtistTracks,
            SearchableTrackOptions {
                on_visible_count_changed: Some(resize_favorite_tracks),
                source_descriptor: Some(PlayContextDescriptor::Artist {
                    artist_id: artist_id.clone(),
                    scope: ArtistTrackScope::AllCredits,
                    music_folder_id: selected_music_folder_id(self),
                }),
                favorites_only: true,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                fixed_layout: Some(crate::LibraryLayout::Row),
            },
        );
        let favorite_panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
        favorite_panel.set_widget_name("artist-favorites");
        favorite_panel.set_hexpand(true);
        favorite_panel.set_halign(gtk::Align::Fill);
        let favorite_toolbar = self
            .library_toolbar_projection(LibraryListKey::ArtistTracks, favorite_projection.search());
        favorite_toolbar.set_layout_control_visible(false);
        favorite_panel.append(&favorite_toolbar.widget());
        configure_fill_width_clip(&favorite_scroller, gtk::PolicyType::Automatic);
        favorite_scroller.set_overlay_scrolling(true);
        install_embedded_track_scroll_latch(&favorite_scroller, COMPACT_TRACK_TABLE_HEADER_HEIGHT);
        favorite_scroller.set_width_request(1);
        favorite_scroller.set_hexpand(true);
        favorite_scroller.set_halign(gtk::Align::Fill);
        let favorite_surface = favorite_projection.mount_in_scroller(&favorite_scroller);
        favorite_panel.append(&favorite_surface);
        favorite_section.append(&favorite_panel);
        let empty = self.placeholder_view(
            "Artist",
            "No cached albums or tracks are linked to this artist yet.",
        );
        let releases = ArtistReleaseProjections::new(
            self,
            library_query.clone(),
            ArtistReleaseRoutePreamble {
                header: header.widget(),
                favorite: Some((favorite_section.upcast(), favorite_projection.search())),
                favorite_present: !favorite_projection.source_is_empty(),
                empty,
            },
            albums,
            appears_on,
        );
        set_artist_route_search(self, releases.primary_search());
        wrapper.append(&releases.widget());
        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&wrapper, Some("detail"));
        route_stack.add_named(
            &self.placeholder_view("Artist", "The selected cached artist was not found."),
            Some("missing"),
        );
        route_stack.set_visible_child_name("detail");

        let shell = Rc::clone(self);
        let apply_stack = route_stack.clone();
        let apply_header = header.clone();
        let delta_favorite_projection = favorite_projection.clone();
        let delta_releases = releases.clone();
        let apply_external_link_policy = Rc::clone(&applied_external_link_policy);
        let apply_loaded: Rc<dyn Fn(Result<Option<CachedArtistDetail>, String>)> =
            Rc::new(move |result| {
                let next = match result {
                    Ok(Some(next)) => next,
                    Ok(None) => {
                        shell.set_route_search(None);
                        apply_stack.set_visible_child_name("missing");
                        return;
                    }
                    Err(error) => {
                        warn!(%error, "failed to refresh Artist detail projection");
                        return;
                    }
                };
                let summary_counts = ArtistSummaryFacts::counts(&next);
                let CachedArtistDetail {
                    artist,
                    albums,
                    appears_on,
                    tracks,
                } = next;
                let favorite_tracks = favorite_artist_tracks(&tracks);
                let tracks = Arc::new(tracks);
                apply_header.replace(&shell, artist, tracks, summary_counts);
                {
                    let settings = shell.settings.current.borrow();
                    apply_external_link_policy
                        .replace((settings.private_mode, settings.external_site_links.clone()));
                }
                delta_favorite_projection.replace(favorite_tracks);
                delta_releases.replace(
                    albums,
                    appears_on,
                    !delta_favorite_projection.source_is_empty(),
                );
                set_artist_route_search(&shell, delta_releases.primary_search());
                apply_stack.set_visible_child_name("detail");
            });
        let refresh = mounted_artist_refresh(
            &library_query,
            &artist_id,
            Rc::downgrade(&apply_loaded),
            "mounted Artist detail",
        );
        let affected_by = Rc::new(artist_detail_affected);
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
            let applied_external_link_policy = Rc::clone(&applied_external_link_policy);
            Rc::new(move || {
                let external_link_policy = {
                    let settings = shell.settings.current.borrow();
                    (settings.private_mode, settings.external_site_links.clone())
                };
                if *applied_external_link_policy.borrow() != external_link_policy {
                    header.apply_external_link_settings(&shell);
                    applied_external_link_policy.replace(external_link_policy);
                }
                let track_settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::ArtistTracks);
                favorite_projection
                    .apply_library_list_settings(LibraryListKey::ArtistTracks, &track_settings);
                favorite_toolbar.apply(LibraryListKey::ArtistTracks, &track_settings);
                let album_settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::ArtistAlbums);
                releases.apply_library_list_settings(LibraryListKey::ArtistAlbums, &album_settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    pub(crate) fn artist_discography_view_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        artist_id: ArtistId,
        detail: Option<CachedArtistDetail>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            return MountedRoute::static_widget(self.placeholder_view(
                msgid("Discography"),
                "The selected cached artist was not found.",
            ));
        };
        let summary_facts = ArtistSummaryFacts::new(ArtistSummaryFacts::counts(&detail));
        let CachedArtistDetail {
            artist,
            albums,
            appears_on,
            tracks: _,
        } = detail;

        let wrapper = detail_route_wrapper(0);
        let header = self.artist_subroute_header(
            &artist,
            msgid("Discography"),
            &summary_facts.summary_text(),
        );
        let summary_facts_for_locale = summary_facts.clone();
        bind_label_text_with(&header.summary, move || {
            summary_facts_for_locale.summary_text()
        });
        let empty = self.placeholder_view(
            msgid("Discography"),
            "No cached albums are linked to this artist yet.",
        );
        let releases = ArtistReleaseProjections::new(
            self,
            library_query.clone(),
            ArtistReleaseRoutePreamble {
                header: header.widget(),
                favorite: None,
                favorite_present: false,
                empty,
            },
            albums,
            appears_on,
        );
        set_artist_route_search(self, releases.primary_search());
        wrapper.append(&releases.widget());
        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&wrapper, Some("detail"));
        route_stack.add_named(
            &self.placeholder_view(
                msgid("Discography"),
                "The selected cached artist was not found.",
            ),
            Some("missing"),
        );
        route_stack.set_visible_child_name("detail");

        let shell = Rc::clone(self);
        let apply_stack = route_stack.clone();
        let delta_releases = releases.clone();
        let apply_summary_facts = summary_facts;
        let apply_loaded: Rc<dyn Fn(Result<Option<CachedArtistDetail>, String>)> =
            Rc::new(move |result| {
                let next = match result {
                    Ok(Some(next)) => next,
                    Ok(None) => {
                        shell.set_route_search(None);
                        apply_stack.set_visible_child_name("missing");
                        return;
                    }
                    Err(error) => {
                        warn!(%error, "failed to refresh Artist discography projection");
                        return;
                    }
                };
                let summary_counts = ArtistSummaryFacts::counts(&next);
                let CachedArtistDetail {
                    artist,
                    albums,
                    appears_on,
                    tracks: _,
                } = next;
                apply_summary_facts.replace(summary_counts);
                header.replace(&artist, &apply_summary_facts.summary_text());
                delta_releases.replace(albums, appears_on, false);
                set_artist_route_search(&shell, delta_releases.primary_search());
                apply_stack.set_visible_child_name("detail");
            });
        let refresh = mounted_artist_refresh(
            &library_query,
            &artist_id,
            Rc::downgrade(&apply_loaded),
            "mounted Artist discography",
        );
        let affected_by = Rc::new(artist_detail_affected);
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
                    .library_list(LibraryListKey::ArtistAlbums);
                releases.apply_library_list_settings(LibraryListKey::ArtistAlbums, &settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    pub(crate) fn artist_tracks_view_from_loaded(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        artist_id: ArtistId,
        detail: Option<CachedArtistDetail>,
    ) -> MountedRoute {
        let Some(detail) = detail else {
            return MountedRoute::static_widget(
                self.placeholder_view("Tracks", "The selected cached artist was not found."),
            );
        };
        let summary_facts = ArtistSummaryFacts::new(ArtistSummaryFacts::counts(&detail));
        let CachedArtistDetail {
            artist,
            albums: _,
            appears_on: _,
            tracks,
        } = detail;

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_width_request(1);

        let header = self.artist_subroute_header(&artist, "Tracks", &summary_facts.summary_text());
        let summary_facts_for_locale = summary_facts.clone();
        bind_label_text_with(&header.summary, move || {
            summary_facts_for_locale.summary_text()
        });
        wrapper.append(&library_route_inset(header.widget()));

        let track_projection = self.searchable_track_collection(
            tracks,
            LibraryListKey::ArtistTracks,
            SearchableTrackOptions {
                on_visible_count_changed: None,
                source_descriptor: Some(PlayContextDescriptor::Artist {
                    artist_id: artist_id.clone(),
                    scope: ArtistTrackScope::AllCredits,
                    music_folder_id: selected_music_folder_id(self),
                }),
                favorites_only: false,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                fixed_layout: None,
            },
        );
        let track_section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        track_section.set_widget_name("artist-tracks");
        track_section.set_hexpand(true);
        track_section.set_halign(gtk::Align::Fill);
        track_section.set_vexpand(true);
        let track_toolbar = self
            .library_toolbar_projection(LibraryListKey::ArtistTracks, track_projection.search());
        track_section.append(&library_route_inset(track_toolbar.widget()));
        self.set_route_search(Some(track_projection.search()));
        track_section.append(&track_projection.scrolling_widget());
        let track_stack = gtk::Stack::new();
        track_stack.set_hexpand(true);
        track_stack.set_vexpand(true);
        track_stack.add_named(&track_section, Some("tracks"));
        track_stack.add_named(
            &library_route_inset(
                self.placeholder_view("Tracks", "No cached tracks are linked to this artist yet."),
            ),
            Some("empty"),
        );
        track_stack.set_visible_child_name(if track_projection.source_is_empty() {
            "empty"
        } else {
            "tracks"
        });
        wrapper.append(&track_stack);

        let route_stack = gtk::Stack::new();
        route_stack.set_hexpand(true);
        route_stack.set_vexpand(true);
        route_stack.add_named(&wrapper, Some("detail"));
        route_stack.add_named(
            &self.placeholder_view("Tracks", "The selected cached artist was not found."),
            Some("missing"),
        );
        route_stack.set_visible_child_name("detail");

        let apply_stack = route_stack.clone();
        let delta_track_projection = track_projection.clone();
        let apply_summary_facts = summary_facts;
        let apply_loaded: Rc<dyn Fn(Result<Option<CachedArtistDetail>, String>)> =
            Rc::new(move |result| {
                let next = match result {
                    Ok(Some(next)) => next,
                    Ok(None) => {
                        apply_stack.set_visible_child_name("missing");
                        return;
                    }
                    Err(error) => {
                        warn!(%error, "failed to refresh Artist tracks projection");
                        return;
                    }
                };
                let summary_counts = ArtistSummaryFacts::counts(&next);
                let CachedArtistDetail {
                    artist,
                    albums: _,
                    appears_on: _,
                    tracks,
                } = next;
                apply_summary_facts.replace(summary_counts);
                header.replace(&artist, &apply_summary_facts.summary_text());
                delta_track_projection.replace(tracks);
                track_stack.set_visible_child_name(if delta_track_projection.source_is_empty() {
                    "empty"
                } else {
                    "tracks"
                });
                apply_stack.set_visible_child_name("detail");
            });
        let refresh = mounted_artist_refresh(
            &library_query,
            &artist_id,
            Rc::downgrade(&apply_loaded),
            "mounted Artist tracks",
        );
        let affected_by = Rc::new(artist_detail_affected);
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
                    .library_list(LibraryListKey::ArtistTracks);
                track_projection
                    .apply_library_list_settings(LibraryListKey::ArtistTracks, &settings);
                track_toolbar.apply(LibraryListKey::ArtistTracks, &settings);
            })
        };
        MountedRoute::new(route_stack.upcast(), affected_by, apply_delta, resume)
    }

    fn artist_detail_header(
        self: &Rc<Self>,
        artist: Artist,
        tracks: Arc<Vec<Track>>,
        summary_facts: ArtistSummaryFacts,
    ) -> ArtistDetailHeaderProjection {
        let artist = Rc::new(RefCell::new(artist));
        let tracks = Rc::new(RefCell::new(tracks));
        let initial_artist = artist.borrow();
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let cover_size = detail_showcase_cover_size(content_width);
        let seed = stable_seed(initial_artist.id.as_str());

        let cover_fetch_size = cover_fetch_size_for_display(cover_size);
        let cover = detail_cover_projection(
            self,
            ArtworkBinding::artist(&initial_artist),
            seed,
            cover_size,
            cover_fetch_size,
            "artist-detail-cover",
        );
        let counts = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        counts.add_css_class("artist-count-row");
        counts.set_halign(gtk::Align::Start);
        let (albums, album_count) = artist_count_button_with_label(
            "rufin-route-albums-symbolic",
            &summary_facts.album_text(),
        );
        let album_count_facts = summary_facts.clone();
        bind_label_text_with(&album_count, move || album_count_facts.album_text());
        let shell = Rc::clone(self);
        let artist_id = initial_artist.id.clone();
        albums.connect_clicked(move |_| {
            shell.navigate(Route::ArtistDiscography(artist_id.clone()));
        });
        counts.append(&albums);
        let (tracks_button, track_count) = artist_count_button_with_label(
            "rufin-route-tracks-symbolic",
            &summary_facts.track_text(),
        );
        let track_count_facts = summary_facts.clone();
        bind_label_text_with(&track_count, move || track_count_facts.track_text());
        let shell = Rc::clone(self);
        let artist_id = initial_artist.id.clone();
        tracks_button.connect_clicked(move |_| {
            shell.navigate(Route::ArtistTracks(artist_id.clone()));
        });
        counts.append(&tracks_button);
        let text_stack = gtk::Box::new(gtk::Orientation::Vertical, 8);
        text_stack.set_hexpand(true);
        text_stack.set_halign(gtk::Align::Fill);
        text_stack.set_width_request(1);
        let kind_row = self.artist_detail_kind_row(Rc::clone(&artist));

        let title = fitted_detail_title_label(&initial_artist.name);

        let actions = detail_action_row();
        actions.add_css_class("artist-detail-actions");
        actions.set_halign(gtk::Align::Start);

        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.products.playback.queue.clone();
        let artist_id = initial_artist.id.clone();
        let play_tracks = Rc::clone(&tracks);
        play.connect_clicked(move |_| {
            let tracks = Arc::clone(&play_tracks.borrow());
            controller.play_artist_window(ArtistWindowPlayRequest {
                artist_id: artist_id.clone(),
                scope: ArtistTrackScope::AllCredits,
                total_items: tracks.len(),
                anchor_index: 0,
                track_at: Box::new(move |index| tracks.get(index).cloned()),
            });
        });
        actions.append(&play);

        let batch_tracks = Rc::clone(&tracks);
        append_track_query_batch_queue_actions(
            &actions,
            &self.products.playback.queue,
            Rc::new(move || batch_tracks.borrow().as_ref().clone()),
        );

        let favorite = favorite_icon_button("Favorite");
        configure_action_button(&favorite, ActionButtonVariant::DetailFavorite, None);
        set_favorite_button_active(&favorite, initial_artist.favorite);
        self.favorites
            .register_button(artist_favorite_key(&initial_artist.id), &favorite);
        let shell = Rc::clone(self);
        let artist_id = initial_artist.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            shell.set_favorite_with_feedback(
                FavoriteItemId::Artist(artist_id.clone()),
                favorite,
                Some(button),
            );
        });
        actions.append(&favorite);

        let external_links = gtk::Box::new(gtk::Orientation::Vertical, 0);
        if let Some(links) = artist_external_links(self, &initial_artist, &tracks.borrow()) {
            external_links.append(&links);
        }

        text_stack.append(&kind_row);
        text_stack.append(&title);
        text_stack.append(&counts);
        let root = media_detail_showcase(
            self,
            MediaDetailShowcase {
                route_class: "artist-detail-showcase",
                seed,
                initial_width: content_width,
                cover: cover.clone(),
                external_links: Some(external_links.clone().upcast()),
                external_links_class: None,
                text_stack: text_stack.upcast(),
                actions: actions.upcast(),
            },
        );
        drop(initial_artist);
        ArtistDetailHeaderProjection {
            root,
            title,
            album_count,
            track_count,
            cover,
            external_links,
            favorite,
            summary_facts,
            artist,
            tracks,
        }
    }

    fn artist_detail_kind_row(self: &Rc<Self>, artist: Rc<RefCell<Artist>>) -> gtk::Box {
        let kind = localized_label("Artist");
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
            controller.play_radio(RadioPlayRequest::now(RadioSeed::Artist(
                artist.borrow().clone(),
            )));
        });
        row.append(&radio);
        row
    }

    fn artist_subroute_header(
        self: &Rc<Self>,
        artist: &Artist,
        kind: &str,
        summary: &str,
    ) -> ArtistSubrouteHeaderProjection {
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let seed = stable_seed(artist.id.as_str());
        let header = gtk::Box::new(gtk::Orientation::Vertical, 8);
        header.add_css_class("detail-showcase");
        header.add_css_class("artist-detail-showcase");
        mark_tiny_detail_showcase(&header, content_width);
        add_album_seed_gradient_class(&header, seed);

        let kind = localized_label(kind);
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);

        let title = gtk::Label::new(Some(&artist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        fit_detail_text(&title, &artist.name);

        let summary_label = gtk::Label::new(Some(summary));
        summary_label.add_css_class("muted");
        summary_label.set_xalign(0.0);

        header.append(&kind);
        header.append(&title);
        header.append(&summary_label);
        let resize_header = header.clone();
        let frame = detail_showcase_frame_with_back(self, header.upcast());
        let allocated_width = Cell::new(content_width);
        let root = width_allocation_owner(&frame, move |width| {
            if width <= 1 || allocated_width.replace(width) == width {
                return;
            }
            mark_tiny_detail_showcase(&resize_header, width);
        });
        ArtistSubrouteHeaderProjection {
            root: root.upcast(),
            title,
            summary: summary_label,
        }
    }
}

fn set_artist_route_search(shell: &Shell, target: Option<ArtistRouteSearchTarget>) {
    match target {
        Some(target) => shell.set_route_search_with_focus(target.search, target.focus),
        None => shell.set_route_search(None),
    }
}

fn section_heading(title: &str) -> gtk::Widget {
    let heading = localized_label(title);
    heading.add_css_class("section-heading");
    heading.set_xalign(0.0);
    heading.upcast()
}

fn artist_summary_text(album_count: usize, appears_on_count: usize, track_count: u32) -> String {
    format!(
        "{} / {}",
        album_count_text((album_count + appears_on_count) as u64),
        track_count_text(track_count.into())
    )
}

fn artist_count_button_with_label(icon_name: &str, text: &str) -> (gtk::Button, gtk::Label) {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("artist-count-button");

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(ARTIST_COUNT_ICON_SIZE);
    icon.set_size_request(ARTIST_COUNT_ICON_SIZE, ARTIST_COUNT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    content.append(&icon);
    let label = gtk::Label::new(Some(text));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&label);
    button.set_child(Some(&content));
    (button, label)
}

fn favorite_artist_tracks(tracks: &[Track]) -> Vec<Track> {
    let mut favorites = tracks
        .iter()
        .filter(|track| track.favorite)
        .cloned()
        .collect::<Vec<_>>();
    favorites.sort_by(|left, right| {
        left.album
            .to_lowercase()
            .cmp(&right.album.to_lowercase())
            .then(left.disc_number.cmp(&right.disc_number))
            .then(left.track_number.cmp(&right.track_number))
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    favorites
}

#[cfg(test)]
mod tests {
    use super::artist_summary_text;

    #[test]
    fn artist_summary_merges_appears_on() {
        let summary = artist_summary_text(0, 2, 3);

        assert_eq!(summary, "2 albums / 3 tracks");
    }
}
