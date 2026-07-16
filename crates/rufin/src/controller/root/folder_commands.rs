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
        let (saved, selected_music_folder_id) = self
            .store
            .with_store_fast(|store| {
                let Some(saved) = store.active_source()? else {
                    return Ok(None);
                };
                let selected_music_folder_id = store.selected_music_folder_id(&saved.source_id)?;
                Ok(Some((saved, selected_music_folder_id)))
            })?
            .ok_or_else(|| "No active server.".to_string())?;
        let active = selected_active_source(&self.active_source, &saved.source_id)?;
        let browser = active
            .folders
            .as_ref()
            .ok_or_else(|| "Folder browsing is not supported by the active source.".to_string())?;
        self.runtime
            .block_on(browser.folder(path.last(), selected_music_folder_id.as_ref()))
            .map_err(|error| error.to_string())
    }
}
