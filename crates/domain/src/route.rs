use crate::{
    domain::{AlbumId, ArtistId, FolderId, GenreId, MoodId, PlaylistId, SmartPlaylistId},
    msgid,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SearchKind {
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderPathItem {
    pub id: FolderId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Route {
    Home,
    Favorites,
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
    Search { query: String, kind: SearchKind },
}

impl Route {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Home => msgid("Home"),
            Self::Favorites => msgid("Favorites"),
            Self::Albums => msgid("Albums"),
            Self::AlbumDetail(_) => msgid("Album"),
            Self::Tracks => msgid("Tracks"),
            Self::Artists => msgid("Artists"),
            Self::ArtistDetail(_) => msgid("Artist"),
            Self::ArtistDiscography(_) => msgid("Discography"),
            Self::ArtistTracks(_) => msgid("Tracks"),
            Self::AlbumArtists => msgid("Album Artists"),
            Self::Genres => msgid("Genres"),
            Self::GenreDetail(_) => msgid("Genre"),
            Self::Moods => msgid("Moods"),
            Self::MoodDetail(_) => msgid("Mood"),
            Self::Folders { .. } => msgid("Folders"),
            Self::Playlists => msgid("Playlists"),
            Self::PlaylistDetail(_) => msgid("Playlist"),
            Self::SmartPlaylists => msgid("Smart Playlists"),
            Self::SmartPlaylistDetail(_) => msgid("Smart Playlist"),
            Self::Search { .. } => msgid("Search"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteStack {
    back: Vec<Route>,
    current: Route,
    forward: Vec<Route>,
}

impl RouteStack {
    pub fn new(initial: Route) -> Self {
        Self {
            back: Vec::new(),
            current: initial,
            forward: Vec::new(),
        }
    }

    pub fn current(&self) -> &Route {
        &self.current
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn navigate(&mut self, route: Route) {
        if self.current == route {
            return;
        }

        let previous = std::mem::replace(&mut self.current, route);
        self.back.push(previous);
        self.forward.clear();
    }

    pub fn back(&mut self) -> Option<&Route> {
        let previous = self.back.pop()?;
        let current = std::mem::replace(&mut self.current, previous);
        self.forward.push(current);
        Some(&self.current)
    }

    pub fn forward(&mut self) -> Option<&Route> {
        let next = self.forward.pop()?;
        let current = std::mem::replace(&mut self.current, next);
        self.back.push(current);
        Some(&self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::{Route, RouteStack};

    #[test]
    fn route_track_history() {
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(Route::Albums);
        stack.navigate(Route::Tracks);

        assert_eq!(stack.current(), &Route::Tracks);
        assert_eq!(stack.back(), Some(&Route::Albums));
        assert_eq!(stack.back(), Some(&Route::Home));
        assert_eq!(stack.back(), None);
        assert_eq!(stack.forward(), Some(&Route::Albums));

        stack.navigate(Route::Favorites);

        assert!(!stack.can_forward());
        assert_eq!(stack.current(), &Route::Favorites);
    }

    #[test]
    fn repeated_route_navigation_is_ignored() {
        let mut stack = RouteStack::new(Route::Home);
        stack.navigate(Route::Home);

        assert!(!stack.can_back());
        assert_eq!(stack.current(), &Route::Home);
    }

    #[test]
    fn route_support_id() {
        let album_route = Route::AlbumDetail(crate::domain::AlbumId::new("jellyfin:album:abc"));
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(album_route.clone());

        assert_eq!(stack.current(), &album_route);
    }

    #[test]
    fn route_keep_history() {
        let root = Route::Folders { path: Vec::new() };
        let nested = Route::Folders {
            path: vec![super::FolderPathItem {
                id: crate::domain::FolderId::new("jellyfin:folder:music"),
                name: "Music".to_string(),
            }],
        };
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(root.clone());
        stack.navigate(nested.clone());

        assert_eq!(stack.current(), &nested);
        assert_eq!(stack.back(), Some(&root));
    }
}
