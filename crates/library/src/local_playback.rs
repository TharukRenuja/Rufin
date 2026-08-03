//! Accepted local files used for playback and lyrics sidecars.
//!
//! Filesystem scanning stays in Sources. Library accepts one bounded access
//! scan, builds exact match indexes once, and answers playback without Store
//! reads or source reconstruction.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    Libraries, Library, LibraryError, LibraryResult, LocalFile, LocalFileKind, LocalReadState,
    MetadataError, MetadataItem, MetadataItemId, MetadataSubject, SourceId, Track, TrackId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalAccessFile {
    pub path: String,
    pub root: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub device_id: Option<u64>,
    pub inode: Option<u64>,
    pub parser_version: u32,
    pub title: String,
    pub album: String,
    pub artist: String,
    pub disc_number: u16,
    pub track_number: u16,
    pub duration_seconds: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalAccessMapping {
    pub root_path: PathBuf,
    pub server_prefix: Option<String>,
    pub local_prefix: Option<String>,
}

/// One exact file projected through the selected source's local-access mapping.
///
/// The file-backed source still canonicalizes this path, checks that it stays
/// inside the selected root, and confirms writer support before reading or
/// mutating it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalAccessTarget {
    root_path: PathBuf,
    path: PathBuf,
}

impl LocalAccessTarget {
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalAccessStatus {
    pub sample_source_path: Option<String>,
    pub sample_local_path: Option<String>,
    pub direct_match_count: usize,
    pub prefix_match_count: usize,
    pub metadata_match_count: usize,
    pub unmatched_count: usize,
    pub total_track_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayableFile {
    File {
        path: PathBuf,
    },
    Cue {
        path: PathBuf,
        start_millis: u64,
        end_millis: u64,
    },
}

impl PlayableFile {
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::File { path } | Self::Cue { path, .. } => path,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidecarAudioFile {
    pub path: PathBuf,
    pub cue_track: bool,
}

#[derive(Debug, Hash, Eq, PartialEq)]
pub(crate) struct LocalMatchKey {
    title: String,
    album: String,
    artist: String,
    disc_number: u16,
    track_number: u16,
}

impl Library {
    pub fn replace_local_access(
        &self,
        mapping: LocalAccessMapping,
        mut files: Vec<LocalAccessFile>,
    ) -> LibraryResult<LocalAccessStatus> {
        if self.source_id().as_str().is_empty() {
            return Err(LibraryError::Persistence(
                "Local access requires a source".to_string(),
            ));
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        self.store
            .replace_local_access(self.source_id().clone(), files.clone())?;
        self.replace_local_access_state(Some(mapping), files)?;
        self.local_access_status().map_err(Into::into)
    }

    pub fn configure_local_access(
        &self,
        mapping: LocalAccessMapping,
    ) -> LibraryResult<LocalAccessStatus> {
        self.configure_local_access_mapping(Some(mapping))?;
        self.local_access_status().map_err(Into::into)
    }

    pub fn configure_local_access_mapping(
        &self,
        mapping: Option<LocalAccessMapping>,
    ) -> LibraryResult<()> {
        self.configure_local_access_state(mapping)?;
        Ok(())
    }

    pub fn accept_local_access_mapping(&self, mapping: LocalAccessMapping) -> LibraryResult<()> {
        self.accept_local_access_mapping_state(mapping)?;
        Ok(())
    }

    pub fn clear_local_access(&self) -> LibraryResult<LocalAccessStatus> {
        self.store.clear_local_access(self.source_id().clone())?;
        self.replace_local_access_state(None, Vec::new())?;
        self.local_access_status().map_err(Into::into)
    }
}

impl Libraries {
    pub fn discard_local_access(&self, source_id: SourceId) -> LibraryResult<()> {
        self.store.clear_local_access(source_id)?;
        Ok(())
    }
}

impl Library {
    pub fn replace_downloaded_files(
        &self,
        mut files: HashMap<TrackId, PathBuf>,
    ) -> crate::LibraryQueryResult<Vec<TrackId>> {
        let mut state = self.write_state()?;
        let removed = files
            .keys()
            .filter(|track_id| state.tracks.get(*track_id).is_none())
            .cloned()
            .collect::<Vec<_>>();
        files.retain(|track_id, _| state.tracks.get(track_id).is_some());
        state.downloaded_files = files;
        let downloaded = state
            .downloaded_files
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        state.download_coverage.replace_downloaded(&downloaded);
        Ok(removed)
    }

    pub fn set_downloaded_file(
        &self,
        track_id: TrackId,
        path: PathBuf,
    ) -> crate::LibraryQueryResult<()> {
        let mut state = self.write_state()?;
        if state.tracks.get(&track_id).is_none() {
            return Err(crate::LibraryQueryError::MissingItem {
                kind: "Track",
                id: track_id.to_string(),
            });
        }
        let first_download = state
            .downloaded_files
            .insert(track_id.clone(), path)
            .is_none();
        if first_download {
            state.download_coverage.set_downloaded(&track_id, true);
        }
        Ok(())
    }

    pub fn remove_downloaded_file(
        &self,
        track_id: &TrackId,
    ) -> crate::LibraryQueryResult<Option<PathBuf>> {
        let mut state = self.write_state()?;
        let removed = state.downloaded_files.remove(track_id);
        if removed.is_some() {
            state.download_coverage.set_downloaded(track_id, false);
        }
        Ok(removed)
    }

    pub fn is_downloaded(&self, track_id: &TrackId) -> crate::LibraryQueryResult<bool> {
        Ok(self.read_state()?.downloaded_files.contains_key(track_id))
    }

    pub fn downloaded_track_ids(&self) -> crate::LibraryQueryResult<HashSet<TrackId>> {
        Ok(self
            .read_state()?
            .downloaded_files
            .keys()
            .cloned()
            .collect())
    }

    pub(crate) fn replace_local_access_state(
        &self,
        mapping: Option<LocalAccessMapping>,
        files: Vec<LocalAccessFile>,
    ) -> crate::LibraryQueryResult<()> {
        let mut state = self.write_state()?;
        let (local_access_paths, local_access_index) = index_local_access(&files, mapping.as_ref());
        state.local_access_mapping = mapping;
        state.local_access = files;
        state.local_access_paths = local_access_paths;
        state.local_access_index = local_access_index;
        Ok(())
    }

    pub(crate) fn configure_local_access_state(
        &self,
        mapping: Option<LocalAccessMapping>,
    ) -> crate::LibraryQueryResult<()> {
        let mut state = self.write_state()?;
        let (local_access_paths, local_access_index) =
            index_local_access(&state.local_access, mapping.as_ref());
        state.local_access_mapping = mapping;
        state.local_access_paths = local_access_paths;
        state.local_access_index = local_access_index;
        Ok(())
    }

    pub(crate) fn accept_local_access_mapping_state(
        &self,
        mapping: LocalAccessMapping,
    ) -> crate::LibraryQueryResult<()> {
        let mut state = self.write_state()?;
        let same_root = state
            .local_access_mapping
            .as_ref()
            .is_some_and(|accepted| accepted.root_path == mapping.root_path);
        state.local_access_mapping = Some(mapping);
        if !same_root {
            state.local_access_paths.clear();
            state.local_access_index.clear();
        }
        Ok(())
    }

    pub fn local_access_status(&self) -> crate::LibraryQueryResult<LocalAccessStatus> {
        let state = self.read_state()?;
        let mut status = LocalAccessStatus {
            total_track_count: state.tracks.len(),
            ..LocalAccessStatus::default()
        };
        for track in state.tracks.values() {
            let Some((file, kind)) = local_access_file_for(
                track,
                state.local_access_mapping.as_ref(),
                &state.local_access,
                &state.local_access_paths,
                &state.local_access_index,
            ) else {
                status.unmatched_count += 1;
                if status.sample_source_path.is_none() {
                    status.sample_source_path.clone_from(&track.source_path);
                }
                continue;
            };
            match kind {
                LocalAccessMatch::Direct => status.direct_match_count += 1,
                LocalAccessMatch::Prefix => status.prefix_match_count += 1,
                LocalAccessMatch::Metadata => status.metadata_match_count += 1,
            }
            if status.sample_source_path.is_none() {
                status.sample_source_path.clone_from(&track.source_path);
                status.sample_local_path = Some(file.path().to_string_lossy().into_owned());
            }
        }
        Ok(status)
    }

    pub fn local_access_files(&self) -> crate::LibraryQueryResult<Vec<LocalAccessFile>> {
        Ok(self.read_state()?.local_access.clone())
    }

    pub fn metadata_item(
        &self,
        item_id: &MetadataItemId,
    ) -> crate::LibraryQueryResult<Option<MetadataItem>> {
        match item_id {
            MetadataItemId::Track(id) => self.track(id).map(|item| item.map(MetadataItem::Track)),
            MetadataItemId::Album(id) => self
                .album(id)
                .map(|item| item.map(|album| MetadataItem::Album((*album).clone()))),
            MetadataItemId::Artist(id) => self
                .artist(id)
                .map(|item| item.map(|artist| MetadataItem::Artist((*artist).clone()))),
        }
    }

    pub fn metadata_subject(
        self: &Arc<Self>,
        item_id: &MetadataItemId,
    ) -> crate::LibraryQueryResult<Option<MetadataSubject>> {
        let Some(item) = self.metadata_item(item_id)? else {
            return Ok(None);
        };
        match (item_id, item) {
            (MetadataItemId::Track(_), MetadataItem::Track(track)) => {
                Ok(Some(MetadataSubject::track(track)))
            }
            (MetadataItemId::Album(id), MetadataItem::Album(album)) => {
                let tracks = self.album_track_selection(id, None);
                Ok(Some(MetadataSubject::aggregate(
                    MetadataItem::Album(album),
                    tracks,
                )))
            }
            (MetadataItemId::Artist(id), MetadataItem::Artist(artist)) => {
                let tracks = self.artist_track_selection(id, None);
                Ok(Some(MetadataSubject::aggregate(
                    MetadataItem::Artist(artist),
                    tracks,
                )))
            }
            _ => unreachable!("metadata item kind follows its ID"),
        }
    }

    pub fn metadata_subject_with_local_access(
        self: &Arc<Self>,
        item_id: &MetadataItemId,
        proposed: Option<&LocalAccessMapping>,
    ) -> Result<Option<(MetadataSubject, Vec<LocalAccessTarget>)>, MetadataError> {
        let Some(mut subject) = self
            .metadata_subject(item_id)
            .map_err(|error| MetadataError::Write(error.to_string()))?
        else {
            return Ok(None);
        };
        let tracks = match subject.item() {
            MetadataItem::Track(track) => vec![track.clone()],
            MetadataItem::Album(_) | MetadataItem::Artist(_) => {
                let prepared = subject
                    .tracks()
                    .cloned()
                    .expect("aggregate metadata has a Track selection")
                    .prepare()
                    .map_err(|error| MetadataError::Write(error.to_string()))?;
                if prepared.is_empty() {
                    return Err(MetadataError::LocalAccessRequired {
                        source_path: String::new(),
                    });
                }
                let tracks = prepared
                    .materialize_owned()
                    .map_err(|error| MetadataError::Write(error.to_string()))?;
                subject = MetadataSubject::aggregate(subject.into_item(), prepared.into());
                tracks
            }
        };
        let accepted_mapping = if proposed.is_none() {
            self.read_state()
                .map_err(|error| MetadataError::Write(error.to_string()))?
                .local_access_mapping
                .clone()
        } else {
            None
        };
        let mapping = proposed.or(accepted_mapping.as_ref());
        let mut targets = Vec::with_capacity(tracks.len());
        let mut push_target = |track: &Track| -> Result<(), MetadataError> {
            let source_path = track.source_path.clone().unwrap_or_default();
            if track.cue.is_some() {
                return Err(MetadataError::LocalAccessRequired { source_path });
            }
            let target = mapping.and_then(|mapping| {
                let direct = PathBuf::from(&source_path);
                let path = if direct.is_absolute() && direct.starts_with(&mapping.root_path) {
                    Some(direct)
                } else {
                    project_local_access_path(&source_path, mapping)
                }?;
                Some(LocalAccessTarget {
                    root_path: mapping.root_path.clone(),
                    path,
                })
            });
            let Some(target) = target else {
                return Err(MetadataError::LocalAccessRequired { source_path });
            };
            targets.push(target);
            Ok(())
        };
        for track in &tracks {
            push_target(track)?;
        }
        Ok(Some((subject, targets)))
    }

    pub fn playable_file(
        &self,
        track_id: &TrackId,
    ) -> crate::LibraryQueryResult<Option<PlayableFile>> {
        let state = self.read_state()?;
        let Some(track) = state.tracks.get(track_id) else {
            return Ok(None);
        };
        if let Some(path) = state.downloaded_files.get(track_id) {
            return Ok(Some(PlayableFile::File { path: path.clone() }));
        }
        Ok(playable_file_for(
            track,
            &state.local_files,
            state.local_access_mapping.as_ref(),
            &state.local_access,
            &state.local_access_index,
        ))
    }

    pub fn sidecar_audio_file(
        &self,
        track_id: &TrackId,
    ) -> crate::LibraryQueryResult<Option<SidecarAudioFile>> {
        let state = self.read_state()?;
        let Some(track) = state.tracks.get(track_id) else {
            return Ok(None);
        };
        Ok(playable_file_for(
            track,
            &state.local_files,
            state.local_access_mapping.as_ref(),
            &state.local_access,
            &state.local_access_index,
        )
        .map(|file| SidecarAudioFile {
            path: file.path().to_path_buf(),
            cue_track: matches!(file, PlayableFile::Cue { .. }),
        }))
    }
}

pub(crate) fn index_local_access(
    files: &[LocalAccessFile],
    mapping: Option<&LocalAccessMapping>,
) -> (HashSet<String>, HashMap<LocalMatchKey, Vec<usize>>) {
    let Some(mapping) = mapping else {
        return (HashSet::new(), HashMap::new());
    };
    let configured_root = mapping.root_path.to_string_lossy();
    let mut paths = HashSet::new();
    let mut index = HashMap::<LocalMatchKey, Vec<usize>>::new();
    for (position, file) in files.iter().enumerate() {
        if file.root != configured_root {
            continue;
        }
        paths.insert(file.path.clone());
        index
            .entry(local_match_key(
                &file.title,
                &file.album,
                &file.artist,
                file.disc_number,
                file.track_number,
            ))
            .or_default()
            .push(position);
    }
    (paths, index)
}

fn playable_file_for(
    track: &Track,
    local_files: &HashMap<String, LocalFile>,
    mapping: Option<&LocalAccessMapping>,
    files: &[LocalAccessFile],
    index: &HashMap<LocalMatchKey, Vec<usize>>,
) -> Option<PlayableFile> {
    if let Some(path) = track.source_path.as_ref().filter(|path| {
        local_files.get(*path).is_some_and(|file| {
            file.kind == LocalFileKind::Audio
                && matches!(
                    file.read_state,
                    LocalReadState::Parsed | LocalReadState::MetadataFallback
                )
        })
    }) {
        return Some(match &track.cue {
            Some(cue) if cue.end_millis > cue.start_millis => PlayableFile::Cue {
                path: path.into(),
                start_millis: cue.start_millis,
                end_millis: cue.end_millis,
            },
            _ => PlayableFile::File { path: path.into() },
        });
    }

    let mapping = mapping?;
    let projected = track.source_path.as_deref().and_then(|source_path| {
        project_local_access_path(source_path, mapping).or_else(|| {
            Path::new(source_path)
                .is_absolute()
                .then(|| PathBuf::from(source_path))
        })
    });
    if let Some(path) = projected.as_ref().filter(|path| path.is_file()) {
        return Some(PlayableFile::File { path: path.clone() });
    }
    if let Some(file) = unique_metadata_file(track, files, index) {
        return Some(PlayableFile::File {
            path: file.path.clone().into(),
        });
    }
    projected.map(|path| PlayableFile::File { path })
}

#[derive(Clone, Copy)]
enum LocalAccessMatch {
    Direct,
    Prefix,
    Metadata,
}

fn local_access_file_for(
    track: &Track,
    mapping: Option<&LocalAccessMapping>,
    files: &[LocalAccessFile],
    paths: &HashSet<String>,
    index: &HashMap<LocalMatchKey, Vec<usize>>,
) -> Option<(PlayableFile, LocalAccessMatch)> {
    let mapping = mapping?;
    if let Some((path, kind)) = exact_local_access_path(track, mapping, paths) {
        return Some((PlayableFile::File { path }, kind));
    }

    unique_metadata_file(track, files, index).map(|file| {
        (
            PlayableFile::File {
                path: file.path.clone().into(),
            },
            LocalAccessMatch::Metadata,
        )
    })
}

fn exact_local_access_path(
    track: &Track,
    mapping: &LocalAccessMapping,
    paths: &HashSet<String>,
) -> Option<(PathBuf, LocalAccessMatch)> {
    let source_path = track.source_path.as_deref()?;
    if paths.contains(source_path) {
        return Some((source_path.into(), LocalAccessMatch::Direct));
    }
    let path = project_local_access_path(source_path, mapping)?;
    paths
        .contains(path.to_string_lossy().as_ref())
        .then_some((path, LocalAccessMatch::Prefix))
}

fn unique_metadata_file<'a>(
    track: &Track,
    files: &'a [LocalAccessFile],
    index: &HashMap<LocalMatchKey, Vec<usize>>,
) -> Option<&'a LocalAccessFile> {
    let candidates = index.get(&local_match_key(
        &track.title,
        &track.album,
        &track.artist,
        track.disc_number,
        track.track_number,
    ))?;
    let mut matches = candidates.iter().filter_map(|position| {
        let candidate = files.get(*position)?;
        durations_close(track.duration_seconds, candidate.duration_seconds).then_some(candidate)
    });
    let candidate = matches.next()?;
    matches.next().is_none().then_some(candidate)
}

/// Projects one source path through the configured remote-to-local mapping.
///
/// The caller decides whether the projected file currently exists. An
/// absolute source path is itself a valid direct candidate when no server
/// prefix was configured.
pub fn project_local_access_path(
    source_path: &str,
    mapping: &LocalAccessMapping,
) -> Option<PathBuf> {
    let target = mapping
        .local_prefix
        .as_deref()
        .filter(|prefix| !prefix.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| mapping.root_path.clone());
    if let Some(prefix) = mapping
        .server_prefix
        .as_deref()
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
    {
        if let Some(suffix) = source_path.strip_prefix(prefix)
            && (suffix.is_empty()
                || prefix.ends_with(['/', '\\'])
                || suffix.starts_with(['/', '\\']))
        {
            return Some(path_from_server_suffix(
                &target,
                suffix.trim_start_matches(['/', '\\']),
            ));
        }
        return None;
    }
    let source = Path::new(source_path);
    if source.is_absolute() {
        Some(source.to_path_buf())
    } else if reported_path_is_absolute(source_path) {
        None
    } else {
        Some(target.join(source))
    }
}

/// Reports whether a source path is rooted in either Unix or Windows syntax.
///
/// A remote server may use a different path syntax from the client. A
/// host-native absolute path can be used directly; a foreign rooted path needs
/// a server prefix before it can be projected beneath the selected local root.
pub fn reported_path_is_absolute(source_path: &str) -> bool {
    let bytes = source_path.as_bytes();
    source_path.starts_with(['/', '\\'])
        || matches!(
            bytes,
            [drive, b':', separator, ..]
                if drive.is_ascii_alphabetic() && matches!(separator, b'/' | b'\\')
        )
}

fn path_from_server_suffix(target: &Path, suffix: &str) -> PathBuf {
    suffix
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .fold(target.to_path_buf(), |path, part| path.join(part))
}

fn local_match_key(
    title: &str,
    album: &str,
    artist: &str,
    disc_number: u16,
    track_number: u16,
) -> LocalMatchKey {
    LocalMatchKey {
        title: normalize_match_text(title),
        album: normalize_match_text(album),
        artist: normalize_match_text(artist),
        disc_number,
        track_number,
    }
}

fn normalize_match_text(value: &str) -> String {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn durations_close(left: u32, right: u32) -> bool {
    left == 0 || right == 0 || left.abs_diff(right) <= 3
}
