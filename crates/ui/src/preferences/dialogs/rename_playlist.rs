use std::rc::Rc;

use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use ::library::{PlaylistEdit, PlaylistId, SmartPlaylist};
use adw::prelude::*;
use localization::tr;

impl Shell {
    pub(crate) fn rename_playlist_dialog(
        self: &Rc<Self>,
        playlist_id: PlaylistId,
        current_name: String,
    ) {
        let Some(source) = self.selected_source_operations() else {
            return;
        };
        self.rename_playlist_dialog_inner(current_name, move |name| {
            source.edit_playlist(PlaylistEdit::Rename {
                playlist_id: playlist_id.clone(),
                name,
            });
        });
    }

    pub(crate) fn rename_smart_playlist_dialog(self: &Rc<Self>, playlist: SmartPlaylist) {
        let Some(source) = self.selected_source_operations() else {
            return;
        };
        let playlist_id = playlist.id.clone();
        let definition = playlist.definition.clone();
        self.rename_playlist_dialog_inner(playlist.name, move |name| {
            source.update_smart_playlist(playlist_id.clone(), name, definition.clone());
        });
    }

    fn rename_playlist_dialog_inner(
        self: &Rc<Self>,
        current_name: String,
        rename: impl Fn(String) + 'static,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Rename Playlist"))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("rename", &tr("Rename"));
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        let entry = gtk::Entry::new();
        entry.set_text(&current_name);
        dialog.set_extra_child(Some(&entry));
        dialog.connect_response(None, move |_, response| {
            if response == "rename" {
                let name = entry.text().trim().to_string();
                if !name.is_empty() {
                    rename(name);
                }
            }
        });
        present_light_dismiss_dialog(&dialog, &self.chrome.window);
    }
}
