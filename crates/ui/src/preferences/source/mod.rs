use library::SourceId;
use localization::{tr, trn_with};
use sources::{
    DiscoveredServer, LibrarySourceSelection, ServerDiscoveryStatus, SourceIdentity,
    SourcePresentationState,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

mod field_layout;
pub(super) mod local_access;
pub(crate) mod login;
pub(crate) mod selector;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LibraryLoad {
    Ready,
    Connecting {
        stage: String,
        first_run: bool,
    },
    Switching {
        target: LibrarySourceSelection,
    },
    WaitingForFirstCommit {
        source_id: SourceId,
    },
    Failed {
        source_id: Option<SourceId>,
        message: String,
    },
}

impl LibraryLoad {
    pub(crate) fn source_setup_active(&self) -> bool {
        matches!(self, Self::Connecting { .. } | Self::Failed { .. })
    }

    pub(crate) fn blocks_library(&self) -> bool {
        !matches!(self, Self::Ready | Self::Failed { .. })
    }
}

pub(crate) struct SourceState {
    pub(crate) presentation: RefCell<SourcePresentationState>,
    pub(crate) load: RefCell<LibraryLoad>,
    pub(crate) syncs: RefCell<HashMap<SourceId, library_sync::SourceSyncChanged>>,
    pub(crate) discovered_servers: RefCell<Vec<DiscoveredServer>>,
    pub(crate) discovery_status: RefCell<ServerDiscoveryStatus>,
    pub(crate) discovery_running: Cell<bool>,
    pub(crate) discovery_started: Cell<bool>,
    pub(crate) add_server: RefCell<Option<login::AddServerDialogHandle>>,
    pub(crate) reconnect_toasts_shown: RefCell<HashSet<SourceId>>,
    pub(crate) sync_toasts: RefCell<HashMap<SourceId, adw::Toast>>,
}

impl SourceState {
    pub(crate) fn login_screen_active(&self) -> bool {
        self.presentation.borrow().first_run || self.load.borrow().source_setup_active()
    }
}

pub(crate) fn configured_source_display_name(source: &SourceIdentity) -> String {
    let name = source.name.trim();
    if name.is_empty() {
        configured_source_kind_display_name(&source.kind)
    } else {
        name.to_string()
    }
}

pub(crate) fn configured_source_kind_display_name(kind: &str) -> String {
    login::source_kind_title(kind).map_or_else(|| kind.to_string(), tr)
}

pub(crate) fn configured_source_icon_name(source: &SourceIdentity) -> &'static str {
    configured_source_kind_icon_name(&source.kind)
}

fn configured_source_kind_icon_name(kind: &str) -> &'static str {
    login::source_kind_icon_name(kind).unwrap_or("network-server-symbolic")
}

pub(crate) fn folder_count_text(count: u64) -> String {
    let label = count.to_string();
    trn_with(
        "{count} folder",
        "{count} folders",
        count,
        &[("count", label.as_str())],
    )
}

pub(crate) fn folder_selected_text(count: u64) -> String {
    let label = count.to_string();
    trn_with(
        "{count} folder selected",
        "{count} folders selected",
        count,
        &[("count", label.as_str())],
    )
}

pub(crate) fn source_sync_progress_text(change: &library_sync::SourceSyncChanged) -> String {
    match change.progress {
        None => "Syncing library...".to_string(),
        Some(library_sync::Progress::LocalScan(progress)) => match progress.stage {
            library_sync::LocalScanStage::Walking => "Scanning folders...".to_string(),
            library_sync::LocalScanStage::ReadingTags => "Reading track metadata...".to_string(),
            library_sync::LocalScanStage::BuildingLibrary => "Preparing local cache...".to_string(),
        },
        Some(library_sync::Progress::CollectionStarted(collection)) => {
            format!("Fetching {}...", sync_collection_name(collection))
        }
        Some(library_sync::Progress::PageFetching {
            collection,
            fetched,
            total,
        }) => match total {
            Some(total) => format!(
                "Fetching {}, {fetched}/{total} fetched...",
                sync_collection_name(collection)
            ),
            None => format!(
                "Fetching {}, {fetched} fetched...",
                sync_collection_name(collection)
            ),
        },
        Some(library_sync::Progress::PageStaged {
            collection,
            fetched,
        }) => format!(
            "Cached {}, {fetched} ready...",
            sync_collection_name(collection)
        ),
        Some(library_sync::Progress::Finalizing) => "Finalizing library cache...".to_string(),
        Some(library_sync::Progress::Finished) => tr("Cached library ready"),
    }
}

fn sync_collection_name(collection: library_sync::Collection) -> &'static str {
    match collection {
        library_sync::Collection::Albums => "albums",
        library_sync::Collection::Tracks => "tracks",
        library_sync::Collection::MusicFolders => "music folders",
        library_sync::Collection::Artists => "artists",
        library_sync::Collection::AlbumArtists => "album artists",
        library_sync::Collection::Genres => "genres",
        library_sync::Collection::Playlists => "playlists",
        library_sync::Collection::HomeSections => "home sections",
    }
}
