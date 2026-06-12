use serde::{Deserialize, Serialize};

pub const TRACK_TABLE_LAYOUT_VERSION: u8 = 4;

pub(super) const DEFAULT_TRACK_TABLE_COLUMNS: [TrackTableColumn; 3] = [
    TrackTableColumn::Title,
    TrackTableColumn::Album,
    TrackTableColumn::Year,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum TrackTableColumn {
    TrackNumber,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}
impl TrackTableColumn {
    pub fn all() -> [Self; 7] {
        [
            Self::TrackNumber,
            Self::Title,
            Self::Album,
            Self::Year,
            Self::Favorite,
            Self::Artist,
            Self::Duration,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TrackNumber => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Duration => "Duration",
            Self::Favorite => "Favorite",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TrackSortKey {
    TrackNumber,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}
impl TrackSortKey {
    pub fn all() -> [Self; 7] {
        [
            Self::TrackNumber,
            Self::Title,
            Self::Artist,
            Self::Album,
            Self::Year,
            Self::Duration,
            Self::Favorite,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TrackNumber => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Duration => "Duration",
            Self::Favorite => "Favorite",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrackTableSettings {
    pub visible_columns: Vec<TrackTableColumn>,
    pub sort_key: TrackSortKey,
    pub descending: bool,
    #[serde(default)]
    pub layout_version: u8,
}
impl Default for TrackTableSettings {
    fn default() -> Self {
        Self {
            visible_columns: DEFAULT_TRACK_TABLE_COLUMNS.to_vec(),
            sort_key: TrackSortKey::Title,
            descending: false,
            layout_version: TRACK_TABLE_LAYOUT_VERSION,
        }
    }
}
impl TrackTableSettings {
    pub fn migrate_defaults(&mut self) {
        const LEGACY_DEFAULT_COLUMNS: [TrackTableColumn; 7] = [
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Artist,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
            TrackTableColumn::Duration,
            TrackTableColumn::Favorite,
        ];
        const COMPOSITE_TITLE_DEFAULT_COLUMNS: [TrackTableColumn; 3] = [
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
        ];
        const PREVIOUS_DEFAULT_COLUMNS: [TrackTableColumn; 5] = [
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
            TrackTableColumn::Favorite,
        ];
        const INDEX_DEFAULT_COLUMNS: [TrackTableColumn; 4] = [
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
        ];

        if self.layout_version < TRACK_TABLE_LAYOUT_VERSION
            && (self.visible_columns.as_slice() == LEGACY_DEFAULT_COLUMNS
                || self.visible_columns.as_slice() == COMPOSITE_TITLE_DEFAULT_COLUMNS
                || self.visible_columns.as_slice() == PREVIOUS_DEFAULT_COLUMNS
                || self.visible_columns.as_slice() == INDEX_DEFAULT_COLUMNS)
        {
            self.visible_columns = DEFAULT_TRACK_TABLE_COLUMNS.to_vec();
            self.sort_key = TrackSortKey::Title;
        }
        self.sanitize();
    }

    pub fn sanitize(&mut self) {
        let mut columns = Vec::new();
        for column in TrackTableColumn::all() {
            if self.visible_columns.contains(&column) {
                columns.push(column);
            }
        }
        if columns.is_empty() {
            columns = DEFAULT_TRACK_TABLE_COLUMNS.to_vec();
        }
        self.visible_columns = columns;
        self.layout_version = TRACK_TABLE_LAYOUT_VERSION;
    }
}
