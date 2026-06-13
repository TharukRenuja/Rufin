use super::*;

pub(super) fn map_reqwest_error(mut error: reqwest::Error) -> ProviderError {
    if let Some(url) = error.url_mut() {
        redact_subsonic_query(url);
    }
    let message = error.to_string();
    if message.to_lowercase().contains("certificate") || message.to_lowercase().contains("tls") {
        ProviderError::Tls(message)
    } else if error.is_connect() || error.is_request() || error.is_timeout() {
        ProviderError::Network(message)
    } else if let Some(status) = error.status() {
        ProviderError::Server {
            status: status.as_u16(),
            message,
        }
    } else {
        ProviderError::Other(message)
    }
}
pub(super) fn redact_subsonic_query(url: &mut Url) {
    let pairs = url
        .query_pairs()
        .map(|(key, value)| {
            let value = if matches!(key.as_ref(), "p" | "s" | "t") {
                "<redacted>".into()
            } else {
                value
            };
            (key.into_owned(), value.into_owned())
        })
        .collect::<Vec<_>>();
    url.query_pairs_mut().clear().extend_pairs(pairs);
}
pub(super) fn redacted_subsonic_url(url: &Url) -> String {
    let mut redacted = url.clone();
    redact_subsonic_query(&mut redacted);
    redacted.to_string()
}
pub(super) fn subsonic_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        lyrics: true,
        playback_reporting: true,
        playlist_mutations: true,
        playlist_delete: true,
        favorite_mutations: true,
        random_tracks: true,
        music_folders: true,
        folder_browsing: true,
        ..ProviderCapabilities::default()
    }
}
pub(super) fn raw_item_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
pub(super) fn raw_id_string(id: &SubsonicId) -> String {
    id.0.clone()
}
pub(super) fn playlist_entry_id(playlist_id: &PlaylistId, index: usize, track_id: &str) -> String {
    format!("{}:{index}:{track_id}", playlist_id.as_str())
}
pub(super) fn page<T>(items: Vec<T>, request: PagedRequest) -> PagedResponse<T> {
    let total = items.len();
    PagedResponse::new(
        items
            .into_iter()
            .skip(request.offset)
            .take(request.limit)
            .collect(),
        total,
    )
}
pub(super) fn current_year() -> u16 {
    let days_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or_default();
    year_from_unix_days(days_since_epoch)
}
pub(super) fn year_from_unix_days(mut days: u64) -> u16 {
    let mut year = 1970_u16;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            return year;
        }
        days -= days_in_year;
        year = year.saturating_add(1);
    }
}
pub(super) fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}
pub(super) fn random_salt() -> String {
    let mut bytes = [0_u8; SALT_BYTES];
    if getrandom::fill(&mut bytes).is_err() {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (seed.rotate_left(index as u32) & 0xff) as u8;
        }
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(super) fn stable_server_id(provider_id: &str, base_url: &str, username: &str) -> String {
    format!(
        "{:016x}",
        stable_hash(&format!("{provider_id}:{base_url}:{username}"))
    )
}
pub(super) fn stable_hash(input: &str) -> u64 {
    input.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
pub(super) fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}
pub(super) fn u16_from_option(value: Option<i32>) -> u16 {
    value.unwrap_or_default().clamp(0, i32::from(u16::MAX)) as u16
}
pub(super) fn u16_from_u32(value: Option<u32>) -> u16 {
    value.unwrap_or_default().min(u32::from(u16::MAX)) as u16
}
pub(super) fn favorite(value: &Option<serde_json::Value>) -> bool {
    value.as_ref().is_some_and(|value| match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        _ => false,
    })
}
pub(super) fn image_ref(
    provider: &SubsonicProvider,
    cover_art: Option<SubsonicId>,
) -> Option<ImageRef> {
    cover_art.map(|id| ImageRef::new(provider.id("cover", &id.0), None))
}
pub(super) fn folder_from_artist(provider: &SubsonicProvider, artist: SubsonicArtist) -> Folder {
    Folder {
        id: FolderId::new(provider.id("folder", artist.id.0.as_str())),
        name: artist.name.unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}
pub(super) fn folder_from_child(provider: &SubsonicProvider, child: SubsonicSong) -> Folder {
    Folder {
        id: FolderId::new(provider.id("folder", child.id.0.as_str())),
        name: child.title.unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}
pub(super) fn folder_from_directory(
    provider: &SubsonicProvider,
    directory: &SubsonicDirectory,
) -> Folder {
    Folder {
        id: FolderId::new(provider.id("folder", directory.id.0.as_str())),
        name: directory
            .name
            .clone()
            .unwrap_or_else(|| "Untitled Folder".to_string()),
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
fn clean_optional(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}
pub(super) fn album_from_dto(provider: &SubsonicProvider, album: SubsonicAlbum) -> Album {
    let raw_id = raw_id_string(&album.id);
    Album {
        id: AlbumId::new(provider.id("album", &raw_id)),
        title: album
            .title
            .or(album.name)
            .or(album.album)
            .unwrap_or_else(|| "Untitled Album".to_string()),
        artist: album.artist.unwrap_or_else(|| "Unknown Artist".to_string()),
        artist_id: album
            .artist_id
            .map(|id| ArtistId::new(provider.id("artist", &id.0))),
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: u16_from_option(album.year),
        release_date: album
            .year
            .map(|year| format!("{}-01-01", year.clamp(0, i32::from(u16::MAX)))),
        date_added: normalized_date(album.created),
        last_played: normalized_date(album.played),
        play_count: album
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: album
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        track_count: u16_from_u32(album.song_count),
        duration_seconds: album.duration.unwrap_or_default(),
        favorite: favorite(&album.starred),
        color_seed: color_seed(&raw_id),
        image_ref: image_ref(provider, album.cover_art),
        genres: genres_from_item(album.genre, album.genres),
        release_types: normalize_release_types(album.release_types),
        is_compilation: album.is_compilation,
        musicbrainz_album_id: clean_optional(album.musicbrainz_album_id),
        musicbrainz_release_group_id: None,
    }
}
pub(super) fn track_from_dto(provider: &SubsonicProvider, song: SubsonicSong) -> Track {
    let raw_id = raw_id_string(&song.id);
    let album_id = song
        .album_id
        .as_ref()
        .or(song.parent.as_ref())
        .map(raw_id_string)
        .unwrap_or_else(|| raw_id.clone());
    let source_format = source_format_from_song(
        song.suffix.as_deref(),
        song.content_type.as_deref(),
        song.path.as_deref(),
    );
    Track {
        id: TrackId::new(provider.id("track", &raw_id)),
        album_id: AlbumId::new(provider.id("album", &album_id)),
        title: song.title.unwrap_or_else(|| "Untitled Track".to_string()),
        artist: song.artist.unwrap_or_else(|| "Unknown Artist".to_string()),
        artist_id: song
            .artist_id
            .map(|id| ArtistId::new(provider.id("artist", &id.0))),
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: song.album.unwrap_or_else(|| "Unknown Album".to_string()),
        year: u16_from_option(song.year),
        release_date: song
            .year
            .map(|year| format!("{}-01-01", year.clamp(0, i32::from(u16::MAX)))),
        date_added: normalized_date(song.created),
        last_played: normalized_date(song.played),
        play_count: song
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: song
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        duration_seconds: song.duration.unwrap_or_default(),
        favorite: favorite(&song.starred),
        disc_number: u16_from_option(song.disc_number).max(1),
        track_number: u16_from_option(song.track).max(1),
        image_ref: image_ref(provider, song.cover_art),
        genres: genres_from_item(song.genre, song.genres),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: song.path,
        source_format,
        comment: song.comment.filter(|value| !value.trim().is_empty()),
        skip_count: None,
    }
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
pub(super) fn artist_from_dto(provider: &SubsonicProvider, artist: SubsonicArtist) -> Artist {
    let raw_id = raw_id_string(&artist.id);
    Artist {
        id: ArtistId::new(provider.id("artist", &raw_id)),
        name: artist.name.unwrap_or_else(|| "Unknown Artist".to_string()),
        album_count: artist.album_count.unwrap_or_default(),
        track_count: artist.song_count.unwrap_or_default(),
        favorite: favorite(&artist.starred),
        last_played: normalized_date(artist.played),
        play_count: artist
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: artist
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        musicbrainz_artist_id: None,
        image_ref: image_ref(provider, artist.cover_art),
    }
}
pub(super) fn genre_from_dto(provider: &SubsonicProvider, genre: SubsonicGenre) -> Genre {
    Genre {
        id: GenreId::new(provider.id("genre", &genre.value)),
        name: genre.value,
        album_count: genre.album_count.unwrap_or_default(),
        track_count: genre.song_count.unwrap_or_default(),
        image_refs: Vec::new(),
        image_ref: None,
    }
}
pub(super) fn normalized_date(value: Option<String>) -> Option<String> {
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
pub(super) fn playlist_from_dto(
    provider: &SubsonicProvider,
    playlist: SubsonicPlaylist,
) -> Playlist {
    let raw_id = raw_id_string(&playlist.id);
    Playlist {
        id: PlaylistId::new(provider.id("playlist", &raw_id)),
        name: playlist
            .name
            .unwrap_or_else(|| "Untitled Playlist".to_string()),
        track_count: playlist.song_count.unwrap_or_default(),
        duration_seconds: playlist.duration.unwrap_or_default(),
        image_refs: Vec::new(),
        image_ref: image_ref(provider, playlist.cover_art),
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SubsonicEmpty {}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicEnvelope<T> {
    #[serde(rename = "subsonic-response")]
    pub(super) response: SubsonicResponse<T>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicResponse<T> {
    pub(super) status: String,
    #[serde(default, rename = "type")]
    pub(super) server_type: Option<String>,
    #[serde(default)]
    pub(super) error: Option<SubsonicError>,
    #[serde(flatten)]
    pub(super) body: T,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicError {
    pub(super) message: String,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct AuthenticateBody {
    pub(super) user: SubsonicUser,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicUser {
    pub(super) username: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct AlbumListBody {
    #[serde(default, rename = "albumList2")]
    pub(super) album_list: AlbumList,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct AlbumList {
    #[serde(default)]
    pub(super) album: Vec<SubsonicAlbum>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct AlbumBody {
    pub(super) album: SubsonicAlbum,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SearchBody {
    #[serde(default, rename = "searchResult3")]
    pub(super) search_result: Option<SearchResult>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SearchResult {
    #[serde(default)]
    pub(super) album: Option<Vec<SubsonicAlbum>>,
    #[serde(default)]
    pub(super) artist: Option<Vec<SubsonicArtist>>,
    #[serde(default)]
    pub(super) song: Option<Vec<SubsonicSong>>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct MusicFoldersBody {
    #[serde(default, rename = "musicFolders")]
    pub(super) music_folders: MusicFolders,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct MusicFolders {
    #[serde(default, rename = "musicFolder")]
    pub(super) music_folder: Vec<SubsonicMusicFolder>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicMusicFolder {
    pub(super) id: SubsonicId,
    pub(super) name: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct IndexesBody {
    #[serde(default)]
    pub(super) indexes: Option<ArtistsIndex>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct MusicDirectoryBody {
    pub(super) directory: SubsonicDirectory,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicDirectory {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) parent: Option<SubsonicId>,
    #[serde(default)]
    pub(super) child: Vec<SubsonicSong>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArtistsBody {
    pub(super) artists: ArtistsIndex,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArtistsIndex {
    #[serde(default)]
    pub(super) index: Vec<ArtistIndex>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArtistIndex {
    #[serde(default)]
    pub(super) artist: Vec<SubsonicArtist>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct GenresBody {
    pub(super) genres: GenresList,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct GenresList {
    #[serde(default)]
    pub(super) genre: Vec<SubsonicGenre>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct PlaylistsBody {
    #[serde(default)]
    pub(super) playlists: Option<PlaylistsList>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct PlaylistsList {
    #[serde(default)]
    pub(super) playlist: Vec<SubsonicPlaylist>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct PlaylistBody {
    pub(super) playlist: SubsonicPlaylist,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SongBody {
    pub(super) song: SubsonicSong,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct RandomSongsBody {
    #[serde(default, rename = "randomSongs")]
    pub(super) random_songs: Option<SongsList>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SongsByGenreBody {
    #[serde(default, rename = "songsByGenre")]
    pub(super) songs_by_genre: Option<SongsList>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SongsList {
    #[serde(default)]
    pub(super) song: Vec<SubsonicSong>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct LyricsBody {
    #[serde(default)]
    pub(super) lyrics: Option<SubsonicLyrics>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicLyrics {
    #[serde(default)]
    pub(super) value: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicAlbum {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) album: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) artist: Option<String>,
    #[serde(default, rename = "artistId")]
    pub(super) artist_id: Option<SubsonicId>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default, rename = "songCount")]
    pub(super) song_count: Option<u32>,
    #[serde(default)]
    pub(super) duration: Option<u32>,
    #[serde(default)]
    pub(super) year: Option<i32>,
    #[serde(default)]
    pub(super) created: Option<String>,
    #[serde(default)]
    pub(super) played: Option<String>,
    #[serde(default, rename = "playCount")]
    pub(super) play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    pub(super) user_rating: Option<u32>,
    #[serde(default)]
    pub(super) genre: Option<String>,
    #[serde(default)]
    pub(super) genres: Vec<GenreName>,
    #[serde(default, rename = "releaseTypes")]
    pub(super) release_types: Vec<String>,
    #[serde(default, rename = "isCompilation")]
    pub(super) is_compilation: Option<bool>,
    #[serde(default, rename = "musicBrainzId")]
    pub(super) musicbrainz_album_id: Option<String>,
    #[serde(default)]
    pub(super) song: Vec<SubsonicSong>,
    #[serde(default)]
    pub(super) starred: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicSong {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) parent: Option<SubsonicId>,
    #[serde(default, rename = "isDir")]
    pub(super) is_dir: Option<bool>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) album: Option<String>,
    #[serde(default, rename = "albumId")]
    pub(super) album_id: Option<SubsonicId>,
    #[serde(default)]
    pub(super) artist: Option<String>,
    #[serde(default, rename = "artistId")]
    pub(super) artist_id: Option<SubsonicId>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default)]
    pub(super) duration: Option<u32>,
    #[serde(default)]
    pub(super) track: Option<i32>,
    #[serde(default)]
    pub(super) year: Option<i32>,
    #[serde(default)]
    pub(super) created: Option<String>,
    #[serde(default)]
    pub(super) played: Option<String>,
    #[serde(default, rename = "playCount")]
    pub(super) play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    pub(super) user_rating: Option<u32>,
    #[serde(default)]
    pub(super) genre: Option<String>,
    #[serde(default)]
    pub(super) comment: Option<String>,
    #[serde(default)]
    pub(super) genres: Vec<GenreName>,
    #[serde(default, rename = "discNumber")]
    pub(super) disc_number: Option<i32>,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(default)]
    pub(super) suffix: Option<String>,
    #[serde(default, rename = "contentType")]
    pub(super) content_type: Option<String>,
    #[serde(default)]
    pub(super) starred: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicArtist {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default, rename = "albumCount")]
    pub(super) album_count: Option<u32>,
    #[serde(default, rename = "songCount")]
    pub(super) song_count: Option<u32>,
    #[serde(default)]
    pub(super) played: Option<String>,
    #[serde(default, rename = "playCount")]
    pub(super) play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    pub(super) user_rating: Option<u32>,
    #[serde(default)]
    pub(super) starred: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicGenre {
    #[serde(default, alias = "name")]
    pub(super) value: String,
    #[serde(default, rename = "albumCount")]
    pub(super) album_count: Option<u32>,
    #[serde(default, rename = "songCount")]
    pub(super) song_count: Option<u32>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicPlaylist {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default, rename = "songCount")]
    pub(super) song_count: Option<u32>,
    #[serde(default)]
    pub(super) duration: Option<u32>,
    #[serde(default)]
    pub(super) entry: Option<Vec<SubsonicSong>>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct GenreName {
    pub(super) name: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SubsonicId(pub(super) String);
impl<'de> Deserialize<'de> for SubsonicId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(SubsonicIdVisitor)
    }
}
pub(super) struct SubsonicIdVisitor;
impl Visitor<'_> for SubsonicIdVisitor {
    type Value = SubsonicId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string or numeric Subsonic id")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value.to_string()))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value.to_string()))
    }
}
