use async_trait::async_trait;
use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rufin_core::{
    Album, AlbumId, Artist, ArtistCredit, ArtistId, Folder, FolderId, Genre, GenreId,
    HOME_SECTION_ITEM_LIMIT, HomeSection, HomeSectionKind, ImageRef, LocalFileFacts,
    LocalManifestCover, LocalManifestCoverKind, LocalManifestEntry, LocalManifestScan,
    LocalScanCounters, Playlist, PlaylistId, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_provider::{
    AlbumDetail, FolderDetail, GenreDetail, ImageBytes, ImageKind, ImageMetadata, ImageRequest,
    MusicProvider, PagedRequest, PagedResponse, PlayedFilter, PlaylistDetail, ProviderCapabilities,
    ProviderError, ProviderIdentity, ProviderResult, RandomTrackRequest, SearchResults,
    StreamDescriptor,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Instant, UNIX_EPOCH};
use url::Url;
use walkdir::WalkDir;

mod provider_impl;

use provider_impl::*;

#[cfg(test)]
mod tests;

pub const LOCAL_PROVIDER_ID: &str = "local";
const LOCAL_COVER_MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct LocalProvider {
    identity: ProviderIdentity,
    capabilities: ProviderCapabilities,
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
impl LocalProvider {
    pub fn from_root(root: PathBuf) -> ProviderResult<Self> {
        let root = normalize_root(root)?;
        let server = identity_for_root(&root);
        Self::from_roots_with_identity(vec![root], server)
    }

    pub fn from_roots(roots: Vec<PathBuf>) -> ProviderResult<Self> {
        let roots = normalize_roots(roots)?;
        let server = identity_for_roots(&roots);
        Self::from_normalized_roots_with_identity(roots, server)
    }

    pub fn from_roots_with_identity(
        roots: Vec<PathBuf>,
        server: ServerIdentity,
    ) -> ProviderResult<Self> {
        let roots = normalize_roots(roots)?;
        Self::from_normalized_roots_with_identity(roots, server)
    }

    fn from_normalized_roots_with_identity(
        roots: Vec<PathBuf>,
        server: ServerIdentity,
    ) -> ProviderResult<Self> {
        let (library, manifest_scan) = scan_library(&roots, Vec::new(), None);
        Ok(Self {
            identity: ProviderIdentity { server },
            capabilities: local_capabilities(),
            library,
            manifest_scan,
        })
    }

    pub fn from_roots_with_manifest_cache(
        roots: Vec<PathBuf>,
        server: ServerIdentity,
        cache: Vec<LocalManifestEntry>,
    ) -> ProviderResult<Self> {
        Self::from_roots_with_manifest_cache_and_progress(roots, server, cache, |_| {})
    }

    pub fn from_roots_with_manifest_cache_and_progress(
        roots: Vec<PathBuf>,
        server: ServerIdentity,
        cache: Vec<LocalManifestEntry>,
        mut progress: impl FnMut(LocalScanProgress),
    ) -> ProviderResult<Self> {
        let roots = normalize_roots(roots)?;
        let (library, manifest_scan) = scan_library(&roots, cache, Some(&mut progress));
        Ok(Self {
            identity: ProviderIdentity { server },
            capabilities: local_capabilities(),
            library,
            manifest_scan,
        })
    }

    pub fn from_server(server: ServerIdentity) -> ProviderResult<Self> {
        let root = normalize_root(PathBuf::from(&server.base_url))?;
        let (library, manifest_scan) = scan_library(&[root], Vec::new(), None);
        Ok(Self {
            identity: ProviderIdentity { server },
            capabilities: local_capabilities(),
            library,
            manifest_scan,
        })
    }

    pub fn manifest_scan(&self) -> &LocalManifestScan {
        &self.manifest_scan
    }

    pub fn identity_for_root(root: impl AsRef<Path>) -> ProviderResult<ServerIdentity> {
        let root = normalize_root(root.as_ref().to_path_buf())?;
        Ok(identity_for_root(&root))
    }

    pub fn cover_item_bytes(
        item_id: &str,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> ProviderResult<ImageBytes> {
        let cover = local_cover_from_item_id(item_id).ok_or(ProviderError::NotFound)?;
        let roots = normalize_roots(roots.into_iter().collect())?;
        if !local_cover_is_in_roots(&cover, &roots) {
            return Err(ProviderError::NotFound);
        }
        image_bytes_for_local_cover(&cover)
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

    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        _music_folder_id: Option<&rufin_core::MusicFolderId>,
    ) -> ProviderResult<FolderDetail> {
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
            .ok_or(ProviderError::NotFound)?;
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
fn image_bytes_for_local_cover(cover: &LocalCover) -> ProviderResult<ImageBytes> {
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
fn embedded_cover_image_bytes(path: &Path) -> ProviderResult<ImageBytes> {
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
        bytes: picture_data_bounded(picture)?,
        content_type: picture.mime_type().map(ToString::to_string),
    })
}
fn read_cover_file_bounded(path: &Path) -> ProviderResult<Vec<u8>> {
    if fs::metadata(path)
        .map_err(|error| ProviderError::Other(error.to_string()))?
        .len()
        > LOCAL_COVER_MAX_BYTES as u64
    {
        return Err(ProviderError::Other(format!(
            "local cover exceeded {} MiB limit",
            bytes_to_mib(LOCAL_COVER_MAX_BYTES)
        )));
    }
    let file = fs::File::open(path).map_err(|error| ProviderError::Other(error.to_string()))?;
    read_bounded(file, LOCAL_COVER_MAX_BYTES, "local cover")
}
fn picture_data_bounded(picture: &Picture) -> ProviderResult<Vec<u8>> {
    let data = picture.data();
    if data.len() > LOCAL_COVER_MAX_BYTES {
        return Err(ProviderError::Other(format!(
            "embedded cover exceeded {} MiB limit",
            bytes_to_mib(LOCAL_COVER_MAX_BYTES)
        )));
    }
    Ok(data.to_vec())
}
fn read_bounded<R: Read>(mut reader: R, limit: usize, context: &str) -> ProviderResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| ProviderError::Other(error.to_string()))?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|length| length > limit)
        {
            return Err(ProviderError::Other(format!(
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
fn normalize_root(root: PathBuf) -> ProviderResult<PathBuf> {
    let expanded = if root.as_os_str().is_empty() {
        std::env::current_dir().map_err(|error| ProviderError::Other(error.to_string()))?
    } else {
        root
    };
    Ok(expanded.canonicalize().unwrap_or(expanded))
}
fn normalize_roots(roots: Vec<PathBuf>) -> ProviderResult<Vec<PathBuf>> {
    let mut normalized = Vec::new();
    for root in roots {
        let root = normalize_root(root)?;
        if !normalized.iter().any(|candidate| candidate == &root) {
            normalized.push(root);
        }
    }
    Ok(normalized)
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
fn identity_for_roots(roots: &[PathBuf]) -> ServerIdentity {
    if roots.len() == 1 {
        return identity_for_root(&roots[0]);
    }
    let joined = roots
        .iter()
        .map(|root| root.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    ServerIdentity {
        id: ServerId::new(format!("local:server:{:016x}", stable_hash(&joined))),
        provider: LOCAL_PROVIDER_ID.to_string(),
        name: "Local".to_string(),
        base_url: joined,
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
        folder_browsing: true,
        ..ProviderCapabilities::default()
    }
}
fn scan_library(
    roots: &[PathBuf],
    cache: Vec<LocalManifestEntry>,
    mut progress: Option<&mut dyn FnMut(LocalScanProgress)>,
) -> (LocalLibrary, LocalManifestScan) {
    let mut counters = LocalScanCounters::default();
    let walk_started = Instant::now();
    let facts = discover_audio_files(roots, &mut counters, &mut progress);
    counters.filesystem_walk_elapsed_ms = elapsed_ms(walk_started);

    let compare_started = Instant::now();
    let mut cache_by_path = cache
        .into_iter()
        .map(|entry| (entry.facts.path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let mut scanned = Vec::with_capacity(facts.len());
    let mut entries = Vec::with_capacity(facts.len());
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
    counters.library_build_elapsed_ms = elapsed_ms(library_started);

    (
        library,
        LocalManifestScan {
            entries,
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

fn discover_audio_files(
    roots: &[PathBuf],
    counters: &mut LocalScanCounters,
    progress: &mut Option<&mut dyn FnMut(LocalScanProgress)>,
) -> Vec<LocalFileFacts> {
    let mut facts = Vec::new();
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
            if !entry.file_type().is_file() || !is_audio_file(entry.path()) {
                continue;
            }
            counters.audio_candidates = counters.audio_candidates.saturating_add(1);
            if let Some(file_facts) = local_file_facts_from_path(root, entry.path()) {
                facts.push(file_facts);
            }
            emit_local_scan_progress(progress, LocalScanStage::Walking, counters, 0, None, false);
        }
    }
    facts.sort_by(|left, right| left.path.cmp(&right.path));
    facts.dedup_by(|left, right| left.path == right.path);
    emit_local_scan_progress(progress, LocalScanStage::Walking, counters, 0, None, true);
    facts
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
        cover: current_cover.as_ref().map(local_cover_from_manifest),
        embedded_cover_path: None,
    };
    let entry = LocalManifestEntry {
        facts,
        track,
        album_artist: cached.album_artist,
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
        cover: scanned_track
            .cover
            .as_ref()
            .and_then(manifest_cover_from_local),
        metadata_hash: track_metadata_hash(&scanned_track.track, &scanned_track.album_artist),
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
    let comment = tag
        .and_then(|tag| tag.get_string(ItemKey::Comment))
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty());
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
            local_path: Some(path_text),
            source_format: path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            comment,
            skip_count: None,
        },
        album_artist,
        cover,
        embedded_cover_path,
    })
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
