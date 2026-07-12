use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum SourceEntityKind {
    Album,
    Track,
    Artist,
    AlbumArtist,
    Genre,
    Playlist,
    MusicFolder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceObjectMapping {
    pub source_object_id: String,
    pub entity_kind: SourceEntityKind,
    pub entity_id: String,
}

impl SourceEntityKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Track => "track",
            Self::Artist => "artist",
            Self::AlbumArtist => "album_artist",
            Self::Genre => "genre",
            Self::Playlist => "playlist",
            Self::MusicFolder => "music_folder",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "album" => Some(Self::Album),
            "track" => Some(Self::Track),
            "artist" => Some(Self::Artist),
            "album_artist" => Some(Self::AlbumArtist),
            "genre" => Some(Self::Genre),
            "playlist" => Some(Self::Playlist),
            "music_folder" => Some(Self::MusicFolder),
            _ => None,
        }
    }
}
