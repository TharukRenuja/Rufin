use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! opaque_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                let value = value.into();
                assert!(
                    !value.is_empty(),
                    concat!(stringify!($name), " cannot be empty")
                );
                Self(value)
            }

            pub fn fake(number: impl fmt::Display) -> Self {
                Self::new(format!("{}{}", $prefix, number))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

opaque_id!(AlbumId, "album-");
opaque_id!(TrackId, "track-");
opaque_id!(ArtistId, "artist-");
opaque_id!(GenreId, "genre-");
opaque_id!(PlaylistId, "playlist-");
opaque_id!(ServerId, "server-");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerIdentity {
    pub id: ServerId,
    pub provider: String,
    pub name: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtistCredit {
    pub id: ArtistId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Album {
    pub id: AlbumId,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<ArtistId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub album_artist_credits: Vec<ArtistCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artist_credits: Vec<ArtistCredit>,
    pub year: u16,
    pub track_count: u16,
    pub duration_seconds: u32,
    pub favorite: bool,
    pub color_seed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Track {
    pub id: TrackId,
    pub album_id: AlbumId,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<ArtistId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artist_credits: Vec<ArtistCredit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub album_artist_credits: Vec<ArtistCredit>,
    pub album: String,
    pub year: u16,
    pub duration_seconds: u32,
    pub favorite: bool,
    pub disc_number: u16,
    pub track_number: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artist {
    pub id: ArtistId,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Genre {
    pub id: GenreId,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub track_count: u32,
    pub duration_seconds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum HomeSectionKind {
    Explore,
    MostPlayed,
    NewlyAdded,
    RecentlyPlayed,
    RecentlyReleased,
}

pub const HOME_SECTION_ITEM_LIMIT: usize = 24;

impl HomeSectionKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Explore => "Explore",
            Self::MostPlayed => "Most played",
            Self::NewlyAdded => "Newly added",
            Self::RecentlyPlayed => "Recently played",
            Self::RecentlyReleased => "Recently released",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HomeSection {
    pub kind: HomeSectionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub albums: Vec<Album>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
}

pub fn format_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::{AlbumId, TrackId, format_duration};

    #[test]
    fn formats_track_duration() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(185), "3:05");
        assert_eq!(format_duration(3_661), "61:01");
    }

    #[test]
    fn opaque_ids_are_displayable_and_comparable() {
        let album = AlbumId::new("jellyfin:album:abc");
        let same_album = AlbumId::from("jellyfin:album:abc");
        let track = TrackId::fake(42);

        assert_eq!(album, same_album);
        assert_eq!(album.as_str(), "jellyfin:album:abc");
        assert_eq!(track.to_string(), "track-42");
    }

    #[test]
    #[should_panic(expected = "AlbumId cannot be empty")]
    fn opaque_ids_reject_empty_values() {
        let _id = AlbumId::new("");
    }
}
