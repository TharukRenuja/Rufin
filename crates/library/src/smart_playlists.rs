//! Smart playlist definitions and LoadedLibrary evaluation.
//!
//! Only user-owned definitions are durable. Membership is derived from the
//! selected source's accepted Track handles, so routes and Playback share one
//! order without querying SQLite.

use serde::{Deserialize, Serialize};

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    AcceptedLibraryChange, AlbumArtwork, MusicFolderId, SmartPlaylistId, Track, TrackActivity,
    TrackId, TrackSort,
    browse::{COLLECTION_ARTWORK_LIMIT, TrackList, album_artwork, compare_tracks, track_in_scope},
    loaded::{LoadedItems, LoadedState, TrackSlot},
    msgid,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistBuiltin {
    MostPlayed,
    NeverPlayed,
    MostSkipped,
}

impl SmartPlaylistBuiltin {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::MostPlayed => "most_played",
            Self::NeverPlayed => "never_played",
            Self::MostSkipped => "most_skipped",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::MostPlayed => msgid("Most Played"),
            Self::NeverPlayed => msgid("Never Played"),
            Self::MostSkipped => msgid("Most Skipped"),
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::MostPlayed, Self::NeverPlayed, Self::MostSkipped]
    }

    pub(crate) fn from_key(value: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|builtin| builtin.key() == value)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SmartPlaylistRule {
    pub field: SmartPlaylistRuleField,
    pub operator: SmartPlaylistRuleOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<SmartPlaylistRuleValue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistRuleField {
    Title,
    Artist,
    Album,
    Comment,
    Genre,
    Mood,
    Bpm,
    Rating,
    Year,
    Favorite,
    Played,
    PlayCount,
    SkipCount,
    LastPlayed,
    DateAdded,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistRuleOperator {
    Contains,
    NotContains,
    Equals,
    NotEquals,
    Above,
    Below,
    Between,
    Is,
    IsNot,
    Before,
    After,
    IsEmpty,
    IsNotEmpty,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistRuleValue {
    Text(String),
    Number(i64),
    NumberRange { min: i64, max: i64 },
    Bool(bool),
    Date(String),
    DateRange { start: String, end: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistSortField {
    Title,
    Artist,
    Album,
    Year,
    DateAdded,
    LastPlayed,
    PlayCount,
    SkipCount,
    Bpm,
    Rating,
    Duration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SmartPlaylistDefinition {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_all: Vec<SmartPlaylistRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_any: Vec<SmartPlaylistRule>,
    pub sort_field: SmartPlaylistSortField,
    pub descending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SmartPlaylistRecord {
    pub id: SmartPlaylistId,
    pub name: String,
    pub position: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<SmartPlaylistBuiltin>,
    pub definition: SmartPlaylistDefinition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SmartPlaylist {
    pub id: SmartPlaylistId,
    pub name: String,
    pub position: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<SmartPlaylistBuiltin>,
    pub definition: SmartPlaylistDefinition,
}

#[derive(Clone, Debug)]
pub struct SmartPlaylistSummary {
    pub smart_playlist: Arc<SmartPlaylist>,
    pub representative_albums: Arc<[AlbumArtwork]>,
    pub track_count: u32,
    pub duration_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct SmartPlaylistDetail {
    pub summary: SmartPlaylistSummary,
    pub tracks: TrackList,
}

#[derive(Debug)]
struct EvaluatedSmartPlaylist {
    smart_playlist: Arc<SmartPlaylist>,
    track_slots: Vec<TrackSlot>,
}

#[derive(Clone, Debug, Default)]
struct SmartPlaylistFacts {
    representative_albums: Vec<AlbumArtwork>,
    track_count: u32,
    duration_seconds: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmartPlaylistRuleValueKind {
    None,
    Text,
    Number,
    NumberRange,
    Date,
    DateRange,
    Bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmartPlaylistRuleOp {
    pub operator: SmartPlaylistRuleOperator,
    pub value_kind: SmartPlaylistRuleValueKind,
}

const RULE_FIELDS: [SmartPlaylistRuleField; 15] = [
    SmartPlaylistRuleField::Title,
    SmartPlaylistRuleField::Artist,
    SmartPlaylistRuleField::Album,
    SmartPlaylistRuleField::Comment,
    SmartPlaylistRuleField::Genre,
    SmartPlaylistRuleField::Mood,
    SmartPlaylistRuleField::Bpm,
    SmartPlaylistRuleField::Rating,
    SmartPlaylistRuleField::Year,
    SmartPlaylistRuleField::Favorite,
    SmartPlaylistRuleField::Played,
    SmartPlaylistRuleField::PlayCount,
    SmartPlaylistRuleField::SkipCount,
    SmartPlaylistRuleField::LastPlayed,
    SmartPlaylistRuleField::DateAdded,
];

const SORT_FIELDS: [SmartPlaylistSortField; 11] = [
    SmartPlaylistSortField::Title,
    SmartPlaylistSortField::Artist,
    SmartPlaylistSortField::Album,
    SmartPlaylistSortField::Year,
    SmartPlaylistSortField::DateAdded,
    SmartPlaylistSortField::LastPlayed,
    SmartPlaylistSortField::PlayCount,
    SmartPlaylistSortField::SkipCount,
    SmartPlaylistSortField::Bpm,
    SmartPlaylistSortField::Rating,
    SmartPlaylistSortField::Duration,
];

const MAX_DEFINITION_BYTES: usize = 256 * 1024;

const TEXT_OPS: [SmartPlaylistRuleOp; 6] = [
    op(
        SmartPlaylistRuleOperator::Contains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotContains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotEquals,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::IsEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
    op(
        SmartPlaylistRuleOperator::IsNotEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
];

const GENRE_OPS: [SmartPlaylistRuleOp; 4] = [
    op(
        SmartPlaylistRuleOperator::Contains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotContains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotEquals,
        SmartPlaylistRuleValueKind::Text,
    ),
];

const RATING_OPS: [SmartPlaylistRuleOp; 6] = [
    op(
        SmartPlaylistRuleOperator::Above,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Below,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Between,
        SmartPlaylistRuleValueKind::NumberRange,
    ),
    op(
        SmartPlaylistRuleOperator::IsEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
    op(
        SmartPlaylistRuleOperator::IsNotEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
];

const NUMBER_OPS: [SmartPlaylistRuleOp; 5] = [
    op(
        SmartPlaylistRuleOperator::Between,
        SmartPlaylistRuleValueKind::NumberRange,
    ),
    op(
        SmartPlaylistRuleOperator::Above,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Below,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::NotEquals,
        SmartPlaylistRuleValueKind::Number,
    ),
];

const BOOL_OPS: [SmartPlaylistRuleOp; 2] = [
    op(
        SmartPlaylistRuleOperator::Is,
        SmartPlaylistRuleValueKind::Bool,
    ),
    op(
        SmartPlaylistRuleOperator::IsNot,
        SmartPlaylistRuleValueKind::Bool,
    ),
];

const DATE_OPS: [SmartPlaylistRuleOp; 6] = [
    op(
        SmartPlaylistRuleOperator::Between,
        SmartPlaylistRuleValueKind::DateRange,
    ),
    op(
        SmartPlaylistRuleOperator::After,
        SmartPlaylistRuleValueKind::Date,
    ),
    op(
        SmartPlaylistRuleOperator::Before,
        SmartPlaylistRuleValueKind::Date,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Date,
    ),
    op(
        SmartPlaylistRuleOperator::IsEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
    op(
        SmartPlaylistRuleOperator::IsNotEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
];

const fn op(
    operator: SmartPlaylistRuleOperator,
    value_kind: SmartPlaylistRuleValueKind,
) -> SmartPlaylistRuleOp {
    SmartPlaylistRuleOp {
        operator,
        value_kind,
    }
}

pub fn rule_fields() -> &'static [SmartPlaylistRuleField] {
    &RULE_FIELDS
}

pub fn sort_fields() -> &'static [SmartPlaylistSortField] {
    &SORT_FIELDS
}

pub fn rule_ops(field: SmartPlaylistRuleField) -> &'static [SmartPlaylistRuleOp] {
    match field {
        SmartPlaylistRuleField::Title
        | SmartPlaylistRuleField::Artist
        | SmartPlaylistRuleField::Album
        | SmartPlaylistRuleField::Comment => &TEXT_OPS,
        SmartPlaylistRuleField::Genre | SmartPlaylistRuleField::Mood => &GENRE_OPS,
        SmartPlaylistRuleField::Rating => &RATING_OPS,
        SmartPlaylistRuleField::Year
        | SmartPlaylistRuleField::Bpm
        | SmartPlaylistRuleField::PlayCount
        | SmartPlaylistRuleField::SkipCount => &NUMBER_OPS,
        SmartPlaylistRuleField::Favorite | SmartPlaylistRuleField::Played => &BOOL_OPS,
        SmartPlaylistRuleField::LastPlayed | SmartPlaylistRuleField::DateAdded => &DATE_OPS,
    }
}

pub fn value_kind(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
) -> Option<SmartPlaylistRuleValueKind> {
    rule_ops(field)
        .iter()
        .find(|spec| spec.operator == operator)
        .map(|spec| spec.value_kind)
}

pub fn default_definition() -> SmartPlaylistDefinition {
    SmartPlaylistDefinition {
        match_all: Vec::new(),
        match_any: Vec::new(),
        sort_field: SmartPlaylistSortField::Title,
        descending: false,
        limit: None,
    }
}

pub fn builtin_definition(builtin: SmartPlaylistBuiltin) -> SmartPlaylistDefinition {
    match builtin {
        SmartPlaylistBuiltin::MostPlayed => SmartPlaylistDefinition {
            match_all: vec![played_rule(true)],
            match_any: Vec::new(),
            sort_field: SmartPlaylistSortField::PlayCount,
            descending: true,
            limit: None,
        },
        SmartPlaylistBuiltin::NeverPlayed => SmartPlaylistDefinition {
            match_all: vec![played_rule(false)],
            match_any: Vec::new(),
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        },
        SmartPlaylistBuiltin::MostSkipped => SmartPlaylistDefinition {
            match_all: vec![number_rule(
                SmartPlaylistRuleField::SkipCount,
                SmartPlaylistRuleOperator::Above,
                0,
            )],
            match_any: Vec::new(),
            sort_field: SmartPlaylistSortField::SkipCount,
            descending: true,
            limit: None,
        },
    }
}

pub fn default_rule(field: SmartPlaylistRuleField) -> SmartPlaylistRule {
    let operator = rule_ops(field)
        .first()
        .map(|spec| spec.operator)
        .unwrap_or(SmartPlaylistRuleOperator::Contains);
    SmartPlaylistRule {
        field,
        operator,
        value: default_value(field, operator),
    }
}

pub fn default_value(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
) -> Option<SmartPlaylistRuleValue> {
    match value_kind(field, operator) {
        Some(SmartPlaylistRuleValueKind::None) | None => None,
        Some(SmartPlaylistRuleValueKind::Text) => Some(SmartPlaylistRuleValue::Text(String::new())),
        Some(SmartPlaylistRuleValueKind::Number) => {
            Some(SmartPlaylistRuleValue::Number(number_bounds(field).2))
        }
        Some(SmartPlaylistRuleValueKind::NumberRange) => {
            let default = number_bounds(field).2;
            Some(SmartPlaylistRuleValue::NumberRange {
                min: default,
                max: default,
            })
        }
        Some(SmartPlaylistRuleValueKind::Date) => Some(SmartPlaylistRuleValue::Date(String::new())),
        Some(SmartPlaylistRuleValueKind::DateRange) => Some(SmartPlaylistRuleValue::DateRange {
            start: String::new(),
            end: String::new(),
        }),
        Some(SmartPlaylistRuleValueKind::Bool) => Some(SmartPlaylistRuleValue::Bool(true)),
    }
}

pub fn number_bounds(field: SmartPlaylistRuleField) -> (i64, i64, i64) {
    match field {
        SmartPlaylistRuleField::Rating => (0, 5, 4),
        SmartPlaylistRuleField::Year => (0, 3000, 2000),
        SmartPlaylistRuleField::Bpm => (0, 400, 120),
        SmartPlaylistRuleField::PlayCount | SmartPlaylistRuleField::SkipCount => (0, 999_999, 1),
        _ => (0, 999_999, 0),
    }
}

pub(crate) fn normalize_definition(definition: &mut SmartPlaylistDefinition) {
    normalize_rules(&mut definition.match_all);
    normalize_rules(&mut definition.match_any);
}

pub fn text_value(rule: &SmartPlaylistRule) -> Option<String> {
    let SmartPlaylistRuleValue::Text(value) = rule.value.as_ref()? else {
        return None;
    };
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

pub fn number_value(rule: &SmartPlaylistRule) -> Option<i64> {
    let SmartPlaylistRuleValue::Number(value) = rule.value.as_ref()? else {
        return None;
    };
    Some(*value)
}

pub fn number_range_value(rule: &SmartPlaylistRule) -> Option<(i64, i64)> {
    let SmartPlaylistRuleValue::NumberRange { min, max } = rule.value.as_ref()? else {
        return None;
    };
    Some((*min, *max))
}

pub fn bool_value(rule: &SmartPlaylistRule) -> Option<bool> {
    let SmartPlaylistRuleValue::Bool(value) = rule.value.as_ref()? else {
        return None;
    };
    Some(*value)
}

pub fn date_value(rule: &SmartPlaylistRule) -> Option<String> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::Date(value) | SmartPlaylistRuleValue::Text(value) => {
            Some(value.trim().to_string())
        }
        SmartPlaylistRuleValue::Number(_)
        | SmartPlaylistRuleValue::NumberRange { .. }
        | SmartPlaylistRuleValue::Bool(_)
        | SmartPlaylistRuleValue::DateRange { .. } => None,
    }
    .filter(|value| !value.is_empty())
}

pub fn date_range_value(rule: &SmartPlaylistRule) -> Option<(String, String)> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::DateRange { start, end } => {
            let start = start.trim().to_string();
            let end = end.trim().to_string();
            if start.is_empty() || end.is_empty() {
                None
            } else if start <= end {
                Some((start, end))
            } else {
                Some((end, start))
            }
        }
        SmartPlaylistRuleValue::Text(_)
        | SmartPlaylistRuleValue::Number(_)
        | SmartPlaylistRuleValue::NumberRange { .. }
        | SmartPlaylistRuleValue::Bool(_)
        | SmartPlaylistRuleValue::Date(_) => None,
    }
}

fn normalize_rules(rules: &mut Vec<SmartPlaylistRule>) {
    rules.retain_mut(|rule| normalize_rule(rule).is_some());
}

fn normalize_rule(rule: &mut SmartPlaylistRule) -> Option<()> {
    match value_kind(rule.field, rule.operator)? {
        SmartPlaylistRuleValueKind::None => {
            rule.value = None;
            Some(())
        }
        SmartPlaylistRuleValueKind::Text => match rule.value.as_mut()? {
            SmartPlaylistRuleValue::Text(value) if !value.trim().is_empty() => {
                *value = value.trim().to_string();
                Some(())
            }
            _ => None,
        },
        SmartPlaylistRuleValueKind::Number => {
            matches!(rule.value, Some(SmartPlaylistRuleValue::Number(_))).then_some(())
        }
        SmartPlaylistRuleValueKind::NumberRange => {
            let Some(SmartPlaylistRuleValue::NumberRange { min, max }) = rule.value.as_mut() else {
                return None;
            };
            if *min > *max {
                std::mem::swap(min, max);
            }
            Some(())
        }
        SmartPlaylistRuleValueKind::Date => match rule.value.as_mut()? {
            SmartPlaylistRuleValue::Date(value) if !value.trim().is_empty() => {
                *value = value.trim().to_string();
                Some(())
            }
            _ => None,
        },
        SmartPlaylistRuleValueKind::DateRange => {
            let Some(SmartPlaylistRuleValue::DateRange { start, end }) = rule.value.as_mut() else {
                return None;
            };
            *start = start.trim().to_string();
            *end = end.trim().to_string();
            if start.is_empty() || end.is_empty() {
                return None;
            }
            if *start > *end {
                std::mem::swap(start, end);
            }
            Some(())
        }
        SmartPlaylistRuleValueKind::Bool => {
            matches!(rule.value, Some(SmartPlaylistRuleValue::Bool(_))).then_some(())
        }
    }
}

pub(crate) fn validated_smart_playlist_json(
    definition: &SmartPlaylistDefinition,
) -> Result<String, String> {
    for rule in definition
        .match_all
        .iter()
        .chain(definition.match_any.iter())
    {
        let mut normalized = rule.clone();
        if normalize_rule(&mut normalized).is_none() || normalized != *rule {
            return Err("smart playlist contains an invalid rule".to_string());
        }
    }
    let encoded = serde_json::to_string(definition).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_DEFINITION_BYTES {
        return Err(format!(
            "smart playlist definition exceeds {MAX_DEFINITION_BYTES} bytes"
        ));
    }
    Ok(encoded)
}

fn played_rule(played: bool) -> SmartPlaylistRule {
    SmartPlaylistRule {
        field: SmartPlaylistRuleField::Played,
        operator: SmartPlaylistRuleOperator::Is,
        value: Some(SmartPlaylistRuleValue::Bool(played)),
    }
}

fn number_rule(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
    value: i64,
) -> SmartPlaylistRule {
    SmartPlaylistRule {
        field,
        operator,
        value: Some(SmartPlaylistRuleValue::Number(value)),
    }
}

fn evaluate_playlist(
    smart_playlist: Arc<SmartPlaylist>,
    state: &LoadedState,
    music_folder_id: Option<&MusicFolderId>,
) -> EvaluatedSmartPlaylist {
    let matches = |slot: &TrackSlot| {
        state.tracks.get_slot(*slot).is_some_and(|track| {
            track_in_scope(track, music_folder_id)
                && matches_definition(
                    track,
                    state.activity.get(&track.id),
                    &smart_playlist.definition,
                )
        })
    };
    let mut track_slots = state
        .tracks
        .live_slots()
        .filter(|slot| matches(slot))
        .collect::<Vec<_>>();
    track_slots.sort_by(|left, right| {
        compare_smart_tracks(
            state
                .tracks
                .get_slot(*left)
                .expect("smart-playlist Track slot must resolve"),
            state
                .tracks
                .get_slot(*left)
                .and_then(|track| state.activity.get(&track.id)),
            state
                .tracks
                .get_slot(*right)
                .expect("smart-playlist Track slot must resolve"),
            state
                .tracks
                .get_slot(*right)
                .and_then(|track| state.activity.get(&track.id)),
            smart_playlist.definition.sort_field,
            smart_playlist.definition.descending,
        )
    });
    if let Some(limit) = smart_playlist.definition.limit {
        track_slots.truncate(limit);
    }
    EvaluatedSmartPlaylist {
        smart_playlist,
        track_slots,
    }
}

impl EvaluatedSmartPlaylist {
    fn browse(
        &self,
        state: &LoadedState,
        music_folder_id: Option<&MusicFolderId>,
    ) -> SmartPlaylistSummary {
        smart_playlist_summary(
            Arc::clone(&self.smart_playlist),
            smart_playlist_facts(state, &self.track_slots, music_folder_id),
        )
    }

    fn detail(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
        state: &LoadedState,
        music_folder_id: Option<&MusicFolderId>,
    ) -> Arc<SmartPlaylistDetail> {
        let tracks = TrackList::new(Arc::clone(loaded), self.track_slots.clone().into(), None);
        Arc::new(SmartPlaylistDetail {
            summary: self.browse(state, music_folder_id),
            tracks,
        })
    }
}

fn smart_playlist_facts(
    state: &LoadedState,
    track_slots: &[TrackSlot],
    music_folder_id: Option<&MusicFolderId>,
) -> SmartPlaylistFacts {
    let mut facts = SmartPlaylistFacts::default();
    for track_slot in track_slots {
        let Some(track) = state.tracks.get_slot(*track_slot) else {
            continue;
        };
        add_smart_playlist_track(state, &mut facts, track, music_folder_id);
    }
    facts
}

fn add_smart_playlist_track(
    state: &LoadedState,
    facts: &mut SmartPlaylistFacts,
    track: &Track,
    music_folder_id: Option<&MusicFolderId>,
) {
    facts.track_count = facts.track_count.saturating_add(1);
    facts.duration_seconds = facts
        .duration_seconds
        .saturating_add(track.duration_seconds);
    if facts.representative_albums.len() >= COLLECTION_ARTWORK_LIMIT {
        return;
    }
    let Some(album_id) = &track.album_id else {
        return;
    };
    let Some(album) = state.albums.get(album_id) else {
        return;
    };
    if facts
        .representative_albums
        .iter()
        .any(|candidate| candidate.album.id == *album_id)
    {
        return;
    }
    if let Some(artwork) = album_artwork(state, album, music_folder_id) {
        facts.representative_albums.push(artwork);
    }
}

fn smart_playlist_summary(
    smart_playlist: Arc<SmartPlaylist>,
    facts: SmartPlaylistFacts,
) -> SmartPlaylistSummary {
    SmartPlaylistSummary {
        smart_playlist,
        representative_albums: facts.representative_albums.into(),
        track_count: facts.track_count,
        duration_seconds: facts.duration_seconds,
    }
}

fn summarize_playlist(
    smart_playlist: Arc<SmartPlaylist>,
    state: &LoadedState,
    music_folder_id: Option<&MusicFolderId>,
) -> SmartPlaylistSummary {
    if smart_playlist.definition.limit.is_some() {
        return evaluate_playlist(Arc::clone(&smart_playlist), state, music_folder_id)
            .browse(state, music_folder_id);
    }

    let mut facts = SmartPlaylistFacts::default();
    for track_slot in state.tracks.live_slots() {
        let Some(track) = state.tracks.get_slot(track_slot) else {
            continue;
        };
        if track_in_scope(track, music_folder_id)
            && matches_definition(
                track,
                state.activity.get(&track.id),
                &smart_playlist.definition,
            )
        {
            add_smart_playlist_track(state, &mut facts, track, music_folder_id);
        }
    }
    smart_playlist_summary(smart_playlist, facts)
}

fn definition_uses_activity(definition: &SmartPlaylistDefinition) -> bool {
    sort_uses_activity(definition.sort_field)
        || definition_rules(definition).any(|rule| {
            matches!(
                rule.field,
                SmartPlaylistRuleField::Played
                    | SmartPlaylistRuleField::PlayCount
                    | SmartPlaylistRuleField::SkipCount
                    | SmartPlaylistRuleField::LastPlayed
            )
        })
}

fn sort_uses_activity(field: SmartPlaylistSortField) -> bool {
    matches!(
        field,
        SmartPlaylistSortField::LastPlayed
            | SmartPlaylistSortField::PlayCount
            | SmartPlaylistSortField::SkipCount
    )
}

fn definition_uses_favorite(definition: &SmartPlaylistDefinition) -> bool {
    definition_rules(definition).any(|rule| rule.field == SmartPlaylistRuleField::Favorite)
}

fn definition_rules(
    definition: &SmartPlaylistDefinition,
) -> impl Iterator<Item = &SmartPlaylistRule> {
    definition
        .match_all
        .iter()
        .chain(definition.match_any.iter())
}

pub(crate) fn changed_by_activity(
    playlists: &HashMap<SmartPlaylistId, Arc<SmartPlaylist>>,
    track: &Track,
    previous: Option<&TrackActivity>,
    current: &TrackActivity,
) -> HashSet<SmartPlaylistId> {
    playlists
        .iter()
        .filter_map(|(id, playlist)| {
            if !definition_uses_activity(&playlist.definition) {
                return None;
            }
            let old_matches = matches_definition(track, previous, &playlist.definition);
            let new_matches = matches_definition(track, Some(current), &playlist.definition);
            (old_matches != new_matches
                || (new_matches
                    && activity_sort_changed(playlist.definition.sort_field, previous, current)))
            .then(|| id.clone())
        })
        .collect()
}

fn activity_sort_changed(
    field: SmartPlaylistSortField,
    previous: Option<&TrackActivity>,
    current: &TrackActivity,
) -> bool {
    match field {
        SmartPlaylistSortField::LastPlayed => {
            previous.and_then(|value| value.last_played.as_ref()) != current.last_played.as_ref()
        }
        SmartPlaylistSortField::PlayCount => {
            previous.map_or(0, |value| value.play_count) != current.play_count
        }
        SmartPlaylistSortField::SkipCount => {
            previous.map_or(0, |value| value.skip_count) != current.skip_count
        }
        SmartPlaylistSortField::Title
        | SmartPlaylistSortField::Artist
        | SmartPlaylistSortField::Album
        | SmartPlaylistSortField::Year
        | SmartPlaylistSortField::DateAdded
        | SmartPlaylistSortField::Bpm
        | SmartPlaylistSortField::Rating
        | SmartPlaylistSortField::Duration => false,
    }
}

pub(crate) fn changed_by_favorite(
    playlists: &HashMap<SmartPlaylistId, Arc<SmartPlaylist>>,
    old_track: &Track,
    track: &Track,
    activity: &HashMap<TrackId, TrackActivity>,
) -> HashSet<SmartPlaylistId> {
    playlists
        .iter()
        .filter_map(|(id, playlist)| {
            if !definition_uses_favorite(&playlist.definition) {
                return None;
            }
            let old_matches =
                matches_definition(old_track, activity.get(&old_track.id), &playlist.definition);
            let new_matches =
                matches_definition(track, activity.get(&track.id), &playlist.definition);
            (old_matches != new_matches).then(|| id.clone())
        })
        .collect()
}

pub(crate) fn changed_by_tracks(
    playlists: &HashMap<SmartPlaylistId, Arc<SmartPlaylist>>,
    old_tracks: &HashMap<TrackId, Track>,
    changed_track_ids: &HashSet<TrackId>,
    tracks: &LoadedItems<TrackId, Track>,
    activity: &HashMap<TrackId, TrackActivity>,
) -> HashSet<SmartPlaylistId> {
    playlists
        .iter()
        .filter_map(|(id, playlist)| {
            changed_track_ids
                .iter()
                .any(
                    |track_id| match (old_tracks.get(track_id), tracks.get(track_id)) {
                        (Some(old_track), Some(track)) => {
                            let old_matches = matches_definition(
                                old_track,
                                activity.get(track_id),
                                &playlist.definition,
                            );
                            let new_matches = matches_definition(
                                track,
                                activity.get(track_id),
                                &playlist.definition,
                            );
                            old_matches != new_matches
                                || (new_matches
                                    && (compare_smart_tracks(
                                        old_track,
                                        activity.get(track_id),
                                        track,
                                        activity.get(track_id),
                                        playlist.definition.sort_field,
                                        false,
                                    ) != Ordering::Equal
                                        || old_track.duration_seconds != track.duration_seconds
                                        || old_track.album_id != track.album_id
                                        || old_track.relations.music_folders
                                            != track.relations.music_folders))
                        }
                        (Some(old_track), None) => matches_definition(
                            old_track,
                            activity.get(track_id),
                            &playlist.definition,
                        ),
                        (None, Some(track)) => {
                            matches_definition(track, activity.get(track_id), &playlist.definition)
                        }
                        (None, None) => false,
                    },
                )
                .then(|| id.clone())
        })
        .collect()
}

fn matches_definition(
    track: &Track,
    activity: Option<&TrackActivity>,
    definition: &SmartPlaylistDefinition,
) -> bool {
    definition
        .match_all
        .iter()
        .all(|rule| matches_rule(track, activity, rule))
        && (definition.match_any.is_empty()
            || definition
                .match_any
                .iter()
                .any(|rule| matches_rule(track, activity, rule)))
}

fn matches_rule(track: &Track, activity: Option<&TrackActivity>, rule: &SmartPlaylistRule) -> bool {
    match rule.field {
        SmartPlaylistRuleField::Title => match_texts([track.title.as_str()], rule),
        SmartPlaylistRuleField::Artist => match_texts([track.artist.as_str()], rule),
        SmartPlaylistRuleField::Album => match_texts([track.album.as_str()], rule),
        SmartPlaylistRuleField::Comment => match_optional_text(track.comment.as_deref(), rule),
        SmartPlaylistRuleField::Genre => match_texts(
            track
                .relations
                .genres
                .iter()
                .map(|genre| genre.name.as_str()),
            rule,
        ),
        SmartPlaylistRuleField::Mood => match_texts(
            track.relations.moods.iter().map(|mood| mood.name.as_str()),
            rule,
        ),
        SmartPlaylistRuleField::Bpm => match_optional_number(track.bpm.map(i64::from), rule),
        SmartPlaylistRuleField::Rating => {
            match_optional_number(track.user_rating.map(i64::from), rule)
        }
        SmartPlaylistRuleField::Year => match_number(i64::from(track.year), rule),
        SmartPlaylistRuleField::Favorite => match_bool(track.favorite, rule),
        SmartPlaylistRuleField::Played => match_bool(
            activity
                .is_some_and(|activity| activity.play_count > 0 || activity.last_played.is_some()),
            rule,
        ),
        SmartPlaylistRuleField::PlayCount => match_number(
            i64::from(activity.map_or(0, |activity| activity.play_count)),
            rule,
        ),
        SmartPlaylistRuleField::SkipCount => match_number(
            i64::from(activity.map_or(0, |activity| activity.skip_count)),
            rule,
        ),
        SmartPlaylistRuleField::LastPlayed => match_optional_date(
            activity.and_then(|activity| activity.last_played.as_deref()),
            rule,
        ),
        SmartPlaylistRuleField::DateAdded => match_optional_date(track.date_added.as_deref(), rule),
    }
}

fn match_texts<'a>(values: impl IntoIterator<Item = &'a str>, rule: &SmartPlaylistRule) -> bool {
    let expected = match rule.operator {
        SmartPlaylistRuleOperator::Contains
        | SmartPlaylistRuleOperator::NotContains
        | SmartPlaylistRuleOperator::Equals
        | SmartPlaylistRuleOperator::NotEquals => text_value_ref(rule),
        _ => None,
    };
    let mut nonempty = false;
    let mut matched = false;
    for value in values {
        nonempty |= !value.trim().is_empty();
        let Some(expected) = expected else {
            continue;
        };
        matched |= match rule.operator {
            SmartPlaylistRuleOperator::Contains | SmartPlaylistRuleOperator::NotContains => {
                contains_ascii_case_insensitive(value, expected)
            }
            SmartPlaylistRuleOperator::Equals | SmartPlaylistRuleOperator::NotEquals => {
                value.eq_ignore_ascii_case(expected)
            }
            _ => false,
        };
    }
    match rule.operator {
        SmartPlaylistRuleOperator::IsEmpty => !nonempty,
        SmartPlaylistRuleOperator::IsNotEmpty => nonempty,
        SmartPlaylistRuleOperator::Contains | SmartPlaylistRuleOperator::Equals => {
            expected.is_some() && matched
        }
        SmartPlaylistRuleOperator::NotContains | SmartPlaylistRuleOperator::NotEquals => {
            expected.is_some() && !matched
        }
        _ => false,
    }
}

fn text_value_ref(rule: &SmartPlaylistRule) -> Option<&str> {
    let Some(SmartPlaylistRuleValue::Text(value)) = rule.value.as_ref() else {
        return None;
    };
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn contains_ascii_case_insensitive(value: &str, expected: &str) -> bool {
    let expected = expected.as_bytes();
    !expected.is_empty()
        && value
            .as_bytes()
            .windows(expected.len())
            .any(|candidate| candidate.eq_ignore_ascii_case(expected))
}

fn match_optional_text(value: Option<&str>, rule: &SmartPlaylistRule) -> bool {
    match value {
        Some(value) => match_texts([value], rule),
        None => matches!(rule.operator, SmartPlaylistRuleOperator::IsEmpty),
    }
}

fn match_optional_number(value: Option<i64>, rule: &SmartPlaylistRule) -> bool {
    match value {
        Some(value) => match_number(value, rule),
        None => matches!(rule.operator, SmartPlaylistRuleOperator::IsEmpty),
    }
}

fn match_number(value: i64, rule: &SmartPlaylistRule) -> bool {
    match rule.operator {
        SmartPlaylistRuleOperator::Above => number_value(rule).is_some_and(|other| value > other),
        SmartPlaylistRuleOperator::Below => number_value(rule).is_some_and(|other| value < other),
        SmartPlaylistRuleOperator::Equals => number_value(rule) == Some(value),
        SmartPlaylistRuleOperator::NotEquals => number_value(rule) != Some(value),
        SmartPlaylistRuleOperator::Between => number_range_value(rule)
            .is_some_and(|(minimum, maximum)| value >= minimum && value <= maximum),
        SmartPlaylistRuleOperator::IsEmpty => false,
        SmartPlaylistRuleOperator::IsNotEmpty => true,
        _ => false,
    }
}

fn match_bool(value: bool, rule: &SmartPlaylistRule) -> bool {
    let Some(expected) = bool_value(rule) else {
        return false;
    };
    match rule.operator {
        SmartPlaylistRuleOperator::Is => value == expected,
        SmartPlaylistRuleOperator::IsNot => value != expected,
        _ => false,
    }
}

fn match_optional_date(value: Option<&str>, rule: &SmartPlaylistRule) -> bool {
    let Some(value) = value else {
        return matches!(rule.operator, SmartPlaylistRuleOperator::IsEmpty);
    };
    match rule.operator {
        SmartPlaylistRuleOperator::Before => {
            date_value_ref(rule).is_some_and(|other| value < other)
        }
        SmartPlaylistRuleOperator::After => date_value_ref(rule).is_some_and(|other| value > other),
        SmartPlaylistRuleOperator::Equals => {
            date_value_ref(rule).is_some_and(|other| value == other)
        }
        SmartPlaylistRuleOperator::Between => {
            date_range_value_ref(rule).is_some_and(|(start, end)| value >= start && value <= end)
        }
        SmartPlaylistRuleOperator::IsEmpty => false,
        SmartPlaylistRuleOperator::IsNotEmpty => true,
        _ => false,
    }
}

fn date_value_ref(rule: &SmartPlaylistRule) -> Option<&str> {
    let value = match rule.value.as_ref()? {
        SmartPlaylistRuleValue::Date(value) | SmartPlaylistRuleValue::Text(value) => value.trim(),
        SmartPlaylistRuleValue::Number(_)
        | SmartPlaylistRuleValue::NumberRange { .. }
        | SmartPlaylistRuleValue::Bool(_)
        | SmartPlaylistRuleValue::DateRange { .. } => return None,
    };
    (!value.is_empty()).then_some(value)
}

fn date_range_value_ref(rule: &SmartPlaylistRule) -> Option<(&str, &str)> {
    let Some(SmartPlaylistRuleValue::DateRange { start, end }) = rule.value.as_ref() else {
        return None;
    };
    let start = start.trim();
    let end = end.trim();
    if start.is_empty() || end.is_empty() {
        None
    } else if start <= end {
        Some((start, end))
    } else {
        Some((end, start))
    }
}

fn compare_smart_tracks(
    left: &Track,
    left_activity: Option<&TrackActivity>,
    right: &Track,
    right_activity: Option<&TrackActivity>,
    field: SmartPlaylistSortField,
    descending: bool,
) -> Ordering {
    let source_sort = match field {
        SmartPlaylistSortField::Title => Some(TrackSort::Title),
        SmartPlaylistSortField::Artist => Some(TrackSort::Artist),
        SmartPlaylistSortField::Album => Some(TrackSort::Album),
        SmartPlaylistSortField::Year => Some(TrackSort::Year),
        SmartPlaylistSortField::DateAdded => Some(TrackSort::DateAdded),
        SmartPlaylistSortField::Bpm => Some(TrackSort::Bpm),
        SmartPlaylistSortField::Rating => Some(TrackSort::UserRating),
        SmartPlaylistSortField::Duration => Some(TrackSort::Duration),
        SmartPlaylistSortField::LastPlayed
        | SmartPlaylistSortField::PlayCount
        | SmartPlaylistSortField::SkipCount => None,
    };
    if let Some(source_sort) = source_sort {
        return compare_tracks(left, right, source_sort, descending);
    }

    let left_last_played = left_activity.and_then(|activity| activity.last_played.as_ref());
    let right_last_played = right_activity.and_then(|activity| activity.last_played.as_ref());
    if field == SmartPlaylistSortField::LastPlayed {
        let missing = left_last_played.is_none().cmp(&right_last_played.is_none());
        if missing != Ordering::Equal {
            return missing;
        }
    }
    let primary = match field {
        SmartPlaylistSortField::LastPlayed => left_last_played.cmp(&right_last_played),
        SmartPlaylistSortField::PlayCount => left_activity
            .map_or(0, |activity| activity.play_count)
            .cmp(&right_activity.map_or(0, |activity| activity.play_count)),
        SmartPlaylistSortField::SkipCount => left_activity
            .map_or(0, |activity| activity.skip_count)
            .cmp(&right_activity.map_or(0, |activity| activity.skip_count)),
        SmartPlaylistSortField::Title
        | SmartPlaylistSortField::Artist
        | SmartPlaylistSortField::Album
        | SmartPlaylistSortField::Year
        | SmartPlaylistSortField::DateAdded
        | SmartPlaylistSortField::Bpm
        | SmartPlaylistSortField::Rating
        | SmartPlaylistSortField::Duration => unreachable!("source sort handled above"),
    }
    .then_with(|| text_cmp(&left.album, &right.album))
    .then(left.disc_number.cmp(&right.disc_number))
    .then(left.track_number.cmp(&right.track_number))
    .then_with(|| text_cmp(&left.title, &right.title))
    .then(left.id.cmp(&right.id));
    descending.then(|| primary.reverse()).unwrap_or(primary)
}

fn text_cmp(left: &str, right: &str) -> Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

impl crate::LoadedLibrary {
    pub fn missing_builtin_smart_playlists(
        &self,
    ) -> crate::LoadedLibraryResult<Vec<SmartPlaylistBuiltin>> {
        let state = self.read_state()?;
        Ok(SmartPlaylistBuiltin::all()
            .into_iter()
            .filter(|builtin| {
                !state
                    .smart_playlists
                    .values()
                    .any(|playlist| playlist.builtin == Some(*builtin))
            })
            .collect())
    }

    pub fn smart_playlist(
        &self,
        id: &SmartPlaylistId,
    ) -> crate::LoadedLibraryResult<Option<Arc<SmartPlaylist>>> {
        Ok(self.read_state()?.smart_playlists.get(id).cloned())
    }

    pub fn smart_playlist_rule_value_suggestions(
        &self,
    ) -> crate::LoadedLibraryResult<(Vec<String>, Vec<String>)> {
        let state = self.read_state()?;
        let mut genres = HashSet::new();
        let mut moods = HashSet::new();
        for track in state.tracks.values() {
            genres.extend(
                track
                    .relations
                    .genres
                    .iter()
                    .map(|genre| genre.name.trim())
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned),
            );
            moods.extend(
                track
                    .relations
                    .moods
                    .iter()
                    .map(|mood| mood.name.trim())
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
        let mut genres = genres.into_iter().collect::<Vec<_>>();
        let mut moods = moods.into_iter().collect::<Vec<_>>();
        genres.sort_by(|left, right| text_cmp(left, right));
        moods.sort_by(|left, right| text_cmp(left, right));
        Ok((genres, moods))
    }

    pub fn smart_playlists(
        &self,
        music_folder_id: Option<&MusicFolderId>,
    ) -> crate::LoadedLibraryResult<Arc<[SmartPlaylistSummary]>> {
        let state = self.read_state()?;
        let mut playlists = state
            .smart_playlists
            .values()
            .map(|playlist| summarize_playlist(Arc::clone(playlist), &state, music_folder_id))
            .collect::<Vec<_>>();
        playlists.sort_by(|left, right| {
            left.smart_playlist
                .position
                .cmp(&right.smart_playlist.position)
                .then(left.smart_playlist.id.cmp(&right.smart_playlist.id))
        });
        Ok(playlists.into())
    }

    pub fn smart_playlist_summary(
        &self,
        id: &SmartPlaylistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> crate::LoadedLibraryResult<Option<SmartPlaylistSummary>> {
        let state = self.read_state()?;
        Ok(state
            .smart_playlists
            .get(id)
            .map(|playlist| summarize_playlist(Arc::clone(playlist), &state, music_folder_id)))
    }

    pub fn smart_playlist_detail(
        self: &Arc<Self>,
        id: &SmartPlaylistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> crate::LoadedLibraryResult<Option<Arc<SmartPlaylistDetail>>> {
        let state = self.read_state()?;
        Ok(state.smart_playlists.get(id).map(|playlist| {
            evaluate_playlist(Arc::clone(playlist), &state, music_folder_id).detail(
                self,
                &state,
                music_folder_id,
            )
        }))
    }

    pub fn smart_playlist_tracks(
        self: &Arc<Self>,
        id: &SmartPlaylistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> crate::LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let slots = state
            .smart_playlists
            .get(id)
            .map_or_else(Vec::new, |playlist| {
                evaluate_playlist(Arc::clone(playlist), &state, music_folder_id).track_slots
            });
        Ok(TrackList::new(Arc::clone(self), slots.into(), None))
    }

    pub(crate) fn replace_smart_playlist(
        &self,
        record: SmartPlaylistRecord,
    ) -> crate::LoadedLibraryResult<()> {
        let mut state = self.write_state()?;
        let id = record.id.clone();
        state
            .smart_playlists
            .insert(id, playlist_from_record(record));
        Ok(())
    }

    fn smart_playlist_record(
        &self,
        id: &SmartPlaylistId,
    ) -> crate::LoadedLibraryResult<Option<SmartPlaylistRecord>> {
        let state = self.read_state()?;
        Ok(state
            .smart_playlists
            .get(id)
            .map(|playlist| record_from_playlist(playlist)))
    }

    fn smart_playlist_records(&self) -> crate::LoadedLibraryResult<Vec<SmartPlaylistRecord>> {
        let state = self.read_state()?;
        let mut records = state
            .smart_playlists
            .values()
            .map(|playlist| record_from_playlist(playlist))
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.position
                .cmp(&right.position)
                .then(left.id.cmp(&right.id))
        });
        Ok(records)
    }

    fn replace_smart_playlist_order(
        &self,
        ordered_ids: &[SmartPlaylistId],
    ) -> crate::LoadedLibraryResult<()> {
        let mut state = self.write_state()?;
        for (position, id) in ordered_ids.iter().enumerate() {
            let playlist = state.smart_playlists.get_mut(id).ok_or_else(|| {
                crate::LoadedLibraryError::MissingItem {
                    kind: "smart playlist",
                    id: id.as_str().to_string(),
                }
            })?;
            Arc::make_mut(playlist).position =
                u32::try_from(position).expect("loaded smart playlist order fits u32 positions");
        }
        Ok(())
    }

    pub(crate) fn remove_smart_playlist(
        &self,
        id: &SmartPlaylistId,
    ) -> crate::LoadedLibraryResult<bool> {
        let mut state = self.write_state()?;
        Ok(state.smart_playlists.remove(id).is_some())
    }
}

impl crate::Library {
    pub fn initialize_smart_playlists(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
    ) -> crate::LibraryResult<Option<AcceptedLibraryChange>> {
        let records = loaded.smart_playlist_records()?;
        let mut next_position = records
            .iter()
            .map(|record| record.position)
            .max()
            .map_or(0, |position| position.saturating_add(1));
        let mut accepted = Vec::new();
        for builtin in SmartPlaylistBuiltin::all().into_iter().filter(|builtin| {
            !records
                .iter()
                .any(|record| record.builtin == Some(*builtin))
        }) {
            let record = builtin_record(builtin, next_position);
            self.store
                .put_smart_playlist(loaded.source_id().clone(), record.clone())?;
            loaded.replace_smart_playlist(record.clone())?;
            accepted.push(record.id);
            next_position = next_position.saturating_add(1);
        }
        Ok((!accepted.is_empty()).then_some(AcceptedLibraryChange {
            smart_playlists: accepted,
            ..AcceptedLibraryChange::default()
        }))
    }

    pub fn create_smart_playlist(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
        name: String,
        definition: SmartPlaylistDefinition,
    ) -> crate::LibraryResult<Option<AcceptedLibraryChange>> {
        let position = loaded
            .smart_playlist_records()?
            .into_iter()
            .map(|record| record.position)
            .max()
            .map_or(0, |position| position.saturating_add(1));
        let record = SmartPlaylistRecord {
            id: SmartPlaylistId::new(format!("custom:{}", random_hex()?)),
            name: name.trim().to_string(),
            position,
            builtin: None,
            definition,
        };
        self.put_smart_playlist(loaded, record)
    }

    pub fn update_smart_playlist(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
        id: SmartPlaylistId,
        name: String,
        definition: SmartPlaylistDefinition,
    ) -> crate::LibraryResult<Option<AcceptedLibraryChange>> {
        let mut record = loaded
            .smart_playlist_record(&id)?
            .ok_or_else(|| missing_smart_playlist(&id))?;
        record.name = name.trim().to_string();
        record.definition = definition;
        self.put_smart_playlist(loaded, record)
    }

    pub fn delete_smart_playlist(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
        id: &SmartPlaylistId,
    ) -> crate::LibraryResult<Option<AcceptedLibraryChange>> {
        self.store
            .remove_smart_playlist(loaded.source_id().clone(), id.clone())?;
        if !loaded.remove_smart_playlist(id)? {
            return Ok(None);
        }
        Ok(Some(AcceptedLibraryChange {
            smart_playlists: vec![id.clone()],
            ..AcceptedLibraryChange::default()
        }))
    }

    pub fn restore_builtin_smart_playlist(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
        builtin: SmartPlaylistBuiltin,
    ) -> crate::LibraryResult<Option<AcceptedLibraryChange>> {
        if loaded
            .smart_playlist_records()?
            .into_iter()
            .any(|record| record.builtin == Some(builtin))
        {
            return Ok(None);
        }
        let position = loaded
            .smart_playlist_records()?
            .into_iter()
            .map(|record| record.position)
            .max()
            .map_or(0, |position| position.saturating_add(1));
        self.put_smart_playlist(loaded, builtin_record(builtin, position))
    }

    pub fn move_smart_playlist_relative(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
        dragged_id: SmartPlaylistId,
        target_id: SmartPlaylistId,
        after: bool,
    ) -> crate::LibraryResult<Option<AcceptedLibraryChange>> {
        let records = loaded.smart_playlist_records()?;
        let ids = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        let Some(ordered_ids) = reordered_ids(&ids, &dragged_id, &target_id, after) else {
            return Ok(None);
        };
        self.store
            .replace_smart_playlist_order(loaded.source_id().clone(), ordered_ids.clone())?;
        loaded.replace_smart_playlist_order(&ordered_ids)?;
        Ok(Some(AcceptedLibraryChange {
            smart_playlists: ordered_ids,
            ..AcceptedLibraryChange::default()
        }))
    }

    fn put_smart_playlist(
        &self,
        loaded: &Arc<crate::LoadedLibrary>,
        mut record: SmartPlaylistRecord,
    ) -> crate::LibraryResult<Option<AcceptedLibraryChange>> {
        normalize_definition(&mut record.definition);
        if loaded.smart_playlist_record(&record.id)?.as_ref() == Some(&record) {
            return Ok(None);
        }
        self.store
            .put_smart_playlist(loaded.source_id().clone(), record.clone())?;
        loaded.replace_smart_playlist(record.clone())?;
        Ok(Some(AcceptedLibraryChange {
            smart_playlists: vec![record.id],
            ..AcceptedLibraryChange::default()
        }))
    }
}

fn record_from_playlist(playlist: &SmartPlaylist) -> SmartPlaylistRecord {
    SmartPlaylistRecord {
        id: playlist.id.clone(),
        name: playlist.name.clone(),
        position: playlist.position,
        builtin: playlist.builtin,
        definition: playlist.definition.clone(),
    }
}

fn playlist_from_record(record: SmartPlaylistRecord) -> Arc<SmartPlaylist> {
    Arc::new(SmartPlaylist {
        id: record.id,
        name: record.name,
        position: record.position,
        builtin: record.builtin,
        definition: record.definition,
    })
}

fn builtin_record(builtin: SmartPlaylistBuiltin, position: u32) -> SmartPlaylistRecord {
    SmartPlaylistRecord {
        id: SmartPlaylistId::new(format!("builtin:{}", builtin.key())),
        name: builtin.title().to_string(),
        position,
        builtin: Some(builtin),
        definition: builtin_definition(builtin),
    }
}

fn reordered_ids(
    ids: &[SmartPlaylistId],
    dragged_id: &SmartPlaylistId,
    target_id: &SmartPlaylistId,
    after: bool,
) -> Option<Vec<SmartPlaylistId>> {
    if dragged_id == target_id {
        return None;
    }
    let source_index = ids.iter().position(|id| id == dragged_id)?;
    let target_index = ids.iter().position(|id| id == target_id)?;
    let mut reordered = ids.to_vec();
    let dragged = reordered.remove(source_index);
    let mut insert_index = target_index + usize::from(after);
    if source_index < insert_index {
        insert_index -= 1;
    }
    reordered.insert(insert_index.min(reordered.len()), dragged);
    (reordered != ids).then_some(reordered)
}

fn missing_smart_playlist(id: &SmartPlaylistId) -> crate::LibraryError {
    crate::LibraryError::Persistence(format!("smart playlist {} is not loaded", id.as_str()))
}

fn random_hex() -> crate::LibraryResult<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| {
        crate::LibraryError::Persistence(format!(
            "could not create a smart playlist identity: {error}"
        ))
    })?;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(value)
}
