use super::*;

impl Shell {
    pub(in crate::ui) fn search_view(
        self: &Rc<Self>,
        query: &str,
        library: LibrarySnapshot,
    ) -> gtk::Widget {
        let wrapper = detail_route_wrapper(0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(24);
        content.set_margin_bottom(28);
        content.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        content.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);
        content.set_vexpand(true);

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
            content.append(&self.home_album_section(&section));
        }

        if has_tracks {
            content.append(&self.library_tracks_panel_with_source(
                library.search.tracks,
                LibraryListKey::Tracks,
                "search",
                Some(PlaySourceDescriptor::SearchResults {
                    query: query.to_string(),
                    selected_music_folder_id: selected_music_folder_id(self),
                }),
                PRIMARY_ROUTE_MARGIN_START + PRIMARY_ROUTE_MARGIN_END + DETAIL_ROUTE_SCROLL_GUTTER,
            ));
        } else if !has_albums && !has_artists && !has_playlists {
            content.append(&self.route_empty_view("No cached results found."));
        }

        wrapper.append(&detail_route_scroller(self, content.upcast()));
        wrapper.upcast()
    }
}
