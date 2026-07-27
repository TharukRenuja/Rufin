//! Effective Home composition for one loaded source.
//!
//! Sources contribute only their ordered native sections. Library derives
//! Explore from the complete loaded source and derives Rufin-defined sections
//! from accepted imports and listening activity. A mounted route keeps its
//! `Arc<HomeSnapshot>` while later work replaces only the next snapshot.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{
    AlbumId, AlbumSummary, FavoriteItemId, GenreSummary, Library, LibraryError, LibraryResult,
    LoadedLibrary, MusicFolderId, SourceId, Track, TrackId,
    browse::{album_in_scope, album_summary, genre_summary, track_in_scope},
    msgid,
};

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
const HOME_GENRE_LIMIT: usize = 12;

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum SourceHomeSectionKind {
    MostPlayed,
    NewlyAdded,
    RecentlyPlayed,
    RecentlyReleased,
}

impl From<SourceHomeSectionKind> for HomeSectionKind {
    fn from(value: SourceHomeSectionKind) -> Self {
        match value {
            SourceHomeSectionKind::MostPlayed => Self::MostPlayed,
            SourceHomeSectionKind::NewlyAdded => Self::NewlyAdded,
            SourceHomeSectionKind::RecentlyPlayed => Self::RecentlyPlayed,
            SourceHomeSectionKind::RecentlyReleased => Self::RecentlyReleased,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum HomeItemId {
    Album(AlbumId),
    Track(TrackId),
}

/// One ordered section returned by a concrete source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceHomeSection {
    pub kind: SourceHomeSectionKind,
    pub items: Vec<HomeItemId>,
}

/// The accepted input to Rufin's one Home composer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HomeFacts {
    RufinDefined,
    Source {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sections: Vec<SourceHomeSection>,
    },
}

impl HomeFacts {
    pub const fn is_rufin_defined(&self) -> bool {
        matches!(self, Self::RufinDefined)
    }
}

#[derive(Clone, Debug)]
pub enum LoadedHomeItem {
    Album(AlbumSummary),
    Track(Track),
}

impl LoadedHomeItem {
    pub fn id(&self) -> HomeItemId {
        match self {
            Self::Album(album) => HomeItemId::Album(album.album.id.clone()),
            Self::Track(track) => HomeItemId::Track(track.id.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadedHomeSection {
    pub kind: HomeSectionKind,
    pub items: Arc<[LoadedHomeItem]>,
}

#[derive(Clone, Debug)]
pub enum ShowcaseItem {
    Album(AlbumSummary),
    Track(Track),
}

impl ShowcaseItem {
    pub fn id(&self) -> HomeItemId {
        match self {
            Self::Album(album) => HomeItemId::Album(album.album.id.clone()),
            Self::Track(track) => HomeItemId::Track(track.id.clone()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HomeSnapshot {
    pub sections: Arc<[Arc<LoadedHomeSection>]>,
    pub genres: Arc<[GenreSummary]>,
    pub showcase: Option<ShowcaseItem>,
}

impl HomeSnapshot {
    pub fn section(&self, kind: HomeSectionKind) -> Option<&Arc<LoadedHomeSection>> {
        self.sections.iter().find(|section| section.kind == kind)
    }

    /// Keeps every unaffected mounted section handle while replacing only the
    /// section the user refreshed.
    pub fn replacing_section(
        &self,
        kind: HomeSectionKind,
        next: Option<Arc<LoadedHomeSection>>,
    ) -> HomeSnapshot {
        let mut sections = self.sections.to_vec();
        let current = sections.iter().position(|section| section.kind == kind);
        match (current, next) {
            (Some(position), Some(section)) => sections[position] = section,
            (Some(position), None) => {
                sections.remove(position);
            }
            (None, Some(section)) => {
                let position = sections
                    .iter()
                    .position(|current| {
                        home_section_position(current.kind) > home_section_position(kind)
                    })
                    .unwrap_or(sections.len());
                sections.insert(position, section);
            }
            (None, None) => {}
        }
        HomeSnapshot {
            sections: sections.into(),
            genres: Arc::clone(&self.genres),
            showcase: self.showcase.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LocalImport {
    pub(crate) track_id: TrackId,
    pub(crate) first_seen_at: i64,
}

#[derive(Clone, Debug)]
struct HomeSessionState {
    explore_variation: u64,
    showcase: Option<HomeItemId>,
}

pub(crate) struct HomeSessions {
    seed: u64,
    sources: Mutex<HashMap<SourceId, HomeSessionState>>,
}

impl HomeSessions {
    pub(crate) fn new() -> Self {
        static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
        let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64);
        Self {
            seed: time.rotate_left(17) ^ counter.wrapping_mul(0x9e37_79b9_7f4a_7c15),
            sources: Mutex::new(HashMap::new()),
        }
    }

    fn state_for(&self, source_id: &SourceId) -> LibraryResult<HomeSessionState> {
        let mut sources = self
            .sources
            .lock()
            .map_err(|_| LibraryError::Persistence("Home session lock was poisoned".to_string()))?;
        let state = sources
            .entry(source_id.clone())
            .or_insert_with(|| HomeSessionState {
                explore_variation: 0,
                showcase: None,
            });
        Ok(state.clone())
    }

    fn advance_explore(&self, source_id: &SourceId) -> LibraryResult<HomeSessionState> {
        let mut sources = self
            .sources
            .lock()
            .map_err(|_| LibraryError::Persistence("Home session lock was poisoned".to_string()))?;
        let state = sources.get_mut(source_id).ok_or_else(|| {
            LibraryError::Persistence(format!("Home session for {source_id} is not loaded"))
        })?;
        state.explore_variation = state.explore_variation.wrapping_add(1);
        Ok(state.clone())
    }

    fn showcase_for(
        &self,
        source_id: &SourceId,
        album_ids: &[AlbumId],
        track_ids: &[TrackId],
    ) -> LibraryResult<Option<HomeItemId>> {
        let mut sources = self
            .sources
            .lock()
            .map_err(|_| LibraryError::Persistence("Home session lock was poisoned".to_string()))?;
        let state = sources
            .entry(source_id.clone())
            .or_insert_with(|| HomeSessionState {
                explore_variation: 0,
                showcase: None,
            });
        let still_eligible = state
            .showcase
            .as_ref()
            .is_some_and(|showcase| match showcase {
                HomeItemId::Album(id) => album_ids.contains(id),
                HomeItemId::Track(id) => track_ids.contains(id),
            });
        if !still_eligible {
            state.showcase = choose_showcase(self.seed, source_id, album_ids, track_ids);
        }
        Ok(state.showcase.clone())
    }

    pub(crate) fn remove_source(&self, source_id: &SourceId) -> LibraryResult<()> {
        self.sources
            .lock()
            .map_err(|_| LibraryError::Persistence("Home session lock was poisoned".to_string()))?
            .remove(source_id);
        Ok(())
    }
}

impl Library {
    pub(crate) fn prepare_home(&self, loaded: &Arc<LoadedLibrary>) -> LibraryResult<()> {
        self.home_sessions.state_for(loaded.source_id())?;
        Ok(())
    }

    pub(crate) fn replace_home_facts(
        &self,
        loaded: &Arc<LoadedLibrary>,
        facts: HomeFacts,
    ) -> LibraryResult<()> {
        let mut state = loaded.write_state()?;
        state.home_facts = facts;
        Ok(())
    }

    pub fn home(
        &self,
        loaded: &Arc<LoadedLibrary>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LibraryResult<Arc<HomeSnapshot>> {
        let session = self.home_sessions.state_for(loaded.source_id())?;
        self.compose_home(loaded, music_folder_id, session.explore_variation)
    }

    /// Updates one favorited item in the snapshot prepared for the next visit.
    pub fn home_after_favorite(
        &self,
        loaded: &Arc<LoadedLibrary>,
        music_folder_id: Option<&MusicFolderId>,
        current: &Arc<HomeSnapshot>,
        favorite: &FavoriteItemId,
    ) -> LibraryResult<Arc<HomeSnapshot>> {
        if matches!(favorite, FavoriteItemId::Artist(_)) {
            return Ok(Arc::clone(current));
        }
        let state = loaded.read_state()?;
        let track_id = match favorite {
            FavoriteItemId::Track(id) => Some(id),
            FavoriteItemId::Album(_) | FavoriteItemId::Artist(_) => None,
        };
        let album_id = match favorite {
            FavoriteItemId::Album(id) => Some(id),
            FavoriteItemId::Track(_) | FavoriteItemId::Artist(_) => None,
        };
        let mut changed = false;
        let sections = current
            .sections
            .iter()
            .map(|section| {
                let touched = section.items.iter().any(|item| match item {
                    LoadedHomeItem::Track(track) => track_id == Some(&track.id),
                    LoadedHomeItem::Album(album) => album_id == Some(&album.album.id),
                });
                if !touched {
                    return Arc::clone(section);
                }
                changed = true;
                let items = section
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        LoadedHomeItem::Track(track) if track_id == Some(&track.id) => state
                            .tracks
                            .get(&track.id)
                            .filter(|track| track_in_scope(track, music_folder_id))
                            .cloned()
                            .map(LoadedHomeItem::Track),
                        LoadedHomeItem::Album(album) if album_id == Some(&album.album.id) => {
                            album_summary(
                                &state,
                                state.albums.get(&album.album.id)?,
                                music_folder_id,
                            )
                            .map(LoadedHomeItem::Album)
                        }
                        item => Some(item.clone()),
                    })
                    .collect::<Vec<_>>();
                Arc::new(LoadedHomeSection {
                    kind: section.kind,
                    items: items.into(),
                })
            })
            .collect::<Vec<_>>();
        let showcase = match current.showcase.as_ref() {
            Some(ShowcaseItem::Track(track)) if track_id == Some(&track.id) => {
                changed = true;
                state
                    .tracks
                    .get(&track.id)
                    .filter(|track| track_in_scope(track, music_folder_id))
                    .cloned()
                    .map(ShowcaseItem::Track)
            }
            Some(ShowcaseItem::Album(album)) if album_id == Some(&album.album.id) => {
                changed = true;
                state
                    .albums
                    .get(&album.album.id)
                    .and_then(|album| album_summary(&state, album, music_folder_id))
                    .map(ShowcaseItem::Album)
            }
            showcase => showcase.cloned(),
        };
        if !changed {
            return Ok(Arc::clone(current));
        }
        Ok(Arc::new(HomeSnapshot {
            sections: sections.into(),
            genres: Arc::clone(&current.genres),
            showcase,
        }))
    }

    /// Prepares Most Played and Recently Played for the next Home visit.
    ///
    /// The mounted snapshot remains untouched, and work stays bounded by the
    /// number of items already shown on Home.
    pub fn home_after_play(
        &self,
        loaded: &Arc<LoadedLibrary>,
        music_folder_id: Option<&MusicFolderId>,
        current: &Arc<HomeSnapshot>,
        track_id: &TrackId,
    ) -> LibraryResult<Arc<HomeSnapshot>> {
        let state = loaded.read_state()?;
        if !state.home_facts.is_rufin_defined() {
            return Ok(Arc::clone(current));
        }
        let Some(track) = state.tracks.get(track_id).cloned() else {
            return Ok(Arc::clone(current));
        };
        let in_scope = track_in_scope(&track, music_folder_id);

        let mut most_played = home_section_tracks(current, HomeSectionKind::MostPlayed);
        most_played.retain(|current| current.id != track.id);
        if in_scope && track.play_count.unwrap_or(0) > 0 {
            most_played.push(track.clone());
        }
        most_played.sort_by(|left, right| {
            right
                .play_count
                .cmp(&left.play_count)
                .then_with(|| right.last_played.cmp(&left.last_played))
                .then_with(|| left.id.cmp(&right.id))
        });
        most_played.truncate(HOME_SECTION_ITEM_LIMIT);

        let mut recently_played = home_section_tracks(current, HomeSectionKind::RecentlyPlayed);
        recently_played.retain(|current| current.id != track.id);
        if in_scope {
            recently_played.insert(0, track);
        }
        recently_played.truncate(HOME_SECTION_ITEM_LIMIT);

        let mut sections = current
            .sections
            .iter()
            .filter(|section| {
                !matches!(
                    section.kind,
                    HomeSectionKind::MostPlayed | HomeSectionKind::RecentlyPlayed
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        push_nonempty(
            &mut sections,
            Arc::new(LoadedHomeSection {
                kind: HomeSectionKind::MostPlayed,
                items: most_played
                    .into_iter()
                    .map(LoadedHomeItem::Track)
                    .collect::<Vec<_>>()
                    .into(),
            }),
        );
        push_nonempty(
            &mut sections,
            Arc::new(LoadedHomeSection {
                kind: HomeSectionKind::RecentlyPlayed,
                items: recently_played
                    .into_iter()
                    .map(LoadedHomeItem::Track)
                    .collect::<Vec<_>>()
                    .into(),
            }),
        );
        sections.sort_by_key(|section| home_section_position(section.kind));
        Ok(Arc::new(HomeSnapshot {
            sections: sections.into(),
            genres: Arc::clone(&current.genres),
            showcase: current.showcase.clone(),
        }))
    }

    fn compose_home(
        &self,
        loaded: &Arc<LoadedLibrary>,
        music_folder_id: Option<&MusicFolderId>,
        explore_variation: u64,
    ) -> LibraryResult<Arc<HomeSnapshot>> {
        let state = loaded.read_state()?;
        let (album_ids, track_ids) = home_candidates(&state, music_folder_id);
        let showcase =
            self.home_sessions
                .showcase_for(loaded.source_id(), &album_ids, &track_ids)?;
        Ok(Arc::new(compose_home(
            loaded.source_id(),
            &state,
            self.home_sessions.seed,
            explore_variation,
            music_folder_id,
            showcase.as_ref(),
        )))
    }

    /// Rebuilds one Rufin-defined section without changing the mounted Home.
    pub fn refresh_rufin_home_section(
        &self,
        loaded: &Arc<LoadedLibrary>,
        music_folder_id: Option<&MusicFolderId>,
        current: &Arc<HomeSnapshot>,
        kind: HomeSectionKind,
    ) -> LibraryResult<Arc<HomeSnapshot>> {
        let session = if kind == HomeSectionKind::Explore {
            self.home_sessions.advance_explore(loaded.source_id())?
        } else {
            self.home_sessions.state_for(loaded.source_id())?
        };
        let state = loaded.read_state()?;
        if kind != HomeSectionKind::Explore && !state.home_facts.is_rufin_defined() {
            return Err(LibraryError::Persistence(format!(
                "{} is supplied by this source",
                kind.title()
            )));
        }
        let section = if kind == HomeSectionKind::Explore {
            compose_explore(
                loaded.source_id(),
                &state,
                self.home_sessions.seed,
                session.explore_variation,
                music_folder_id,
            )
        } else {
            compose_rufin_section(kind, &state, music_folder_id)
        };
        Ok(Arc::new(current.replacing_section(
            kind,
            (!section.items.is_empty()).then_some(section),
        )))
    }

    /// Accepts one provider-owned Home section and returns a snapshot whose
    /// unrelated section handles, showcase, and genres remain unchanged.
    pub fn accept_home_section(
        &self,
        loaded: &Arc<LoadedLibrary>,
        music_folder_id: Option<&MusicFolderId>,
        current: &Arc<HomeSnapshot>,
        section: SourceHomeSection,
    ) -> LibraryResult<Arc<HomeSnapshot>> {
        let kind = HomeSectionKind::from(section.kind);
        let (home, next_section) = {
            let state = loaded.read_state()?;
            let HomeFacts::Source { sections } = &state.home_facts else {
                return Err(LibraryError::Persistence(format!(
                    "{} is provided by Rufin for this source",
                    kind.title()
                )));
            };
            let mut next = sections
                .iter()
                .filter(|current| current.kind != section.kind)
                .cloned()
                .collect::<Vec<_>>();
            let next_section = if section.items.is_empty() {
                None
            } else {
                let resolved = resolve_source_section(&section, &state, music_folder_id);
                next.push(section);
                (!resolved.items.is_empty()).then_some(resolved)
            };
            next.sort_by_key(|section| home_section_position(section.kind.into()));
            (HomeFacts::Source { sections: next }, next_section)
        };
        self.store.replace_home(
            loaded.source_id().clone(),
            loaded.library_id(),
            home.clone(),
        )?;
        {
            let mut state = loaded.write_state()?;
            state.home_facts = home;
        }
        Ok(Arc::new(current.replacing_section(kind, next_section)))
    }
}

fn home_candidates(
    state: &crate::loaded::LoadedState,
    music_folder_id: Option<&MusicFolderId>,
) -> (Vec<AlbumId>, Vec<TrackId>) {
    let mut album_ids = state
        .albums
        .values()
        .filter(|album| album_in_scope(state, album, music_folder_id))
        .map(|album| album.id.clone())
        .collect::<Vec<_>>();
    album_ids.sort();
    if !album_ids.is_empty() {
        return (album_ids, Vec::new());
    }
    let mut track_ids = state
        .tracks
        .values()
        .filter(|track| track_in_scope(track, music_folder_id))
        .map(|track| track.id.clone())
        .collect::<Vec<_>>();
    track_ids.sort();
    (album_ids, track_ids)
}

fn compose_home(
    source_id: &SourceId,
    state: &crate::loaded::LoadedState,
    seed: u64,
    explore_variation: u64,
    music_folder_id: Option<&MusicFolderId>,
    showcase_id: Option<&HomeItemId>,
) -> HomeSnapshot {
    let mut sections = Vec::new();
    push_nonempty(
        &mut sections,
        compose_explore(source_id, state, seed, explore_variation, music_folder_id),
    );
    match &state.home_facts {
        HomeFacts::RufinDefined => {
            for kind in [
                HomeSectionKind::MostPlayed,
                HomeSectionKind::NewlyAdded,
                HomeSectionKind::RecentlyPlayed,
                HomeSectionKind::RecentlyReleased,
            ] {
                push_nonempty(
                    &mut sections,
                    compose_rufin_section(kind, state, music_folder_id),
                );
            }
        }
        HomeFacts::Source {
            sections: source_sections,
        } => {
            for section in source_sections {
                push_nonempty(
                    &mut sections,
                    resolve_source_section(section, state, music_folder_id),
                );
            }
        }
    }
    let mut genre_ids = state
        .genres
        .keys()
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    genre_ids.sort_by(|left, right| {
        let left = &state.genres[left];
        let right = &state.genres[right];
        text_cmp(&left.name, &right.name).then(left.id.cmp(&right.id))
    });
    let showcase = showcase_id.and_then(|item| resolve_showcase(item, state, music_folder_id));
    HomeSnapshot {
        sections: sections.into(),
        genres: genre_ids
            .into_iter()
            .filter_map(|id| genre_summary(state, state.genres.get(&id)?, music_folder_id))
            .take(HOME_GENRE_LIMIT)
            .collect::<Vec<_>>()
            .into(),
        showcase,
    }
}

fn compose_explore(
    source_id: &SourceId,
    state: &crate::loaded::LoadedState,
    seed: u64,
    variation: u64,
    music_folder_id: Option<&MusicFolderId>,
) -> Arc<LoadedHomeSection> {
    let mut albums = state
        .albums
        .values()
        .filter(|album| album_in_scope(state, album, music_folder_id))
        .cloned()
        .collect::<Vec<_>>();
    let items = if albums.is_empty() {
        let mut tracks = state.tracks.values().cloned().collect::<Vec<_>>();
        tracks.retain(|track| track_in_scope(track, music_folder_id));
        tracks.sort_by(|left, right| left.id.cmp(&right.id));
        sampled_indexes(tracks.len(), seed, source_id, variation)
            .filter_map(|index| tracks.get(index).cloned())
            .map(LoadedHomeItem::Track)
            .collect()
    } else {
        albums.sort_by(|left, right| left.id.cmp(&right.id));
        sampled_indexes(albums.len(), seed, source_id, variation)
            .filter_map(|index| album_summary(state, albums.get(index)?, music_folder_id))
            .map(LoadedHomeItem::Album)
            .collect()
    };
    Arc::new(LoadedHomeSection {
        kind: HomeSectionKind::Explore,
        items,
    })
}

fn compose_rufin_section(
    kind: HomeSectionKind,
    state: &crate::loaded::LoadedState,
    music_folder_id: Option<&MusicFolderId>,
) -> Arc<LoadedHomeSection> {
    let items = match kind {
        HomeSectionKind::Explore => Vec::new(),
        HomeSectionKind::MostPlayed => {
            let mut tracks = state
                .tracks
                .values()
                .filter(|track| track.play_count.unwrap_or(0) > 0)
                .filter(|track| track_in_scope(track, music_folder_id))
                .cloned()
                .collect::<Vec<_>>();
            tracks.sort_by(|left, right| {
                right
                    .play_count
                    .cmp(&left.play_count)
                    .then_with(|| right.last_played.cmp(&left.last_played))
                    .then_with(|| left.id.cmp(&right.id))
            });
            tracks
                .into_iter()
                .take(HOME_SECTION_ITEM_LIMIT)
                .map(LoadedHomeItem::Track)
                .collect()
        }
        HomeSectionKind::RecentlyPlayed => {
            let mut seen = HashSet::new();
            state
                .recent_plays
                .iter()
                .map(|play| &play.track_id)
                .filter(|id| seen.insert((*id).clone()))
                .filter_map(|id| state.tracks.get(id).cloned())
                .filter(|track| track_in_scope(track, music_folder_id))
                .take(HOME_SECTION_ITEM_LIMIT)
                .map(LoadedHomeItem::Track)
                .collect()
        }
        HomeSectionKind::NewlyAdded => {
            let mut albums = state
                .albums
                .values()
                .filter_map(|album| {
                    let first_seen_at = state
                        .albums
                        .get(&album.id)?
                        .tracks
                        .iter()
                        .filter(|slot| {
                            state
                                .tracks
                                .get_slot(**slot)
                                .is_some_and(|track| track_in_scope(track, music_folder_id))
                        })
                        .filter_map(|slot| state.local_imports.get(slot))
                        .copied()
                        .max()?;
                    Some((first_seen_at, album.id.clone()))
                })
                .collect::<Vec<_>>();
            albums.sort_by(|(left_seen, left), (right_seen, right)| {
                right_seen.cmp(left_seen).then_with(|| left.cmp(right))
            });
            albums
                .into_iter()
                .take(HOME_SECTION_ITEM_LIMIT)
                .filter_map(|(_, album_id)| {
                    album_summary(state, state.albums.get(&album_id)?, music_folder_id)
                        .map(LoadedHomeItem::Album)
                })
                .collect()
        }
        HomeSectionKind::RecentlyReleased => {
            let mut albums = state
                .albums
                .values()
                .filter(|album| album.release_date.is_some() || album.year > 0)
                .filter(|album| album_in_scope(state, album, music_folder_id))
                .collect::<Vec<_>>();
            albums.sort_by(|left, right| {
                right
                    .release_date
                    .cmp(&left.release_date)
                    .then_with(|| right.year.cmp(&left.year))
                    .then_with(|| left.id.cmp(&right.id))
            });
            albums
                .into_iter()
                .take(HOME_SECTION_ITEM_LIMIT)
                .filter_map(|album| {
                    album_summary(state, &album, music_folder_id).map(LoadedHomeItem::Album)
                })
                .collect()
        }
    };
    Arc::new(LoadedHomeSection {
        kind,
        items: items.into(),
    })
}

fn resolve_source_section(
    section: &SourceHomeSection,
    state: &crate::loaded::LoadedState,
    music_folder_id: Option<&MusicFolderId>,
) -> Arc<LoadedHomeSection> {
    let mut seen = HashSet::new();
    let items = section
        .items
        .iter()
        .filter(|item| seen.insert((*item).clone()))
        .filter_map(|item| match item {
            HomeItemId::Album(id) => album_summary(state, state.albums.get(id)?, music_folder_id)
                .map(LoadedHomeItem::Album),
            HomeItemId::Track(id) => state
                .tracks
                .get(id)
                .filter(|track| track_in_scope(track, music_folder_id))
                .cloned()
                .map(LoadedHomeItem::Track),
        })
        .take(HOME_SECTION_ITEM_LIMIT)
        .collect();
    Arc::new(LoadedHomeSection {
        kind: section.kind.into(),
        items,
    })
}

fn resolve_showcase(
    item: &HomeItemId,
    state: &crate::loaded::LoadedState,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<ShowcaseItem> {
    match item {
        HomeItemId::Album(id) => {
            album_summary(state, state.albums.get(id)?, music_folder_id).map(ShowcaseItem::Album)
        }
        HomeItemId::Track(id) => state
            .tracks
            .get(id)
            .filter(|track| track_in_scope(track, music_folder_id))
            .cloned()
            .map(ShowcaseItem::Track),
    }
}

fn push_nonempty(sections: &mut Vec<Arc<LoadedHomeSection>>, section: Arc<LoadedHomeSection>) {
    if !section.items.is_empty() {
        sections.retain(|current| current.kind != section.kind);
        sections.push(section);
    }
}

fn home_section_tracks(current: &HomeSnapshot, kind: HomeSectionKind) -> Vec<Track> {
    current
        .section(kind)
        .into_iter()
        .flat_map(|section| section.items.iter())
        .filter_map(|item| match item {
            LoadedHomeItem::Track(track) => Some(track.clone()),
            LoadedHomeItem::Album(_) => None,
        })
        .collect()
}

fn home_section_position(kind: HomeSectionKind) -> u8 {
    match kind {
        HomeSectionKind::Explore => 0,
        HomeSectionKind::MostPlayed => 1,
        HomeSectionKind::NewlyAdded => 2,
        HomeSectionKind::RecentlyPlayed => 3,
        HomeSectionKind::RecentlyReleased => 4,
    }
}

fn sampled_indexes(
    len: usize,
    seed: u64,
    source_id: &SourceId,
    variation: u64,
) -> impl Iterator<Item = usize> {
    let start = (home_hash(
        seed ^ variation.rotate_left(23),
        source_id.as_str(),
        "explore-start",
    ) as usize)
        .checked_rem(len)
        .unwrap_or(0);
    let mut step = (home_hash(
        seed ^ variation.rotate_left(41),
        source_id.as_str(),
        "explore-step",
    ) as usize)
        .checked_rem(len)
        .unwrap_or(0)
        .max(1);
    while len > 1 && greatest_common_divisor(step, len) != 1 {
        step = (step + 1) % len;
        if step == 0 {
            step = 1;
        }
    }
    (0..len.min(HOME_SECTION_ITEM_LIMIT))
        .map(move |offset| (start + offset.wrapping_mul(step)) % len.max(1))
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn choose_showcase(
    seed: u64,
    source_id: &SourceId,
    album_ids: &[AlbumId],
    track_ids: &[TrackId],
) -> Option<HomeItemId> {
    if !album_ids.is_empty() {
        let index = home_hash(seed, source_id.as_str(), "showcase") as usize % album_ids.len();
        return Some(HomeItemId::Album(album_ids[index].clone()));
    }
    if !track_ids.is_empty() {
        let index = home_hash(seed, source_id.as_str(), "showcase") as usize % track_ids.len();
        return Some(HomeItemId::Track(track_ids[index].clone()));
    }
    None
}

fn text_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn home_hash(seed: u64, source: &str, value: &str) -> u64 {
    const OFFSET: u64 = 14_695_981_039_346_656_037;
    const PRIME: u64 = 1_099_511_628_211;
    source
        .bytes()
        .chain(seed.to_le_bytes())
        .chain([0xff])
        .chain(value.bytes())
        .fold(OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{HomeSectionKind, HomeSnapshot, LoadedHomeSection};

    fn section(kind: HomeSectionKind) -> Arc<LoadedHomeSection> {
        Arc::new(LoadedHomeSection {
            kind,
            items: Vec::new().into(),
        })
    }

    #[test]
    fn replacing_one_home_section_preserves_every_other_mounted_section() {
        let explore = section(HomeSectionKind::Explore);
        let old_recent = section(HomeSectionKind::RecentlyPlayed);
        let most_played = section(HomeSectionKind::MostPlayed);
        let genres = Vec::new().into();
        let current = HomeSnapshot {
            sections: vec![
                Arc::clone(&explore),
                Arc::clone(&old_recent),
                Arc::clone(&most_played),
            ]
            .into(),
            genres,
            showcase: None,
        };
        let next_recent = section(HomeSectionKind::RecentlyPlayed);
        let replaced = current.replacing_section(
            HomeSectionKind::RecentlyPlayed,
            Some(Arc::clone(&next_recent)),
        );

        assert!(Arc::ptr_eq(&replaced.sections[0], &explore));
        assert!(Arc::ptr_eq(&replaced.sections[1], &next_recent));
        assert!(Arc::ptr_eq(&replaced.sections[2], &most_played));
        assert!(Arc::ptr_eq(&replaced.genres, &current.genres));
    }

    #[test]
    fn replacing_one_home_section_can_remove_only_that_section() {
        let explore = section(HomeSectionKind::Explore);
        let recent = section(HomeSectionKind::RecentlyPlayed);
        let current = HomeSnapshot {
            sections: vec![Arc::clone(&explore), recent].into(),
            ..HomeSnapshot::default()
        };

        let replaced = current.replacing_section(HomeSectionKind::RecentlyPlayed, None);

        assert_eq!(replaced.sections.len(), 1);
        assert!(Arc::ptr_eq(&replaced.sections[0], &explore));
    }
}
