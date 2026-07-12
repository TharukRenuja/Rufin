use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Track, TrackId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalManifestEntry {
    pub facts: LocalFileFacts,
    pub track: Track,
    pub album_artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_album_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_release_group_id: Option<String>,
    pub cover: Option<LocalManifestCover>,
    pub metadata_hash: String,
    pub search_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalCueTrackSource {
    pub source_object_id: String,
    pub track_id: TrackId,
    pub source_path: String,
    pub root_path: String,
    pub relative_path: String,
    pub cue_path: String,
    pub cue_revision: String,
    pub cue_track_index: i64,
    pub segment_start_ms: i64,
    pub segment_end_ms: i64,
    pub sync_generation: i64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LocalCueDependency {
    pub cue_path: PathBuf,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct LocalFileFacts {
    pub path: PathBuf,
    pub root_path: PathBuf,
    pub relative_path: String,
    pub file_size: u64,
    pub mtime_seconds: i64,
    pub mtime_nanos: u32,
    pub inode: Option<u64>,
    pub device: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct LocalManifestCover {
    pub item_id: String,
    pub kind: LocalManifestCoverKind,
    pub source_path: PathBuf,
    pub revision: String,
    pub embedded_index: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum LocalManifestCoverKind {
    File,
    Embedded,
}
