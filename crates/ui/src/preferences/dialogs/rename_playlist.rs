use std::rc::Rc;

use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use ::library::PlaylistId;
use adw::prelude::*;
use localization::tr;

impl Shell {
    pub(crate) fn rename_playlist_dialog(
        self: &Rc<Self>,
        playlist_id: PlaylistId,
        current_name: String,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Rename Playlist"))
            .body(tr("Enter a new playlist name."))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("rename", &tr("Rename"));
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        let entry = gtk::Entry::new();
        entry.set_text(&current_name);
        dialog.set_extra_child(Some(&entry));
        let library = self.products.library.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "rename" {
                let name = entry.text().trim().to_string();
                if !name.is_empty() {
                    library.rename_playlist(playlist_id.clone(), name);
                }
            }
        });
        present_light_dismiss_dialog(&dialog, &self.chrome.window);
    }
}
