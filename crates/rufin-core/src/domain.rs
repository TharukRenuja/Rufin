use std::{fmt, path::PathBuf};

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
opaque_id!(SmartPlaylistId, "smart-playlist-");
opaque_id!(ServerId, "server-");
opaque_id!(MusicFolderId, "music-folder-");
opaque_id!(FolderId, "folder-");

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalManifestEntry {
    pub facts: LocalFileFacts,
    pub track: Track,
    pub album_artist: String,
    pub cover: Option<LocalManifestCover>,
    pub metadata_hash: String,
    pub search_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct LocalFileFacts {
    pub path: PathBuf,
    pub root_path: PathBuf,
    pub relative_path: String,
    pub file_size: u64,
    pub mtime_seconds: i64,
    pub mtime_nanos: u32,
    pub inode: Option<u64>,
    pub device: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct LocalManifestCover {
    pub item_id: String,
    pub kind: LocalManifestCoverKind,
    pub source_path: PathBuf,
    pub revision: String,
    pub embedded_index: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum LocalManifestCoverKind {
    File,
    Embedded,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalManifestScan {
    pub entries: Vec<LocalManifestEntry>,
    pub deleted_paths: Vec<PathBuf>,
    pub changed_track_ids: Vec<TrackId>,
    pub metadata_track_ids: Vec<TrackId>,
    pub artwork_track_ids: Vec<TrackId>,
    pub retained_track_ids: Vec<TrackId>,
    pub deleted_track_ids: Vec<TrackId>,
    pub dirty_album_ids: Vec<AlbumId>,
    pub dirty_artist_ids: Vec<ArtistId>,
    pub dirty_album_artist_ids: Vec<ArtistId>,
    pub dirty_genre_names: Vec<String>,
    pub counters: LocalScanCounters,
    pub library_changed: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalScanCounters {
    pub roots_walked: u64,
    pub directory_entries_visited: u64,
    pub audio_candidates: u64,
    pub artwork_candidates: u64,
    pub unchanged_reused: u64,
    pub changed_reparsed: u64,
    pub new_parsed: u64,
    pub deleted: u64,
    pub artwork_changed: u64,
    pub tag_reads: u64,
    pub parse_failures: u64,
    pub reused_track_rows: u64,
    pub repaired_stale_manifest_rows: u64,
    pub filesystem_walk_elapsed_ms: u64,
    pub manifest_compare_elapsed_ms: u64,
    pub tag_parse_elapsed_ms: u64,
    pub library_build_elapsed_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistBuiltin {
    MostPlayed,
    NeverPlayed,
    MostSkipped,
}

impl SmartPlaylistBuiltin {
    pub fn key(self) -> &'static str {
        match self {
            Self::MostPlayed => "most_played",
            Self::NeverPlayed => "never_played",
            Self::MostSkipped => "most_skipped",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::MostPlayed => "Most Played",
            Self::NeverPlayed => "Never Played",
            Self::MostSkipped => "Most Skipped",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::MostPlayed, Self::NeverPlayed, Self::MostSkipped]
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|builtin| builtin.key() == value)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistMatchMode {
    All,
    Any,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SmartPlaylistRuleGroup {
    pub mode: SmartPlaylistMatchMode,
    pub rules: Vec<SmartPlaylistRuleNode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistRuleNode {
    Group(SmartPlaylistRuleGroup),
    Rule(SmartPlaylistRule),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SmartPlaylistRule {
    pub field: SmartPlaylistRuleField,
    pub operator: SmartPlaylistRuleOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<SmartPlaylistRuleValue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistRuleField {
    Title,
    Artist,
    Album,
    Comment,
    Genre,
    Rating,
    Year,
    Favorite,
    Played,
    PlayCount,
    SkipCount,
    LastPlayed,
    DateAdded,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistRuleOperator {
    Contains,
    NotContains,
    Equals,
    NotEquals,
    Above,
    Below,
    Between,
    Is,
    IsNot,
    Before,
    After,
    IsEmpty,
    IsNotEmpty,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistRuleValue {
    Text(String),
    Number(i64),
    NumberRange { min: i64, max: i64 },
    Bool(bool),
    Date(String),
    DateRange { start: String, end: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistSortField {
    Title,
    Artist,
    Album,
    Year,
    DateAdded,
    LastPlayed,
    PlayCount,
    SkipCount,
    Rating,
    Duration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SmartPlaylistDefinition {
    pub root: SmartPlaylistRuleGroup,
    pub sort_field: SmartPlaylistSortField,
    pub descending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SmartPlaylist {
    pub id: SmartPlaylistId,
    pub name: String,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub position: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<SmartPlaylistBuiltin>,
    pub definition: SmartPlaylistDefinition,
    pub track_count: u32,
    pub duration_seconds: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_refs: Vec<ImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SmartPlaylistDetail {
    pub smart_playlist: SmartPlaylist,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artist {
    pub id: ArtistId,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Genre {
    pub id: GenreId,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_refs: Vec<ImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub track_count: u32,
    pub duration_seconds: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_refs: Vec<ImageRef>,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum HomeBlockKind {
    Showcase,
    Explore,
    MostPlayed,
    NewlyAdded,
    RecentlyPlayed,
    RecentlyReleased,
    Genres,
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

impl HomeBlockKind {
    pub fn all() -> [Self; 7] {
        [
            Self::Showcase,
            Self::Explore,
            Self::MostPlayed,
            Self::NewlyAdded,
            Self::RecentlyPlayed,
            Self::RecentlyReleased,
            Self::Genres,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Showcase => "Showcase",
            Self::Explore => HomeSectionKind::Explore.title(),
            Self::MostPlayed => HomeSectionKind::MostPlayed.title(),
            Self::NewlyAdded => HomeSectionKind::NewlyAdded.title(),
            Self::RecentlyPlayed => HomeSectionKind::RecentlyPlayed.title(),
            Self::RecentlyReleased => HomeSectionKind::RecentlyReleased.title(),
            Self::Genres => "Featured genres",
        }
    }

    pub fn section_kind(self) -> Option<HomeSectionKind> {
        match self {
            Self::Explore => Some(HomeSectionKind::Explore),
            Self::MostPlayed => Some(HomeSectionKind::MostPlayed),
            Self::NewlyAdded => Some(HomeSectionKind::NewlyAdded),
            Self::RecentlyPlayed => Some(HomeSectionKind::RecentlyPlayed),
            Self::RecentlyReleased => Some(HomeSectionKind::RecentlyReleased),
            Self::Showcase | Self::Genres => None,
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

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
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
    fn domain_opaque_comparable() {
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
