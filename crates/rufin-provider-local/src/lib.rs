use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rufin_core::{
    Album, AlbumId, Artist, ArtistCredit, ArtistId, Genre, GenreId, HOME_SECTION_ITEM_LIMIT,
    HomeSection, HomeSectionKind, ImageRef, Playlist, PlaylistId, ServerId, ServerIdentity, Track,
    TrackId,
};
use rufin_provider::{
    AlbumDetail, GenreDetail, ImageBytes, ImageKind, ImageMetadata, ImageRequest, MusicProvider,
    PagedRequest, PagedResponse, PlayedFilter, PlaylistDetail, ProviderCapabilities, ProviderError,
    ProviderIdentity, ProviderResult, RandomTrackRequest, SearchResults, StreamDescriptor,
};
use url::Url;
use walkdir::WalkDir;

pub const LOCAL_PROVIDER_ID: &str = "local";

#[derive(Clone, Debug)]
pub struct LocalProvider {
    identity: ProviderIdentity,
    capabilities: ProviderCapabilities,
    library: LocalLibrary,
}

#[derive(Clone, Debug, Default)]
struct LocalLibrary {
    albums: Vec<Album>,
    tracks: Vec<Track>,
    artists: Vec<Artist>,
    album_artists: Vec<Artist>,
    genres: Vec<Genre>,
    covers: HashMap<String, LocalCover>,
}

#[derive(Clone, Debug)]
enum LocalCover {
    File(PathBuf),
    Embedded {
        path: PathBuf,
        content_type: Option<String>,
    },
}

#[derive(Clone, Debug)]
struct ScannedTrack {
    track: Track,
    album_artist: String,
    cover: Option<LocalCover>,
}

#[derive(Clone, Debug)]
struct AlbumAccumulator {
    album: Album,
    album_artist_keys: BTreeSet<String>,
    artist_keys: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
struct ArtistAccumulator {
    name: String,
    albums: BTreeSet<AlbumId>,
    tracks: BTreeSet<TrackId>,
}

#[derive(Clone, Debug, Default)]
struct GenreAccumulator {
    name: String,
    albums: BTreeSet<AlbumId>,
    tracks: BTreeSet<TrackId>,
}

impl LocalProvider {
    pub fn from_root(root: PathBuf) -> ProviderResult<Self> {
        let root = normalize_root(root)?;
        let server = identity_for_root(&root);
        Ok(Self {
            identity: ProviderIdentity { server },
            capabilities: local_capabilities(),
            library: scan_library(&root),
        })
    }

    pub fn from_server(server: ServerIdentity) -> ProviderResult<Self> {
        let root = normalize_root(PathBuf::from(&server.base_url))?;
        Ok(Self {
            identity: ProviderIdentity { server },
            capabilities: local_capabilities(),
            library: scan_library(&root),
        })
    }

    pub fn identity_for_root(root: impl AsRef<Path>) -> ProviderResult<ServerIdentity> {
        let root = normalize_root(root.as_ref().to_path_buf())?;
        Ok(identity_for_root(&root))
    }
}

#[async_trait(?Send)]
impl MusicProvider for LocalProvider {
    fn identity(&self) -> &ProviderIdentity {
        &self.identity
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    async fn home_sections(&self) -> ProviderResult<Vec<HomeSection>> {
        let albums = self
            .library
            .albums
            .iter()
            .take(HOME_SECTION_ITEM_LIMIT)
            .cloned()
            .collect::<Vec<_>>();
        let tracks = self
            .library
            .tracks
            .iter()
            .take(HOME_SECTION_ITEM_LIMIT)
            .cloned()
            .collect::<Vec<_>>();
        Ok(vec![
            HomeSection {
                kind: HomeSectionKind::Explore,
                albums: albums.clone(),
                tracks: Vec::new(),
            },
            HomeSection {
                kind: HomeSectionKind::NewlyAdded,
                albums: albums.clone(),
                tracks: Vec::new(),
            },
            HomeSection {
                kind: HomeSectionKind::RecentlyReleased,
                albums,
                tracks: Vec::new(),
            },
            HomeSection {
                kind: HomeSectionKind::MostPlayed,
                albums: Vec::new(),
                tracks: tracks.clone(),
            },
            HomeSection {
                kind: HomeSectionKind::RecentlyPlayed,
                albums: Vec::new(),
                tracks,
            },
        ])
    }

    async fn albums(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Album>> {
        Ok(page(&self.library.albums, request))
    }

    async fn album_detail(&self, album_id: &AlbumId) -> ProviderResult<AlbumDetail> {
        let album = self
            .library
            .albums
            .iter()
            .find(|album| album.id == *album_id)
            .cloned()
            .ok_or(ProviderError::NotFound)?;
        let tracks = self
            .library
            .tracks
            .iter()
            .filter(|track| track.album_id == *album_id)
            .cloned()
            .collect();
        Ok(AlbumDetail { album, tracks })
    }

    async fn tracks(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Track>> {
        Ok(page(&self.library.tracks, request))
    }

    async fn random_tracks(&self, request: RandomTrackRequest) -> ProviderResult<Vec<Track>> {
        if request.played_filter != PlayedFilter::All {
            return Err(ProviderError::Unsupported("random played filter"));
        }
        if let (Some(min_year), Some(max_year)) = (request.min_year, request.max_year)
            && min_year > max_year
        {
            return Err(ProviderError::Other(
                "minimum year cannot be greater than maximum year".to_string(),
            ));
        }

        let genre_id = request.genre_id.as_ref();
        let genre_name = request.genre_name.as_deref();
        let seed = stable_hash(&format!(
            "{}:{}:{}",
            request.min_year.unwrap_or_default(),
            request.max_year.unwrap_or_default(),
            genre_name.unwrap_or_default()
        ));
        let mut tracks = self
            .library
            .tracks
            .iter()
            .filter(|track| {
                request.min_year.is_none_or(|year| track.year >= year)
                    && request.max_year.is_none_or(|year| track.year <= year)
                    && genre_name.is_none_or(|name| {
                        track.genres.iter().any(|track_genre| track_genre == name)
                    })
                    && genre_id.is_none_or(|id| {
                        track
                            .genres
                            .iter()
                            .any(|track_genre| local_id::<GenreId>("genre", track_genre) == *id)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        tracks.sort_by_key(|track| stable_hash(&format!("{}:{seed}", track.id.as_str())));
        Ok(tracks
            .into_iter()
            .take(request.limit.clamp(1, 500))
            .collect())
    }

    async fn artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        Ok(page(&self.library.artists, request))
    }

    async fn album_artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        Ok(page(&self.library.album_artists, request))
    }

    async fn genres(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Genre>> {
        Ok(page(&self.library.genres, request))
    }

    async fn playlists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Playlist>> {
        let _unused = request;
        Ok(PagedResponse::new(Vec::new(), 0))
    }

    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> ProviderResult<PlaylistDetail> {
        let _unused = playlist_id;
        Err(ProviderError::NotFound)
    }

    async fn genre_detail(&self, genre_id: &GenreId) -> ProviderResult<GenreDetail> {
        let genre = self
            .library
            .genres
            .iter()
            .find(|genre| genre.id == *genre_id)
            .cloned()
            .ok_or(ProviderError::NotFound)?;
        let tracks = self
            .library
            .tracks
            .iter()
            .filter(|track| {
                track
                    .genres
                    .iter()
                    .any(|name| local_id::<GenreId>("genre", name) == genre.id)
            })
            .cloned()
            .collect::<Vec<_>>();
        let album_ids = tracks
            .iter()
            .map(|track| track.album_id.clone())
            .collect::<BTreeSet<_>>();
        let albums = self
            .library
            .albums
            .iter()
            .filter(|album| album_ids.contains(&album.id))
            .cloned()
            .collect();
        Ok(GenreDetail {
            genre,
            albums,
            tracks,
        })
    }

    async fn track(&self, track_id: &TrackId) -> ProviderResult<Track> {
        self.library
            .tracks
            .iter()
            .find(|track| track.id == *track_id)
            .cloned()
            .ok_or(ProviderError::NotFound)
    }

    async fn stream(&self, track_id: &TrackId) -> ProviderResult<StreamDescriptor> {
        let track = self.track(track_id).await?;
        let Some(local_path) = track.local_path else {
            return Err(ProviderError::NotFound);
        };
        let url = Url::from_file_path(local_path).map_err(|()| {
            ProviderError::Other("could not turn local track path into a file URI".to_string())
        })?;
        Ok(StreamDescriptor::new(url.to_string()))
    }

    async fn search(&self, query: &str) -> ProviderResult<SearchResults> {
        let query = normalize_search(query);
        if query.is_empty() {
            return Ok(SearchResults::default());
        }
        Ok(SearchResults {
            albums: self
                .library
                .albums
                .iter()
                .filter(|album| {
                    searchable_matches(&query, [&album.title, &album.artist].into_iter())
                })
                .take(50)
                .cloned()
                .collect(),
            tracks: self
                .library
                .tracks
                .iter()
                .filter(|track| {
                    searchable_matches(
                        &query,
                        [&track.title, &track.artist, &track.album].into_iter(),
                    )
                })
                .take(50)
                .cloned()
                .collect(),
            artists: self
                .library
                .artists
                .iter()
                .filter(|artist| searchable_matches(&query, [&artist.name].into_iter()))
                .take(50)
                .cloned()
                .collect(),
            playlists: Vec::new(),
        })
    }

    async fn image_metadata(
        &self,
        item_id: &str,
        kind: ImageKind,
    ) -> ProviderResult<ImageMetadata> {
        let _unused = kind;
        let cover = self
            .library
            .covers
            .get(item_id)
            .ok_or(ProviderError::NotFound)?;
        Ok(ImageMetadata {
            item_id: item_id.to_string(),
            kind: ImageKind::Primary,
            tag: None,
            url: cover_url(cover)?,
        })
    }

    async fn image_bytes(&self, request: ImageRequest) -> ProviderResult<ImageBytes> {
        let cover = self
            .library
            .covers
            .get(&request.item_id)
            .ok_or(ProviderError::NotFound)?;
        match cover {
            LocalCover::File(path) => Ok(ImageBytes {
                bytes: fs::read(path).map_err(|error| ProviderError::Other(error.to_string()))?,
                content_type: content_type_from_path(path),
            }),
            LocalCover::Embedded { path, content_type } => {
                let tagged = Probe::open(path)
                    .and_then(|probe| probe.read())
                    .map_err(|error| ProviderError::Other(error.to_string()))?;
                let picture = tagged
                    .primary_tag()
                    .or_else(|| tagged.first_tag())
                    .and_then(|tag| select_best_picture(tag.pictures()))
                    .or_else(|| select_best_picture_from_tags(tagged.tags()))
                    .ok_or(ProviderError::NotFound)?;
                Ok(ImageBytes {
                    bytes: picture.data().to_vec(),
                    content_type: content_type.clone(),
                })
            }
        }
    }
}

fn normalize_root(root: PathBuf) -> ProviderResult<PathBuf> {
    let expanded = if root.as_os_str().is_empty() {
        std::env::current_dir().map_err(|error| ProviderError::Other(error.to_string()))?
    } else {
        root
    };
    Ok(expanded.canonicalize().unwrap_or(expanded))
}

fn identity_for_root(root: &Path) -> ServerIdentity {
    let root_text = root.to_string_lossy().into_owned();
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Local")
        .to_string();
    ServerIdentity {
        id: ServerId::new(format!("local:server:{:016x}", stable_hash(&root_text))),
        provider: LOCAL_PROVIDER_ID.to_string(),
        name,
        base_url: root_text,
    }
}

fn local_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        favorites: false,
        lyrics: false,
        playback_reporting: false,
        playlist_mutations: false,
        favorite_mutations: false,
        auto_dj: false,
        playlists: false,
        random_tracks: true,
        ..ProviderCapabilities::default()
    }
}

fn scan_library(root: &Path) -> LocalLibrary {
    let mut scanned = WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| is_audio_file(path))
        .filter_map(read_track)
        .collect::<Vec<_>>();
    scanned.sort_by(|left, right| {
        left.track
            .album
            .to_lowercase()
            .cmp(&right.track.album.to_lowercase())
            .then(left.track.disc_number.cmp(&right.track.disc_number))
            .then(left.track.track_number.cmp(&right.track.track_number))
            .then(
                left.track
                    .title
                    .to_lowercase()
                    .cmp(&right.track.title.to_lowercase()),
            )
    });
    build_library(scanned)
}

fn read_track(path: PathBuf) -> Option<ScannedTrack> {
    let tagged_file = Probe::open(&path).and_then(|probe| probe.read()).ok();
    let tag = tagged_file
        .as_ref()
        .and_then(|file| file.primary_tag().or_else(|| file.first_tag()));
    let properties = tagged_file.as_ref().map(|file| file.properties());

    let fallback_title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown Title")
        .to_string();
    let parent_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown Album")
        .to_string();

    let title =
        tag_string(tag, |tag| tag.title().map(|value| value.to_string())).unwrap_or(fallback_title);
    let artist = tag_string(tag, |tag| tag.artist().map(|value| value.to_string()))
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let album =
        tag_string(tag, |tag| tag.album().map(|value| value.to_string())).unwrap_or(parent_name);
    let album_artist = tag
        .and_then(|tag| tag.get_string(&ItemKey::AlbumArtist))
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| artist.clone());
    let artist_names = artist_names(tag, &artist);
    let artist_credits = artist_names
        .iter()
        .map(|name| ArtistCredit {
            id: local_id("artist", name),
            name: name.clone(),
        })
        .collect::<Vec<_>>();
    let album_artist_credits = split_credit_names(&album_artist)
        .into_iter()
        .map(|name| ArtistCredit {
            id: local_id("artist", &name),
            name,
        })
        .collect::<Vec<_>>();
    let artist_id = artist_credits
        .first()
        .or_else(|| album_artist_credits.first())
        .map(|artist| artist.id.clone());
    let path_text = path.to_string_lossy().into_owned();
    let album_id = local_id(
        "album",
        &format!("{}:{}:{}", album_artist, album, album_grouping_path(&path)),
    );
    let genres = tag
        .and_then(|tag| tag.genre().map(|genre| split_credit_names(&genre)))
        .unwrap_or_default();
    let cover = embedded_cover(&path, tagged_file.as_ref(), tag)
        .or_else(|| path.parent().and_then(folder_cover).map(LocalCover::File));
    let year = tag
        .and_then(|tag| tag.year())
        .map(|year| year.min(u32::from(u16::MAX)) as u16)
        .unwrap_or_default();
    let duration_seconds = properties
        .map(|properties| properties.duration().as_secs().min(u64::from(u32::MAX)) as u32)
        .unwrap_or_default();

    Some(ScannedTrack {
        track: Track {
            id: local_id("track", &path_text),
            album_id,
            title,
            artist,
            artist_id,
            artist_credits,
            album_artist_credits,
            album,
            year,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds,
            favorite: false,
            disc_number: tag
                .and_then(|tag| tag.disk())
                .unwrap_or(1)
                .min(u32::from(u16::MAX)) as u16,
            track_number: tag
                .and_then(|tag| tag.track())
                .unwrap_or_default()
                .min(u32::from(u16::MAX)) as u16,
            image_ref: None,
            genres,
            local_path: Some(path_text),
        },
        album_artist,
        cover,
    })
}

fn build_library(scanned: Vec<ScannedTrack>) -> LocalLibrary {
    let mut albums = BTreeMap::<AlbumId, AlbumAccumulator>::new();
    let mut artists = BTreeMap::<ArtistId, ArtistAccumulator>::new();
    let mut album_artists = BTreeMap::<ArtistId, ArtistAccumulator>::new();
    let mut genres = BTreeMap::<GenreId, GenreAccumulator>::new();
    let mut covers = HashMap::new();
    let mut tracks = Vec::with_capacity(scanned.len());

    for mut scanned_track in scanned {
        let track = &mut scanned_track.track;
        let cover_ref = scanned_track.cover.map(|cover| {
            let cover_id = cover_id(&cover);
            covers.entry(cover_id.clone()).or_insert(cover);
            ImageRef::new(cover_id, None)
        });
        track.image_ref = cover_ref.clone();

        let album_entry =
            albums
                .entry(track.album_id.clone())
                .or_insert_with(|| AlbumAccumulator {
                    album: Album {
                        id: track.album_id.clone(),
                        title: track.album.clone(),
                        artist: scanned_track.album_artist.clone(),
                        artist_id: track
                            .album_artist_credits
                            .first()
                            .map(|artist| artist.id.clone()),
                        album_artist_credits: track.album_artist_credits.clone(),
                        artist_credits: track.artist_credits.clone(),
                        year: track.year,
                        release_date: track.release_date.clone(),
                        date_added: None,
                        last_played: None,
                        play_count: None,
                        user_rating: None,
                        track_count: 0,
                        duration_seconds: 0,
                        favorite: false,
                        color_seed: stable_hash(track.album_id.as_str()) as u32,
                        image_ref: cover_ref.clone(),
                        genres: Vec::new(),
                    },
                    album_artist_keys: BTreeSet::new(),
                    artist_keys: BTreeSet::new(),
                });
        if album_entry.album.image_ref.is_none() {
            album_entry.album.image_ref = cover_ref;
        }
        album_entry.album.track_count = album_entry.album.track_count.saturating_add(1);
        album_entry.album.duration_seconds = album_entry
            .album
            .duration_seconds
            .saturating_add(track.duration_seconds);
        if album_entry.album.year == 0 {
            album_entry.album.year = track.year;
        }
        merge_genres(&mut album_entry.album.genres, &track.genres);

        for artist in &track.artist_credits {
            album_entry
                .artist_keys
                .insert(artist.id.as_str().to_string());
            artists
                .entry(artist.id.clone())
                .or_insert_with(|| ArtistAccumulator {
                    name: artist.name.clone(),
                    ..ArtistAccumulator::default()
                })
                .tracks
                .insert(track.id.clone());
            artists
                .entry(artist.id.clone())
                .or_insert_with(|| ArtistAccumulator {
                    name: artist.name.clone(),
                    ..ArtistAccumulator::default()
                })
                .albums
                .insert(track.album_id.clone());
        }
        for artist in &track.album_artist_credits {
            album_entry
                .album_artist_keys
                .insert(artist.id.as_str().to_string());
            album_artists
                .entry(artist.id.clone())
                .or_insert_with(|| ArtistAccumulator {
                    name: artist.name.clone(),
                    ..ArtistAccumulator::default()
                })
                .albums
                .insert(track.album_id.clone());
        }
        for genre_name in &track.genres {
            let genre_id = local_id("genre", genre_name);
            let genre = genres.entry(genre_id).or_insert_with(|| GenreAccumulator {
                name: genre_name.clone(),
                ..GenreAccumulator::default()
            });
            genre.albums.insert(track.album_id.clone());
            genre.tracks.insert(track.id.clone());
        }
        tracks.push(track.clone());
    }

    let mut albums = albums
        .into_values()
        .map(|entry| entry.album)
        .collect::<Vec<_>>();
    albums.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then(left.artist.to_lowercase().cmp(&right.artist.to_lowercase()))
    });

    let mut artists = artists
        .into_iter()
        .map(|(id, artist)| artist_from_accumulator(id, artist))
        .collect::<Vec<_>>();
    artists.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

    let mut album_artists = album_artists
        .into_iter()
        .map(|(id, artist)| artist_from_accumulator(id, artist))
        .collect::<Vec<_>>();
    album_artists.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

    let mut genres = genres
        .into_iter()
        .map(|(id, genre)| Genre {
            id,
            name: genre.name,
            album_count: genre.albums.len().min(u32::MAX as usize) as u32,
            track_count: genre.tracks.len().min(u32::MAX as usize) as u32,
            image_ref: None,
        })
        .collect::<Vec<_>>();
    genres.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

    LocalLibrary {
        albums,
        tracks,
        artists,
        album_artists,
        genres,
        covers,
    }
}

fn artist_from_accumulator(id: ArtistId, artist: ArtistAccumulator) -> Artist {
    Artist {
        id,
        name: artist.name,
        album_count: artist.albums.len().min(u32::MAX as usize) as u32,
        track_count: artist.tracks.len().min(u32::MAX as usize) as u32,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        image_ref: None,
    }
}

fn page<T: Clone>(items: &[T], request: PagedRequest) -> PagedResponse<T> {
    let start = request.offset.min(items.len());
    let end = start.saturating_add(request.limit).min(items.len());
    PagedResponse::new(items[start..end].to_vec(), items.len())
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["mp3", "flac", "m4a", "wav", "ogg", "opus", "mp4", "mka"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn folder_cover(dir: &Path) -> Option<PathBuf> {
    ["cover", "folder", "front", "album"]
        .into_iter()
        .flat_map(|stem| {
            ["jpg", "jpeg", "png", "webp"].map(move |ext| dir.join(format!("{stem}.{ext}")))
        })
        .find(|path| path.is_file())
}

fn embedded_cover(
    path: &Path,
    tagged_file: Option<&lofty::file::TaggedFile>,
    tag: Option<&Tag>,
) -> Option<LocalCover> {
    let picture = tag
        .and_then(|tag| select_best_picture(tag.pictures()))
        .or_else(|| tagged_file.and_then(|file| select_best_picture_from_tags(file.tags())))?;
    Some(LocalCover::Embedded {
        path: path.to_path_buf(),
        content_type: picture.mime_type().map(ToString::to_string),
    })
}

fn select_best_picture(pictures: &[Picture]) -> Option<&Picture> {
    pictures
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
}

fn select_best_picture_from_tags(tags: &[Tag]) -> Option<&Picture> {
    tags.iter()
        .find_map(|tag| select_best_picture(tag.pictures()))
}

fn cover_id(cover: &LocalCover) -> String {
    let raw = match cover {
        LocalCover::File(path) => format!("file:{}", path.to_string_lossy()),
        LocalCover::Embedded { path, .. } => format!("embedded:{}", path.to_string_lossy()),
    };
    format!(
        "local:cover:{}",
        utf8_percent_encode(&raw, NON_ALPHANUMERIC)
    )
}

fn cover_url(cover: &LocalCover) -> ProviderResult<String> {
    match cover {
        LocalCover::File(path) | LocalCover::Embedded { path, .. } => Url::from_file_path(path)
            .map(|url| url.to_string())
            .map_err(|()| {
                ProviderError::Other("could not turn cover path into a file URI".to_string())
            }),
    }
}

fn content_type_from_path(path: &Path) -> Option<String> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => Some("image/jpeg".to_string()),
        Some("png") => Some("image/png".to_string()),
        Some("webp") => Some("image/webp".to_string()),
        _ => None,
    }
}

fn tag_string(tag: Option<&Tag>, read: impl FnOnce(&Tag) -> Option<String>) -> Option<String> {
    tag.and_then(read).filter(|value| !value.trim().is_empty())
}

fn artist_names(tag: Option<&Tag>, fallback: &str) -> Vec<String> {
    let tagged = tag
        .map(|tag| {
            tag.get_strings(&ItemKey::TrackArtists)
                .flat_map(split_credit_names)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if tagged.is_empty() {
        split_credit_names(fallback)
    } else {
        tagged
    }
}

fn split_credit_names(value: &str) -> Vec<String> {
    let names = value
        .split([';', '/'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if names.is_empty() { Vec::new() } else { names }
}

fn album_grouping_path(path: &Path) -> String {
    path.parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn merge_genres(target: &mut Vec<String>, source: &[String]) {
    for genre in source {
        if !target.iter().any(|candidate| candidate == genre) {
            target.push(genre.clone());
        }
    }
}

fn local_id<T>(kind: &str, value: &str) -> T
where
    T: From<String>,
{
    T::from(format!("local:{kind}:{:016x}", stable_hash(value)))
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn normalize_search(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn searchable_matches<'a>(query: &str, mut values: impl Iterator<Item = &'a String>) -> bool {
    values.any(|value| normalize_search(value).contains(query))
}

#[allow(dead_code)]
fn decode_cover_id(item_id: &str) -> Option<String> {
    item_id
        .strip_prefix("local:cover:")
        .and_then(|encoded| percent_decode_str(encoded).decode_utf8().ok())
        .map(|decoded| decoded.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_identity_is_stable_for_root() {
        let dir = tempfile::tempdir().expect("tempdir");

        let first = LocalProvider::identity_for_root(dir.path()).expect("identity");
        let second = LocalProvider::identity_for_root(dir.path()).expect("identity");

        assert_eq!(first, second);
        assert_eq!(first.provider, LOCAL_PROVIDER_ID);
    }

    #[tokio::test]
    async fn local_stream_uses_file_uri() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("track.mp3");
        fs::write(&path, []).expect("audio file");
        let provider = LocalProvider::from_root(dir.path().to_path_buf()).expect("provider");
        let track = provider
            .tracks(PagedRequest::new(0, 1))
            .await
            .expect("tracks")
            .items
            .into_iter()
            .next()
            .expect("track");

        let stream = provider.stream(&track.id).await.expect("stream");

        assert!(stream.uri().starts_with("file://"));
    }
}
