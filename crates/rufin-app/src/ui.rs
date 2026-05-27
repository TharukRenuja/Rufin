mod artist;

mod cards;

mod chrome;

mod discord;

mod favorites;

mod folders;

mod fullscreen_player;

mod home;

mod layout;

mod library;

mod local_access_mapping;

mod login;

#[cfg(unix)]
mod mpris;

mod navigation;

mod paging;

mod player;

mod player_icons;

mod preferences;

mod queue;

mod random_play;

mod right_panel;

mod settings_persistence;

mod source_selector;

mod update_notice;

include!("ui/root/types.rs");
include!("ui/root/build.rs");
include!("ui/root/home_visible_sections.rs");
include!("ui/root/route_navigation.rs");
include!("ui/root/folder_route_state.rs");
include!("ui/root/home_route_refresh.rs");
include!("ui/root/responsive_layout_state.rs");
include!("ui/root/startup_reveal.rs");
include!("ui/root/sidebar_route_controls.rs");
include!("ui/root/responsive_route_render.rs");
include!("ui/root/lyrics_playback_state.rs");
include!("ui/root/lyrics_highlight_timers.rs");
include!("ui/root/route_rendering.rs");
include!("ui/root/favorite_controls.rs");
include!("ui/root/album_detail_view.rs");
include!("ui/root/track_table.rs");
include!("ui/root/new_playlist_dialog.rs");
include!("ui/root/genre_detail_view.rs");
include!("ui/root/playlist_detail_view.rs");
include!("ui/root/playlist_rename_dialog.rs");
include!("ui/root/grouped_detail_view.rs");
include!("ui/root/search_view.rs");
include!("ui/root/lyrics_panel.rs");
include!("ui/root/empty_states.rs");
include!("ui/root/cover_tiles.rs");
include!("ui/root/cover_warming.rs");
include!("ui/root/cover_cache_lookup.rs");
include!("ui/root/cover_decode_queue.rs");
include!("ui/root/perf_recording.rs");
include!("ui/root/track_table_popover.rs");
include!("ui/root/cover_size_helpers.rs");
include!("ui/root/cover_startup.rs");
include!("ui/root/shell_navigation.rs");
include!("ui/root/home_refresh.rs");
include!("ui/root/layout_rendering.rs");

#[cfg(test)]
mod tests {
    include!("ui/root/shell_tests.rs");
}
