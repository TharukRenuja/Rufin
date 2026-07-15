use std::path::PathBuf;
use std::sync::Arc;

use library::{MusicFolderId, SourceId};
use sources::{
    EditableSource, LibrarySourceSelection, SourceIdentity, SourceLocalAccessInput,
    SourceSettingsInput, SourceSetupInput,
};

pub trait SourcePort: Send + Sync {
    fn configured_source(&self, source_id: &SourceId) -> Result<Option<EditableSource>, String>;
    fn local_source_identity(&self) -> SourceIdentity;
    fn discover_servers(&self);
    fn refresh_freshness(&self);
    fn configure_source(&self, input: SourceSetupInput);
    fn update_source(&self, input: SourceSettingsInput);
    fn select_source(&self, selection: LibrarySourceSelection);
    fn add_local_library_folder(&self, path: PathBuf);
    fn remove_local_library_folder(&self, path: String);
    fn resync_local_library(&self);
    fn resync_source(&self, source_id: SourceId);
    fn save_local_access(&self, input: SourceLocalAccessInput);
    fn clear_local_access(&self, source_id: SourceId);
    fn clear_source_cache(&self, source_id: SourceId);
    fn forget_source(&self, source_id: SourceId);
    fn set_selected_music_folder(&self, source_id: SourceId, folder_id: Option<MusicFolderId>);
}

pub type SourceHandle = Arc<dyn SourcePort>;
