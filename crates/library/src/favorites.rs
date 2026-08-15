//! One accepted favorite operation for Track, Album, and Artist.
//!
//! Local persists Rufin's true-row boolean. Remote sources acknowledge their
//! authoritative value, which Library writes into the accepted source facts.
//! Neither path creates a second accepted UI map.

use crate::{
    AcceptedHomeChange, AcceptedLibraryChange, Album, Artist, FavoriteItemId, Library,
    LibraryResult,
};

pub fn rating_from_five_star(value: f64) -> Option<u8> {
    (value.is_finite() && value > 0.0).then(|| (value * 2.0).round().clamp(1.0, 10.0) as u8)
}

pub fn rating_from_ten_point(value: f64) -> Option<u8> {
    (value.is_finite() && value > 0.0).then(|| value.round().clamp(1.0, 10.0) as u8)
}

pub fn rating_to_whole_star(rating: Option<u8>) -> u8 {
    rating.map_or(0, |rating| rating.clamp(1, 10).div_ceil(2))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingFavorite {
    pub item: FavoriteItemId,
    pub favorite: bool,
    pub attempts: u32,
}

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
    pub fn set_rating(
        &self,
        item: FavoriteItemId,
        rating: Option<u8>,
    ) -> LibraryResult<AcceptedLibraryChange> {
        self.store
            .set_rating(self.source_id().clone(), item.clone(), rating)?;
        let mut accepted = self.replace_rating(&item, rating)?;
        accepted.home = AcceptedHomeChange::Rebuild;
        accepted.download_coverage_changed = true;
        Ok(accepted)
    }

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

    pub fn queue_remote_favorite(
        &self,
        item: FavoriteItemId,
        favorite: bool,
        next_attempt_at: i64,
    ) -> LibraryResult<AcceptedLibraryChange> {
        let previous = match &item {
            FavoriteItemId::Track(id) => {
                self.track(id)?
                    .ok_or_else(|| crate::LibraryQueryError::MissingItem {
                        kind: "track",
                        id: id.to_string(),
                    })?
                    .favorite
            }
            FavoriteItemId::Album(id) => {
                self.album(id)?
                    .ok_or_else(|| crate::LibraryQueryError::MissingItem {
                        kind: "album",
                        id: id.to_string(),
                    })?
                    .favorite
            }
            FavoriteItemId::Artist(id) => {
                self.artist(id)?
                    .ok_or_else(|| crate::LibraryQueryError::MissingItem {
                        kind: "artist",
                        id: id.to_string(),
                    })?
                    .favorite
            }
        };
        let fallback = self.favorite_value_if_derived(&item)?;
        self.store.queue_remote_favorite(
            self.source_id().clone(),
            item.clone(),
            favorite,
            previous,
            next_attempt_at,
            fallback,
        )?;
        let mut accepted = self.replace_favorite(&item, favorite)?;
        accepted.home = AcceptedHomeChange::Favorite(item);
        accepted.download_coverage_changed = true;
        Ok(accepted)
    }

    pub fn due_remote_favorites(
        &self,
        now: i64,
        limit: usize,
    ) -> LibraryResult<Vec<PendingFavorite>> {
        if now < 0 || limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .store
            .due_remote_favorites(self.source_id().clone(), now, limit.min(500))?)
    }

    pub fn complete_remote_favorite(
        &self,
        item: FavoriteItemId,
        favorite: bool,
    ) -> LibraryResult<()> {
        self.store
            .complete_remote_favorite(self.source_id().clone(), item, favorite)?;
        Ok(())
    }

    pub fn defer_remote_favorite(
        &self,
        item: FavoriteItemId,
        favorite: bool,
        next_attempt_at: i64,
    ) -> LibraryResult<()> {
        self.store.defer_remote_favorite(
            self.source_id().clone(),
            item,
            favorite,
            next_attempt_at,
        )?;
        Ok(())
    }

    pub fn reject_remote_favorite(
        &self,
        item: FavoriteItemId,
        favorite: bool,
    ) -> LibraryResult<Option<AcceptedLibraryChange>> {
        let Some(previous) =
            self.store
                .reject_remote_favorite(self.source_id().clone(), item.clone(), favorite)?
        else {
            return Ok(None);
        };
        let mut accepted = self.replace_favorite(&item, previous)?;
        accepted.home = AcceptedHomeChange::Favorite(item);
        accepted.download_coverage_changed = true;
        Ok(Some(accepted))
    }
}
