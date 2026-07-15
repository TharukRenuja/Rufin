use super::*;

impl SourceCommands {
    pub fn set_selected_music_folder(&self, source_id: SourceId, folder_id: Option<MusicFolderId>) {
        let store = self.store.clone();
        let source_presentation = self.source_events.presentation.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.set_selected_music_folder_id(&source_id, folder_id.as_ref())
            }) {
                warn!(%error, %source_id, "failed to select music folder");
                return;
            }
            match load_source_presentation(&store) {
                Ok(presentation) => {
                    let _sent = source_presentation.try_send(presentation);
                }
                Err(error) => {
                    warn!(%error, "failed to reload source presentation after folder selection");
                }
            }
        });
    }
}

impl LibraryCommands {
    pub(crate) fn folder_for_active(&self, path: &[FolderId]) -> Result<FolderDetail, String> {
        load_folder_detail(&self.store, &self.runtime, &self.active_source, path)
    }
}
