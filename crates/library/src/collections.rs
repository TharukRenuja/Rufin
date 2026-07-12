use serde::{Deserialize, Serialize};

use crate::{AlbumArtwork, ImageRef};

opaque_id!(MoodId, "mood-");
opaque_id!(PlaylistId, "playlist-");
opaque_id!(MusicFolderId, "music-folder-");
opaque_id!(FolderId, "folder-");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourceFeatureOwner {
    Native,
    Store,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicFolder {
    pub id: MusicFolderId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Mood {
    pub id: MoodId,
    pub name: String,
    pub track_count: u32,
    #[serde(default)]
    pub duration_seconds: u32,
    #[serde(skip)]
    pub representative_albums: Vec<AlbumArtwork>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<SourceFeatureOwner>,
    pub track_count: u32,
    pub duration_seconds: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
    #[serde(skip)]
    pub representative_albums: Vec<AlbumArtwork>,
}
