mod covers;

mod discovery;

mod random;

include!("controller/root/types.rs");
include!("controller/root/controller_settings.rs");
include!("controller/root/cached_library_api.rs");
include!("controller/root/controller_bootstrap.rs");
include!("controller/root/app_cache_commands.rs");
include!("controller/root/refresh_commands.rs");
include!("controller/root/queue_commands.rs");
include!("controller/root/auto_dj_commands.rs");
include!("controller/root/playback_commands.rs");
include!("controller/root/server_cache_commands.rs");
include!("controller/root/server_lifecycle_commands.rs");
include!("controller/root/local_source_commands.rs");
include!("controller/root/source_selection.rs");
include!("controller/root/server_local_access_commands.rs");
include!("controller/root/folder_search_commands.rs");
include!("controller/root/library_mutations.rs");
include!("controller/root/playlist_commands.rs");
include!("controller/root/lyrics_commands.rs");
include!("controller/root/queue_mutation.rs");
include!("controller/root/auto_dj.rs");
include!("controller/root/queue_state.rs");
include!("controller/root/playback_runtime.rs");
include!("controller/root/playback_reporting.rs");
include!("controller/root/playback_advance.rs");
include!("controller/root/sync_command.rs");
include!("controller/root/controller_startup.rs");
include!("controller/root/cached_reads.rs");
include!("controller/root/sync_requests.rs");
include!("controller/root/playback_queue.rs");

#[cfg(test)]
mod tests {
    include!("controller/root/tests_01.rs");
    include!("controller/root/tests_02.rs");
    include!("controller/root/tests_03.rs");
    include!("controller/root/tests_04.rs");
}
