use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use library::{LoadedLibrary, SourceId, Track, TrackId};
use tracing::warn;

use crate::actor::{
    AUDIO_EXTENSION, DownloadJob, DownloadOwner, DownloadPaths, DownloadQuality, DownloadRecord,
    PART_EXTENSION, QUEUE_FILE, QUEUE_PART_FILE, QUEUE_VERSION, QueueFile, RECORD_EXTENSION,
    RECORD_VERSION,
};

pub(super) async fn add_owner_to_existing_download(
    root: &Path,
    source_id: &SourceId,
    track_id: &TrackId,
    owner: &DownloadOwner,
) -> Result<bool, String> {
    let metadata_paths = download_paths(root, source_id, track_id);
    if !metadata_paths.record.is_file() {
        return Ok(false);
    }
    let bytes = tokio::fs::read(&metadata_paths.record)
        .await
        .map_err(|error| format!("could not read the download record: {error}"))?;
    let mut record = serde_json::from_slice::<DownloadRecord>(&bytes)
        .map_err(|error| format!("could not decode the download record: {error}"))?;
    if record.source_id != *source_id || record.track_id != *track_id {
        return Ok(false);
    }
    let paths = record_download_paths(root, source_id, &record);
    if !paths.audio.is_file() {
        return Ok(false);
    }
    if record.owners.is_empty() {
        record.owners.insert(DownloadOwner::Retained);
    }
    if record.owners.insert(owner.clone()) {
        write_record(&paths, &record).await?;
    }
    Ok(true)
}

pub(super) async fn write_record(
    paths: &DownloadPaths,
    record: &DownloadRecord,
) -> Result<(), String> {
    let encoded = serde_json::to_vec(record)
        .map_err(|error| format!("could not encode the download record: {error}"))?;
    tokio::fs::write(&paths.record_part, encoded)
        .await
        .map_err(|error| format!("could not save the download record: {error}"))?;
    remove_file_if_present(&paths.record).await?;
    tokio::fs::rename(&paths.record_part, &paths.record)
        .await
        .map_err(|error| format!("could not finish the download record: {error}"))
}

pub(super) async fn persist_queue(
    root: &Path,
    source_id: &SourceId,
    jobs: &[DownloadJob],
) -> Result<(), String> {
    let directory = source_directory(root, source_id);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| format!("could not create the download queue directory: {error}"))?;
    let path = directory.join(QUEUE_FILE);
    let part = directory.join(QUEUE_PART_FILE);
    if jobs.is_empty() {
        remove_file_if_present(&path).await?;
        remove_file_if_present(&part).await?;
        return Ok(());
    }
    let encoded = serde_json::to_vec(&QueueFile {
        version: QUEUE_VERSION,
        source_id: source_id.clone(),
        jobs: jobs.to_vec(),
    })
    .map_err(|error| format!("could not encode the download queue: {error}"))?;
    tokio::fs::write(&part, encoded)
        .await
        .map_err(|error| format!("could not save the download queue: {error}"))?;
    remove_file_if_present(&path).await?;
    tokio::fs::rename(&part, &path)
        .await
        .map_err(|error| format!("could not finish the download queue: {error}"))
}

pub(super) fn load_queue(root: &Path, source_id: &SourceId) -> Result<Vec<DownloadJob>, String> {
    let path = source_directory(root, source_id).join(QUEUE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let queue = serde_json::from_slice::<QueueFile>(&bytes)
        .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    if queue.version != QUEUE_VERSION || queue.source_id != *source_id {
        return Err("the saved download queue does not match this source".to_string());
    }
    Ok(queue.jobs)
}

pub(super) fn load_download_records(
    root: &Path,
    source_id: &SourceId,
) -> Result<HashMap<TrackId, DownloadRecord>, String> {
    let directory = source_directory(root, source_id);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(format!(
                "could not read downloads at {}: {error}",
                directory.display()
            ));
        }
    };
    let mut records = HashMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read a download entry: {error}"))?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(QUEUE_FILE) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some(RECORD_EXTENSION) {
            continue;
        }
        let Ok(mut record) = std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<DownloadRecord>(&bytes).map_err(|error| error.to_string())
            })
        else {
            continue;
        };
        if record.version > RECORD_VERSION || record.source_id != *source_id {
            continue;
        }
        if record.owners.is_empty() {
            record.owners.insert(DownloadOwner::Retained);
        }
        records.insert(record.track_id.clone(), record);
    }
    Ok(records)
}

pub(super) fn load_downloaded_files(
    root: &Path,
    source_id: &SourceId,
) -> Result<HashMap<TrackId, PathBuf>, String> {
    let directory = source_directory(root, source_id);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(format!(
                "could not read downloads at {}: {error}",
                directory.display()
            ));
        }
    };
    let mut files = HashMap::new();
    let mut referenced_audio = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read a download entry: {error}"))?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(QUEUE_PART_FILE) {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some(PART_EXTENSION) {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some(QUEUE_FILE)
            || path.extension().and_then(|value| value.to_str()) != Some(RECORD_EXTENSION)
        {
            continue;
        }
        let record = match std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<DownloadRecord>(&bytes).map_err(|error| error.to_string())
            }) {
            Ok(record) if record.version <= RECORD_VERSION && record.source_id == *source_id => {
                record
            }
            Ok(_) | Err(_) => {
                warn!(path = %path.display(), "ignored an invalid download record");
                let _ = std::fs::remove_file(path);
                continue;
            }
        };
        let expected = record_download_paths(root, source_id, &record);
        if path != expected.record {
            warn!(path = %path.display(), "ignored a misplaced download record");
            let _ = std::fs::remove_file(path);
            continue;
        }
        if expected.audio.is_file() {
            if expected.audio_root.is_none() {
                referenced_audio.insert(
                    expected
                        .audio
                        .file_name()
                        .expect("download audio path has a file name")
                        .to_os_string(),
                );
            }
            let _ = std::fs::remove_file(&expected.audio_part);
            files.insert(record.track_id, expected.audio);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some(AUDIO_EXTENSION)
                && path
                    .file_name()
                    .is_some_and(|name| !referenced_audio.contains(name))
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(files)
}

pub(super) fn attach_downloaded_files(
    root: &Path,
    loaded: &Arc<LoadedLibrary>,
) -> Result<(), String> {
    let source_id = loaded.source_id();
    let files = load_downloaded_files(root, source_id)?;
    let removed = loaded
        .replace_downloaded_files(files)
        .map_err(|error| error.to_string())?;
    let records = load_download_records(root, source_id)?;
    for track_id in removed {
        let paths = records
            .get(&track_id)
            .map(|record| record_download_paths(root, source_id, record))
            .unwrap_or_else(|| download_paths(root, source_id, &track_id));
        remove_download_files_blocking(&paths)?;
    }
    Ok(())
}

pub(super) async fn remove_download_files(paths: &DownloadPaths) -> Result<bool, String> {
    let mut present = false;
    for path in [
        &paths.audio,
        &paths.audio_part,
        &paths.record,
        &paths.record_part,
    ] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => present = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not remove {}: {error}", path.display())),
        }
    }
    remove_empty_audio_directories(paths).await;
    Ok(present)
}

fn remove_download_files_blocking(paths: &DownloadPaths) -> Result<(), String> {
    for path in [
        &paths.audio,
        &paths.audio_part,
        &paths.record,
        &paths.record_part,
    ] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not remove {}: {error}", path.display())),
        }
    }
    remove_empty_audio_directories_blocking(paths);
    Ok(())
}

async fn remove_empty_audio_directories(paths: &DownloadPaths) {
    let Some(root) = paths.audio_root.as_ref() else {
        return;
    };
    let Some(album) = paths.audio.parent() else {
        return;
    };
    let Some(artist) = album.parent() else {
        return;
    };
    for directory in [album, artist] {
        if directory == root {
            break;
        }
        match tokio::fs::remove_dir(directory).await {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }
}

fn remove_empty_audio_directories_blocking(paths: &DownloadPaths) {
    let Some(root) = paths.audio_root.as_ref() else {
        return;
    };
    let Some(album) = paths.audio.parent() else {
        return;
    };
    let Some(artist) = album.parent() else {
        return;
    };
    for directory in [album, artist] {
        if directory == root {
            break;
        }
        match std::fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }
}

pub(super) async fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not replace {}: {error}", path.display())),
    }
}

pub(super) fn source_directory(root: &Path, source_id: &SourceId) -> PathBuf {
    root.join(hash_id(source_id.as_str()))
}

pub(super) fn download_paths(
    root: &Path,
    source_id: &SourceId,
    track_id: &TrackId,
) -> DownloadPaths {
    let directory = source_directory(root, source_id);
    let stem = hash_id(track_id.as_str());
    DownloadPaths {
        audio_root: None,
        audio: directory.join(format!("{stem}.{AUDIO_EXTENSION}")),
        audio_part: directory.join(format!("{stem}.{AUDIO_EXTENSION}.{PART_EXTENSION}")),
        record: directory.join(format!("{stem}.{RECORD_EXTENSION}")),
        record_part: directory.join(format!("{stem}.{RECORD_EXTENSION}.{PART_EXTENSION}")),
        directory,
    }
}

pub(super) fn new_download_paths(
    root: &Path,
    source_id: &SourceId,
    track: &Track,
    directory: Option<&Path>,
    quality: DownloadQuality,
) -> DownloadPaths {
    let mut paths = download_paths(root, source_id, &track.id);
    let audio_root = directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| source_directory(root, source_id));
    let artist = safe_path_component(&track.artist, "Unknown Artist");
    let album = safe_path_component(&track.album, "Unknown Album");
    let title = safe_path_component(&track.title, "Untitled");
    let id = source_track_hash(source_id, &track.id);
    let short_id = id.chars().take(12).collect::<String>();
    let extension = download_extension(track, quality);
    let file_name = format!(
        "{:02}-{:02} {title} [{}].{extension}",
        track.disc_number, track.track_number, short_id
    );
    let audio = audio_root.join(artist).join(album).join(file_name);
    let audio_part = part_path(&audio);
    paths.audio_root = Some(audio_root);
    paths.audio = audio;
    paths.audio_part = audio_part;
    paths
}

pub(super) fn record_download_paths(
    root: &Path,
    source_id: &SourceId,
    record: &DownloadRecord,
) -> DownloadPaths {
    let mut paths = download_paths(root, source_id, &record.track_id);
    let Some(audio_root) = record.audio_root.as_ref() else {
        return paths;
    };
    let Some(audio) = record.audio_path.as_ref() else {
        return paths;
    };
    if !valid_managed_audio_path(audio_root, audio) {
        return paths;
    }
    paths.audio_root = Some(audio_root.clone());
    paths.audio = audio.clone();
    paths.audio_part = part_path(audio);
    paths
}

fn part_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{PART_EXTENSION}"));
    value.into()
}

fn valid_managed_audio_path(root: &Path, audio: &Path) -> bool {
    let Ok(relative) = audio.strip_prefix(root) else {
        return false;
    };
    let components = relative.components().collect::<Vec<_>>();
    components.len() == 3
        && components
            .iter()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn safe_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .take(80)
        .collect::<String>();
    let sanitized = sanitized.trim_matches([' ', '.']);
    if sanitized.is_empty() {
        fallback.to_string()
    } else {
        sanitized.to_string()
    }
}

fn download_extension(track: &Track, quality: DownloadQuality) -> String {
    if matches!(quality, DownloadQuality::MaxBitrateKbps(_)) {
        return "mp3".to_string();
    }
    let extension = track
        .source_format
        .as_deref()
        .unwrap_or(AUDIO_EXTENSION)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    if extension.is_empty() {
        AUDIO_EXTENSION.to_string()
    } else {
        extension
    }
}

fn hash_id(value: &str) -> String {
    hash_id_bytes(value.as_bytes())
}

fn source_track_hash(source_id: &SourceId, track_id: &TrackId) -> String {
    let mut value = Vec::with_capacity(source_id.as_str().len() + track_id.as_str().len() + 1);
    value.extend_from_slice(source_id.as_str().as_bytes());
    value.push(0);
    value.extend_from_slice(track_id.as_str().as_bytes());
    hash_id_bytes(&value)
}

pub(super) fn hash_id_bytes(value: &[u8]) -> String {
    blake3::hash(value).to_hex().to_string()
}
