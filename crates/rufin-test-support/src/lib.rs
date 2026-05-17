use async_trait::async_trait;
use rufin_core::{
    Album, AlbumId, Artist, ArtistId, Genre, GenreId, HOME_SECTION_ITEM_LIMIT, HomeSection,
    HomeSectionKind, ImageRef, Playlist, PlaylistId, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_provider::{
    AlbumDetail, GenreDetail, ImageBytes, ImageKind, ImageMetadata, ImageRequest, MusicProvider,
    PagedRequest, PagedResponse, PlaylistDetail, PlaylistEntry, ProviderCapabilities,
    ProviderError, ProviderIdentity, ProviderResult, SearchResults,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FakeScale {
    Small,
    Large,
}

impl FakeScale {
    pub fn album_count(self) -> usize {
        match self {
            Self::Small => 240,
            Self::Large => 20_000,
        }
    }

    pub fn track_count(self) -> usize {
        match self {
            Self::Small => 2_400,
            Self::Large => 100_000,
        }
    }
}

#[derive(Clone, Debug)]
struct FakeLibrary {
    albums: Vec<Album>,
    tracks: Vec<Track>,
    artists: Vec<Artist>,
    album_artists: Vec<Artist>,
    genres: Vec<Genre>,
    playlists: Vec<Playlist>,
}

#[derive(Clone, Debug)]
pub struct FakeProvider {
    identity: ProviderIdentity,
    capabilities: ProviderCapabilities,
    library: FakeLibrary,
}

impl FakeProvider {
    pub fn new(scale: FakeScale) -> Self {
        Self {
            identity: ProviderIdentity {
                server: ServerIdentity {
                    id: ServerId::new("fake:server:local"),
                    provider: "fake".to_string(),
                    name: "Fake Library".to_string(),
                    base_url: "fake://local".to_string(),
                },
            },
            capabilities: ProviderCapabilities::default(),
            library: generate_fake_library(scale),
        }
    }

    pub fn album_count(&self) -> usize {
        self.library.albums.len()
    }

    pub fn track_count(&self) -> usize {
        self.library.tracks.len()
    }
}

#[async_trait(?Send)]
impl MusicProvider for FakeProvider {
    fn identity(&self) -> &ProviderIdentity {
        &self.identity
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    async fn home_sections(&self) -> ProviderResult<Vec<HomeSection>> {
        let album_section = |kind, offset| HomeSection {
            kind,
            albums: self
                .library
                .albums
                .iter()
                .skip(offset)
                .take(HOME_SECTION_ITEM_LIMIT)
                .cloned()
                .collect(),
            tracks: Vec::new(),
        };
        let track_section = |kind, offset| HomeSection {
            kind,
            albums: Vec::new(),
            tracks: self
                .library
                .tracks
                .iter()
                .skip(offset)
                .take(HOME_SECTION_ITEM_LIMIT)
                .cloned()
                .collect(),
        };
        let sections = vec![
            album_section(HomeSectionKind::Explore, 0),
            track_section(HomeSectionKind::MostPlayed, 0),
            album_section(HomeSectionKind::NewlyAdded, 24),
            track_section(HomeSectionKind::RecentlyPlayed, 24),
            album_section(HomeSectionKind::RecentlyReleased, 48),
        ];

        Ok(sections)
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
        let tracks: Vec<Track> = self
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
        Ok(page(&self.library.playlists, request))
    }

    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> ProviderResult<PlaylistDetail> {
        let playlist = self
            .library
            .playlists
            .iter()
            .find(|playlist| playlist.id == *playlist_id)
            .cloned()
            .ok_or(ProviderError::NotFound)?;
        let tracks: Vec<Track> = self
            .library
            .tracks
            .iter()
            .take(playlist.track_count as usize)
            .cloned()
            .collect();
        let entries = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| PlaylistEntry {
                entry_id: format!("fake-playlist-entry-{}-{index}", playlist.id.as_str()),
                track: track.clone(),
            })
            .collect();

        Ok(PlaylistDetail {
            playlist,
            tracks,
            entries,
        })
    }

    async fn genre_detail(&self, genre_id: &GenreId) -> ProviderResult<GenreDetail> {
        let genre = self
            .library
            .genres
            .iter()
            .find(|genre| genre.id == *genre_id)
            .cloned()
            .ok_or(ProviderError::NotFound)?;
        let albums = self
            .library
            .albums
            .iter()
            .filter(|album| album.genres.iter().any(|name| name == &genre.name))
            .cloned()
            .collect();
        let tracks = self
            .library
            .tracks
            .iter()
            .filter(|track| track.genres.iter().any(|name| name == &genre.name))
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

    async fn stream(&self, track_id: &TrackId) -> ProviderResult<rufin_provider::StreamDescriptor> {
        self.track(track_id).await?;
        Ok(rufin_provider::StreamDescriptor::new(format!(
            "fake://local/stream/{}",
            track_id.as_str()
        )))
    }

    async fn search(&self, query: &str) -> ProviderResult<SearchResults> {
        let query = query.to_lowercase();
        let albums = self
            .library
            .albums
            .iter()
            .filter(|album| {
                album.title.to_lowercase().contains(&query)
                    || album.artist.to_lowercase().contains(&query)
            })
            .take(25)
            .cloned()
            .collect();
        let tracks = self
            .library
            .tracks
            .iter()
            .filter(|track| {
                track.title.to_lowercase().contains(&query)
                    || track.album.to_lowercase().contains(&query)
                    || track.artist.to_lowercase().contains(&query)
            })
            .take(50)
            .cloned()
            .collect();
        let artists = self
            .library
            .artists
            .iter()
            .filter(|artist| artist.name.to_lowercase().contains(&query))
            .take(25)
            .cloned()
            .collect();
        let playlists = self
            .library
            .playlists
            .iter()
            .filter(|playlist| playlist.name.to_lowercase().contains(&query))
            .take(25)
            .cloned()
            .collect();

        Ok(SearchResults {
            albums,
            tracks,
            artists,
            playlists,
        })
    }

    async fn image_metadata(
        &self,
        item_id: &str,
        kind: ImageKind,
    ) -> ProviderResult<ImageMetadata> {
        Ok(ImageMetadata {
            item_id: item_id.to_string(),
            kind,
            tag: Some(format!("fake-{}", color_seed(item_id.len() as u32))),
            url: format!("fake://local/images/{item_id}"),
        })
    }

    async fn image_bytes(&self, request: ImageRequest) -> ProviderResult<ImageBytes> {
        let seed = color_seed(request.item_id.len() as u32);
        let red = (seed & 0xff) as u8;
        let green = ((seed >> 8) & 0xff) as u8;
        let blue = ((seed >> 16) & 0xff) as u8;
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{0}" height="{0}"><rect width="100%" height="100%" fill="#{1:02x}{2:02x}{3:02x}"/></svg>"##,
            request.size.max(1),
            red,
            green,
            blue
        );
        Ok(ImageBytes {
            bytes: svg.into_bytes(),
            content_type: Some("image/svg+xml".to_string()),
        })
    }
}

fn page<T: Clone>(items: &[T], request: PagedRequest) -> PagedResponse<T> {
    PagedResponse::new(
        items
            .iter()
            .skip(request.offset)
            .take(request.limit)
            .cloned()
            .collect(),
        items.len(),
    )
}

fn generate_fake_library(scale: FakeScale) -> FakeLibrary {
    let album_count = scale.album_count();
    let track_count = scale.track_count();
    let mut album_track_counts = vec![0_u16; album_count];

    for index in 0..track_count {
        album_track_counts[index % album_count] += 1;
    }

    let artists = ARTISTS
        .iter()
        .enumerate()
        .map(|(index, name)| Artist {
            id: ArtistId::fake(index + 1),
            name: (*name).to_string(),
            album_count: (album_count / ARTISTS.len()) as u32,
            track_count: (track_count / ARTISTS.len()) as u32,
            favorite: index.is_multiple_of(5),
            image_ref: Some(fake_image_ref("artist", index + 1)),
        })
        .collect::<Vec<_>>();

    let albums = (0..album_count)
        .map(|index| fake_album(index, album_track_counts[index], &artists))
        .collect::<Vec<_>>();

    let mut album_positions = vec![0_u16; album_count];
    let tracks = (0..track_count)
        .map(|index| {
            let album_index = index % album_count;
            album_positions[album_index] += 1;
            fake_track(index, &albums[album_index], album_positions[album_index])
        })
        .collect::<Vec<_>>();

    let genres = GENRES
        .iter()
        .enumerate()
        .map(|(index, name)| Genre {
            id: GenreId::fake(index + 1),
            name: (*name).to_string(),
            album_count: (album_count / GENRES.len()) as u32,
            track_count: (track_count / GENRES.len()) as u32,
            image_ref: Some(fake_image_ref("genre", index + 1)),
        })
        .collect::<Vec<_>>();

    let playlists = PLAYLISTS
        .iter()
        .enumerate()
        .map(|(index, name)| Playlist {
            id: PlaylistId::fake(index + 1),
            name: (*name).to_string(),
            track_count: 25 + index as u32,
            duration_seconds: 4_500 + index as u32 * 300,
            image_ref: Some(fake_image_ref("playlist", index + 1)),
        })
        .collect();

    FakeLibrary {
        albums,
        tracks,
        artists: artists.clone(),
        album_artists: artists,
        genres,
        playlists,
    }
}

fn fake_album(index: usize, track_count: u16, artists: &[Artist]) -> Album {
    let title_word = ALBUM_WORDS[index % ALBUM_WORDS.len()];
    let title_tail = ALBUM_TAILS[(index / ALBUM_WORDS.len()) % ALBUM_TAILS.len()];
    let artist = &artists[index % artists.len()];
    let year = 1980 + (index % 46) as u16;
    let duration_seconds = u32::from(track_count) * (175 + (index % 110) as u32);

    Album {
        id: AlbumId::fake(index + 1),
        title: format!("{title_word} {title_tail} {}", index + 1),
        artist: artist.name.clone(),
        artist_id: Some(artist.id.clone()),
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year,
        track_count,
        duration_seconds,
        favorite: index.is_multiple_of(17),
        color_seed: color_seed(index as u32),
        image_ref: Some(fake_image_ref("album", index + 1)),
        genres: vec![GENRES[index % GENRES.len()].to_string()],
    }
}

fn fake_track(index: usize, album: &Album, track_number: u16) -> Track {
    let title_word = TRACK_WORDS[index % TRACK_WORDS.len()];
    let title_tail = TRACK_TAILS[(index / TRACK_WORDS.len()) % TRACK_TAILS.len()];

    Track {
        id: TrackId::fake(index + 1),
        album_id: album.id.clone(),
        title: format!("{title_word} {title_tail} {track_number}"),
        artist: album.artist.clone(),
        artist_id: album.artist_id.clone(),
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: album.title.clone(),
        year: album.year,
        duration_seconds: 145 + (index % 210) as u32,
        favorite: index.is_multiple_of(23),
        disc_number: 1,
        track_number,
        image_ref: album.image_ref.clone(),
        genres: album.genres.clone(),
    }
}

fn fake_image_ref(kind: &str, index: usize) -> ImageRef {
    ImageRef {
        item_id: format!("fake:{kind}:{index}"),
        tag: Some(format!("fake-tag-{index}")),
    }
}

fn color_seed(value: u32) -> u32 {
    value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

const ALBUM_WORDS: &[&str] = &[
    "Blue", "Glass", "Signal", "Late", "Neon", "Velvet", "Static", "Golden", "Silver", "Hidden",
    "Open", "Electric",
];

const ALBUM_TAILS: &[&str] = &[
    "Rooms", "Weather", "Letters", "Harbors", "Fields", "Cities", "Mirrors", "Tides", "Maps",
    "Windows", "Gardens", "Stations",
];

const TRACK_WORDS: &[&str] = &[
    "First", "Quiet", "Long", "Night", "Soft", "Bright", "Heavy", "North", "South", "Broken",
    "Clear", "Slow",
];

const TRACK_TAILS: &[&str] = &[
    "Motion", "Pattern", "Street", "Answer", "Signal", "Memory", "Promise", "Line", "Frame",
    "Turn", "Light", "Wake",
];

const ARTISTS: &[&str] = &[
    "Astral Kin",
    "Blue Hour",
    "City Archive",
    "Distant Rooms",
    "Glass Method",
    "North Index",
    "Parallel Echo",
    "Signal Park",
    "Soft Circuit",
    "Velvet Relay",
];

const GENRES: &[&str] = &[
    "Alternative",
    "Electronic",
    "Folk",
    "Indie",
    "Jazz",
    "Pop",
    "Rock",
    "Soundtrack",
];

const PLAYLISTS: &[&str] = &[
    "Morning Queue",
    "Late Work",
    "Long Albums",
    "Favorites Draft",
    "Weekend",
];

#[cfg(test)]
mod tests {
    use futures_executor::block_on;
    use rufin_core::{AlbumId, HOME_SECTION_ITEM_LIMIT};
    use rufin_provider::{MusicProvider, PagedRequest, ProviderError};

    use super::{FakeProvider, FakeScale};

    #[test]
    fn large_dataset_matches_m0_targets() {
        let provider = FakeProvider::new(FakeScale::Large);

        assert_eq!(provider.album_count(), 20_000);
        assert_eq!(provider.track_count(), 100_000);
    }

    #[test]
    fn paged_album_and_track_reads_are_stable() {
        let provider = FakeProvider::new(FakeScale::Small);

        let albums = block_on(provider.albums(PagedRequest::new(10, 3))).expect("albums");
        let tracks = block_on(provider.tracks(PagedRequest::new(10, 3))).expect("tracks");

        assert_eq!(albums.total, 240);
        assert_eq!(albums.items.len(), 3);
        assert_eq!(albums.items[0].id, AlbumId::fake(11));
        assert_eq!(tracks.total, 2_400);
        assert_eq!(tracks.items.len(), 3);
    }

    #[test]
    fn home_sections_provide_enough_items_for_wide_rows() {
        let provider = FakeProvider::new(FakeScale::Small);

        let sections = block_on(provider.home_sections()).expect("home sections");

        assert_eq!(sections.len(), 5);
        assert_eq!(sections[0].albums.len(), HOME_SECTION_ITEM_LIMIT);
        assert_eq!(sections[1].tracks.len(), HOME_SECTION_ITEM_LIMIT);
        assert_eq!(sections[2].albums.len(), HOME_SECTION_ITEM_LIMIT);
        assert_eq!(sections[3].tracks.len(), HOME_SECTION_ITEM_LIMIT);
        assert_eq!(sections[4].albums.len(), HOME_SECTION_ITEM_LIMIT);
    }

    #[test]
    fn album_detail_returns_matching_tracks() {
        let provider = FakeProvider::new(FakeScale::Small);
        let detail = block_on(provider.album_detail(&AlbumId::fake(1))).expect("album detail");

        assert_eq!(detail.album.id, AlbumId::fake(1));
        assert_eq!(detail.tracks.len(), 10);
        assert!(
            detail
                .tracks
                .iter()
                .all(|track| track.album_id == detail.album.id)
        );
    }

    #[test]
    fn missing_album_returns_not_found() {
        let provider = FakeProvider::new(FakeScale::Small);
        let error = block_on(provider.album_detail(&AlbumId::new("missing"))).expect_err("error");

        assert!(matches!(error, ProviderError::NotFound));
    }
}
