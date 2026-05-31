use std::collections::HashMap;
use std::path::Path;

use rufin_core::{
    Album, AlbumId, Artist, ArtistCredit, ArtistId, Folder, FolderId, Genre, GenreId, ImageRef,
    Playlist, PlaylistId, Track, TrackId,
};
use serde::Deserialize;

use crate::root::{jellyfin_id, stable_hash};

pub(super) const ITEM_FIELDS: &str = "Path,Overview,Container,Genres,DateCreated,PremiereDate,ProductionYear,RunTimeTicks,ParentId,AlbumId,AlbumPrimaryImageTag,AlbumArtists,ArtistItems,UserData,ImageTags,ChildCount,AlbumCount,SongCount";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct ItemQueryResult {
    #[serde(default)]
    pub(super) items: Vec<JellyfinItem>,
    pub(super) total_record_count: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct JellyfinItem {
    pub(super) id: String,
    pub(super) name: Option<String>,
    overview: Option<String>,
    #[serde(rename = "Type")]
    pub(super) item_type: Option<String>,
    pub(super) collection_type: Option<String>,
    album_artist: Option<String>,
    album_artists: Option<Vec<NameIdPair>>,
    artists: Option<Vec<String>>,
    genres: Option<Vec<String>>,
    artist_items: Option<Vec<NameIdPair>>,
    album: Option<String>,
    pub(super) album_id: Option<String>,
    album_primary_image_tag: Option<String>,
    path: Option<String>,
    container: Option<String>,
    parent_id: Option<String>,
    production_year: Option<i32>,
    date_created: Option<String>,
    premiere_date: Option<String>,
    run_time_ticks: Option<i64>,
    child_count: Option<i32>,
    album_count: Option<i32>,
    song_count: Option<i32>,
    item_counts: Option<JellyfinItemCounts>,
    index_number: Option<i32>,
    parent_index_number: Option<i32>,
    user_data: Option<UserData>,
    image_tags: Option<HashMap<String, String>>,
    pub(super) playlist_item_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinItemCounts {
    album_count: Option<i32>,
    song_count: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NameIdPair {
    name: Option<String>,
    id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserData {
    is_favorite: Option<bool>,
    play_count: Option<i32>,
    last_played_date: Option<String>,
    rating: Option<i32>,
}

pub(super) fn album_from_item(item: JellyfinItem) -> Album {
    let item_id = item.id.clone();
    let album_artist_credits = artist_credits_from_pairs(item.album_artists.as_deref());
    let artist_credits = artist_credits_from_pairs(item.artist_items.as_deref());
    let artist_id = album_artist_credits.first().map(|artist| artist.id.clone());
    let artist = item
        .album_artist
        .clone()
        .filter(|artist| !artist.trim().is_empty())
        .or_else(|| joined_credit_names(&album_artist_credits))
        .or_else(|| {
            item.artists
                .as_ref()
                .and_then(|artists| joined_artist_names(Some(artists)))
        })
        .unwrap_or_else(|| "Unknown Artist".to_string());
    Album {
        id: AlbumId::new(jellyfin_id("album", &item.id)),
        title: item.name.unwrap_or_else(|| "Untitled Album".to_string()),
        artist,
        artist_id,
        album_artist_credits,
        artist_credits,
        year: u16_from_option(item.production_year),
        release_date: normalized_date(item.premiere_date),
        date_added: normalized_date(item.date_created),
        last_played: normalized_date(
            item.user_data
                .as_ref()
                .and_then(|data| data.last_played_date.clone()),
        ),
        play_count: play_count(&item.user_data),
        user_rating: user_rating(&item.user_data),
        track_count: u16_from_option(item.child_count),
        duration_seconds: duration_seconds(item.run_time_ticks),
        favorite: favorite(&item.user_data),
        color_seed: color_seed(&item_id),
        image_ref: primary_image_ref("album", &item.id, &item.image_tags),
        genres: item.genres.unwrap_or_default(),
    }
}

pub(super) fn track_from_item(item: JellyfinItem) -> Track {
    let image_ref =
        album_image_ref(&item).or_else(|| primary_image_ref("track", &item.id, &item.image_tags));
    let artist_credits = artist_credits_from_pairs(item.artist_items.as_deref());
    let album_artist_credits = artist_credits_from_pairs(item.album_artists.as_deref());
    let artist_id = artist_credits
        .first()
        .or_else(|| album_artist_credits.first())
        .map(|artist| artist.id.clone());
    let album_id = item
        .album_id
        .as_deref()
        .or(item.parent_id.as_deref())
        .unwrap_or(&item.id);
    let source_format = source_format_from_item(item.container.as_deref(), item.path.as_deref());
    Track {
        id: TrackId::new(jellyfin_id("track", &item.id)),
        album_id: AlbumId::new(jellyfin_id("album", album_id)),
        title: item.name.unwrap_or_else(|| "Untitled Track".to_string()),
        artist: item
            .artists
            .as_ref()
            .and_then(|artists| joined_artist_names(Some(artists)))
            .or_else(|| joined_credit_names(&artist_credits))
            .unwrap_or_else(|| {
                item.album_artist
                    .unwrap_or_else(|| "Unknown Artist".to_string())
            }),
        artist_id,
        artist_credits,
        album_artist_credits,
        album: item.album.unwrap_or_else(|| "Unknown Album".to_string()),
        year: u16_from_option(item.production_year),
        release_date: normalized_date(item.premiere_date),
        date_added: normalized_date(item.date_created),
        last_played: normalized_date(
            item.user_data
                .as_ref()
                .and_then(|data| data.last_played_date.clone()),
        ),
        play_count: play_count(&item.user_data),
        user_rating: user_rating(&item.user_data),
        duration_seconds: duration_seconds(item.run_time_ticks),
        favorite: favorite(&item.user_data),
        disc_number: u16_from_option(item.parent_index_number),
        track_number: u16_from_option(item.index_number),
        image_ref,
        genres: item.genres.unwrap_or_default(),
        local_path: item.path,
        source_format,
        comment: item.overview.filter(|value| !value.trim().is_empty()),
        skip_count: None,
    }
}

fn source_format_from_item(container: Option<&str>, path: Option<&str>) -> Option<String> {
    container
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            let raw_path = path?;
            let path = raw_path.split(['?', '#']).next().unwrap_or(raw_path);
            Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}

pub(super) fn folder_from_item(item: JellyfinItem) -> Folder {
    Folder {
        id: FolderId::new(jellyfin_id("folder", &item.id)),
        name: item.name.unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}

pub(super) fn parent_folder_id(item: &JellyfinItem) -> Option<FolderId> {
    item.parent_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .map(|id| FolderId::new(jellyfin_id("folder", id)))
}

pub(super) fn is_audio_item(item: &JellyfinItem) -> bool {
    item.item_type
        .as_deref()
        .is_some_and(|item_type| item_type.eq_ignore_ascii_case("Audio"))
}

pub(super) fn artist_from_item(item: JellyfinItem) -> Artist {
    Artist {
        id: ArtistId::new(jellyfin_id("artist", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Artist".to_string()),
        album_count: u32_from_option(
            item.album_count
                .or_else(|| {
                    item.item_counts
                        .as_ref()
                        .and_then(|counts| counts.album_count)
                })
                .or(item.child_count),
        ),
        track_count: u32_from_option(item.song_count.or_else(|| {
            item.item_counts
                .as_ref()
                .and_then(|counts| counts.song_count)
        })),
        favorite: favorite(&item.user_data),
        last_played: normalized_date(
            item.user_data
                .as_ref()
                .and_then(|data| data.last_played_date.clone()),
        ),
        play_count: play_count(&item.user_data),
        user_rating: user_rating(&item.user_data),
        image_ref: primary_image_ref("artist", &item.id, &item.image_tags),
    }
}

pub(super) fn genre_from_item(item: JellyfinItem) -> Genre {
    Genre {
        id: GenreId::new(jellyfin_id("genre", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Genre".to_string()),
        album_count: u32_from_option(
            item.album_count
                .or_else(|| {
                    item.item_counts
                        .as_ref()
                        .and_then(|counts| counts.album_count)
                })
                .or(item.child_count),
        ),
        track_count: u32_from_option(item.song_count.or_else(|| {
            item.item_counts
                .as_ref()
                .and_then(|counts| counts.song_count)
        })),
        image_refs: Vec::new(),
        image_ref: primary_image_ref("genre", &item.id, &item.image_tags),
    }
}

pub(super) fn playlist_from_item(item: JellyfinItem) -> Playlist {
    Playlist {
        id: PlaylistId::new(jellyfin_id("playlist", &item.id)),
        name: item.name.unwrap_or_else(|| "Untitled Playlist".to_string()),
        track_count: u32_from_option(item.child_count),
        duration_seconds: duration_seconds(item.run_time_ticks),
        image_refs: Vec::new(),
        image_ref: primary_image_ref("playlist", &item.id, &item.image_tags),
    }
}

fn artist_credits_from_pairs(pairs: Option<&[NameIdPair]>) -> Vec<ArtistCredit> {
    pairs
        .unwrap_or_default()
        .iter()
        .filter(|pair| !pair.id.trim().is_empty())
        .map(|pair| ArtistCredit {
            id: ArtistId::new(jellyfin_id("artist", &pair.id)),
            name: pair
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Unknown Artist")
                .to_string(),
        })
        .collect()
}

fn joined_credit_names(credits: &[ArtistCredit]) -> Option<String> {
    let names = credits
        .iter()
        .map(|credit| credit.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    (!names.is_empty()).then(|| names.join(", "))
}

fn joined_artist_names(artists: Option<&[String]>) -> Option<String> {
    let names = artists
        .unwrap_or_default()
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    (!names.is_empty()).then(|| names.join(", "))
}

fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}

fn duration_seconds(ticks: Option<i64>) -> u32 {
    ticks
        .map(|value| (value.max(0) / 10_000_000) as u32)
        .unwrap_or(0)
}

fn u16_from_option(value: Option<i32>) -> u16 {
    value.unwrap_or_default().clamp(0, i32::from(u16::MAX)) as u16
}

fn u32_from_option(value: Option<i32>) -> u32 {
    value.unwrap_or_default().max(0) as u32
}

fn favorite(user_data: &Option<UserData>) -> bool {
    user_data
        .as_ref()
        .and_then(|data| data.is_favorite)
        .unwrap_or(false)
}

fn play_count(user_data: &Option<UserData>) -> Option<u32> {
    user_data
        .as_ref()
        .and_then(|data| data.play_count)
        .map(|value| value.max(0) as u32)
}

fn user_rating(user_data: &Option<UserData>) -> Option<u8> {
    user_data
        .as_ref()
        .and_then(|data| data.rating)
        .map(|value| value.clamp(0, i32::from(u8::MAX)) as u8)
}

fn normalized_date(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    if value.is_empty() {
        return None;
    }
    if value.len() >= 10 {
        let prefix = &value[..10];
        if prefix.as_bytes().get(4) == Some(&b'-') && prefix.as_bytes().get(7) == Some(&b'-') {
            return Some(prefix.to_string());
        }
    }
    Some(value)
}

fn primary_image_ref(
    kind: &str,
    item_id: &str,
    image_tags: &Option<HashMap<String, String>>,
) -> Option<ImageRef> {
    image_tags
        .as_ref()
        .and_then(|tags| tags.get("Primary"))
        .filter(|tag| !tag.is_empty())
        .map(|tag| ImageRef {
            item_id: jellyfin_id(kind, item_id),
            tag: Some(tag.clone()),
        })
}

fn album_image_ref(item: &JellyfinItem) -> Option<ImageRef> {
    let album_id = item.album_id.as_deref()?.trim();
    let tag = item.album_primary_image_tag.as_deref()?.trim();
    if album_id.is_empty() || tag.is_empty() {
        return None;
    }
    Some(ImageRef {
        item_id: jellyfin_id("album", album_id),
        tag: Some(tag.to_string()),
    })
}
