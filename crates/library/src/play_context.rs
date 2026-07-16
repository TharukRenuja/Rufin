use crate::{
    AlbumId, ArtistId, GenreId, MoodId, MusicFolderId, PlaylistId, SmartPlaylistDefinition,
    SmartPlaylistId, Track, TrackId, TrackSort,
};

pub fn smart_playlist_definition_fingerprint(definition: &SmartPlaylistDefinition) -> String {
    serde_json::to_string(definition).unwrap_or_else(|_| "unavailable".to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtistTrackScope {
    MainArtist,
    AllCredits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayContextDescriptor {
    Album {
        album_id: AlbumId,
        music_folder_id: Option<MusicFolderId>,
    },
    Playlist {
        playlist_id: PlaylistId,
    },
    SmartPlaylist {
        smart_playlist_id: SmartPlaylistId,
        definition_fingerprint: String,
        music_folder_id: Option<MusicFolderId>,
    },
    Folder {
        path: Vec<String>,
        music_folder_id: Option<MusicFolderId>,
    },
    Artist {
        artist_id: ArtistId,
        scope: ArtistTrackScope,
        music_folder_id: Option<MusicFolderId>,
    },
    Genre {
        genre_id: GenreId,
        music_folder_id: Option<MusicFolderId>,
    },
    Mood {
        mood_id: MoodId,
        music_folder_id: Option<MusicFolderId>,
    },
    Favorites {
        music_folder_id: Option<MusicFolderId>,
    },
    Global {
        music_folder_id: Option<MusicFolderId>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaylistSort {
    Position,
    Title,
    Artist,
    Album,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackFilter {
    pub query: Option<String>,
    pub favorites_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayContextOrder {
    Canonical,
    Tracks {
        filter: TrackFilter,
        sort: TrackSort,
        descending: bool,
        favorite_first: bool,
    },
    Playlist {
        query: Option<String>,
        sort: PlaylistSort,
        descending: bool,
    },
    SmartPlaylist,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayContext {
    pub descriptor: PlayContextDescriptor,
    pub order: PlayContextOrder,
}

pub fn context_id(context: &PlayContext) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in format!("{context:?}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("context:{hash:016x}")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayContextAnchor {
    pub track_id: TrackId,
    pub source_rank: usize,
    pub source_item_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayContextItem {
    pub track: Track,
    pub source_rank: usize,
    pub source_item_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializedPlayContext {
    pub items: Vec<PlayContextItem>,
    pub anchor_index: usize,
}
