use serde::{Deserialize, Serialize};

use crate::domain::{HomeBlockKind, HomeSectionKind};

use super::*;

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
        LibraryListKey::Playlists => &[
            LibraryField::Title,
            LibraryField::SongCount,
            LibraryField::Duration,
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
pub(super) fn default_row_fields(key: LibraryListKey) -> Vec<LibraryField> {
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
        LibraryListKey::Playlists => vec![
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::SongCount,
            LibraryField::Duration,
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
pub(super) fn default_grid_fields(key: LibraryListKey) -> Vec<LibraryField> {
    match key {
        LibraryListKey::Albums | LibraryListKey::ArtistAlbums => {
            vec![LibraryField::AlbumArtist, LibraryField::Year]
        }
        LibraryListKey::Artists | LibraryListKey::AlbumArtists => Vec::new(),
        LibraryListKey::Genres => Vec::new(),
        LibraryListKey::Playlists => vec![LibraryField::SongCount, LibraryField::Duration],
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
pub(super) fn default_detail_track_fields() -> Vec<LibraryField> {
    vec![
        LibraryField::TrackNumber,
        LibraryField::Title,
        LibraryField::Duration,
    ]
}
pub(super) fn default_sort_key(key: LibraryListKey) -> LibraryField {
    match key {
        LibraryListKey::Albums
        | LibraryListKey::Artists
        | LibraryListKey::AlbumArtists
        | LibraryListKey::Genres
        | LibraryListKey::ArtistAlbums
        | LibraryListKey::Playlists
        | LibraryListKey::Tracks
        | LibraryListKey::FavoriteTracks => LibraryField::Title,
        LibraryListKey::AlbumDetailTracks
        | LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => LibraryField::TrackNumber,
    }
}
pub(super) fn sanitize_optional_fields(fields: &mut Vec<LibraryField>, available: &[LibraryField]) {
    let mut seen = Vec::new();
    fields.retain(|field| {
        if !available.contains(field) || seen.contains(field) {
            return false;
        }
        seen.push(*field);
        true
    });
}
pub(super) fn sanitize_required_fields(
    fields: &mut Vec<LibraryField>,
    available: &[LibraryField],
    fallback: Vec<LibraryField>,
) {
    sanitize_optional_fields(fields, available);
    if fields.is_empty() {
        *fields = fallback;
    }
}
pub(super) fn ensure_usable_row_field(fields: &mut Vec<LibraryField>, fallback: Vec<LibraryField>) {
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

        if self.layout_version < TRACK_TABLE_LAYOUT_VERSION
            && (self.visible_columns.as_slice() == LEGACY_DEFAULT_COLUMNS
                || self.visible_columns.as_slice() == COMPOSITE_TITLE_DEFAULT_COLUMNS)
        {
            self.visible_columns = DEFAULT_TRACK_TABLE_COLUMNS.to_vec();
            self.sort_key = TrackSortKey::Title;
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
        self.crossfade_seconds = self
            .crossfade_seconds
            .clamp(MIN_CROSSFADE_SECONDS, MAX_CROSSFADE_SECONDS);
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
    #[serde(default)]
    pub sources: LibrarySourceSettings,
    pub theme_preference: ThemePreference,
    #[serde(default = "default_language_preference")]
    pub language: String,
    pub private_mode: bool,
    pub notifications_enabled: bool,
    pub external_lyrics_enabled: bool,
    #[serde(default = "default_true")]
    pub external_metadata_enabled: bool,
    #[serde(default = "default_true")]
    pub prefer_server_lyrics: bool,
    pub discord_presence_enabled: bool,
    #[serde(default = "default_discord_client_id")]
    pub discord_client_id: String,
    #[serde(default)]
    pub discord_display_type: DiscordDisplayType,
    #[serde(default = "default_discord_link_type")]
    pub discord_link_type: DiscordLinkType,
    #[serde(default)]
    pub discord_show_paused: bool,
    #[serde(default = "default_true")]
    pub discord_show_as_listening: bool,
    #[serde(default = "default_true")]
    pub discord_show_state_icon: bool,
    #[serde(default)]
    pub lastfm_api_key: String,
    #[serde(default)]
    pub scrobbling: ScrobblingSettings,
    #[serde(default = "default_true")]
    pub auto_dj_enabled: bool,
    #[serde(default)]
    pub playback: PlaybackSettings,
    #[serde(default = "default_home_sections")]
    pub home_sections: Vec<HomeSectionKind>,
    #[serde(default)]
    pub home_blocks: Vec<HomeBlockKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<i32>,
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
            sources: LibrarySourceSettings::default(),
            theme_preference: ThemePreference::System,
            language: default_language_preference(),
            private_mode: false,
            notifications_enabled: false,
            external_lyrics_enabled: true,
            external_metadata_enabled: true,
            prefer_server_lyrics: true,
            discord_presence_enabled: false,
            discord_client_id: default_discord_client_id(),
            discord_display_type: DiscordDisplayType::Application,
            discord_link_type: default_discord_link_type(),
            discord_show_paused: false,
            discord_show_as_listening: true,
            discord_show_state_icon: true,
            lastfm_api_key: String::new(),
            scrobbling: ScrobblingSettings::default(),
            auto_dj_enabled: true,
            playback: PlaybackSettings::default(),
            home_sections: default_home_sections(),
            home_blocks: default_home_blocks(),
            window_width: None,
            window_height: None,
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
        self.language = sanitize_language_preference(&self.language);
        if self.lastfm_api_key.is_empty() && !self.scrobbling.lastfm.api_key.is_empty() {
            self.lastfm_api_key = self.scrobbling.lastfm.api_key.clone();
        } else if self.scrobbling.lastfm.api_key.is_empty() && !self.lastfm_api_key.is_empty() {
            self.scrobbling.lastfm.api_key = self.lastfm_api_key.clone();
        }
        self.layout.sanitize();
        self.sidebar.sanitize();
        self.sources.sanitize();
        if let Some((width, height)) = sanitized_window_size(self.window_width, self.window_height)
        {
            self.window_width = Some(width);
            self.window_height = Some(height);
        } else {
            self.window_width = None;
            self.window_height = None;
        }
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
pub fn sanitized_window_size(width: Option<i32>, height: Option<i32>) -> Option<(i32, i32)> {
    let (width, height) = (width?, height?);
    if width < MIN_RESTORED_WINDOW_WIDTH || height < MIN_RESTORED_WINDOW_HEIGHT {
        return None;
    }
    Some((
        width.clamp(MIN_RESTORED_WINDOW_WIDTH, MAX_RESTORED_WINDOW_WIDTH),
        height.clamp(MIN_RESTORED_WINDOW_HEIGHT, MAX_RESTORED_WINDOW_HEIGHT),
    ))
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
