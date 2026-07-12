pub mod route;
pub mod settings;

pub(crate) const fn msgid(message: &'static str) -> &'static str {
    message
}

pub use route::{FolderPathItem, Route, RouteStack, SearchKind};
pub use settings::{
    DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH, ExternalSiteLinkSettings, LayoutProfile,
    LayoutSettings, LeftSidebarMode, LibraryField, LibraryLayout, LibraryListKey,
    LibraryListSettings, LibraryListSettingsEntry, LibrarySourceSelection, LibrarySourceSettings,
    LocalLibraryFolder, MAX_AUTO_DJ_REFILL_THRESHOLD, MAX_NARROW_LAYOUT_THRESHOLD,
    MIN_AUTO_DJ_REFILL_THRESHOLD, MIN_NARROW_LAYOUT_THRESHOLD, RightSidebarMode,
    SYSTEM_LANGUAGE_PREFERENCE, SecretStorageMode, SidebarRouteItem, SidebarRouteItemSettings,
    SidebarSettings, ThemePreference, TrackSortKey, TrackTableColumn, TrackTableSettings,
    available_detail_track_fields, available_grid_fields, available_row_fields,
    available_sort_fields, default_language_preference, sanitize_language_preference,
    sanitized_window_size,
};

pub fn format_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_track_duration() {
        assert_eq!(super::format_duration(0), "0:00");
        assert_eq!(super::format_duration(185), "3:05");
        assert_eq!(super::format_duration(3_661), "61:01");
    }
}
