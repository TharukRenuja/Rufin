use std::sync::Arc;

use library::play_context::{ArtistTrackScope, PlayContextDescriptor, PlaylistSort};
use library::{
    Album, AlbumId, Artist, ArtistId, Genre, GenreId, MoodId, MusicFolderId, Playlist,
    PlaylistEntry, PlaylistId, SmartPlaylist, Track, TrackId, TrackSort,
};
use sources::{GeneratedTrackSeedKind, PlayedFilter, RandomTrackDomain};

use crate::{AudioOutput, OccurrenceId, Placement, QueuePage, QueuePageQuery, RepeatMode};

pub type TrackLookup = Box<dyn FnMut(usize) -> Option<Track>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueuePlacement {
    Now,
    Next,
    Last,
}

impl From<QueuePlacement> for Placement {
    fn from(value: QueuePlacement) -> Self {
        match value {
            QueuePlacement::Now => Self::Replace { anchor_index: 0 },
            QueuePlacement::Next => Self::AfterCurrent,
            QueuePlacement::Last => Self::End,
        }
    }
}

pub struct AlbumPlayRequest {
    pub album_id: AlbumId,
    pub tracks: Vec<Track>,
    pub anchor_index: usize,
    pub shuffled_start: bool,
}

pub struct PlaylistEntryPlayRequest {
    pub playlist_id: PlaylistId,
    pub entry: PlaylistEntry,
    pub source_index: usize,
    pub query: Option<String>,
    pub sort: PlaylistSort,
    pub descending: bool,
    pub shuffled_start: bool,
}

pub struct CachedPlaylistPlayRequest {
    pub playlist_id: PlaylistId,
    pub placement: QueuePlacement,
}

impl CachedPlaylistPlayRequest {
    pub fn new(playlist_id: PlaylistId, placement: QueuePlacement) -> Self {
        Self {
            playlist_id,
            placement,
        }
    }
}

pub struct SmartPlaylistPlayRequest {
    pub playlist: SmartPlaylist,
    pub anchor_track_id: Option<TrackId>,
    pub music_folder_id: Option<MusicFolderId>,
}

pub struct LibraryWindowPlayRequest {
    pub descriptor: PlayContextDescriptor,
    pub sort: TrackSort,
    pub descending: bool,
    pub query: String,
    pub favorites_only: bool,
    pub favorite_first: bool,
    pub total_items: usize,
    pub anchor_index: usize,
    pub track_at: TrackLookup,
}

pub struct FolderWindowPlayRequest {
    pub path: Vec<String>,
    pub query: String,
    pub sort: TrackSort,
    pub descending: bool,
    pub tracks: Arc<Vec<Track>>,
    pub anchor_index: usize,
}

pub struct ArtistWindowPlayRequest {
    pub artist_id: ArtistId,
    pub scope: ArtistTrackScope,
    pub total_items: usize,
    pub anchor_index: usize,
    pub track_at: TrackLookup,
}

pub struct GenreWindowPlayRequest {
    pub genre_id: GenreId,
    pub total_items: usize,
    pub anchor_index: usize,
    pub track_at: TrackLookup,
}

pub struct MoodWindowPlayRequest {
    pub mood_id: MoodId,
    pub total_items: usize,
    pub anchor_index: usize,
    pub track_at: TrackLookup,
}

pub struct QueueReorderRequest {
    pub occurrence: OccurrenceId,
    pub target_index: usize,
    pub after: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RandomPlayRequest {
    pub placement: QueuePlacement,
    pub limit: usize,
    pub min_year: Option<u16>,
    pub max_year: Option<u16>,
    pub genre_id: Option<GenreId>,
    pub genre_name: Option<String>,
    pub played_filter: PlayedFilter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RadioSeed {
    Track(Track),
    Album(Album),
    Artist(Artist),
    Genre(Genre),
    Playlist(Playlist),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RadioPlayRequest {
    pub placement: QueuePlacement,
    pub seed: RadioSeed,
}

impl RadioPlayRequest {
    pub fn now(seed: RadioSeed) -> Self {
        Self {
            placement: QueuePlacement::Now,
            seed,
        }
    }

    pub fn next(seed: RadioSeed) -> Self {
        Self {
            placement: QueuePlacement::Next,
            seed,
        }
    }

    pub fn last(seed: RadioSeed) -> Self {
        Self {
            placement: QueuePlacement::Last,
            seed,
        }
    }
}

pub trait QueueCommandPort: Send + Sync {
    fn play_tracks_now(&self, tracks: Vec<Track>);
    fn play_now(&self, track: Track);
    fn play_album(&self, request: AlbumPlayRequest);
    fn play_playlist_entry(&self, request: PlaylistEntryPlayRequest);
    fn play_cached_playlist(&self, request: CachedPlaylistPlayRequest);
    fn play_smart_playlist(&self, request: SmartPlaylistPlayRequest);
    fn play_library_window(&self, request: LibraryWindowPlayRequest) -> bool;
    fn play_folder_window(&self, request: FolderWindowPlayRequest) -> bool;
    fn play_artist_window(&self, request: ArtistWindowPlayRequest) -> bool;
    fn play_genre_window(&self, request: GenreWindowPlayRequest) -> bool;
    fn play_mood_window(&self, request: MoodWindowPlayRequest) -> bool;
    fn play_next(&self, track: Track);
    fn play_last(&self, tracks: Vec<Track>);
    fn remove(&self, occurrence: OccurrenceId);
    fn activate(&self, occurrence: OccurrenceId);
    fn move_after_current(&self, occurrence: OccurrenceId);
    fn reorder(&self, request: QueueReorderRequest);
    fn clear(&self);
    fn request_page(&self, query: QueuePageQuery) -> Option<QueuePage>;
}

pub trait RadioCommandPort: Send + Sync {
    fn random_track_domain(&self) -> Option<RandomTrackDomain>;
    fn play_random(&self, request: RandomPlayRequest);
    fn manual_radio_supported(&self, kind: GeneratedTrackSeedKind) -> bool;
    fn play_radio(&self, request: RadioPlayRequest);
}

pub trait TransportCommandPort: Send + Sync {
    fn play_pause(&self);
    fn play(&self);
    fn pause(&self);
    fn stop(&self);
    fn next(&self);
    fn previous(&self);
    fn seek_seconds(&self, seconds: u32);
    fn seek_millis(&self, millis: u64);
    fn set_volume(&self, volume: f64);
    fn persist_volume(&self, volume: f64);
    fn set_muted(&self, muted: bool);
    fn toggle_shuffle(&self);
    fn set_shuffle(&self, enabled: bool);
    fn cycle_repeat(&self);
    fn set_repeat(&self, repeat: RepeatMode);
    fn toggle_auto_dj(&self);
    fn set_visualizer_enabled(&self, enabled: bool);
    fn available_audio_outputs(&self) -> Vec<AudioOutput>;
    fn poll_events(&self);
    fn shutdown(&self);
}

pub trait WaveformCommandPort: Send + Sync {
    fn request_current(&self);
    fn warm_queue(&self);
}

pub type TransportHandle = Arc<dyn TransportCommandPort>;
pub type QueueHandle = Arc<dyn QueueCommandPort>;
pub type RadioHandle = Arc<dyn RadioCommandPort>;
pub type WaveformHandle = Arc<dyn WaveformCommandPort>;

#[derive(Clone)]
pub struct PlaybackHandles {
    pub transport: TransportHandle,
    pub queue: QueueHandle,
    pub radio: RadioHandle,
    pub waveform: WaveformHandle,
}
