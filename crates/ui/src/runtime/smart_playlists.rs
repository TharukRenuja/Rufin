//! Operation-scoped smart-playlist changes.
//!
//! Routes read evaluated smart playlists from the selected LoadedLibrary. This
//! handle only submits durable definition edits to Rufin.

use std::sync::Arc;

use library::{SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistId};

pub trait SmartPlaylistPort: Send + Sync {
    fn create(&self, name: String, definition: SmartPlaylistDefinition);
    fn update(&self, id: SmartPlaylistId, name: String, definition: SmartPlaylistDefinition);
    fn delete(&self, id: SmartPlaylistId);
    fn restore_builtin(&self, builtin: SmartPlaylistBuiltin);
    fn move_relative(&self, dragged: SmartPlaylistId, target: SmartPlaylistId, after: bool);
}

pub type SmartPlaylistHandle = Arc<dyn SmartPlaylistPort>;
