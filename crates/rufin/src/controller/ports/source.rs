use std::path::PathBuf;

use library::{MusicFolderId, SourceId};
use sources::{
    EditableSource, LibrarySourceSelection, SourceIdentity, SourceLocalAccessInput,
    SourceSettingsInput, SourceSetupInput,
};
use ui::runtime::source::SourcePort;

use crate::source_setup::{
    configure_source, editable_configured_source, local_source_identity, update_source,
};

use super::super::root::SourceCommands;

impl SourcePort for SourceCommands {
    fn configured_source(&self, source_id: &SourceId) -> Result<Option<EditableSource>, String> {
        SourceCommands::configured_source(self, source_id)
            .map(|saved| editable_configured_source(&saved))
            .transpose()
    }

    fn local_source_identity(&self) -> SourceIdentity {
        let mut identity = local_source_identity();
        identity.name.clear();
        identity
    }

    fn discover_servers(&self) {
        SourceCommands::discover_servers(self);
    }

    fn refresh_freshness(&self) {
        self.refresh_source_freshness();
    }

    fn configure_source(&self, input: SourceSetupInput) {
        configure_source(self, input);
    }

    fn update_source(&self, input: SourceSettingsInput) {
        update_source(self, input);
    }

    fn select_source(&self, selection: LibrarySourceSelection) {
        SourceCommands::select_source(self, selection);
    }

    fn add_local_library_folder(&self, path: PathBuf) {
        SourceCommands::add_local_library_folder(self, path);
    }

    fn remove_local_library_folder(&self, path: String) {
        SourceCommands::remove_local_library_folder(self, path);
    }

    fn resync_local_library(&self) {
        SourceCommands::resync_local_library(self);
    }

    fn resync_source(&self, source_id: SourceId) {
        self.resync_server(source_id);
    }

    fn save_local_access(&self, input: SourceLocalAccessInput) {
        SourceCommands::save_source_local_access(
            self,
            input.source_id,
            input.root_path,
            input.server_prefix,
            input.local_prefix,
        );
    }

    fn clear_local_access(&self, source_id: SourceId) {
        SourceCommands::clear_source_local_access(self, source_id);
    }

    fn clear_source_cache(&self, source_id: SourceId) {
        SourceCommands::clear_source_cache(self, source_id);
    }

    fn forget_source(&self, source_id: SourceId) {
        SourceCommands::forget_source(self, source_id);
    }

    fn set_selected_music_folder(&self, source_id: SourceId, folder_id: Option<MusicFolderId>) {
        SourceCommands::set_selected_music_folder(self, source_id, folder_id);
    }
}
