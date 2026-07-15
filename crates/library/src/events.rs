use crate::LibraryDelta;
use crate::{FavoriteItemId, HomeSection, SourceId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryCommitted {
    pub source_id: SourceId,
    pub revision: i64,
    pub delta: LibraryDelta,
}

#[derive(Clone, Debug)]
pub enum LibraryEvent {
    Delta(Box<LibraryDelta>),
    HomeSectionsChanged {
        source_id: SourceId,
    },
    HomeSectionPrefetched {
        source_id: SourceId,
        section: HomeSection,
    },
    HomeSectionProjectionFinished {
        source_id: SourceId,
        section: HomeSection,
    },
    FavoriteChanged {
        item_id: FavoriteItemId,
        favorite: bool,
    },
    FavoriteChangeFailed {
        item_id: FavoriteItemId,
        previous_favorite: bool,
        error: String,
    },
}
