//! Canonical music values accepted from a source.
//!
//! Sources construct these values, the Store persists them, and one selected
//! [`LoadedLibrary`](crate::LoadedLibrary) shares them with routes and
//! Playback. Relationships stay with the item that reported them; callers do
//! not rebuild a second relation model.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

opaque_id!(AlbumId, "album-");
opaque_id!(TrackId, "track-");
opaque_id!(ArtistId, "artist-");
opaque_id!(GenreId, "genre-");
opaque_id!(MoodId, "mood-");
opaque_id!(PlaylistId, "playlist-");
opaque_id!(MusicFolderId, "music-folder-");
opaque_id!(FolderId, "folder-");
opaque_id!(SmartPlaylistId, "smart-playlist-");
opaque_id!(SourceId, "source-");

pub(crate) fn color_seed(value: &str) -> u32 {
    value.bytes().fold(2_166_136_261_u32, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ImageRef {
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl ImageRef {
    pub fn new(item_id: impl Into<String>, tag: impl Into<Option<String>>) -> Self {
        let item_id = item_id.into();
        assert!(!item_id.is_empty(), "ImageRef item_id cannot be empty");
        Self {
            item_id,
            tag: tag.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum LocalArtworkRef {
    File {
        path: String,
        revision: String,
    },
    Embedded {
        path: String,
        picture_index: u32,
        revision: String,
    },
}

/// One source-owned image that can be prepared without an external lookup.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SourceArtwork {
    Native(ImageRef),
    Local(LocalArtworkRef),
}

impl LocalArtworkRef {
    pub fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::Embedded { path, .. } => path,
        }
    }

    pub fn revision(&self) -> &str {
        match self {
            Self::File { revision, .. } | Self::Embedded { revision, .. } => revision,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumArtwork {
    pub album: Arc<Album>,
    pub representative_track: Option<Track>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtistCredit {
    pub id: ArtistId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_artist_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GenreCredit {
    pub id: GenreId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MoodCredit {
    pub id: MoodId,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AlbumRelations {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub album_artists: Vec<ArtistCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<ArtistCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<GenreCredit>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrackRelations {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<ArtistCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub album_artists: Vec<ArtistCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<GenreCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moods: Vec<MoodCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub music_folders: Vec<MusicFolderId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Album {
    pub id: AlbumId,
    pub title: String,
    pub artist: String,
    pub year: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_added: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<u8>,
    pub favorite: bool,
    pub color_seed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_artwork: Option<LocalArtworkRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub release_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_compilation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_album_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_release_group_id: Option<String>,
    #[serde(default)]
    pub relations: AlbumRelations,
}

/// The Album facts a Track needs for album-first artwork selection.
///
/// One shared value is attached to every Track on an Album. Album fields that
/// do not affect artwork stay on the Album and therefore do not turn an Album
/// favorite or release classification into a Track replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumArtworkFacts {
    pub local_artwork: Option<LocalArtworkRef>,
    pub image_ref: Option<ImageRef>,
    pub musicbrainz_release_group_id: Option<String>,
    pub musicbrainz_album_id: Option<String>,
    pub artist: String,
    pub title: String,
}

impl From<&Album> for AlbumArtworkFacts {
    fn from(album: &Album) -> Self {
        Self {
            local_artwork: album.local_artwork.clone(),
            image_ref: album.image_ref.clone(),
            musicbrainz_release_group_id: album.musicbrainz_release_group_id.clone(),
            musicbrainz_album_id: album.musicbrainz_album_id.clone(),
            artist: album.artist.clone(),
            title: album.title.clone(),
        }
    }
}

impl Album {
    pub fn primary_artist_id(&self) -> Option<&ArtistId> {
        self.relations
            .album_artists
            .first()
            .or_else(|| self.relations.artists.first())
            .map(|credit| &credit.id)
    }

    pub fn genre_names(&self) -> impl Iterator<Item = &str> {
        self.relations
            .genres
            .iter()
            .map(|credit| credit.name.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CueSegment {
    pub cue_path: String,
    pub start_millis: u64,
    pub end_millis: u64,
}

/// A cheap immutable handle to canonical Track facts.
///
/// The selected Library, routes, queue, and current-media integrations all
/// clone this handle. Mutating a detached candidate uses copy-on-write and
/// never changes an already accepted Track.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Track(Arc<TrackData>);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrackData {
    pub id: TrackId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<AlbumId>,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(skip)]
    pub album_artwork: Option<Arc<AlbumArtworkFacts>>,
    pub year: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_added: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<u8>,
    pub duration_seconds: u32,
    pub favorite: bool,
    pub disc_number: u16,
    pub track_number: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_artwork: Option<LocalArtworkRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_recording_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_release_track_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cue: Option<CueSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<u16>,
    #[serde(default)]
    pub relations: TrackRelations,
}

impl Track {
    pub fn new(data: TrackData) -> Self {
        Self(Arc::new(data))
    }

    pub fn make_mut(&mut self) -> &mut TrackData {
        Arc::make_mut(&mut self.0)
    }

    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        Arc::ptr_eq(&left.0, &right.0)
    }

    pub fn primary_artist_id(&self) -> Option<&ArtistId> {
        self.relations
            .artists
            .first()
            .or_else(|| self.relations.album_artists.first())
            .map(|credit| &credit.id)
    }

    pub fn artist_credits(&self) -> &[ArtistCredit] {
        &self.relations.artists
    }

    pub fn album_artist_credits(&self) -> &[ArtistCredit] {
        &self.relations.album_artists
    }

    pub fn genre_names(&self) -> impl Iterator<Item = &str> {
        self.relations
            .genres
            .iter()
            .map(|credit| credit.name.as_str())
    }

    pub fn mood_names(&self) -> impl Iterator<Item = &str> {
        self.relations
            .moods
            .iter()
            .map(|credit| credit.name.as_str())
    }

    pub fn album_artwork_facts(&self) -> Option<&AlbumArtworkFacts> {
        self.album_artwork.as_deref()
    }
}

impl From<TrackData> for Track {
    fn from(data: TrackData) -> Self {
        Self::new(data)
    }
}

impl Deref for Track {
    type Target = TrackData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Track {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.make_mut()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artist {
    pub id: ArtistId,
    pub name: String,
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_artwork: Option<LocalArtworkRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Genre {
    pub id: GenreId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Mood {
    pub id: MoodId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicFolder {
    pub id: MusicFolderId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlaylistEntry {
    pub occurrence_id: String,
    pub track_id: TrackId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlaylistSnapshot {
    pub playlist: Playlist,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum FavoriteItemId {
    Album(AlbumId),
    Track(TrackId),
    Artist(ArtistId),
}

impl FavoriteItemId {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Album(id) => id.as_str(),
            Self::Track(id) => id.as_str(),
            Self::Artist(id) => id.as_str(),
        }
    }

    pub(crate) const fn kind(&self) -> FavoriteItemKind {
        match self {
            Self::Album(_) => FavoriteItemKind::Album,
            Self::Track(_) => FavoriteItemKind::Track,
            Self::Artist(_) => FavoriteItemKind::Artist,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FavoriteItemKind {
    Album,
    Track,
    Artist,
}

impl FavoriteItemKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Track => "track",
            Self::Artist => "artist",
        }
    }
}

pub fn normalize_release_types(types: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    let mut values = Vec::new();
    for release_type in types {
        let value = release_type.as_ref().trim().to_ascii_lowercase();
        if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
            values.push(value);
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::Track;

    #[test]
    fn track_is_one_shared_pointer() {
        assert_eq!(size_of::<Track>(), size_of::<usize>());
    }
}
