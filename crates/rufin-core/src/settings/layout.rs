use serde::{Deserialize, Deserializer, Serialize, de};

use super::sidebar::*;
use crate::domain::{HomeBlockKind, HomeSectionKind, ServerId};
pub const TRACK_TABLE_LAYOUT_VERSION: u8 = 3;
pub const LIBRARY_LIST_LAYOUT_VERSION: u8 = 5;
pub const QUEUE_LYRICS_LAYOUT_VERSION: u8 = 5;
pub const DEFAULT_WINDOW_WIDTH: i32 = 1_500;
pub const DEFAULT_WINDOW_HEIGHT: i32 = 900;
pub const MIN_RESTORED_WINDOW_WIDTH: i32 = 480;
pub const MIN_RESTORED_WINDOW_HEIGHT: i32 = 634;
pub const MAX_RESTORED_WINDOW_WIDTH: i32 = 3_400;
pub const MAX_RESTORED_WINDOW_HEIGHT: i32 = 2_000;
pub const DEFAULT_DISCORD_CLIENT_ID: &str = "1505345384686419979";
pub const MIN_CROSSFADE_SECONDS: u8 = 1;
pub const MAX_CROSSFADE_SECONDS: u8 = 30;
pub const DEFAULT_AUTO_DJ_REFILL_THRESHOLD: u8 = 1;
pub const MIN_AUTO_DJ_REFILL_THRESHOLD: u8 = 1;
pub const MAX_AUTO_DJ_REFILL_THRESHOLD: u8 = 10;
pub(super) const LEGACY_APPLICATION_DISPLAY_BYTES: &[u8] = &[102, 101, 105, 115, 104, 105, 110];
pub(super) fn default_lyrics_panel_visible() -> bool {
    true
}
pub(super) fn default_discord_client_id() -> String {
    DEFAULT_DISCORD_CLIENT_ID.to_string()
}
pub(super) fn default_discord_link_type() -> DiscordLinkType {
    DiscordLinkType::MusicBrainz
}
pub(super) fn default_true() -> bool {
    true
}
pub(super) fn default_volume() -> f64 {
    1.0
}
pub(super) fn default_crossfade_seconds() -> u8 {
    5
}
pub(super) fn default_auto_dj_refill_threshold() -> u8 {
    DEFAULT_AUTO_DJ_REFILL_THRESHOLD
}
fn default_narrow_layout_enabled() -> bool {
    true
}
fn default_narrow_layout_threshold() -> i32 {
    1_300
}
pub const MIN_NARROW_LAYOUT_THRESHOLD: i32 = 700;
pub const MAX_NARROW_LAYOUT_THRESHOLD: i32 = 3_400;
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum LeftSidebarMode {
    #[default]
    Full,
    Compact,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum RightSidebarMode {
    Hidden,
    Compact,
    Default,
    #[default]
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
    Folders,
    Playlists,
    SmartPlaylists,
}
impl SidebarRouteItem {
    pub fn all() -> [Self; 10] {
        [
            Self::Home,
            Self::Favorites,
            Self::Albums,
            Self::Tracks,
            Self::Artists,
            Self::AlbumArtists,
            Self::Genres,
            Self::Folders,
            Self::Playlists,
            Self::SmartPlaylists,
        ]
    }

    fn default_visible(self) -> bool {
        true
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
        if !sanitized.iter().any(|entry| entry.visible)
            && let Some(home) = sanitized
                .iter_mut()
                .find(|entry| entry.item == SidebarRouteItem::Home)
        {
            home.visible = true;
        }
        self.route_items = sanitized;
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LibrarySourceSelection {
    Local,
    Server(ServerId),
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalLibraryFolder {
    pub path: String,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibrarySourceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<LibrarySourceSelection>,
    #[serde(default)]
    pub local_folders: Vec<LocalLibraryFolder>,
}
impl LibrarySourceSettings {
    pub fn sanitize(&mut self) {
        let mut seen = Vec::<String>::new();
        self.local_folders.retain_mut(|folder| {
            folder.path = folder.path.trim().to_string();
            if folder.path.is_empty() || seen.iter().any(|path| path == &folder.path) {
                return false;
            }
            seen.push(folder.path.clone());
            true
        });
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
pub(super) const DEFAULT_TRACK_TABLE_COLUMNS: [TrackTableColumn; 4] = [
    TrackTableColumn::TrackNumber,
    TrackTableColumn::Title,
    TrackTableColumn::Album,
    TrackTableColumn::Year,
];
pub(super) fn default_home_sections() -> Vec<HomeSectionKind> {
    vec![
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::NewlyAdded,
        HomeSectionKind::RecentlyPlayed,
        HomeSectionKind::RecentlyReleased,
    ]
}
pub(super) fn default_home_blocks() -> Vec<HomeBlockKind> {
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
pub const SYSTEM_LANGUAGE_PREFERENCE: &str = "system";
pub fn default_language_preference() -> String {
    SYSTEM_LANGUAGE_PREFERENCE.to_string()
}
pub fn sanitize_language_preference(value: &str) -> String {
    let value = value.trim();
    if value.is_empty()
        || value.eq_ignore_ascii_case("default")
        || value.eq_ignore_ascii_case(SYSTEM_LANGUAGE_PREFERENCE)
    {
        return default_language_preference();
    }
    if value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'@'))
    {
        return default_language_preference();
    }
    value.to_string()
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
    Playlists,
    SmartPlaylists,
    AlbumDetailTracks,
    ArtistAlbums,
    ArtistTracks,
    GenreTracks,
    PlaylistTracks,
    SmartPlaylistTracks,
}
impl LibraryListKey {
    pub fn all() -> [Self; 14] {
        [
            Self::Albums,
            Self::Artists,
            Self::AlbumArtists,
            Self::Tracks,
            Self::FavoriteTracks,
            Self::Genres,
            Self::Playlists,
            Self::SmartPlaylists,
            Self::AlbumDetailTracks,
            Self::ArtistAlbums,
            Self::ArtistTracks,
            Self::GenreTracks,
            Self::PlaylistTracks,
            Self::SmartPlaylistTracks,
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
            Self::Playlists => "Playlists",
            Self::SmartPlaylists => "Smart playlists",
            Self::AlbumDetailTracks => "Album tracks",
            Self::ArtistAlbums => "Artist albums",
            Self::ArtistTracks => "Artist tracks",
            Self::GenreTracks => "Genre tracks",
            Self::PlaylistTracks => "Playlist tracks",
            Self::SmartPlaylistTracks => "Smart playlist tracks",
        }
    }

    pub fn supports_layout(self, layout: LibraryLayout) -> bool {
        match layout {
            LibraryLayout::Detail => matches!(self, Self::Albums),
            LibraryLayout::Row | LibraryLayout::Grid => true,
        }
    }

    fn default_layout(self) -> LibraryLayout {
        match self {
            Self::Albums => LibraryLayout::Row,
            Self::Tracks
            | Self::FavoriteTracks
            | Self::AlbumDetailTracks
            | Self::ArtistTracks
            | Self::GenreTracks
            | Self::PlaylistTracks
            | Self::SmartPlaylistTracks => LibraryLayout::Row,
            Self::Artists
            | Self::AlbumArtists
            | Self::Genres
            | Self::Playlists
            | Self::SmartPlaylists
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
            Self::PlayCount => "Plays",
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
        self.migrate_defaults(key);
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
            available_detail_track_fields(),
            default_detail_track_fields(),
        );
        ensure_usable_row_field(&mut self.detail_track_fields, default_detail_track_fields());
        if !available_sort_fields(key).contains(&self.sort_key) {
            self.sort_key = default_sort_key(key);
        }
        self.layout_version = LIBRARY_LIST_LAYOUT_VERSION;
    }

    fn migrate_defaults(&mut self, key: LibraryListKey) {
        if self.layout_version >= LIBRARY_LIST_LAYOUT_VERSION {
            return;
        }

        if key == LibraryListKey::Playlists {
            if self.row_fields
                == [
                    LibraryField::Image,
                    LibraryField::Title,
                    LibraryField::SongCount,
                    LibraryField::Duration,
                ]
            {
                self.row_fields = default_row_fields(key);
            }
            if self.grid_fields == [LibraryField::SongCount, LibraryField::Duration] {
                self.grid_fields = default_grid_fields(key);
            }
        }

        if key == LibraryListKey::SmartPlaylists
            && self.layout_version < 4
            && self.sort_key == LibraryField::Title
        {
            self.sort_key = default_sort_key(key);
        }

        if key.supports_layout(LibraryLayout::Detail)
            && self.layout_version < 5
            && self
                .detail_track_fields
                .iter()
                .any(|field| !available_detail_track_fields().contains(field))
        {
            self.detail_track_fields = default_detail_track_fields();
        }
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
        LibraryListKey::Playlists | LibraryListKey::SmartPlaylists => &[
            LibraryField::RowIndex,
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::SongCount,
            LibraryField::Duration,
        ],
        LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks
        | LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks
        | LibraryListKey::SmartPlaylistTracks => &[
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
        LibraryListKey::Playlists | LibraryListKey::SmartPlaylists => {
            &[LibraryField::SongCount, LibraryField::Duration]
        }
        LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks
        | LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks
        | LibraryListKey::SmartPlaylistTracks => &[
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
