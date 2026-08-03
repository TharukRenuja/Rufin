use crate::SettingsHandle;
use library::{
    AcceptedLibraryChange, FavoriteItemId, HomeSectionKind, HomeSnapshot, LoadedLibrary,
    MetadataItemId, MusicFolderId, SourceId, Track, TrackSelection,
};
use playback::{LoadedPlayRequest, PlaybackProjection, QueuePlacement, SourceSessionEpoch};
use std::sync::Arc;

use super::source::{ConfiguredSources, SourceOperation};
use downloads::DownloadSubject;

use super::source::{DownloadRequest, RemoveDownloadRequest};
use super::source::{MetadataEditRequest, MetadataIdentificationRequest, MetadataRequest};
use super::{DiagnosticsHandle, ProductHandles, ProductReceivers};

#[derive(Clone)]
pub struct SelectedLibrary {
    pub source_id: SourceId,
    pub source_session_epoch: SourceSessionEpoch,
    pub music_folder_id: Option<MusicFolderId>,
    pub playlist_tracks_can_repeat: bool,
    pub artwork: artwork::SourceImages,
    pub loaded: Arc<LoadedLibrary>,
    pub home: Arc<HomeSnapshot>,
}

impl SelectedLibrary {
    pub fn play_request(
        &self,
        tracks: TrackSelection,
        anchor_index: usize,
        placement: QueuePlacement,
        context_id: impl Into<String>,
        shuffled_start: bool,
    ) -> Option<LoadedPlayRequest> {
        LoadedPlayRequest::context(
            self.source_id.clone(),
            self.source_session_epoch,
            tracks,
            anchor_index,
            placement,
            context_id,
            shuffled_start,
        )
    }

    pub fn one_track(&self, track: Track, placement: QueuePlacement) -> LoadedPlayRequest {
        LoadedPlayRequest::one(
            self.source_id.clone(),
            self.source_session_epoch,
            track,
            placement,
        )
    }

    pub fn download_request(
        &self,
        subject: DownloadSubject,
        tracks: TrackSelection,
    ) -> DownloadRequest {
        DownloadRequest {
            source_id: self.source_id.clone(),
            source_session_epoch: self.source_session_epoch,
            subject,
            tracks,
        }
    }

    pub fn remove_download_request(&self, tracks: TrackSelection) -> RemoveDownloadRequest {
        RemoveDownloadRequest {
            source_id: self.source_id.clone(),
            source_session_epoch: self.source_session_epoch,
            tracks,
        }
    }

    pub fn metadata_request(&self, item_id: MetadataItemId) -> MetadataRequest {
        MetadataRequest {
            source_id: self.source_id.clone(),
            source_session_epoch: self.source_session_epoch,
            item_id,
        }
    }

    pub fn metadata_edit_request(&self, edit: library::MetadataEdit) -> MetadataEditRequest {
        MetadataEditRequest {
            source_id: self.source_id.clone(),
            source_session_epoch: self.source_session_epoch,
            edit,
        }
    }

    pub fn metadata_identification_request(
        &self,
        item_id: MetadataItemId,
        editing: library::MetadataEditing,
        values: library::MetadataValues,
    ) -> MetadataIdentificationRequest {
        MetadataIdentificationRequest {
            source_id: self.source_id.clone(),
            source_session_epoch: self.source_session_epoch,
            item_id,
            editing,
            values,
        }
    }
}

/// Ordered selected-source lifecycle publication.
///
/// Source gate state, the drop-before-build handoff, and source replacement
/// share one lane. A new source carries its Library and Playback together;
/// same-source refreshes publish the accepted Library before its matching
/// Playback update without creating another state owner.
pub enum SourceEvent {
    Configured(ConfiguredSources),
    Selected {
        configured: ConfiguredSources,
        selected: SelectedLibrary,
        playback: PlaybackProjection,
    },
    LibraryReplaced {
        configured: ConfiguredSources,
        selected: SelectedLibrary,
    },
    Playback {
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        projection: PlaybackProjection,
    },
    Operation(SourceOperation),
    Home(HomePublication),
    HomeReplaced {
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        home: Arc<HomeSnapshot>,
    },
    LibraryUpdate(SelectedLibraryUpdate),
    FavoriteFailure(FavoriteFailure),
    Downloads(downloads::DownloadEvent),
    ReleaseSelected {
        acknowledged: async_channel::Sender<()>,
    },
}

#[derive(Clone)]
pub struct HomePublication {
    pub source_id: SourceId,
    pub source_session_epoch: SourceSessionEpoch,
    pub kind: HomeSectionKind,
    pub home: Arc<HomeSnapshot>,
}

#[derive(Clone, Debug)]
pub struct SelectedLibraryUpdate {
    pub source_id: SourceId,
    pub source_session_epoch: SourceSessionEpoch,
    pub change: AcceptedLibraryChange,
    pub home: Option<Arc<HomeSnapshot>>,
}

#[derive(Clone, Debug)]
pub struct FavoriteFailure {
    pub source_id: SourceId,
    pub source_session_epoch: SourceSessionEpoch,
    pub item_id: FavoriteItemId,
    pub authoritative_favorite: bool,
    pub message: String,
}

pub struct RuntimeInputs {
    pub diagnostics: DiagnosticsHandle,
    pub products: ProductHandles,
    pub settings: SettingsHandle,
    pub receivers: ProductReceivers,
    pub configured_sources: ConfiguredSources,
    pub source_operation: SourceOperation,
    pub release_history: super::ReleaseHistory,
}
