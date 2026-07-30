//! One accepted favorite operation for Track, Album, and Artist.
//!
//! Local persists Rufin's true-row boolean. Remote sources acknowledge their
//! authoritative value, which Library writes into the accepted source facts.
//! Neither path creates a second accepted UI map.

use crate::{
    AcceptedLibraryChange, Album, Artist, FavoriteItemId, Library, LibraryResult, LoadedLibrary,
};

pub(crate) enum FavoriteValue {
    Album(Album),
    Artist(Artist),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FavoriteAcceptance {
    RufinOwned {
        item: FavoriteItemId,
        favorite: bool,
    },
    SourceAcknowledged {
        item: FavoriteItemId,
        favorite: bool,
    },
}

impl Library {
    pub fn accept_favorite(
        &self,
        loaded: &std::sync::Arc<LoadedLibrary>,
        acceptance: FavoriteAcceptance,
    ) -> LibraryResult<AcceptedLibraryChange> {
        let (item_id, favorite, local) = match acceptance {
            FavoriteAcceptance::RufinOwned { item, favorite } => (item, favorite, true),
            FavoriteAcceptance::SourceAcknowledged { item, favorite } => (item, favorite, false),
        };
        let fallback = loaded.favorite_value_if_derived(&item_id)?;
        self.store.set_favorite(
            loaded.source_id().clone(),
            item_id.clone(),
            favorite,
            local,
            fallback,
        )?;
        loaded
            .replace_favorite(&item_id, favorite)
            .map_err(Into::into)
    }
}
