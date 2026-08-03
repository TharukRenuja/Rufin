use library::{GenreId, RadioSeed, TrackId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LyricsSearch {
    ServerOnly,
    ServerThenRemote,
    RemoteThenServer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeLyricsOrigin {
    Server,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricLine {
    pub text: String,
    pub start_millis: Option<u64>,
    pub end_millis: Option<u64>,
    pub cue_lines: Vec<NativeLyricCueLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricCueLine {
    pub text: String,
    pub start_millis: Option<u64>,
    pub end_millis: Option<u64>,
    pub agent_id: Option<String>,
    pub cues: Vec<NativeLyricCue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricCue {
    pub text: String,
    pub start_millis: u64,
    pub end_millis: Option<u64>,
    pub byte_start: usize,
    pub byte_end_exclusive: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeLyricsRole {
    Original,
    Translation,
    Pronunciation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeLyricAgentRole {
    Main,
    Voice,
    Background,
    Group,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricAgent {
    pub id: String,
    pub role: NativeLyricAgentRole,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricsDocument {
    pub role: NativeLyricsRole,
    pub language: Option<String>,
    pub offset_millis: i64,
    pub lines: Vec<NativeLyricLine>,
    pub agents: Vec<NativeLyricAgent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyrics {
    pub origin: NativeLyricsOrigin,
    pub documents: Vec<NativeLyricsDocument>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlayedFilter {
    #[default]
    All,
    Unplayed,
    Played,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RandomTrackRequest {
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre_id: Option<GenreId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre_name: Option<String>,
    pub played_filter: PlayedFilter,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GeneratedTracksRequest {
    pub seed: RadioSeed,
    pub limit: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlaybackReportKind {
    Started,
    Progress,
    QualifiedPlay,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaybackReport {
    pub kind: PlaybackReportKind,
    pub track_id: TrackId,
    pub started_at_unix_seconds: i64,
    pub position_seconds: u32,
    pub paused: bool,
    pub muted: bool,
    pub volume_percent: u8,
    pub shuffle: bool,
    pub repeat_one: bool,
    pub repeat_all: bool,
    pub failed: bool,
}
