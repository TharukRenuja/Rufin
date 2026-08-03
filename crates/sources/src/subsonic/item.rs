use super::*;
use crate::policy::normalized_date;

pub(super) fn image_ref(
    source: &SubsonicSource,
    cover_art: Option<SubsonicId>,
) -> Option<ImageRef> {
    cover_art.map(|id| ImageRef::new(source.id("cover", &id.0), None))
}
pub(super) fn folder_from_artist(source: &SubsonicSource, artist: SubsonicArtist) -> Folder {
    Folder {
        id: FolderId::new(source.id("folder", artist.id.0.as_str())),
        name: artist.name.unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}
pub(super) fn folder_from_child(source: &SubsonicSource, child: SubsonicSong) -> Folder {
    Folder {
        id: FolderId::new(source.id("folder", child.id.0.as_str())),
        name: child.title.unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}
pub(super) fn sort_folders_by_name(folders: &mut [Folder]) {
    folders.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
}
pub(super) fn genres_from_item(genre: Option<String>, genres: Vec<GenreName>) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(genre) = genre.filter(|genre| !genre.trim().is_empty()) {
        values.push(genre);
    }
    for genre in genres {
        if !genre.name.trim().is_empty() && !values.iter().any(|value| value == &genre.name) {
            values.push(genre.name);
        }
    }
    values
}
fn genre_credits_from_item(
    source: &SubsonicSource,
    genre: Option<String>,
    genres: Vec<GenreName>,
) -> Vec<GenreCredit> {
    genres_from_item(genre, genres)
        .into_iter()
        .map(|name| GenreCredit {
            id: GenreId::new(source.id("genre", &name)),
            name,
        })
        .collect()
}
pub(super) fn moods_from_item(source: &SubsonicSource, moods: Vec<String>) -> Vec<MoodCredit> {
    let mut values = Vec::new();
    for mood in moods {
        let mood = mood.trim();
        if !mood.is_empty()
            && !values
                .iter()
                .any(|value: &MoodCredit| value.name.eq_ignore_ascii_case(mood))
        {
            values.push(MoodCredit {
                id: MoodId::new(source.id("mood", mood)),
                name: mood.to_string(),
            });
        }
    }
    values
}
fn artist_credit(
    source: &SubsonicSource,
    id: Option<&SubsonicId>,
    name: &str,
) -> Option<ArtistCredit> {
    let id = id?;
    (!id.0.trim().is_empty()).then(|| ArtistCredit {
        id: ArtistId::new(source.id("artist", &id.0)),
        name: name.to_string(),
        musicbrainz_artist_id: None,
    })
}

fn artist_credits_from_refs(
    source: &SubsonicSource,
    artists: Vec<SubsonicArtistRef>,
) -> Vec<ArtistCredit> {
    artists
        .into_iter()
        .filter(|artist| !artist.id.0.trim().is_empty())
        .map(|artist| ArtistCredit {
            id: ArtistId::new(source.id("artist", &artist.id.0)),
            name: artist.name,
            musicbrainz_artist_id: None,
        })
        .collect()
}

pub(super) fn joined_artist_names(artists: &[ArtistCredit]) -> Option<String> {
    let names = artists
        .iter()
        .map(|artist| artist.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    (!names.is_empty()).then(|| names.join(", "))
}

fn structured_release_date(date: Option<SubsonicItemDate>) -> Option<String> {
    let date = date?;
    let year = u16::try_from(date.year).ok().filter(|year| *year > 0)?;
    match (
        u8::try_from(date.month)
            .ok()
            .filter(|month| (1..=12).contains(month)),
        u8::try_from(date.day)
            .ok()
            .filter(|day| (1..=31).contains(day)),
    ) {
        (Some(month), Some(day)) => Some(format!("{year:04}-{month:02}-{day:02}")),
        (Some(month), None) => Some(format!("{year:04}-{month:02}")),
        _ => Some(format!("{year:04}")),
    }
}
fn bpm_from_u32(value: u32) -> Option<u16> {
    if value == 0 || value > u32::from(u16::MAX) {
        None
    } else {
        Some(value as u16)
    }
}
fn clean_optional(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}
pub(super) fn album_from_dto(source: &SubsonicSource, album: SubsonicAlbum) -> Album {
    let raw_id = raw_id_string(&album.id);
    let structured_artists = artist_credits_from_refs(source, album.artists);
    let artist = clean_optional(album.display_artist)
        .or_else(|| clean_optional(album.artist.clone()))
        .or_else(|| joined_artist_names(&structured_artists))
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let album_artists = if structured_artists.is_empty() {
        artist_credit(source, album.artist_id.as_ref(), &artist)
            .into_iter()
            .collect()
    } else {
        structured_artists
    };
    let genres = genre_credits_from_item(source, album.genre, album.genres);
    let release_date = structured_release_date(album.release_date);
    let year = {
        let scalar = u16_from_option(album.year);
        if scalar > 0 {
            scalar
        } else {
            release_date
                .as_deref()
                .and_then(|date| date.get(..4))
                .and_then(|year| year.parse().ok())
                .unwrap_or_default()
        }
    };
    Album {
        id: AlbumId::new(source.id("album", &raw_id)),
        title: album
            .title
            .or(album.name)
            .or(album.album)
            .unwrap_or_else(|| "Untitled Album".to_string()),
        artist,
        year,
        release_date,
        date_added: normalized_date(album.created),
        last_played: normalized_timestamp(album.played),
        play_count: album
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: album
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        favorite: favorite(&album.starred),
        color_seed: color_seed(&raw_id),
        image_ref: image_ref(source, album.cover_art),
        local_artwork: None,
        release_types: normalize_release_types(album.release_types),
        is_compilation: album.is_compilation,
        musicbrainz_album_id: clean_optional(album.musicbrainz_album_id),
        musicbrainz_release_group_id: None,
        relations: AlbumRelations {
            album_artists,
            artists: Vec::new(),
            genres,
        },
    }
}
pub(super) fn track_from_dto(source: &SubsonicSource, song: SubsonicSong) -> Track {
    let raw_id = raw_id_string(&song.id);
    let album_id = song
        .album_id
        .as_ref()
        .map(raw_id_string)
        .map(|id| AlbumId::new(source.id("album", &id)));
    let structured_artists = artist_credits_from_refs(source, song.artists);
    let album_artist_credits = artist_credits_from_refs(source, song.album_artists);
    let artist = clean_optional(song.display_artist)
        .or_else(|| clean_optional(song.artist.clone()))
        .or_else(|| joined_artist_names(&structured_artists))
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let artist_credits = if structured_artists.is_empty() {
        artist_credit(source, song.artist_id.as_ref(), &artist)
            .into_iter()
            .collect()
    } else {
        structured_artists
    };
    let genres = genre_credits_from_item(source, song.genre, song.genres);
    let moods = moods_from_item(source, song.moods);
    let source_format = source_format_from_song(
        song.suffix.as_deref(),
        song.content_type.as_deref(),
        song.path.as_deref(),
    );
    Track::new(TrackData {
        id: TrackId::new(source.id("track", &raw_id)),
        album_id,
        title: song.title.unwrap_or_else(|| "Untitled Track".to_string()),
        artist,
        album: song.album.unwrap_or_else(|| "Unknown Album".to_string()),
        album_artwork: None,
        year: u16_from_option(song.year),
        release_date: None,
        date_added: normalized_date(song.created),
        last_played: normalized_timestamp(song.played),
        play_count: song
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: song
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        duration_seconds: song.duration.unwrap_or_default(),
        favorite: favorite(&song.starred),
        disc_number: u16_from_option(song.disc_number),
        track_number: u16_from_option(song.track),
        image_ref: image_ref(source, song.cover_art),
        local_artwork: None,
        musicbrainz_recording_id: clean_optional(song.musicbrainz_recording_id),
        musicbrainz_release_track_id: None,
        source_path: song.path,
        cue: None,
        source_format,
        comment: song.comment.filter(|value| !value.trim().is_empty()),
        skip_count: None,
        bpm: song.bpm.and_then(bpm_from_u32),
        relations: TrackRelations {
            artists: artist_credits,
            album_artists: album_artist_credits,
            genres,
            moods,
            music_folders: Vec::new(),
        },
    })
}

pub(super) fn source_format_from_song(
    suffix: Option<&str>,
    content_type: Option<&str>,
    path: Option<&str>,
) -> Option<String> {
    suffix
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            content_type
                .and_then(|value| value.rsplit('/').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .or_else(|| {
            let raw_path = path?;
            let path = raw_path.split(['?', '#']).next().unwrap_or(raw_path);
            std::path::Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}
pub(super) fn artist_from_dto(source: &SubsonicSource, artist: SubsonicArtist) -> Artist {
    let raw_id = raw_id_string(&artist.id);
    Artist {
        id: ArtistId::new(source.id("artist", &raw_id)),
        name: artist.name.unwrap_or_else(|| "Unknown Artist".to_string()),
        favorite: favorite(&artist.starred),
        last_played: normalized_timestamp(artist.played),
        play_count: artist
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: artist
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        musicbrainz_artist_id: clean_optional(artist.musicbrainz_artist_id),
        image_ref: image_ref(source, artist.cover_art),
        local_artwork: None,
    }
}
pub(super) fn genre_from_dto(source: &SubsonicSource, genre: SubsonicGenre) -> Genre {
    Genre {
        id: GenreId::new(source.id("genre", &genre.value)),
        name: genre.value,
        image_ref: None,
    }
}
fn normalized_timestamp(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn playlist_from_dto(source: &SubsonicSource, playlist: SubsonicPlaylist) -> Playlist {
    let raw_id = raw_id_string(&playlist.id);
    Playlist {
        id: PlaylistId::new(source.id("playlist", &raw_id)),
        name: playlist
            .name
            .unwrap_or_else(|| "Untitled Playlist".to_string()),
        image_ref: image_ref(source, playlist.cover_art),
    }
}
