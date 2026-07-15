use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::{
    Album, AlbumId, Artist, ArtistId, Folder, FolderId, Genre, GenreId, Playlist, Track, TrackId,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PagedResponse<T> {
    pub items: Vec<T>,
    pub total: usize,
}

impl<T> PagedResponse<T> {
    pub fn new(items: Vec<T>, total: usize) -> Self {
        Self { items, total }
    }
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub entry_id: String,
    pub track: Track,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlbumDetail {
    pub album: Album,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenreDetail {
    pub genre: Genre,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FolderDetail {
    pub folder: Folder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<FolderId>,
    pub folders: Vec<Folder>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackSort {
    Title,
    TrackNumber,
    Artist,
    AlbumArtist,
    Album,
    Year,
    ReleaseDate,
    DateAdded,
    LastPlayed,
    PlayCount,
    UserRating,
    Genre,
    Bpm,
    Duration,
    Favorite,
}

pub fn compare_tracks(left: &Track, right: &Track, field: TrackSort, descending: bool) -> Ordering {
    let missing =
        track_sort_value_missing(left, field).cmp(&track_sort_value_missing(right, field));
    if missing != Ordering::Equal {
        return missing;
    }

    let primary = match field {
        TrackSort::TrackNumber => left
            .disc_number
            .cmp(&right.disc_number)
            .then(left.track_number.cmp(&right.track_number)),
        TrackSort::Artist => sqlite_nocase_cmp(&left.artist, &right.artist),
        TrackSort::AlbumArtist => sqlite_nocase_cmp(
            track_album_artist_sort_name(left),
            track_album_artist_sort_name(right),
        ),
        TrackSort::Album => sqlite_nocase_cmp(&left.album, &right.album),
        TrackSort::Year => left.year.cmp(&right.year),
        TrackSort::ReleaseDate => left.release_date.cmp(&right.release_date),
        TrackSort::DateAdded => left.date_added.cmp(&right.date_added),
        TrackSort::LastPlayed => left.last_played.cmp(&right.last_played),
        TrackSort::PlayCount => left.play_count.cmp(&right.play_count),
        TrackSort::UserRating => left.user_rating.cmp(&right.user_rating),
        TrackSort::Genre => sqlite_nocase_cmp(
            first_track_genre_sort_name(left),
            first_track_genre_sort_name(right),
        ),
        TrackSort::Bpm => left.bpm.cmp(&right.bpm),
        TrackSort::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        TrackSort::Favorite => left.favorite.cmp(&right.favorite),
        TrackSort::Title => sqlite_nocase_cmp(&left.title, &right.title),
    }
    .then_with(|| sqlite_nocase_cmp(&left.album, &right.album))
    .then(left.disc_number.cmp(&right.disc_number))
    .then(left.track_number.cmp(&right.track_number))
    .then_with(|| sqlite_nocase_cmp(&left.title, &right.title))
    .then_with(|| left.id.cmp(&right.id));

    if descending {
        primary.reverse()
    } else {
        primary
    }
}

fn track_sort_value_missing(track: &Track, field: TrackSort) -> bool {
    match field {
        TrackSort::ReleaseDate => track.release_date.is_none(),
        TrackSort::DateAdded => track.date_added.is_none(),
        TrackSort::LastPlayed => track.last_played.is_none(),
        TrackSort::PlayCount => track.play_count.is_none(),
        TrackSort::UserRating => track.user_rating.is_none(),
        TrackSort::Bpm => track.bpm.is_none(),
        TrackSort::Title
        | TrackSort::TrackNumber
        | TrackSort::Artist
        | TrackSort::AlbumArtist
        | TrackSort::Album
        | TrackSort::Year
        | TrackSort::Genre
        | TrackSort::Duration
        | TrackSort::Favorite => false,
    }
}

fn track_album_artist_sort_name(track: &Track) -> &str {
    track
        .album_artist_credits
        .first()
        .map_or(track.artist.as_str(), |credit| credit.name.as_str())
}

fn first_track_genre_sort_name(track: &Track) -> &str {
    track
        .genres
        .iter()
        .min_by(|left, right| sqlite_nocase_cmp(left, right))
        .map_or("", String::as_str)
}

fn sqlite_nocase_cmp(left: &str, right: &str) -> Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RandomTrackQuery {
    pub limit: usize,
    pub min_year: Option<u16>,
    pub max_year: Option<u16>,
    pub genre_id: Option<GenreId>,
    pub genre_name: Option<String>,
}
