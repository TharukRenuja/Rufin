use library::{
    ActiveLibraryQuery, AlbumId, ArtistId, FolderDetail, FolderId, HomeSection, HomeSectionKind,
    PlaylistId, SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistId, SourceFeatureOwner,
    SourceId, Track, TrackId,
};
use sources::SourcePlaylistOperation;
use ui::runtime::library::LibraryPort;

use super::super::root::LibraryCommands;

impl LibraryPort for LibraryCommands {
    fn query(&self, source_id: SourceId) -> ActiveLibraryQuery {
        self.library_query(source_id)
    }

    fn folder(&self, path: &[FolderId]) -> Result<FolderDetail, String> {
        self.folder_for_active(path)
    }

    fn set_album_favorite(&self, album_id: AlbumId, favorite: bool) {
        LibraryCommands::set_album_favorite(self, album_id, favorite);
    }

    fn set_artist_favorite(&self, artist_id: ArtistId, favorite: bool) {
        LibraryCommands::set_artist_favorite(self, artist_id, favorite);
    }

    fn set_track_favorite(&self, track_id: TrackId, favorite: bool) {
        LibraryCommands::set_track_favorite(self, track_id, favorite);
    }

    fn playlist_creation_supported(&self) -> bool {
        LibraryCommands::playlist_creation_supported(self)
    }

    fn playlist_operation_supported(
        &self,
        owner: SourceFeatureOwner,
        operation: SourcePlaylistOperation,
    ) -> bool {
        LibraryCommands::playlist_operation_supported(self, owner, operation)
    }

    fn create_playlist(&self, name: String, tracks: Vec<Track>) {
        LibraryCommands::create_playlist(self, name, tracks);
    }

    fn rename_playlist(&self, playlist_id: PlaylistId, name: String) {
        LibraryCommands::rename_playlist(self, playlist_id, name);
    }

    fn delete_playlist(&self, playlist_id: PlaylistId) {
        LibraryCommands::delete_playlist(self, playlist_id);
    }

    fn add_tracks_to_playlist(&self, playlist_id: PlaylistId, tracks: Vec<Track>) {
        LibraryCommands::add_tracks_to_playlist(self, playlist_id, tracks);
    }

    fn remove_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String) {
        LibraryCommands::remove_playlist_entry(self, playlist_id, entry_id);
    }

    fn move_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String, new_index: usize) {
        LibraryCommands::move_playlist_entry(self, playlist_id, entry_id, new_index);
    }

    fn save_smart_playlist(&self, name: String, definition: SmartPlaylistDefinition) {
        LibraryCommands::save_smart_playlist(self, name, definition);
    }

    fn update_smart_playlist(
        &self,
        playlist_id: SmartPlaylistId,
        name: String,
        definition: SmartPlaylistDefinition,
    ) {
        LibraryCommands::update_smart_playlist(self, playlist_id, name, definition);
    }

    fn delete_smart_playlist(&self, playlist_id: SmartPlaylistId) {
        LibraryCommands::delete_smart_playlist(self, playlist_id);
    }

    fn restore_builtin_smart_playlist(&self, builtin: SmartPlaylistBuiltin) {
        LibraryCommands::restore_builtin_smart_playlist(self, builtin);
    }

    fn move_smart_playlist(
        &self,
        dragged_id: SmartPlaylistId,
        target_id: SmartPlaylistId,
        after: bool,
    ) {
        LibraryCommands::move_smart_playlist(self, dragged_id, target_id, after);
    }

    fn refresh_home_section(&self, source_id: SourceId, kind: HomeSectionKind) {
        LibraryCommands::refresh_home_section_for_active(self, source_id, kind);
    }

    fn prefetch_explore(&self, source_id: SourceId) {
        LibraryCommands::prefetch_explore_for_active(self, source_id);
    }

    fn save_explore_projection(&self, source_id: SourceId, section: HomeSection) {
        LibraryCommands::save_explore_projection_for_active(self, source_id, section);
    }
}
