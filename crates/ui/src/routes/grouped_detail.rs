use std::rc::Rc;

use ::library::{Track, play_context::PlayContextDescriptor};
use adw::prelude::*;
use artwork::ArtworkBinding;

use crate::LibraryListKey;
use crate::shell::Shell;
use crate::shell::cover::{CoverGroupProjection, GRID_COVER_SIZE};

use super::collections::library_route_inset;
use super::detail_showcase::{
    CollectionDetailShowcase, DetailSummaryProjection, collection_detail_showcase,
};
use super::playlist_detail::playlist_cover_size;
use super::route_layout::{
    PRIMARY_ROUTE_HORIZONTAL_INSET, PRIMARY_ROUTE_MARGIN_START, ROUTE_TOP_MARGIN,
    detail_route_inner_width,
};
use super::route_shell::LibraryToolbarProjection;
use super::routes::{SearchableTrackOptions, TrackListProjection};

pub(crate) const GROUPED_DETAIL_COVER_FETCH_SIZE: u32 = GRID_COVER_SIZE;

pub(crate) struct GroupedDetailData {
    pub(super) key: LibraryListKey,
    pub(super) kind_row: Option<gtk::Widget>,
    pub(super) title: String,
    pub(super) artwork: Vec<ArtworkBinding>,
    pub(super) seed: u32,
    pub(super) summary_items: Vec<(&'static str, String)>,
    pub(super) actions: Option<gtk::Widget>,
    pub(super) tracks: Vec<Track>,
    pub(super) table_context: &'static str,
    pub(super) source_descriptor: Option<PlayContextDescriptor>,
}

#[derive(Clone)]
pub(crate) struct GroupedDetailView {
    root: gtk::Widget,
    title: gtk::Label,
    summary: DetailSummaryProjection,
    cover: CoverGroupProjection,
    track_stack: gtk::Stack,
    tracks: TrackListProjection,
    toolbar: LibraryToolbarProjection,
}

impl GroupedDetailView {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    pub(crate) fn replace(
        &self,
        shell: &Rc<Shell>,
        title: &str,
        summary_items: &[(&str, String)],
        artwork: &[ArtworkBinding],
        seed: u32,
        tracks: Vec<Track>,
    ) {
        self.title.set_text(title);
        self.summary.replace(summary_items);
        self.cover.replace(shell, artwork, seed);
        self.tracks.replace(tracks);
        self.track_stack
            .set_visible_child_name(if self.tracks.source_is_empty() {
                "empty"
            } else {
                "tracks"
            });
    }

    pub(crate) fn bind_summary_text_with(&self, index: usize, text: impl Fn() -> String + 'static) {
        self.summary.bind_text_with(index, text);
    }

    pub(crate) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &crate::LibraryListSettings,
    ) {
        self.tracks.apply_library_list_settings(key, settings);
        self.toolbar.apply(key, settings);
    }
}

impl Shell {
    pub(crate) fn grouped_detail_view(
        self: &Rc<Self>,
        data: GroupedDetailData,
    ) -> GroupedDetailView {
        let GroupedDetailData {
            key,
            kind_row,
            title,
            artwork,
            seed,
            summary_items,
            actions,
            tracks,
            table_context,
            source_descriptor,
        } = data;
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);

        let cover = self.cover_group_projection_for_artwork(
            &artwork,
            seed,
            cover_size,
            GROUPED_DETAIL_COVER_FETCH_SIZE,
        );
        let title_label = gtk::Label::new(Some(&title));
        title_label.add_css_class("detail-title");
        title_label.set_xalign(0.0);
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        let mut metadata = Vec::new();
        if let Some(kind_row) = kind_row {
            metadata.push(kind_row);
        }
        metadata.push(title_label.clone().upcast());
        let summary = DetailSummaryProjection::new(&summary_items);
        metadata.push(summary.widget());
        if let Some(actions) = actions {
            metadata.push(actions);
        }
        let showcase = collection_detail_showcase(
            self,
            CollectionDetailShowcase {
                seed,
                initial_width: content_width,
                compact_spacing: 22,
                wide_spacing: 22,
                cover: cover.clone(),
                metadata,
            },
        );
        wrapper.append(&library_route_inset(showcase));

        let track_projection = self.searchable_track_collection(
            tracks,
            key,
            SearchableTrackOptions {
                on_visible_count_changed: None,
                source_descriptor,
                favorites_only: false,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                fixed_layout: None,
            },
        );
        let track_section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        track_section.set_widget_name(table_context);
        track_section.set_hexpand(true);
        track_section.set_halign(gtk::Align::Fill);
        track_section.set_vexpand(true);
        let toolbar = self.library_toolbar_projection(key, track_projection.search());
        track_section.append(&library_route_inset(toolbar.widget()));
        self.set_route_search(Some(track_projection.search()));
        track_section.append(&track_projection.scrolling_widget());

        let track_stack = gtk::Stack::new();
        track_stack.set_hexpand(true);
        track_stack.set_vexpand(true);
        track_stack.add_named(
            &library_route_inset(
                self.placeholder_view("Tracks", "No cached tracks are linked here yet."),
            ),
            Some("empty"),
        );
        track_stack.add_named(&track_section, Some("tracks"));
        track_stack.set_visible_child_name(if track_projection.source_is_empty() {
            "empty"
        } else {
            "tracks"
        });
        wrapper.append(&track_stack);

        GroupedDetailView {
            root: wrapper.upcast(),
            title: title_label,
            summary,
            cover,
            track_stack,
            tracks: track_projection,
            toolbar,
        }
    }
}
