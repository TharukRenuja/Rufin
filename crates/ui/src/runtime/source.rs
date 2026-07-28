//! User intent and presentation for configured music sources.
//!
//! Provider configuration and lifecycle policy stay in Rufin and Sources. UI
//! sees only form values, settings-derived summaries, and one operation state.

use std::path::PathBuf;
use std::sync::Arc;

use async_channel::Receiver;
use downloads::{DownloadRule, DownloadSubject};
use library::{
    FavoriteItemId, FolderContents, FolderId, HomeSectionKind, MusicFolderId, PlaylistEdit,
    PlaylistTrackAdd, SearchRequest as LibrarySearchRequest, SearchResults, SourceId, TrackId,
    TrackSelection,
};
use secrets::SecretStorageMode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSummary {
    pub id: SourceId,
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalFolder {
    pub path: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalAccessStatus {
    pub sample_source_path: Option<String>,
    pub sample_local_path: Option<String>,
    pub direct_match_count: usize,
    pub prefix_match_count: usize,
    pub metadata_match_count: usize,
    pub unmatched_count: usize,
    pub total_track_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalAccess {
    pub source_id: SourceId,
    pub root_path: PathBuf,
    pub server_prefix: Option<String>,
    pub local_prefix: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalAccessSummary {
    pub source_id: SourceId,
    pub access: Option<SourceLocalAccess>,
    pub status: LocalAccessStatus,
    pub selected_music_folder_name: Option<String>,
    pub album_count: usize,
    pub track_count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ConfiguredSources {
    pub sources: Arc<[SourceSummary]>,
    pub selected_source_id: Option<SourceId>,
    pub local_folders: Arc<[LocalFolder]>,
    pub local_access: Arc<[SourceLocalAccessSummary]>,
    pub first_run: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenSubsonicKind {
    Navidrome,
    OpenSubsonic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialInput {
    pub source_name: Option<String>,
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceSetup {
    Jellyfin {
        credentials: CredentialInput,
        use_instant_mix: bool,
    },
    OpenSubsonic {
        kind: OpenSubsonicKind,
        credentials: CredentialInput,
    },
    Local {
        roots: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialPreset {
    pub source_name: String,
    pub server_url: String,
    pub username: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditableSource {
    pub source: SourceSummary,
    pub credentials: CredentialPreset,
    pub jellyfin_use_instant_mix: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceSettingsChange {
    Jellyfin {
        source_id: SourceId,
        credentials: CredentialInput,
        use_instant_mix: bool,
    },
    OpenSubsonic {
        source_id: SourceId,
        kind: OpenSubsonicKind,
        credentials: CredentialInput,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceProgressStage {
    Connecting,
    Albums,
    Tracks,
    Artists,
    Genres,
    Playlists,
    Home,
    Artwork,
    Files,
    Finalizing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceProgress {
    pub stage: SourceProgressStage,
    pub completed: usize,
    pub total: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceOperation {
    Idle,
    Adding {
        progress: SourceProgress,
    },
    Switching {
        target: SourceId,
        progress: SourceProgress,
    },
    Refreshing {
        source_id: SourceId,
        progress: SourceProgress,
    },
    Failed {
        source_id: Option<SourceId>,
        message: String,
        add_form: bool,
    },
}

impl SourceOperation {
    pub fn blocks_library(&self) -> bool {
        matches!(self, Self::Adding { .. } | Self::Switching { .. })
    }

    pub fn add_form_active(&self) -> bool {
        matches!(
            self,
            Self::Adding { .. } | Self::Failed { add_form: true, .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredServer {
    pub name: String,
    pub address: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryStatus {
    Idle,
    Searching,
    Empty,
    Found(u64),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryUpdate {
    pub servers: Arc<[DiscoveredServer]>,
    pub status: DiscoveryStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderRequest {
    pub source_id: SourceId,
    pub source_session_epoch: playback::SourceSessionEpoch,
    pub folder_id: Option<FolderId>,
    pub music_folder_id: Option<MusicFolderId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    pub source_id: SourceId,
    pub source_session_epoch: playback::SourceSessionEpoch,
    pub search: LibrarySearchRequest,
}

#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub source_id: SourceId,
    pub source_session_epoch: playback::SourceSessionEpoch,
    pub subject: DownloadSubject,
    pub tracks: TrackSelection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveDownloadRequest {
    pub source_id: SourceId,
    pub source_session_epoch: playback::SourceSessionEpoch,
    pub track_id: TrackId,
}

pub trait SourcePort: Send + Sync {
    fn configured_source(&self, source_id: &SourceId) -> Result<Option<EditableSource>, String>;
    fn discover_servers(&self);
    fn configure_source(&self, input: SourceSetup);
    fn update_source(&self, input: SourceSettingsChange);
    fn select_source(&self, source_id: SourceId);
    fn change_secret_storage(&self, mode: SecretStorageMode) -> Receiver<Result<(), String>>;
    fn add_local_folder(&self, path: PathBuf);
    fn remove_local_folder(&self, path: String);
    fn refresh_source(&self, source_id: SourceId);
    fn check_for_source_changes(&self);
    fn selected_library_revealed(&self);
    fn refresh_home(&self, kind: HomeSectionKind);
    fn save_local_access(&self, input: SourceLocalAccess);
    fn clear_local_access(&self, source_id: SourceId);
    fn forget_source(&self, source_id: SourceId);
    fn set_music_folder(&self, source_id: SourceId, folder_id: Option<MusicFolderId>);
    fn set_favorite(&self, item: FavoriteItemId, favorite: bool);
    fn add_playlist_tracks(&self, request: PlaylistTrackAdd) -> usize;
    fn edit_playlist(&self, edit: PlaylistEdit);
    fn download(&self, request: DownloadRequest);
    fn remove_download(&self, request: RemoveDownloadRequest);
    fn remove_download_rule(&self, source_id: SourceId, rule: DownloadRule, delete_downloads: bool);
    fn cancel_download(&self, source_id: SourceId, job_id: String);
    fn move_download(
        &self,
        source_id: SourceId,
        job_id: String,
        target_job_id: String,
        after: bool,
    );
    fn clear_downloads(&self, source_id: SourceId);
    fn folder(&self, request: FolderRequest) -> Receiver<Result<FolderContents, String>>;
    fn search(&self, request: SearchRequest) -> Receiver<Result<SearchResults, String>>;
}

pub type SourceHandle = Arc<dyn SourcePort>;

#[cfg(test)]
mod tests {
    use super::*;

    fn progress() -> SourceProgress {
        SourceProgress {
            stage: SourceProgressStage::Connecting,
            completed: 0,
            total: None,
        }
    }

    #[test]
    fn adding_and_switching_gate_the_selected_library() {
        assert!(
            SourceOperation::Adding {
                progress: progress()
            }
            .blocks_library()
        );
        assert!(
            SourceOperation::Switching {
                target: SourceId::new("target"),
                progress: progress(),
            }
            .blocks_library()
        );
        assert!(
            !SourceOperation::Refreshing {
                source_id: SourceId::new("selected"),
                progress: progress(),
            }
            .blocks_library()
        );
    }
}
