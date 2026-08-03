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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}
