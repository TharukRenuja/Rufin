use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use library::{Library, SourceId, Track, TrackId};
use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, DATE, ETAG, IF_RANGE,
    LAST_MODIFIED, RANGE,
};
use serde::{Deserialize, Serialize};
use sources::{
    NativeSourceResult, Source, SourceError, SourceResult, StreamQuality, StreamRequest,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use tracing::warn;

use crate::{DownloadOwner, DownloadQuality};

pub(super) const RECORD_VERSION: u32 = 3;
pub(super) const AUDIO_EXTENSION: &str = "audio";
pub(super) const RECORD_EXTENSION: &str = "json";
pub(super) const PART_EXTENSION: &str = "part";
const CHECKPOINT_EXTENSION: &str = "resume";
const CUSTOM_STAGING_DIRECTORY: &str = ".rufin-partials";

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct DownloadRecord {
    pub(super) version: u32,
    pub(super) source_id: SourceId,
    pub(super) track_id: TrackId,
    #[serde(default)]
    pub(super) owners: HashSet<DownloadOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) audio_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) audio_path: Option<PathBuf>,
}

#[derive(Clone)]
pub(super) struct DownloadPaths {
    pub(super) directory: PathBuf,
    pub(super) audio_root: Option<PathBuf>,
    pub(super) audio: PathBuf,
    pub(super) audio_part: PathBuf,
    pub(super) record: PathBuf,
    pub(super) record_part: PathBuf,
    pub(super) checkpoint: PathBuf,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct TransferCheckpoint {
    representation: String,
    validator: String,
    length: u64,
}

fn resume_validator(headers: &reqwest::header::HeaderMap) -> Option<String> {
    if let Some(etag) = headers.get(ETAG) {
        let etag = etag.to_str().ok()?;
        return strong_etag(etag).then(|| etag.to_string());
    }
    let modified = headers.get(LAST_MODIFIED)?.to_str().ok()?;
    let date = headers.get(DATE)?.to_str().ok()?;
    let age = httpdate::parse_http_date(date)
        .ok()?
        .duration_since(httpdate::parse_http_date(modified).ok()?)
        .ok()?;
    (age >= Duration::from_secs(60)).then(|| modified.to_string())
}

#[derive(Default)]
pub(super) struct TransferClients {
    strict: tokio::sync::OnceCell<reqwest::Client>,
    insecure: tokio::sync::OnceCell<reqwest::Client>,
}

impl TransferClients {
    async fn download_cancellable(
        &self,
        source: &Source,
        request: &StreamRequest,
        paths: &DownloadPaths,
        cancellation: &mut oneshot::Receiver<()>,
    ) -> SourceResult<NativeSourceResult<()>> {
        let stream = tokio::select! {
            biased;
            result = source.stream(request) => result,
            _ = &mut *cancellation => Err(SourceError::Cancelled),
        }?;
        let NativeSourceResult::Available(stream) = stream else {
            return Ok(NativeSourceResult::Unavailable);
        };
        let clients = if stream.trust_invalid_certificate() {
            &self.insecure
        } else {
            &self.strict
        };
        let trust_invalid_certificate = stream.trust_invalid_certificate();
        let representation = representation_key(source.source_id(), request, stream.redacted_uri());
        let client = clients
            .get_or_try_init(|| async move {
                reqwest::Client::builder()
                    .danger_accept_invalid_certs(trust_invalid_certificate)
                    .connect_timeout(Duration::from_secs(15))
                    .build()
                    .map_err(download_request_error)
            })
            .await?;
        let resume = read_checkpoint(paths, Some(&representation)).await?;
        let response = send_request(client, stream.uri(), resume.as_ref(), cancellation).await?;
        let status = response.status();
        if status == reqwest::StatusCode::OK {
            return download_full(response, paths, &representation, cancellation).await;
        }
        if status == reqwest::StatusCode::PARTIAL_CONTENT
            && let Some((checkpoint, offset)) = resume.as_ref()
            && valid_partial(&response, checkpoint, *offset)
        {
            return download_partial(response, paths, checkpoint, *offset, cancellation).await;
        }
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE
            && let Some((checkpoint, offset)) = resume.as_ref()
            && *offset == checkpoint.length
            && unsatisfied_total(&response) == Some(checkpoint.length)
        {
            return Ok(NativeSourceResult::Available(()));
        }
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && resume.is_some() {
            discard_staging(paths).await?;
            let response = send_request(client, stream.uri(), None, cancellation).await?;
            if response.status() == reqwest::StatusCode::OK {
                return download_full(response, paths, &representation, cancellation).await;
            }
            if !response.status().is_success() {
                return response_error(response.status(), paths).await;
            }
            return Err(SourceError::Other(
                "the download server returned an unsupported successful response".to_string(),
            ));
        }
        if !status.is_success() {
            return response_error(status, paths).await;
        }
        Err(SourceError::Other(
            "the download server returned an unsupported successful response".to_string(),
        ))
    }

    #[cfg(test)]
    async fn download(
        &self,
        source: &Source,
        request: &StreamRequest,
        paths: &DownloadPaths,
    ) -> SourceResult<NativeSourceResult<()>> {
        let (_cancellation, mut receiver) = oneshot::channel();
        self.download_cancellable(source, request, paths, &mut receiver)
            .await
    }
}

fn representation_key(source_id: &SourceId, request: &StreamRequest, redacted_uri: &str) -> String {
    let value = serde_json::to_vec(&(source_id, request, redacted_uri))
        .expect("a download representation can be encoded");
    hash_id_bytes(&value)
}

async fn send_request(
    client: &reqwest::Client,
    uri: &str,
    resume: Option<&(TransferCheckpoint, u64)>,
    cancellation: &mut oneshot::Receiver<()>,
) -> SourceResult<reqwest::Response> {
    let mut request = client.get(uri).header(ACCEPT_ENCODING, "identity");
    if let Some((checkpoint, offset)) = resume {
        request = request
            .header(RANGE, format!("bytes={offset}-"))
            .header(IF_RANGE, &checkpoint.validator);
    }
    tokio::select! {
        biased;
        result = request.send() => result.map_err(download_request_error),
        _ = &mut *cancellation => Err(SourceError::Cancelled),
    }
}

async fn download_full(
    response: reqwest::Response,
    paths: &DownloadPaths,
    representation: &str,
    cancellation: &mut oneshot::Receiver<()>,
) -> SourceResult<NativeSourceResult<()>> {
    if !identity_response(&response) {
        discard_staging(paths).await?;
        return Err(SourceError::Other(
            "the download response used an unsupported encoding".to_string(),
        ));
    }
    let expected = response_length(&response).filter(|length| *length > 0);
    let checkpoint = expected.and_then(|length| {
        resume_validator(response.headers()).map(|validator| TransferCheckpoint {
            representation: representation.to_string(),
            validator,
            length,
        })
    });
    discard_staging(paths).await?;
    let file = tokio::fs::File::create(&paths.audio_part)
        .await
        .map_err(|error| SourceError::Other(format!("could not create download: {error}")))?;
    if let Some(checkpoint) = checkpoint.as_ref() {
        write_checkpoint(paths, checkpoint).await?;
    }
    write_response(
        response,
        paths,
        file,
        expected,
        checkpoint.is_some(),
        0,
        cancellation,
    )
    .await?;
    Ok(NativeSourceResult::Available(()))
}

async fn download_partial(
    response: reqwest::Response,
    paths: &DownloadPaths,
    checkpoint: &TransferCheckpoint,
    offset: u64,
    cancellation: &mut oneshot::Receiver<()>,
) -> SourceResult<NativeSourceResult<()>> {
    let file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.audio_part)
        .await
        .map_err(|error| SourceError::Other(format!("could not resume download: {error}")))?;
    write_response(
        response,
        paths,
        file,
        Some(checkpoint.length - offset),
        true,
        offset,
        cancellation,
    )
    .await?;
    Ok(NativeSourceResult::Available(()))
}

async fn write_response(
    response: reqwest::Response,
    paths: &DownloadPaths,
    mut file: tokio::fs::File,
    expected: Option<u64>,
    resumable: bool,
    starting_length: u64,
    cancellation: &mut oneshot::Receiver<()>,
) -> SourceResult<()> {
    let written = match stream_body(response, &mut file, expected, cancellation).await {
        Ok(written) => written,
        Err(error) => return finish_failed_body(paths, file, resumable, error).await,
    };
    if written == 0 || expected.is_some_and(|expected| written != expected) {
        let error = if expected.is_some() {
            SourceError::Network("the download ended before its declared length".to_string())
        } else {
            SourceError::Other("the download response was empty".to_string())
        };
        return finish_failed_body(
            paths,
            file,
            resumable && (starting_length > 0 || written > 0),
            error,
        )
        .await;
    }
    finish_file(file).await
}

async fn stream_body(
    mut response: reqwest::Response,
    file: &mut tokio::fs::File,
    maximum: Option<u64>,
    cancellation: &mut oneshot::Receiver<()>,
) -> SourceResult<u64> {
    let mut bytes_written = 0u64;
    loop {
        let chunk = tokio::select! {
            biased;
            result = tokio::time::timeout(Duration::from_secs(60), response.chunk()) => result
                .map_err(|_| SourceError::Network("the download stalled".to_string()))?
                .map_err(download_request_error)?,
            _ = &mut *cancellation => return Err(SourceError::Cancelled),
        };
        let Some(chunk) = chunk else {
            break;
        };
        let next = bytes_written.saturating_add(chunk.len() as u64);
        if maximum.is_some_and(|maximum| next > maximum) {
            return Err(SourceError::Other(
                "the download response exceeded its declared length".to_string(),
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| SourceError::Other(format!("could not write download: {error}")))?;
        bytes_written = next;
    }
    Ok(bytes_written)
}

async fn finish_failed_body<T>(
    paths: &DownloadPaths,
    mut file: tokio::fs::File,
    resumable: bool,
    error: SourceError,
) -> SourceResult<T> {
    if resumable && file.flush().await.is_ok() && file.sync_all().await.is_ok() {
        return Err(error);
    }
    drop(file);
    discard_staging(paths).await?;
    Err(error)
}

async fn finish_file(mut file: tokio::fs::File) -> SourceResult<()> {
    file.flush()
        .await
        .map_err(|error| SourceError::Other(format!("could not finish download: {error}")))?;
    file.sync_all()
        .await
        .map_err(|error| SourceError::Other(format!("could not save download: {error}")))
}

async fn read_checkpoint(
    paths: &DownloadPaths,
    representation: Option<&str>,
) -> SourceResult<Option<(TransferCheckpoint, u64)>> {
    let encoded = match tokio::fs::read(&paths.checkpoint).await {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            discard_staging(paths).await?;
            return Ok(None);
        }
        Err(error) => {
            return Err(SourceError::Other(format!(
                "could not read download checkpoint: {error}"
            )));
        }
    };
    let checkpoint = serde_json::from_slice::<TransferCheckpoint>(&encoded).ok();
    let length = tokio::fs::metadata(&paths.audio_part)
        .await
        .ok()
        .map(|metadata| metadata.len());
    if checkpoint.as_ref().is_some_and(|checkpoint| {
        !checkpoint.representation.is_empty()
            && representation.is_none_or(|expected| checkpoint.representation == expected)
            && checkpoint.length > 0
            && (strong_etag(&checkpoint.validator)
                || httpdate::parse_http_date(&checkpoint.validator).is_ok())
    }) && let (Some(checkpoint), Some(length)) = (checkpoint, length)
        && length > 0
        && length <= checkpoint.length
    {
        return Ok(Some((checkpoint, length)));
    }
    discard_staging(paths).await?;
    Ok(None)
}

async fn write_checkpoint(
    paths: &DownloadPaths,
    checkpoint: &TransferCheckpoint,
) -> SourceResult<()> {
    let encoded = serde_json::to_vec(checkpoint)
        .map_err(|error| SourceError::Other(format!("could not encode checkpoint: {error}")))?;
    tokio::fs::write(&paths.checkpoint, encoded)
        .await
        .map_err(|error| SourceError::Other(format!("could not write checkpoint: {error}")))
}

pub(super) async fn discard_staging(paths: &DownloadPaths) -> SourceResult<()> {
    for path in [&paths.audio_part, &paths.checkpoint] {
        remove_file_if_present(path)
            .await
            .map_err(SourceError::Other)?;
    }
    Ok(())
}

pub(super) async fn cleanup_staging(
    root: &Path,
    source_id: &SourceId,
    directory: Option<&Path>,
    track_ids: &HashSet<TrackId>,
) -> SourceResult<()> {
    let expected = track_ids
        .iter()
        .map(|track_id| staging_paths(root, source_id, track_id, directory))
        .flat_map(|paths| [paths.audio_part.clone(), paths.checkpoint.clone()])
        .collect::<HashSet<_>>();
    let custom = directory.map(|directory| {
        directory
            .join(CUSTOM_STAGING_DIRECTORY)
            .join(hash_id(source_id.as_str()))
    });
    for directory in [Some(source_directory(root, source_id)), custom.clone()]
        .into_iter()
        .flatten()
    {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(SourceError::Other(format!(
                    "could not inspect download staging at {}: {error}",
                    directory.display()
                )));
            }
        };
        while let Some(entry) = entries.next_entry().await.map_err(|error| {
            SourceError::Other(format!("could not inspect download staging: {error}"))
        })? {
            let path = entry.path();
            if managed_staging_file(&path) && !expected.contains(&path) {
                remove_file_if_present(&path)
                    .await
                    .map_err(SourceError::Other)?;
            }
        }
    }
    if let Some(source_staging) = custom {
        let _ = tokio::fs::remove_dir(source_staging).await;
    }
    if let Some(directory) = directory {
        let _ = tokio::fs::remove_dir(directory.join(CUSTOM_STAGING_DIRECTORY)).await;
    }
    Ok(())
}

fn managed_staging_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".audio.part") || name.ends_with(".audio.part.resume"))
}

fn managed_record_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some(RECORD_EXTENSION)
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|stem| {
                stem.len() == 64
                    && stem
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            })
}

fn response_length(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn strong_etag(value: &str) -> bool {
    !value.starts_with("W/") && value.starts_with('"') && value.ends_with('"') && value.len() >= 2
}

fn valid_partial(
    response: &reqwest::Response,
    checkpoint: &TransferCheckpoint,
    offset: u64,
) -> bool {
    if !identity_response(response) {
        return false;
    }
    let Some((start, end, total)) = satisfied_range(response) else {
        return false;
    };
    let expected_suffix = checkpoint.length.saturating_sub(offset);
    start == offset
        && end.checked_add(1) == Some(total)
        && total == checkpoint.length
        && response_length(response).is_none_or(|length| length == expected_suffix)
}

fn identity_response(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| value.eq_ignore_ascii_case("identity"))
}

fn satisfied_range(response: &reqwest::Response) -> Option<(u64, u64, u64)> {
    let value = response.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

fn unsatisfied_total(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(CONTENT_RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes */")?
        .parse()
        .ok()
}

fn status_error(status: u16) -> SourceError {
    match status {
        401 | 403 => SourceError::Auth("the download was not authorized".to_string()),
        404 => SourceError::NotFound,
        status => SourceError::Server {
            status,
            message: "the download request failed".to_string(),
        },
    }
}

async fn response_error<T>(status: reqwest::StatusCode, paths: &DownloadPaths) -> SourceResult<T> {
    if status == reqwest::StatusCode::NOT_FOUND
        || (status.is_client_error() && !matches!(status.as_u16(), 401 | 403 | 429))
    {
        discard_staging(paths).await?;
    }
    Err(status_error(status.as_u16()))
}

fn download_request_error(error: reqwest::Error) -> SourceError {
    if error.is_timeout() {
        SourceError::Network("the download timed out".to_string())
    } else if error.is_connect() {
        SourceError::Network("could not connect for the download".to_string())
    } else if error
        .to_string()
        .to_ascii_lowercase()
        .contains("certificate")
    {
        SourceError::Tls("the download certificate was rejected".to_string())
    } else {
        SourceError::Network("the download was interrupted".to_string())
    }
}

pub(super) async fn run_transfer(
    source: &Source,
    track_id: TrackId,
    quality: DownloadQuality,
    paths: &DownloadPaths,
    transfers: &TransferClients,
    mut cancellation: oneshot::Receiver<()>,
) -> SourceResult<NativeSourceResult<()>> {
    for directory in [
        Some(paths.directory.as_path()),
        paths.audio.parent(),
        paths.audio_part.parent(),
    ]
    .into_iter()
    .flatten()
    {
        tokio::fs::create_dir_all(directory)
            .await
            .map_err(|error| {
                SourceError::Other(format!("could not create a download directory: {error}"))
            })?;
    }
    remove_file_if_present(&paths.record_part)
        .await
        .map_err(SourceError::Other)?;
    let quality = match quality {
        DownloadQuality::Original => StreamQuality::Original,
        DownloadQuality::MaxBitrateKbps(value) => StreamQuality::MaxBitrateKbps(value),
    };
    let request = StreamRequest::new(track_id, quality);
    let NativeSourceResult::Available(()) = transfers
        .download_cancellable(source, &request, paths, &mut cancellation)
        .await?
    else {
        return Ok(NativeSourceResult::Unavailable);
    };
    Ok(NativeSourceResult::Available(()))
}

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
    tokio::fs::rename(&paths.record_part, &paths.record)
        .await
        .map_err(|error| format!("could not finish the download record: {error}"))
}

pub(super) async fn finalize_download(
    paths: &DownloadPaths,
    source_id: SourceId,
    track_id: TrackId,
    owner: DownloadOwner,
) -> Result<(), String> {
    let record = DownloadRecord {
        version: RECORD_VERSION,
        source_id,
        track_id,
        owners: HashSet::from([owner]),
        audio_root: paths.audio_root.clone(),
        audio_path: Some(paths.audio.clone()),
    };
    write_record(paths, &record).await?;
    tokio::fs::rename(&paths.audio_part, &paths.audio)
        .await
        .map_err(|error| format!("could not save the downloaded track: {error}"))?;
    let _ = remove_file_if_present(&paths.checkpoint).await;
    Ok(())
}

pub(super) fn load_download_records(
    root: &Path,
    source_id: &SourceId,
) -> Result<HashMap<TrackId, DownloadRecord>, String> {
    load_download_state(root, source_id).map(|(_, records)| records)
}

fn load_download_state(
    root: &Path,
    source_id: &SourceId,
) -> Result<(HashMap<TrackId, PathBuf>, HashMap<TrackId, DownloadRecord>), String> {
    let directory = source_directory(root, source_id);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((HashMap::new(), HashMap::new()));
        }
        Err(error) => {
            return Err(format!(
                "could not read downloads at {}: {error}",
                directory.display()
            ));
        }
    };
    let mut files = HashMap::new();
    let mut records = HashMap::new();
    let mut referenced_audio = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read a download entry: {error}"))?;
        let path = entry.path();
        if managed_staging_file(&path) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some(PART_EXTENSION)
            && managed_record_file(&path.with_extension(""))
        {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if !managed_record_file(&path) {
            continue;
        }
        let mut record = match std::fs::read(&path)
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
        if record.owners.is_empty() {
            record.owners.insert(DownloadOwner::Retained);
        }
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
            let _ = std::fs::remove_file(&expected.checkpoint);
            files.insert(record.track_id.clone(), expected.audio);
            records.insert(record.track_id.clone(), record);
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
    Ok((files, records))
}

pub(super) fn attach_downloaded_files(
    root: &Path,
    loaded: &Arc<Library>,
) -> Result<Vec<DownloadPaths>, String> {
    let source_id = loaded.source_id();
    let (files, records) = load_download_state(root, source_id)?;
    let removed = loaded
        .replace_downloaded_files(files)
        .map_err(|error| error.to_string())?;
    Ok(removed
        .into_iter()
        .map(|track_id| {
            records
                .get(&track_id)
                .map(|record| record_download_paths(root, source_id, record))
                .unwrap_or_else(|| download_paths(root, source_id, &track_id))
        })
        .collect())
}

pub(super) async fn remove_download_files(paths: &DownloadPaths) -> Result<bool, String> {
    let mut present = false;
    for path in [
        &paths.audio,
        &paths.audio_part,
        &paths.record,
        &paths.record_part,
        &paths.checkpoint,
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
        if tokio::fs::remove_dir(directory).await.is_err() {
            break;
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
    let audio = directory.join(format!("{stem}.{AUDIO_EXTENSION}"));
    let audio_part = part_path(&audio);
    let checkpoint = checkpoint_path(&audio_part);
    DownloadPaths {
        audio_root: None,
        audio,
        audio_part,
        record: directory.join(format!("{stem}.{RECORD_EXTENSION}")),
        record_part: directory.join(format!("{stem}.{RECORD_EXTENSION}.{PART_EXTENSION}")),
        checkpoint,
        directory,
    }
}

pub(super) fn staging_paths(
    root: &Path,
    source_id: &SourceId,
    track_id: &TrackId,
    directory: Option<&Path>,
) -> DownloadPaths {
    let mut paths = download_paths(root, source_id, track_id);
    let Some(directory) = directory else {
        return paths;
    };
    let staging = directory
        .join(CUSTOM_STAGING_DIRECTORY)
        .join(hash_id(source_id.as_str()));
    paths.audio_part = staging.join(format!(
        "{}.{}.{}",
        hash_id(track_id.as_str()),
        AUDIO_EXTENSION,
        PART_EXTENSION
    ));
    paths.checkpoint = checkpoint_path(&paths.audio_part);
    paths
}

pub(super) fn new_download_paths(
    root: &Path,
    source_id: &SourceId,
    track: &Track,
    directory: Option<&Path>,
    quality: DownloadQuality,
) -> DownloadPaths {
    let mut paths = staging_paths(root, source_id, &track.id, directory);
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
    paths.audio_root = Some(audio_root);
    paths.audio = audio;
    paths
}

pub(super) fn record_download_paths(
    root: &Path,
    source_id: &SourceId,
    record: &DownloadRecord,
) -> DownloadPaths {
    let internal_root = source_directory(root, source_id);
    let Some(audio_root) = record.audio_root.as_ref() else {
        return download_paths(root, source_id, &record.track_id);
    };
    let Some(audio) = record.audio_path.as_ref() else {
        return download_paths(root, source_id, &record.track_id);
    };
    if !valid_managed_audio_path(audio_root, audio) {
        return download_paths(root, source_id, &record.track_id);
    }
    let mut paths = staging_paths(
        root,
        source_id,
        &record.track_id,
        (audio_root != &internal_root).then_some(audio_root.as_path()),
    );
    paths.audio_root = Some(audio_root.clone());
    paths.audio = audio.clone();
    paths
}

fn part_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{PART_EXTENSION}"));
    value.into()
}

fn checkpoint_path(audio_part: &Path) -> PathBuf {
    let mut value = audio_part.as_os_str().to_os_string();
    value.push(format!(".{CHECKPOINT_EXTENSION}"));
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
    let value =
        serde_json::to_vec(&(source_id, track_id)).expect("a download identity can be encoded");
    hash_id_bytes(&value)
}

pub(super) fn hash_id_bytes(value: &[u8]) -> String {
    blake3::hash(value).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::SourceConfiguration;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn scripted_server(
        responses: Vec<Vec<u8>>,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind download server");
        let address = listener.local_addr().expect("download server address");
        let (requests, received) = mpsc::channel();
        let task = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept download request");
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let read = stream.read(&mut buffer).expect("read download request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                requests
                    .send(String::from_utf8_lossy(&request).into_owned())
                    .expect("record download request");
                stream
                    .write_all(&response)
                    .expect("write download response");
            }
        });
        (format!("http://{address}"), received, task)
    }

    fn remote_source(base_url: &str) -> Source {
        Source::open(
            SourceConfiguration {
                source_id: SourceId::new("configured:jellyfin"),
                kind: "jellyfin".to_string(),
                name: "Server".to_string(),
                provider_payload: serde_json::json!({
                    "version": 1,
                    "base_url": base_url,
                    "server_id": null,
                    "user_id": "account",
                    "username": "listener",
                    "trust_invalid_cert": false,
                    "use_jellyfin_instant_mix": false,
                })
                .to_string(),
            },
            Some("secret-token".to_string()),
            Some("device-one".to_string()),
        )
        .expect("open remote source")
    }

    #[test]
    fn last_modified_is_used_only_when_no_entity_tag_exists() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(ETAG, reqwest::header::HeaderValue::from_static("W/\"v1\""));
        headers.insert(
            DATE,
            reqwest::header::HeaderValue::from_static("Wed, 21 Oct 2015 07:27:01 GMT"),
        );
        headers.insert(
            LAST_MODIFIED,
            reqwest::header::HeaderValue::from_static("Wed, 21 Oct 2015 07:27:00 GMT"),
        );

        assert_eq!(resume_validator(&headers), None);
        headers.remove(ETAG);
        assert_eq!(resume_validator(&headers), None);
        headers.insert(
            DATE,
            reqwest::header::HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
        );
        assert_eq!(
            resume_validator(&headers),
            Some("Wed, 21 Oct 2015 07:27:00 GMT".to_string())
        );
    }

    #[tokio::test]
    async fn a_record_failure_keeps_the_completed_transfer() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::new("source");
        let track_id = TrackId::new("track");
        let paths = download_paths(directory.path(), &source_id, &track_id);
        tokio::fs::create_dir_all(&paths.directory)
            .await
            .expect("download directory");
        tokio::fs::write(&paths.audio_part, b"complete audio")
            .await
            .expect("completed transfer");
        tokio::fs::create_dir(&paths.record_part)
            .await
            .expect("block record write");

        assert!(
            finalize_download(&paths, source_id, track_id, DownloadOwner::Retained,)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&paths.audio_part).unwrap(), b"complete audio");
        assert!(!paths.audio.exists());
    }

    #[tokio::test]
    async fn a_checkpoint_cannot_resume_a_different_representation() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let paths = download_paths(
            directory.path(),
            &SourceId::new("source"),
            &TrackId::new("track"),
        );
        tokio::fs::create_dir_all(&paths.directory)
            .await
            .expect("download directory");
        tokio::fs::write(&paths.audio_part, b"abcd")
            .await
            .expect("partial download");
        write_checkpoint(
            &paths,
            &TransferCheckpoint {
                representation: "old".to_string(),
                validator: "\"v1\"".to_string(),
                length: 10,
            },
        )
        .await
        .expect("download checkpoint");

        assert!(
            read_checkpoint(&paths, Some("new"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(!paths.audio_part.exists());
        assert!(!paths.checkpoint.exists());
    }

    #[tokio::test]
    async fn an_interrupted_body_resumes_from_its_saved_bytes() {
        let (url, requests, server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nDate: Wed, 21 Oct 2015 07:29:00 GMT\r\nLast-Modified: Wed, 21 Oct 2015 07:27:00 GMT\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\nabcd".to_vec(),
            b"HTTP/1.1 206 Partial Content\r\nTransfer-Encoding: chunked\r\nContent-Range: bytes 4-9/10\r\nConnection: close\r\n\r\n0\r\n\r\n".to_vec(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 6\r\nContent-Range: bytes 4-9/10\r\nDate: Wed, 21 Oct 2015 07:30:00 GMT\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\nefghij".to_vec(),
        ]);
        let source = remote_source(&url);
        let track_id = TrackId::new("jellyfin:track:one");
        let directory = tempfile::tempdir().expect("temporary downloads");
        let paths = download_paths(directory.path(), source.source_id(), &track_id);
        tokio::fs::create_dir_all(&paths.directory)
            .await
            .expect("download directory");
        let transfers = TransferClients::default();
        let request = StreamRequest::original(track_id);

        assert!(matches!(
            transfers.download(&source, &request, &paths).await,
            Err(SourceError::Network(_))
        ));
        assert_eq!(std::fs::read(&paths.audio_part).unwrap(), b"abcd");
        assert!(paths.checkpoint.is_file());
        let orphan = download_paths(
            directory.path(),
            source.source_id(),
            &TrackId::new("jellyfin:track:orphan"),
        );
        std::fs::write(&orphan.audio_part, b"orphan").expect("orphan partial");
        load_download_state(directory.path(), source.source_id()).expect("reload downloads");
        cleanup_staging(
            directory.path(),
            source.source_id(),
            None,
            &HashSet::from([request.track_id.clone()]),
        )
        .await
        .expect("clean staging against the queue");
        assert!(paths.audio_part.is_file());
        assert!(!orphan.audio_part.exists());
        assert!(matches!(
            transfers.download(&source, &request, &paths).await,
            Err(SourceError::Network(_))
        ));
        assert_eq!(std::fs::read(&paths.audio_part).unwrap(), b"abcd");
        assert!(paths.checkpoint.is_file());
        assert!(matches!(
            transfers.download(&source, &request, &paths).await,
            Ok(NativeSourceResult::Available(()))
        ));
        assert_eq!(std::fs::read(&paths.audio_part).unwrap(), b"abcdefghij");

        server.join().expect("download server");
        let requests = requests.into_iter().collect::<Vec<_>>();
        assert!(!requests[0].to_ascii_lowercase().contains("range:"));
        assert!(
            requests[1..]
                .iter()
                .all(|request| request.to_ascii_lowercase().contains("range: bytes=4-"))
        );
        assert!(
            requests[2]
                .to_ascii_lowercase()
                .contains("if-range: wed, 21 oct 2015 07:27:00 gmt")
        );
    }

    #[tokio::test]
    async fn cancellation_settles_before_staging_cleanup() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind download server");
        let address = listener.local_addr().expect("download server address");
        let (headers_sent, headers_received) = mpsc::channel();
        let (release_body, body_released) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept download request");
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).expect("read download request");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"v1\"\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\n",
                )
                .expect("write download headers");
            stream.flush().expect("flush download headers");
            headers_sent.send(()).expect("signal download headers");
            body_released.recv().expect("release download body");
            let _ = stream.write_all(b"abcdefghij");
        });
        let source = remote_source(&format!("http://{address}"));
        let track_id = TrackId::new("jellyfin:track:one");
        let directory = tempfile::tempdir().expect("temporary downloads");
        let paths = download_paths(directory.path(), source.source_id(), &track_id);
        let task_paths = paths.clone();
        let (cancel, cancelled) = oneshot::channel();
        let transfer = tokio::spawn(async move {
            run_transfer(
                &source,
                track_id,
                DownloadQuality::Original,
                &task_paths,
                &TransferClients::default(),
                cancelled,
            )
            .await
        });
        tokio::task::spawn_blocking(move || headers_received.recv())
            .await
            .expect("wait for download headers")
            .expect("download headers");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !paths.checkpoint.is_file() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("download staging checkpoint");

        cancel.send(()).expect("cancel transfer");
        assert!(matches!(
            transfer.await.expect("join transfer"),
            Err(SourceError::Cancelled)
        ));
        discard_staging(&paths).await.expect("discard staging");
        release_body.send(()).expect("release server body");
        tokio::task::spawn_blocking(move || server.join())
            .await
            .expect("wait for download server")
            .expect("download server");

        assert!(!paths.audio_part.exists());
        assert!(!paths.checkpoint.exists());
    }

    #[tokio::test]
    async fn a_stale_range_restarts_once_without_range() {
        let (url, requests, server) = scripted_server(vec![
            b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */20\r\nConnection: close\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"v2\"\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\nabcdefghij".to_vec(),
        ]);
        let source = remote_source(&url);
        let track_id = TrackId::new("jellyfin:track:one");
        let request = StreamRequest::original(track_id.clone());
        let NativeSourceResult::Available(stream) = source
            .stream(&request)
            .await
            .expect("resolve download stream")
        else {
            panic!("remote source has no stream");
        };
        let directory = tempfile::tempdir().expect("temporary downloads");
        let paths = download_paths(directory.path(), source.source_id(), &track_id);
        tokio::fs::create_dir_all(&paths.directory)
            .await
            .expect("download directory");
        tokio::fs::write(&paths.audio_part, b"abcd")
            .await
            .expect("partial download");
        write_checkpoint(
            &paths,
            &TransferCheckpoint {
                representation: representation_key(
                    source.source_id(),
                    &request,
                    stream.redacted_uri(),
                ),
                validator: "\"v1\"".to_string(),
                length: 10,
            },
        )
        .await
        .expect("download checkpoint");

        assert!(matches!(
            TransferClients::default()
                .download(&source, &request, &paths)
                .await,
            Ok(NativeSourceResult::Available(()))
        ));
        assert_eq!(std::fs::read(&paths.audio_part).unwrap(), b"abcdefghij");
        server.join().expect("download server");
        let requests = requests.into_iter().collect::<Vec<_>>();
        assert!(requests[0].to_ascii_lowercase().contains("range: bytes=4-"));
        assert!(!requests[1].to_ascii_lowercase().contains("range:"));
    }

    #[tokio::test]
    async fn an_invalid_partial_response_does_not_change_staging() {
        let (url, requests, server) = scripted_server(vec![
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 6\r\nContent-Range: bytes 0-5/10\r\nETag: \"v1\"\r\nConnection: close\r\n\r\nXXXXXX".to_vec(),
        ]);
        let source = remote_source(&url);
        let track_id = TrackId::new("jellyfin:track:one");
        let request = StreamRequest::original(track_id.clone());
        let NativeSourceResult::Available(stream) = source
            .stream(&request)
            .await
            .expect("resolve download stream")
        else {
            panic!("remote source has no stream");
        };
        let representation =
            representation_key(source.source_id(), &request, stream.redacted_uri());
        let directory = tempfile::tempdir().expect("temporary downloads");
        let paths = download_paths(directory.path(), source.source_id(), &track_id);
        tokio::fs::create_dir_all(&paths.directory)
            .await
            .expect("download directory");
        tokio::fs::write(&paths.audio_part, b"abcd")
            .await
            .expect("partial download");
        write_checkpoint(
            &paths,
            &TransferCheckpoint {
                representation,
                validator: "\"v1\"".to_string(),
                length: 10,
            },
        )
        .await
        .expect("download checkpoint");

        assert!(matches!(
            TransferClients::default()
                .download(&source, &request, &paths,)
                .await,
            Err(SourceError::Other(_))
        ));
        assert_eq!(std::fs::read(&paths.audio_part).unwrap(), b"abcd");
        assert!(paths.checkpoint.exists());

        server.join().expect("download server");
        let requests = requests.into_iter().collect::<Vec<_>>();
        assert!(requests[0].to_ascii_lowercase().contains("range: bytes=4-"));
        assert_eq!(requests.len(), 1);
    }
}
