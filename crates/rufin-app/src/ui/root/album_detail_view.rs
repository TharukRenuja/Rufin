use super::*;

const ALBUM_DETAIL_ROUTE_MARGIN: i32 = 32;

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

        let wrapper = detail_route_wrapper(0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
        content.set_margin_top(20);
        content.set_margin_bottom(36);
        content.set_margin_start(ALBUM_DETAIL_ROUTE_MARGIN);
        content.set_margin_end(ALBUM_DETAIL_ROUTE_MARGIN);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);

        let inner_content_width = detail_route_inner_width(self, ALBUM_DETAIL_ROUTE_MARGIN * 2);
        let cover_size = detail_showcase_cover_size(inner_content_width);
        let header = gtk::Box::new(
            gtk::Orientation::Vertical,
            detail_showcase_spacing(inner_content_width),
        );
        header.add_css_class("detail-showcase");
        header.add_css_class("album-detail-showcase");
        add_album_seed_gradient_class(&header, album.color_seed);
        self.prime_cached_cover(album.image_ref.as_ref(), DETAIL_COVER_SIZE, cover_size);
        let cover = self.cover_tile_for(
            album.image_ref.as_ref(),
            album.color_seed,
            cover_size,
            DETAIL_COVER_SIZE,
        );
        cover.add_css_class("detail-showcase-cover");
        cover.add_css_class("album-detail-cover");
        cover.set_halign(gtk::Align::Center);
        header.append(&cover);

        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_halign(gtk::Align::Center);
        let kind = gtk::Label::new(Some(&tr("Album")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.5);
        kind.set_halign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&album.title));
        title.add_css_class("detail-title");
        title.set_xalign(0.5);
        title.set_justify(gtk::Justification::Center);
        title.set_wrap(true);
        title.set_width_chars(1);
        title.set_max_width_chars(24);
        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("detail-artist");
        artist.set_xalign(0.5);
        artist.set_halign(gtk::Align::Center);
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
        facts.set_xalign(0.5);
        facts.set_halign(gtk::Align::Center);

        let actions = detail_action_row();
        actions.add_css_class("album-detail-actions");
        let play_album = detail_action_button("media-playback-start-symbolic", "Play");
        play_album.add_css_class("detail-showcase-play-button");
        let controller = self.controller.clone();
        let album_id_for_play = album.id.clone();
        let album_tracks = tracks.clone();
        let selected_folder_for_play = selected_music_folder_id(self);
        play_album.connect_clicked(move |_| {
            if let Some(activation) = album_play_activation(
                album_id_for_play.clone(),
                album_tracks.clone(),
                0,
                selected_folder_for_play.clone(),
            ) {
                controller.play_activation(activation);
            }
        });
        actions.append(&play_album);

        let play_next = detail_action_button(PLAY_NEXT_ICON, "Next");
        let controller = self.controller.clone();
        let next_tracks = tracks.clone();
        play_next.connect_clicked(move |_| {
            for track in next_tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        });
        actions.append(&play_next);

        let play_later = detail_action_button(PLAY_LATER_ICON, "Play Later");
        let controller = self.controller.clone();
        let later_tracks = tracks.clone();
        play_later.connect_clicked(move |_| controller.play_last(later_tracks.clone()));
        actions.append(&play_later);

        let favorite = favorite_icon_button("Favorite");
        favorite.add_css_class("detail-showcase-action-button");
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
        metadata.append(&facts);
        metadata.append(&artist);
        metadata.append(&actions);
        header.append(&metadata);
        content.append(&detail_showcase_frame(header.upcast()));

        let table = self.library_tracks_panel_with_source(
            tracks,
            LibraryListKey::AlbumDetailTracks,
            "album-detail",
            Some(PlaySourceDescriptor::Album {
                album_id: album.id.clone(),
                selected_music_folder_id: selected_music_folder_id(self),
            }),
            ALBUM_DETAIL_ROUTE_MARGIN * 2 + DETAIL_ROUTE_SCROLL_GUTTER,
        );
        content.append(&table);

        wrapper.append(&detail_route_scroller(self, content.upcast()));
        wrapper.upcast()
    }
}
