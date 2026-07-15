use super::*;

use std::collections::{BTreeMap, BTreeSet};

const ITEM_BATCH_SIZE: usize = 100;

#[async_trait(?Send)]
impl LibraryChangeResolver for JellyfinSource {
    async fn resolve_changes(
        &self,
        changes: &SourceObjectChanges,
        known: &[SourceObjectMapping],
    ) -> SourceResult<LibraryChangeResolution> {
        if changes.is_empty() {
            return Ok(LibraryChangeResolution::Ignored);
        }

        let known = KnownObjects::new(known);
        let requested = changes.iter().cloned().collect::<BTreeSet<_>>();
        let available = self.items_by_ids(&requested).await?;
        let mut observation = Observation::default();
        let mut album_ids = BTreeSet::new();

        for raw_id in &requested {
            let Some(item) = available.get(raw_id) else {
                match known.kinds(raw_id) {
                    None => {
                        observation.ignored_source_objects.insert(raw_id.clone());
                    }
                    Some(kinds)
                        if kinds.len() == 1
                            && matches!(
                                kinds.first(),
                                Some(SourceEntityKind::Track | SourceEntityKind::Playlist)
                            ) =>
                    {
                        observation.missing_source_objects.insert(raw_id.clone());
                    }
                    Some(_) => return Ok(LibraryChangeResolution::Full),
                }
                continue;
            };

            match current_item_kind(item) {
                CurrentItemKind::Track => {
                    if !known.matches_only(raw_id, SourceEntityKind::Track) {
                        return Ok(LibraryChangeResolution::Full);
                    }
                    let track = track_from_item(item.clone());
                    if let Some(album_id) = raw_entity_id(track.album_id.as_str(), "album") {
                        album_ids.insert(album_id.to_string());
                    }
                    observation.add_track(raw_id, track);
                }
                CurrentItemKind::Album => {
                    if !known.matches_only(raw_id, SourceEntityKind::Album) {
                        return Ok(LibraryChangeResolution::Full);
                    }
                    observation.add_album(raw_id, album_from_item(item.clone()));
                }
                CurrentItemKind::Artist => {
                    let Some(roles) = known.artist_roles(raw_id) else {
                        return Ok(LibraryChangeResolution::Full);
                    };
                    observation.add_artist(raw_id, artist_from_item(item.clone()), roles);
                }
                CurrentItemKind::Playlist => {
                    if !known.matches_only(raw_id, SourceEntityKind::Playlist) {
                        return Ok(LibraryChangeResolution::Full);
                    }
                    let playlist = playlist_from_item(item.clone());
                    let Some(snapshot) = self.read_playlist_snapshot(playlist).await? else {
                        return Ok(LibraryChangeResolution::Full);
                    };
                    observation.add_playlist(raw_id, snapshot);
                }
                CurrentItemKind::Other if known.kinds(raw_id).is_none() => {
                    observation.ignored_source_objects.insert(raw_id.clone());
                }
                CurrentItemKind::Other => return Ok(LibraryChangeResolution::Full),
            }
        }

        let missing_albums = album_ids
            .into_iter()
            .filter(|raw_id| !observation.has_mapping(raw_id, SourceEntityKind::Album))
            .collect::<BTreeSet<_>>();
        let albums = self.items_by_ids(&missing_albums).await?;
        for raw_id in missing_albums {
            let Some(item) = albums.get(&raw_id) else {
                return Ok(LibraryChangeResolution::Full);
            };
            if current_item_kind(item) != CurrentItemKind::Album
                || !known.matches_only(&raw_id, SourceEntityKind::Album)
            {
                return Ok(LibraryChangeResolution::Full);
            }
            observation.add_album(&raw_id, album_from_item(item.clone()));
        }

        if observation.mappings.is_empty() && observation.missing_source_objects.is_empty() {
            return Ok(LibraryChangeResolution::Ignored);
        }

        Ok(LibraryChangeResolution::Exact(Box::new(
            observation.finish(),
        )))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurrentItemKind {
    Track,
    Album,
    Artist,
    Playlist,
    Other,
}

fn current_item_kind(item: &JellyfinItem) -> CurrentItemKind {
    match item.item_type.as_deref() {
        Some(kind) if kind.eq_ignore_ascii_case("Audio") => CurrentItemKind::Track,
        Some(kind) if kind.eq_ignore_ascii_case("MusicAlbum") => CurrentItemKind::Album,
        Some(kind)
            if kind.eq_ignore_ascii_case("MusicArtist") || kind.eq_ignore_ascii_case("Artist") =>
        {
            CurrentItemKind::Artist
        }
        Some(kind) if kind.eq_ignore_ascii_case("Playlist") => CurrentItemKind::Playlist,
        _ => CurrentItemKind::Other,
    }
}

struct KnownObjects<'a> {
    by_source_id: BTreeMap<&'a str, BTreeSet<SourceEntityKind>>,
}

impl<'a> KnownObjects<'a> {
    fn new(mappings: &'a [SourceObjectMapping]) -> Self {
        let mut by_source_id = BTreeMap::<&str, BTreeSet<SourceEntityKind>>::new();
        for mapping in mappings {
            by_source_id
                .entry(&mapping.source_object_id)
                .or_default()
                .insert(mapping.entity_kind);
        }
        Self { by_source_id }
    }

    fn kinds(&self, raw_id: &str) -> Option<&BTreeSet<SourceEntityKind>> {
        self.by_source_id.get(raw_id)
    }

    fn matches_only(&self, raw_id: &str, kind: SourceEntityKind) -> bool {
        self.kinds(raw_id)
            .is_none_or(|kinds| kinds.len() == 1 && kinds.contains(&kind))
    }

    fn artist_roles(&self, raw_id: &str) -> Option<&BTreeSet<SourceEntityKind>> {
        self.kinds(raw_id).filter(|roles| {
            !roles.is_empty()
                && roles.iter().all(|role| {
                    matches!(
                        role,
                        SourceEntityKind::Artist | SourceEntityKind::AlbumArtist
                    )
                })
        })
    }
}

#[derive(Default)]
struct Observation {
    mappings: BTreeMap<(String, SourceEntityKind), SourceObjectMapping>,
    missing_source_objects: BTreeSet<String>,
    ignored_source_objects: BTreeSet<String>,
    albums: BTreeMap<String, Album>,
    tracks: BTreeMap<String, Track>,
    artists: BTreeMap<String, Artist>,
    album_artists: BTreeMap<String, Artist>,
    playlists: BTreeMap<String, PlaylistSnapshot>,
}

impl Observation {
    fn add_track(&mut self, raw_id: &str, track: Track) {
        self.add_mapping(
            raw_id,
            SourceEntityKind::Track,
            track.id.as_str().to_string(),
        );
        self.tracks.insert(track.id.as_str().to_string(), track);
    }

    fn add_album(&mut self, raw_id: &str, album: Album) {
        self.add_mapping(
            raw_id,
            SourceEntityKind::Album,
            album.id.as_str().to_string(),
        );
        self.albums.insert(album.id.as_str().to_string(), album);
    }

    fn add_artist(&mut self, raw_id: &str, artist: Artist, roles: &BTreeSet<SourceEntityKind>) {
        for role in roles {
            match role {
                SourceEntityKind::Artist => {
                    self.add_mapping(raw_id, *role, artist.id.as_str().to_string());
                    self.artists
                        .insert(artist.id.as_str().to_string(), artist.clone());
                }
                SourceEntityKind::AlbumArtist => {
                    self.add_mapping(raw_id, *role, artist.id.as_str().to_string());
                    self.album_artists
                        .insert(artist.id.as_str().to_string(), artist.clone());
                }
                _ => {}
            }
        }
    }

    fn add_playlist(&mut self, raw_id: &str, snapshot: PlaylistSnapshot) {
        self.add_mapping(
            raw_id,
            SourceEntityKind::Playlist,
            snapshot.playlist.id.as_str().to_string(),
        );
        self.playlists
            .insert(snapshot.playlist.id.as_str().to_string(), snapshot);
    }

    fn add_mapping(&mut self, raw_id: &str, kind: SourceEntityKind, entity_id: String) {
        self.mappings.insert(
            (raw_id.to_string(), kind),
            SourceObjectMapping {
                source_object_id: raw_id.to_string(),
                entity_kind: kind,
                entity_id,
            },
        );
    }

    fn has_mapping(&self, raw_id: &str, kind: SourceEntityKind) -> bool {
        self.mappings.contains_key(&(raw_id.to_string(), kind))
    }

    fn finish(self) -> LibraryObjectObservation {
        LibraryObjectObservation {
            mappings: self.mappings.into_values().collect(),
            missing_source_objects: self.missing_source_objects,
            ignored_source_objects: self.ignored_source_objects,
            albums: self.albums.into_values().collect(),
            tracks: self.tracks.into_values().collect(),
            artists: self.artists.into_values().collect(),
            album_artists: self.album_artists.into_values().collect(),
            genres: Vec::new(),
            playlists: self.playlists.into_values().collect(),
            home_sections: Vec::new(),
            track_music_folders: Vec::new(),
        }
    }
}

fn raw_entity_id<'a>(item_id: &'a str, kind: &str) -> Option<&'a str> {
    item_id
        .strip_prefix(&format!("jellyfin:{kind}:"))
        .filter(|raw_id| !raw_id.is_empty())
}

impl JellyfinSource {
    async fn items_by_ids(
        &self,
        ids: &BTreeSet<String>,
    ) -> SourceResult<BTreeMap<String, JellyfinItem>> {
        let ids = ids.iter().cloned().collect::<Vec<_>>();
        let mut items = BTreeMap::new();
        for chunk in ids.chunks(ITEM_BATCH_SIZE) {
            let mut url = endpoint(&self.base_url, "Items")?;
            url.query_pairs_mut()
                .append_pair("UserId", &self.user_id)
                .append_pair("Recursive", "true")
                .append_pair("Ids", &chunk.join(","))
                .append_pair("Limit", &chunk.len().to_string())
                .append_pair("Fields", MIXED_ITEM_FIELDS);
            let response = self.get_json::<ItemQueryResult>(url).await?;
            for item in response.items {
                items.insert(item.id.clone(), item);
            }
        }
        Ok(items)
    }
}
