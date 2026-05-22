mod artist;

mod cards;

mod chrome;

mod discord;

mod favorites;

mod folders;

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

include!("ui/root/types.rs");
include!("ui/root/build.rs");
include!("ui/root/perf_monitor_01.rs");
include!("ui/root/perf_monitor_02.rs");
include!("ui/root/perf_monitor_03.rs");
include!("ui/root/perf_monitor_04.rs");
include!("ui/root/cover_startup.rs");
include!("ui/root/shell_navigation.rs");
include!("ui/root/home_refresh.rs");
include!("ui/root/layout_rendering.rs");

#[cfg(test)]
mod tests {
    include!("ui/root/tests_01.rs");
}
