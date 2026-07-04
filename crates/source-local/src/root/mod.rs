use async_trait::async_trait;
use domain::{
    Album, AlbumId, Artist, ArtistCredit, ArtistId, Folder, FolderId, Genre, GenreId,
    HOME_SECTION_ITEM_LIMIT, HomeSection, HomeSectionKind, ImageRef, LocalCueTrackSource,
    LocalFileFacts, LocalManifestCover, LocalManifestCoverKind, LocalManifestEntry,
    LocalManifestScan, LocalScanCounters, Playlist, PlaylistId, SourceId, Track, TrackId,
};
use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use source::{
    AlbumDetail, FolderDetail, GeneratedTrackSeed, GeneratedTracksRequest, GenreDetail, ImageBytes,
    ImageKind, ImageMetadata, ImageRequest, MusicSource, PagedRequest, PagedResponse, PlayedFilter,
    PlaylistDetail, RandomTrackRequest, SearchResults, SourceError, SourceIdentity, SourceResult,
    StreamDescriptor,
};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Instant, UNIX_EPOCH};
use url::Url;
use walkdir::WalkDir;

mod cue;
mod source_impl;

use cue::*;
use source_impl::*;

#[cfg(test)]
mod tests;

pub const LOCAL_SOURCE_ID: &str = "local";
const LOCAL_COVER_MAX_BYTES: usize = 32 * 1024 * 1024;
const LOCAL_CUE_MAX_BYTES: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub struct LocalSource {
    identity: SourceIdentity,
    library: LocalLibrary,
    manifest_scan: LocalManifestScan,
}
#[derive(Clone, Debug, Default)]
struct LocalLibrary {
    roots: Vec<LocalFolderEntry>,
    folders: HashMap<FolderId, LocalFolderEntry>,
    albums: Vec<Album>,
    tracks: Vec<Track>,
    artists: Vec<Artist>,
    album_artists: Vec<Artist>,
    genres: Vec<Genre>,
    covers: HashMap<String, LocalCover>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalFolderEntry {
    folder: Folder,
    path: PathBuf,
    parent_id: Option<FolderId>,
}
#[derive(Clone, Debug)]
enum LocalCover {
    File {
        path: PathBuf,
        revision: Option<String>,
    },
    Embedded {
        path: PathBuf,
        bytes: Arc<[u8]>,
        content_type: Option<String>,
        revision: Option<String>,
    },
}
#[derive(Clone, Debug)]
struct ScannedTrack {
    track: Track,
    album_artist: String,
    musicbrainz_album_id: Option<String>,
    musicbrainz_release_group_id: Option<String>,
    cue_source: Option<LocalCueTrackSource>,
    cover: Option<LocalCover>,
    embedded_cover_path: Option<PathBuf>,
}
#[derive(Clone, Debug)]
struct LocalParseJob {
    index: usize,
    facts: LocalFileFacts,
    stale_entry: Option<LocalManifestEntry>,
}
#[derive(Clone, Debug)]
struct LocalParsedTrack {
    index: usize,
    facts: LocalFileFacts,
    stale_entry: Option<LocalManifestEntry>,
    scanned_track: Option<ScannedTrack>,
    parse_elapsed_ms: u64,
}
#[derive(Clone, Debug)]
struct LocalReusedTrack {
    track_id: TrackId,
    scanned_track: ScannedTrack,
    entry: LocalManifestEntry,
    artwork_changed: bool,
}
#[derive(Clone, Debug, Default)]
struct LocalDiscoveredFiles {
    audio: Vec<LocalFileFacts>,
    cues: Vec<LocalFileFacts>,
}
#[derive(Clone, Debug)]
struct AlbumAccumulator {
    album: Album,
    album_artist_keys: BTreeSet<String>,
    artist_keys: BTreeSet<String>,
    embedded_cover_path: Option<PathBuf>,
}
#[derive(Clone, Debug, Default)]
struct ArtistAccumulator {
    name: String,
    albums: BTreeSet<AlbumId>,
    tracks: BTreeSet<TrackId>,
    image_ref: Option<ImageRef>,
}
#[derive(Clone, Debug, Default)]
struct GenreAccumulator {
    name: String,
    albums: BTreeSet<AlbumId>,
    tracks: BTreeSet<TrackId>,
    duration_seconds: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalScanStage {
    Walking,
    ReadingTags,
    BuildingLibrary,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalScanProgress {
    pub stage: LocalScanStage,
    pub roots_walked: u64,
    pub directory_entries_visited: u64,
    pub audio_candidates: u64,
    pub processed_tracks: usize,
    pub total_tracks: Option<usize>,
}
impl LocalSource {
    pub fn from_root(root: PathBuf) -> SourceResult<Self> {
        let root = normalize_root(root)?;
        let source = identity_for_root(&root);
        Self::from_roots_with_identity(vec![root], source)
    }

    pub fn from_roots(roots: Vec<PathBuf>) -> SourceResult<Self> {
        let roots = normalize_roots(roots)?;
        let source = identity_for_roots(&roots);
        Self::from_normalized_roots_with_identity(roots, source)
    }

    pub fn from_roots_with_identity(
        roots: Vec<PathBuf>,
        source: SourceIdentity,
    ) -> SourceResult<Self> {
        let roots = normalize_roots(roots)?;
        Self::from_normalized_roots_with_identity(roots, source)
    }

    fn from_normalized_roots_with_identity(
        roots: Vec<PathBuf>,
        source: SourceIdentity,
    ) -> SourceResult<Self> {
        let (library, manifest_scan) = scan_library(&roots, Vec::new(), None);
        Ok(Self {
            identity: source,
            library,
            manifest_scan,
        })
    }

    pub fn from_roots_with_manifest_cache(
        roots: Vec<PathBuf>,
        source: SourceIdentity,
        cache: Vec<LocalManifestEntry>,
    ) -> SourceResult<Self> {
        Self::from_roots_with_manifest_cache_and_progress(roots, source, cache, |_| {})
    }

    pub fn from_roots_with_manifest_cache_and_progress(
        roots: Vec<PathBuf>,
        source: SourceIdentity,
        cache: Vec<LocalManifestEntry>,
        mut progress: impl FnMut(LocalScanProgress),
    ) -> SourceResult<Self> {
        let roots = normalize_roots(roots)?;
        let (library, manifest_scan) = scan_library(&roots, cache, Some(&mut progress));
        Ok(Self {
            identity: source,
            library,
            manifest_scan,
        })
    }

    pub fn from_source(source: SourceIdentity) -> SourceResult<Self> {
        let root = normalize_root(PathBuf::from(&source.base_url))?;
        let (library, manifest_scan) = scan_library(&[root], Vec::new(), None);
        Ok(Self {
            identity: source,
            library,
            manifest_scan,
        })
    }

    pub fn manifest_scan(&self) -> &LocalManifestScan {
        &self.manifest_scan
    }

    pub fn identity_for_root(root: impl AsRef<Path>) -> SourceResult<SourceIdentity> {
        let root = normalize_root(root.as_ref().to_path_buf())?;
        Ok(identity_for_root(&root))
    }

    pub fn cover_item_bytes(
        item_id: &str,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> SourceResult<ImageBytes> {
        let cover = local_cover_from_item_id(item_id).ok_or(SourceError::NotFound)?;
        let roots = normalize_roots(roots.into_iter().collect())?;
        if !local_cover_is_in_roots(&cover, &roots) {
            return Err(SourceError::NotFound);
        }
        image_bytes_for_local_cover(&cover)
    }
}
#[async_trait(?Send)]
impl MusicSource for LocalSource {
    fn identity(&self) -> &SourceIdentity {
        &self.identity
    }

    async fn home_sections(&self) -> SourceResult<Vec<HomeSection>> {
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

    async fn albums(&self, request: PagedRequest) -> SourceResult<PagedResponse<Album>> {
        Ok(page(&self.library.albums, request))
    }

    async fn album_detail(&self, album_id: &AlbumId) -> SourceResult<AlbumDetail> {
        let album = self
            .library
            .albums
            .iter()
            .find(|album| album.id == *album_id)
            .cloned()
            .ok_or(SourceError::NotFound)?;
        let tracks = self
            .library
            .tracks
            .iter()
            .filter(|track| track.album_id == *album_id)
            .cloned()
            .collect();
        Ok(AlbumDetail { album, tracks })
    }

    async fn tracks(&self, request: PagedRequest) -> SourceResult<PagedResponse<Track>> {
        Ok(page(&self.library.tracks, request))
    }

    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        _music_folder_id: Option<&domain::MusicFolderId>,
    ) -> SourceResult<FolderDetail> {
        let Some(folder_id) = folder_id else {
            return Ok(FolderDetail {
                folder: Folder {
                    id: FolderId::new("local:folder:root"),
                    name: "Folders".to_string(),
                },
                parent_id: None,
                folders: self
                    .library
                    .roots
                    .iter()
                    .map(|entry| entry.folder.clone())
                    .collect(),
                tracks: Vec::new(),
            });
        };

        let entry = self
            .library
            .folders
            .get(folder_id)
            .ok_or(SourceError::NotFound)?;
        let mut folders = self
            .library
            .folders
            .values()
            .filter(|candidate| candidate.parent_id.as_ref() == Some(folder_id))
            .map(|candidate| candidate.folder.clone())
            .collect::<Vec<_>>();
        folders.sort_by(folder_sort);
        let mut tracks = self
            .library
            .tracks
            .iter()
            .filter(|track| {
                track
                    .local_path
                    .as_deref()
                    .map(Path::new)
                    .and_then(Path::parent)
                    .is_some_and(|parent| parent == entry.path)
            })
            .cloned()
            .collect::<Vec<_>>();
        tracks.sort_by(|left, right| {
            left.disc_number
                .cmp(&right.disc_number)
                .then_with(|| left.track_number.cmp(&right.track_number))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(FolderDetail {
            folder: entry.folder.clone(),
            parent_id: entry.parent_id.clone(),
            folders,
            tracks,
        })
    }

    async fn random_tracks(&self, request: RandomTrackRequest) -> SourceResult<Vec<Track>> {
        if request.played_filter != PlayedFilter::All {
            return Err(SourceError::Unsupported("random played filter"));
        }
        if let (Some(min_year), Some(max_year)) = (request.min_year, request.max_year)
            && min_year > max_year
        {
            return Err(SourceError::Other(
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

    async fn generated_tracks(&self, request: GeneratedTracksRequest) -> SourceResult<Vec<Track>> {
        let limit = request.limit.clamp(1, 500);
        match request.seed {
            GeneratedTrackSeed::Track(track_id) => {
                let seed = self
                    .library
                    .tracks
                    .iter()
                    .find(|track| track.id == track_id)
                    .ok_or(SourceError::NotFound)?;
                let mut tracks = self
                    .random_tracks(RandomTrackRequest {
                        limit: limit.saturating_add(1),
                        min_year: None,
                        max_year: None,
                        genre_id: None,
                        genre_name: seed.genres.first().cloned(),
                        played_filter: PlayedFilter::All,
                    })
                    .await?;
                tracks.retain(|track| track.id != track_id);
                Ok(tracks.into_iter().take(limit).collect())
            }
            GeneratedTrackSeed::Album(album_id) => {
                let album = self
                    .library
                    .albums
                    .iter()
                    .find(|album| album.id == album_id)
                    .ok_or(SourceError::NotFound)?;
                let mut tracks = self
                    .random_tracks(RandomTrackRequest {
                        limit: limit.saturating_mul(2),
                        min_year: None,
                        max_year: None,
                        genre_id: None,
                        genre_name: album.genres.first().cloned(),
                        played_filter: PlayedFilter::All,
                    })
                    .await?;
                tracks.retain(|track| track.album_id != album_id);
                Ok(tracks.into_iter().take(limit).collect())
            }
            GeneratedTrackSeed::Artist(artist_id) => {
                let mut tracks = self
                    .library
                    .tracks
                    .iter()
                    .filter(|track| track.artist_id.as_ref() == Some(&artist_id))
                    .cloned()
                    .collect::<Vec<_>>();
                tracks.sort_by_key(|track| track.id.as_str().to_string());
                Ok(tracks.into_iter().take(limit).collect())
            }
            GeneratedTrackSeed::Genre { id, name } => {
                self.random_tracks(RandomTrackRequest {
                    limit,
                    min_year: None,
                    max_year: None,
                    genre_id: id,
                    genre_name: (!name.trim().is_empty()).then_some(name),
                    played_filter: PlayedFilter::All,
                })
                .await
            }
            GeneratedTrackSeed::Playlist(_) => Err(SourceError::Unsupported("playlist radio")),
        }
    }

    async fn artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>> {
        Ok(page(&self.library.artists, request))
    }

    async fn album_artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>> {
        Ok(page(&self.library.album_artists, request))
    }

    async fn genres(&self, request: PagedRequest) -> SourceResult<PagedResponse<Genre>> {
        Ok(page(&self.library.genres, request))
    }

    async fn playlists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Playlist>> {
        let _unused = request;
        Ok(PagedResponse::new(Vec::new(), 0))
    }

    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> SourceResult<PlaylistDetail> {
        let _unused = playlist_id;
        Err(SourceError::NotFound)
    }

    async fn genre_detail(&self, genre_id: &GenreId) -> SourceResult<GenreDetail> {
        let genre = self
            .library
            .genres
            .iter()
            .find(|genre| genre.id == *genre_id)
            .cloned()
            .ok_or(SourceError::NotFound)?;
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

    async fn track(&self, track_id: &TrackId) -> SourceResult<Track> {
        self.library
            .tracks
            .iter()
            .find(|track| track.id == *track_id)
            .cloned()
            .ok_or(SourceError::NotFound)
    }

    async fn stream(&self, track_id: &TrackId) -> SourceResult<StreamDescriptor> {
        let track = self.track(track_id).await?;
        let Some(local_path) = track.local_path else {
            return Err(SourceError::NotFound);
        };
        let url = Url::from_file_path(local_path).map_err(|()| {
            SourceError::Other("could not turn local track path into a file URI".to_string())
        })?;
        Ok(StreamDescriptor::new(url.to_string()))
    }

    async fn search(&self, query: &str) -> SourceResult<SearchResults> {
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

    async fn image_metadata(&self, item_id: &str, kind: ImageKind) -> SourceResult<ImageMetadata> {
        let _unused = kind;
        let cover = self
            .library
            .covers
            .get(item_id)
            .ok_or(SourceError::NotFound)?;
        Ok(ImageMetadata {
            item_id: item_id.to_string(),
            kind: ImageKind::Primary,
            tag: None,
            url: cover_url(cover)?,
        })
    }

    async fn image_bytes(&self, request: ImageRequest) -> SourceResult<ImageBytes> {
        let cover = self
            .library
            .covers
            .get(&request.item_id)
            .ok_or(SourceError::NotFound)?;
        image_bytes_for_local_cover(cover)
    }
}
fn local_cover_from_item_id(item_id: &str) -> Option<LocalCover> {
    let decoded = decode_cover_id(item_id)?;
    if let Some(path) = decoded.strip_prefix("file:") {
        return Some(LocalCover::File {
            path: PathBuf::from(path),
            revision: None,
        });
    }
    decoded
        .strip_prefix("embedded:")
        .map(|path| LocalCover::Embedded {
            path: PathBuf::from(path),
            bytes: Arc::<[u8]>::from([]),
            content_type: None,
            revision: None,
        })
}
fn local_cover_is_in_roots(cover: &LocalCover, roots: &[PathBuf]) -> bool {
    let path = match cover {
        LocalCover::File { path, .. } | LocalCover::Embedded { path, .. } => path,
    };
    let path = normalize_path_components(path);
    roots
        .iter()
        .map(|root| normalize_path_components(root))
        .any(|root| path.starts_with(root))
}
fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}
fn image_bytes_for_local_cover(cover: &LocalCover) -> SourceResult<ImageBytes> {
    match cover {
        LocalCover::File { path, .. } => Ok(ImageBytes {
            bytes: read_cover_file_bounded(path)?,
            content_type: content_type_from_path(path),
        }),
        LocalCover::Embedded {
            path,
            bytes,
            content_type,
            ..
        } if bytes.is_empty() => embedded_cover_image_bytes(path).map(|mut image| {
            if image.content_type.is_none() {
                image.content_type = content_type.clone();
            }
            image
        }),
        LocalCover::Embedded {
            bytes,
            content_type,
            ..
        } => Ok(ImageBytes {
            bytes: bytes.to_vec(),
            content_type: content_type.clone(),
        }),
    }
}
fn embedded_cover_image_bytes(path: &Path) -> SourceResult<ImageBytes> {
    let tagged = Probe::open(path)
        .and_then(|probe| probe.read())
        .map_err(|error| SourceError::Other(error.to_string()))?;
    let picture = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .and_then(|tag| select_best_picture(tag.pictures()))
        .or_else(|| select_best_picture_from_tags(tagged.tags()))
        .ok_or(SourceError::NotFound)?;
    Ok(ImageBytes {
        bytes: picture_data_bounded(picture)?,
        content_type: picture.mime_type().map(ToString::to_string),
    })
}
fn read_cover_file_bounded(path: &Path) -> SourceResult<Vec<u8>> {
    if fs::metadata(path)
        .map_err(|error| SourceError::Other(error.to_string()))?
        .len()
        > LOCAL_COVER_MAX_BYTES as u64
    {
        return Err(SourceError::Other(format!(
            "local cover exceeded {} MiB limit",
            bytes_to_mib(LOCAL_COVER_MAX_BYTES)
        )));
    }
    let file = fs::File::open(path).map_err(|error| SourceError::Other(error.to_string()))?;
    read_bounded(file, LOCAL_COVER_MAX_BYTES, "local cover")
}
fn read_cue_file_bounded(facts: &LocalFileFacts) -> SourceResult<Vec<u8>> {
    if facts.file_size > LOCAL_CUE_MAX_BYTES as u64 {
        return Err(SourceError::Other(format!(
            "local CUE sheet exceeded {} MiB limit",
            bytes_to_mib(LOCAL_CUE_MAX_BYTES)
        )));
    }
    let file =
        fs::File::open(&facts.path).map_err(|error| SourceError::Other(error.to_string()))?;
    read_bounded(file, LOCAL_CUE_MAX_BYTES, "local CUE sheet")
}
fn picture_data_bounded(picture: &Picture) -> SourceResult<Vec<u8>> {
    let data = picture.data();
    if data.len() > LOCAL_COVER_MAX_BYTES {
        return Err(SourceError::Other(format!(
            "embedded cover exceeded {} MiB limit",
            bytes_to_mib(LOCAL_COVER_MAX_BYTES)
        )));
    }
    Ok(data.to_vec())
}
fn read_bounded<R: Read>(mut reader: R, limit: usize, context: &str) -> SourceResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| SourceError::Other(error.to_string()))?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|length| length > limit)
        {
            return Err(SourceError::Other(format!(
                "{context} exceeded {} MiB limit",
                bytes_to_mib(limit)
            )));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}
fn bytes_to_mib(bytes: usize) -> usize {
    bytes / 1024 / 1024
}
fn normalize_root(root: PathBuf) -> SourceResult<PathBuf> {
    let expanded = if root.as_os_str().is_empty() {
        std::env::current_dir().map_err(|error| SourceError::Other(error.to_string()))?
    } else {
        root
    };
    Ok(expanded.canonicalize().unwrap_or(expanded))
}
fn normalize_roots(roots: Vec<PathBuf>) -> SourceResult<Vec<PathBuf>> {
    let mut normalized = Vec::new();
    for root in roots {
        let root = normalize_root(root)?;
        if !normalized.iter().any(|candidate| candidate == &root) {
            normalized.push(root);
        }
    }
    Ok(normalized)
}
fn identity_for_root(root: &Path) -> SourceIdentity {
    let root_text = root.to_string_lossy().into_owned();
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Local")
        .to_string();
    SourceIdentity {
        id: SourceId::new(format!("local:server:{:016x}", stable_hash(&root_text))),
        kind: LOCAL_SOURCE_ID.to_string(),
        name,
        base_url: root_text,
    }
}
fn identity_for_roots(roots: &[PathBuf]) -> SourceIdentity {
    if roots.len() == 1 {
        return identity_for_root(&roots[0]);
    }
    let joined = roots
        .iter()
        .map(|root| root.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    SourceIdentity {
        id: SourceId::new(format!("local:server:{:016x}", stable_hash(&joined))),
        kind: LOCAL_SOURCE_ID.to_string(),
        name: "Local".to_string(),
        base_url: joined,
    }
}

fn scan_library(
    roots: &[PathBuf],
    cache: Vec<LocalManifestEntry>,
    mut progress: Option<&mut dyn FnMut(LocalScanProgress)>,
) -> (LocalLibrary, LocalManifestScan) {
    let mut counters = LocalScanCounters::default();
    let walk_started = Instant::now();
    let discovered = discover_local_files(roots, &mut counters, &mut progress);
    counters.filesystem_walk_elapsed_ms = elapsed_ms(walk_started);

    let compare_started = Instant::now();
    let mut cache_by_path = cache
        .into_iter()
        .map(|entry| (entry.facts.path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let cue_scan = scan_cue_sheets(
        &discovered.cues,
        &discovered.audio,
        &mut cache_by_path,
        &mut counters,
    );
    let suppressed_audio_paths = cue_scan.suppressed_audio_paths;
    let cue_tracks = cue_scan.tracks;
    let facts = discovered
        .audio
        .into_iter()
        .filter(|facts| !suppressed_audio_paths.contains(&facts.path))
        .collect::<Vec<_>>();
    let mut scanned = Vec::with_capacity(facts.len());
    let mut entries = Vec::with_capacity(facts.len());
    let mut cue_track_sources = Vec::new();
    let mut changed_track_ids = Vec::new();
    let mut metadata_track_ids = Vec::new();
    let mut artwork_track_ids = Vec::new();
    let mut retained_track_ids = Vec::new();
    let mut dirty_album_ids = BTreeSet::new();
    let mut dirty_artist_ids = BTreeSet::new();
    let mut dirty_album_artist_ids = BTreeSet::new();
    let mut dirty_genre_names = BTreeSet::new();
    let mut library_changed = false;

    let total_tracks = facts.len();
    let mut reused_tracks = HashMap::new();
    let mut parse_jobs = Vec::new();
    emit_local_scan_progress(
        &mut progress,
        LocalScanStage::ReadingTags,
        &counters,
        0,
        Some(total_tracks),
        true,
    );
    for (index, facts) in facts.into_iter().enumerate() {
        let cached = cache_by_path.remove(&facts.path);
        let stale_entry = match cached {
            Some(cached) if local_file_facts_match(&cached.facts, &facts) => {
                let track_id = cached.track.id.clone();
                let (scanned_track, entry, artwork_changed) = reuse_manifest_track(facts, cached);
                counters.unchanged_reused = counters.unchanged_reused.saturating_add(1);
                counters.reused_track_rows = counters.reused_track_rows.saturating_add(1);
                reused_tracks.insert(
                    index,
                    LocalReusedTrack {
                        track_id,
                        scanned_track,
                        entry,
                        artwork_changed,
                    },
                );
                emit_local_scan_progress(
                    &mut progress,
                    LocalScanStage::ReadingTags,
                    &counters,
                    reused_tracks.len(),
                    Some(total_tracks),
                    false,
                );
                continue;
            }
            Some(cached) => {
                counters.changed_reparsed = counters.changed_reparsed.saturating_add(1);
                Some(cached)
            }
            None => {
                counters.new_parsed = counters.new_parsed.saturating_add(1);
                None
            }
        };
        library_changed = true;
        counters.tag_reads = counters.tag_reads.saturating_add(1);
        parse_jobs.push(LocalParseJob {
            index,
            facts,
            stale_entry,
        });
    }
    let parsed_tracks =
        parse_local_tracks(parse_jobs, reused_tracks.len(), total_tracks, |count| {
            emit_local_scan_progress(
                &mut progress,
                LocalScanStage::ReadingTags,
                &counters,
                count,
                Some(total_tracks),
                false,
            );
        });
    let mut parsed_tracks = parsed_tracks
        .into_iter()
        .map(|parsed| (parsed.index, parsed))
        .collect::<HashMap<_, _>>();
    for index in 0..total_tracks {
        if let Some(reused) = reused_tracks.remove(&index) {
            if reused.artwork_changed {
                counters.artwork_changed = counters.artwork_changed.saturating_add(1);
                artwork_track_ids.push(reused.track_id);
                mark_track_aggregate_dirty(
                    &reused.entry.track,
                    &mut dirty_album_ids,
                    &mut dirty_artist_ids,
                    &mut dirty_album_artist_ids,
                    &mut dirty_genre_names,
                );
                library_changed = true;
            } else {
                retained_track_ids.push(reused.track_id);
            }
            scanned.push(reused.scanned_track);
            entries.push(reused.entry);
            continue;
        }
        let Some(parsed) = parsed_tracks.remove(&index) else {
            continue;
        };
        counters.tag_parse_elapsed_ms = counters
            .tag_parse_elapsed_ms
            .saturating_add(parsed.parse_elapsed_ms);
        match parsed.scanned_track {
            Some(scanned_track) => {
                let entry = manifest_entry_for_scanned(&parsed.facts, &scanned_track);
                let track_changed = classify_reparsed_track(
                    parsed.stale_entry.as_ref(),
                    &entry,
                    &mut changed_track_ids,
                    &mut metadata_track_ids,
                    &mut artwork_track_ids,
                    &mut retained_track_ids,
                    &mut counters,
                );
                if track_changed {
                    if let Some(stale_entry) = &parsed.stale_entry {
                        mark_track_aggregate_dirty(
                            &stale_entry.track,
                            &mut dirty_album_ids,
                            &mut dirty_artist_ids,
                            &mut dirty_album_artist_ids,
                            &mut dirty_genre_names,
                        );
                    }
                    mark_track_aggregate_dirty(
                        &scanned_track.track,
                        &mut dirty_album_ids,
                        &mut dirty_artist_ids,
                        &mut dirty_album_artist_ids,
                        &mut dirty_genre_names,
                    );
                }
                entries.push(entry);
                scanned.push(scanned_track);
            }
            None => {
                counters.parse_failures = counters.parse_failures.saturating_add(1);
            }
        }
    }
    for cue_track in cue_tracks {
        let CueScannedTrack {
            facts,
            stale_entry,
            scanned_track,
        } = cue_track;
        if let Some(source) = &scanned_track.cue_source {
            cue_track_sources.push(source.clone());
        }
        let entry = manifest_entry_for_scanned(&facts, &scanned_track);
        let track_changed = classify_reparsed_track(
            stale_entry.as_ref(),
            &entry,
            &mut changed_track_ids,
            &mut metadata_track_ids,
            &mut artwork_track_ids,
            &mut retained_track_ids,
            &mut counters,
        );
        if track_changed {
            if let Some(stale_entry) = &stale_entry {
                mark_track_aggregate_dirty(
                    &stale_entry.track,
                    &mut dirty_album_ids,
                    &mut dirty_artist_ids,
                    &mut dirty_album_artist_ids,
                    &mut dirty_genre_names,
                );
            }
            mark_track_aggregate_dirty(
                &scanned_track.track,
                &mut dirty_album_ids,
                &mut dirty_artist_ids,
                &mut dirty_album_artist_ids,
                &mut dirty_genre_names,
            );
            library_changed = true;
        }
        entries.push(entry);
        scanned.push(scanned_track);
    }

    let deleted_entries = cache_by_path.into_values().collect::<Vec<_>>();
    let deleted_track_ids = deleted_entries
        .iter()
        .map(|entry| entry.track.id.clone())
        .collect::<Vec<_>>();
    for entry in &deleted_entries {
        mark_track_aggregate_dirty(
            &entry.track,
            &mut dirty_album_ids,
            &mut dirty_artist_ids,
            &mut dirty_album_artist_ids,
            &mut dirty_genre_names,
        );
    }
    let deleted_paths = deleted_entries
        .into_iter()
        .map(|entry| entry.facts.path)
        .collect::<Vec<_>>();
    counters.deleted = deleted_paths.len().min(u64::MAX as usize) as u64;
    if !deleted_paths.is_empty() {
        library_changed = true;
    }
    counters.manifest_compare_elapsed_ms = elapsed_ms(compare_started);

    emit_local_scan_progress(
        &mut progress,
        LocalScanStage::BuildingLibrary,
        &counters,
        scanned.len(),
        Some(total_tracks),
        true,
    );
    scanned.sort_by(local_scanned_track_sort);
    let library_started = Instant::now();
    let (root_entries, folders) = scan_folders(roots);
    let library = build_library(scanned, root_entries, folders);
    sync_manifest_covers_from_library(&library, &mut entries);
    counters.library_build_elapsed_ms = elapsed_ms(library_started);

    (
        library,
        LocalManifestScan {
            entries,
            cue_track_sources,
            deleted_paths,
            changed_track_ids,
            metadata_track_ids,
            artwork_track_ids,
            retained_track_ids,
            deleted_track_ids,
            dirty_album_ids: dirty_album_ids.into_iter().collect(),
            dirty_artist_ids: dirty_artist_ids.into_iter().collect(),
            dirty_album_artist_ids: dirty_album_artist_ids.into_iter().collect(),
            dirty_genre_names: dirty_genre_names.into_iter().collect(),
            counters,
            library_changed,
        },
    )
}
fn mark_track_aggregate_dirty(
    track: &Track,
    album_ids: &mut BTreeSet<AlbumId>,
    artist_ids: &mut BTreeSet<ArtistId>,
    album_artist_ids: &mut BTreeSet<ArtistId>,
    genre_names: &mut BTreeSet<String>,
) {
    album_ids.insert(track.album_id.clone());
    if let Some(artist_id) = &track.artist_id {
        artist_ids.insert(artist_id.clone());
    }
    for artist in &track.artist_credits {
        artist_ids.insert(artist.id.clone());
    }
    for artist in &track.album_artist_credits {
        album_artist_ids.insert(artist.id.clone());
    }
    for genre in &track.genres {
        if !genre.trim().is_empty() {
            genre_names.insert(genre.trim().to_string());
        }
    }
}

fn parse_local_tracks(
    jobs: Vec<LocalParseJob>,
    reused_count: usize,
    total_tracks: usize,
    mut progress: impl FnMut(usize),
) -> Vec<LocalParsedTrack> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let worker_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 4)
        .min(jobs.len());
    let chunk_size = jobs.len().div_ceil(worker_count).max(1);
    let (tx, rx) = mpsc::channel();
    thread::scope(|scope| {
        for chunk in jobs.chunks(chunk_size) {
            let tx = tx.clone();
            let chunk = chunk.to_vec();
            scope.spawn(move || {
                for job in chunk {
                    let _sent = tx.send(parse_local_track(job));
                }
            });
        }
        drop(tx);
        let mut parsed_tracks = Vec::new();
        for parsed in rx {
            parsed_tracks.push(parsed);
            progress((reused_count + parsed_tracks.len()).min(total_tracks));
        }
        parsed_tracks
    })
}

fn parse_local_track(job: LocalParseJob) -> LocalParsedTrack {
    let parse_started = Instant::now();
    let scanned_track = read_track(job.facts.path.clone());
    LocalParsedTrack {
        index: job.index,
        facts: job.facts,
        stale_entry: job.stale_entry,
        scanned_track,
        parse_elapsed_ms: elapsed_ms(parse_started),
    }
}

fn classify_reparsed_track(
    stale_entry: Option<&LocalManifestEntry>,
    entry: &LocalManifestEntry,
    changed_track_ids: &mut Vec<TrackId>,
    metadata_track_ids: &mut Vec<TrackId>,
    artwork_track_ids: &mut Vec<TrackId>,
    retained_track_ids: &mut Vec<TrackId>,
    counters: &mut LocalScanCounters,
) -> bool {
    let Some(stale_entry) = stale_entry else {
        changed_track_ids.push(entry.track.id.clone());
        return true;
    };
    let metadata_changed = stale_entry.metadata_hash != entry.metadata_hash;
    let search_changed = stale_entry.search_hash != entry.search_hash;
    let artwork_changed = stale_entry.cover != entry.cover;
    if search_changed {
        changed_track_ids.push(entry.track.id.clone());
        return true;
    }
    if metadata_changed {
        metadata_track_ids.push(entry.track.id.clone());
        return true;
    }
    if artwork_changed {
        counters.artwork_changed = counters.artwork_changed.saturating_add(1);
        artwork_track_ids.push(entry.track.id.clone());
        return true;
    }
    retained_track_ids.push(entry.track.id.clone());
    false
}

#[derive(Clone, Debug, Default)]
struct CueScan {
    tracks: Vec<CueScannedTrack>,
    suppressed_audio_paths: BTreeSet<PathBuf>,
}

#[derive(Clone, Debug)]
struct CueScannedTrack {
    facts: LocalFileFacts,
    stale_entry: Option<LocalManifestEntry>,
    scanned_track: ScannedTrack,
}

fn discover_local_files(
    roots: &[PathBuf],
    counters: &mut LocalScanCounters,
    progress: &mut Option<&mut dyn FnMut(LocalScanProgress)>,
) -> LocalDiscoveredFiles {
    let mut discovered = LocalDiscoveredFiles::default();
    emit_local_scan_progress(progress, LocalScanStage::Walking, counters, 0, None, true);
    for root in roots {
        counters.roots_walked = counters.roots_walked.saturating_add(1);
        for entry in WalkDir::new(root).follow_links(true).into_iter() {
            let Ok(entry) = entry else {
                continue;
            };
            counters.directory_entries_visited =
                counters.directory_entries_visited.saturating_add(1);
            if entry.file_type().is_file() && supported_cover_extension(entry.path()) {
                counters.artwork_candidates = counters.artwork_candidates.saturating_add(1);
            }
            if !entry.file_type().is_file() {
                continue;
            }
            if is_cue_file(entry.path()) {
                if let Some(file_facts) = local_file_facts_from_path(root, entry.path()) {
                    discovered.cues.push(file_facts);
                }
                continue;
            }
            if is_audio_file(entry.path()) {
                counters.audio_candidates = counters.audio_candidates.saturating_add(1);
                if let Some(file_facts) = local_file_facts_from_path(root, entry.path()) {
                    discovered.audio.push(file_facts);
                }
            }
            emit_local_scan_progress(progress, LocalScanStage::Walking, counters, 0, None, false);
        }
    }
    discovered
        .audio
        .sort_by(|left, right| left.path.cmp(&right.path));
    discovered
        .audio
        .dedup_by(|left, right| left.path == right.path);
    discovered
        .cues
        .sort_by(|left, right| left.path.cmp(&right.path));
    discovered
        .cues
        .dedup_by(|left, right| left.path == right.path);
    emit_local_scan_progress(progress, LocalScanStage::Walking, counters, 0, None, true);
    discovered
}

fn scan_cue_sheets(
    cue_facts: &[LocalFileFacts],
    audio_facts: &[LocalFileFacts],
    cache_by_path: &mut HashMap<PathBuf, LocalManifestEntry>,
    counters: &mut LocalScanCounters,
) -> CueScan {
    let audio_by_path = audio_facts
        .iter()
        .map(|facts| (facts.path.clone(), facts))
        .collect::<HashMap<_, _>>();
    let mut scan = CueScan::default();
    for cue_facts in cue_facts {
        let Ok(bytes) = read_cue_file_bounded(cue_facts) else {
            continue;
        };
        let cue_text = String::from_utf8_lossy(&bytes);
        let Some(sheet) = parse_cue_sheet(&cue_facts.path, &cue_text) else {
            continue;
        };
        counters.cue_sheets = counters.cue_sheets.saturating_add(1);
        let cue_revision =
            file_revision(&cue_facts.path).unwrap_or_else(|| cue_revision_from_facts(cue_facts));
        for cue_file in &sheet.files {
            let Some(audio_facts) = audio_by_path.get(&cue_file.path).copied() else {
                continue;
            };
            if reuse_cached_cue_file(
                cue_facts,
                audio_facts,
                cue_file,
                &cue_revision,
                cache_by_path,
                counters,
                &mut scan,
            ) {
                continue;
            }
            counters.cue_backing_reads = counters.cue_backing_reads.saturating_add(1);
            let Some(backing_track) = read_track(audio_facts.path.clone()) else {
                continue;
            };
            let backing_duration_ms = u64::from(backing_track.track.duration_seconds) * 1_000;
            if backing_duration_ms == 0 {
                continue;
            }
            scan.suppressed_audio_paths.insert(audio_facts.path.clone());
            for (position, cue_track) in cue_file.tracks.iter().enumerate() {
                let segment_start_ms = cue_track.index_start_ms;
                let segment_end_ms = cue_file
                    .tracks
                    .get(position + 1)
                    .map(|next| next.index_start_ms)
                    .unwrap_or(backing_duration_ms);
                if segment_end_ms <= segment_start_ms {
                    continue;
                }
                let facts = cue_track_facts(cue_facts, audio_facts, cue_track.number);
                let stale_entry = cache_by_path.remove(&facts.path);
                let source = cue_track_source(
                    cue_facts,
                    audio_facts,
                    &cue_revision,
                    cue_track.number,
                    segment_start_ms,
                    segment_end_ms,
                );
                let scanned_track = match stale_entry.as_ref() {
                    Some(entry) if local_file_facts_match(&entry.facts, &facts) => {
                        counters.cue_reused_tracks = counters.cue_reused_tracks.saturating_add(1);
                        reused_cue_manifest_track(entry, audio_facts, source)
                    }
                    _ => cue_scanned_track(
                        cue_facts,
                        audio_facts,
                        &sheet,
                        &backing_track,
                        cue_track,
                        source,
                    ),
                };
                counters.cue_tracks = counters.cue_tracks.saturating_add(1);
                scan.tracks.push(CueScannedTrack {
                    facts,
                    stale_entry,
                    scanned_track,
                });
            }
        }
    }
    scan
}

fn reuse_cached_cue_file(
    cue_facts: &LocalFileFacts,
    audio_facts: &LocalFileFacts,
    cue_file: &CueFile,
    cue_revision: &str,
    cache_by_path: &mut HashMap<PathBuf, LocalManifestEntry>,
    counters: &mut LocalScanCounters,
    scan: &mut CueScan,
) -> bool {
    let mut candidates = Vec::new();
    for (position, cue_track) in cue_file.tracks.iter().enumerate() {
        let segment_start_ms = cue_track.index_start_ms;
        let facts = cue_track_facts(cue_facts, audio_facts, cue_track.number);
        let Some(entry) = cache_by_path.get(&facts.path) else {
            return false;
        };
        if !local_file_facts_match(&entry.facts, &facts) {
            return false;
        }
        let segment_end_ms = cue_file
            .tracks
            .get(position + 1)
            .map(|next| next.index_start_ms)
            .unwrap_or_else(|| {
                segment_start_ms.saturating_add(u64::from(entry.track.duration_seconds) * 1_000)
            });
        if segment_end_ms <= segment_start_ms {
            continue;
        }
        let source = cue_track_source(
            cue_facts,
            audio_facts,
            cue_revision,
            cue_track.number,
            segment_start_ms,
            segment_end_ms,
        );
        candidates.push((facts, source));
    }
    if candidates.is_empty() {
        return false;
    }
    scan.suppressed_audio_paths.insert(audio_facts.path.clone());
    for (facts, source) in candidates {
        let Some(stale_entry) = cache_by_path.remove(&facts.path) else {
            continue;
        };
        let scanned_track = reused_cue_manifest_track(&stale_entry, audio_facts, source);
        counters.cue_tracks = counters.cue_tracks.saturating_add(1);
        counters.cue_reused_tracks = counters.cue_reused_tracks.saturating_add(1);
        scan.tracks.push(CueScannedTrack {
            facts,
            stale_entry: Some(stale_entry),
            scanned_track,
        });
    }
    true
}

fn reused_cue_manifest_track(
    entry: &LocalManifestEntry,
    audio_facts: &LocalFileFacts,
    source: LocalCueTrackSource,
) -> ScannedTrack {
    let mut track = entry.track.clone();
    track.local_path = Some(audio_facts.path.to_string_lossy().into_owned());
    ScannedTrack {
        track,
        album_artist: entry.album_artist.clone(),
        musicbrainz_album_id: entry.musicbrainz_album_id.clone(),
        musicbrainz_release_group_id: entry.musicbrainz_release_group_id.clone(),
        cue_source: Some(source),
        cover: entry.cover.as_ref().map(local_cover_from_manifest),
        embedded_cover_path: None,
    }
}

fn cue_scanned_track(
    cue_facts: &LocalFileFacts,
    audio_facts: &LocalFileFacts,
    sheet: &CueSheet,
    backing_track: &ScannedTrack,
    cue_track: &CueTrack,
    cue_source: LocalCueTrackSource,
) -> ScannedTrack {
    let album = sheet
        .album_title
        .clone()
        .unwrap_or_else(|| backing_track.track.album.clone());
    let album_artist = sheet
        .album_performer
        .clone()
        .unwrap_or_else(|| backing_track.album_artist.clone());
    let artist = cue_track
        .performer
        .clone()
        .unwrap_or_else(|| album_artist.clone());
    let artist_credits = split_credit_names(&artist)
        .iter()
        .map(|name| artist_credit(name, None))
        .collect::<Vec<_>>();
    let album_artist_credits = split_credit_names(&album_artist)
        .iter()
        .map(|name| artist_credit(name, None))
        .collect::<Vec<_>>();
    let artist_id = artist_credits
        .first()
        .or_else(|| album_artist_credits.first())
        .map(|artist| artist.id.clone());
    let album_id = local_album_id(
        &album_artist_credits,
        &album,
        &album_grouping_path(&cue_facts.path),
    );
    let cue_identity = format!("{}:{}", cue_facts.path.to_string_lossy(), cue_track.number);
    let duration_millis = cue_source
        .segment_end_ms
        .saturating_sub(cue_source.segment_start_ms)
        .max(1) as u64;
    let duration_seconds = (duration_millis / 1_000).min(u64::from(u32::MAX)).max(1) as u32;
    ScannedTrack {
        track: Track {
            id: local_id("track", &cue_identity),
            album_id,
            title: cue_track
                .title
                .clone()
                .unwrap_or_else(|| format!("Track {}", cue_track.number)),
            artist,
            artist_id,
            artist_credits,
            album_artist_credits,
            album,
            year: backing_track.track.year,
            release_date: backing_track.track.release_date.clone(),
            date_added: backing_track.track.date_added.clone(),
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds,
            favorite: false,
            disc_number: backing_track.track.disc_number.max(1),
            track_number: cue_track.number,
            image_ref: None,
            genres: backing_track.track.genres.clone(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: Some(audio_facts.path.to_string_lossy().into_owned()),
            source_format: backing_track.track.source_format.clone(),
            comment: None,
            skip_count: None,
            bpm: backing_track.track.bpm,
            moods: backing_track.track.moods.clone(),
        },
        album_artist,
        musicbrainz_album_id: backing_track.musicbrainz_album_id.clone(),
        musicbrainz_release_group_id: backing_track.musicbrainz_release_group_id.clone(),
        cue_source: Some(cue_source),
        cover: backing_track.cover.clone(),
        embedded_cover_path: backing_track.embedded_cover_path.clone(),
    }
}

fn cue_track_source(
    cue_facts: &LocalFileFacts,
    audio_facts: &LocalFileFacts,
    cue_revision: &str,
    track_number: u16,
    segment_start_ms: u64,
    segment_end_ms: u64,
) -> LocalCueTrackSource {
    let key = format!(
        "{}\u{1f}{}\u{1f}{track_number}",
        cue_facts.path.to_string_lossy(),
        audio_facts.path.to_string_lossy()
    );
    LocalCueTrackSource {
        source_object_id: format!("local:cue:{:016x}", stable_hash(&key)),
        track_id: local_id(
            "track",
            &format!("{}:{}", cue_facts.path.to_string_lossy(), track_number),
        ),
        source_path: audio_facts.path.to_string_lossy().into_owned(),
        root_path: audio_facts.root_path.to_string_lossy().into_owned(),
        relative_path: audio_facts.relative_path.clone(),
        cue_path: cue_facts.path.to_string_lossy().into_owned(),
        cue_revision: cue_revision.to_string(),
        cue_track_index: i64::from(track_number),
        segment_start_ms: segment_start_ms.min(i64::MAX as u64) as i64,
        segment_end_ms: segment_end_ms.min(i64::MAX as u64) as i64,
        sync_generation: 0,
    }
}

fn cue_track_facts(
    cue_facts: &LocalFileFacts,
    audio_facts: &LocalFileFacts,
    track_number: u16,
) -> LocalFileFacts {
    let path = PathBuf::from(format!(
        "{}#track={track_number:02}",
        cue_facts.path.to_string_lossy()
    ));
    let relative_path = format!("{}#track={track_number:02}", cue_facts.relative_path);
    LocalFileFacts {
        path,
        root_path: cue_facts.root_path.clone(),
        relative_path,
        file_size: cue_facts
            .file_size
            .wrapping_add(audio_facts.file_size)
            .wrapping_add(u64::from(track_number)),
        mtime_seconds: cue_facts.mtime_seconds.max(audio_facts.mtime_seconds),
        mtime_nanos: cue_facts.mtime_nanos.max(audio_facts.mtime_nanos),
        inode: None,
        device: None,
    }
}

fn cue_revision_from_facts(facts: &LocalFileFacts) -> String {
    format!(
        "{}:{}:{}",
        facts.file_size, facts.mtime_seconds, facts.mtime_nanos
    )
}
fn emit_local_scan_progress(
    progress: &mut Option<&mut dyn FnMut(LocalScanProgress)>,
    stage: LocalScanStage,
    counters: &LocalScanCounters,
    processed_tracks: usize,
    total_tracks: Option<usize>,
    force: bool,
) {
    let Some(progress) = progress.as_deref_mut() else {
        return;
    };
    if !force {
        let count = match stage {
            LocalScanStage::Walking => counters.audio_candidates,
            LocalScanStage::ReadingTags | LocalScanStage::BuildingLibrary => {
                processed_tracks.min(u64::MAX as usize) as u64
            }
        };
        if count == 0 || count % 25 != 0 {
            return;
        }
    }
    progress(LocalScanProgress {
        stage,
        roots_walked: counters.roots_walked,
        directory_entries_visited: counters.directory_entries_visited,
        audio_candidates: counters.audio_candidates,
        processed_tracks,
        total_tracks,
    });
}
fn reuse_manifest_track(
    facts: LocalFileFacts,
    cached: LocalManifestEntry,
) -> (ScannedTrack, LocalManifestEntry, bool) {
    let mut track = cached.track;
    track.local_path = Some(facts.path.to_string_lossy().into_owned());
    let current_cover = reused_cover_for_track(&facts.path, cached.cover.as_ref());
    let artwork_changed = current_cover.as_ref() != cached.cover.as_ref();
    let scanned_track = ScannedTrack {
        track: track.clone(),
        album_artist: cached.album_artist.clone(),
        musicbrainz_album_id: cached.musicbrainz_album_id.clone(),
        musicbrainz_release_group_id: cached.musicbrainz_release_group_id.clone(),
        cue_source: None,
        cover: current_cover.as_ref().map(local_cover_from_manifest),
        embedded_cover_path: None,
    };
    let entry = LocalManifestEntry {
        facts,
        track,
        album_artist: cached.album_artist,
        musicbrainz_album_id: cached.musicbrainz_album_id,
        musicbrainz_release_group_id: cached.musicbrainz_release_group_id,
        cover: current_cover,
        metadata_hash: cached.metadata_hash,
        search_hash: cached.search_hash,
    };
    (scanned_track, entry, artwork_changed)
}
fn reused_cover_for_track(
    path: &Path,
    cached: Option<&LocalManifestCover>,
) -> Option<LocalManifestCover> {
    if cached.is_some_and(|cover| cover.kind == LocalManifestCoverKind::Embedded) {
        return cached.cloned();
    }
    path.parent()
        .and_then(folder_cover)
        .map(local_file_cover)
        .and_then(|cover| manifest_cover_from_local(&cover))
}
fn manifest_entry_for_scanned(
    facts: &LocalFileFacts,
    scanned_track: &ScannedTrack,
) -> LocalManifestEntry {
    LocalManifestEntry {
        facts: facts.clone(),
        track: scanned_track.track.clone(),
        album_artist: scanned_track.album_artist.clone(),
        musicbrainz_album_id: scanned_track.musicbrainz_album_id.clone(),
        musicbrainz_release_group_id: scanned_track.musicbrainz_release_group_id.clone(),
        cover: scanned_track
            .cover
            .as_ref()
            .and_then(manifest_cover_from_local),
        metadata_hash: track_metadata_hash(
            &scanned_track.track,
            &scanned_track.album_artist,
            scanned_track.musicbrainz_album_id.as_deref(),
            scanned_track.musicbrainz_release_group_id.as_deref(),
        ),
        search_hash: track_search_hash(&scanned_track.track),
    }
}
fn local_scanned_track_sort(left: &ScannedTrack, right: &ScannedTrack) -> std::cmp::Ordering {
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
}
fn scan_folders(roots: &[PathBuf]) -> (Vec<LocalFolderEntry>, HashMap<FolderId, LocalFolderEntry>) {
    let mut entries = HashMap::<FolderId, LocalFolderEntry>::new();
    let mut root_entries = Vec::new();
    for root in roots {
        for entry in WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_dir())
        {
            let path = entry.path().to_path_buf();
            let folder = folder_for_path(&path);
            let parent_id = if path == *root {
                None
            } else {
                path.parent()
                    .filter(|parent| parent.starts_with(root))
                    .map(|parent| folder_for_path(parent).id)
            };
            let local_entry = LocalFolderEntry {
                folder: folder.clone(),
                path,
                parent_id,
            };
            if entry.path() == root {
                root_entries.push(local_entry.clone());
            }
            entries.insert(folder.id.clone(), local_entry);
        }
    }
    root_entries.sort_by(|left, right| folder_sort(&left.folder, &right.folder));
    (root_entries, entries)
}
fn folder_for_path(path: &Path) -> Folder {
    let path_text = path.to_string_lossy();
    Folder {
        id: local_id("folder", &path_text),
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| path_text.as_ref())
            .to_string(),
    }
}
fn folder_sort(left: &Folder, right: &Folder) -> std::cmp::Ordering {
    left.name
        .to_lowercase()
        .cmp(&right.name.to_lowercase())
        .then_with(|| left.id.cmp(&right.id))
}
fn read_track(path: PathBuf) -> Option<ScannedTrack> {
    let tagged_file = Probe::open(&path)
        .and_then(|probe| probe.options(local_scan_parse_options()).read())
        .ok();
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
        .and_then(|tag| tag.get_string(ItemKey::AlbumArtist))
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| artist.clone());
    let artist_names = artist_names(tag, &artist);
    let artist_mbids = aligned_mbids(&artist_names, tag_mbids(tag, ItemKey::MusicBrainzArtistId));
    let artist_credits = artist_names
        .iter()
        .zip(artist_mbids.iter())
        .map(|(name, mbid)| artist_credit(name, mbid.as_deref()))
        .collect::<Vec<_>>();
    let album_artist_names = split_credit_names(&album_artist);
    let album_artist_mbids = aligned_mbids(
        &album_artist_names,
        tag_mbids(tag, ItemKey::MusicBrainzReleaseArtistId),
    );
    let album_artist_credits = album_artist_names
        .iter()
        .zip(album_artist_mbids.iter())
        .map(|(name, mbid)| artist_credit(name, mbid.as_deref()))
        .collect::<Vec<_>>();
    let artist_id = artist_credits
        .first()
        .or_else(|| album_artist_credits.first())
        .map(|artist| artist.id.clone());
    let path_text = path.to_string_lossy().into_owned();
    let album_id = local_album_id(&album_artist_credits, &album, &album_grouping_path(&path));
    let genres = tag
        .and_then(|tag| tag.genre().map(|genre| split_credit_names(&genre)))
        .unwrap_or_default();
    let comment = tag
        .and_then(|tag| tag.get_string(ItemKey::Comment))
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty());
    let musicbrainz_album_id = tag.and_then(|tag| tag_mbid(tag, ItemKey::MusicBrainzReleaseId));
    let musicbrainz_release_group_id =
        tag.and_then(|tag| tag_mbid(tag, ItemKey::MusicBrainzReleaseGroupId));
    let musicbrainz_recording_id =
        tag.and_then(|tag| tag_mbid(tag, ItemKey::MusicBrainzRecordingId));
    let musicbrainz_release_track_id =
        tag.and_then(|tag| tag_mbid(tag, ItemKey::MusicBrainzTrackId));
    let bpm = tag_bpm(tag);
    let moods = tag_moods(tag);
    let cover = path.parent().and_then(folder_cover).map(local_file_cover);
    let embedded_cover_path = cover.is_none().then(|| path.clone());
    let year = tag
        .and_then(|tag| tag.date())
        .map(|date| date.year)
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
            musicbrainz_recording_id,
            musicbrainz_release_track_id,
            local_path: Some(path_text),
            source_format: path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            comment,
            skip_count: None,
            bpm,
            moods,
        },
        album_artist,
        musicbrainz_album_id,
        musicbrainz_release_group_id,
        cue_source: None,
        cover,
        embedded_cover_path,
    })
}

fn sync_manifest_covers_from_library(library: &LocalLibrary, entries: &mut [LocalManifestEntry]) {
    let cover_by_track = library
        .tracks
        .iter()
        .filter_map(|track| {
            track
                .image_ref
                .as_ref()
                .map(|image_ref| (track.id.clone(), image_ref.clone()))
        })
        .collect::<HashMap<_, _>>();
    for entry in entries {
        let Some(image_ref) = cover_by_track.get(&entry.track.id) else {
            entry.cover = None;
            continue;
        };
        entry.cover = manifest_cover_from_image_ref(image_ref);
    }
}

fn manifest_cover_from_image_ref(image_ref: &ImageRef) -> Option<LocalManifestCover> {
    let cover = local_cover_from_item_id(&image_ref.item_id)?;
    let mut manifest = manifest_cover_from_local(&cover)?;
    if let Some(tag) = &image_ref.tag {
        manifest.revision.clone_from(tag);
    }
    Some(manifest)
}

fn tag_mbid(tag: &Tag, key: ItemKey) -> Option<String> {
    tag_values(tag, key)
        .into_iter()
        .find_map(|value| clean_mbid(&value))
}

fn tag_mbids(tag: Option<&Tag>, key: ItemKey) -> Vec<String> {
    tag.map(|tag| {
        tag_values(tag, key)
            .into_iter()
            .flat_map(|value| split_credit_names(&value))
            .filter_map(|value| clean_mbid(&value))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default()
}

fn tag_bpm(tag: Option<&Tag>) -> Option<u16> {
    let tag = tag?;
    tag_values(tag, ItemKey::IntegerBpm)
        .into_iter()
        .chain(tag_values(tag, ItemKey::Bpm))
        .find_map(|value| clean_bpm(&value))
}

fn tag_moods(tag: Option<&Tag>) -> Vec<String> {
    tag.map(|tag| {
        tag_values(tag, ItemKey::Mood)
            .into_iter()
            .flat_map(|value| split_credit_names(&value))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
    })
    .unwrap_or_default()
}

fn tag_values(tag: &Tag, key: ItemKey) -> Vec<String> {
    tag.get_items(key)
        .filter_map(|item| item.value().text().map(ToString::to_string))
        .collect()
}

fn aligned_mbids(names: &[String], mbids: Vec<String>) -> Vec<Option<String>> {
    if names.len() == mbids.len() {
        mbids.into_iter().map(Some).collect()
    } else {
        names.iter().map(|_| None).collect()
    }
}

fn artist_credit(name: &str, musicbrainz_artist_id: Option<&str>) -> ArtistCredit {
    let clean_mbid = musicbrainz_artist_id.and_then(clean_mbid);
    let id = clean_mbid
        .as_deref()
        .map(|mbid| ArtistId::new(format!("local:artist:musicbrainz:{mbid}")))
        .unwrap_or_else(|| local_id("artist", &normalized_identity_value(name)));
    ArtistCredit {
        id,
        name: name.to_string(),
        musicbrainz_artist_id: clean_mbid,
    }
}

fn credit_identity_value(credits: &[ArtistCredit]) -> String {
    credits
        .iter()
        .map(|credit| credit.id.as_str())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn local_album_id(
    album_artist_credits: &[ArtistCredit],
    album: &str,
    grouping_key: &str,
) -> AlbumId {
    local_id(
        "album",
        &format!(
            "{}:{}:{}",
            credit_identity_value(album_artist_credits),
            normalized_identity_value(album),
            grouping_key
        ),
    )
}

fn normalized_identity_value(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn clean_mbid(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(value.to_string())
}

fn clean_bpm(value: &str) -> Option<u16> {
    let rounded = value.trim().parse::<f64>().ok()?.round();
    if !(1.0..=f64::from(u16::MAX)).contains(&rounded) {
        return None;
    }
    Some(rounded as u16)
}

fn local_scan_parse_options() -> ParseOptions {
    ParseOptions::new().read_cover_art(false)
}
fn local_file_facts_from_path(root: &Path, path: &Path) -> Option<LocalFileFacts> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(LocalFileFacts {
        path: path.to_path_buf(),
        root_path: root.to_path_buf(),
        relative_path: path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned(),
        file_size: metadata.len(),
        mtime_seconds: duration.as_secs().min(i64::MAX as u64) as i64,
        mtime_nanos: duration.subsec_nanos(),
        inode: metadata_inode(&metadata),
        device: metadata_device(&metadata),
    })
}
fn local_file_facts_match(left: &LocalFileFacts, right: &LocalFileFacts) -> bool {
    left.file_size == right.file_size
        && left.mtime_seconds == right.mtime_seconds
        && left.mtime_nanos == right.mtime_nanos
        && left.inode == right.inode
        && left.device == right.device
}
#[cfg(unix)]
fn metadata_inode(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}
#[cfg(not(unix))]
fn metadata_inode(_metadata: &fs::Metadata) -> Option<u64> {
    None
}
#[cfg(unix)]
fn metadata_device(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}
#[cfg(not(unix))]
fn metadata_device(_metadata: &fs::Metadata) -> Option<u64> {
    None
}
fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
