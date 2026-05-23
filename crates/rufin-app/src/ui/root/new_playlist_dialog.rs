impl Shell {
    fn new_playlist_dialog(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("New Playlist"))
            .body(tr(
                "Create a playlist. If a track is playing, it will be added.",
            ))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("create", &tr("Create"));
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(&tr("Playlist name")));
        dialog.set_extra_child(Some(&entry));
        let controller = self.controller.clone();
        let current_track = self
            .state
            .player
            .borrow()
            .current
            .as_ref()
            .and_then(|entry| {
                self.state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .find(|track| track.id == entry.track_id)
                    .cloned()
            });
        dialog.connect_response(None, move |_, response| {
            if response == "create" {
                let name = entry.text().trim().to_string();
                if !name.is_empty() {
                    controller.create_playlist(name, current_track.clone().into_iter().collect());
                }
            }
        });
        dialog.present(Some(&self.window));
    }
}
