mod covers;

mod discovery;

mod random;

include!("controller/root/types.rs");
include!("controller/root/store_handle_01.rs");
include!("controller/root/store_handle_02.rs");
include!("controller/root/store_handle_03.rs");
include!("controller/root/store_handle_04.rs");
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
