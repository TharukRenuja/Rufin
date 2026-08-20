use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use library::{
    Album, AlbumId, AlbumRelations, Artist, ArtistCredit, ArtistId, Folder, FolderId, Genre,
    GenreCredit, GenreId, ImageRef, Playlist, PlaylistId, Track, TrackData, TrackId,
    TrackRelations,
};
use serde::Deserialize;

use crate::policy::{normalized_date, u16_from_option};

use super::{jellyfin_id, stable_hash};

pub(super) const ALBUM_FIELDS: &str = "Genres,DateCreated,PremiereDate,ProductionYear,RunTimeTicks,AlbumArtists,ArtistItems,ProviderIds,UserData,ImageTags,BackdropImageTags,ParentBackdropItemId,ParentBackdropImageTags,ChildCount";
pub(super) const TRACK_FIELDS: &str = "Path,Overview,Container,Genres,DateCreated,PremiereDate,ProductionYear,RunTimeTicks,AlbumId,AlbumPrimaryImageTag,AlbumArtists,ArtistItems,ProviderIds,UserData,ImageTags,BackdropImageTags,ParentBackdropItemId,ParentBackdropImageTags";
pub(super) const PLAYLIST_FIELDS: &str = "RunTimeTicks,ImageTags,ChildCount";
pub(super) const MIXED_ITEM_FIELDS: &str = "Path,Overview,Container,Genres,DateCreated,PremiereDate,ProductionYear,RunTimeTicks,ParentId,AlbumId,AlbumPrimaryImageTag,AlbumArtists,ArtistItems,ProviderIds,UserData,ImageTags,BackdropImageTags,ParentBackdropItemId,ParentBackdropImageTags,ChildCount,AlbumCount,SongCount";

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
    parent_id: Option<String>,
    album_artist: Option<String>,
    album_artists: Option<Vec<NameIdPair>>,
    artists: Option<Vec<String>>,
    genre_items: Option<Vec<NameIdPair>>,
    artist_items: Option<Vec<NameIdPair>>,
    provider_ids: Option<HashMap<String, String>>,
    album: Option<String>,
    pub(super) album_id: Option<String>,
    album_primary_image_tag: Option<String>,
    path: Option<String>,
    container: Option<String>,
    production_year: Option<i32>,
    date_created: Option<String>,
    premiere_date: Option<String>,
    run_time_ticks: Option<i64>,
    index_number: Option<i32>,
    parent_index_number: Option<i32>,
    user_data: Option<UserData>,
    pub(super) image_tags: Option<HashMap<String, String>>,
    backdrop_image_tags: Option<Vec<String>>,
    parent_backdrop_item_id: Option<String>,
    parent_backdrop_image_tags: Option<Vec<String>>,
    pub(super) playlist_item_id: Option<String>,
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
    rating: Option<f64>,
}

pub(super) fn album_from_item(item: JellyfinItem) -> Album {
    let item_id = item.id.clone();
    let image_ref = primary_image_ref("album", &item.id, &item.image_tags)
        .or_else(|| backdrop_image_ref(&item));
    let album_artist_credits = artist_credits_from_pairs(item.album_artists.as_deref());
    let artist_credits = artist_credits_from_pairs(item.artist_items.as_deref());
    let album_artist_credits = if album_artist_credits.is_empty() {
        musicbrainz_album_artist_credit(item.album_artist.as_deref(), &item.provider_ids)
    } else {
        album_artist_credits
    };
    let genres = genre_credits_from_pairs(item.genre_items.as_deref());
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
        year: u16_from_option(item.production_year),
        release_date: normalized_date(item.premiere_date),
        date_added: normalized_date(item.date_created),
        last_played: normalized_timestamp(
            item.user_data
                .as_ref()
                .and_then(|data| data.last_played_date.clone()),
        ),
        play_count: play_count(&item.user_data),
        user_rating: user_rating(&item.user_data),
        favorite: favorite(&item.user_data),
        color_seed: color_seed(&item_id),
        image_ref,
        local_artwork: None,
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: source_id(&item.provider_ids, "MusicBrainzAlbum"),
        musicbrainz_release_group_id: source_id(&item.provider_ids, "MusicBrainzReleaseGroup"),
        relations: AlbumRelations {
            album_artists: album_artist_credits,
            artists: artist_credits,
            genres,
        },
    }
}

fn musicbrainz_album_artist_credit(
    album_artist: Option<&str>,
    provider_ids: &Option<HashMap<String, String>>,
) -> Vec<ArtistCredit> {
    let Some(name) = album_artist.map(str::trim).filter(|name| !name.is_empty()) else {
        return Vec::new();
    };
    let Some(artist_id) = source_id(provider_ids, "MusicBrainzAlbumArtist") else {
        return Vec::new();
    };
    let artist_id = artist_id.trim();
    if artist_id.is_empty() {
        return Vec::new();
    }
    vec![ArtistCredit {
        id: ArtistId::new(jellyfin_id("artist", &format!("musicbrainz:{artist_id}"))),
        name: name.to_string(),
        musicbrainz_artist_id: Some(artist_id.to_string()),
    }]
}

pub(super) fn track_from_item(item: JellyfinItem) -> Track {
    let image_ref = album_image_ref(&item)
        .or_else(|| primary_image_ref("track", &item.id, &item.image_tags))
        .or_else(|| backdrop_image_ref(&item));
    let artist_credits = artist_credits_from_pairs(item.artist_items.as_deref());
    let album_artist_credits = artist_credits_from_pairs(item.album_artists.as_deref());
    let album_artist_credits = if album_artist_credits.is_empty() {
        musicbrainz_album_artist_credit(item.album_artist.as_deref(), &item.provider_ids)
    } else {
        album_artist_credits
    };
    let genres = genre_credits_from_pairs(item.genre_items.as_deref());
    let album_id = item
        .album_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .map(|id| AlbumId::new(jellyfin_id("album", id)));
    let source_format = source_format_from_item(item.container.as_deref(), item.path.as_deref());
    Track::new(TrackData {
        id: TrackId::new(jellyfin_id("track", &item.id)),
        album_id,
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
        album: item.album.unwrap_or_else(|| "Unknown Album".to_string()),
        album_artwork: None,
        year: u16_from_option(item.production_year),
        release_date: normalized_date(item.premiere_date),
        date_added: normalized_date(item.date_created),
        last_played: normalized_timestamp(
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
        local_artwork: None,
        musicbrainz_recording_id: source_id(&item.provider_ids, "MusicBrainzRecording"),
        musicbrainz_release_track_id: source_id(&item.provider_ids, "MusicBrainzTrack"),
        source_path: item.path,
        cue: None,
        source_format,
        comment: item.overview.filter(|value| !value.trim().is_empty()),
        skip_count: None,
        bpm: None,
        relations: TrackRelations {
            artists: artist_credits,
            album_artists: album_artist_credits,
            genres,
            moods: Vec::new(),
            music_folders: Vec::new(),
        },
    })
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

pub(super) fn is_audio_item(item: &JellyfinItem) -> bool {
    item.item_type
        .as_deref()
        .is_some_and(|item_type| item_type.eq_ignore_ascii_case("Audio"))
}

pub(super) fn artist_from_item(item: JellyfinItem) -> Artist {
    Artist {
        id: ArtistId::new(jellyfin_id("artist", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Artist".to_string()),
        favorite: favorite(&item.user_data),
        last_played: normalized_timestamp(
            item.user_data
                .as_ref()
                .and_then(|data| data.last_played_date.clone()),
        ),
        play_count: play_count(&item.user_data),
        user_rating: user_rating(&item.user_data),
        musicbrainz_artist_id: source_id(&item.provider_ids, "MusicBrainzArtist"),
        image_ref: primary_image_ref("artist", &item.id, &item.image_tags),
        local_artwork: None,
    }
}

struct ArtistInput {
    artist: Artist,
    name_key: Option<String>,
    name_accessed: bool,
}

/// Collapses Jellyfin's folder-backed and name-accessed representations before
/// they cross the source boundary. Jellyfin defines MusicArtist as a
/// dual-access, name-grouped item; Track and Album relations use the
/// parentless name aggregate while a folder-backed twin may carry richer
/// metadata.
pub(super) fn normalize_artist_items(items: Vec<JellyfinItem>) -> Vec<Artist> {
    let mut by_id = BTreeMap::<String, ArtistInput>::new();
    for item in items {
        let name_key = item
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_lowercase);
        let name_accessed = item
            .parent_id
            .as_deref()
            .is_none_or(|parent| parent.trim().is_empty());
        let artist = artist_from_item(item);
        by_id
            .entry(artist.id.as_str().to_string())
            .or_insert(ArtistInput {
                artist,
                name_key,
                name_accessed,
            });
    }

    let mut named = BTreeMap::<String, Vec<ArtistInput>>::new();
    let mut independent = Vec::new();
    for input in by_id.into_values() {
        match input.name_key.as_ref() {
            Some(key) => named.entry(key.clone()).or_default().push(input),
            None => independent.push(input.artist),
        }
    }

    for mut group in named.into_values() {
        let aggregates = group
            .iter()
            .enumerate()
            .filter_map(|(index, input)| input.name_accessed.then_some(index))
            .collect::<Vec<_>>();
        if aggregates.len() != 1 {
            independent.extend(group.into_iter().map(|input| input.artist));
            continue;
        }
        let mut canonical = group.swap_remove(aggregates[0]).artist;
        merge_artist_group(&mut canonical, group.iter().map(|input| &input.artist));
        independent.push(canonical);
    }
    independent.sort_by(|left, right| left.id.cmp(&right.id));
    independent
}

fn merge_artist_group<'a>(canonical: &mut Artist, aliases: impl Iterator<Item = &'a Artist>) {
    let aliases = aliases.collect::<Vec<_>>();
    canonical.favorite |= aliases.iter().any(|artist| artist.favorite);
    canonical.last_played = aliases
        .iter()
        .filter_map(|artist| artist.last_played.as_ref())
        .chain(canonical.last_played.as_ref())
        .max()
        .cloned();
    canonical.play_count = aliases
        .iter()
        .filter_map(|artist| artist.play_count)
        .chain(canonical.play_count)
        .max();
    if canonical.user_rating.is_none() {
        canonical.user_rating = unique(aliases.iter().filter_map(|artist| artist.user_rating));
    }
    if canonical.musicbrainz_artist_id.is_none() {
        canonical.musicbrainz_artist_id = unique(
            aliases
                .iter()
                .filter_map(|artist| artist.musicbrainz_artist_id.clone()),
        );
    }
    if canonical.image_ref.is_none() {
        canonical.image_ref = unique(aliases.iter().filter_map(|artist| artist.image_ref.clone()));
    }
}

fn unique<T: Eq + Clone>(values: impl Iterator<Item = T>) -> Option<T> {
    let mut unique = None;
    for value in values {
        match unique.as_ref() {
            None => unique = Some(value),
            Some(current) if current == &value => {}
            Some(_) => return None,
        }
    }
    unique
}

pub(super) fn genre_from_item(item: JellyfinItem) -> Genre {
    Genre {
        id: GenreId::new(jellyfin_id("genre", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Genre".to_string()),
        image_ref: primary_image_ref("genre", &item.id, &item.image_tags),
    }
}

pub(super) fn playlist_from_item(item: JellyfinItem) -> Playlist {
    Playlist {
        id: PlaylistId::new(jellyfin_id("playlist", &item.id)),
        name: item.name.unwrap_or_else(|| "Untitled Playlist".to_string()),
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
            musicbrainz_artist_id: None,
        })
        .collect()
}

fn genre_credits_from_pairs(pairs: Option<&[NameIdPair]>) -> Vec<GenreCredit> {
    pairs
        .unwrap_or_default()
        .iter()
        .filter(|pair| !pair.id.trim().is_empty())
        .map(|pair| GenreCredit {
            id: GenreId::new(jellyfin_id("genre", &pair.id)),
            name: pair
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Unknown Genre")
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

fn source_id(ids: &Option<HashMap<String, String>>, key: &str) -> Option<String> {
    ids.as_ref()
        .and_then(|ids| ids.get(key))
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}

fn duration_seconds(ticks: Option<i64>) -> u32 {
    ticks
        .map(|value| (value.max(0) / 10_000_000) as u32)
        .unwrap_or(0)
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
        .and_then(library::rating_from_ten_point)
}

fn normalized_timestamp(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn primary_image_ref(
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

fn backdrop_image_ref(item: &JellyfinItem) -> Option<ImageRef> {
    let item_tag = first_image_tag(item.backdrop_image_tags.as_deref());
    if let Some(tag) = item_tag {
        return Some(ImageRef {
            item_id: jellyfin_id("backdrop", &item.id),
            tag: Some(tag),
        });
    }

    let parent_id = item.parent_backdrop_item_id.as_deref()?.trim();
    let tag = first_image_tag(item.parent_backdrop_image_tags.as_deref())?;
    if parent_id.is_empty() {
        return None;
    }
    Some(ImageRef {
        item_id: jellyfin_id("backdrop", parent_id),
        tag: Some(tag),
    })
}

fn first_image_tag(tags: Option<&[String]>) -> Option<String> {
    tags.unwrap_or_default()
        .iter()
        .map(|tag| tag.trim())
        .find(|tag| !tag.is_empty())
        .map(ToString::to_string)
}
