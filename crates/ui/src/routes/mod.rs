mod album_detail;
mod album_detail_view;
mod artist;
mod artist_releases;
pub(super) mod cards;
pub(crate) mod collection_context;
mod collection_routes;
pub(crate) mod collections;
mod columns;
pub(crate) mod detail_links;
mod detail_showcase;
pub(super) mod favorites;
mod folders;
mod genre_detail;
mod grid_cells;
mod grouped_detail;
pub(crate) mod home;
mod home_layout;
pub(crate) mod library_fields;
pub(crate) mod models;
mod mood_detail;
pub(crate) mod named_collections;
mod play_context;
pub(crate) mod playlist_detail;
pub(crate) mod playlist_entries;
pub(crate) mod playlist_picker;
pub(super) mod release_kind;
pub(crate) mod route;
pub(crate) mod route_layout;
mod route_shell;
mod routes;
mod table_links;
mod table_sizing;
mod track_model;

use std::cell::{Cell, RefCell};

use library::{ActiveLibraryQuery, HomeSection, SourceId};

pub(crate) use album_detail_view::load_album_detail_for_revision;
pub(crate) use collection_routes::{complete_prepared_items, load_complete_cached_items};
pub(crate) use playlist_detail::load_playlist_detail_refresh;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceHomeSection {
    pub(crate) source_id: SourceId,
    pub(crate) section: HomeSection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PreparedHomeExplore {
    Rotation(SourceHomeSection),
    Prefetched(SourceHomeSection),
}

impl PreparedHomeExplore {
    pub(crate) fn projection(&self) -> &SourceHomeSection {
        match self {
            Self::Rotation(projection) | Self::Prefetched(projection) => projection,
        }
    }
}

pub(crate) struct LibraryState {
    pub(crate) query: RefCell<Option<ActiveLibraryQuery>>,
    pub(crate) home_showcase_seed: Cell<u64>,
    pub(crate) next_home_showcase_seed: Cell<u64>,
    pub(crate) prepared_home_explore: RefCell<Option<PreparedHomeExplore>>,
    pub(crate) pending_home_explore: RefCell<Option<SourceHomeSection>>,
}

#[cfg(test)]
mod route_tests;
