use std::{rc::Rc, sync::Arc};

use crate::LibraryListKey;
use crate::shell::Shell;
use crate::shell::route::{MountedRoute, MountedRouteDeltaApplier};
use ::library::play_context::PlayContextDescriptor;
use ::library::{ActiveLibraryQuery, LibraryDelta, Track};

use super::collection_routes::{CollectionLoader, MountedRouteRefresh};
use super::play_context::selected_music_folder_id;
use super::route_layout::PRIMARY_ROUTE_HORIZONTAL_INSET;
use super::route_shell::LibraryPageShellOptions;
use super::routes::SearchableTrackOptions;

struct MountedFavoritesState {
    projection: super::routes::TrackListProjection,
    page_shell: super::route_shell::LibraryPageShell,
}

impl MountedFavoritesState {
    fn replace(&self, favorites: Vec<Track>) {
        self.projection.replace(favorites);
        self.page_shell.set_empty(self.projection.source_is_empty());
    }
}

pub(crate) fn favorites_delta_affects(delta: &LibraryDelta) -> bool {
    delta.reset.is_some()
        || !delta.tracks.added.is_empty()
        || !delta.tracks.deleted.is_empty()
        || !delta.tracks.fields.is_empty()
        || !delta.tracks.metadata.is_empty()
        || !delta.tracks.stats.is_empty()
        || !delta.tracks.favorite.is_empty()
        || !delta.tracks.cover_refs.is_empty()
}

impl Shell {
    pub(crate) fn favorites_route_from_prepared(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        favorites: Vec<Track>,
    ) -> MountedRoute {
        let projection = self.searchable_track_collection(
            favorites,
            LibraryListKey::FavoriteTracks,
            SearchableTrackOptions {
                on_visible_count_changed: None,
                source_descriptor: Some(PlayContextDescriptor::Favorites {
                    music_folder_id: selected_music_folder_id(self),
                }),
                favorites_only: false,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                fixed_layout: None,
            },
        );
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::FavoriteTracks,
            empty: projection.source_is_empty(),
            empty_body: "Favorite tracks will appear here after you add them.",
            search: projection.search(),
            content: projection.scrolling_widget(),
        });
        let state = Rc::new(MountedFavoritesState {
            projection: projection.clone(),
            page_shell: page_shell.clone(),
        });
        let apply_loaded: Rc<dyn Fn(Vec<Track>)> = {
            let state = Rc::clone(&state);
            Rc::new(move |favorites| state.replace(favorites))
        };
        let load_query = library_query;
        let load_items: CollectionLoader<Track> = Arc::new(move || {
            load_query.favorite_tracks().unwrap_or_else(|error| {
                tracing::warn!(%error, "failed to refresh favorite tracks projection");
                Vec::new()
            })
        });
        let refresh = MountedRouteRefresh::new(
            Rc::downgrade(&apply_loaded),
            load_items,
            "mounted Favorites",
        );
        let affected_by = Rc::new(favorites_delta_affects);
        let apply_delta = {
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &::library::LibraryDelta| {
                let _ = &apply_loaded;
                refresh.request();
            }) as MountedRouteDeltaApplier
        };
        let resume = {
            let shell = Rc::clone(self);
            let projection = projection.clone();
            let page_shell = page_shell.clone();
            Rc::new(move || {
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::FavoriteTracks);
                projection.apply_library_list_settings(LibraryListKey::FavoriteTracks, &settings);
                page_shell.apply_library_list_settings(LibraryListKey::FavoriteTracks, &settings);
            })
        };
        MountedRoute::new(page_shell.widget(), affected_by, apply_delta, resume)
    }
}

#[cfg(test)]
mod tests {
    use ::library::{LibraryDelta, TrackDelta, TrackId};

    use super::favorites_delta_affects;

    #[test]
    fn favorites_preserves_the_selective_track_invalidation_boundary() {
        let track_id = TrackId::fake(1);
        assert!(!favorites_delta_affects(&LibraryDelta {
            tracks: TrackDelta {
                skip_stats: vec![track_id.clone()],
                ..TrackDelta::default()
            },
            ..LibraryDelta::default()
        }));
        assert!(favorites_delta_affects(&LibraryDelta {
            tracks: TrackDelta {
                metadata: vec![track_id.clone()],
                ..TrackDelta::default()
            },
            ..LibraryDelta::default()
        }));
        assert!(favorites_delta_affects(&LibraryDelta {
            tracks: TrackDelta {
                stats: vec![track_id.clone()],
                ..TrackDelta::default()
            },
            ..LibraryDelta::default()
        }));
        assert!(favorites_delta_affects(&LibraryDelta {
            tracks: TrackDelta {
                favorite: vec![track_id],
                ..TrackDelta::default()
            },
            ..LibraryDelta::default()
        }));
    }
}
