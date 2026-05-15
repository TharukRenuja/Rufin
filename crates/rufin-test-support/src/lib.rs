use rufin_core::{Album, AlbumId, Track, TrackId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FakeScale {
    Small,
    Large,
}

impl FakeScale {
    pub fn album_count(self) -> usize {
        match self {
            Self::Small => 240,
            Self::Large => 20_000,
        }
    }

    pub fn track_count(self) -> usize {
        match self {
            Self::Small => 2_400,
            Self::Large => 100_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FakeLibrary {
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

pub fn generate_fake_library(scale: FakeScale) -> FakeLibrary {
    let album_count = scale.album_count();
    let track_count = scale.track_count();
    let mut album_track_counts = vec![0_u16; album_count];

    for index in 0..track_count {
        album_track_counts[index % album_count] += 1;
    }

    let albums = (0..album_count)
        .map(|index| fake_album(index, album_track_counts[index]))
        .collect::<Vec<_>>();

    let mut album_positions = vec![0_u16; album_count];
    let tracks = (0..track_count)
        .map(|index| {
            let album_index = index % album_count;
            album_positions[album_index] += 1;
            fake_track(index, &albums[album_index], album_positions[album_index])
        })
        .collect::<Vec<_>>();

    FakeLibrary { albums, tracks }
}

fn fake_album(index: usize, track_count: u16) -> Album {
    let title_word = ALBUM_WORDS[index % ALBUM_WORDS.len()];
    let title_tail = ALBUM_TAILS[(index / ALBUM_WORDS.len()) % ALBUM_TAILS.len()];
    let artist = ARTISTS[index % ARTISTS.len()];
    let year = 1980 + (index % 46) as u16;
    let duration_seconds = u32::from(track_count) * (175 + (index % 110) as u32);

    Album {
        id: AlbumId(index as u32 + 1),
        title: format!("{title_word} {title_tail} {}", index + 1),
        artist: artist.to_string(),
        year,
        track_count,
        duration_seconds,
        favorite: index.is_multiple_of(17),
        color_seed: color_seed(index as u32),
    }
}

fn fake_track(index: usize, album: &Album, track_number: u16) -> Track {
    let title_word = TRACK_WORDS[index % TRACK_WORDS.len()];
    let title_tail = TRACK_TAILS[(index / TRACK_WORDS.len()) % TRACK_TAILS.len()];

    Track {
        id: TrackId(index as u32 + 1),
        album_id: album.id,
        title: format!("{title_word} {title_tail} {track_number}"),
        artist: album.artist.clone(),
        album: album.title.clone(),
        year: album.year,
        duration_seconds: 145 + (index % 210) as u32,
        favorite: index.is_multiple_of(23),
        disc_number: 1,
        track_number,
    }
}

fn color_seed(value: u32) -> u32 {
    value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

const ALBUM_WORDS: &[&str] = &[
    "Blue", "Glass", "Signal", "Late", "Neon", "Velvet", "Static", "Golden", "Silver", "Hidden",
    "Open", "Electric",
];

const ALBUM_TAILS: &[&str] = &[
    "Rooms", "Weather", "Letters", "Harbors", "Fields", "Cities", "Mirrors", "Tides", "Maps",
    "Windows", "Gardens", "Stations",
];

const TRACK_WORDS: &[&str] = &[
    "First", "Quiet", "Long", "Night", "Soft", "Bright", "Heavy", "North", "South", "Broken",
    "Clear", "Slow",
];

const TRACK_TAILS: &[&str] = &[
    "Motion", "Pattern", "Street", "Answer", "Signal", "Memory", "Promise", "Line", "Frame",
    "Turn", "Light", "Wake",
];

const ARTISTS: &[&str] = &[
    "Astral Kin",
    "Blue Hour",
    "City Archive",
    "Distant Rooms",
    "Glass Method",
    "North Index",
    "Parallel Echo",
    "Signal Park",
    "Soft Circuit",
    "Velvet Relay",
];

#[cfg(test)]
mod tests {
    use super::{FakeScale, generate_fake_library};

    #[test]
    fn small_dataset_has_expected_shape() {
        let library = generate_fake_library(FakeScale::Small);

        assert_eq!(library.albums.len(), 240);
        assert_eq!(library.tracks.len(), 2_400);
        assert_eq!(library.albums[0].track_count, 10);
        assert_eq!(library.tracks[0].album_id, library.albums[0].id);
    }

    #[test]
    fn large_dataset_matches_m0_targets() {
        let library = generate_fake_library(FakeScale::Large);

        assert_eq!(library.albums.len(), 20_000);
        assert_eq!(library.tracks.len(), 100_000);
        let generated_track_count = library
            .albums
            .iter()
            .map(|album| album.track_count as usize)
            .sum::<usize>();

        assert_eq!(generated_track_count, 100_000);
    }
}
