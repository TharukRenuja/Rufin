use super::*;

impl Shell {
    pub(in crate::ui) fn mood_detail_view(self: &Rc<Self>, mood_id: domain::MoodId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_mood_detail(&mood_id)
            .ok()
            .flatten()
            .or_else(|| self.mood_detail_from_memory(&mood_id));
        let Some(detail) = detail else {
            return self.placeholder_view(
                "Mood",
                "Files need Mood/BPM tags written on them. Not supported for Jellyfin",
            );
        };
        let seed = stable_seed(detail.mood.id.as_str());
        let mut summary_items = vec![(
            "rufin-route-tracks-symbolic",
            track_count_text(detail.mood.track_count.into()),
        )];
        if detail.mood.duration_seconds > 0 {
            summary_items.push((
                "appointment-soon-symbolic",
                format_duration_units(detail.mood.duration_seconds),
            ));
        }
        let cover_refs = if detail.mood.image_refs.is_empty() {
            grouped_cover_refs_for_items(&detail.albums, &detail.tracks)
        } else {
            detail.mood.image_refs.clone()
        };
        let mut mood = detail.mood;
        mood.image_refs = cover_refs;
        let artwork = crate::cover_art_policy::selected_mood_artwork(&mood);
        let kind_row = self.mood_detail_kind_row();
        let action_tracks = Rc::new(detail.tracks.clone());
        let track_selection: TrackTableSelectionHandle = Rc::new(RefCell::new(None));
        let actions = self.mood_detail_actions(mood_id.clone(), Rc::clone(&action_tracks));
        self.grouped_detail_view(GroupedDetailData {
            kind_row: Some(kind_row.upcast()),
            title: mood.name,
            artwork,
            seed,
            summary_items,
            actions: Some(actions.upcast()),
            selection_handle: Some(track_selection),
            tracks: detail.tracks,
            table_context: "mood-detail",
            source_descriptor: Some(PlaySourceDescriptor::MoodTracks {
                mood_id,
                selected_music_folder_id: selected_music_folder_id(self),
            }),
        })
    }

    fn mood_detail_kind_row(self: &Rc<Self>) -> gtk::Box {
        let kind = gtk::Label::new(Some(&tr("Mood")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        kind.set_halign(gtk::Align::Start);
        kind.set_valign(gtk::Align::Center);
        kind.set_margin_end(6);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.add_css_class("album-detail-kind-row");
        row.set_valign(gtk::Align::Center);
        row.set_halign(gtk::Align::Start);
        row.append(&kind);
        row
    }

    fn mood_detail_actions(
        self: &Rc<Self>,
        mood_id: domain::MoodId,
        tracks: Rc<Vec<Track>>,
    ) -> gtk::Box {
        let actions = detail_action_row();
        actions.set_halign(gtk::Align::Start);

        let play = detail_primary_action_button(PLAY_ICON, "Play");
        let controller = self.controller.clone();
        let play_tracks = Rc::clone(&tracks);
        play.connect_clicked(move |_| {
            controller.play_mood_tracks_window(mood_id.clone(), play_tracks.len(), 0, |index| {
                play_tracks.as_ref().get(index).cloned()
            });
        });
        actions.append(&play);

        append_track_batch_queue_actions(&actions, &self.controller, tracks);

        actions
    }

    fn mood_detail_from_memory(
        self: &Rc<Self>,
        mood_id: &domain::MoodId,
    ) -> Option<CachedMoodDetail> {
        let library = self.state.library.borrow();
        if library.cached_track_count > library.tracks.len() {
            return None;
        }
        let tracks = library
            .tracks
            .iter()
            .filter(|track| track.moods.iter().any(|name| name == mood_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if tracks.is_empty() {
            return None;
        }
        let duration_seconds = tracks
            .iter()
            .map(|track| track.duration_seconds)
            .sum::<u32>();
        Some(CachedMoodDetail {
            mood: Mood {
                id: mood_id.clone(),
                name: mood_id.as_str().to_string(),
                track_count: tracks.len().min(u32::MAX as usize) as u32,
                duration_seconds,
                image_refs: grouped_cover_refs_for_items(&[], &tracks),
                image_ref: None,
            },
            albums: Vec::new(),
            tracks,
        })
    }
}
