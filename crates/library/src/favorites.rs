//! One accepted favorite operation for Track, Album, and Artist.
//!
//! Local persists Rufin's true-row boolean. Remote sources acknowledge their
//! authoritative value, which Library writes into the accepted source facts.
//! Neither path creates a second accepted UI map.

use crate::{
    AcceptedHomeChange, AcceptedLibraryChange, Album, Artist, FavoriteItemId, Library,
    LibraryResult,
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
        acceptance: FavoriteAcceptance,
    ) -> LibraryResult<AcceptedLibraryChange> {
        let (item_id, favorite, local) = match acceptance {
            FavoriteAcceptance::RufinOwned { item, favorite } => (item, favorite, true),
            FavoriteAcceptance::SourceAcknowledged { item, favorite } => (item, favorite, false),
        };
        let fallback = self.favorite_value_if_derived(&item_id)?;
        self.store.set_favorite(
            self.source_id().clone(),
            item_id.clone(),
            favorite,
            local,
            fallback,
        )?;
        let mut accepted = self.replace_favorite(&item_id, favorite)?;
        accepted.home = AcceptedHomeChange::Favorite(item_id);
        accepted.download_coverage_changed = true;
        Ok(accepted)
    }
}
