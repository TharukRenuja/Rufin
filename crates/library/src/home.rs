use serde::{Deserialize, Serialize};

use crate::{Album, Track, msgid};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum HomeSectionKind {
    Explore,
    MostPlayed,
    NewlyAdded,
    RecentlyPlayed,
    RecentlyReleased,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum HomeBlockKind {
    Showcase,
    Explore,
    MostPlayed,
    NewlyAdded,
    RecentlyPlayed,
    RecentlyReleased,
    Genres,
}

pub const HOME_SECTION_ITEM_LIMIT: usize = 24;

impl HomeSectionKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Explore => msgid("Explore"),
            Self::MostPlayed => msgid("Most played"),
            Self::NewlyAdded => msgid("Newly added"),
            Self::RecentlyPlayed => msgid("Recently played"),
            Self::RecentlyReleased => msgid("Recently released"),
        }
    }
}

impl HomeBlockKind {
    pub fn all() -> [Self; 7] {
        [
            Self::Showcase,
            Self::Explore,
            Self::MostPlayed,
            Self::NewlyAdded,
            Self::RecentlyPlayed,
            Self::RecentlyReleased,
            Self::Genres,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Showcase => msgid("Showcase"),
            Self::Explore => HomeSectionKind::Explore.title(),
            Self::MostPlayed => HomeSectionKind::MostPlayed.title(),
            Self::NewlyAdded => HomeSectionKind::NewlyAdded.title(),
            Self::RecentlyPlayed => HomeSectionKind::RecentlyPlayed.title(),
            Self::RecentlyReleased => HomeSectionKind::RecentlyReleased.title(),
            Self::Genres => msgid("Featured genres"),
        }
    }

    pub fn section_kind(self) -> Option<HomeSectionKind> {
        match self {
            Self::Explore => Some(HomeSectionKind::Explore),
            Self::MostPlayed => Some(HomeSectionKind::MostPlayed),
            Self::NewlyAdded => Some(HomeSectionKind::NewlyAdded),
            Self::RecentlyPlayed => Some(HomeSectionKind::RecentlyPlayed),
            Self::RecentlyReleased => Some(HomeSectionKind::RecentlyReleased),
            Self::Showcase | Self::Genres => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HomeSection {
    pub kind: HomeSectionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub albums: Vec<Album>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
}
