use super::*;
use crate::ui::root::playlist_detail_view::{playlist_cover_size, playlist_route_margin};

pub(in crate::ui) const GROUPED_DETAIL_COVER_FETCH_SIZE: u32 = GRID_COVER_SIZE;

impl Shell {
    pub(in crate::ui) fn grouped_detail_view(
        self: &Rc<Self>,
        data: GroupedDetailData,
    ) -> gtk::Widget {
        let GroupedDetailData {
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
        let content_width = route_content_width(self);
        let route_margin = playlist_route_margin(content_width);
        let cover_size = playlist_cover_size(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(36);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);

        let cover = self.cover_group_tile_for_artwork(
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
        metadata.push(title_label.upcast());
        metadata.push(detail_summary_row(&summary_items).upcast());
        if let Some(actions) = actions {
            metadata.push(actions);
        }
        let showcase = collection_detail_showcase(
            self,
            CollectionDetailShowcase {
                seed,
                content_width,
                orientation: gtk::Orientation::Horizontal,
                spacing: 22,
                cover,
                metadata,
            },
        );
        wrapper.append(&showcase);

        if tracks.is_empty() {
            let placeholder =
                self.placeholder_view("Tracks", "No cached tracks are linked here yet.");
            placeholder.set_margin_start(route_margin);
            placeholder.set_margin_end(route_margin);
            wrapper.append(&placeholder);
        } else {
            let key = if table_context == "genre-detail" {
                LibraryListKey::GenreTracks
            } else {
                LibraryListKey::Tracks
            };
            wrapper.append(&self.library_tracks_scrolling_panel(
                tracks,
                key,
                table_context,
                route_margin,
                source_descriptor,
            ));
        }
        wrapper.upcast()
    }
}
