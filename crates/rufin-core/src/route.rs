use crate::domain::{AlbumId, ArtistId, GenreId, PlaylistId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DensityMode {
    Auto,
    Normal,
    Compact,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EffectiveDensity {
    Normal,
    Compact,
}

impl DensityMode {
    pub const AUTO_COMPACT_THRESHOLD: i32 = 1_180;

    pub fn resolve(self, window_width: i32) -> EffectiveDensity {
        match self {
            Self::Auto if window_width < Self::AUTO_COMPACT_THRESHOLD => EffectiveDensity::Compact,
            Self::Auto | Self::Normal => EffectiveDensity::Normal,
            Self::Compact => EffectiveDensity::Compact,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SearchKind {
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
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
    AlbumArtists,
    Genres,
    GenreDetail(GenreId),
    Playlists,
    PlaylistDetail(PlaylistId),
    Search { query: String, kind: SearchKind },
    Settings,
}

impl Route {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Favorites => "Favorites",
            Self::Albums => "Albums",
            Self::AlbumDetail(_) => "Album",
            Self::Tracks => "Tracks",
            Self::Artists => "Artists",
            Self::ArtistDetail(_) => "Artist",
            Self::AlbumArtists => "Album Artists",
            Self::Genres => "Genres",
            Self::GenreDetail(_) => "Genre",
            Self::Playlists => "Playlists",
            Self::PlaylistDetail(_) => "Playlist",
            Self::Search { .. } => "Search",
            Self::Settings => "Settings",
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
    use super::{DensityMode, EffectiveDensity, Route, RouteStack};

    #[test]
    fn route_stack_tracks_back_and_forward_history() {
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(Route::Albums);
        stack.navigate(Route::Tracks);

        assert_eq!(stack.current(), &Route::Tracks);
        assert_eq!(stack.back(), Some(&Route::Albums));
        assert_eq!(stack.back(), Some(&Route::Home));
        assert_eq!(stack.back(), None);
        assert_eq!(stack.forward(), Some(&Route::Albums));

        stack.navigate(Route::Settings);

        assert!(!stack.can_forward());
        assert_eq!(stack.current(), &Route::Settings);
    }

    #[test]
    fn repeated_route_navigation_is_ignored() {
        let mut stack = RouteStack::new(Route::Home);
        stack.navigate(Route::Home);

        assert!(!stack.can_back());
        assert_eq!(stack.current(), &Route::Home);
    }

    #[test]
    fn route_stack_supports_opaque_string_ids() {
        let album_route = Route::AlbumDetail(crate::domain::AlbumId::new("jellyfin:album:abc"));
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(album_route.clone());

        assert_eq!(stack.current(), &album_route);
    }

    #[test]
    fn density_auto_uses_compact_threshold() {
        assert_eq!(DensityMode::Auto.resolve(900), EffectiveDensity::Compact);
        assert_eq!(DensityMode::Auto.resolve(1_500), EffectiveDensity::Normal);
        assert_eq!(DensityMode::Normal.resolve(900), EffectiveDensity::Normal);
        assert_eq!(
            DensityMode::Compact.resolve(1_500),
            EffectiveDensity::Compact
        );
    }
}
