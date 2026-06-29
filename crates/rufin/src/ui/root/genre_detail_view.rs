use super::*;

impl Shell {
    pub(in crate::ui) fn genre_detail_view(
        self: &Rc<Self>,
        genre_id: domain::GenreId,
    ) -> gtk::Widget {
        let detail = self
            .genre_detail_from_memory(&genre_id)
            .or_else(|| {
                self.controller
                    .cached_genre_detail(&genre_id)
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                let library = self.state.library.borrow();
                let genre = library
                    .genres
                    .iter()
                    .find(|genre| genre.id.as_str() == genre_id.as_str())
                    .cloned()?;
                Some(CachedGenreDetail {
                    genre,
                    albums: Vec::new(),
                    tracks: Vec::new(),
                })
            });
        let Some(detail) = detail else {
            return self.placeholder_view("Genre", "The selected cached genre was not found.");
        };
        let seed = stable_seed(detail.genre.id.as_str());
        let mut summary_items = vec![(
            "rufin-route-tracks-symbolic",
            track_count_text(detail.genre.track_count.into()),
        )];
        if detail.genre.duration_seconds > 0 {
            summary_items.push((
                "appointment-soon-symbolic",
                format_duration_units(detail.genre.duration_seconds),
            ));
        }
        let cover_refs = if detail.genre.image_refs.is_empty() {
            grouped_cover_refs_for_items(&detail.albums, &detail.tracks)
        } else {
            detail.genre.image_refs.clone()
        };
        let mut genre = detail.genre;
        genre.image_refs = cover_refs;
        let artwork = crate::cover_art_policy::selected_genre_artwork(&genre);
        let kind_row = self.genre_detail_kind_row(&genre);
        let action_tracks = Rc::new(detail.tracks.clone());
        let track_selection: TrackTableSelectionHandle = Rc::new(RefCell::new(None));
        let actions = self.genre_detail_actions(
            genre_id.clone(),
            Rc::clone(&action_tracks),
            Rc::clone(&track_selection),
        );
        self.grouped_detail_view(GroupedDetailData {
            kind_row: Some(kind_row.upcast()),
            title: genre.name,
            artwork,
            seed,
            summary_items,
            actions: Some(actions.upcast()),
            selection_handle: Some(track_selection),
            tracks: detail.tracks,
            table_context: "genre-detail",
            source_descriptor: Some(PlaySourceDescriptor::GenreTracks {
                genre_id,
                selected_music_folder_id: selected_music_folder_id(self),
            }),
        })
    }

    fn genre_detail_kind_row(self: &Rc<Self>, genre: &Genre) -> gtk::Box {
        let kind = gtk::Label::new(Some(&tr("Genre")));
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
        let controller = self.controller.clone();
        let genre = genre.clone();
        radio.connect_clicked(move |_| {
            controller.play_genre_radio(genre.clone());
        });
        row.append(&radio);
        row
    }

    fn genre_detail_actions(
        self: &Rc<Self>,
        genre_id: domain::GenreId,
        tracks: Rc<Vec<Track>>,
        selection: TrackTableSelectionHandle,
    ) -> gtk::Box {
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);

        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.controller.clone();
        let shell = Rc::clone(self);
        let play_tracks = Rc::clone(&tracks);
        let play_selection = Rc::clone(&selection);
        play.connect_clicked(move |_| {
            if !play_tracks.is_empty() {
                let play_selection = Rc::clone(&play_selection);
                shell.arm_now_playing_selection(Rc::new(move |queue| {
                    let Some(entry) = queue_current_entry(queue) else {
                        return;
                    };
                    if let Some(selection) = play_selection.borrow().as_ref() {
                        selection.select_track_id(&entry.track_id);
                    }
                }));
            }
            controller.play_genre_tracks_window(genre_id.clone(), play_tracks.len(), 0, |index| {
                play_tracks.as_ref().get(index).cloned()
            });
        });
        actions.append(&play);

        append_track_batch_queue_actions(&actions, &self.controller, tracks);

        actions
    }

    fn genre_detail_from_memory(
        self: &Rc<Self>,
        genre_id: &domain::GenreId,
    ) -> Option<CachedGenreDetail> {
        let library = self.state.library.borrow();
        if library.cached_track_count > library.tracks.len() {
            return None;
        }
        let genre = library
            .genres
            .iter()
            .find(|genre| genre.id.as_str() == genre_id.as_str())
            .cloned()?;
        let tracks = library
            .tracks
            .iter()
            .filter(|track| track.genres.iter().any(|name| name == &genre.name))
            .cloned()
            .collect::<Vec<_>>();
        Some(CachedGenreDetail {
            genre,
            albums: Vec::new(),
            tracks,
        })
    }
}
