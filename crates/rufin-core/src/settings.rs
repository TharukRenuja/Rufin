use serde::{Deserialize, Deserializer, Serialize, de};

use crate::domain::{HomeBlockKind, HomeSectionKind};

pub const TRACK_TABLE_LAYOUT_VERSION: u8 = 2;
pub const LIBRARY_LIST_LAYOUT_VERSION: u8 = 2;
pub const QUEUE_LYRICS_LAYOUT_VERSION: u8 = 3;
pub const DEFAULT_DISCORD_CLIENT_ID: &str = "1505345384686419979";
const LEGACY_APPLICATION_DISPLAY_BYTES: &[u8] = &[102, 101, 105, 115, 104, 105, 110];

fn default_lyrics_panel_visible() -> bool {
    true
}

fn default_discord_client_id() -> String {
    DEFAULT_DISCORD_CLIENT_ID.to_string()
}

fn default_true() -> bool {
    true
}

fn default_volume() -> f64 {
    1.0
}

fn default_crossfade_seconds() -> u8 {
    5
}

fn default_narrow_layout_enabled() -> bool {
    true
}

fn default_narrow_layout_threshold() -> i32 {
    1_000
}

pub const MIN_NARROW_LAYOUT_THRESHOLD: i32 = 700;
pub const MAX_NARROW_LAYOUT_THRESHOLD: i32 = 3_400;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LeftSidebarMode {
    Full,
    Compact,
}

impl Default for LeftSidebarMode {
    fn default() -> Self {
        Self::Full
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RightSidebarMode {
    Hidden,
    Compact,
    Default,
    Comfortable,
    Spacious,
}

impl RightSidebarMode {
    pub fn is_visible(self) -> bool {
        !matches!(self, Self::Hidden)
    }

    pub fn fallback_visible() -> Self {
        Self::Default
    }
}

impl Default for RightSidebarMode {
    fn default() -> Self {
        Self::Comfortable
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LayoutProfile {
    #[serde(default)]
    pub left_sidebar: LeftSidebarMode,
    #[serde(default)]
    pub right_sidebar: RightSidebarMode,
    #[serde(default = "RightSidebarMode::fallback_visible")]
    pub last_visible_right_sidebar: RightSidebarMode,
}

impl LayoutProfile {
    pub fn new(left_sidebar: LeftSidebarMode, right_sidebar: RightSidebarMode) -> Self {
        let last_visible_right_sidebar = if right_sidebar.is_visible() {
            right_sidebar
        } else {
            RightSidebarMode::fallback_visible()
        };
        Self {
            left_sidebar,
            right_sidebar,
            last_visible_right_sidebar,
        }
    }

    fn sanitize(&mut self, fallback_visible: RightSidebarMode) {
        if !self.last_visible_right_sidebar.is_visible() {
            self.last_visible_right_sidebar = fallback_visible;
        }
        if self.right_sidebar.is_visible() {
            self.last_visible_right_sidebar = self.right_sidebar;
        }
    }
}

impl Default for LayoutProfile {
    fn default() -> Self {
        Self::new(LeftSidebarMode::Full, RightSidebarMode::Comfortable)
    }
}

fn default_narrow_layout_profile() -> LayoutProfile {
    LayoutProfile::new(LeftSidebarMode::Compact, RightSidebarMode::Default)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LayoutSettings {
    #[serde(default)]
    pub default_profile: LayoutProfile,
    #[serde(default = "default_narrow_layout_enabled")]
    pub narrow_enabled: bool,
    #[serde(default = "default_narrow_layout_threshold")]
    pub narrow_threshold: i32,
    #[serde(default = "default_narrow_layout_profile")]
    pub narrow_profile: LayoutProfile,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            default_profile: LayoutProfile::default(),
            narrow_enabled: true,
            narrow_threshold: default_narrow_layout_threshold(),
            narrow_profile: default_narrow_layout_profile(),
        }
    }
}

impl LayoutSettings {
    pub fn sanitize(&mut self) {
        self.narrow_threshold = self
            .narrow_threshold
            .clamp(MIN_NARROW_LAYOUT_THRESHOLD, MAX_NARROW_LAYOUT_THRESHOLD);
        self.default_profile.sanitize(RightSidebarMode::Comfortable);
        self.narrow_profile.sanitize(RightSidebarMode::Default);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum SidebarRouteItem {
    Home,
    Favorites,
    Albums,
    Tracks,
    Artists,
    AlbumArtists,
    Genres,
    Playlists,
}

impl SidebarRouteItem {
    pub fn all() -> [Self; 8] {
        [
            Self::Home,
            Self::Favorites,
            Self::Albums,
            Self::Tracks,
            Self::Artists,
            Self::AlbumArtists,
            Self::Genres,
            Self::Playlists,
        ]
    }

    fn default_visible(self) -> bool {
        !matches!(self, Self::AlbumArtists)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SidebarRouteItemSettings {
    pub item: SidebarRouteItem,
    pub visible: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SidebarSettings {
    #[serde(default = "default_sidebar_route_items")]
    pub route_items: Vec<SidebarRouteItemSettings>,
    #[serde(default = "default_true")]
    pub server_visible: bool,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            route_items: default_sidebar_route_items(),
            server_visible: true,
        }
    }
}

impl SidebarSettings {
    pub fn sanitize(&mut self) {
        let mut sanitized = Vec::with_capacity(SidebarRouteItem::all().len());
        for entry in &self.route_items {
            if !SidebarRouteItem::all().contains(&entry.item)
                || sanitized
                    .iter()
                    .any(|existing: &SidebarRouteItemSettings| existing.item == entry.item)
            {
                continue;
            }
            sanitized.push(entry.clone());
        }
        for item in SidebarRouteItem::all() {
            if !sanitized.iter().any(|entry| entry.item == item) {
                sanitized.push(SidebarRouteItemSettings {
                    item,
                    visible: item.default_visible(),
                });
            }
        }
        if !sanitized.iter().any(|entry| entry.visible) {
            if let Some(home) = sanitized
                .iter_mut()
                .find(|entry| entry.item == SidebarRouteItem::Home)
            {
                home.visible = true;
            }
        }
        self.route_items = sanitized;
    }
}

fn default_sidebar_route_items() -> Vec<SidebarRouteItemSettings> {
    SidebarRouteItem::all()
        .into_iter()
        .map(|item| SidebarRouteItemSettings {
            item,
            visible: item.default_visible(),
        })
        .collect()
}

const DEFAULT_TRACK_TABLE_COLUMNS: [TrackTableColumn; 5] = [
    TrackTableColumn::TrackNumber,
    TrackTableColumn::Title,
    TrackTableColumn::Album,
    TrackTableColumn::Year,
    TrackTableColumn::Favorite,
];

fn default_home_sections() -> Vec<HomeSectionKind> {
    vec![
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::NewlyAdded,
        HomeSectionKind::RecentlyPlayed,
        HomeSectionKind::RecentlyReleased,
    ]
}

fn default_home_blocks() -> Vec<HomeBlockKind> {
    vec![
        HomeBlockKind::Showcase,
        HomeBlockKind::Explore,
        HomeBlockKind::MostPlayed,
        HomeBlockKind::NewlyAdded,
        HomeBlockKind::RecentlyPlayed,
        HomeBlockKind::RecentlyReleased,
        HomeBlockKind::Genres,
    ]
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum DiscordDisplayType {
    #[serde(rename = "artist")]
    Artist,
    #[serde(rename = "application")]
    #[default]
    Application,
    #[serde(rename = "song")]
    Song,
}

impl<'de> Deserialize<'de> for DiscordDisplayType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "artist" => Ok(Self::Artist),
            "application" | "app" => Ok(Self::Application),
            "song" => Ok(Self::Song),
            legacy if legacy.as_bytes() == LEGACY_APPLICATION_DISPLAY_BYTES => {
                Ok(Self::Application)
            }
            other => Err(de::Error::unknown_variant(
                other,
                &["artist", "application", "song"],
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum DiscordLinkType {
    #[serde(rename = "last_fm")]
    LastFm,
    #[serde(rename = "musicbrainz")]
    MusicBrainz,
    #[serde(rename = "musicbrainz_last_fm")]
    MusicBrainzLastFm,
    #[serde(rename = "none")]
    #[default]
    None,
}

fn default_librefm_api_key() -> String {
    "rufin".to_string()
}

fn default_librefm_api_secret() -> String {
    "rufin".to_string()
}

fn default_now_playing_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioscrobblerScrobbleSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub session_key: String,
    #[serde(default = "default_now_playing_enabled")]
    pub now_playing_enabled: bool,
}

impl Default for AudioscrobblerScrobbleSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            username: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            session_key: String::new(),
            now_playing_enabled: true,
        }
    }
}

impl AudioscrobblerScrobbleSettings {
    fn sanitize(&mut self) {
        self.username = self.username.trim().to_string();
        self.api_key = self.api_key.trim().to_string();
        self.api_secret = self.api_secret.trim().to_string();
        self.session_key = self.session_key.trim().to_string();
    }

    fn with_librefm_defaults() -> Self {
        Self {
            api_key: default_librefm_api_key(),
            api_secret: default_librefm_api_secret(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListenBrainzScrobbleSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub user_token: String,
    #[serde(default = "default_now_playing_enabled")]
    pub now_playing_enabled: bool,
}

impl Default for ListenBrainzScrobbleSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            user_token: String::new(),
            now_playing_enabled: true,
        }
    }
}

impl ListenBrainzScrobbleSettings {
    fn sanitize(&mut self) {
        self.user_token = self.user_token.trim().to_string();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScrobblingSettings {
    #[serde(default)]
    pub lastfm: AudioscrobblerScrobbleSettings,
    #[serde(default)]
    pub librefm: AudioscrobblerScrobbleSettings,
    #[serde(default)]
    pub listenbrainz: ListenBrainzScrobbleSettings,
}

impl Default for ScrobblingSettings {
    fn default() -> Self {
        Self {
            lastfm: AudioscrobblerScrobbleSettings::default(),
            librefm: AudioscrobblerScrobbleSettings::with_librefm_defaults(),
            listenbrainz: ListenBrainzScrobbleSettings::default(),
        }
    }
}

impl ScrobblingSettings {
    pub fn sanitize(&mut self) {
        self.lastfm.sanitize();
        self.librefm.sanitize();
        if self.librefm.api_key.is_empty() {
            self.librefm.api_key = default_librefm_api_key();
        }
        if self.librefm.api_secret.is_empty() {
            self.librefm.api_secret = default_librefm_api_secret();
        }
        self.listenbrainz.sanitize();
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum TrackTableColumn {
    TrackNumber,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}

impl TrackTableColumn {
    pub fn all() -> [Self; 7] {
        [
            Self::TrackNumber,
            Self::Title,
            Self::Album,
            Self::Year,
            Self::Favorite,
            Self::Artist,
            Self::Duration,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TrackNumber => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Duration => "Duration",
            Self::Favorite => "Favorite",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TrackSortKey {
    TrackNumber,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}

impl TrackSortKey {
    pub fn all() -> [Self; 7] {
        [
            Self::TrackNumber,
            Self::Title,
            Self::Artist,
            Self::Album,
            Self::Year,
            Self::Duration,
            Self::Favorite,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TrackNumber => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Duration => "Duration",
            Self::Favorite => "Favorite",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum LibraryLayout {
    Row,
    Grid,
    Detail,
}

impl<'de> Deserialize<'de> for LibraryLayout {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "Row" | "row" | "Table" | "table" => Self::Row,
            "Detail" | "detail" => Self::Detail,
            "Grid" | "grid" => Self::Grid,
            _ => Self::Grid,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum LibraryListKey {
    Albums,
    Artists,
    AlbumArtists,
    Tracks,
    FavoriteTracks,
    Genres,
    AlbumDetailTracks,
    ArtistAlbums,
    ArtistTracks,
    GenreTracks,
    PlaylistTracks,
}

impl LibraryListKey {
    pub fn all() -> [Self; 11] {
        [
            Self::Albums,
            Self::Artists,
            Self::AlbumArtists,
            Self::Tracks,
            Self::FavoriteTracks,
            Self::Genres,
            Self::AlbumDetailTracks,
            Self::ArtistAlbums,
            Self::ArtistTracks,
            Self::GenreTracks,
            Self::PlaylistTracks,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::AlbumArtists => "Album artists",
            Self::Tracks => "Tracks",
            Self::FavoriteTracks => "Favorites",
            Self::Genres => "Genres",
            Self::AlbumDetailTracks => "Album tracks",
            Self::ArtistAlbums => "Artist albums",
            Self::ArtistTracks => "Artist tracks",
            Self::GenreTracks => "Genre tracks",
            Self::PlaylistTracks => "Playlist tracks",
        }
    }

    pub fn supports_layout(self, layout: LibraryLayout) -> bool {
        match layout {
            LibraryLayout::Detail => matches!(self, Self::Albums | Self::ArtistAlbums),
            LibraryLayout::Row | LibraryLayout::Grid => true,
        }
    }

    fn default_layout(self) -> LibraryLayout {
        match self {
            Self::Tracks
            | Self::FavoriteTracks
            | Self::AlbumDetailTracks
            | Self::ArtistTracks
            | Self::GenreTracks
            | Self::PlaylistTracks => LibraryLayout::Row,
            Self::Albums
            | Self::Artists
            | Self::AlbumArtists
            | Self::Genres
            | Self::ArtistAlbums => LibraryLayout::Grid,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum LibraryField {
    RowIndex,
    Image,
    Title,
    TitleMerged,
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
    TrackNumber,
    DiscNumber,
    SongCount,
    AlbumCount,
    Duration,
    Favorite,
}

impl LibraryField {
    pub fn title(self) -> &'static str {
        match self {
            Self::RowIndex => "#",
            Self::Image => "Image",
            Self::Title => "Title",
            Self::TitleMerged => "Title (merged)",
            Self::Artist => "Artist",
            Self::AlbumArtist => "Album artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::ReleaseDate => "Release date",
            Self::DateAdded => "Date added",
            Self::LastPlayed => "Last played",
            Self::PlayCount => "Play count",
            Self::UserRating => "Rating",
            Self::Genre => "Genre",
            Self::TrackNumber => "Track",
            Self::DiscNumber => "Disc",
            Self::SongCount => "Songs",
            Self::AlbumCount => "Albums",
            Self::Duration => "Duration",
            Self::Favorite => "Favorite",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryListSettings {
    pub layout: LibraryLayout,
    pub row_fields: Vec<LibraryField>,
    pub grid_fields: Vec<LibraryField>,
    pub detail_track_fields: Vec<LibraryField>,
    pub sort_key: LibraryField,
    pub descending: bool,
    #[serde(default)]
    pub layout_version: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryListSettingsEntry {
    pub key: LibraryListKey,
    pub settings: LibraryListSettings,
}

impl LibraryListSettings {
    pub fn for_key(key: LibraryListKey) -> Self {
        Self {
            layout: key.default_layout(),
            row_fields: default_row_fields(key),
            grid_fields: default_grid_fields(key),
            detail_track_fields: default_detail_track_fields(),
            sort_key: default_sort_key(key),
            descending: false,
            layout_version: LIBRARY_LIST_LAYOUT_VERSION,
        }
    }

    pub fn sanitize(&mut self, key: LibraryListKey) {
        if !key.supports_layout(self.layout) {
            self.layout = key.default_layout();
        }
        sanitize_required_fields(
            &mut self.row_fields,
            available_row_fields(key),
            default_row_fields(key),
        );
        ensure_usable_row_field(&mut self.row_fields, default_row_fields(key));
        sanitize_optional_fields(&mut self.grid_fields, available_grid_fields(key));
        sanitize_required_fields(
            &mut self.detail_track_fields,
            available_row_fields(LibraryListKey::Tracks),
            default_detail_track_fields(),
        );
        ensure_usable_row_field(&mut self.detail_track_fields, default_detail_track_fields());
        if !available_sort_fields(key).contains(&self.sort_key) {
            self.sort_key = default_sort_key(key);
        }
        self.layout_version = LIBRARY_LIST_LAYOUT_VERSION;
    }
}

pub fn default_library_list_settings() -> Vec<LibraryListSettingsEntry> {
    LibraryListKey::all()
        .into_iter()
        .map(|key| LibraryListSettingsEntry {
            key,
            settings: LibraryListSettings::for_key(key),
        })
        .collect()
}

pub fn available_row_fields(key: LibraryListKey) -> &'static [LibraryField] {
    match key {
        LibraryListKey::Albums | LibraryListKey::ArtistAlbums => &[
            LibraryField::RowIndex,
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::TitleMerged,
            LibraryField::AlbumArtist,
            LibraryField::Year,
            LibraryField::ReleaseDate,
            LibraryField::DateAdded,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::Genre,
            LibraryField::SongCount,
            LibraryField::Duration,
            LibraryField::Favorite,
        ],
        LibraryListKey::Artists | LibraryListKey::AlbumArtists => &[
            LibraryField::RowIndex,
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::SongCount,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::Favorite,
        ],
        LibraryListKey::Genres => &[
            LibraryField::RowIndex,
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::SongCount,
        ],
        LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks
        | LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => &[
            LibraryField::RowIndex,
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::TitleMerged,
            LibraryField::Artist,
            LibraryField::AlbumArtist,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::ReleaseDate,
            LibraryField::DateAdded,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::Genre,
            LibraryField::DiscNumber,
            LibraryField::TrackNumber,
            LibraryField::Duration,
            LibraryField::Favorite,
        ],
    }
}

pub fn available_grid_fields(key: LibraryListKey) -> &'static [LibraryField] {
    match key {
        LibraryListKey::Albums | LibraryListKey::ArtistAlbums => &[
            LibraryField::AlbumArtist,
            LibraryField::Year,
            LibraryField::ReleaseDate,
            LibraryField::DateAdded,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::Genre,
            LibraryField::SongCount,
            LibraryField::Duration,
        ],
        LibraryListKey::Artists | LibraryListKey::AlbumArtists => &[
            LibraryField::AlbumCount,
            LibraryField::SongCount,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
        ],
        LibraryListKey::Genres => &[LibraryField::AlbumCount, LibraryField::SongCount],
        LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks
        | LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => &[
            LibraryField::Artist,
            LibraryField::AlbumArtist,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::ReleaseDate,
            LibraryField::DateAdded,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::Genre,
            LibraryField::Duration,
        ],
    }
}

pub fn available_sort_fields(key: LibraryListKey) -> &'static [LibraryField] {
    match key {
        LibraryListKey::Albums | LibraryListKey::ArtistAlbums => &[
            LibraryField::Title,
            LibraryField::AlbumArtist,
            LibraryField::Year,
            LibraryField::ReleaseDate,
            LibraryField::DateAdded,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::SongCount,
            LibraryField::Duration,
            LibraryField::Favorite,
        ],
        LibraryListKey::Artists | LibraryListKey::AlbumArtists => &[
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::SongCount,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::Favorite,
        ],
        LibraryListKey::Genres => &[
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::SongCount,
        ],
        LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks
        | LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => &[
            LibraryField::TrackNumber,
            LibraryField::Title,
            LibraryField::Artist,
            LibraryField::AlbumArtist,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::ReleaseDate,
            LibraryField::DateAdded,
            LibraryField::LastPlayed,
            LibraryField::PlayCount,
            LibraryField::UserRating,
            LibraryField::Genre,
            LibraryField::Duration,
            LibraryField::Favorite,
        ],
    }
}

fn default_row_fields(key: LibraryListKey) -> Vec<LibraryField> {
    match key {
        LibraryListKey::Albums | LibraryListKey::ArtistAlbums => vec![
            LibraryField::TitleMerged,
            LibraryField::AlbumArtist,
            LibraryField::Year,
            LibraryField::Favorite,
        ],
        LibraryListKey::Artists | LibraryListKey::AlbumArtists => vec![
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::Favorite,
        ],
        LibraryListKey::Genres => vec![
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::SongCount,
        ],
        LibraryListKey::Tracks | LibraryListKey::FavoriteTracks => vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::Favorite,
        ],
        LibraryListKey::AlbumDetailTracks => default_detail_track_fields(),
        LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => {
            vec![
                LibraryField::RowIndex,
                LibraryField::TitleMerged,
                LibraryField::Album,
                LibraryField::Duration,
                LibraryField::Favorite,
            ]
        }
    }
}

fn default_grid_fields(key: LibraryListKey) -> Vec<LibraryField> {
    match key {
        LibraryListKey::Albums | LibraryListKey::ArtistAlbums => {
            vec![LibraryField::AlbumArtist, LibraryField::Year]
        }
        LibraryListKey::Artists | LibraryListKey::AlbumArtists => Vec::new(),
        LibraryListKey::Genres => Vec::new(),
        LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks
        | LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => {
            vec![
                LibraryField::Artist,
                LibraryField::Album,
                LibraryField::Duration,
            ]
        }
    }
}

fn default_detail_track_fields() -> Vec<LibraryField> {
    vec![
        LibraryField::TrackNumber,
        LibraryField::Title,
        LibraryField::Duration,
        LibraryField::Favorite,
    ]
}

fn default_sort_key(key: LibraryListKey) -> LibraryField {
    match key {
        LibraryListKey::Albums
        | LibraryListKey::Artists
        | LibraryListKey::AlbumArtists
        | LibraryListKey::Genres
        | LibraryListKey::ArtistAlbums
        | LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks => LibraryField::Title,
        LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => LibraryField::TrackNumber,
    }
}

fn sanitize_optional_fields(fields: &mut Vec<LibraryField>, available: &[LibraryField]) {
    let mut seen = Vec::new();
    fields.retain(|field| {
        if !available.contains(field) || seen.contains(field) {
            return false;
        }
        seen.push(*field);
        true
    });
}

fn sanitize_required_fields(
    fields: &mut Vec<LibraryField>,
    available: &[LibraryField],
    fallback: Vec<LibraryField>,
) {
    sanitize_optional_fields(fields, available);
    if fields.is_empty() {
        *fields = fallback;
    }
}

fn ensure_usable_row_field(fields: &mut Vec<LibraryField>, fallback: Vec<LibraryField>) {
    if fields.iter().any(|field| row_field_is_usable(*field)) {
        return;
    }
    if let Some(field) = fallback
        .into_iter()
        .find(|field| row_field_is_usable(*field))
    {
        fields.push(field);
    }
}

fn row_field_is_usable(field: LibraryField) -> bool {
    !matches!(
        field,
        LibraryField::RowIndex
            | LibraryField::Image
            | LibraryField::TrackNumber
            | LibraryField::DiscNumber
            | LibraryField::Favorite
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrackTableSettings {
    pub visible_columns: Vec<TrackTableColumn>,
    pub sort_key: TrackSortKey,
    pub descending: bool,
    #[serde(default)]
    pub layout_version: u8,
}

impl Default for TrackTableSettings {
    fn default() -> Self {
        Self {
            visible_columns: DEFAULT_TRACK_TABLE_COLUMNS.to_vec(),
            sort_key: TrackSortKey::Title,
            descending: false,
            layout_version: TRACK_TABLE_LAYOUT_VERSION,
        }
    }
}

impl TrackTableSettings {
    pub fn migrate_defaults(&mut self) {
        const LEGACY_DEFAULT_COLUMNS: [TrackTableColumn; 7] = [
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Artist,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
            TrackTableColumn::Duration,
            TrackTableColumn::Favorite,
        ];
        const COMPOSITE_TITLE_DEFAULT_COLUMNS: [TrackTableColumn; 3] = [
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
        ];

        if self.layout_version < TRACK_TABLE_LAYOUT_VERSION {
            if self.visible_columns.as_slice() == LEGACY_DEFAULT_COLUMNS
                || self.visible_columns.as_slice() == COMPOSITE_TITLE_DEFAULT_COLUMNS
            {
                self.visible_columns = DEFAULT_TRACK_TABLE_COLUMNS.to_vec();
                self.sort_key = TrackSortKey::Title;
            }
        }
        self.sanitize();
    }

    pub fn sanitize(&mut self) {
        let mut columns = Vec::new();
        for column in TrackTableColumn::all() {
            if self.visible_columns.contains(&column) {
                columns.push(column);
            }
        }
        if columns.is_empty() {
            columns = DEFAULT_TRACK_TABLE_COLUMNS.to_vec();
        }
        self.visible_columns = columns;
        self.layout_version = TRACK_TABLE_LAYOUT_VERSION;
    }
}

pub const EQUALIZER_BAND_COUNT: usize = 10;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlaybackTransitionMode {
    #[default]
    Gapless,
    Crossfade,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReplayGainMode {
    #[default]
    Off,
    Track,
    Album,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum StreamQuality {
    #[default]
    Original,
    MaxBitrateKbps(u32),
}

impl StreamQuality {
    pub fn max_bitrate_kbps(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::MaxBitrateKbps(kbps) => Some(kbps),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EqualizerSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_equalizer_bands")]
    pub bands: Vec<f64>,
}

impl Default for EqualizerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bands: default_equalizer_bands(),
        }
    }
}

impl EqualizerSettings {
    pub fn sanitize(&mut self) {
        if self.bands.len() != EQUALIZER_BAND_COUNT {
            self.bands.resize(EQUALIZER_BAND_COUNT, 0.0);
        }
        for gain in &mut self.bands {
            if !gain.is_finite() {
                *gain = 0.0;
            }
            *gain = gain.clamp(-12.0, 12.0);
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PlaybackSettings {
    #[serde(default)]
    pub transition_mode: PlaybackTransitionMode,
    #[serde(default = "default_crossfade_seconds")]
    pub crossfade_seconds: u8,
    #[serde(default)]
    pub replay_gain: ReplayGainMode,
    #[serde(default)]
    pub stream_quality: StreamQuality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_output: Option<String>,
    #[serde(default)]
    pub equalizer: EqualizerSettings,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            transition_mode: PlaybackTransitionMode::Gapless,
            crossfade_seconds: default_crossfade_seconds(),
            replay_gain: ReplayGainMode::Off,
            stream_quality: StreamQuality::Original,
            audio_output: None,
            equalizer: EqualizerSettings::default(),
            volume: default_volume(),
            muted: false,
        }
    }
}

impl PlaybackSettings {
    pub fn sanitize(&mut self) {
        self.crossfade_seconds = self.crossfade_seconds.clamp(1, 12);
        if !self.volume.is_finite() {
            self.volume = default_volume();
        }
        self.volume = self.volume.clamp(0.0, 1.0);
        if self
            .audio_output
            .as_deref()
            .is_some_and(|output| output.trim().is_empty())
        {
            self.audio_output = None;
        }
        self.equalizer.sanitize();
    }
}

fn default_equalizer_bands() -> Vec<f64> {
    vec![0.0; EQUALIZER_BAND_COUNT]
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AppSettings {
    #[serde(default)]
    pub layout: LayoutSettings,
    #[serde(default)]
    pub sidebar: SidebarSettings,
    pub theme_preference: ThemePreference,
    pub private_mode: bool,
    pub notifications_enabled: bool,
    pub external_lyrics_enabled: bool,
    #[serde(default = "default_true")]
    pub external_metadata_enabled: bool,
    #[serde(default = "default_true")]
    pub prefer_server_lyrics: bool,
    #[serde(default)]
    pub ask_lyrics_save_path: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lyrics_export_folder: Option<String>,
    pub discord_presence_enabled: bool,
    #[serde(default = "default_discord_client_id")]
    pub discord_client_id: String,
    #[serde(default)]
    pub discord_display_type: DiscordDisplayType,
    #[serde(default)]
    pub discord_link_type: DiscordLinkType,
    #[serde(default = "default_true")]
    pub discord_show_paused: bool,
    #[serde(default)]
    pub discord_show_as_listening: bool,
    #[serde(default = "default_true")]
    pub discord_show_state_icon: bool,
    #[serde(default)]
    pub lastfm_api_key: String,
    #[serde(default)]
    pub scrobbling: ScrobblingSettings,
    #[serde(default)]
    pub auto_dj_enabled: bool,
    #[serde(default)]
    pub playback: PlaybackSettings,
    #[serde(default = "default_home_sections")]
    pub home_sections: Vec<HomeSectionKind>,
    #[serde(default)]
    pub home_blocks: Vec<HomeBlockKind>,
    #[serde(default = "default_lyrics_panel_visible")]
    pub lyrics_panel_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_lyrics_position: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_lyrics_ratio: Option<f64>,
    #[serde(default)]
    pub queue_lyrics_layout_version: u8,
    #[serde(default)]
    pub track_table: TrackTableSettings,
    #[serde(default)]
    pub library_lists: Vec<LibraryListSettingsEntry>,
    #[serde(default)]
    pub suppressed_auto_lyrics_track_ids: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            layout: LayoutSettings::default(),
            sidebar: SidebarSettings::default(),
            theme_preference: ThemePreference::System,
            private_mode: false,
            notifications_enabled: false,
            external_lyrics_enabled: false,
            external_metadata_enabled: true,
            prefer_server_lyrics: true,
            ask_lyrics_save_path: false,
            lyrics_export_folder: None,
            discord_presence_enabled: false,
            discord_client_id: default_discord_client_id(),
            discord_display_type: DiscordDisplayType::Application,
            discord_link_type: DiscordLinkType::None,
            discord_show_paused: true,
            discord_show_as_listening: false,
            discord_show_state_icon: true,
            lastfm_api_key: String::new(),
            scrobbling: ScrobblingSettings::default(),
            auto_dj_enabled: false,
            playback: PlaybackSettings::default(),
            home_sections: default_home_sections(),
            home_blocks: default_home_blocks(),
            lyrics_panel_visible: true,
            queue_lyrics_position: None,
            queue_lyrics_ratio: None,
            queue_lyrics_layout_version: QUEUE_LYRICS_LAYOUT_VERSION,
            track_table: TrackTableSettings::default(),
            library_lists: default_library_list_settings(),
            suppressed_auto_lyrics_track_ids: Vec::new(),
        }
    }
}

impl AppSettings {
    pub fn migrate_defaults(&mut self) {
        if self.queue_lyrics_layout_version < QUEUE_LYRICS_LAYOUT_VERSION {
            self.queue_lyrics_position = None;
            self.queue_lyrics_ratio = None;
            self.queue_lyrics_layout_version = QUEUE_LYRICS_LAYOUT_VERSION;
        }
        if self.discord_client_id.trim().is_empty() {
            self.discord_client_id = default_discord_client_id();
            self.discord_presence_enabled = true;
        }
        self.track_table.migrate_defaults();
        self.playback.sanitize();
        self.scrobbling.sanitize();
        self.lastfm_api_key = self.lastfm_api_key.trim().to_string();
        if self.lastfm_api_key.is_empty() && !self.scrobbling.lastfm.api_key.is_empty() {
            self.lastfm_api_key = self.scrobbling.lastfm.api_key.clone();
        } else if self.scrobbling.lastfm.api_key.is_empty() && !self.lastfm_api_key.is_empty() {
            self.scrobbling.lastfm.api_key = self.lastfm_api_key.clone();
        }
        self.layout.sanitize();
        self.sidebar.sanitize();
        self.migrate_home_blocks();
        self.migrate_library_lists();
    }

    fn migrate_home_blocks(&mut self) {
        if self.home_sections.is_empty() {
            self.home_sections = default_home_sections();
        }
        if self.home_blocks.is_empty() {
            self.home_blocks = Vec::with_capacity(self.home_sections.len() + 2);
            self.home_blocks.push(HomeBlockKind::Showcase);
            for section in &self.home_sections {
                self.home_blocks.push(match section {
                    HomeSectionKind::Explore => HomeBlockKind::Explore,
                    HomeSectionKind::MostPlayed => HomeBlockKind::MostPlayed,
                    HomeSectionKind::NewlyAdded => HomeBlockKind::NewlyAdded,
                    HomeSectionKind::RecentlyPlayed => HomeBlockKind::RecentlyPlayed,
                    HomeSectionKind::RecentlyReleased => HomeBlockKind::RecentlyReleased,
                });
            }
            if !self.home_blocks.contains(&HomeBlockKind::Genres) {
                self.home_blocks.push(HomeBlockKind::Genres);
            }
        }
        sanitize_home_blocks(&mut self.home_blocks);
        self.home_sections = self
            .home_blocks
            .iter()
            .filter_map(|block| block.section_kind())
            .collect();
    }

    fn migrate_library_lists(&mut self) {
        if self.library_lists.is_empty() {
            self.library_lists = default_library_list_settings();
        }

        for key in LibraryListKey::all() {
            if !self.library_lists.iter().any(|entry| entry.key == key) {
                self.library_lists.push(LibraryListSettingsEntry {
                    key,
                    settings: LibraryListSettings::for_key(key),
                });
            }
        }
        self.library_lists
            .retain(|entry| LibraryListKey::all().contains(&entry.key));
        self.library_lists.sort_by_key(|entry| {
            LibraryListKey::all()
                .iter()
                .position(|key| *key == entry.key)
                .unwrap_or(usize::MAX)
        });
        for entry in &mut self.library_lists {
            entry.settings.sanitize(entry.key);
        }
    }

    pub fn library_list(&self, key: LibraryListKey) -> LibraryListSettings {
        self.library_lists
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.settings.clone())
            .unwrap_or_else(|| LibraryListSettings::for_key(key))
    }
}

fn sanitize_home_blocks(blocks: &mut Vec<HomeBlockKind>) {
    let mut seen = Vec::new();
    blocks.retain(|block| {
        if seen.contains(block) {
            false
        } else {
            seen.push(*block);
            true
        }
    });
    if blocks.is_empty() {
        *blocks = default_home_blocks();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppSettings, AudioscrobblerScrobbleSettings, DEFAULT_DISCORD_CLIENT_ID, DiscordDisplayType,
        DiscordLinkType, EQUALIZER_BAND_COUNT, LEGACY_APPLICATION_DISPLAY_BYTES, LeftSidebarMode,
        LibraryField, LibraryLayout, LibraryListKey, PlaybackTransitionMode, ReplayGainMode,
        RightSidebarMode, ScrobblingSettings, SidebarRouteItem, StreamQuality, TrackSortKey,
        TrackTableColumn,
    };

    #[test]
    fn settings_default_to_privacy_preserving_remote_features() {
        let settings = AppSettings::default();

        assert!(!settings.notifications_enabled);
        assert!(!settings.external_lyrics_enabled);
        assert!(settings.external_metadata_enabled);
        assert!(settings.prefer_server_lyrics);
        assert!(!settings.discord_presence_enabled);
        assert_eq!(settings.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
        assert_eq!(
            settings.discord_display_type,
            DiscordDisplayType::Application
        );
        assert_eq!(settings.discord_link_type, DiscordLinkType::None);
        assert!(settings.discord_show_paused);
        assert!(!settings.discord_show_as_listening);
        assert!(settings.discord_show_state_icon);
        assert_eq!(settings.lastfm_api_key, "");
        assert!(!settings.scrobbling.lastfm.enabled);
        assert_eq!(settings.scrobbling.lastfm.username, "");
        assert_eq!(settings.scrobbling.lastfm.api_key, "");
        assert_eq!(settings.scrobbling.lastfm.api_secret, "");
        assert_eq!(settings.scrobbling.lastfm.session_key, "");
        assert!(settings.scrobbling.lastfm.now_playing_enabled);
        assert!(!settings.scrobbling.librefm.enabled);
        assert_eq!(settings.scrobbling.librefm.username, "");
        assert_eq!(settings.scrobbling.librefm.api_key, "rufin");
        assert_eq!(settings.scrobbling.librefm.api_secret, "rufin");
        assert_eq!(settings.scrobbling.librefm.session_key, "");
        assert!(settings.scrobbling.librefm.now_playing_enabled);
        assert!(!settings.scrobbling.listenbrainz.enabled);
        assert_eq!(settings.scrobbling.listenbrainz.user_token, "");
        assert!(settings.scrobbling.listenbrainz.now_playing_enabled);
        assert!(!settings.auto_dj_enabled);
        assert_eq!(
            settings.playback.transition_mode,
            PlaybackTransitionMode::Gapless
        );
        assert_eq!(settings.playback.crossfade_seconds, 5);
        assert_eq!(settings.playback.replay_gain, ReplayGainMode::Off);
        assert_eq!(settings.playback.stream_quality, StreamQuality::Original);
        assert_eq!(settings.playback.audio_output, None);
        assert!(!settings.playback.equalizer.enabled);
        assert_eq!(
            settings.playback.equalizer.bands.len(),
            EQUALIZER_BAND_COUNT
        );
        assert_eq!(settings.playback.volume, 1.0);
        assert!(!settings.playback.muted);
        assert!(settings.lyrics_panel_visible);
        assert_eq!(
            settings.layout.default_profile.left_sidebar,
            LeftSidebarMode::Full
        );
        assert_eq!(
            settings.layout.default_profile.right_sidebar,
            RightSidebarMode::Comfortable
        );
        assert!(settings.layout.narrow_enabled);
        assert_eq!(settings.layout.narrow_threshold, 1_000);
        assert_eq!(
            settings.layout.narrow_profile.left_sidebar,
            LeftSidebarMode::Compact
        );
        assert_eq!(
            settings.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Default
        );
        assert!(settings.sidebar.server_visible);
        assert!(
            settings
                .sidebar
                .route_items
                .iter()
                .any(|entry| entry.item == SidebarRouteItem::AlbumArtists && !entry.visible)
        );
        assert_eq!(settings.queue_lyrics_layout_version, 3);
        assert_eq!(settings.home_sections.len(), 5);
        assert_eq!(settings.home_blocks.len(), 7);
        assert_eq!(
            settings.library_list(LibraryListKey::Albums).layout,
            LibraryLayout::Grid
        );
        assert_eq!(
            settings.library_list(LibraryListKey::Tracks).layout,
            LibraryLayout::Row
        );
        assert_eq!(
            settings.library_list(LibraryListKey::Tracks).row_fields,
            vec![
                LibraryField::TitleMerged,
                LibraryField::Album,
                LibraryField::Year,
                LibraryField::Favorite,
            ]
        );
        assert_eq!(
            settings.library_list(LibraryListKey::Tracks).sort_key,
            LibraryField::Title
        );
        assert_eq!(
            settings
                .library_list(LibraryListKey::FavoriteTracks)
                .row_fields,
            settings.library_list(LibraryListKey::Tracks).row_fields
        );
        assert_eq!(
            settings.library_list(LibraryListKey::Albums).row_fields,
            vec![
                LibraryField::TitleMerged,
                LibraryField::AlbumArtist,
                LibraryField::Year,
                LibraryField::Favorite,
            ]
        );
        assert_eq!(
            settings.library_list(LibraryListKey::Artists).row_fields,
            vec![
                LibraryField::Image,
                LibraryField::Title,
                LibraryField::AlbumCount,
                LibraryField::Favorite,
            ]
        );
        assert_eq!(
            settings
                .library_list(LibraryListKey::AlbumArtists)
                .row_fields,
            settings.library_list(LibraryListKey::Artists).row_fields
        );
        assert_eq!(
            settings.library_list(LibraryListKey::Genres).row_fields,
            vec![
                LibraryField::Title,
                LibraryField::AlbumCount,
                LibraryField::SongCount,
            ]
        );
        assert!(
            settings
                .library_list(LibraryListKey::Artists)
                .grid_fields
                .is_empty()
        );
        assert!(
            settings
                .library_list(LibraryListKey::Genres)
                .grid_fields
                .is_empty()
        );
        assert_eq!(
            settings.track_table.visible_columns,
            vec![
                TrackTableColumn::TrackNumber,
                TrackTableColumn::Title,
                TrackTableColumn::Album,
                TrackTableColumn::Year,
                TrackTableColumn::Favorite,
            ]
        );
        assert_eq!(settings.track_table.sort_key, TrackSortKey::Title);
        assert!(settings.suppressed_auto_lyrics_track_ids.is_empty());
    }

    #[test]
    fn settings_serialize_to_json() {
        let settings = AppSettings {
            lyrics_panel_visible: false,
            queue_lyrics_position: Some(520),
            queue_lyrics_ratio: Some(0.7),
            ..AppSettings::default()
        };

        let json = serde_json::to_string(&settings).expect("serialize settings");
        let restored = serde_json::from_str::<AppSettings>(&json).expect("deserialize settings");

        assert_eq!(restored, settings);
    }

    #[test]
    fn settings_restore_without_window_geometry() {
        let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "home_sections":["Explore"]
        }"#;

        let restored = serde_json::from_str::<AppSettings>(json).expect("deserialize settings");

        assert!(restored.lyrics_panel_visible);
        assert_eq!(
            restored.layout.default_profile.right_sidebar,
            RightSidebarMode::Comfortable
        );
        assert_eq!(
            restored.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Default
        );
        assert!(restored.sidebar.server_visible);
        assert_eq!(restored.queue_lyrics_position, None);
        assert_eq!(restored.queue_lyrics_ratio, None);
        assert!(!restored.auto_dj_enabled);
        assert!(restored.external_metadata_enabled);
        assert!(restored.prefer_server_lyrics);
        assert_eq!(
            restored.playback.transition_mode,
            PlaybackTransitionMode::Gapless
        );
        assert_eq!(restored.playback.volume, 1.0);
        assert!(!restored.playback.muted);
        assert_eq!(restored.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
        assert_eq!(
            restored.discord_display_type,
            DiscordDisplayType::Application
        );
        assert_eq!(restored.discord_link_type, DiscordLinkType::None);
        assert!(restored.discord_show_paused);
        assert!(!restored.discord_show_as_listening);
        assert!(restored.discord_show_state_icon);
        assert_eq!(restored.lastfm_api_key, "");
        assert!(!restored.scrobbling.lastfm.enabled);
        assert_eq!(restored.scrobbling.librefm.api_key, "rufin");
        assert_eq!(restored.scrobbling.librefm.api_secret, "rufin");
        assert!(!restored.scrobbling.listenbrainz.enabled);
        assert_eq!(restored.track_table.sort_key, TrackSortKey::Title);
    }

    #[test]
    fn settings_share_lastfm_api_key_when_only_one_value_exists() {
        let mut from_global_key = AppSettings {
            lastfm_api_key: "global-key".to_string(),
            ..AppSettings::default()
        };
        from_global_key.migrate_defaults();
        assert_eq!(from_global_key.scrobbling.lastfm.api_key, "global-key");

        let mut from_scrobbling_key = AppSettings {
            scrobbling: ScrobblingSettings {
                lastfm: AudioscrobblerScrobbleSettings {
                    api_key: "scrobble-key".to_string(),
                    ..AudioscrobblerScrobbleSettings::default()
                },
                ..ScrobblingSettings::default()
            },
            ..AppSettings::default()
        };
        from_scrobbling_key.migrate_defaults();
        assert_eq!(from_scrobbling_key.lastfm_api_key, "scrobble-key");
    }

    #[test]
    fn settings_migrate_legacy_home_sections_to_home_blocks() {
        let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "home_sections":["Explore","RecentlyPlayed"]
        }"#;

        let mut settings = serde_json::from_str::<AppSettings>(json).expect("deserialize settings");
        settings.migrate_defaults();

        assert_eq!(
            settings.home_blocks,
            vec![
                crate::domain::HomeBlockKind::Showcase,
                crate::domain::HomeBlockKind::Explore,
                crate::domain::HomeBlockKind::RecentlyPlayed,
                crate::domain::HomeBlockKind::Genres,
            ]
        );
        assert_eq!(
            settings.home_sections,
            vec![
                crate::domain::HomeSectionKind::Explore,
                crate::domain::HomeSectionKind::RecentlyPlayed
            ]
        );
    }

    #[test]
    fn library_layout_unknown_values_fall_back_to_grid() {
        let layout =
            serde_json::from_str::<LibraryLayout>("\"weird\"").expect("deserialize layout");

        assert_eq!(layout, LibraryLayout::Grid);
    }

    #[test]
    fn library_list_settings_sanitize_fields_and_layouts() {
        let mut settings = AppSettings {
            library_lists: vec![super::LibraryListSettingsEntry {
                key: LibraryListKey::Genres,
                settings: super::LibraryListSettings {
                    layout: LibraryLayout::Detail,
                    row_fields: vec![
                        LibraryField::Title,
                        LibraryField::Album,
                        LibraryField::Title,
                    ],
                    grid_fields: vec![LibraryField::Artist],
                    detail_track_fields: Vec::new(),
                    sort_key: LibraryField::Album,
                    descending: true,
                    layout_version: 0,
                },
            }],
            ..AppSettings::default()
        };

        settings.migrate_defaults();
        let genres = settings.library_list(LibraryListKey::Genres);

        assert_eq!(genres.layout, LibraryLayout::Grid);
        assert_eq!(genres.row_fields, vec![LibraryField::Title]);
        assert!(genres.grid_fields.is_empty());
        assert_eq!(genres.sort_key, LibraryField::Title);
    }

    #[test]
    fn library_list_settings_keep_a_usable_row_field() {
        let mut settings = AppSettings {
            library_lists: vec![super::LibraryListSettingsEntry {
                key: LibraryListKey::Tracks,
                settings: super::LibraryListSettings {
                    layout: LibraryLayout::Row,
                    row_fields: vec![LibraryField::RowIndex, LibraryField::Favorite],
                    grid_fields: vec![LibraryField::Artist],
                    detail_track_fields: vec![LibraryField::Favorite],
                    sort_key: LibraryField::TrackNumber,
                    descending: false,
                    layout_version: 0,
                },
            }],
            ..AppSettings::default()
        };

        settings.migrate_defaults();
        let tracks = settings.library_list(LibraryListKey::Tracks);

        assert!(tracks.row_fields.contains(&LibraryField::TitleMerged));
        assert!(tracks.detail_track_fields.contains(&LibraryField::Title));
    }

    #[test]
    fn settings_migrate_legacy_queue_lyrics_split_state() {
        let mut settings = AppSettings {
            queue_lyrics_position: Some(160),
            queue_lyrics_ratio: Some(0.3),
            queue_lyrics_layout_version: 2,
            ..AppSettings::default()
        };

        settings.migrate_defaults();

        assert_eq!(settings.queue_lyrics_position, None);
        assert_eq!(settings.queue_lyrics_ratio, None);
        assert_eq!(settings.queue_lyrics_layout_version, 3);
    }

    #[test]
    fn settings_migrate_empty_discord_identity_defaults() {
        let mut settings = AppSettings {
            discord_presence_enabled: false,
            discord_client_id: String::new(),
            ..AppSettings::default()
        };

        settings.migrate_defaults();

        assert_eq!(settings.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
        assert!(settings.discord_presence_enabled);
    }

    #[test]
    fn settings_restore_previous_application_display_value() {
        let legacy_value =
            std::str::from_utf8(LEGACY_APPLICATION_DISPLAY_BYTES).expect("legacy value");
        let json = format!("\"{}\"", legacy_value);

        let restored =
            serde_json::from_str::<DiscordDisplayType>(&json).expect("deserialize display type");

        assert_eq!(restored, DiscordDisplayType::Application);
    }

    #[test]
    fn settings_migrate_legacy_track_table_default_columns() {
        let json = r#"{
            "visible_columns":["TrackNumber","Title","Artist","Album","Year","Duration","Favorite"],
            "sort_key":"TrackNumber",
            "descending":false
        }"#;

        let mut settings =
            serde_json::from_str::<super::TrackTableSettings>(json).expect("deserialize settings");
        settings.migrate_defaults();

        assert_eq!(
            settings.visible_columns,
            vec![
                TrackTableColumn::TrackNumber,
                TrackTableColumn::Title,
                TrackTableColumn::Album,
                TrackTableColumn::Year,
                TrackTableColumn::Favorite,
            ]
        );
        assert_eq!(settings.sort_key, TrackSortKey::Title);
        assert_eq!(settings.layout_version, super::TRACK_TABLE_LAYOUT_VERSION);
    }

    #[test]
    fn settings_migrate_previous_composite_title_default_columns() {
        let json = r#"{
            "visible_columns":["Title","Album","Year"],
            "sort_key":"TrackNumber",
            "descending":false,
            "layout_version":1
        }"#;

        let mut settings =
            serde_json::from_str::<super::TrackTableSettings>(json).expect("deserialize settings");
        settings.migrate_defaults();

        assert_eq!(
            settings.visible_columns,
            vec![
                TrackTableColumn::TrackNumber,
                TrackTableColumn::Title,
                TrackTableColumn::Album,
                TrackTableColumn::Year,
                TrackTableColumn::Favorite,
            ]
        );
        assert_eq!(settings.sort_key, TrackSortKey::Title);
        assert_eq!(settings.layout_version, super::TRACK_TABLE_LAYOUT_VERSION);
    }
}
