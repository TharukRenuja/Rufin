use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use library::{Album, AlbumArtwork, Artist, Genre, ImageRef, Mood, Playlist, SmartPlaylist, Track};

const COLLECTION_SLOT_LIMIT: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Candidate {
    Native(ImageRef),
    MusicBrainzReleaseGroup(String),
    MusicBrainzRelease(String),
    AlbumText { artist: String, album: String },
}

impl Candidate {
    pub(crate) fn stable_identity(&self) -> String {
        match self {
            Self::Native(image_ref) => format!(
                "native\0{}\0{}",
                image_ref.item_id,
                image_ref.tag.as_deref().unwrap_or_default()
            ),
            Self::MusicBrainzReleaseGroup(id) => format!("mb-release-group\0{id}"),
            Self::MusicBrainzRelease(id) => format!("mb-release\0{id}"),
            Self::AlbumText { artist, album } => format!("album-text\0{artist}\0{album}"),
        }
    }

    pub const fn is_external(&self) -> bool {
        !matches!(self, Self::Native(_))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtworkBinding {
    candidates: Arc<[Candidate]>,
    stable_identity: Arc<str>,
}

impl ArtworkBinding {
    pub fn new() -> Self {
        Self {
            candidates: Arc::new([]),
            stable_identity: Arc::from(""),
        }
    }

    pub fn album(album: &Album) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_album(
            album.image_ref.as_ref(),
            album.musicbrainz_release_group_id.as_deref(),
            album.musicbrainz_album_id.as_deref(),
            &album.artist,
            &album.title,
        );
        candidates.finish()
    }

    pub fn album_artwork(album: &AlbumArtwork) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_album_artwork(album);
        candidates.finish()
    }

    pub fn track(track: &Track) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_native(track.image_ref.as_ref());
        if let Some(album) = track.album_artwork.as_ref() {
            candidates.push_album_artwork(album);
        } else {
            candidates.push_album_text(&track.artist, &track.album);
        }
        candidates.finish()
    }

    pub fn artist(artist: &Artist) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_native(artist.image_ref.as_ref());
        candidates.push_representative_native(&artist.representative_albums);
        candidates.push_representative_external(&artist.representative_albums);
        candidates.finish()
    }

    pub fn genre(genre: &Genre) -> Self {
        Self::collection(
            &genre.representative_albums,
            genre.image_ref.as_ref(),
            false,
        )
    }

    pub fn genre_slots(genre: &Genre) -> Vec<Self> {
        Self::collection_slots(
            &genre.representative_albums,
            genre.image_ref.as_ref(),
            false,
        )
    }

    pub fn mood(mood: &Mood) -> Self {
        Self::collection(&mood.representative_albums, None, false)
    }

    pub fn mood_slots(mood: &Mood) -> Vec<Self> {
        Self::collection_slots(&mood.representative_albums, None, false)
    }

    pub fn playlist(playlist: &Playlist, prefer_server_cover: bool) -> Self {
        Self::collection(
            &playlist.representative_albums,
            playlist.image_ref.as_ref(),
            prefer_server_cover,
        )
    }

    pub fn playlist_slots(playlist: &Playlist, prefer_server_cover: bool) -> Vec<Self> {
        Self::collection_slots(
            &playlist.representative_albums,
            playlist.image_ref.as_ref(),
            prefer_server_cover,
        )
    }

    pub fn smart_playlist(playlist: &SmartPlaylist) -> Self {
        Self::collection(&playlist.representative_albums, None, false)
    }

    pub fn smart_playlist_slots(playlist: &SmartPlaylist) -> Vec<Self> {
        Self::collection_slots(&playlist.representative_albums, None, false)
    }

    pub fn album_text(artist: &str, album: &str) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_album_text(artist, album);
        candidates.finish()
    }

    pub fn album_facts(
        artist: &str,
        album: &str,
        musicbrainz_release_group_id: Option<&str>,
        musicbrainz_album_id: Option<&str>,
    ) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_album(
            None,
            musicbrainz_release_group_id,
            musicbrainz_album_id,
            artist,
            album,
        );
        candidates.finish()
    }

    fn collection(
        representative_albums: &[AlbumArtwork],
        direct_ref: Option<&ImageRef>,
        prefer_direct: bool,
    ) -> Self {
        if prefer_direct && direct_ref.is_some() {
            return Self::from_native(direct_ref);
        }
        let mut candidates = CandidateBuilder::default();
        for representative in Self::unique_representatives(representative_albums) {
            for candidate in representative.candidates.iter().cloned() {
                candidates.push(candidate);
            }
        }
        if candidates.is_empty() {
            candidates.push_native(direct_ref);
        }
        candidates.finish()
    }

    fn collection_slots(
        representative_albums: &[AlbumArtwork],
        direct_ref: Option<&ImageRef>,
        prefer_direct: bool,
    ) -> Vec<Self> {
        if prefer_direct && direct_ref.is_some() {
            return vec![Self::from_native(direct_ref)];
        }
        let slots = Self::unique_representatives(representative_albums);
        if slots.is_empty() {
            return direct_ref
                .map(|image_ref| vec![Self::from_native(Some(image_ref))])
                .unwrap_or_default();
        }
        if slots.len() == 1 {
            return slots;
        }
        (0..COLLECTION_SLOT_LIMIT)
            .map(|index| slots[index % slots.len()].clone())
            .collect()
    }

    fn unique_representatives(representative_albums: &[AlbumArtwork]) -> Vec<Self> {
        let mut seen = HashSet::new();
        representative_albums
            .iter()
            .filter_map(|album| {
                let candidates = Self::album_artwork(album);
                let identity = candidates.candidates.first()?.stable_identity();
                seen.insert(identity).then_some(candidates)
            })
            .take(COLLECTION_SLOT_LIMIT)
            .collect()
    }

    pub(crate) fn from_native(image_ref: Option<&ImageRef>) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_native(image_ref);
        candidates.finish()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub(crate) fn has_external(&self) -> bool {
        self.candidates.iter().any(Candidate::is_external)
    }

    pub(crate) fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    pub(crate) fn stable_identity(&self) -> &str {
        &self.stable_identity
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtworkPresentation {
    primary: ArtworkBinding,
    slots: Arc<[ArtworkBinding]>,
}

impl ArtworkPresentation {
    pub fn track(track: &Track) -> Self {
        Self::single(ArtworkBinding::track(track))
    }

    pub fn album(album: &Album) -> Self {
        Self::single(ArtworkBinding::album(album))
    }

    pub fn artist(artist: &Artist) -> Self {
        Self::single(ArtworkBinding::artist(artist))
    }

    pub fn genre(genre: &Genre) -> Self {
        Self::collection(
            ArtworkBinding::genre(genre),
            ArtworkBinding::genre_slots(genre),
        )
    }

    pub fn mood(mood: &Mood) -> Self {
        Self::collection(ArtworkBinding::mood(mood), ArtworkBinding::mood_slots(mood))
    }

    pub fn playlist(playlist: &Playlist, prefer_server_cover: bool) -> Self {
        Self::collection(
            ArtworkBinding::playlist(playlist, prefer_server_cover),
            ArtworkBinding::playlist_slots(playlist, prefer_server_cover),
        )
    }

    pub fn smart_playlist(playlist: &SmartPlaylist) -> Self {
        Self::collection(
            ArtworkBinding::smart_playlist(playlist),
            ArtworkBinding::smart_playlist_slots(playlist),
        )
    }

    pub fn single(primary: ArtworkBinding) -> Self {
        Self {
            primary,
            slots: Arc::new([]),
        }
    }

    pub fn collection(primary: ArtworkBinding, slots: Vec<ArtworkBinding>) -> Self {
        Self {
            primary,
            slots: slots.into(),
        }
    }

    pub fn primary(&self) -> &ArtworkBinding {
        &self.primary
    }

    pub fn slots(&self) -> &[ArtworkBinding] {
        &self.slots
    }
}

#[derive(Default)]
struct CandidateBuilder {
    candidates: Vec<Candidate>,
}

impl CandidateBuilder {
    fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn finish(self) -> ArtworkBinding {
        let mut stable_identity = String::new();
        for candidate in &self.candidates {
            if !stable_identity.is_empty() {
                stable_identity.push('\u{1e}');
            }
            stable_identity.push_str(&candidate.stable_identity());
        }
        ArtworkBinding {
            candidates: self.candidates.into(),
            stable_identity: stable_identity.into(),
        }
    }

    fn push_representative_native(&mut self, albums: &[AlbumArtwork]) {
        for album in albums.iter().take(COLLECTION_SLOT_LIMIT) {
            self.push_native(album.image_ref.as_ref());
        }
    }

    fn push_representative_external(&mut self, albums: &[AlbumArtwork]) {
        for album in albums.iter().take(COLLECTION_SLOT_LIMIT) {
            self.push_release_group(album.musicbrainz_release_group_id.as_deref());
            self.push_release(album.musicbrainz_album_id.as_deref());
            self.push_album_text(&album.artist, &album.title);
        }
    }

    fn push_album_artwork(&mut self, album: &AlbumArtwork) {
        self.push_album(
            album.image_ref.as_ref(),
            album.musicbrainz_release_group_id.as_deref(),
            album.musicbrainz_album_id.as_deref(),
            &album.artist,
            &album.title,
        );
    }

    fn push_album(
        &mut self,
        image_ref: Option<&ImageRef>,
        release_group_id: Option<&str>,
        release_id: Option<&str>,
        artist: &str,
        album: &str,
    ) {
        self.push_native(image_ref);
        self.push_release_group(release_group_id);
        self.push_release(release_id);
        self.push_album_text(artist, album);
    }

    fn push_native(&mut self, image_ref: Option<&ImageRef>) {
        if let Some(image_ref) = image_ref {
            self.push(Candidate::Native(image_ref.clone()));
        }
    }

    fn push_release_group(&mut self, id: Option<&str>) {
        if let Some(id) = usable_text(id).filter(|id| valid_mbid(id)) {
            self.push(Candidate::MusicBrainzReleaseGroup(id.to_string()));
        }
    }

    fn push_release(&mut self, id: Option<&str>) {
        if let Some(id) = usable_text(id).filter(|id| valid_mbid(id)) {
            self.push(Candidate::MusicBrainzRelease(id.to_string()));
        }
    }

    fn push_album_text(&mut self, artist: &str, album: &str) {
        if let (Some(artist), Some(album)) = (lookup_text(artist), lookup_text(album)) {
            self.push(Candidate::AlbumText {
                artist: artist.to_string(),
                album: album.to_string(),
            });
        }
    }

    fn push(&mut self, candidate: Candidate) {
        if !self.candidates.contains(&candidate) {
            self.candidates.push(candidate);
        }
    }
}

impl fmt::Display for ArtworkBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.stable_identity())
    }
}

fn usable_text(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn lookup_text(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty()
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "unknown" | "unknown album" | "unknown artist" | "untitled album" | "untitled track"
        )
    {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn valid_mbid(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::{AlbumId, ArtistId, GenreId, PlaylistId, SourceFeatureOwner, TrackId};

    fn image_ref(id: &str) -> ImageRef {
        ImageRef::new(id, None)
    }

    fn album_artwork(id: &str, image_ref: Option<ImageRef>) -> AlbumArtwork {
        AlbumArtwork {
            id: AlbumId::new(id),
            title: format!("Album {id}"),
            artist: format!("Artist {id}"),
            image_ref,
            musicbrainz_album_id: Some(format!("release-{id}")),
            musicbrainz_release_group_id: Some(format!("group-{id}")),
        }
    }

    fn track() -> Track {
        Track {
            id: TrackId::new("track-one"),
            album_id: AlbumId::new("album-one"),
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 0,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 0,
            favorite: false,
            disc_number: 0,
            track_number: 0,
            image_ref: Some(image_ref("track-native")),
            album_artwork: Some(AlbumArtwork {
                id: AlbumId::new("album-one"),
                title: "Album".to_string(),
                artist: "Album Artist".to_string(),
                image_ref: Some(image_ref("album-native")),
                musicbrainz_album_id: Some("release-one".to_string()),
                musicbrainz_release_group_id: Some("group-one".to_string()),
            }),
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        }
    }

    #[test]
    fn track_candidates_keep_the_effective_album_order() {
        let candidates = ArtworkBinding::track(&track());

        assert_eq!(
            candidates.candidates.as_ref(),
            &[
                Candidate::Native(image_ref("track-native")),
                Candidate::Native(image_ref("album-native")),
                Candidate::MusicBrainzReleaseGroup("group-one".to_string()),
                Candidate::MusicBrainzRelease("release-one".to_string()),
                Candidate::AlbumText {
                    artist: "Album Artist".to_string(),
                    album: "Album".to_string(),
                },
            ]
        );
    }

    #[test]
    fn artist_tries_source_art_then_ordered_representative_album_facts() {
        let artist = Artist {
            id: ArtistId::new("artist-one"),
            name: "Artist".to_string(),
            album_count: 2,
            track_count: 2,
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
            musicbrainz_artist_id: None,
            image_ref: Some(image_ref("artist-native")),
            representative_albums: vec![
                album_artwork("one", None),
                album_artwork("two", Some(image_ref("album-two-native"))),
            ],
        };

        let candidates = ArtworkBinding::artist(&artist);

        assert_eq!(
            candidates.candidates.first(),
            Some(&Candidate::Native(image_ref("artist-native")))
        );
        assert_eq!(
            candidates.candidates.get(1),
            Some(&Candidate::Native(image_ref("album-two-native")))
        );
        assert_eq!(
            candidates.candidates.get(2),
            Some(&Candidate::MusicBrainzReleaseGroup("group-one".to_string()))
        );
    }

    #[test]
    fn genre_slots_fill_a_four_cell_tile_with_full_album_fallbacks() {
        let genre = Genre {
            id: GenreId::new("genre-one"),
            name: "Genre".to_string(),
            album_count: 2,
            track_count: 2,
            duration_seconds: 0,
            image_ref: Some(image_ref("genre-native")),
            representative_albums: vec![
                album_artwork("one", None),
                album_artwork("two", Some(image_ref("album-two-native"))),
            ],
        };

        let slots = ArtworkBinding::genre_slots(&genre);

        assert_eq!(slots.len(), 4);
        assert_eq!(slots[0], slots[2]);
        assert_eq!(slots[1], slots[3]);
        assert!(slots[0].has_external());
        assert_eq!(
            slots[1].candidates.first(),
            Some(&Candidate::Native(image_ref("album-two-native")))
        );
    }

    #[test]
    fn preferred_playlist_cover_stays_a_single_source_candidate() {
        let playlist = Playlist {
            id: PlaylistId::new("playlist-one"),
            name: "Playlist".to_string(),
            owner: Some(SourceFeatureOwner::Native),
            track_count: 2,
            duration_seconds: 0,
            top_genres: Vec::new(),
            image_ref: Some(image_ref("server-playlist-cover")),
            representative_albums: vec![album_artwork("one", None)],
        };

        let slots = ArtworkBinding::playlist_slots(&playlist, true);

        assert_eq!(
            slots,
            vec![ArtworkBinding::from_native(Some(&image_ref(
                "server-playlist-cover"
            )))]
        );
    }

    #[test]
    fn collection_uses_representatives_before_an_unpreferred_server_cover() {
        let playlist = Playlist {
            id: PlaylistId::new("playlist-one"),
            name: "Playlist".to_string(),
            owner: Some(SourceFeatureOwner::Native),
            track_count: 2,
            duration_seconds: 0,
            top_genres: Vec::new(),
            image_ref: Some(image_ref("server-playlist-cover")),
            representative_albums: vec![
                album_artwork("one", None),
                album_artwork("two", Some(image_ref("album-two-native"))),
            ],
        };

        let candidates = ArtworkBinding::playlist(&playlist, false);

        assert_eq!(
            &candidates.candidates[..4],
            &[
                Candidate::MusicBrainzReleaseGroup("group-one".to_string()),
                Candidate::MusicBrainzRelease("release-one".to_string()),
                Candidate::AlbumText {
                    artist: "Artist one".to_string(),
                    album: "Album one".to_string(),
                },
                Candidate::Native(image_ref("album-two-native")),
            ]
        );
        assert!(
            !candidates
                .candidates
                .contains(&Candidate::Native(image_ref("server-playlist-cover")))
        );
    }

    #[test]
    fn collection_slots_skip_duplicate_first_choices_before_taking_four() {
        let shared = image_ref("shared-native");
        let genre = Genre {
            id: GenreId::new("genre-one"),
            name: "Genre".to_string(),
            album_count: 5,
            track_count: 5,
            duration_seconds: 0,
            image_ref: None,
            representative_albums: vec![
                album_artwork("one", Some(shared.clone())),
                album_artwork("two", Some(shared)),
                album_artwork("three", Some(image_ref("three-native"))),
                album_artwork("four", Some(image_ref("four-native"))),
                album_artwork("five", Some(image_ref("five-native"))),
            ],
        };

        let slots = ArtworkBinding::genre_slots(&genre);

        assert_eq!(slots.len(), 4);
        assert_eq!(
            slots
                .iter()
                .filter_map(|slot| slot.candidates.first())
                .collect::<Vec<_>>(),
            vec![
                &Candidate::Native(image_ref("shared-native")),
                &Candidate::Native(image_ref("three-native")),
                &Candidate::Native(image_ref("four-native")),
                &Candidate::Native(image_ref("five-native")),
            ]
        );
    }

    #[test]
    fn unusable_identity_and_placeholder_text_do_not_create_external_work() {
        let artwork = AlbumArtwork {
            id: AlbumId::new("unknown"),
            title: "Unknown Album".to_string(),
            artist: "Unknown Artist".to_string(),
            image_ref: None,
            musicbrainz_album_id: Some("not an mbid".to_string()),
            musicbrainz_release_group_id: Some("/invalid/".to_string()),
        };

        assert!(ArtworkBinding::album_artwork(&artwork).is_empty());
    }
}
