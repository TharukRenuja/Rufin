fn map_reqwest_error(mut error: reqwest::Error) -> ProviderError {
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
fn redact_subsonic_query(url: &mut Url) {
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
fn redacted_subsonic_url(url: &Url) -> String {
    let mut redacted = url.clone();
    redact_subsonic_query(&mut redacted);
    redacted.to_string()
}
fn subsonic_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        lyrics: true,
        playback_reporting: true,
        playlist_mutations: true,
        favorite_mutations: true,
        random_tracks: true,
        music_folders: true,
        folder_browsing: true,
        ..ProviderCapabilities::default()
    }
}
fn raw_item_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
fn raw_id_string(id: &SubsonicId) -> String {
    id.0.clone()
}
fn playlist_entry_id(playlist_id: &PlaylistId, index: usize, track_id: &str) -> String {
    format!("{}:{index}:{track_id}", playlist_id.as_str())
}
fn page<T>(items: Vec<T>, request: PagedRequest) -> PagedResponse<T> {
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
fn current_year() -> u16 {
    let days_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or_default();
    year_from_unix_days(days_since_epoch)
}
fn year_from_unix_days(mut days: u64) -> u16 {
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
fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}
fn random_salt() -> String {
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
fn stable_server_id(provider_id: &str, base_url: &str, username: &str) -> String {
    format!(
        "{:016x}",
        stable_hash(&format!("{provider_id}:{base_url}:{username}"))
    )
}
fn stable_hash(input: &str) -> u64 {
    input.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}
fn u16_from_option(value: Option<i32>) -> u16 {
    value.unwrap_or_default().clamp(0, i32::from(u16::MAX)) as u16
}
fn u16_from_u32(value: Option<u32>) -> u16 {
    value.unwrap_or_default().min(u32::from(u16::MAX)) as u16
}
fn favorite(value: &Option<serde_json::Value>) -> bool {
    value.as_ref().is_some_and(|value| match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        _ => false,
    })
}
fn image_ref(provider: &SubsonicProvider, cover_art: Option<SubsonicId>) -> Option<ImageRef> {
    cover_art.map(|id| ImageRef::new(provider.id("cover", &id.0), None))
}
fn folder_from_artist(provider: &SubsonicProvider, artist: SubsonicArtist) -> Folder {
    Folder {
        id: FolderId::new(provider.id("folder", artist.id.0.as_str())),
        name: artist.name.unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}
fn folder_from_child(provider: &SubsonicProvider, child: SubsonicSong) -> Folder {
    Folder {
        id: FolderId::new(provider.id("folder", child.id.0.as_str())),
        name: child.title.unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}
fn folder_from_directory(provider: &SubsonicProvider, directory: &SubsonicDirectory) -> Folder {
    Folder {
        id: FolderId::new(provider.id("folder", directory.id.0.as_str())),
        name: directory
            .name
            .clone()
            .unwrap_or_else(|| "Untitled Folder".to_string()),
    }
}
fn sort_folders_by_name(folders: &mut [Folder]) {
    folders.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
}
fn genres_from_item(genre: Option<String>, genres: Vec<GenreName>) -> Vec<String> {
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
fn album_from_dto(provider: &SubsonicProvider, album: SubsonicAlbum) -> Album {
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
    }
}
fn track_from_dto(provider: &SubsonicProvider, song: SubsonicSong) -> Track {
    let raw_id = raw_id_string(&song.id);
    let album_id = song
        .album_id
        .as_ref()
        .or(song.parent.as_ref())
        .map(raw_id_string)
        .unwrap_or_else(|| raw_id.clone());
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
        local_path: song.path,
    }
}
fn artist_from_dto(provider: &SubsonicProvider, artist: SubsonicArtist) -> Artist {
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
        image_ref: image_ref(provider, artist.cover_art),
    }
}
fn genre_from_dto(provider: &SubsonicProvider, genre: SubsonicGenre) -> Genre {
    Genre {
        id: GenreId::new(provider.id("genre", &genre.value)),
        name: genre.value,
        album_count: genre.album_count.unwrap_or_default(),
        track_count: genre.song_count.unwrap_or_default(),
        image_ref: None,
    }
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
fn playlist_from_dto(provider: &SubsonicProvider, playlist: SubsonicPlaylist) -> Playlist {
    let raw_id = raw_id_string(&playlist.id);
    Playlist {
        id: PlaylistId::new(provider.id("playlist", &raw_id)),
        name: playlist
            .name
            .unwrap_or_else(|| "Untitled Playlist".to_string()),
        track_count: playlist.song_count.unwrap_or_default(),
        duration_seconds: playlist.duration.unwrap_or_default(),
        image_ref: image_ref(provider, playlist.cover_art),
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
struct SubsonicEmpty {}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicEnvelope<T> {
    #[serde(rename = "subsonic-response")]
    response: SubsonicResponse<T>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicResponse<T> {
    status: String,
    #[serde(default, rename = "type")]
    server_type: Option<String>,
    #[serde(default)]
    error: Option<SubsonicError>,
    #[serde(flatten)]
    body: T,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicError {
    message: String,
}
#[derive(Clone, Debug, Deserialize)]
struct AuthenticateBody {
    user: SubsonicUser,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicUser {
    username: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct AlbumListBody {
    #[serde(default, rename = "albumList2")]
    album_list: AlbumList,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct AlbumList {
    #[serde(default)]
    album: Vec<SubsonicAlbum>,
}
#[derive(Clone, Debug, Deserialize)]
struct AlbumBody {
    album: SubsonicAlbum,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct SearchBody {
    #[serde(default, rename = "searchResult3")]
    search_result: Option<SearchResult>,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct SearchResult {
    #[serde(default)]
    album: Option<Vec<SubsonicAlbum>>,
    #[serde(default)]
    artist: Option<Vec<SubsonicArtist>>,
    #[serde(default)]
    song: Option<Vec<SubsonicSong>>,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct MusicFoldersBody {
    #[serde(default, rename = "musicFolders")]
    music_folders: MusicFolders,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct MusicFolders {
    #[serde(default, rename = "musicFolder")]
    music_folder: Vec<SubsonicMusicFolder>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicMusicFolder {
    id: SubsonicId,
    name: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct IndexesBody {
    #[serde(default)]
    indexes: Option<ArtistsIndex>,
}
#[derive(Clone, Debug, Deserialize)]
struct MusicDirectoryBody {
    directory: SubsonicDirectory,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicDirectory {
    id: SubsonicId,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parent: Option<SubsonicId>,
    #[serde(default)]
    child: Vec<SubsonicSong>,
}
#[derive(Clone, Debug, Deserialize)]
struct ArtistsBody {
    artists: ArtistsIndex,
}
#[derive(Clone, Debug, Deserialize)]
struct ArtistsIndex {
    #[serde(default)]
    index: Vec<ArtistIndex>,
}
#[derive(Clone, Debug, Deserialize)]
struct ArtistIndex {
    #[serde(default)]
    artist: Vec<SubsonicArtist>,
}
#[derive(Clone, Debug, Deserialize)]
struct GenresBody {
    genres: GenresList,
}
#[derive(Clone, Debug, Deserialize)]
struct GenresList {
    #[serde(default)]
    genre: Vec<SubsonicGenre>,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct PlaylistsBody {
    #[serde(default)]
    playlists: Option<PlaylistsList>,
}
#[derive(Clone, Debug, Deserialize)]
struct PlaylistsList {
    #[serde(default)]
    playlist: Vec<SubsonicPlaylist>,
}
#[derive(Clone, Debug, Deserialize)]
struct PlaylistBody {
    playlist: SubsonicPlaylist,
}
#[derive(Clone, Debug, Deserialize)]
struct SongBody {
    song: SubsonicSong,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct RandomSongsBody {
    #[serde(default, rename = "randomSongs")]
    random_songs: Option<SongsList>,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct SongsByGenreBody {
    #[serde(default, rename = "songsByGenre")]
    songs_by_genre: Option<SongsList>,
}
#[derive(Clone, Debug, Deserialize)]
struct SongsList {
    #[serde(default)]
    song: Vec<SubsonicSong>,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct LyricsBody {
    #[serde(default)]
    lyrics: Option<SubsonicLyrics>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicLyrics {
    #[serde(default)]
    value: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicAlbum {
    id: SubsonicId,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default, rename = "artistId")]
    artist_id: Option<SubsonicId>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    played: Option<String>,
    #[serde(default, rename = "playCount")]
    play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    user_rating: Option<u32>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    genres: Vec<GenreName>,
    #[serde(default)]
    song: Vec<SubsonicSong>,
    #[serde(default)]
    starred: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicSong {
    id: SubsonicId,
    #[serde(default)]
    parent: Option<SubsonicId>,
    #[serde(default, rename = "isDir")]
    is_dir: Option<bool>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    album: Option<String>,
    #[serde(default, rename = "albumId")]
    album_id: Option<SubsonicId>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default, rename = "artistId")]
    artist_id: Option<SubsonicId>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    track: Option<i32>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    played: Option<String>,
    #[serde(default, rename = "playCount")]
    play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    user_rating: Option<u32>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    genres: Vec<GenreName>,
    #[serde(default, rename = "discNumber")]
    disc_number: Option<i32>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    starred: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicArtist {
    id: SubsonicId,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default, rename = "albumCount")]
    album_count: Option<u32>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
    #[serde(default)]
    played: Option<String>,
    #[serde(default, rename = "playCount")]
    play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    user_rating: Option<u32>,
    #[serde(default)]
    starred: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicGenre {
    #[serde(default, alias = "name")]
    value: String,
    #[serde(default, rename = "albumCount")]
    album_count: Option<u32>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
}
#[derive(Clone, Debug, Deserialize)]
struct SubsonicPlaylist {
    id: SubsonicId,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    entry: Option<Vec<SubsonicSong>>,
}
#[derive(Clone, Debug, Deserialize)]
struct GenreName {
    name: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct SubsonicId(String);
impl<'de> Deserialize<'de> for SubsonicId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(SubsonicIdVisitor)
    }
}
struct SubsonicIdVisitor;
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
