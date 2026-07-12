use super::*;

impl Shell {
    pub(in crate::ui) fn mood_detail_view(
        self: &Rc<Self>,
        mood_id: ::library::MoodId,
    ) -> gtk::Widget {
        let detail = self.controller.cached_mood_detail(&mood_id).ok().flatten();
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
        let mood = detail.mood;
        let artwork = CandidateSet::mood_slots(&mood);
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
            source_descriptor: Some(PlayContextDescriptor::Mood {
                mood_id,
                music_folder_id: selected_music_folder_id(self),
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
        mood_id: ::library::MoodId,
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
}
