use super::*;

impl AppController {
    pub(in crate::controller) fn start_sync(&self, saved: SavedServer) {
        start_sync_thread(self.sync_context(), saved);
    }
}
