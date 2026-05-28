use super::*;

impl Shell {
    pub(in crate::ui) fn album_detail_view(self: &Rc<Self>, album_id: AlbumId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_album_detail(&album_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let album = library
                    .albums
                    .iter()
                    .find(|album| album.id.as_str() == album_id.as_str())
                    .cloned()?;
                let tracks = library
                    .tracks
                    .iter()
                    .filter(|track| track.album_id.as_str() == album_id.as_str())
                    .cloned()
                    .collect::<Vec<_>>();
                Some((album, tracks))
            });
        let Some((album, tracks)) = detail else {
            return self.placeholder_view("Album", "The selected cached album was not found.");
        };

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
        content.add_css_class("route-content");
        content.set_margin_top(20);
        content.set_margin_bottom(36);
        content.set_margin_start(32);
        content.set_margin_end(32);

        let content_width = route_content_width(self);
        let compact = content_width < 760;
        let cover_size = if compact { 164 } else { 204 };
        let header_orientation = if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        };
        let header = gtk::Box::new(header_orientation, if compact { 16 } else { 24 });
        header.add_css_class("album-detail-showcase");
        add_album_seed_gradient_class(&header, album.color_seed);
        header.set_hexpand(true);
        self.prime_cover_ref_from_cache_now(
            album.image_ref.as_ref(),
            DETAIL_COVER_SIZE,
            cover_size,
        );
        let cover = self.cover_tile_for(
            album.image_ref.as_ref(),
            album.color_seed,
            cover_size,
            DETAIL_COVER_SIZE,
        );
        cover.add_css_class("album-detail-cover");
        header.append(&cover);

        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        metadata.set_hexpand(true);
        let kind = gtk::Label::new(Some(&tr("Album")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        let title = gtk::Label::new(Some(&album.title));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("detail-artist");
        artist.set_xalign(0.0);
        artist.set_halign(gtk::Align::Start);
        artist.set_cursor_from_name(Some("pointer"));
        add_dynamic_link_hover(artist.upcast_ref(), &artist);
        if let Some(artist_id) = album.artist_id.clone() {
            let shell = Rc::clone(self);
            add_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        } else if !album.artist.trim().is_empty() {
            let shell = Rc::clone(self);
            let artist_name = album.artist.clone();
            add_label_click(&artist, move || {
                shell.navigate(Route::Search {
                    query: artist_name.clone(),
                    kind: SearchKind::Artists,
                });
            });
        }
        let facts = gtk::Label::new(Some(&format!(
            "{} • {} {} • {}",
            album.year,
            album.track_count,
            tr("tracks"),
            format_duration(album.duration_seconds)
        )));
        facts.add_css_class("muted");
        facts.set_xalign(0.0);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("album-detail-actions");
        let play_album = icon_button("media-playback-start-symbolic", "Play");
        play_album.add_css_class("album-detail-action-button");
        play_album.add_css_class("album-detail-play-button");
        let controller = self.controller.clone();
        let album_tracks = tracks.clone();
        play_album.connect_clicked(move |_| controller.play_tracks_now(album_tracks.clone()));
        actions.append(&play_album);

        let play_next = icon_button(PLAY_NEXT_ICON, "Play next");
        play_next.add_css_class("album-detail-action-button");
        let controller = self.controller.clone();
        let next_tracks = tracks.clone();
        play_next.connect_clicked(move |_| {
            for track in next_tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        });
        actions.append(&play_next);

        let favorite = favorite_icon_button("Favorite");
        favorite.add_css_class("album-detail-action-button");
        set_favorite_button_active(&favorite, album.favorite);
        self.register_favorite_button(album_favorite_key(&album.id), &favorite);
        let controller = self.controller.clone();
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_album_favorite(album_id.clone(), !favorite_button_is_active(button));
        });
        actions.append(&favorite);

        metadata.append(&kind);
        metadata.append(&title);
        metadata.append(&artist);
        metadata.append(&actions);
        metadata.append(&facts);
        header.append(&metadata);
        content.append(&header);

        let table =
            self.library_tracks_panel(tracks, LibraryListKey::AlbumDetailTracks, "album-detail");
        content.append(&table);

        scroller.set_child(Some(&content));
        scroller.upcast()
    }
}
