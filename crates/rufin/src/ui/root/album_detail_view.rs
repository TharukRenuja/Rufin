use super::*;

const ALBUM_DETAIL_ROUTE_INSET: i32 = PRIMARY_ROUTE_MARGIN_START + PRIMARY_ROUTE_MARGIN_END;

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
            let active_server_id = self
                .state
                .library
                .borrow()
                .server
                .as_ref()
                .map(|server| server.id.to_string());
            let queue_server_id = self
                .state
                .queue
                .borrow()
                .as_ref()
                .map(|queue| queue.server_id.to_string());
            let player_server_id = self
                .state
                .player
                .borrow()
                .current_server_id
                .as_ref()
                .map(ToString::to_string);
            warn!(
                album_id = album_id.as_str(),
                active_server_id, queue_server_id, player_server_id, "cached album route missing"
            );
            return self.placeholder_view("Album", "The selected cached album was not found.");
        };

        let wrapper = detail_route_wrapper(0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
        content.set_margin_top(ROUTE_TOP_MARGIN);
        content.set_margin_bottom(36);
        content.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        content.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);

        let inner_content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let cover_size = detail_showcase_cover_size(inner_content_width);
        let external_links = album_external_links(self, &album);
        let cover_fetch_size = cover_fetch_size_for_display(cover_size);
        let cover = detail_cover_button(
            self,
            album.image_ref.as_ref(),
            album.color_seed,
            cover_size,
            cover_fetch_size,
            "album-detail-cover",
        );
        let facts = detail_summary_row(&[
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
        let track_selection: TrackTableSelectionHandle = Rc::new(RefCell::new(None));
        let text_stack = gtk::Box::new(gtk::Orientation::Vertical, 8);
        text_stack.set_hexpand(true);
        text_stack.set_halign(gtk::Align::Fill);
        text_stack.set_width_request(1);
        let kind = gtk::Label::new(Some(&tr(album_release_kind_label(&album))));
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
        let controller = self.controller.clone();
        let album_for_radio = album.clone();
        radio.connect_clicked(move |_| {
            controller.play_album_radio(album_for_radio.clone());
        });
        kind_row.append(&radio);
        for genre_name in album
            .genres
            .iter()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
        {
            let button = detail_genre_pill_button(genre_name);
            if let Some(genre_id) = self.album_genre_id(genre_name) {
                let shell = Rc::clone(self);
                button
                    .connect_clicked(move |_| shell.navigate(Route::GenreDetail(genre_id.clone())));
            } else {
                button.set_sensitive(false);
            }
            kind_row.append(&button);
        }
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
        if let Some(route) = album_artist_route(&album) {
            artist.set_cursor_from_name(Some("pointer"));
            add_dynamic_link_hover(artist.upcast_ref(), &artist);
            let shell = Rc::clone(self);
            add_label_click(&artist, move || shell.navigate(route.clone()));
        }
        text_stack.append(&kind_row);
        text_stack.append(&title);
        text_stack.append(&artist);
        text_stack.append(&facts);
        let actions = detail_action_row();
        actions.add_css_class("album-detail-actions");
        actions.set_halign(gtk::Align::Start);
        let play_album = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.controller.clone();
        let shell = Rc::clone(self);
        let album_id_for_play = album.id.clone();
        let album_tracks = tracks.clone();
        let play_selection = Rc::clone(&track_selection);
        play_album.connect_clicked(move |_| {
            let play_selection = Rc::clone(&play_selection);
            shell.arm_now_playing_selection(Rc::new(move |queue| {
                let Some(entry) = queue_current_entry(queue) else {
                    return;
                };
                if let Some(selection) = play_selection.borrow().as_ref() {
                    selection.select_track_id(&entry.track_id);
                }
            }));
            controller.play_album_tracks(album_id_for_play.clone(), album_tracks.clone(), 0, true);
        });
        actions.append(&play_album);

        append_track_batch_queue_actions(&actions, &self.controller, Rc::new(tracks.clone()));

        let favorite = favorite_icon_button("Favorite");
        configure_action_button(&favorite, ActionButtonVariant::DetailFavorite, None);
        set_favorite_button_active(&favorite, album.favorite);
        self.register_favorite_button(album_favorite_key(&album.id), &favorite);
        let shell = Rc::clone(self);
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            shell.set_favorite_with_feedback(
                FavoriteItemId::Album(album_id.clone()),
                favorite,
                Some(button),
            );
        });
        actions.append(&favorite);

        let showcase = media_detail_showcase(
            self,
            MediaDetailShowcase {
                route_class: "album-detail-showcase",
                seed: album.color_seed,
                content_width: inner_content_width,
                cover_size,
                cover: cover.upcast(),
                external_links,
                external_links_class: Some("album-detail-link-stack"),
                text_stack: text_stack.upcast(),
                actions: actions.upcast(),
            },
        );
        content.append(&showcase);

        let table = self.library_tracks_panel_with_source_selection(
            tracks,
            LibraryListKey::AlbumDetailTracks,
            "album-detail",
            Some(PlaySourceDescriptor::Album {
                album_id: album.id.clone(),
                selected_music_folder_id: selected_music_folder_id(self),
            }),
            ALBUM_DETAIL_ROUTE_INSET,
            track_selection,
        );
        content.append(&table);

        wrapper.append(&detail_route_scroller(self, content.upcast()));
        wrapper.upcast()
    }

    fn album_genre_id(&self, name: &str) -> Option<domain::GenreId> {
        let library = self.state.library.borrow();
        if let Some(genre) = library
            .genres
            .iter()
            .find(|genre| genre.name.eq_ignore_ascii_case(name))
        {
            return Some(genre.id.clone());
        }
        drop(library);

        self.controller
            .cached_genres_page_matching(name, 0, 8)
            .ok()?
            .items
            .into_iter()
            .find(|genre| genre.name.eq_ignore_ascii_case(name))
            .map(|genre| genre.id)
    }
}
