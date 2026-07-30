pub(crate) mod build;
pub(crate) mod chrome;
mod diagnostics;
pub(crate) mod layout;
pub(crate) mod navigation;
use crate::downloads::DownloadsState;
use crate::favorites::FavoriteState;
use crate::player::PlayerDesktopWidgets;
use crate::player::desktop::DesktopState;
use crate::player::lyrics::state::LyricsState;
use crate::player::queue::QueueState;
use crate::player::right_panel::RightPanelWidgets;
use crate::player::state::PlaybackState;
use crate::preferences::PreferencesState;
use crate::preferences::source::SourceState;
use crate::routes::LibraryState;
use crate::routes::playlist_picker::PlaylistPickerState;
use crate::runtime::{DiagnosticsHandle, ProductHandles};
use crate::settings::SettingsState;
use actions::ControlFeedbackState;
use chrome::WindowChrome;
use cover::ArtworkState;
use layout::ShellLayoutState;
use localization::LocalizationState;
use navigation::{NavigationState, NavigationWidgets};
use startup::StartupState;

pub(crate) mod actions;
pub(crate) mod cover;
mod events;
mod localization;
pub(crate) mod route;
mod route_position;
mod startup;
mod window_state;

use route::RouteViewport;

pub(crate) struct Shell {
    pub(crate) diagnostics: DiagnosticsHandle,
    pub(crate) settings: SettingsState,
    pub(crate) navigation: NavigationState,
    pub(crate) library: LibraryState,
    pub(crate) source: SourceState,
    startup: StartupState,
    pub(crate) playback: PlaybackState,
    pub(crate) queue: QueueState,
    pub(crate) lyrics: LyricsState,
    pub(crate) preferences: PreferencesState,
    pub(crate) playlist_picker: PlaylistPickerState,
    pub(crate) downloads: DownloadsState,
    pub(crate) control_feedback: ControlFeedbackState,
    localization: LocalizationState,
    pub(crate) desktop: DesktopState,
    artwork: ArtworkState,
    pub(crate) favorites: FavoriteState,
    pub(crate) products: ProductHandles,
    pub(crate) chrome: WindowChrome,
    layout_state: ShellLayoutState,
    pub(crate) navigation_view: NavigationWidgets,
    pub(crate) route_viewport: RouteViewport,
    pub(crate) right_panel: RightPanelWidgets,
    pub(crate) player_view: PlayerDesktopWidgets,
}
