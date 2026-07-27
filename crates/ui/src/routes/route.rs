use library::{AlbumId, ArtistId, FolderId, GenreId, MoodId, PlaylistId, SmartPlaylistId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) struct FolderPathItem {
    pub(crate) id: FolderId,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) enum Route {
    Home,
    Favorites,
    History,
    Albums,
    AlbumDetail(AlbumId),
    Tracks,
    Artists,
    ArtistDetail(ArtistId),
    ArtistDiscography(ArtistId),
    ArtistTracks(ArtistId),
    AlbumArtists,
    Genres,
    GenreDetail(GenreId),
    Moods,
    MoodDetail(MoodId),
    Folders { path: Vec<FolderPathItem> },
    Playlists,
    PlaylistDetail(PlaylistId),
    SmartPlaylists,
    SmartPlaylistDetail(SmartPlaylistId),
}
