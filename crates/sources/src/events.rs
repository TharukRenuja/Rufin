use library::{MusicFolder, MusicFolderId, SourceId, SourceLocalAccess};

use crate::{
    LibrarySourceSelection, LocalLibraryFolder, SourceIdentity, jellyfin::DiscoveredJellyfinServer,
};

#[derive(Clone, Debug)]
pub struct SourcePresentationState {
    pub source: Option<SourceIdentity>,
    pub sources: Vec<SourceIdentity>,
    pub selected_source: Option<LibrarySourceSelection>,
    pub local_folders: Vec<LocalLibraryFolder>,
    pub source_local_access: Vec<SourceLocalAccessPresentation>,
    pub local_access: Option<SourceLocalAccess>,
    pub local_access_status: LocalAccessStatus,
    pub music_folders: Vec<MusicFolder>,
    pub selected_music_folder_id: Option<MusicFolderId>,
    pub first_run: bool,
    pub cache: LibraryCacheState,
}

impl SourcePresentationState {
    pub fn first_run() -> Self {
        Self {
            source: None,
            sources: Vec::new(),
            selected_source: None,
            local_folders: Vec::new(),
            source_local_access: Vec::new(),
            local_access: None,
            local_access_status: LocalAccessStatus::default(),
            music_folders: Vec::new(),
            selected_music_folder_id: None,
            first_run: true,
            cache: LibraryCacheState::NoCache { revision: 0 },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryCacheState {
    NoCache { revision: i64 },
    Committed { revision: i64 },
}

impl LibraryCacheState {
    pub fn revision(self) -> i64 {
        match self {
            Self::NoCache { revision } | Self::Committed { revision } => revision,
        }
    }

    pub fn is_committed(self) -> bool {
        matches!(self, Self::Committed { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalAccessPresentation {
    pub source_id: SourceId,
    pub access: Option<SourceLocalAccess>,
    pub status: LocalAccessStatus,
    pub selected_music_folder_name: Option<String>,
    pub cached_album_count: usize,
    pub cached_track_count: usize,
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
pub struct DiscoveredServer {
    pub kind: String,
    pub name: String,
    pub address: String,
    pub id: Option<String>,
}

impl From<DiscoveredJellyfinServer> for DiscoveredServer {
    fn from(server: DiscoveredJellyfinServer) -> Self {
        Self {
            kind: "Jellyfin".to_string(),
            name: server.name,
            address: server.address,
            id: server.id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerDiscoveryStatus {
    Idle,
    Searching,
    Empty,
    Found(u64),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerDiscoveryUpdate {
    pub servers: Vec<DiscoveredServer>,
    pub status: ServerDiscoveryStatus,
    pub running: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceNotice {
    Checking { source_name: String },
    Connected,
    SettingsSaved,
    NoChanges,
    CacheCleared,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSelectionChanged {
    pub selected_source: LibrarySourceSelection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionFailed {
    pub source_id: Option<SourceId>,
    pub error: String,
}
