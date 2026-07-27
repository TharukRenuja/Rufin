mod app;
mod layout;
mod persistence;
mod sidebar;

pub use app::{Settings, SettingsHandle, SettingsPort, default_home_blocks};
pub use layout::{
    DEFAULT_LEFT_SIDEBAR_WIDTH, DEFAULT_RIGHT_SIDEBAR_WIDTH, DEFAULT_WINDOW_HEIGHT,
    DEFAULT_WINDOW_WIDTH, LayoutProfile, LayoutSettings, LeftSidebarMode, LibraryField,
    LibraryLayout, LibraryListKey, LibraryListSettings, LibraryListSettingsEntry,
    MAX_LEFT_SIDEBAR_WIDTH, MAX_NARROW_LAYOUT_THRESHOLD, MAX_RESTORED_WINDOW_HEIGHT,
    MAX_RESTORED_WINDOW_WIDTH, MAX_RIGHT_SIDEBAR_WIDTH, MIN_LEFT_SIDEBAR_WIDTH,
    MIN_NARROW_LAYOUT_THRESHOLD, MIN_RIGHT_SIDEBAR_WIDTH, RightSidebarMode, SidebarRouteItem,
    SidebarRouteItemSettings, SidebarSettings, ThemePreference, available_grid_fields,
    available_row_fields, default_library_list_settings,
};
pub(crate) use persistence::SettingsState;
pub use sidebar::{
    ExternalSiteLinkSettings, available_detail_track_fields, available_sort_fields,
    sanitized_window_size,
};
#[cfg(test)]
mod tests;
