use super::*;
use crate::ui::root::playlist_detail_view::playlist_route_margin;

pub(in crate::ui) const GROUPED_DETAIL_COVER_FETCH_SIZE: u32 = GRID_COVER_SIZE;

impl Shell {
    pub(in crate::ui) fn grouped_detail_view(
        self: &Rc<Self>,
        data: GroupedDetailData,
    ) -> gtk::Widget {
        let GroupedDetailData {
            title,
            image_ref,
            cover_refs,
            seed,
            summary,
            tracks,
            table_context,
            source_descriptor,
        } = data;
        let content_width = route_content_width(self);
        let route_margin = playlist_route_margin(content_width);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        wrapper.set_vexpand(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        header.add_css_class("playlist-detail-showcase");
        add_album_seed_gradient_class(&header, seed);
        header.set_margin_start(route_margin);
        header.set_margin_end(route_margin);
        header.set_hexpand(true);
        header.set_halign(gtk::Align::Fill);
        header.set_width_request(1);
        header.append(&self.cover_group_tile_for(
            cover_refs,
            image_ref.as_ref(),
            seed,
            160,
            GROUPED_DETAIL_COVER_FETCH_SIZE,
        ));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        metadata.set_hexpand(true);
        metadata.set_width_request(1);
        let title_label = gtk::Label::new(Some(&title));
        title_label.add_css_class("detail-title");
        title_label.set_xalign(0.0);
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        let summary_label = gtk::Label::new(Some(&summary));
        summary_label.add_css_class("muted");
        summary_label.set_xalign(0.0);
        metadata.append(&title_label);
        metadata.append(&summary_label);
        header.append(&metadata);
        wrapper.append(&header);

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
