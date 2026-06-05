use super::*;

impl Shell {
    pub(in crate::ui) fn search_view(
        self: &Rc<Self>,
        query: &str,
        library: LibrarySnapshot,
    ) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        mark_route_scroll_owner(&scroller);
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);

        let has_albums = !library.search.albums.is_empty();
        let has_tracks = !library.search.tracks.is_empty();
        let has_artists = !library.search.artists.is_empty();
        let has_playlists = !library.search.playlists.is_empty();
        let albums = library.search.albums;
        if !albums.is_empty() {
            let section = HomeSection {
                kind: rufin_core::HomeSectionKind::Explore,
                albums,
                tracks: Vec::new(),
            };
            wrapper.append(&self.home_album_section(&section));
        }

        if has_tracks {
            wrapper.append(&self.library_tracks_panel_with_source(
                library.search.tracks,
                LibraryListKey::Tracks,
                "search",
                Some(PlaySourceDescriptor::SearchResults {
                    query: query.to_string(),
                    selected_music_folder_id: selected_music_folder_id(self),
                }),
            ));
        } else if !has_albums && !has_artists && !has_playlists {
            wrapper.append(&self.route_empty_view("No cached results found."));
        }

        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }
}
