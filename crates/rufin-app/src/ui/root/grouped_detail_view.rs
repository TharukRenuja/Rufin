use super::*;

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
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        header.append(&self.cover_group_tile_for(
            cover_refs,
            image_ref.as_ref(),
            seed,
            160,
            DETAIL_COVER_SIZE,
        ));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let title_label = gtk::Label::new(Some(&title));
        title_label.add_css_class("detail-title");
        title_label.set_xalign(0.0);
        title_label.set_wrap(true);
        let summary_label = gtk::Label::new(Some(&summary));
        summary_label.add_css_class("muted");
        summary_label.set_xalign(0.0);
        metadata.append(&title_label);
        metadata.append(&summary_label);
        header.append(&metadata);
        wrapper.append(&header);

        if tracks.is_empty() {
            wrapper
                .append(&self.placeholder_view("Tracks", "No cached tracks are linked here yet."));
        } else {
            let key = if table_context == "genre-detail" {
                LibraryListKey::GenreTracks
            } else {
                LibraryListKey::Tracks
            };
            wrapper.append(&self.library_tracks_panel_with_source(
                tracks,
                key,
                table_context,
                source_descriptor,
            ));
        }
        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }
}
