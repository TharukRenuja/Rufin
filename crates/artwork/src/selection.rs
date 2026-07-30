use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use library::{
    Album, AlbumArtwork, Artist, Genre, ImageRef, LocalArtworkRef, Mood, Playlist, SmartPlaylist,
    SourceArtwork, Track,
};

const COLLECTION_SLOT_LIMIT: usize = 4;
const REPRESENTATIVE_ALBUM_LIMIT: usize = 16;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Candidate {
    Native(ImageRef),
    Local(LocalArtworkRef),
    Album(album_lookup::AlbumCover),
}

impl Candidate {
    pub(crate) fn stable_identity(&self) -> String {
        match self {
            Self::Native(image_ref) => format!(
                "native\0{}\0{}",
                image_ref.item_id,
                image_ref.tag.as_deref().unwrap_or_default()
            ),
            Self::Local(LocalArtworkRef::File { path, revision }) => {
                format!("local-file\0{path}\0{revision}")
            }
            Self::Local(LocalArtworkRef::Embedded {
                path,
                picture_index,
                revision,
            }) => format!("local-embedded\0{path}\0{picture_index}\0{revision}"),
            Self::Album(album) => album.stable_identity(),
        }
    }

    pub const fn is_external(&self) -> bool {
        matches!(self, Self::Album(_))
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
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
        candidates.push_album_facts(
            album.local_artwork.as_ref(),
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
        if let Some(album) = track.album_artwork_facts() {
            candidates.push_local(album.local_artwork.as_ref());
            candidates.push_native(album.image_ref.as_ref());
            candidates.push_local(track.local_artwork.as_ref());
            candidates.push_native(track.image_ref.as_ref());
            candidates.push_album_lookup(
                &album.artist,
                &album.title,
                album.musicbrainz_release_group_id.as_deref(),
                album.musicbrainz_album_id.as_deref(),
            );
        } else {
            candidates.push_local(track.local_artwork.as_ref());
            candidates.push_native(track.image_ref.as_ref());
            candidates.push_album_lookup(&track.artist, &track.album, None, None);
        }
        candidates.finish()
    }

    pub fn artist(artist: &Artist, representative_albums: &[AlbumArtwork]) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_local(artist.local_artwork.as_ref());
        candidates.push_native(artist.image_ref.as_ref());
        candidates.push_representative_native(representative_albums);
        candidates.push_representative_external(representative_albums);
        candidates.finish()
    }

    pub fn genre(genre: &Genre, representative_albums: &[AlbumArtwork]) -> Self {
        Self::collection(representative_albums, genre.image_ref.as_ref(), false)
    }

    pub fn genre_slots(genre: &Genre, representative_albums: &[AlbumArtwork]) -> Vec<Self> {
        Self::collection_slots(representative_albums, genre.image_ref.as_ref(), false)
    }

    pub fn mood(_mood: &Mood, representative_albums: &[AlbumArtwork]) -> Self {
        Self::collection(representative_albums, None, false)
    }

    pub fn mood_slots(_mood: &Mood, representative_albums: &[AlbumArtwork]) -> Vec<Self> {
        Self::collection_slots(representative_albums, None, false)
    }

    pub fn playlist(
        playlist: &Playlist,
        representative_albums: &[AlbumArtwork],
        prefer_server_cover: bool,
    ) -> Self {
        Self::collection(
            representative_albums,
            playlist.image_ref.as_ref(),
            prefer_server_cover,
        )
    }

    pub fn playlist_slots(
        playlist: &Playlist,
        representative_albums: &[AlbumArtwork],
        prefer_server_cover: bool,
    ) -> Vec<Self> {
        Self::collection_slots(
            representative_albums,
            playlist.image_ref.as_ref(),
            prefer_server_cover,
        )
    }

    pub fn smart_playlist(
        _playlist: &SmartPlaylist,
        representative_albums: &[AlbumArtwork],
    ) -> Self {
        Self::collection(representative_albums, None, false)
    }

    pub fn smart_playlist_slots(
        _playlist: &SmartPlaylist,
        representative_albums: &[AlbumArtwork],
    ) -> Vec<Self> {
        Self::collection_slots(representative_albums, None, false)
    }

    pub fn album_text(artist: &str, album: &str) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_album_lookup(artist, album, None, None);
        candidates.finish()
    }

    pub fn album_facts(
        artist: &str,
        album: &str,
        musicbrainz_release_group_id: Option<&str>,
        musicbrainz_album_id: Option<&str>,
    ) -> Self {
        let mut candidates = CandidateBuilder::default();
        candidates.push_album_facts(
            None,
            None,
            musicbrainz_release_group_id,
            musicbrainz_album_id,
            artist,
            album,
        );
        candidates.finish()
    }

    pub fn source_artwork(artwork: &SourceArtwork) -> Self {
        let mut candidates = CandidateBuilder::default();
        match artwork {
            SourceArtwork::Native(image_ref) => candidates.push_native(Some(image_ref)),
            SourceArtwork::Local(local_artwork) => candidates.push_local(Some(local_artwork)),
        }
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
        for artwork in albums.iter().take(REPRESENTATIVE_ALBUM_LIMIT) {
            if let Some(track) = &artwork.representative_track {
                self.push_local(track.local_artwork.as_ref());
                self.push_native(track.image_ref.as_ref());
            }
            self.push_local(artwork.album.local_artwork.as_ref());
            self.push_native(artwork.album.image_ref.as_ref());
        }
    }

    fn push_representative_external(&mut self, albums: &[AlbumArtwork]) {
        for artwork in albums.iter().take(REPRESENTATIVE_ALBUM_LIMIT) {
            let album = &artwork.album;
            self.push_album_lookup(
                &album.artist,
                &album.title,
                album.musicbrainz_release_group_id.as_deref(),
                album.musicbrainz_album_id.as_deref(),
            );
        }
    }

    fn push_album_artwork(&mut self, album: &AlbumArtwork) {
        self.push_local(album.album.local_artwork.as_ref());
        self.push_native(album.album.image_ref.as_ref());
        if let Some(track) = &album.representative_track {
            self.push_local(track.local_artwork.as_ref());
            self.push_native(track.image_ref.as_ref());
        }
        self.push_album_lookup(
            &album.album.artist,
            &album.album.title,
            album.album.musicbrainz_release_group_id.as_deref(),
            album.album.musicbrainz_album_id.as_deref(),
        );
    }

    fn push_album_facts(
        &mut self,
        local_artwork: Option<&LocalArtworkRef>,
        image_ref: Option<&ImageRef>,
        release_group_id: Option<&str>,
        release_id: Option<&str>,
        artist: &str,
        album: &str,
    ) {
        self.push_local(local_artwork);
        self.push_native(image_ref);
        self.push_album_lookup(artist, album, release_group_id, release_id);
    }

    fn push_native(&mut self, image_ref: Option<&ImageRef>) {
        if let Some(image_ref) = image_ref {
            self.push(Candidate::Native(image_ref.clone()));
        }
    }

    fn push_local(&mut self, local_artwork: Option<&LocalArtworkRef>) {
        if let Some(local_artwork) = local_artwork {
            self.push(Candidate::Local(local_artwork.clone()));
        }
    }

    fn push_album_lookup(
        &mut self,
        artist: &str,
        album: &str,
        release_group_id: Option<&str>,
        release_id: Option<&str>,
    ) {
        if let Some(album) =
            album_lookup::AlbumCover::new(artist, album, release_group_id, release_id)
        {
            self.push(Candidate::Album(album));
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

#[cfg(test)]
mod tests {
    use super::*;
    use library::{
        AlbumArtworkFacts, AlbumId, ArtistId, GenreId, PlaylistId, Track, TrackId, TrackRelations,
    };

    fn image_ref(id: &str) -> ImageRef {
        ImageRef::new(id, None)
    }

    fn album_candidate(
        artist: &str,
        album: &str,
        release_group_id: Option<&str>,
        release_id: Option<&str>,
    ) -> Candidate {
        Candidate::Album(
            album_lookup::AlbumCover::new(artist, album, release_group_id, release_id)
                .expect("album cover candidate"),
        )
    }

    fn album_artwork(id: &str, image_ref: Option<ImageRef>) -> AlbumArtwork {
        AlbumArtwork {
            album: Arc::new(Album {
                id: AlbumId::new(id),
                title: format!("Album {id}"),
                artist: format!("Artist {id}"),
                year: 0,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                favorite: false,
                color_seed: 0,
                image_ref,
                local_artwork: None,
                release_types: Vec::new(),
                is_compilation: None,
                musicbrainz_album_id: Some(format!("release-{id}")),
                musicbrainz_release_group_id: Some(format!("group-{id}")),
                relations: library::AlbumRelations::default(),
            }),
            representative_track: None,
        }
    }

    fn track() -> Track {
        Track::new(library::TrackData {
            id: TrackId::new("track-one"),
            album_id: Some(AlbumId::new("album-one")),
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            album_artwork: None,
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
            local_artwork: None,
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            source_path: None,
            cue: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            relations: TrackRelations::default(),
        })
    }

    #[test]
    fn track_candidates_keep_album_art_before_track_fallback() {
        let album = Arc::new(Album {
            id: AlbumId::new("album-one"),
            title: "Album".to_string(),
            artist: "Album Artist".to_string(),
            year: 0,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            favorite: false,
            color_seed: 0,
            image_ref: Some(image_ref("album-native")),
            local_artwork: None,
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: Some("release-one".to_string()),
            musicbrainz_release_group_id: Some("group-one".to_string()),
            relations: library::AlbumRelations::default(),
        });
        let mut track = track();
        track.album_artwork = Some(Arc::new(AlbumArtworkFacts::from(album.as_ref())));
        let candidates = ArtworkBinding::track(&track);

        assert_eq!(
            candidates.candidates.as_ref(),
            &[
                Candidate::Native(image_ref("album-native")),
                Candidate::Native(image_ref("track-native")),
                album_candidate(
                    "Album Artist",
                    "Album",
                    Some("group-one"),
                    Some("release-one"),
                ),
            ]
        );
    }

    #[test]
    fn artist_tries_source_art_then_ordered_representative_album_facts() {
        let artist = Artist {
            id: ArtistId::new("artist-one"),
            name: "Artist".to_string(),
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
            musicbrainz_artist_id: None,
            image_ref: Some(image_ref("artist-native")),
            local_artwork: None,
        };
        let representatives = [
            album_artwork("one", None),
            album_artwork("two", Some(image_ref("album-two-native"))),
        ];

        let candidates = ArtworkBinding::artist(&artist, &representatives);

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
            Some(&album_candidate(
                "Artist one",
                "Album one",
                Some("group-one"),
                Some("release-one"),
            ))
        );
    }

    #[test]
    fn genre_slots_fill_a_four_cell_tile_with_full_album_fallbacks() {
        let genre = Genre {
            id: GenreId::new("genre-one"),
            name: "Genre".to_string(),
            image_ref: Some(image_ref("genre-native")),
        };
        let representatives = [
            album_artwork("one", None),
            album_artwork("two", Some(image_ref("album-two-native"))),
        ];

        let slots = ArtworkBinding::genre_slots(&genre, &representatives);

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
            image_ref: Some(image_ref("server-playlist-cover")),
        };
        let representatives = [album_artwork("one", None)];

        let slots = ArtworkBinding::playlist_slots(&playlist, &representatives, true);

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
            image_ref: Some(image_ref("server-playlist-cover")),
        };
        let representatives = [
            album_artwork("one", None),
            album_artwork("two", Some(image_ref("album-two-native"))),
        ];

        let candidates = ArtworkBinding::playlist(&playlist, &representatives, false);

        assert_eq!(
            candidates.candidates.as_ref(),
            &[
                album_candidate(
                    "Artist one",
                    "Album one",
                    Some("group-one"),
                    Some("release-one"),
                ),
                Candidate::Native(image_ref("album-two-native")),
                album_candidate(
                    "Artist two",
                    "Album two",
                    Some("group-two"),
                    Some("release-two"),
                ),
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
            image_ref: None,
        };
        let representatives = [
            album_artwork("one", Some(shared.clone())),
            album_artwork("two", Some(shared)),
            album_artwork("three", Some(image_ref("three-native"))),
            album_artwork("four", Some(image_ref("four-native"))),
            album_artwork("five", Some(image_ref("five-native"))),
        ];

        let slots = ArtworkBinding::genre_slots(&genre, &representatives);

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
            album: Arc::new(Album {
                id: AlbumId::new("unknown"),
                title: "Unknown Album".to_string(),
                artist: "Unknown Artist".to_string(),
                year: 0,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                favorite: false,
                color_seed: 0,
                image_ref: None,
                local_artwork: None,
                release_types: Vec::new(),
                is_compilation: None,
                musicbrainz_album_id: Some("not an mbid".to_string()),
                musicbrainz_release_group_id: Some("/invalid/".to_string()),
                relations: library::AlbumRelations::default(),
            }),
            representative_track: None,
        };

        assert!(ArtworkBinding::album_artwork(&artwork).is_empty());
    }
}
