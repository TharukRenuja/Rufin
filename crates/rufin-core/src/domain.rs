use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AlbumId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TrackId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtistId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GenreId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlaylistId(pub u32);

impl fmt::Display for AlbumId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "album-{}", self.0)
    }
}

impl fmt::Display for TrackId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "track-{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Album {
    pub id: AlbumId,
    pub title: String,
    pub artist: String,
    pub year: u16,
    pub track_count: u16,
    pub duration_seconds: u32,
    pub favorite: bool,
    pub color_seed: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Track {
    pub id: TrackId,
    pub album_id: AlbumId,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: u16,
    pub duration_seconds: u32,
    pub favorite: bool,
    pub disc_number: u16,
    pub track_number: u16,
}

pub fn format_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn formats_track_duration() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(185), "3:05");
        assert_eq!(format_duration(3_661), "61:01");
    }
}
