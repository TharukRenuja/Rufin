use std::sync::Arc;

use library::{
    ActiveLibraryQuery, AlbumId, ArtistId, FolderDetail, FolderId, HomeSection, HomeSectionKind,
    PlaylistId, SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistId, SourceFeatureOwner,
    SourceId, Track, TrackId,
};
use sources::SourcePlaylistOperation;

pub trait LibraryPort: Send + Sync {
    fn query(&self, source_id: SourceId) -> ActiveLibraryQuery;
    fn folder(&self, path: &[FolderId]) -> Result<FolderDetail, String>;

    fn set_album_favorite(&self, album_id: AlbumId, favorite: bool);
    fn set_artist_favorite(&self, artist_id: ArtistId, favorite: bool);
    fn set_track_favorite(&self, track_id: TrackId, favorite: bool);

    fn playlist_creation_supported(&self) -> bool;
    fn playlist_operation_supported(
        &self,
        owner: SourceFeatureOwner,
        operation: SourcePlaylistOperation,
    ) -> bool;
    fn create_playlist(&self, name: String, tracks: Vec<Track>);
    fn rename_playlist(&self, playlist_id: PlaylistId, name: String);
    fn delete_playlist(&self, playlist_id: PlaylistId);
    fn add_tracks_to_playlist(&self, playlist_id: PlaylistId, tracks: Vec<Track>);
    fn remove_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String);
    fn move_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String, new_index: usize);

    fn save_smart_playlist(&self, name: String, definition: SmartPlaylistDefinition);
    fn update_smart_playlist(
        &self,
        playlist_id: SmartPlaylistId,
        name: String,
        definition: SmartPlaylistDefinition,
    );
    fn delete_smart_playlist(&self, playlist_id: SmartPlaylistId);
    fn restore_builtin_smart_playlist(&self, builtin: SmartPlaylistBuiltin);
    fn move_smart_playlist(
        &self,
        dragged_id: SmartPlaylistId,
        target_id: SmartPlaylistId,
        after: bool,
    );

    fn refresh_home_section(&self, source_id: SourceId, kind: HomeSectionKind);
    fn prefetch_explore(&self, source_id: SourceId);
    fn save_explore_projection(&self, source_id: SourceId, section: HomeSection);
}

pub type LibraryHandle = Arc<dyn LibraryPort>;
